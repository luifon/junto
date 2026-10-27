//! The time limit of one live test (ADR 0904). A live test against PostgreSQL and LiveKit
//! waits for the network, for other processes and for locks that it shares with the other
//! tests of its binary. A wait without an end would hold the run and the lock of the
//! machine.
//!
//! The body of each async live test runs under [`limited`]. A watch is armed before the
//! body starts and covers the body and every cleanup of it, on every way the test ends.
//! When the body has not ended after [`TEST_LIMIT`], the failure is reported, the body is
//! dropped, which ends its servers and calls, and the test fails with its last steps. When
//! the test and its cleanup have not ended after [`TEST_LIMIT`] and [`CLEANUP_LIMIT`], the
//! watch ends the processes that the tests started ([`adopt`]), reports the test, and ends
//! the test process with [`LIMIT_EXIT_CODE`]. A test that is not one future holds a
//! [`watch`] for its whole run.
//!
//! A test that waits for a lock that other tests hold, such as the media server of the
//! tests of calls, calls [`wait_for_others`] before the wait and [`restart_limit`] when it
//! holds the lock: the wait does not count, because the holders have limits of their own, so
//! a test fails for its own steps only.

use std::collections::VecDeque;
use std::future::Future;
use std::io::Write as _;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// The longest time that the body of one live test may take. A wait for a lock that other
/// tests hold under their own limits does not count ([`wait_for_others`]). All 40 tests of
/// calls of the media engine take about three minutes together.
pub const TEST_LIMIT: Duration = Duration::from_secs(300);

/// The longest time that the cleanup of a test may take after its body.
pub const CLEANUP_LIMIT: Duration = Duration::from_secs(60);

/// The exit code of a test process that a limit ended.
pub const LIMIT_EXIT_CODE: i32 = 124;

/// How long a forced end waits for the report and for the end of the started processes.
/// The standard error can be a pipe that nobody reads.
const FORCED_END_WAIT: Duration = Duration::from_secs(2);

/// How many steps a failure names.
const KEPT_STEPS: usize = 12;

/// The steps that one test began, with the time at which each began.
type Steps = Arc<Mutex<VecDeque<(Instant, String)>>>;

thread_local! {
    /// The steps of the test of this thread. A test runs its body on its own thread; a
    /// watch reads them from its own thread.
    static STEPS: Steps = Steps::default();
}

/// The processes that the tests of this process started and that would remain after a
/// forced end: call participants, Docker commands and helper programs. Each holds an
/// inherited copy of what the test process inherited, such as the lock of the machine.
static CHILDREN: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// The latest time at which the holders of the lock of the tests end, cleanup included: each
/// holder that takes the lock ([`restart_limit`]) moves it to its own limit and cleanup limit
/// from then, if that is later.
static HELD_UNTIL: Mutex<Option<tokio::time::Instant>> = Mutex::new(None);

/// When the limit of a body ends, on the clock of the runtime, which a test of a limit
/// pauses. While the body waits for others, its end is its whole limit after the start of the
/// wait, or the end of the holders of the lock when that is later; a restart gives it its
/// whole limit again.
struct Deadline {
    /// The end, and since when the body waits for others.
    state: Mutex<(tokio::time::Instant, Option<tokio::time::Instant>)>,
    limit: Duration,
    cleanup: Duration,
}

impl Deadline {
    fn new(limit: Duration, cleanup: Duration) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new((tokio::time::Instant::now() + limit, None)),
            limit,
            cleanup,
        })
    }

    /// The end as it stands now.
    fn at(&self) -> tokio::time::Instant {
        let (at, waits) = *self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match waits {
            Some(since) => {
                let held = *HELD_UNTIL.lock().unwrap_or_else(PoisonError::into_inner);
                held.map_or(since + self.limit, |held| held.max(since + self.limit))
            }
            None => at,
        }
    }

    fn waits(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .1
            .is_some()
    }

    fn wait_for_others(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.1.get_or_insert_with(tokio::time::Instant::now);
    }

    fn restart(&self) {
        let now = tokio::time::Instant::now();
        *self.state.lock().unwrap_or_else(PoisonError::into_inner) = (now + self.limit, None);
        let mut held = HELD_UNTIL.lock().unwrap_or_else(PoisonError::into_inner);
        let until = now + self.limit + self.cleanup;
        *held = Some(held.map_or(until, |held| held.max(until)));
    }
}

tokio::task_local! {
    /// The limit of the body that runs, for [`restart_limit`]. It follows the body from
    /// thread to thread.
    static RUNNING: Arc<Deadline>;
}

/// The test of this body waits for a lock that other tests hold under their own limits: the
/// time until [`restart_limit`] does not count while the lock changes holder within the limits
/// of its holders. A wait that outlasts both its own limit and the limits of the last holder
/// fails. Outside a limited body it does nothing.
pub fn wait_for_others() {
    let _ = RUNNING.try_with(|deadline| deadline.wait_for_others());
}

