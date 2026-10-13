//! From the microphone to the media connection: the samples of the input device become
//! mono frames of 10 ms at 48 kHz, pass the audio processing, are measured, and are sent
//! or replaced by silence (ADR 0733).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use livekit::webrtc::native::audio_resampler::AudioResampler;
use tokio::sync::mpsc;

use super::host::Format;
use super::level::{LevelCell, Speaking};
use super::processing::Processor;
use super::rate::Span;
use super::recent::Recent;
use super::{FRAME_SAMPLES, SAMPLE_RATE, to_i16};

/// What the engine and the thread of the input device share.
#[derive(Debug, Default)]
pub struct CaptureLink {
    /// The frames of the microphone are sent. While this is false, silence is sent: the
    /// microphone is muted, or push-to-talk waits for its key.
    open: AtomicBool,
    /// How often the gate closed.
    closures: AtomicU64,
    /// The level of the microphone after the processing, also while nothing is sent. A
    /// person checks a microphone with it.
    pub input: LevelCell,
    /// The level of what is sent.
    pub sent: LevelCell,
    speaking: AtomicBool,
    dropped: AtomicU64,
    /// The frames that the thread of the device made.
    pub captured: Recent,
    /// The frames that the media connection took.
    pub taken: Taken,
}

/// The frames that the media connection took: with the sound of the microphone, and as
/// silence while the gate was closed.
#[derive(Debug, Default)]
pub struct Taken {
    pub sound: Recent,
    pub silence: Recent,
}

