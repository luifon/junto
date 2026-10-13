//! From the other people to the output device: one queue per remote track, a volume per
//! person, the mix, and the conversion to the format of the device (ADR 0507, ADR 0733).
//!
//! The call library delivers each remote track in frames of 10 ms by its own clock; the
//! output device asks for samples by its clock. A short queue per track takes the
//! difference.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use livekit::webrtc::native::audio_resampler::AudioResampler;

use super::host::Format;
use super::processing::Processor;
use super::rate::{MAX_SPAN_FRAMES, Span};
use super::recent::Recent;
use super::{FRAME_SAMPLES, PLAYOUT_CHANNELS, SAMPLE_RATE, to_f32};

/// The samples of one frame of the mix.
pub const BLOCK_SAMPLES: usize = FRAME_SAMPLES * PLAYOUT_CHANNELS;
/// A track is played when this much of it waits, and again after its queue ran empty.
const START_BLOCKS: usize = 3;
/// A queue never holds more than this. What is older is dropped, so that a delay of the
/// output device does not become a lasting delay of the sound.
const MAX_BLOCKS: usize = 20;
/// The largest volume of one person: 200 %.
pub const MAX_GAIN: f32 = 2.0;

struct Source {
    id: u64,
    queue: VecDeque<i16>,
    gain: f32,
    playing: bool,
}

#[derive(Default)]
struct State {
    next: u64,
    sources: Vec<Source>,
    sound_off: bool,
    /// The frames of the mix that were made.
    mixed: u64,
}

/// The remote tracks that are played, mixed on demand.
#[derive(Default)]
pub struct Mixer {
    state: Mutex<State>,
    /// What passed the mixer lately, for the diagnosis of a live test.
    pub passed: Passed,
}

/// The frames that passed the mixer.
#[derive(Debug, Default)]
pub struct Passed {
    /// The frames that the remote tracks gave.
    pub received: Recent,
    /// The frames of the mix that held the sound of a track, and those that held none.
    pub played: Recent,
    pub silent: Recent,
    /// How often the queue of a track that was played ran empty.
    pub ran_empty: AtomicU64,
    /// How often a queue was cut because it was full.
    pub cut: AtomicU64,
}

impl Mixer {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds a track with the volume of its person: 1.0 is 100 %.
    pub fn add(self: &Arc<Self>, gain: f32) -> SourceHandle {
        let mut state = self.lock();
        let id = state.next;
        state.next += 1;
        state.sources.push(Source {
            id,
            queue: VecDeque::new(),
            gain: gain.clamp(0.0, MAX_GAIN),
            playing: false,
        });
        SourceHandle {
            mixer: self.clone(),
            id,
        }
    }

    /// The person turned the sound off, or a moderator blocked it (ADR 0503). The tracks
    /// are still taken, so that nothing old is played when the sound returns.
    pub fn set_sound_off(&self, off: bool) {
        self.lock().sound_off = off;
    }

    /// The number of frames of the mix that were made.
    pub fn mixed(&self) -> u64 {
        self.lock().mixed
    }

    /// The frames of a track that wait to be played.
    pub fn waiting(&self, source: &SourceHandle) -> usize {
        self.lock()
            .sources
            .iter()
            .find(|held| held.id == source.id)
            .map_or(0, |held| held.queue.len() / BLOCK_SAMPLES)
    }

    /// For each track: whether it is played now, and the frames that wait.
    pub fn queues(&self) -> Vec<(bool, usize)> {
        self.lock()
            .sources
            .iter()
            .map(|held| (held.playing, held.queue.len() / BLOCK_SAMPLES))
            .collect()
    }

