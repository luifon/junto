//! Cancellation fences delivery immediately; native resources close on their owning thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;
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

    /// Holds `device` for this capture: waits while another capture of the process holds it,
    /// because the driver refuses a second open until the first capture has closed it. Returns
    /// `None` when this capture was stopped first.
    pub fn claim(&self, device: &str) -> Option<Claim> {
        let mut claimed = CLAIMED.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if self.stopped() {
                return None;
            }
            if !claimed.iter().any(|held| held == device) {
                claimed.push(device.to_owned());
                return Some(Claim(device.to_owned()));
            }
            // A stop does not signal the condition; it is read again after this bound.
            claimed = RELEASED
                .wait_timeout(claimed, Duration::from_millis(100))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
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

/// A device that one capture holds; dropped after the native capture has closed.
pub(super) struct Claim(String);
impl Drop for Claim {
    fn drop(&mut self) {
        CLAIMED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|held| *held != self.0);
        RELEASED.notify_all();
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

/// The size of the pictures and the frame rate that the driver was asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Started {
    pub width: u32,
    pub height: u32,
    pub rate: Option<(u32, u32)>,
}

pub(super) type Ready = oneshot::Sender<Result<Started, CameraError>>;

/// The rate of the last capture that started, for the device check.
#[cfg(feature = "native-device-tests")]
pub(super) static LAST_RATE: Mutex<Option<(u32, u32)>> = Mutex::new(None);

/// The pictures of each device in the process, for the device check: those that native
/// capture received complete, before the limit of 30 per second, and those that the driver
/// lost for lack of a queued buffer, when the driver numbers its pictures.
#[cfg(all(
    any(target_os = "windows", target_os = "linux"),
    feature = "native-device-tests"
))]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub received: u64,
    pub lost: Option<u64>,
}

#[cfg(all(
    any(target_os = "windows", target_os = "linux"),
    feature = "native-device-tests"
))]
pub(super) static COUNTS: Mutex<Vec<(String, Counts)>> = Mutex::new(Vec::new());

/// Counts `received` complete pictures and `lost` pictures of `device` for the device check;
/// nothing without the `native-device-tests` feature.
#[cfg(any(target_os = "windows", target_os = "linux"))]
pub(super) fn count(device: &str, received: u64, lost: Option<u32>) {
    #[cfg(feature = "native-device-tests")]
    {
        let mut all = COUNTS.lock().unwrap_or_else(PoisonError::into_inner);
        let index = match all.iter().position(|(id, _)| id == device) {
            Some(index) => index,
            None => {
                all.push((device.to_owned(), Counts::default()));
                all.len() - 1
            }
        };
        let counts = &mut all[index].1;
        counts.received += received;
        if let Some(lost) = lost {
            counts.lost = Some(counts.lost.unwrap_or(0) + u64::from(lost));
        }
    }
    #[cfg(not(feature = "native-device-tests"))]
    let _ = (device, received, lost);
}

const INTERVAL: Duration = Duration::from_nanos(1_000_000_000 / 30);

/// Limits delivery to 30 pictures per second, from the time stamps of the pictures. A camera
/// whose average gap is at least that of 30.5 pictures per second keeps every picture. A
/// faster picture is taken when it is at its place, one interval after the place or the time
/// of the last taken picture, whichever is later, so that the gaps stay even. For a camera
/// whose gap is at most about half an interval, a picture up to a quarter interval, and less
/// than half its own gap, before its place counts as at its place, so that the jitter of a
/// camera of 60 does not move every taken picture by one gap.
#[derive(Default)]
pub(super) struct Pacer {
    last: Option<Duration>,
    /// The average gap between pictures, over about the last eight.
    gap: Option<Duration>,
    next: Duration,
}

impl Pacer {
    /// `at`: the time stamp of a picture, from a fixed origin of the stream.
    pub fn due(&mut self, at: Duration) -> bool {
        let gap = match self.last.replace(at) {
            Some(last) if at > last => at - last,
            // The first picture, or time stamps that started again.
            _ => {
                self.gap = None;
                self.next = at + INTERVAL;
                return true;
            }
        };
        let gap = self.gap.map_or(gap, |average| (average * 7 + gap) / 8);
        self.gap = Some(gap);
        if gap * 61 >= INTERVAL * 60 {
            self.next = at + INTERVAL;
            return true;
        }
        let early = if gap * 40 <= INTERVAL * 21 {
            (INTERVAL / 4).min(gap / 2)
        } else {
            Duration::ZERO
        };
        if at + early < self.next {
            return false;
        }
        // A picture after its place moves the place of the next one, so that the gaps follow a
        // camera that lags the limit, such as one of 59.94 pictures per second.
        self.next = self.next.max(at) + INTERVAL;
        true
    }
}

