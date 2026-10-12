//! Linux screen and application-sound capture (ADR 0751).

mod audio;
mod connection;
mod graph;
mod pixels;
mod portal;
mod registry;
mod video;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use tokio::sync::oneshot;

use super::{
    CaptureError, CaptureRequest, CaptureSink, CaptureStream, Pending, PictureSource, ScreenAccess,
    ScreenCapture, ShareSources, SoundChoice,
};
use crate::audio::Format;
use portal::error;

#[derive(Default)]
struct Life {
    stopped: AtomicBool,
    cancelled: AtomicBool,
    failure: Mutex<Option<CaptureError>>,
}
impl Life {
    fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
    }
    fn fail(&self, failure: CaptureError) {
        let mut held = self.failure.lock().unwrap_or_else(PoisonError::into_inner);
        if !self.stopped() {
            *held = Some(failure);
            self.stop();
        }
    }
    fn result(&self) -> Result<(), CaptureError> {
        self.failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .map_or(Ok(()), Err)
    }
}

struct Workers {
    life: Arc<Life>,
    threads: Vec<JoinHandle<()>>,
    portal: Option<oneshot::Sender<()>>,
}
impl Drop for Workers {
    fn drop(&mut self) {
        self.life.cancelled.store(true, Ordering::Release);
        self.life.stop();
        if let Some(portal) = self.portal.take() {
            let _ = portal.send(());
        }
        for thread in self.threads.drain(..) {
            thread.thread().unpark();
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
        }
    }
}

/// The authorisation chooser is owned by the desktop and appears only at start.
pub struct SystemCapture;
impl ScreenCapture for SystemCapture {
    fn access(&self) -> ScreenAccess {
        ScreenAccess::AskOnStart
    }
    fn request_access(&self) {}
    fn open_settings(&self) {}

    fn sources(&self) -> Pending<Result<ShareSources, CaptureError>> {
        Box::pin(async {
            let portal = portal::Portal::connect().await?;
            let capabilities = portal.capabilities().await?;
            let sound = tokio::task::spawn_blocking(audio::sources)
                .await
                .map_err(error)?;
            let (system_sound, applications) = match sound {
                Ok(sources) => sources,
                Err(failure) => {
                    tracing::debug!(%failure, "the local sound graph is unavailable for sharing");
                    (false, Vec::new())
                }
            };
            Ok(ShareSources {
                portal: capabilities.kinds,
                system_sound,
                applications,
                ..ShareSources::default()
            })
        })
    }

    fn start(
        &self,
        request: CaptureRequest,
        sink: Arc<dyn CaptureSink>,
    ) -> Pending<Result<CaptureStream, CaptureError>> {
        Box::pin(start(request, sink))
    }
}

async fn prepared<T>(
    ready: oneshot::Receiver<Result<T, CaptureError>>,
    life: &Life,
) -> Result<T, CaptureError> {
    match tokio::time::timeout(Duration::from_secs(30), ready).await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => {
            life.result()?;
            Err(error("the capture stopped before its first buffer"))
        }
        Err(_) => Err(error("the capture did not produce its first buffer")),
    }
}

async fn start(
    request: CaptureRequest,
    sink: Arc<dyn CaptureSink>,
) -> Result<CaptureStream, CaptureError> {
    let PictureSource::Portal(kind) = request.picture else {
        return Err(CaptureError::SourceGone);
    };
    if !super::FRAME_RATES.contains(&request.quality.frame_rate) {
        return Err(error("unsupported share frame rate"));
    }
    let mut portal = portal::Portal::connect().await?;
    let (source, mut closed) = portal.start(kind).await?;
    let life = Arc::new(Life::default());
    let (stop_portal, stopped) = oneshot::channel();
    let mut workers = Workers {
        life: life.clone(),
        threads: Vec::new(),
        portal: Some(stop_portal),
    };
    tokio::spawn({
        let life = life.clone();
        async move {
            tokio::select! {
                _ = stopped => {},
                _ = closed.next() => life.fail(CaptureError::SourceGone),
            }
            drop(portal);
        }
    });
    let (send_frames, frames) = mpsc::sync_channel(2);
    let (ready, first_frame) = oneshot::channel();
    workers.threads.push(
        std::thread::Builder::new()
            .name("share-picture".into())
            .spawn({
                let life = life.clone();
                move || video::run(life, source, request.quality, send_frames, ready)
            })
            .map_err(error)?,
    );
    let (width, height) = prepared(first_frame, &life).await?;
    let samples = if request.sound != SoundChoice::None {
        let (send_samples, samples) = rtrb::RingBuffer::new(audio::CAPACITY);
        let (ready, sound_ready) = oneshot::channel();
        workers.threads.push(
            std::thread::Builder::new()
                .name("share-sound".into())
                .spawn({
                    let life = life.clone();
                    move || audio::run(life, request.sound, send_samples, ready)
                })
                .map_err(error)?,
        );
        prepared(sound_ready, &life).await?;
        Some(samples)
    } else {
        None
    };
    life.result()?;
    workers.threads.push(
        std::thread::Builder::new()
            .name("share-delivery".into())
            .spawn({
                let life = life.clone();
                move || deliver(life, sink, frames, samples, request.quality.resolution)
            })
            .map_err(error)?,
    );
    Ok(CaptureStream::new(width, height, workers))
}