    /// Writes the next 10 ms of the mix into `block`.
    pub fn mix(&self, block: &mut [i16]) {
        let mut sums = [0i32; BLOCK_SAMPLES];
        let mut state = self.lock();
        state.mixed += 1;
        let sound_off = state.sound_off;
        let mut sounded = false;
        for source in &mut state.sources {
            if !source.playing && source.queue.len() >= START_BLOCKS * BLOCK_SAMPLES {
                source.playing = true;
            }
            if !source.playing {
                continue;
            }
            if source.queue.len() < BLOCK_SAMPLES {
                source.playing = false;
                self.passed.ran_empty.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            sounded = true;
            for (sum, sample) in sums.iter_mut().zip(source.queue.drain(..BLOCK_SAMPLES)) {
                *sum += (f32::from(sample) * source.gain) as i32;
            }
        }
        drop(state);
        if sounded && !sound_off {
            self.passed.played.count();
        } else {
            self.passed.silent.count();
        }
        for (out, sum) in block.iter_mut().zip(sums) {
            *out = if sound_off {
                0
            } else {
                sum.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16
            };
        }
    }
}

/// One remote track in the mix. The track leaves the mix when this value is dropped.
pub struct SourceHandle {
    mixer: Arc<Mixer>,
    id: u64,
}

impl SourceHandle {
    /// Gives the next frame of the track: 10 ms, stereo, 48 kHz.
    pub fn push(&self, frame: &[i16]) {
        let mut state = self.mixer.lock();
        let Some(source) = state.sources.iter_mut().find(|held| held.id == self.id) else {
            return;
        };
        source.queue.extend(frame.iter().copied());
        self.mixer.passed.received.count();
        let surplus = source
            .queue
            .len()
            .saturating_sub(MAX_BLOCKS * BLOCK_SAMPLES);
        if surplus > 0 {
            self.mixer.passed.cut.fetch_add(1, Ordering::Relaxed);
            // Whole frames, down to the amount at which a track starts.
            let keep = START_BLOCKS * BLOCK_SAMPLES;
            let drop = (source.queue.len() - keep) / BLOCK_SAMPLES * BLOCK_SAMPLES;
            source.queue.drain(..drop);
        }
    }

    pub fn set_gain(&self, gain: f32) {
        let mut state = self.mixer.lock();
        if let Some(source) = state.sources.iter_mut().find(|held| held.id == self.id) {
            source.gain = gain.clamp(0.0, MAX_GAIN);
        }
    }
}

impl Drop for SourceHandle {
    fn drop(&mut self) {
        self.mixer
            .lock()
            .sources
            .retain(|source| source.id != self.id);
    }
}

/// The work on the thread of the output device.
pub struct PlayoutChain {
    mixer: Arc<Mixer>,
    processor: Arc<Processor>,
    resampler: AudioResampler,
    format: Option<Format>,
    /// Samples in the format of the device that the device has not asked for yet.
    ready: VecDeque<f32>,
}

impl PlayoutChain {
    pub fn new(mixer: Arc<Mixer>, processor: Arc<Processor>) -> Self {
        Self {
            mixer,
            processor,
            resampler: AudioResampler::default(),
            format: None,
            ready: VecDeque::new(),
        }
    }

