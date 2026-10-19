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
        message.starts_with("this did not end within 50ms"),
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
