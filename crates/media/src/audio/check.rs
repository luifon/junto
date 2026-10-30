use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use tokio::sync::{mpsc, watch};

use super::capture::{CaptureChain, CaptureLink, FrameQueue};
use super::host::usable;
use super::playout::{Mixer, PlayoutChain};
use super::processing::Processor;
use super::{AudioError, AudioHost, OpenStream, level};
use crate::engine::{Levels, VoiceConfig};

pub(crate) type Failure = Arc<dyn Fn(AudioError) + Send + Sync>;

pub(crate) struct AudioCheck {
    _streams: Vec<OpenStream>,
    active: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl AudioCheck {
    pub(crate) fn microphone(
        host: &dyn AudioHost,
        voice: &VoiceConfig,
        levels: watch::Sender<Levels>,
        failed: Failure,
        active: Arc<AtomicBool>,
        runtime: &tokio::runtime::Handle,
    ) -> Result<Self, AudioError> {
        let devices = host.devices()?;
        let processor = Arc::new(Processor::new(voice.processing));
        let mixer = Mixer::shared();
        let source = mixer.add(1.0);
        let mut playout = PlayoutChain::new(mixer, processor.clone());
        let output_failed = failed.clone();
        let output_active = active.clone();
        let output = host.open_output(
            usable(voice.output_device.as_ref(), &devices.outputs),
            Box::new(move |samples, format| {
                if output_active.load(Ordering::Acquire) {
                    playout.fill(samples, format);
                } else {
                    samples.fill(0.0);
                }
            }),
            Box::new(move |error| output_failed(error)),
        )?;
        let link = Arc::new(CaptureLink::default());
        link.set_open(true);
        let (given, taken) = mpsc::channel(20);
        let mut capture = CaptureChain::new(link.clone(), processor, given);
        let input_active = active.clone();
        let input = host.open_input(
            usable(voice.input_device.as_ref(), &devices.inputs),
            Box::new(move |samples, format| {
                if input_active.load(Ordering::Acquire) {
                    capture.deliver(samples, format);
                }
            }),
            Box::new(move |error| failed(error)),
        )?;
        let mut frames = FrameQueue::new(link.clone(), taken);
        let publishing = active.clone();
        let task = runtime.spawn(async move {
            let mut timer = tokio::time::interval(std::time::Duration::from_millis(50));
            loop {
                tokio::select! {
                    frame = frames.next() => {
                        let Some(frame) = frame else { break; };
                        let stereo: Vec<_> = frame.into_iter().flat_map(|sample| [sample, sample]).collect();
                        source.push(&stereo);
                    }
                    _ = timer.tick() => {
                        if publishing.load(Ordering::Acquire) { levels.send_modify(|levels| levels.input = level::meter(link.input.read())); }
                    }
                }
            }
        });
        Ok(Self {
            _streams: vec![input, output],
            active,
            task: Some(task),
        })
    }

    pub(crate) fn output(
        host: &dyn AudioHost,
        voice: &VoiceConfig,
        failed: Failure,
        active: Arc<AtomicBool>,
    ) -> Result<Self, AudioError> {
        let devices = host.devices()?;
        let mut elapsed = 0.0_f64;
        let playing = active.clone();
        let output = host.open_output(
            usable(voice.output_device.as_ref(), &devices.outputs),
            Box::new(move |samples, format| {
                if !playing.load(Ordering::Acquire) {
                    samples.fill(0.0);
                    return;
                }
                for frame in samples.chunks_mut(usize::from(format.channels.max(1))) {
                    let time = elapsed % 1.0;
                    let fade = (time / 0.01).min((0.35 - time) / 0.01).clamp(0.0, 1.0);
                    let sample = (elapsed * 660.0 * std::f64::consts::TAU).sin() * 0.03 * fade;
                    frame.fill(sample as f32);
                    elapsed += 1.0 / f64::from(format.sample_rate.max(1));
                }
            }),
            Box::new(move |error| failed(error)),
        )?;
        Ok(Self {
            _streams: vec![output],
            active,
            task: None,
        })
    }
}

impl Drop for AudioCheck {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
