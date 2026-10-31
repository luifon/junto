//! Cancellation and cleanup results shared by capture workers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, PoisonError};

use super::CaptureError;

/// A call that makes a waiting thread of a capture look at the state of the capture.
type Waker = Box<dyn Fn() + Send + Sync>;

/// The state of one capture that its threads share.
#[derive(Default)]
pub(crate) struct Life {
    stopped: AtomicBool,
    /// The owner of the capture stopped it; the sink is not told an end.
    pub cancelled: AtomicBool,
    failure: Mutex<Option<CaptureError>>,
    cleanup_failure: Mutex<Option<CaptureError>>,
    /// Called when the capture stops, so that no thread waits for a time limit to see it.
    wakers: Mutex<Vec<Waker>>,
}
impl Life {
    pub fn stopped(&self) -> bool {
        self.stopped.load(Ordering::Acquire)
    }
    /// Stops the capture. The first stop runs each waker once and drops it; a later stop
    /// runs none, so that no waker acts on a thread that has ended since.
    pub fn stop(&self) {
        let mut held = self.wakers.lock().unwrap_or_else(PoisonError::into_inner);
        self.stopped.store(true, Ordering::Release);
        let wakers = std::mem::take(&mut *held);
        drop(held);
        for wake in wakers {
            wake();
        }
    }
    /// Calls `wake` when the capture stops, or now when it has stopped.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    pub fn on_stop(&self, wake: impl Fn() + Send + Sync + 'static) {
        let mut wakers = self.wakers.lock().unwrap_or_else(PoisonError::into_inner);
        if self.stopped() {
            drop(wakers);
            wake();
        } else {
            wakers.push(Box::new(wake));
        }
    }
    pub fn fail(&self, failure: CaptureError) {
        let mut held = self.failure.lock().unwrap_or_else(PoisonError::into_inner);
        if !self.stopped() && held.is_none() {
            *held = Some(failure);
            drop(held);
            self.stop();
        }
    }
    pub fn cleanup_failed(&self, failure: CaptureError) {
        tracing::error!(%failure, "screen capture cleanup failed");
        let mut held = self
            .cleanup_failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if held.is_none() {
            *held = Some(failure);
        }
    }
    pub fn cleanup_result(&self) -> Result<(), CaptureError> {
        self.cleanup_failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .map_or(Ok(()), Err)
    }
    #[cfg(any(target_os = "linux", target_os = "windows", test))]
    pub fn result(&self) -> Result<(), CaptureError> {
        self.failure
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .map_or(Ok(()), Err)
    }
}