/// The devices that a capture holds, of every capture of the process.
static CLAIMED: Mutex<Vec<String>> = Mutex::new(Vec::new());
static RELEASED: Condvar = Condvar::new();

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
    let started = tokio::time::timeout(Duration::from_secs(10), answer)
        .await
        .map_err(|_| {
            CameraError::Failed("camera sent no picture before the start deadline".into())
        })?
        .map_err(|_| CameraError::Failed("camera stopped before starting".into()))??;
    #[cfg(feature = "native-device-tests")]
    {
        *LAST_RATE.lock().unwrap_or_else(PoisonError::into_inner) = started.rate;
    }
    Ok(CaptureStream::new(started.width, started.height, running))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// The pictures of `rate` per second over `seconds`, each moved by `jitter(index)`, that
    /// the pacer delivers, as their time stamps.
    fn paced(rate: f64, seconds: u32, jitter: impl Fn(u32) -> f64) -> Vec<Duration> {
        let mut pacer = Pacer::default();
        let count = (rate * f64::from(seconds)) as u32;
        (0..count)
            .map(|index| {
                let at = f64::from(index) / rate + jitter(index) / 1000.0;
                Duration::from_secs_f64(at.max(0.0))
            })
            .filter(|at| pacer.due(*at))
            .collect()
    }

    #[test]
    fn a_thirty_fps_camera_with_jitter_keeps_every_picture() {
        // Up to two milliseconds early or late, in a pattern without a period of the grid.
        let pattern = [0.0, 1.8, -1.5, 0.7, -2.0, 1.1, -0.4, 2.0, -1.2];
        let delivered = paced(30.0, 10, |index| pattern[index as usize % pattern.len()]);
        assert_eq!(delivered.len(), 300);
        let delivered = paced(30.01, 10, |_| 0.0);
        assert_eq!(delivered.len(), 300);
        let delivered = paced(29.97, 10, |_| 0.0);
        assert_eq!(delivered.len(), 299);
        let delivered = paced(30.4, 10, |_| 0.0);
        assert_eq!(delivered.len(), 304);
    }

    /// A camera between 30.5 and twice 30 pictures per second gives every second or third
    /// picture with even gaps, not more pictures with uneven gaps.
    #[test]
    fn a_camera_somewhat_faster_than_thirty_keeps_even_gaps() {
        for (rate, count) in [(32.0, 160), (36.0, 180), (45.0, 225)] {
            let delivered = paced(rate, 10, |_| 0.0);
            assert!(
                delivered.len().abs_diff(count) <= 1,
                "{rate}: {}",
                delivered.len()
            );
            let gap = Duration::from_secs_f64(10.0 / count as f64);
            for pair in delivered.windows(2) {
                let between = pair[1] - pair[0];
                assert!(
                    between.abs_diff(gap) < Duration::from_millis(1),
                    "{rate}: {between:?}"
                );
            }
        }
    }

    #[test]
    fn a_sixty_fps_camera_with_driver_jitter_gives_thirty_with_even_gaps() {
        let pattern = [0.0, 0.2, -0.3, 0.1, -0.2, 0.3, -0.1];
        let delivered = paced(60.0, 10, |index| pattern[index as usize % pattern.len()]);
        assert!(delivered.len().abs_diff(300) <= 1, "{}", delivered.len());
        for pair in delivered.windows(2) {
            let between = pair[1] - pair[0];
            assert!(
                between.abs_diff(INTERVAL) < Duration::from_millis(2),
                "{between:?}"
            );
        }
    }

    #[test]
    fn a_slower_camera_keeps_every_picture() {
        for rate in [24.0, 15.0, 7.5] {
            let delivered = paced(rate, 4, |_| 0.0);
            assert_eq!(delivered.len(), (rate * 4.0) as usize);
        }
    }

    #[test]
    fn a_faster_camera_is_limited_to_thirty_with_even_gaps() {
        // A camera of 50 gives every second picture: 25 per second with even gaps.
        for (rate, count) in [(60.0, 300), (120.0, 300), (59.94, 300), (50.0, 250)] {
            let delivered = paced(rate, 10, |_| 0.0);
            assert!(
                delivered.len().abs_diff(count) <= 1,
                "{rate}: {}",
                delivered.len()
            );
            let gap = Duration::from_secs_f64(10.0 / count as f64);
            for pair in delivered.windows(2) {
                let between = pair[1] - pair[0];
                assert!(
                    between.abs_diff(gap) < Duration::from_millis(1),
                    "{rate}: {between:?}"
                );
            }
        }
    }

    #[test]
    fn time_stamps_that_start_again_start_the_pacing_again() {
        let mut pacer = Pacer::default();
        assert!(pacer.due(Duration::from_secs(100)));
        assert!(pacer.due(Duration::from_secs(100) + INTERVAL));
        assert!(pacer.due(Duration::from_millis(5)));
        assert!(pacer.due(Duration::from_millis(5) + INTERVAL));
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
            assert!(
                ready
                    .send(Ok(Started {
                        width: 4,
                        height: 2,
                        rate: None,
                    }))
                    .is_err()
            );
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
            ready
                .send(Ok(Started {
                    width: 640,
                    height: 480,
                    rate: Some((30, 1)),
                }))
                .unwrap();
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

    fn delivery() -> Arc<Delivery> {
        Arc::new(Delivery {
            stopped: AtomicBool::new(false),
            sink: Mutex::new(None),
        })
    }

    #[test]
    fn a_second_capture_of_a_device_waits_until_the_first_has_closed_it() {
        let first = delivery().claim("claimed-device").unwrap();
        let (claimed, second) = std::sync::mpsc::channel();
        let waiting = std::thread::spawn(move || {
            let claim = delivery().claim("claimed-device");
            claimed.send(()).unwrap();
            claim.is_some()
        });
        // Another device is not held by the first claim.
        assert!(delivery().claim("other-device").is_some());
        assert!(second.recv_timeout(Duration::from_millis(300)).is_err());
        drop(first);
        second.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(waiting.join().unwrap());
    }

    #[test]
    fn a_stopped_capture_stops_waiting_for_its_device() {
        let _first = delivery().claim("stopped-device").unwrap();
        let waiting = delivery();
        let stopped = waiting.clone();
        let thread = std::thread::spawn(move || waiting.claim("stopped-device").is_none());
        stopped.stopped.store(true, Ordering::Release);
        assert!(thread.join().unwrap());
    }
}
