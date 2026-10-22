//! Echo cancellation, noise suppression and gain control: the audio processing module of
//! the call library, shared by the microphone side and the playout side (ADR 0701,
//! ADR 0733). The echo canceller removes from the microphone what the playout side played,
//! so both sides give their frames to one module.

use std::sync::{Mutex, PoisonError};

use livekit::webrtc::native::apm::AudioProcessingModule;

use super::{PLAYOUT_CHANNELS, SAMPLE_RATE};

/// The processing of the microphone that a person can turn off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Processing {
    pub echo_cancellation: bool,
    pub noise_suppression: bool,
    pub automatic_gain: bool,
}

impl Default for Processing {
    fn default() -> Self {
        Self {
            echo_cancellation: true,
            noise_suppression: true,
            automatic_gain: true,
        }
    }
}

impl Processing {
    pub const OFF: Self = Self {
        echo_cancellation: false,
        noise_suppression: false,
        automatic_gain: false,
    };

    const fn any(self) -> bool {
        self.echo_cancellation || self.noise_suppression || self.automatic_gain
    }
}

struct Module {
    settings: Processing,
    /// Absent while every part is off: the frames pass unchanged.
    apm: Option<AudioProcessingModule>,
    /// A failure of the module is logged once per module.
    failed: bool,
}

impl Module {
    fn new(settings: Processing) -> Self {
        let apm = settings.any().then(|| {
            AudioProcessingModule::new(
                settings.echo_cancellation,
                settings.automatic_gain,
                // The high-pass filter removes the hum below speech. The module applies
                // it together with its other parts.
                true,
                settings.noise_suppression,
            )
        });
        Self {
            settings,
            apm,
            failed: false,
        }
    }
}

pub struct Processor {
    module: Mutex<Module>,
}

impl Processor {
    pub fn new(settings: Processing) -> Self {
        Self {
            module: Mutex::new(Module::new(settings)),
        }
    }

    pub fn settings(&self) -> Processing {
        self.lock().settings
    }

    /// Replaces the module when a setting changed. The echo canceller starts again without
    /// its history.
    pub fn set(&self, settings: Processing) {
        let mut module = self.lock();
        if module.settings != settings {
            *module = Module::new(settings);
        }
    }

    /// Processes one mono frame of 10 ms of the microphone in place.
    pub fn capture(&self, frame: &mut [i16]) {
        let mut module = self.lock();
        let Module { apm, failed, .. } = &mut *module;
        if let Some(apm) = apm
            && let Err(error) = apm.process_stream(frame, SAMPLE_RATE as i32, 1)
            && !*failed
        {
            *failed = true;
            tracing::warn!(error = %error.message, "the audio processing of the microphone failed");
        }
    }

    /// Takes one frame of 10 ms of what is played, as the reference of the echo canceller.
    pub fn render(&self, frame: &mut [i16]) {
        let mut module = self.lock();
        let Module {
            apm,
            failed,
            settings,
        } = &mut *module;
        if !settings.echo_cancellation {
            return;
        }
        if let Some(apm) = apm
            && let Err(error) =
                apm.process_reverse_stream(frame, SAMPLE_RATE as i32, PLAYOUT_CHANNELS as i32)
            && !*failed
        {
            *failed = true;
            tracing::warn!(error = %error.message, "the audio processing of the playout failed");
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Module> {
        self.module.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::FRAME_SAMPLES;
    use crate::audio::level::dbfs;

    fn tone(frame: usize, hz: f32, amplitude: f32) -> Vec<i16> {
        (0..FRAME_SAMPLES)
            .map(|index| {
                let position = (frame * FRAME_SAMPLES + index) as f32;
                let phase = std::f32::consts::TAU * hz * position / SAMPLE_RATE as f32;
                (phase.sin() * amplitude * f32::from(i16::MAX)) as i16
            })
            .collect()
    }

    #[test]
    fn frames_pass_unchanged_while_every_part_is_off() {
        let processor = Processor::new(Processing::OFF);
        let mut frame = tone(0, 440.0, 0.25);
        let before = frame.clone();
        processor.capture(&mut frame);
        assert_eq!(frame, before);
        let mut played = vec![1000i16; FRAME_SAMPLES * PLAYOUT_CHANNELS];
        processor.render(&mut played);
        assert!(played.iter().all(|sample| *sample == 1000));
    }

    #[test]
    fn the_module_takes_frames_of_both_sides_and_keeps_a_voice_band_tone() {
        let processor = Processor::new(Processing::default());
        let mut last = Vec::new();
        for index in 0..200 {
            let mut played = vec![0i16; FRAME_SAMPLES * PLAYOUT_CHANNELS];
            processor.render(&mut played);
            let mut frame = tone(index, 440.0, 0.25);
            processor.capture(&mut frame);
            last = frame;
        }
        // Gain control and noise suppression change the level and do not remove a signal.
        assert!(dbfs(&last) > -40.0, "{}", dbfs(&last));
        assert_eq!(processor.settings(), Processing::default());

        processor.set(Processing::OFF);
        let mut frame = tone(0, 440.0, 0.25);
        let before = frame.clone();
        processor.capture(&mut frame);
        assert_eq!(frame, before);
    }

    /// The power of one frequency in a frame (the Goertzel filter).
    fn power(frame: &[i16], hz: f32) -> f64 {
        let coefficient =
            2.0 * (std::f64::consts::TAU * f64::from(hz) / f64::from(SAMPLE_RATE)).cos();
        let (mut one, mut two) = (0.0, 0.0);
        for sample in frame {
            let next = f64::from(*sample) + coefficient * one - two;
            two = one;
            one = next;
        }
        one * one + two * two - coefficient * one * two
    }

    /// A module that works at 32 kHz inside resamples the microphone and loses everything
    /// above 16 kHz. The microphone and the pipeline are at 48 kHz (ADR 0733).
    #[test]
    fn the_module_keeps_the_microphone_above_16_khz() {
        let processor = Processor::new(Processing {
            echo_cancellation: true,
            noise_suppression: false,
            automatic_gain: false,
        });
        let (mut sent, mut kept) = (0.0, 0.0);
        for index in 0..300 {
            let mut played = vec![0i16; FRAME_SAMPLES * PLAYOUT_CHANNELS];
            processor.render(&mut played);
            let mut frame = tone(index, 18_000.0, 0.25);
            let before = frame.clone();
            processor.capture(&mut frame);
            if index >= 200 {
                sent += power(&before, 18_000.0);
                kept += power(&frame, 18_000.0);
            }
        }
        let change = 10.0 * (kept / sent).log10();
        assert!(
            change > -6.0,
            "the tone of 18 kHz changed by {change:.1} dB"
        );
    }
}
