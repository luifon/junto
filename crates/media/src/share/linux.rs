//! Linux screen and application-sound capture (ADR 0751).

mod audio;
mod connection;
mod graph;
mod portal;
mod registry;
mod video;

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::oneshot;

use super::delivery::{CAPACITY, Latest, Life, deliver, latest, pixels};
use super::{
    CaptureError, CaptureRequest, CaptureSink, CaptureStream, Pending, PictureSource, ScreenAccess,
    ScreenCapture, ShareSources, SoundChoice,
};
use portal::error;

use super::workers::Workers;

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
    portal.report_failures_to(sink.clone());
    let parent = request.dialog_parent.as_deref().unwrap_or_default();
    let (source, mut closed) = portal.start(kind, parent).await?;
    let life = Arc::new(Life::default());
    let (stop_portal, stopped) = oneshot::channel();
    let pictures = Arc::new(Latest::default());
    let (mut workers, result) = Workers::new(life.clone());
    workers.report_failures_to(sink.clone());
    life.on_stop({
        let pictures = pictures.clone();
        let stop = Mutex::new(Some(stop_portal));
        move || {
            pictures.wake();
            if let Some(stop) = stop.lock().unwrap_or_else(PoisonError::into_inner).take() {
                let _ = stop.send(());
            }
        }
    });
    let (closed_portal, portal_closed) = oneshot::channel();
    tokio::spawn({
        let life = life.clone();
        async move {
            tokio::select! {
                _ = stopped => {},
                _ = closed.next() => life.fail(CaptureError::SourceGone),
            }
            let result = portal.close().await;
            if let Err(error) = &result {
                life.cleanup_failed(error.clone());
            }
            let _ = closed_portal.send(result);
        }
    });
    workers.spawn("linux-share-portal", {
        let life = life.clone();
        move || {
            if let Err(error) = portal_closed.blocking_recv() {
                life.cleanup_failed(CaptureError::StopFailed(format!(
                    "desktop portal cleanup ended without a result: {error}"
                )));
            }
        }
    })?;
    let (ready, first_frame) = oneshot::channel();
    workers.spawn("linux-share-picture", {
        let (life, pictures) = (life.clone(), pictures.clone());
        move || video::run(life, source, request.quality, pictures, ready)
    })?;
    let (width, height) = prepared(first_frame, &life).await?;
    let samples = if request.sound != SoundChoice::None {
        let (send_samples, samples) = rtrb::RingBuffer::new(CAPACITY);
        let (ready, sound_ready) = oneshot::channel();
        workers.spawn("linux-share-sound", {
            let life = life.clone();
            move || audio::run(life, request.sound, send_samples, ready)
        })?;
        prepared(sound_ready, &life).await?;
        Some(samples)
    } else {
        None
    };
    life.result()?;
    workers.spawn("linux-share-delivery", {
        let life = life.clone();
        move || {
            deliver(
                life,
                sink,
                pictures,
                |frame| Ok(Some(frame)),
                samples,
                request.quality,
            )
        }
    })?;
    Ok(CaptureStream::new(width, height, workers).with_stop_result(result))
}

// The real portal test requires the dedicated virtual-display command, not the call tier.
#[cfg(all(test, feature = "portal-tests"))]
mod live;