fn deliver(
    life: Arc<Life>,
    sink: Arc<dyn CaptureSink>,
    frames: mpsc::Receiver<pixels::Frame>,
    samples: Option<rtrb::Consumer<[f32; 2]>>,
    resolution: super::Resolution,
) {
    let mut sound = samples.map(SoundClock::new);
    let mut next = Instant::now();
    while !life.stopped() {
        let mut latest = None;
        while let Ok(frame) = frames.try_recv() {
            latest = Some(frame);
        }
        if let Some(frame) = latest {
            let buffer = frame.convert(resolution);
            if !life.stopped() {
                sink.picture(Arc::new(buffer));
            }
        }
        let now = Instant::now();
        if now >= next {
            if let Some(sound) = sound.as_mut() {
                let samples = sound.tick(now);
                if !life.stopped() {
                    sink.sound(
                        &samples,
                        Format {
                            sample_rate: 48_000,
                            channels: 2,
                        },
                    );
                }
            }
            next += Duration::from_millis(10);
            if now.duration_since(next.min(now)) > Duration::from_millis(50) {
                next = now + Duration::from_millis(10);
            }
        }
        std::thread::park_timeout(next.saturating_duration_since(Instant::now()));
    }
    if !life.cancelled.load(Ordering::Acquire)
        && let Err(failure) = life.result()
    {
        sink.ended(failure);
    }
}

struct SoundClock {
    samples: rtrb::Consumer<[f32; 2]>,
    prime: Option<Instant>,
    playing: bool,
}
impl SoundClock {
    fn new(samples: rtrb::Consumer<[f32; 2]>) -> Self {
        Self {
            samples,
            prime: None,
            playing: false,
        }
    }
    fn tick(&mut self, now: Instant) -> [f32; 960] {
        let mut out = [0.0; 960];
        if !self.playing {
            if self.samples.slots() == 0 {
                return out;
            }
            let prime = self.prime.get_or_insert(now + Duration::from_millis(60));
            if now < *prime {
                return out;
            }
            self.playing = true;
        }
        for frame in out.as_chunks_mut::<2>().0 {
            match self.samples.pop() {
                Ok(pair) => frame.copy_from_slice(&pair),
                Err(_) => {
                    self.playing = false;
                    self.prime = None;
                    break;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_audio_clock_supplies_silence_through_a_pause_and_restarts() {
        let (mut input, output) = rtrb::RingBuffer::new(4_800);
        let mut clock = SoundClock::new(output);
        let now = Instant::now();
        assert_eq!(clock.tick(now), [0.0; 960]);
        for _ in 0..960 {
            assert!(input.push([0.2, 0.3]).is_ok());
        }
        assert_eq!(clock.tick(now), [0.0; 960]);
        assert_eq!(
            &clock.tick(now + Duration::from_millis(60))[..4],
            &[0.2, 0.3, 0.2, 0.3]
        );
        assert_eq!(
            &clock.tick(now + Duration::from_millis(70))[..4],
            &[0.2, 0.3, 0.2, 0.3]
        );
        for tick in 8..20 {
            assert_eq!(
                clock.tick(now + Duration::from_millis(tick * 10)),
                [0.0; 960]
            );
        }
        for _ in 0..480 {
            assert!(input.push([0.4, 0.5]).is_ok());
        }
        assert_eq!(clock.tick(now + Duration::from_millis(200)), [0.0; 960]);
        assert_eq!(
            &clock.tick(now + Duration::from_millis(260))[..4],
            &[0.4, 0.5, 0.4, 0.5]
        );
    }
}

// The real portal test requires the dedicated virtual-display command, not the call tier.
#[cfg(all(test, feature = "portal-tests"))]
mod live;
