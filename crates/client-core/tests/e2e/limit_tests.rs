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
        .find("its cleanup did not end within 300ms")
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
