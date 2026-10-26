//! The time limit of a live test (ADR 0904).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::limit::{limited_for, step};

/// Tells when it is dropped.
struct Dropped(Arc<AtomicBool>);

impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

#[tokio::test]
async fn a_body_that_does_not_end_is_dropped_and_fails_with_its_last_steps() {
    let dropped = Arc::new(AtomicBool::new(false));
    let held = Dropped(dropped.clone());
    let failed = tokio::spawn(async move {
        step("waits for an answer that does not come");
        limited_for(Duration::from_millis(50), async move {
            let _held = held;
            std::future::pending::<()>().await;
        })
        .await;
    })
    .await
    .unwrap_err();
    assert!(dropped.load(Ordering::SeqCst));
    let message = failed.into_panic();
    let message = message.downcast_ref::<String>().unwrap();
    assert!(
        message.contains("this did not end within 50ms"),
        "{message}"
    );
    assert!(
        message.contains("waits for an answer that does not come"),
        "{message}"
    );
}

/// A body that waited for a lock that other tests held restarts its limit when it holds the
/// lock: it ends after more than its limit in all, and its steps after the restart have the
/// whole limit.
#[tokio::test(start_paused = true)]
async fn a_restarted_limit_does_not_count_the_time_before_the_restart() {
    let limit = Duration::from_secs(300);
    let output = limited_for(limit, async {
        step("waits for a lock that another test holds");
        tokio::time::sleep(Duration::from_secs(250)).await;
        crate::limit::restart_limit();
        tokio::time::sleep(Duration::from_secs(250)).await;
        7
    })
    .await;
    assert_eq!(output, 7);
    let failed = tokio::spawn(limited_for(limit, async {
        tokio::time::sleep(Duration::from_secs(250)).await;
        crate::limit::restart_limit();
        std::future::pending::<()>().await;
    }))
    .await
    .unwrap_err();
    assert!(failed.is_panic());
    // Outside a limited body a restart does nothing.
    crate::limit::restart_limit();
}

#[tokio::test]
async fn a_body_that_ends_gives_its_output() {
    assert_eq!(limited_for(Duration::from_secs(5), async { 7 }).await, 7);
}

/// Set for a run of this test binary by a test below, which runs one test of this file
/// in a process of its own.
const CHILD: &str = "JUNTO_LIMIT_TEST_CHILD";

/// Runs one ignored test of this file in a new process of this binary. Returns its exit
/// code, its standard error and how long it ran.
fn run_child(test: &str) -> (Option<i32>, String, std::time::Duration) {
    let started = std::time::Instant::now();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            test,
            "--ignored",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(CHILD, "1")
        .output()
        .unwrap();
    (
        output.status.code(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        started.elapsed(),
    )
}

/// Waits in its drop for longer than every limit of these tests.
struct Stuck;

impl Drop for Stuck {
    fn drop(&mut self) {
        std::thread::sleep(Duration::from_secs(20));
    }
}

#[tokio::test]
#[ignore = "run in its own process by a_cleanup_that_does_not_end_ends_the_test_process_after_the_report"]
async fn child_whose_cleanup_does_not_end() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    step("waits for an answer that does not come");
    crate::limit::limited_within(
        Duration::from_millis(50),
        Duration::from_millis(300),
        async {
            let _stuck = Stuck;
            std::future::pending::<()>().await;
        },
    )
    .await;
}

#[test]
fn a_cleanup_that_does_not_end_ends_the_test_process_after_the_report() {
    let (code, errors, took) = run_child("limit_tests::child_whose_cleanup_does_not_end");
    assert_eq!(code, Some(crate::limit::LIMIT_EXIT_CODE), "{errors}");
    assert!(took < Duration::from_secs(10), "it took {took:?}: {errors}");
    let reported = errors.find("did not end within 50ms").expect(&errors);
    assert!(
        errors.contains("waits for an answer that does not come"),
        "{errors}"
    );
    let ended = errors
        .find("this and its cleanup did not end within 350ms")
        .expect(&errors);
    assert!(reported < ended, "{errors}");
}

#[test]
#[ignore = "run in its own process by a_test_that_is_not_one_future_ends_the_test_process_after_its_limit"]
fn child_that_is_not_one_future_and_does_not_end() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let _limit = crate::limit::watch(Duration::from_millis(100));
    step("reads a pipe whose writer stays open");
    std::thread::sleep(Duration::from_secs(20));
}

