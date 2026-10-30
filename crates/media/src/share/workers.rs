//! Capture worker cancellation and bounded cleanup outside the media engine.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tokio::sync::oneshot;

use super::delivery::Life;
use super::{CaptureError, CaptureSink};

const STOP_LIMIT: Duration = Duration::from_secs(2);

pub(crate) struct Workers {
    life: Arc<Life>,
    threads: Vec<JoinHandle<()>>,
    stopped: Option<oneshot::Sender<Result<(), CaptureError>>>,
    sink: Option<Arc<dyn CaptureSink>>,
}

impl Workers {
    pub fn new(life: Arc<Life>) -> (Self, oneshot::Receiver<Result<(), CaptureError>>) {
        let (stopped, result) = oneshot::channel();
        (
            Self {
                life,
                threads: Vec::new(),
                stopped: Some(stopped),
                sink: None,
            },
            result,
        )
    }

    pub fn report_failures_to(&mut self, sink: Arc<dyn CaptureSink>) {
        self.sink = Some(sink);
    }

    pub fn spawn(
        &mut self,
        name: &str,
        work: impl FnOnce() + Send + 'static,
    ) -> Result<(), CaptureError> {
        let life = self.life.clone();
        let name = name.to_owned();
        let thread = std::thread::Builder::new()
            .name(name.clone())
            .spawn(move || {
                if std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err() {
                    let error = CaptureError::Failed(format!("capture worker {name} panicked"));
                    life.cleanup_failed(error.clone());
                    life.fail(error);
                    life.stop();
                }
            })
            .map_err(|error| CaptureError::Failed(error.to_string()))?;
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
        let sink = self.sink.take();
        let fallback = sink.clone();
        let result = std::thread::Builder::new()
            .name("share-stop".into())
            .spawn(move || {
                let result = finish(threads, &life, STOP_LIMIT);
                if let (Err(error), Some(sink)) = (&result, sink) {
                    sink.stop_failed(error.clone());
                }
                if let Some(stopped) = stopped {
                    let _ = stopped.send(result);
                }
            });
        if let Err(error) = result {
            tracing::error!("screen capture cleanup could not start: {error}");
            if let Some(sink) = fallback {
                sink.stop_failed(CaptureError::StopFailed(format!(
                    "capture cleanup could not start: {error}"
                )));
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
    loop {
        let mut index = 0;
        while index < threads.len() {
            #[cfg(target_os = "windows")]
            let finished = super::windows::worker_finished(&threads[index]);
            #[cfg(not(target_os = "windows"))]
            let finished: Result<bool, CaptureError> = Ok(threads[index].is_finished());
            match finished {
                Ok(true) => drop(threads.swap_remove(index)),
                Ok(false) => index += 1,
                Err(error) => {
                    tracing::error!(%error, "capture worker completion check failed");
                    errors.push(error.to_string());
                    drop(threads.swap_remove(index));
                }
            }
        }
        if threads.is_empty() {
            break;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            for thread in threads {
                let name = thread.thread().name().unwrap_or("unnamed");
                let error = format!(
                    "capture worker {name} did not stop within {limit:?}; restart Junto to release its remaining capture resources"
                );
                tracing::error!("{error}");
                errors.push(error);
            }
            break;
        }
        std::thread::park_timeout(remaining.min(Duration::from_millis(5)));
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
