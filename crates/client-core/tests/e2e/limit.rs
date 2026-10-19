//! The time limit of one live test (ADR 0904). A live test against PostgreSQL and LiveKit
//! waits for the network, for other processes and for locks that it shares with the other
//! tests of its binary. A wait without an end would hold the run and the lock of the
//! machine. The body of each live test runs under [`limited`]: when it has not ended after
//! [`TEST_LIMIT`], the body is dropped, which ends its servers and calls, and the test fails
//! with its last steps.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::time::{Duration, Instant};

/// The longest time that the body of one live test may take, with its waits for what it
/// shares with the other tests of its binary. All 40 tests of calls of the media engine
/// take about three minutes together.
pub const TEST_LIMIT: Duration = Duration::from_secs(300);

/// How many steps a failure names.
const KEPT_STEPS: usize = 12;

thread_local! {
    /// The steps that the test of this thread began, with the time at which each began.
    /// A test runs its body on its own thread, so these are the steps of one test.
    static STEPS: RefCell<VecDeque<(Instant, String)>> = const { RefCell::new(VecDeque::new()) };
}

/// Notes that the test of this thread begins a step.
pub fn step(what: impl Into<String>) {
    STEPS.with(|steps| {
        let mut steps = steps.borrow_mut();
        if steps.len() == KEPT_STEPS {
            steps.pop_front();
        }
        steps.push_back((Instant::now(), what.into()));
    });
}

/// The last steps of the test of this thread, oldest first, each with the time since it
/// began.
pub fn last_steps() -> String {
    STEPS.with(|steps| {
        let steps = steps.borrow();
        if steps.is_empty() {
            return "none noted".to_owned();
        }
        steps
            .iter()
            .map(|(began, what)| format!("\n  {what} (began {:.1?} ago)", began.elapsed()))
            .collect()
    })
}

/// Runs the body of a live test. A body that has not ended after [`TEST_LIMIT`] is
/// dropped, and the test fails with its last steps.
pub async fn limited<F: Future>(body: F) -> F::Output {
    STEPS.with(|steps| steps.borrow_mut().clear());
    limited_step(body).await
}

/// Runs one step of a test whose body is not one future, as a test of the window is. A
/// step that has not ended after [`TEST_LIMIT`] is dropped, and the test fails with its
/// last steps.
pub async fn limited_step<F: Future>(body: F) -> F::Output {
    limited_for(TEST_LIMIT, body).await
}

/// Runs one step with its own limit, for a step that takes longer than a live test may.
pub async fn limited_for<F: Future>(limit: Duration, body: F) -> F::Output {
    // The body is dropped at the end of this statement, before the test fails.
    let ended = tokio::time::timeout(limit, body).await;
    match ended {
        Ok(output) => output,
        Err(_) => panic!(
            "this did not end within {limit:?}; the last steps of the test:{}",
            last_steps()
        ),
    }
}