#[test]
fn a_test_that_is_not_one_future_ends_the_test_process_after_its_limit() {
    let (code, errors, took) =
        run_child("limit_tests::child_that_is_not_one_future_and_does_not_end");
    assert_eq!(code, Some(crate::limit::LIMIT_EXIT_CODE), "{errors}");
    assert!(took < Duration::from_secs(10), "it took {took:?}: {errors}");
    assert!(errors.contains("did not end within 100ms"), "{errors}");
    assert!(
        errors.contains("reads a pipe whose writer stays open"),
        "{errors}"
    );
}

#[tokio::test]
#[ignore = "run in its own process by a_cleanup_after_the_end_of_the_body_is_bounded_too"]
async fn child_whose_body_ends_and_whose_cleanup_does_not_end() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    step("ends at once");
    crate::limit::limited_within(Duration::from_secs(5), Duration::from_millis(300), async {
        let _stuck = Stuck;
    })
    .await;
}

#[tokio::test]
#[ignore = "run in its own process by a_cleanup_after_the_end_of_the_body_is_bounded_too"]
async fn child_whose_body_fails_and_whose_cleanup_does_not_end() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    step("fails at once");
    crate::limit::limited_within(Duration::from_secs(5), Duration::from_millis(300), async {
        let _stuck = Stuck;
        panic!("the body fails");
    })
    .await;
}

#[test]
fn a_cleanup_after_the_end_of_the_body_is_bounded_too() {
    for child in [
        "limit_tests::child_whose_body_ends_and_whose_cleanup_does_not_end",
        "limit_tests::child_whose_body_fails_and_whose_cleanup_does_not_end",
    ] {
        let (code, errors, took) = run_child(child);
        assert_eq!(
            code,
            Some(crate::limit::LIMIT_EXIT_CODE),
            "{child}: {errors}"
        );
        assert!(
            took < Duration::from_secs(12),
            "{child} took {took:?}: {errors}"
        );
        assert!(
            errors.contains("this and its cleanup did not end within 5.3s"),
            "{child}: {errors}"
        );
    }
}

/// True while a process with this number exists.
#[cfg(unix)]
fn exists(process: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &process.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
#[ignore = "run in its own process by a_forced_end_also_ends_the_processes_that_the_tests_started"]
fn child_with_a_started_process_that_does_not_end() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    let mut started = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let _adopted = crate::limit::adopt(started.id());
    eprintln!("started process {}", started.id());
    let _limit = crate::limit::watch(Duration::from_millis(300));
    std::thread::sleep(Duration::from_secs(20));
    let _ = started.kill();
    let _ = started.wait();
}

#[cfg(unix)]
#[test]
fn a_forced_end_also_ends_the_processes_that_the_tests_started() {
    // The started process holds the pipes of the child, so the child ends for its parent
    // only when the started process has ended.
    let (code, errors, took) =
        run_child("limit_tests::child_with_a_started_process_that_does_not_end");
    assert_eq!(code, Some(crate::limit::LIMIT_EXIT_CODE), "{errors}");
    assert!(took < Duration::from_secs(10), "it took {took:?}: {errors}");
    let process: u32 = errors
        .lines()
        .find_map(|line| line.strip_prefix("started process "))
        .expect(&errors)
        .trim()
        .parse()
        .unwrap();
    let ended = (0..100).any(|_| {
        std::thread::sleep(Duration::from_millis(20));
        !exists(process)
    });
    if !ended {
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &process.to_string()])
            .status();
    }
    assert!(ended, "the started process {process} still runs: {errors}");
}

#[test]
#[ignore = "run in its own process by a_forced_end_does_not_wait_for_a_standard_error_that_nobody_reads"]
fn child_whose_standard_error_is_full() {
    if std::env::var_os(CHILD).is_none() {
        return;
    }
    // A writer fills the pipe of the standard error, which the parent does not read, and
    // waits there.
    std::thread::spawn(|| {
        use std::io::Write as _;
        let _ = std::io::stderr().write_all(&vec![b'.'; 1 << 20]);
    });
    std::thread::sleep(Duration::from_millis(300));
    let _limit = crate::limit::watch(Duration::from_millis(100));
    std::thread::sleep(Duration::from_secs(20));
}

#[test]
fn a_forced_end_does_not_wait_for_a_standard_error_that_nobody_reads() {
    let started = std::time::Instant::now();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "limit_tests::child_whose_standard_error_is_full",
            "--ignored",
            "--nocapture",
            "--test-threads",
            "1",
        ])
        .env(CHILD, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut ended_in_time = false;
    while started.elapsed() < Duration::from_secs(12) {
        if child.try_wait().unwrap().is_some() {
            ended_in_time = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !ended_in_time {
        let _ = child.kill();
    }
    let status = child.wait().unwrap();
    assert!(ended_in_time, "the child did not end within 12 s");
    assert_eq!(status.code(), Some(crate::limit::LIMIT_EXIT_CODE));
}
