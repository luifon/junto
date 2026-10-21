//! Cancellation fences delivery immediately; native resources close on their owning thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};
use tokio::sync::oneshot;

use super::{CameraError, CameraSink};
use crate::share::CaptureStream;
use livekit::webrtc::video_frame::VideoBuffer;

pub(super) struct Delivery {
    stopped: AtomicBool,
    sink: Mutex<Option<Arc<dyn CameraSink>>>,
}

impl Delivery {
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }

    pub fn picture(&self, frame: Arc<dyn VideoBuffer>) {
        if let Some(sink) = &*self.sink.lock().unwrap_or_else(PoisonError::into_inner) {
            sink.picture(frame);
        }
    }

    pub fn ended(&self, error: CameraError) {
        if let Some(sink) = self
            .sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            sink.ended(error);
        }
        self.stopped.store(true, Ordering::Release);
    }
}

struct Running(Arc<Delivery>);
impl Drop for Running {
    fn drop(&mut self) {
        self.0.stopped.store(true, Ordering::Release);
        self.0
            .sink
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
    }
}

pub(super) type Ready = oneshot::Sender<Result<(u32, u32), CameraError>>;

pub(super) fn frame_due(last: &mut Option<Instant>, arrived: Instant) -> bool {
    let interval = Duration::from_nanos(1_000_000_000 / 30);
    if last.is_some_and(|last| arrived.saturating_duration_since(last) < interval) {
        return false;
    }
    *last = Some(arrived);
    true
}

pub(super) async fn start(
    sink: Arc<dyn CameraSink>,
    capture: impl FnOnce(Arc<Delivery>, Ready) + Send + 'static,
) -> Result<CaptureStream, CameraError> {
    let delivery = Arc::new(Delivery {
        stopped: AtomicBool::new(false),
        sink: Mutex::new(Some(sink)),
    });
    let running = Running(delivery.clone());
    let (ready, answer) = oneshot::channel();
    std::thread::Builder::new()
        .name("camera-capture".into())
        .spawn(move || capture(delivery, ready))
        .map_err(|error| CameraError::Failed(error.to_string()))?;
    let (w, h) = tokio::time::timeout(Duration::from_secs(10), answer)
        .await
        .map_err(|_| {
            CameraError::Failed("camera sent no picture before the start deadline".into())
        })?
        .map_err(|_| CameraError::Failed("camera stopped before starting".into()))??;
    Ok(CaptureStream::new(w, h, running))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn thirty_fps_is_measured_from_arrival_and_faster_frames_are_discarded() {
        let start = Instant::now();
        let interval = Duration::from_nanos(1_000_000_000 / 30);
        let mut last = None;
        for index in 0..30 {
            let arrived = start + interval * index;
            assert!(frame_due(&mut last, arrived));
            assert!(!frame_due(&mut last, arrived + Duration::from_millis(10)));
        }
        assert!(frame_due(&mut last, start + Duration::from_secs(5)));
    }

    #[derive(Default)]
    struct Sink {
        pictures: AtomicUsize,
        errors: AtomicUsize,
    }
    impl CameraSink for Sink {
        fn picture(&self, _: Arc<dyn VideoBuffer>) {
            self.pictures.fetch_add(1, Ordering::Relaxed);
        }
        fn ended(&self, _: CameraError) {
            self.errors.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[tokio::test]
    async fn cancellation_during_start_closes_delivery_and_native_worker() {
        let sink = Arc::new(Sink::default());
        let (entered, thread_entered) = oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let (exited, thread_exited) = oneshot::channel();
        let capture = tokio::spawn(start(sink.clone(), move |delivery, ready| {
            let _ = entered.send(());
            released.recv().unwrap();
            assert!(delivery.stopped());
            delivery.picture(Arc::new(livekit::webrtc::video_frame::I420Buffer::new(
                4, 2,
            )));
            delivery.ended(CameraError::DeviceGone);
            assert!(ready.send(Ok((4, 2))).is_err());
            let _ = exited.send(());
        }));
        thread_entered.await.unwrap();
        capture.abort();
        assert!(capture.await.unwrap_err().is_cancelled());
        release.send(()).unwrap();
        thread_exited.await.unwrap();
        assert_eq!(sink.pictures.load(Ordering::Relaxed), 0);
        assert_eq!(sink.errors.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn dropping_a_running_camera_fences_frames_and_failure() {
        let sink = Arc::new(Sink::default());
        let (entered, thread_entered) = oneshot::channel();
        let stream = start(sink.clone(), move |delivery, ready| {
            ready.send(Ok((640, 480))).unwrap();
            entered
                .send(delivery)
                .unwrap_or_else(|_| panic!("receiver is alive"));
        })
        .await
        .unwrap();
        let delivery = thread_entered.await.unwrap();
        delivery.picture(Arc::new(livekit::webrtc::video_frame::I420Buffer::new(
            4, 2,
        )));
        drop(stream);
        assert!(delivery.stopped());
        delivery.picture(Arc::new(livekit::webrtc::video_frame::I420Buffer::new(
            4, 2,
        )));
        delivery.ended(CameraError::DeviceGone);
        assert_eq!(sink.pictures.load(Ordering::Relaxed), 1);
        assert_eq!(sink.errors.load(Ordering::Relaxed), 0);
    }

    #[tokio::test]
    async fn startup_failure_is_returned_without_a_running_capture() {
        let sink = Arc::new(Sink::default());
        let result = start(sink.clone(), |_, ready| {
            let _ = ready.send(Err(CameraError::NoPermission(
                super::super::CameraAccess::Refused,
            )));
        })
        .await;
        assert!(matches!(result, Err(CameraError::NoPermission(_))));
        assert_eq!(sink.errors.load(Ordering::Relaxed), 0);
    }
}