/// The test of this body holds the lock that it waited for: its whole limit starts again, and
/// the waiters of the lock wait at most until it ended with its cleanup. Outside a limited body
/// it does nothing.
pub fn restart_limit() {
    let _ = RUNNING.try_with(|deadline| deadline.restart());
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

/// Writes to the standard error of the process, past the capture of the test runner, on a
/// thread of its own: the standard error can be a pipe that nobody reads. The receiver
/// tells when the text is written.
fn report(text: String) -> mpsc::Receiver<()> {
    let (written, writing) = mpsc::channel();
    std::thread::spawn(move || {
        let mut errors = std::io::stderr().lock();
        let _ = writeln!(errors, "{text}");
        let _ = errors.flush();
        let _ = written.send(());
    });
    writing
}

/// A process that a test started, ended by a forced end of the test process. The record
/// ends when this is dropped, after the test has ended the process itself.
pub struct Adopted(u32);

/// Records a process that a test started.
pub fn adopt(process: u32) -> Adopted {
    CHILDREN
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(process);
    Adopted(process)
}

impl Drop for Adopted {
    fn drop(&mut self) {
        let mut children = CHILDREN.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(at) = children.iter().position(|child| *child == self.0) {
            children.remove(at);
        }
    }
}

/// Runs a command to its end, as `Command::output` does, with the process recorded.
pub fn output_of(command: &mut std::process::Command) -> std::io::Result<std::process::Output> {
    let child = command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let _adopted = adopt(child.id());
    child.wait_with_output()
}

/// Ends every recorded process, and waits for that a bounded time.
fn end_children() {
    let children: Vec<String> = CHILDREN
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .map(u32::to_string)
        .collect();
    if children.is_empty() {
        return;
    }
    let mut ending = if cfg!(windows) {
        let mut command = std::process::Command::new("taskkill");
        command.arg("/F").arg("/T");
        for child in &children {
            command.args(["/PID", child]);
        }
        command
    } else {
        let mut command = std::process::Command::new("kill");
        command.arg("-KILL").args(&children);
        command
    };
    let Ok(mut ending) = ending
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return;
    };
    let started = Instant::now();
    while matches!(ending.try_wait(), Ok(None)) && started.elapsed() < FORCED_END_WAIT {
        std::thread::sleep(Duration::from_millis(20));
    }
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

/// Runs a body with a limit, and every cleanup of it within `cleanup_limit` more. The watch
/// is armed first, so a cleanup that waits after the end of the body, after its failure or
/// after its limit ends the test process.
pub async fn limited_within<F: Future>(
    limit: Duration,
    cleanup_limit: Duration,
    body: F,
) -> F::Output {
    let deadline = Deadline::new(limit, cleanup_limit);
    let watch = Watch::start(
        deadline.clone(),
        cleanup_limit,
        format!(
            "{}: this and its cleanup did not end within {:?}",
            this_test(),
            limit + cleanup_limit
        ),
    );
    let mut body = Box::pin(RUNNING.scope(deadline.clone(), body));
    loop {
        let at = deadline.at();
        match tokio::time::timeout_at(at, body.as_mut()).await {
            Ok(output) => {
                drop(body);
                drop(watch);
                return output;
            }
            // The body restarted its limit while it ran.
            Err(_) if deadline.at() > at => {}
            Err(_) => break,
        }
    }
    let waited = if deadline.waits() {
        " while it waited for a lock of other tests that did not change holder within the \
         limits of its holders"
    } else {
        ""
    };
    let failure = format!(
        "{}: this did not end within {limit:?}{waited}; the last steps of the test:{}",
        this_test(),
        last_steps()
    );
    drop(report(format!("{failure}\nIts cleanup begins.")));
    drop(body);
    drop(watch);
    panic!("{failure}");
}

/// The limit of a test that is not one future, such as a test that waits for processes and
/// reads their pipes. When it has not been dropped after its limit, it ends the processes
/// that the tests started, reports the test and its last steps, and ends the test process.
pub struct Watch {
    ended: Option<mpsc::Sender<()>>,
    watching: Option<std::thread::JoinHandle<()>>,
}

/// Starts the limit of the test of this thread. Hold it for the whole test, cleanup
/// included.
pub fn watch(limit: Duration) -> Watch {
    Watch::start(
        Deadline::new(limit, Duration::ZERO),
        Duration::ZERO,
        format!("{}: this did not end within {limit:?}", this_test()),
    )
}

impl Watch {
    /// Watches until `extra` after `deadline`, which a restart of the body moves on.
    fn start(deadline: Arc<Deadline>, extra: Duration, failure: String) -> Self {
        let steps = steps_of_this_thread();
        let (ended, ending) = mpsc::channel::<()>();
        let watching = std::thread::spawn(move || {
            let mut over = false;
            while !over {
                let end = deadline.at().into_std() + extra;
                match ending.recv_timeout(end.saturating_duration_since(Instant::now())) {
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        over = deadline.at().into_std() + extra <= end;
                    }
                    _ => return,
                }
            }
            end_children();
            let reported = report(format!(
                "{failure}; the last steps of the test:{}\nThe processes that the tests \
                 started and the test process end.",
                described(&steps)
            ));
            let _ = reported.recv_timeout(FORCED_END_WAIT);
            std::process::exit(LIMIT_EXIT_CODE);
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
