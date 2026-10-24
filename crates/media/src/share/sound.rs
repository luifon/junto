//! The sound of a share on its way to the media connection (ADR 0502, ADR 0737).
//!
//! A capture gives interleaved samples at its own rate, in blocks of its own size, which
//! can change during a capture. The track of the share takes stereo frames of 10 ms at
//! 48 kHz. No processing is applied: noise suppression and echo cancellation damage sound
//! that holds several voices or music.

use livekit::webrtc::native::audio_resampler::AudioResampler;
use tokio::sync::mpsc;

use crate::audio::rate::{Span, whole_step};
use crate::audio::{FRAME_SAMPLES, Format, SAMPLE_RATE, to_i16};

/// The channels of the track of a share.
pub const SHARE_CHANNELS: usize = 2;
/// The samples of one frame of the track.
pub const SHARE_FRAME_SAMPLES: usize = FRAME_SAMPLES * SHARE_CHANNELS;
/// The frames that wait when the media connection takes them late: half a second. A
/// frame that finds the queue full is dropped.
pub const WAITING_FRAMES: usize = 50;

/// The work on the thread of the capture.
pub struct SoundChain {
    frames: mpsc::Sender<Vec<i16>>,
    resampler: AudioResampler,
    format: Option<Format>,
    /// Stereo samples at the rate of the capture that do not fill a step yet.
    pending: Vec<i16>,
    dropped: u64,
}

impl SoundChain {
    /// `frames` takes each frame of 10 ms: 480 samples of two channels, interleaved.
    pub fn new(frames: mpsc::Sender<Vec<i16>>) -> Self {
        Self {
            frames,
            resampler: AudioResampler::default(),
            format: None,
            pending: Vec::new(),
            dropped: 0,
        }
    }

    /// The frames that were dropped because the media connection did not take them.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Takes interleaved samples of the capture. A format that the pipeline does not
    /// convert gives no frame.
    pub fn deliver(&mut self, samples: &[f32], format: Format) {
        let channels = usize::from(format.channels);
        let Some(span) = Span::of(format.sample_rate).filter(|_| channels > 0) else {
            return;
        };
        if self.format != Some(format) {
            self.format = Some(format);
            self.pending.clear();
        }
        // One channel is sent on both; of more than two, the first two are the front pair.
        for frame in samples.chunks_exact(channels) {
            let left = to_i16(frame[0]);
            let right = if channels == 1 {
                left
            } else {
                to_i16(frame[1])
            };
            self.pending.extend([left, right]);
        }
        let step = span.device_samples * SHARE_CHANNELS;
        let mut taken = 0;
        while self.pending.len() - taken >= step {
            let part = &self.pending[taken..taken + step];
            taken += step;
            if format.sample_rate == SAMPLE_RATE {
                self.send(part.to_vec());
                continue;
            }
            let resampled = self.resampler.remix_and_resample(
                part,
                span.device_samples as u32,
                SHARE_CHANNELS as u32,
                span.device_rate(),
                SHARE_CHANNELS as u32,
                span.pipeline_rate(),
            );
            let mut converted = Vec::new();
            whole_step(
                &mut converted,
                resampled,
                span.pipeline_samples() * SHARE_CHANNELS,
            );
            for frame in converted.as_chunks::<SHARE_FRAME_SAMPLES>().0 {
                self.send(frame.to_vec());
            }
        }
        self.pending.drain(..taken);
    }

