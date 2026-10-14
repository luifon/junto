//! The newest captured picture, for a reader that can be slower than the capture.
//!
//! A screen sends a picture only when it changes. A queue that refuses a picture when it is
//! full would keep an older picture as the last one; here a new picture replaces the one
//! that still waits.

use std::sync::{Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

pub(super) struct Latest<T> {
    waiting: Mutex<Option<T>>,
    changed: Condvar,
}

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self {
            waiting: Mutex::new(None),
            changed: Condvar::new(),
        }
    }
}

impl<T> Latest<T> {
    /// Replaces the value that waits. Never blocks on the reader.
    pub fn put(&self, value: T) {
        *self.waiting.lock().unwrap_or_else(PoisonError::into_inner) = Some(value);
        self.changed.notify_one();
    }

    /// The newest value, or nothing once `stopped` holds.
    pub fn take(&self, stopped: impl Fn() -> bool) -> Option<T> {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if stopped() {
                return None;
            }
            if let Some(value) = waiting.take() {
                return Some(value);
            }
            // The limit bounds the wait for a stop that no call of `wake` announced.
            waiting = self
                .changed
                .wait_timeout(waiting, Duration::from_millis(100))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Waits until a time; a value that arrives does not end the wait. False once `stopped`
    /// holds.
    pub fn pause(&self, until: Instant, stopped: impl Fn() -> bool) -> bool {
        let mut waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if stopped() {
                return false;
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return true;
            }
            waiting = self
                .changed
                .wait_timeout(waiting, left.min(Duration::from_millis(100)))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Makes a waiting reader check `stopped` now.
    pub fn wake(&self) {
        self.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_picture_replaces_the_ones_that_no_reader_took() {
        let latest = Latest::default();
        for picture in 1..=5 {
            latest.put(picture);
        }
        assert_eq!(latest.take(|| false), Some(5));
        latest.put(6);
        assert_eq!(latest.take(|| false), Some(6));
        assert_eq!(latest.take(|| true), None);
    }

    #[test]
    fn a_waiting_reader_gets_a_picture_that_arrives_later_and_ends_with_a_stop() {
        let latest = std::sync::Arc::new(Latest::default());
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = std::thread::spawn({
            let (latest, stopped) = (latest.clone(), stopped.clone());
            move || {
                let first = latest.take(|| stopped.load(std::sync::atomic::Ordering::Acquire));
                let second = latest.take(|| stopped.load(std::sync::atomic::Ordering::Acquire));
                (first, second)
            }
        });
        latest.put(1);
        while latest
            .waiting
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
        {
            std::thread::yield_now();
        }
        stopped.store(true, std::sync::atomic::Ordering::Release);
        latest.wake();
        assert!(matches!(reader.join(), Ok((Some(1), None))));
    }
}
