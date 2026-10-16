// Copyright 2025 LiveKit, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// Modified: this file is not in the published livekit 0.9.4. It is an extract of
// `rtc_session.rs` of that version (the `NegotiationState` enum), changed so that one state
// under one lock decides the publisher negotiation queue.

//! The decisions of the publisher negotiation queue of the fast publish mode, apart from the
//! session so that a test can order them. The file uses the standard library only.
//!
//! One state, changed under one lock, decides whether a task negotiates. A task ends only
//! after it has set the state to idle under that lock, so a request that comes later finds
//! the queue idle and starts a new task. A separate flag for a running task let a request
//! that came between the two steps of the end of a task find the queue idle and the task
//! still running: it marked a retry that no task made, and every later request was dropped.

use std::sync::{Mutex, PoisonError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NegotiationState {
    /// No task runs.
    Idle,
    /// A task negotiates.
    InProgress,
    /// A task negotiates, and negotiates again after it.
    PendingRetry,
}

#[derive(Debug)]
pub(crate) struct NegotiationQueue {
    state: Mutex<NegotiationState>,
}

impl Default for NegotiationQueue {
    fn default() -> Self {
        Self {
            state: Mutex::new(NegotiationState::Idle),
        }
    }
}

impl NegotiationQueue {
    fn lock(&self) -> std::sync::MutexGuard<'_, NegotiationState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A negotiation is needed. True when the caller must start the negotiation task.
    pub(crate) fn request(&self) -> bool {
        let mut state = self.lock();
        match *state {
            NegotiationState::Idle => {
                *state = NegotiationState::InProgress;
                true
            }
            NegotiationState::InProgress => {
                *state = NegotiationState::PendingRetry;
                false
            }
            NegotiationState::PendingRetry => false,
        }
    }

    /// The task has finished one negotiation. True when it must negotiate again. False when
    /// the queue is idle: the task ends, and owns nothing of the queue from now on.
    pub(crate) fn finished(&self) -> bool {
        let mut state = self.lock();
        match *state {
            NegotiationState::PendingRetry => {
                *state = NegotiationState::InProgress;
                true
            }
            NegotiationState::Idle | NegotiationState::InProgress => {
                *state = NegotiationState::Idle;
                false
            }
        }
    }

    #[cfg(test)]
    fn state(&self) -> NegotiationState {
        *self.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs the task of the queue as the session runs it: negotiates while `finished` asks
    /// for it, then ends. Returns the number of negotiations.
    fn run_task(queue: &NegotiationQueue) -> usize {
        let mut negotiations = 1;
        while queue.finished() {
            negotiations += 1;
        }
        negotiations
    }

    #[test]
    fn a_request_without_a_task_starts_one() {
        let queue = NegotiationQueue::default();
        assert!(queue.request());
        assert_eq!(run_task(&queue), 1);
        assert_eq!(queue.state(), NegotiationState::Idle);
        assert!(queue.request(), "a later request starts a new task");
    }

    #[test]
    fn requests_during_a_negotiation_give_one_more_negotiation() {
        let queue = NegotiationQueue::default();
        assert!(queue.request());
        assert!(!queue.request());
        assert!(!queue.request());
        assert_eq!(run_task(&queue), 2);
        assert_eq!(queue.state(), NegotiationState::Idle);
    }

    #[test]
    fn a_request_while_the_task_ends_is_negotiated() {
        let queue = NegotiationQueue::default();
        assert!(queue.request());
        // The task has finished its last negotiation and runs the code after its loop.
        assert!(!queue.finished());
        let started = queue.request();
        // The request is negotiated by a new task. Without it, every later request finds a
        // retry that no task will make, and no negotiation happens again.
        assert!(
            started,
            "the request was not negotiated, and the queue is {:?}",
            queue.state()
        );
        assert_eq!(run_task(&queue), 1);
        assert!(queue.request(), "a later request starts a new task");
    }

    #[test]
    fn requests_from_many_threads_are_all_negotiated() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let queue = Arc::new(NegotiationQueue::default());
        let tasks = Arc::new(AtomicUsize::new(0));
        let mut threads = Vec::new();
        for _ in 0..8 {
            let (queue, tasks) = (queue.clone(), tasks.clone());
            threads.push(std::thread::spawn(move || {
                for _ in 0..10_000 {
                    if queue.request() {
                        tasks.fetch_add(1, Ordering::Relaxed);
                        let task = queue.clone();
                        std::thread::spawn(move || run_task(&task)).join().unwrap();
                    }
                }
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        // Every request has been negotiated or is held by a task: no task runs now, so the
        // queue is idle, and a new request starts a task.
        assert_eq!(queue.state(), NegotiationState::Idle);
        assert!(queue.request());
        assert!(tasks.load(Ordering::Relaxed) > 0);
    }
}