    fn send(&mut self, frame: Vec<i16>) {
        if self.frames.try_send(frame).is_err() {
            self.dropped += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chain() -> (SoundChain, mpsc::Receiver<Vec<i16>>) {
        let (frames, taken) = mpsc::channel(WAITING_FRAMES);
        (SoundChain::new(frames), taken)
    }

    fn drain(taken: &mut mpsc::Receiver<Vec<i16>>) -> Vec<Vec<i16>> {
        let mut frames = Vec::new();
        while let Ok(frame) = taken.try_recv() {
            frames.push(frame);
        }
        frames
    }

    /// A sine of amplitude 0.5 on the left channel and silence on the right one.
    fn left_tone(sample_rate: u32, channels: u16, samples: usize, hz: f32) -> Vec<f32> {
        (0..samples)
            .flat_map(|index| {
                let phase = index as f32 * hz / sample_rate as f32 * std::f32::consts::TAU;
                let mut frame = vec![0.0; usize::from(channels)];
                frame[0] = 0.5 * phase.sin();
                frame
            })
            .collect()
    }

    fn peak(frames: &[Vec<i16>], channel: usize) -> i16 {
        frames
            .iter()
            .flat_map(|frame| frame.as_chunks::<SHARE_CHANNELS>().0)
            .map(|pair| pair[channel].saturating_abs())
            .max()
            .unwrap_or(0)
    }

    #[test]
    fn stereo_at_the_rate_of_the_pipeline_passes_in_frames_of_10_ms_whatever_the_blocks() {
        let (mut chain, mut taken) = chain();
        let format = Format {
            sample_rate: 48_000,
            channels: 2,
        };
        let sound = left_tone(48_000, 2, 4_800, 1_000.0);
        // Blocks of 20 ms, as one system gives them, then blocks of other sizes.
        let mut at = 0;
        for block in [960, 960, 256, 2048, 1, 575] {
            chain.deliver(&sound[at * 2..(at + block) * 2], format);
            at += block;
        }
        assert_eq!(at, 4_800);
        let frames = drain(&mut taken);
        assert_eq!(frames.len(), 10);
        assert!(
            frames
                .iter()
                .all(|frame| frame.len() == SHARE_FRAME_SAMPLES)
        );
        // Nothing is lost between the blocks, and the channels stay apart.
        let sent: Vec<i16> = frames.concat();
        let expected: Vec<i16> = sound.iter().map(|sample| to_i16(*sample)).collect();
        assert_eq!(sent, expected);
        assert!(peak(&frames, 0) > 16_000);
        assert_eq!(peak(&frames, 1), 0);
        assert_eq!(chain.dropped(), 0);
    }

    #[test]
    fn one_channel_is_sent_on_both_and_another_rate_is_converted() {
        let (mut chain, mut taken) = chain();
        let format = Format {
            sample_rate: 44_100,
            channels: 1,
        };
        chain.deliver(&left_tone(44_100, 1, 44_100 / 10, 440.0), format);
        let frames = drain(&mut taken);
        assert_eq!(frames.len(), 10);
        assert!(
            frames
                .iter()
                .all(|frame| frame.len() == SHARE_FRAME_SAMPLES)
        );
        assert!(peak(&frames[2..], 0) > 14_000);
        assert!(
            frames
                .iter()
                .flat_map(|frame| frame.as_chunks::<2>().0)
                .all(|pair| pair[0] == pair[1])
        );
        // A format that the pipeline does not convert gives nothing.
        let odd = Format {
            sample_rate: 7_000,
            channels: 2,
        };
        chain.deliver(&[0.5; 1_400], odd);
        chain.deliver(
            &[0.5; 16],
            Format {
                sample_rate: 48_000,
                channels: 0,
            },
        );
        assert!(drain(&mut taken).is_empty());
    }

    #[test]
    fn a_frame_that_the_connection_does_not_take_is_dropped_and_counted() {
        let (frames, mut taken) = mpsc::channel(2);
        let mut chain = SoundChain::new(frames);
        let format = Format {
            sample_rate: 48_000,
            channels: 2,
        };
        chain.deliver(&vec![0.25; 480 * 2 * 5], format);
        assert_eq!(chain.dropped(), 3);
        assert_eq!(drain(&mut taken).len(), 2);
        chain.deliver(&vec![0.25; 480 * 2], format);
        assert_eq!(drain(&mut taken).len(), 1);
    }
}