impl CaptureLink {
    pub fn set_open(&self, open: bool) {
        if self.open.swap(open, Ordering::Relaxed) && !open {
            self.closures.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn closures(&self) -> u64 {
        self.closures.load(Ordering::Relaxed)
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }

    /// The person speaks: the level of what is sent is above the threshold, or was a
    /// moment ago (ADR 0513).
    pub fn speaking(&self) -> bool {
        self.speaking.load(Ordering::Relaxed)
    }

    /// The frames that were not sent because the media connection did not take them in
    /// time.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// The device stopped: the levels show nothing.
    pub fn closed(&self) {
        self.input.silence();
        self.sent.silence();
        self.speaking.store(false, Ordering::Relaxed);
    }
}

/// One frame of the microphone: 480 mono samples at 48 kHz.
#[derive(Debug)]
pub struct Frame {
    samples: Vec<i16>,
    /// How often the gate had closed when the frame was made.
    closures: u64,
}

/// The frames of the microphone on their way from the thread of the device to the media
/// connection.
pub struct FrameQueue {
    link: Arc<CaptureLink>,
    frames: mpsc::Receiver<Frame>,
    /// The frame that `next` gave last was silence.
    silent: bool,
}

impl FrameQueue {
    pub fn new(link: Arc<CaptureLink>, frames: mpsc::Receiver<Frame>) -> Self {
        Self {
            link,
            frames,
            silent: false,
        }
    }

    /// The media connection took the frame that `next` gave last.
    pub fn taken(&self) {
        if self.silent {
            self.link.taken.silence.count();
        } else {
            self.link.taken.sound.count();
        }
    }

    /// The next frame for the media connection. A frame that waited is sent as silence
    /// when the gate is closed now, or closed at any time since the frame was made: sound
    /// from before the person released the key or closed the microphone is never sent
    /// after it.
    pub async fn next(&mut self) -> Option<Vec<i16>> {
        let Frame {
            mut samples,
            closures,
        } = self.frames.recv().await?;
        self.silent = !self.link.is_open() || closures != self.link.closures();
        if self.silent {
            samples.fill(0);
        }
        Some(samples)
    }
}

/// The work on the thread of the input device.
pub struct CaptureChain {
    link: Arc<CaptureLink>,
    processor: Arc<Processor>,
    frames: mpsc::Sender<Frame>,
    resampler: AudioResampler,
    format: Option<Format>,
    /// Mono samples at the rate of the device that do not fill a step yet.
    pending: Vec<i16>,
    /// The frames of one step at the rate of the pipeline.
    converted: Vec<i16>,
    speaking: Speaking,
}

impl CaptureChain {
    /// `frames` takes each frame of 10 ms: 480 mono samples at 48 kHz.
    pub fn new(
        link: Arc<CaptureLink>,
        processor: Arc<Processor>,
        frames: mpsc::Sender<Frame>,
    ) -> Self {
        Self {
            link,
            processor,
            frames,
            resampler: AudioResampler::default(),
            format: None,
            pending: Vec::new(),
            converted: Vec::new(),
            speaking: Speaking::default(),
        }
    }

    /// Takes interleaved samples of the device. A format that the pipeline does not
    /// convert gives no frame; a host opens no stream with such a format.
    pub fn deliver(&mut self, samples: &[f32], format: Format) {
        let channels = usize::from(format.channels);
        let Some(span) = Span::of(format.sample_rate).filter(|_| channels > 0) else {
            return;
        };
        if self.format != Some(format) {
            self.format = Some(format);
            self.pending.clear();
        }
        // The first two channels carry the microphone on every usual device. A device with
        // more channels has other inputs on the others.
        self.pending
            .extend(samples.chunks_exact(channels).map(|frame| {
                if channels == 1 {
                    to_i16(frame[0])
                } else {
                    to_i16((frame[0] + frame[1]) / 2.0)
                }
            }));
        let step = span.device_samples;
        let mut taken = 0;
        while self.pending.len() - taken >= step {
            let part = &self.pending[taken..taken + step];
            self.converted.clear();
            if format.sample_rate == SAMPLE_RATE {
                self.converted.extend_from_slice(part);
            } else {
                self.converted
                    .extend_from_slice(self.resampler.remix_and_resample(
                        part,
                        step as u32,
                        1,
                        span.device_rate(),
                        1,
                        span.pipeline_rate(),
                    ));
            }
            taken += step;
            self.converted.resize(span.pipeline_samples(), 0);
            for index in 0..span.frames {
                let from = index * FRAME_SAMPLES;
                let frame = self.converted[from..from + FRAME_SAMPLES].to_vec();
                self.send(frame);
            }
        }
        self.pending.drain(..taken);
    }

    fn send(&mut self, mut frame: Vec<i16>) {
        self.link.captured.count();
        self.processor.capture(&mut frame);
        self.link.input.frame(&frame);
        // Read before the gate: a closure after this line makes the frame an old one.
        let closures = self.link.closures();
        if self.link.is_open() {
            let level = self.link.sent.frame(&frame);
            self.link
                .speaking
                .store(self.speaking.frame(level), Ordering::Relaxed);
        } else {
            frame.fill(0);
            self.link.sent.silence();
            self.speaking.reset();
            self.link.speaking.store(false, Ordering::Relaxed);
        }
        let frame = Frame {
            samples: frame,
            closures,
        };
        if self.frames.try_send(frame).is_err() {
            self.link.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::Processing;
    use crate::audio::level::SILENCE_DBFS;

    fn chain() -> (Arc<CaptureLink>, CaptureChain, mpsc::Receiver<Frame>) {
        let link = Arc::new(CaptureLink::default());
        let (frames, taken) = mpsc::channel(1000);
        let processor = Arc::new(Processor::new(Processing::OFF));
        (
            link.clone(),
            CaptureChain::new(link, processor, frames),
            taken,
        )
    }

    fn tone(format: Format, hz: f32, frames: usize, from: usize) -> Vec<f32> {
        let mut samples = Vec::with_capacity(frames * usize::from(format.channels));
        for index in from..from + frames {
            let phase = std::f32::consts::TAU * hz * index as f32 / format.sample_rate as f32;
            for _ in 0..format.channels {
                samples.push(phase.sin() * 0.25);
            }
        }
        samples
    }

    fn drain(taken: &mut mpsc::Receiver<Frame>) -> Vec<Vec<i16>> {
        let mut frames = Vec::new();
        while let Ok(frame) = taken.try_recv() {
            frames.push(frame.samples);
        }
        frames
    }

    /// The frequency of a tone, from the number of times it passes zero upwards.
    fn frequency(samples: &[i16]) -> f32 {
        let rises = samples
            .windows(2)
            .filter(|pair| pair[0] < 0 && pair[1] >= 0)
            .count();
        rises as f32 * SAMPLE_RATE as f32 / samples.len() as f32
    }

    #[test]
    fn a_stereo_device_at_another_rate_gives_mono_frames_of_10_ms_with_the_same_tone() {
        let (link, mut chain, mut taken) = chain();
        link.set_open(true);
        let format = Format {
            sample_rate: 44_100,
            channels: 2,
        };
        // The device delivers buffers that are no multiple of 10 ms.
        let mut from = 0;
        for _ in 0..200 {
            chain.deliver(&tone(format, 1000.0, 512, from), format);
            from += 512;
        }
        let frames = drain(&mut taken);
        // 102 400 device frames are 232 whole chunks of 441.
        assert_eq!(frames.len(), 232);
        assert!(frames.iter().all(|frame| frame.len() == FRAME_SAMPLES));
        let sound: Vec<i16> = frames[20..].concat();
        let hz = frequency(&sound);
        assert!((hz - 1000.0).abs() < 5.0, "{hz}");
        let level = crate::audio::level::dbfs(&sound);
        assert!((level - -15.05).abs() < 1.0, "{level}");
        assert!(link.speaking());
        assert!(link.sent.read() > -20.0 && link.input.read() > -20.0);
        assert_eq!(link.dropped(), 0);
    }

    #[test]
    fn a_closed_gate_sends_silence_and_keeps_the_input_level() {
        let (link, mut chain, mut taken) = chain();
        let format = Format {
            sample_rate: SAMPLE_RATE,
            channels: 1,
        };
        chain.deliver(&tone(format, 440.0, 4800, 0), format);
        let frames = drain(&mut taken);
        assert_eq!(frames.len(), 10);
        assert!(frames.iter().flatten().all(|sample| *sample == 0));
        assert!(link.input.read() > -20.0);
        assert_eq!(link.sent.read(), SILENCE_DBFS);
        assert!(!link.speaking());

        // The gate opens between two frames: push-to-talk has no delay.
        link.set_open(true);
        chain.deliver(&tone(format, 440.0, 480, 4800), format);
        let frames = drain(&mut taken);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].iter().any(|sample| *sample != 0));
        assert!(link.speaking());

        link.set_open(false);
        chain.deliver(&tone(format, 440.0, 480, 5280), format);
        assert!(drain(&mut taken)[0].iter().all(|sample| *sample == 0));
        assert!(!link.speaking());
    }

    #[test]
    fn a_change_of_the_device_format_starts_a_new_frame() {
        let (link, mut chain, mut taken) = chain();
        link.set_open(true);
        let first = Format {
            sample_rate: SAMPLE_RATE,
            channels: 1,
        };
        chain.deliver(&tone(first, 440.0, 700, 0), first);
        assert_eq!(drain(&mut taken).len(), 1);
        let second = Format {
            sample_rate: 16_000,
            channels: 1,
        };
        chain.deliver(&tone(second, 440.0, 160, 0), second);
        let frames = drain(&mut taken);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].len(), FRAME_SAMPLES);
    }

