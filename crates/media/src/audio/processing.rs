//! Echo cancellation and gain control: the audio processing module of the call library,
//! shared by the microphone side and the playout side (ADR 0701, ADR 0733), and noise
//! suppression by the suppressor of `denoise` after it. Sending follows the voice settings.
//! A separate echo filter receives copies of both sides for the muted-speech notice.

use std::sync::{Mutex, PoisonError};

use livekit::webrtc::native::apm::AudioProcessingModule;

use super::denoise::Denoiser;
use super::{FRAME_SAMPLES, PLAYOUT_CHANNELS, SAMPLE_RATE};

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
    /// The noise suppressor, while noise suppression is on.
    denoiser: Option<Box<Denoiser>>,
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
                // The suppressor of `denoise` replaces the one of the module (ADR 0733).
                false,
            )
        });
        Self {
            settings,
            apm,
            denoiser: settings.noise_suppression.then(Box::default),
            failed: false,
        }
    }
}

pub struct Processor {
    module: Mutex<Module>,
    speech: Mutex<Module>,
}

impl Processor {
    pub fn new(settings: Processing) -> Self {
        Self {
            module: Mutex::new(Module::new(settings)),
            speech: Mutex::new(Module::new(Processing {
                echo_cancellation: true,
                noise_suppression: false,
                automatic_gain: false,
            })),
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

    /// Processes one mono frame of 10 ms of the microphone in place: the echo canceller, the
    /// high-pass filter and the gain control of the module, then the noise suppressor.
    pub fn capture(&self, frame: &mut [i16]) {
        let mut module = self.lock();
        let Module {
            apm,
            denoiser,
            failed,
            ..
        } = &mut *module;
        if let Some(apm) = apm
            && let Err(error) = apm.process_stream(frame, SAMPLE_RATE as i32, 1)
            && !*failed
        {
            *failed = true;
            tracing::warn!(error = %error.message, "the audio processing of the microphone failed");
        }
        if let (Some(denoiser), Ok(frame)) =
            (denoiser, <&mut [i16; FRAME_SAMPLES]>::try_from(frame))
        {
            denoiser.process(frame);
        }
    }

    /// Filters a copy of the local input for the muted-speech notice, independently of
    /// the sending settings and without opening the sending gate.
    pub fn speech(&self, frame: &mut [i16; FRAME_SAMPLES]) {
        let mut module = self.speech.lock().unwrap_or_else(PoisonError::into_inner);
        let Module { apm, failed, .. } = &mut *module;
        if let Some(apm) = apm
            && let Err(error) = apm
                .set_stream_delay_ms(0)
                .and_then(|()| apm.process_stream(frame, SAMPLE_RATE as i32, 1))
        {
            if !*failed {
                tracing::warn!(error = %error.message, "the echo filter for muted speech failed");
            }
            *failed = true;
            frame.fill(0);
        }
    }

    /// Takes one frame of 10 ms of what is played, as the reference of the echo canceller.
    pub fn render(&self, frame: &mut [i16]) {
        if let Ok(mut reference) = <[i16; FRAME_SAMPLES * PLAYOUT_CHANNELS]>::try_from(&*frame) {
            let mut module = self.speech.lock().unwrap_or_else(PoisonError::into_inner);
            let Module { apm, failed, .. } = &mut *module;
            if let Some(apm) = apm
                && let Err(error) = apm.process_reverse_stream(
                    &mut reference,
                    SAMPLE_RATE as i32,
                    PLAYOUT_CHANNELS as i32,
                )
            {
                if !*failed {
                    tracing::warn!(error = %error.message, "the playback reference for muted speech failed");
                }
                *failed = true;
            }
        }
        let mut module = self.lock();
        let Module {
            apm,
            failed,
            settings,
            ..
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

    #[cfg(test)]
    pub(crate) fn hold_for_test(&self) -> impl Sized + '_ {
        self.lock()
    }

    #[cfg(test)]
    pub(crate) fn hold_speech_for_test(&self) -> impl Sized + '_ {
        self.speech.lock().unwrap_or_else(PoisonError::into_inner)
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
    fn the_muted_speech_filter_keeps_local_input_with_sending_processing_off() {
        let processor = Processor::new(Processing::OFF);
        let mut loudest = -96.0_f32;
        for index in 0..200 {
            processor.render(&mut [0; FRAME_SAMPLES * PLAYOUT_CHANNELS]);
            let mut frame: [i16; FRAME_SAMPLES] = tone(index, 1700.0, 0.15).try_into().unwrap();
            processor.speech(&mut frame);
            loudest = loudest.max(dbfs(&frame));
        }
        assert!(
            !processor.speech.lock().unwrap().failed,
            "the detector failed"
        );
        assert!(loudest > -40.0, "local input was removed: {loudest}");
    }

    #[test]
    fn the_sending_echo_canceller_removes_playback_with_a_known_delay() {
        for enabled in [false, true] {
            let processor = Processor::new(Processing {
                echo_cancellation: enabled,
                noise_suppression: false,
                automatic_gain: false,
            });
            let mut delayed = std::collections::VecDeque::from(vec![[0i16; FRAME_SAMPLES]; 5]);
            let mut seed = 0x1234_5678u32;
            let (mut raw, mut sent) = (0.0_f64, 0.0_f64);
            for index in 0..600 {
                let mut current = [0i16; FRAME_SAMPLES];
                let mut played = [0i16; FRAME_SAMPLES * PLAYOUT_CHANNELS];
                for (sample, stereo) in current
                    .iter_mut()
                    .zip(played.as_chunks_mut::<PLAYOUT_CHANNELS>().0)
                {
                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    *sample = (((seed >> 16) as i32 - 32_768) / 4) as i16;
                    stereo.fill(*sample);
                }
                processor.render(&mut played);
                let mut captured = delayed.pop_front().unwrap();
                delayed.push_back(current);
                for sample in &mut captured {
                    *sample /= 2;
                }
                let before = dbfs(&captured);
                let mut detection = captured;
                processor.speech(&mut detection);
                processor.capture(&mut captured);
                if index >= 400 {
                    raw += 10.0_f64.powf(f64::from(before) / 10.0);
                    sent += 10.0_f64.powf(f64::from(dbfs(&captured)) / 10.0);
                }
            }
            let change = 10.0 * (sent / raw).log10();
            eprintln!(
                "sending echo cancellation {enabled}, 50 ms delay, change {change:.2} dB, processing error {}",
                processor.lock().failed
            );
            if enabled {
                assert!(
                    change < -25.0,
                    "delayed playback remains in the sent microphone: {change:.2} dB"
                );
            } else {
                assert!(
                    change.abs() < 0.1,
                    "disabled processing changed the sent samples: {change:.2} dB"
                );
            }
        }
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
        // A steady tone is noise for the noise suppressor (`denoise`), so it is off here.
        let settings = Processing {
            noise_suppression: false,
            ..Processing::default()
        };
        let processor = Processor::new(settings);
        let mut last = Vec::new();
        for index in 0..200 {
            let mut played = vec![0i16; FRAME_SAMPLES * PLAYOUT_CHANNELS];
            processor.render(&mut played);
            let mut frame = tone(index, 440.0, 0.25);
            processor.capture(&mut frame);
            last = frame;
        }
        // Gain control changes the level and does not remove a signal.
        assert!(dbfs(&last) > -40.0, "{}", dbfs(&last));
        assert_eq!(processor.settings(), settings);

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
