//! The order of the mute requests of the vendored call library (ADR 0733). A mute and an
//! unmute of a local track each send a request from a task of their own; requests that wait
//! together for a restored connection must still reach the media server in the order in
//! which the track was muted and unmuted, or the server holds the track as muted while the
//! device sends.
//!
//! The ordering is private to the call library, so this test compiles the same source file.
//! The file uses the standard library and Tokio only.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

#[allow(dead_code)]
#[path = "../../../vendor/livekit-0.9.4/src/room/participant/in_order.rs"]
mod in_order;

use in_order::InOrder;

type Sent = Arc<Mutex<Vec<&'static str>>>;

fn record(sent: &Sent, request: &'static str) -> impl Future<Output = ()> + Send + 'static {
    let sent = sent.clone();
    async move { sent.lock().unwrap().push(request) }
}

/// The mute waits, as a request waits for a restored connection, and the unmute that was
/// given after it is ready at once. The tasks run on one thread in the order in which they
/// were started, so a task of the unmute that did not wait would send it first.
#[tokio::test(flavor = "current_thread")]
async fn an_unmute_does_not_overtake_a_mute_that_waits() {
    let order = InOrder::default();
    let sent: Sent = Arc::default();
    let (open, restored) = oneshot::channel::<()>();
    let waiting = sent.clone();
    order.run(async move {
        let _ = restored.await;
        waiting.lock().unwrap().push("mute");
    });
    order.run(record(&sent, "unmute"));
    // Both tasks run until they wait.
    for _ in 0..10 {
        tokio::task::yield_now().await;
    }
    let early = sent.lock().unwrap().clone();
    assert!(early.is_empty(), "sent before the mute was: {early:?}");
    open.send(()).unwrap();
    let (done, finished) = oneshot::channel();
    order.run(async move {
        let _ = done.send(());
    });
    finished.await.unwrap();
    let sent = sent.lock().unwrap().clone();
    assert_eq!(sent, ["mute", "unmute"]);
}

/// Many requests on a runtime with several threads, each waiting a different time, arrive
/// in the order in which they were given.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_arrive_in_the_order_in_which_they_were_given() {
    let order = InOrder::default();
    let sent: Arc<Mutex<Vec<usize>>> = Arc::default();
    for index in 0..200 {
        let sent = sent.clone();
        order.run(async move {
            // A different number of waits for each request: 0 to 6.
            for _ in 0..(index * 5 + 3) % 7 {
                tokio::task::yield_now().await;
            }
            sent.lock().unwrap().push(index);
        });
    }
    let (done, finished) = oneshot::channel();
    order.run(async move {
        let _ = done.send(());
    });
    finished.await.unwrap();
    let sent = sent.lock().unwrap().clone();
    assert_eq!(sent, (0..200).collect::<Vec<_>>());
}

/// A request whose task ends without finishing does not hold back the requests after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_request_whose_task_panics_does_not_hold_back_the_next() {
    let order = InOrder::default();
    let sent: Sent = Arc::default();
    order.run(async { panic!("the request failed") });
    order.run(record(&sent, "unmute"));
    let (done, finished) = oneshot::channel();
    order.run(async move {
        let _ = done.send(());
    });
    finished.await.unwrap();
    let sent = sent.lock().unwrap().clone();
    assert_eq!(sent, ["unmute"]);
}