    /// Fills the buffer of the device with interleaved samples. A format that the pipeline
    /// does not convert leaves the buffer as it is: silence.
    pub fn fill(&mut self, out: &mut [f32], format: Format) {
        let channels = usize::from(format.channels);
        let Some(span) = Span::of(format.sample_rate).filter(|_| channels > 0) else {
            return;
        };
        if self.format != Some(format) {
            self.format = Some(format);
            self.ready.clear();
        }
        let mut blocks = [0i16; BLOCK_SAMPLES * MAX_SPAN_FRAMES];
        let blocks = &mut blocks[..BLOCK_SAMPLES * span.frames];
        while self.ready.len() < out.len() {
            for block in blocks.as_chunks_mut::<BLOCK_SAMPLES>().0 {
                self.mixer.mix(block);
                // The mix is what the microphone can pick up again from the loudspeakers.
                self.processor.render(block);
            }
            let carried = channels.min(PLAYOUT_CHANNELS);
            let converted: &[i16] =
                if format.sample_rate == SAMPLE_RATE && carried == PLAYOUT_CHANNELS {
                    blocks
                } else {
                    self.resampler.remix_and_resample(
                        blocks,
                        span.pipeline_samples() as u32,
                        PLAYOUT_CHANNELS as u32,
                        span.pipeline_rate(),
                        carried as u32,
                        span.device_rate(),
                    )
                };
            for frame in converted.chunks_exact(carried) {
                self.ready
                    .extend(frame.iter().map(|sample| to_f32(*sample)));
                // A device with more channels plays the mix on its first two.
                self.ready
                    .extend(std::iter::repeat_n(0.0, channels - carried));
            }
        }
        let wanted = out.len();
        for (sample, ready) in out.iter_mut().zip(self.ready.drain(..wanted)) {
            *sample = ready;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Processing;

    fn block(value: i16) -> Vec<i16> {
        vec![value; BLOCK_SAMPLES]
    }

    fn mixed(mixer: &Mixer) -> Vec<i16> {
        let mut out = vec![7i16; BLOCK_SAMPLES];
        mixer.mix(&mut out);
        out
    }

    #[test]
    fn a_track_plays_after_its_first_frames_and_two_tracks_are_summed_with_their_volumes() {
        let mixer = Mixer::shared();
        let (ana, ben) = (mixer.add(1.0), mixer.add(0.5));
        ana.push(&block(1000));
        ana.push(&block(1000));
        assert!(mixed(&mixer).iter().all(|sample| *sample == 0));
        ana.push(&block(1000));
        for _ in 0..START_BLOCKS {
            ben.push(&block(4000));
        }
        assert!(mixed(&mixer).iter().all(|sample| *sample == 3000));

        ben.set_gain(2.0);
        assert!(mixed(&mixer).iter().all(|sample| *sample == 9000));
        // A volume above 200 % is not taken.
        ben.set_gain(9.0);
        assert!(mixed(&mixer).iter().all(|sample| *sample == 9000));

        // Both queues are empty: silence, and a track waits for its first frames again.
        assert!(mixed(&mixer).iter().all(|sample| *sample == 0));
        ana.push(&block(1000));
        assert!(mixed(&mixer).iter().all(|sample| *sample == 0));
    }

    #[test]
    fn a_sum_above_full_scale_is_limited_and_does_not_wrap() {
        let mixer = Mixer::shared();
        let (ana, ben) = (mixer.add(2.0), mixer.add(2.0));
        for _ in 0..START_BLOCKS {
            ana.push(&block(30_000));
            ben.push(&block(30_000));
        }
        assert!(mixed(&mixer).iter().all(|sample| *sample == i16::MAX));
        for _ in 0..START_BLOCKS {
            ana.push(&block(-30_000));
            ben.push(&block(-30_000));
        }
        mixed(&mixer);
        mixed(&mixer);
        assert!(mixed(&mixer).iter().all(|sample| *sample == i16::MIN));
    }

    #[test]
    fn sound_off_plays_silence_and_keeps_no_old_sound() {
        let mixer = Mixer::shared();
        let ana = mixer.add(1.0);
        for _ in 0..START_BLOCKS + 1 {
            ana.push(&block(1000));
        }
        mixer.set_sound_off(true);
        for _ in 0..START_BLOCKS {
            assert!(mixed(&mixer).iter().all(|sample| *sample == 0));
        }
        mixer.set_sound_off(false);
        assert!(mixed(&mixer).iter().all(|sample| *sample == 1000));
        assert!(mixed(&mixer).iter().all(|sample| *sample == 0));
    }

    #[test]
    fn a_queue_that_is_not_played_is_cut_to_its_start_amount() {
        let mixer = Mixer::shared();
        let ana = mixer.add(1.0);
        for index in 0..=MAX_BLOCKS {
            ana.push(&block(index as i16));
        }
        // The newest frames stay: the last START_BLOCKS of the 21 that were given.
        for expected in MAX_BLOCKS + 1 - START_BLOCKS..=MAX_BLOCKS {
            assert!(
                mixed(&mixer)
                    .iter()
                    .all(|sample| *sample == expected as i16)
            );
        }
        assert!(mixed(&mixer).iter().all(|sample| *sample == 0));
    }

    #[test]
    fn a_track_leaves_the_mix_when_its_handle_is_dropped() {
        let mixer = Mixer::shared();
        let ana = mixer.add(1.0);
        for _ in 0..START_BLOCKS {
            ana.push(&block(1000));
        }
        drop(ana);
        assert!(mixed(&mixer).iter().all(|sample| *sample == 0));
    }

    #[test]
    fn the_device_gets_its_own_rate_and_channel_count() {
        let mixer = Mixer::shared();
        let ana = mixer.add(1.0);
        let processor = Arc::new(Processor::new(Processing::OFF));
        let mut chain = PlayoutChain::new(mixer, processor);
        for _ in 0..MAX_BLOCKS {
            ana.push(&block(8000));
        }
        // The device asks for buffers that are no multiple of 10 ms.
        let stereo = Format {
            sample_rate: SAMPLE_RATE,
            channels: 2,
        };
        let mut out = vec![0.0f32; 512 * 2];
        chain.fill(&mut out, stereo);
        let expected = to_f32(8000);
        assert!(out.iter().all(|sample| (*sample - expected).abs() < 0.001));

        // Four channels: the mix is on the first two.
        let four = Format {
            sample_rate: SAMPLE_RATE,
            channels: 4,
        };
        let mut out = vec![1.0f32; 480 * 4];
        chain.fill(&mut out, four);
        for frame in out.as_chunks::<4>().0 {
            assert!((frame[0] - expected).abs() < 0.001 && (frame[1] - expected).abs() < 0.001);
            assert_eq!((frame[2], frame[3]), (0.0, 0.0));
        }

        // Mono at 44.1 kHz: a steady value stays a steady value after the conversion.
        let mono = Format {
            sample_rate: 44_100,
            channels: 1,
        };
        let mut out = vec![0.0f32; 441 * 4];
        chain.fill(&mut out, mono);
        let settled = &out[441..];
        assert!(
            settled
                .iter()
                .all(|sample| (*sample - expected).abs() < 0.02),
            "{:?}",
            &settled[..8]
        );
    }

    #[test]
    fn a_device_rate_that_10_ms_do_not_divide_takes_the_mix_at_the_time_of_the_device() {
        for (sample_rate, buffer) in [(22_050, 2205), (11_025, 2205)] {
            let mixer = Mixer::shared();
            let ana = mixer.add(1.0);
            let processor = Arc::new(Processor::new(Processing::OFF));
            let mut chain = PlayoutChain::new(mixer.clone(), processor);
            let format = Format {
                sample_rate,
                channels: 1,
            };
            // Ten seconds of the device take ten seconds of the mix: 1000 frames. A device
            // that takes the mix faster than it is made empties the queues.
            let fills = 10 * sample_rate as usize / buffer;
            let mut out = vec![0.0f32; buffer];
            let mut last = Vec::new();
            for _ in 0..fills {
                while mixer.waiting(&ana) < MAX_BLOCKS {
                    ana.push(&block(8000));
                }
                chain.fill(&mut out, format);
                last.clone_from(&out);
            }
            assert_eq!(mixer.mixed(), 1000, "{sample_rate}");
            // A steady value stays a steady value after the conversion.
            let expected = to_f32(8000);
            assert!(
                last.iter().all(|sample| (*sample - expected).abs() < 0.02),
                "{sample_rate}: {:?}",
                &last[..8]
            );
        }

        // A rate that the pipeline does not convert is played as silence.
        let mixer = Mixer::shared();
        let processor = Arc::new(Processor::new(Processing::OFF));
        let mut chain = PlayoutChain::new(mixer.clone(), processor);
        let odd = Format {
            sample_rate: 384_000,
            channels: 2,
        };
        let mut out = vec![0.0f32; 7680];
        chain.fill(&mut out, odd);
        assert!(out.iter().all(|sample| *sample == 0.0));
        assert_eq!(mixer.mixed(), 0);
    }
}
