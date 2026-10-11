//! Where the network work of the client runs.
//!
//! The HTTP client needs a tokio runtime. A user interface with its own executor (gpui)
//! has none, so every public operation runs as a task on a runtime that the client holds,
//! and the future that the caller gets only waits for that task. Such a future can be
//! polled by any executor. An operation that has started runs to its end when the caller
//! drops the future, so a state change is never left half done.

use std::future::Future;
use std::io;
use std::sync::Arc;

#[derive(Clone)]
pub struct Runtime {
    handle: tokio::runtime::Handle,
    _owned: Option<Arc<Owned>>,
}

struct Owned(Option<tokio::runtime::Runtime>);

impl Drop for Owned {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

/// The task of an operation ended without a result: the runtime is shutting down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Stopped;

impl Runtime {
    /// A runtime of its own with two worker threads.
    pub fn new() -> io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("junto-client")
            .enable_all()
            .build()?;
        Ok(Self {
            handle: runtime.handle().clone(),
            _owned: Some(Arc::new(Owned(Some(runtime)))),
        })
    }

    /// The runtime of the caller when the caller runs inside one, a new one otherwise.
    pub fn current_or_new() -> io::Result<Self> {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => Ok(Self::from_handle(handle)),
            Err(_) => Self::new(),
        }
    }

    pub fn from_handle(handle: tokio::runtime::Handle) -> Self {
        Self {
            handle,
            _owned: None,
        }
    }

    /// The handle of the runtime, for a part of the client that keeps tasks of its own on
    /// it, such as the media engine.
    pub fn handle(&self) -> &tokio::runtime::Handle {
        &self.handle
    }

    pub(crate) fn run<F>(
        &self,
        work: F,
    ) -> impl Future<Output = Result<F::Output, Stopped>> + Send + 'static + use<F>
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        let task = self.handle.spawn(work);
        async move { task.await.map_err(|_| Stopped) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn work_runs_on_the_runtime_and_any_executor_can_wait_for_it() {
        let runtime = Runtime::new().unwrap();
        let waiting = runtime.run(async {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            tokio::runtime::Handle::try_current().is_ok()
        });
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || sender.send(block_on_plain(waiting)).unwrap());
        assert_eq!(
            receiver
                .recv_timeout(std::time::Duration::from_secs(10))
                .unwrap(),
            Ok(true)
        );
    }

    #[test]
    fn started_work_finishes_when_the_caller_drops_the_future() {
        let runtime = Runtime::new().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        drop(runtime.run(async move {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            sender.send(()).unwrap();
        }));
        receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
    }

    #[tokio::test]
    async fn a_runtime_from_the_caller_is_not_shut_down_with_the_client() {
        let runtime = Runtime::current_or_new().unwrap();
        assert_eq!(runtime.run(async { 7 }).await, Ok(7));
        drop(runtime);
        assert_eq!(tokio::spawn(async { 8 }).await.unwrap(), 8);
    }

    /// An executor that knows nothing about tokio.
    fn block_on_plain<F: Future>(future: F) -> F::Output {
        use std::sync::mpsc::{Sender, channel};
        use std::task::{Context, Poll, Wake, Waker};

        struct Notify(Sender<()>);
        impl Wake for Notify {
            fn wake(self: Arc<Self>) {
                let _ = self.0.send(());
            }
        }

        let (sender, receiver) = channel();
        let waker = Waker::from(Arc::new(Notify(sender)));
        let mut context = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            receiver.recv().unwrap();
        }
    }
}
