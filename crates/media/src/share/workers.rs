//! Capture worker cancellation and bounded cleanup outside the media engine.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use super::life::Life;
use super::{CaptureError, CaptureNotice, CaptureSink};

const STOP_LIMIT: Duration = Duration::from_secs(2);

pub(crate) struct Workers {
    life: Arc<Life>,
    threads: Vec<JoinHandle<()>>,
    stopped: Option<oneshot::Sender<Result<(), CaptureError>>>,
    report: Option<Arc<dyn Fn(CaptureNotice) + Send + Sync>>,
}

impl Workers {
    pub fn new(life: Arc<Life>) -> (Self, oneshot::Receiver<Result<(), CaptureError>>) {
        let (stopped, result) = oneshot::channel();
        (
            Self {
                life,
                threads: Vec::new(),
                stopped: Some(stopped),
                report: None,
            },
            result,
        )
    }

    pub fn report_failures_to(&mut self, sink: Arc<dyn CaptureSink>) {
        self.report_failures_with(move |notice| sink.stop_failed(notice));
    }

    pub fn report_failures_with(&mut self, report: impl Fn(CaptureNotice) + Send + Sync + 'static) {
        self.report = Some(Arc::new(report));
    }

    pub fn spawn(
        &mut self,
        name: &str,
        work: impl FnOnce() + Send + 'static,
    ) -> Result<(), CaptureError> {
        let life = self.life.clone();
        let name = name.to_owned();
        let pending = Arc::new(std::sync::Mutex::new(Some(work)));
        let task = pending.clone();
        let thread = std::thread::Builder::new()
            .name(name.clone())
            .spawn(move || {
                let work = task
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(work) = work
                    && std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err()
                {
                    let error = CaptureError::Failed(format!("capture worker {name} panicked"));
                    life.cleanup_failed(error.clone());
                    life.fail(error);
                    life.stop();
                }
            });
        let thread = match thread {
            Ok(thread) => thread,
            Err(error) => {
                // Retain native resources when no worker can take ownership of their destructor.
                std::mem::forget(pending);
                let error =
                    CaptureError::StopFailed(format!("capture worker could not start: {error}"));
                self.life.cleanup_failed(error.clone());
                return Err(error);
            }
        };
        self.threads.push(thread);
        Ok(())
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        self.life.cancelled.store(true, Ordering::Release);
        self.life.stop();
        let threads = std::mem::take(&mut self.threads);
        let stopped = self.stopped.take();
        let life = self.life.clone();
        let report = self.report.take();
        let fallback = report.clone();
        let result = std::thread::Builder::new()
            .name("capture-stop".into())
            .spawn(move || {
                let result = finish(threads, &life, STOP_LIMIT);
                if let (Err(error), Some(report)) = (&result, report) {
                    report(CaptureNotice::new(error.clone()));
                }
                if let Some(stopped) = stopped {
                    let _ = stopped.send(result);
                }
            });
        if let Err(error) = result {
            tracing::error!("screen capture cleanup could not start: {error}");
            if let Some(report) = fallback {
                report(CaptureNotice::new(CaptureError::StopFailed(format!(
                    "capture cleanup could not start: {error}"
                ))));
            }
        }
    }
}

fn finish(
    mut threads: Vec<JoinHandle<()>>,
    life: &Life,
    limit: Duration,
) -> Result<(), CaptureError> {
    let deadline = Instant::now() + limit;
    let mut errors = Vec::new();
    let (finished, results) = std::sync::mpsc::channel();
    let mut pending = Vec::new();
    for thread in threads.drain(..) {
        let name = thread.thread().name().unwrap_or("unnamed").to_owned();
        pending.push(name.clone());
        let finished = finished.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("capture-join".into())
            .spawn(move || {
                let result = thread
                    .join()
                    .map_err(|_| format!("capture worker {name} panicked"));
                let _ = finished.send((name, result));
            })
        {
            errors.push(format!(
                "capture completion observer could not start: {error}"
            ));
        }
    }
    drop(finished);
    while !pending.is_empty() {
        match results.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok((name, result)) => {
                if let Some(index) = pending.iter().position(|pending| *pending == name) {
                    pending.swap_remove(index);
                }
                if let Err(error) = result {
                    errors.push(error);
                }
            }
            Err(_) => {
                for name in pending {
                    errors.push(format!("capture worker {name} did not stop within {limit:?}; restart Junto to release its remaining capture resources"));
                }
                break;
            }
        }
    }
    for error in &errors {
        tracing::error!("{error}");
    }
    if let Err(error) = life.cleanup_result() {
        errors.push(error.to_string());
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(CaptureError::StopFailed(errors.join("; ")))
    }
}

