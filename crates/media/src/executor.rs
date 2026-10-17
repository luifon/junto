//! The executors that poll the futures of this crate (ADR 0733).
//!
//! Any executor can poll a future that [`crate::Calls`] returns: the window polls them on
//! the executor of gpui, which has no Tokio runtime. Such a future only waits for a channel
//! or for a task; the work that needs Tokio (its timers, its tasks, its connections and the
//! calls of the call library) runs in tasks of the runtime of the client.
//!
//! The functions of the device interfaces that return a future
//! ([`crate::share::ScreenCapture`], [`crate::camera::CameraCapture`] and
//! [`crate::permission::MicrophonePermission`]) are called, and their futures polled, only
//! by this crate and only in tasks of that runtime. Their implementations may use Tokio.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::runtime::Handle;
use tokio::task::{JoinError, JoinHandle};

/// Runs `work` as a task of `runtime`. The returned future only waits for the task, so any
/// executor can poll it. Dropping it ends the task.
pub(crate) fn on_runtime<T: Send + 'static>(
    runtime: &Handle,
    work: impl Future<Output = T> + Send + 'static,
) -> Task<T> {
    Task(runtime.spawn(work))
}

/// A task of the runtime of the client, ended when this value is dropped.
pub(crate) struct Task<T>(JoinHandle<T>);

impl<T> Future for Task<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx)
    }
}

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        // A task that has ended is not changed by this.
        self.0.abort();
    }
}

/// Panics when the caller does not run inside a Tokio runtime. The test devices call it in
/// each of their futures: a test fails at the place where the crate polls the future of a
/// device interface outside the runtime of the client, where the devices of the systems
/// would fail too.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn require_runtime(future: &str) {
    if let Err(error) = Handle::try_current() {
        // The text of the error is the one of the panic of a Tokio timer outside a runtime.
        panic!(
            "{future}: {error}; the futures of a device interface are polled on the runtime \
             of the client (ADR 0733)"
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    /// How long a step of these tests waits, on channels of the standard library.
    const LIMIT: Duration = Duration::from_secs(10);

    /// Sends on its channel when it is dropped.
    struct Dropped(mpsc::Sender<()>);

    impl Drop for Dropped {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    #[test]
    fn dropping_the_future_ends_the_task() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap();
        let (started, has_started) = mpsc::channel();
        let (dropped, was_dropped) = mpsc::channel();
        let task = on_runtime(runtime.handle(), async move {
            let _held = Dropped(dropped);
            let _ = started.send(());
            std::future::pending::<()>().await;
        });
        has_started.recv_timeout(LIMIT).expect("the task started");
        assert!(was_dropped.try_recv().is_err());
        drop(task);
        was_dropped
            .recv_timeout(LIMIT)
            .expect("the task ended when its future was dropped");
    }
}
