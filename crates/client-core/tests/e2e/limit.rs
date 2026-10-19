//! The time limit of one live test (ADR 0904). A live test against PostgreSQL and LiveKit
//! waits for the network, for other processes and for locks that it shares with the other
//! tests of its binary. A wait without an end would hold the run and the lock of the
//! machine. The body of each async live test runs under [`limited`]: when it has not ended
//! after [`TEST_LIMIT`], the failure is written to the standard error of the process, the
//! body is dropped, which ends its servers and calls, and the test fails with its last
//! steps. A cleanup that has not ended after [`CLEANUP_LIMIT`] ends the test process. A
//! test that is not one future holds a [`watch`], which ends the test process when the
//! test has not ended after its limit.

use std::collections::VecDeque;
use std::future::Future;
use std::io::Write as _;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The longest time that the body of one live test may take, with its waits for what it
/// shares with the other tests of its binary. All 40 tests of calls of the media engine
/// take about three minutes together.
pub const TEST_LIMIT: Duration = Duration::from_secs(300);

/// The longest time that the cleanup of a test that did not end may take.
pub const CLEANUP_LIMIT: Duration = Duration::from_secs(60);

/// The exit code of a test process that a limit ended.
pub const LIMIT_EXIT_CODE: i32 = 124;

/// How many steps a failure names.
const KEPT_STEPS: usize = 12;

/// The steps that one test began, with the time at which each began.
type Steps = Arc<Mutex<VecDeque<(Instant, String)>>>;

thread_local! {
    /// The steps of the test of this thread. A test runs its body on its own thread; a
    /// watch reads them from its own thread.
    static STEPS: Steps = Steps::default();
}

fn steps_of_this_thread() -> Steps {
    STEPS.with(Arc::clone)
}

/// Notes that the test of this thread begins a step.
pub fn step(what: impl Into<String>) {
    let steps = steps_of_this_thread();
    let mut steps = steps.lock().unwrap_or_else(PoisonError::into_inner);
    if steps.len() == KEPT_STEPS {
        steps.pop_front();
    }
    steps.push_back((Instant::now(), what.into()));
}

/// The last steps of the test of this thread, oldest first, each with the time since it
/// began.
pub fn last_steps() -> String {
    described(&steps_of_this_thread())
}

fn described(steps: &Steps) -> String {
    let steps = steps.lock().unwrap_or_else(PoisonError::into_inner);
    if steps.is_empty() {
        return "none noted".to_owned();
    }
    steps
        .iter()
        .map(|(began, what)| format!("\n  {what} (began {:.1?} ago)", began.elapsed()))
        .collect()
}

fn clear_steps() {
    steps_of_this_thread()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear();
}

/// The name of the test of this thread, as the test runner gives it.
fn this_test() -> String {
    std::thread::current().name().unwrap_or("a test").to_owned()
}

/// Writes to the standard error of the process, past the capture of the test runner: a
/// test that does not end never shows what it captured.
fn report(text: &str) {
    let mut errors = std::io::stderr().lock();
    let _ = writeln!(errors, "{text}");
    let _ = errors.flush();
}

/// Runs the body of a live test. A body that has not ended after [`TEST_LIMIT`] is
/// dropped, and the test fails with its last steps.
pub async fn limited<F: Future>(body: F) -> F::Output {
    clear_steps();
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
    limited_within(limit, CLEANUP_LIMIT, body).await
}

/// Runs a body with a limit. When the limit passes, the failure is reported first, then the
/// body is dropped under `cleanup_limit`, and then the test fails.
pub async fn limited_within<F: Future>(
    limit: Duration,
    cleanup_limit: Duration,
    body: F,
) -> F::Output {
    let mut body = Box::pin(body);
    if let Ok(output) = tokio::time::timeout(limit, body.as_mut()).await {
        return output;
    }
    let failure = format!(
        "{}: this did not end within {limit:?}; the last steps of the test:{}",
        this_test(),
        last_steps()
    );
    report(&format!("{failure}\nIts cleanup begins."));
    let cleanup = Watch::start(
        cleanup_limit,
        format!(
            "{}: its cleanup did not end within {cleanup_limit:?}",
            this_test()
        ),
    );
    drop(body);
    drop(cleanup);
    panic!("{failure}");
}

/// The limit of a test that is not one future, such as a test that waits for processes and
/// reads their pipes. When it has not been dropped after its limit, it reports the test and
/// its last steps, and ends the test process.
pub struct Watch {
    ended: Option<mpsc::Sender<()>>,
    watching: Option<std::thread::JoinHandle<()>>,
}

/// Starts the limit of the test of this thread.
pub fn watch(limit: Duration) -> Watch {
    Watch::start(
        limit,
        format!("{}: this did not end within {limit:?}", this_test()),
    )
}

impl Watch {
    fn start(limit: Duration, failure: String) -> Self {
        let steps = steps_of_this_thread();
        let (ended, ending) = mpsc::channel::<()>();
        let watching = std::thread::spawn(move || {
            if let Err(mpsc::RecvTimeoutError::Timeout) = ending.recv_timeout(limit) {
                report(&format!(
                    "{failure}; the last steps of the test:{}\nThe test process ends.",
                    described(&steps)
                ));
                std::process::exit(LIMIT_EXIT_CODE);
            }
        });
        Self {
            ended: Some(ended),
            watching: Some(watching),
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        drop(self.ended.take());
        if let Some(watching) = self.watching.take() {
            let _ = watching.join();
        }
    }
}