    #[test]
    fn frames_that_the_connection_does_not_take_are_counted_and_not_kept() {
        let link = Arc::new(CaptureLink::default());
        let (frames, _taken) = mpsc::channel(2);
        let processor = Arc::new(Processor::new(Processing::OFF));
        let mut chain = CaptureChain::new(link.clone(), processor, frames);
        let format = Format {
            sample_rate: SAMPLE_RATE,
            channels: 1,
        };
        chain.deliver(&vec![0.0; 480 * 5], format);
        assert_eq!(link.dropped(), 3);
    }

    #[tokio::test]
    async fn frames_that_wait_when_the_gate_closes_are_sent_as_silence() {
        let link = Arc::new(CaptureLink::default());
        let (frames, taken) = mpsc::channel(100);
        let processor = Arc::new(Processor::new(Processing::OFF));
        let mut chain = CaptureChain::new(link.clone(), processor, frames);
        let mut queue = FrameQueue::new(link.clone(), taken);
        let format = Format {
            sample_rate: SAMPLE_RATE,
            channels: 1,
        };
        // The key is held, and the connection takes no frame for a moment.
        link.set_open(true);
        chain.deliver(&tone(format, 440.0, 480 * 3, 0), format);
        let first = queue.next().await.unwrap();
        assert!(first.iter().any(|sample| *sample != 0));
        // The key is released while two frames with the voice still wait.
        link.set_open(false);
        for _ in 0..2 {
            let waiting = queue.next().await.unwrap();
            assert_eq!(waiting.len(), FRAME_SAMPLES);
            assert!(waiting.iter().all(|sample| *sample == 0));
        }

        // The key is released and pressed again while frames from before the release
        // wait: they are not sent after it. What is made after the new press is sent.
        chain.deliver(&tone(format, 440.0, 480 * 2, 0), format);
        link.set_open(true);
        chain.deliver(&tone(format, 440.0, 480 * 2, 0), format);
        link.set_open(false);
        link.set_open(true);
        chain.deliver(&tone(format, 440.0, 480, 0), format);
        let mut sent = Vec::new();
        for _ in 0..5 {
            let frame = queue.next().await.unwrap();
            sent.push(frame.iter().any(|sample| *sample != 0));
        }
        assert_eq!(sent, [false, false, false, false, true]);
    }

    #[test]
    fn a_device_rate_that_10_ms_do_not_divide_keeps_the_time_of_the_device() {
        let (link, mut chain, mut taken) = chain();
        link.set_open(true);
        let format = Format {
            sample_rate: 11_025,
            channels: 1,
        };
        // Ten seconds of the device are ten seconds of frames: 1000, and not 1002.
        let mut from = 0;
        let mut frames = Vec::new();
        for _ in 0..100 {
            chain.deliver(&tone(format, 1000.0, 1102, from), format);
            from += 1102;
            frames.extend(drain(&mut taken));
        }
        chain.deliver(&tone(format, 1000.0, 50, from), format);
        frames.extend(drain(&mut taken));
        assert_eq!(frames.len(), 1000);
        assert!(frames.iter().all(|frame| frame.len() == FRAME_SAMPLES));
        // The tone keeps its frequency and its level: no sample is left out or made up.
        let sound: Vec<i16> = frames[100..].concat();
        let hz = frequency(&sound);
        assert!((hz - 1000.0).abs() < 1.0, "{hz}");
        let level = crate::audio::level::dbfs(&sound);
        assert!((level - -15.05).abs() < 1.0, "{level}");
        assert_eq!(link.dropped(), 0);

        // A rate that the pipeline does not convert gives no frame.
        let odd = Format {
            sample_rate: 44_101,
            channels: 1,
        };
        chain.deliver(&tone(odd, 1000.0, 44_101, 0), odd);
        assert!(drain(&mut taken).is_empty());
    }
}