#[cfg(any(target_os = "macos", test))]
pub(crate) fn wait_for_start(
    mut started: oneshot::Receiver<Result<(), CaptureError>>,
    deadline: Instant,
) -> Result<(), CaptureError> {
    loop {
        match started.try_recv() {
            Ok(result) => return result,
            Err(oneshot::error::TryRecvError::Closed) => {
                return Err(CaptureError::Failed(
                    "capture stopped before starting".into(),
                ));
            }
            Err(oneshot::error::TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            return Err(CaptureError::Failed(
                "capture start exceeded its deadline".into(),
            ));
        }
        std::thread::park_timeout(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_capture_worker_that_ignores_stop_does_not_hold_its_owner() {
        let life = Arc::new(Life::default());
        let (mut workers, result) = Workers::new(life.clone());
        let (release, held) = std::sync::mpsc::channel();
        workers
            .spawn("stuck-picture", move || {
                let _ = held.recv_timeout(Duration::from_secs(10));
            })
            .unwrap();
        let (dropped, returned) = std::sync::mpsc::channel();
        let owner = std::thread::spawn(move || {
            drop(workers);
            let _ = dropped.send(());
        });
        let immediate = returned.recv_timeout(Duration::from_secs(1));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let stopped =
            runtime.block_on(async { tokio::time::timeout(Duration::from_secs(4), result).await });
        let _ = release.send(());
        owner.join().unwrap();
        assert!(immediate.is_ok(), "the capture drop waited for its worker");
        assert!(life.stopped());
        let error = stopped.unwrap().unwrap().unwrap_err().to_string();
        assert!(
            error.contains("stuck-picture") && error.contains("did not stop"),
            "{error}"
        );
    }

    #[test]
    fn thread_local_native_cleanup_is_included_in_the_deadline() {
        struct NativeCleanup(std::sync::mpsc::Receiver<()>);
        impl Drop for NativeCleanup {
            fn drop(&mut self) {
                let _ = self.0.recv_timeout(Duration::from_secs(5));
            }
        }
        thread_local! {
            static NATIVE: std::cell::RefCell<Option<NativeCleanup>> = const { std::cell::RefCell::new(None) };
        }
        let (release, held) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            NATIVE.with_borrow_mut(|native| *native = Some(NativeCleanup(held)))
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while !worker.is_finished() {
            assert!(
                Instant::now() < deadline,
                "worker never reached native cleanup"
            );
            std::thread::park_timeout(Duration::from_millis(1));
        }
        let result = finish(vec![worker], &Life::default(), Duration::from_millis(50));
        let _ = release.send(());
        assert!(matches!(result, Err(CaptureError::StopFailed(_))));
    }

    fn platform_capture_stop(platform: &str) {
        let (mut workers, result) = Workers::new(Arc::new(Life::default()));
        let mut releases = Vec::new();
        let mut returned = Vec::new();
        for part in ["screen", "window", "sound", "camera"] {
            let (release, held) = std::sync::mpsc::channel();
            let (done, stopped) = std::sync::mpsc::channel();
            workers
                .spawn(&format!("{platform}-{part}"), move || {
                    let _ = held.recv_timeout(Duration::from_secs(10));
                    let _ = done.send(());
                })
                .unwrap();
            releases.push(release);
            returned.push(stopped);
        }
        let before = Instant::now();
        drop(workers);
        assert!(before.elapsed() < Duration::from_secs(1));
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let result =
            runtime.block_on(async { tokio::time::timeout(Duration::from_secs(4), result).await });
        for release in releases {
            let _ = release.send(());
        }
        for stopped in returned {
            stopped.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        let error = result.unwrap().unwrap().unwrap_err().to_string();
        for part in ["screen", "window", "sound", "camera"] {
            assert!(error.contains(&format!("{platform}-{part}")), "{error}");
        }
    }

    #[test]
    fn linux_capture_cleanup_does_not_hold_its_owner() {
        platform_capture_stop("linux");
    }

    #[test]
    fn macos_capture_cleanup_does_not_hold_its_owner() {
        platform_capture_stop("macos");
    }

    #[test]
    fn windows_capture_cleanup_does_not_hold_its_owner() {
        platform_capture_stop("windows");
    }

    #[tokio::test]
    async fn macos_cleanup_includes_the_completion_of_pending_native_starts() {
        let life = Arc::new(Life::default());
        let (mut workers, result) = Workers::new(life);
        let (start, started) = oneshot::channel();
        let (cleaned, finished) = std::sync::mpsc::channel();
        workers
            .spawn("macos-pending-start", move || {
                wait_for_start(started, Instant::now() + Duration::from_secs(10)).unwrap();
                let _ = cleaned.send(());
            })
            .unwrap();
        drop(workers);
        let result = tokio::time::timeout(Duration::from_secs(4), result).await;
        let premature = finished.try_recv();
        let _ = start.send(Ok(()));
        finished.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(
            premature.is_err(),
            "cleanup completed before the native start answered"
        );
        assert!(matches!(
            result.unwrap().unwrap(),
            Err(CaptureError::StopFailed(_))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn a_missing_cleanup_acknowledgement_is_reported_with_a_bound() {
        let (held, stopped) = oneshot::channel();
        let stream = super::super::CaptureStream::new(2, 2, ()).with_stop_result(stopped);
        let result = stream.stop().await;
        drop(held);
        assert!(
            matches!(result, Err(CaptureError::StopFailed(reason)) if reason.contains("within 3 seconds"))
        );
    }

    #[test]
    fn a_capture_worker_panic_is_reported_by_the_stop() {
        let (mut workers, result) = Workers::new(Arc::new(Life::default()));
        workers
            .spawn("failed-picture", || panic!("worker failure"))
            .unwrap();
        drop(workers);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let error = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(4), result)
                .await
                .unwrap()
                .unwrap()
                .unwrap_err()
        });
        assert!(error.to_string().contains("failed-picture"));
    }
}
