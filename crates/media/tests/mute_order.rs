//! The order of the mute requests of the vendored call library (ADR 0733). A mute and an
//! unmute of a local track each send a request from a task of their own; requests that wait
//! together for a restored connection must still reach the media server in the order in
//! which the track was muted and unmuted, or the server holds the track as muted while the
//! device sends.
//!
//! The ordering is private to the call library, so this test compiles the same source file.
//! The file uses the standard library and Tokio only. The participant that uses it needs a
//! connected engine to run, so a test here reads its source: every mute request goes through
//! the one `InOrder` of the participant, and the request sent after a resume reads the state
//! in its turn. The media live test
//! `sound::mute_requests_that_wait_for_a_resume_reach_the_media_server_in_their_order` checks
//! with a media server the requests that the participant writes through it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::oneshot;

#[allow(dead_code)]
#[path = "../../../vendor/livekit-0.9.4/src/room/participant/in_order.rs"]
mod in_order;

use in_order::InOrder;

type Sent = Arc<Mutex<Vec<&'static str>>>;

/// How long a test waits for the requests that it gave to end. A request that is held back
/// forever fails the test instead of stopping the suite.
const LIMIT: Duration = Duration::from_secs(10);

/// Waits until the work given by `last` has ended.
async fn until_ended(last: oneshot::Receiver<()>) {
    tokio::time::timeout(LIMIT, last)
        .await
        .unwrap_or_else(|_| panic!("the requests did not end within {LIMIT:?}"))
        .unwrap();
}

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
    until_ended(finished).await;
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
    until_ended(finished).await;
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
    until_ended(finished).await;
    let sent = sent.lock().unwrap().clone();
    assert_eq!(sent, ["unmute"]);
}

/// The participant of the call library, which gives the mute requests of its local tracks to
/// one `InOrder`.
const PARTICIPANT: &str = include_str!("../../../vendor/livekit-0.9.4/src/room/participant/mod.rs");

/// The participant gives every mute request to its one `InOrder` and starts no task of its
/// own. A handler that started a task for each request, as the published version does, or a
/// new `InOrder` for each request, would send requests that wait together in any order.
#[test]
fn every_mute_request_of_the_participant_goes_through_its_one_in_order() {
    let code = without_comments(PARTICIPANT);
    let line = |at: usize| code[..at].matches('\n').count() + 1;
    assert!(
        !code.contains("spawn"),
        "the participant starts a task at line {}",
        line(code.find("spawn").unwrap())
    );
    assert!(code.contains("    mute_requests: Arc<in_order::InOrder>,\n"));
    assert_eq!(
        code.matches("InOrder").count(),
        1,
        "the participant names an InOrder outside the field that holds its own"
    );
    for (at, _) in code.match_indices("let mute_requests =") {
        assert!(
            code[at..].starts_with("let mute_requests = inner.mute_requests.clone();"),
            "the requests at line {} are not given to the InOrder of the participant",
            line(at)
        );
    }

    let chains: Vec<(usize, usize)> = code
        .match_indices("mute_requests.run(")
        .map(|(at, call)| {
            let open = at + call.len() - 1;
            (open, closing(&code, open))
        })
        .collect();
    let requests: Vec<usize> = code
        .match_indices(".mute_track(")
        .map(|(at, _)| at)
        .collect();
    for &request in &requests {
        assert!(
            chains
                .iter()
                .any(|&(open, close)| open < request && request < close),
            "the request at line {} is not given to the InOrder",
            line(request)
        );
    }
    let add = code.find("pub(super) fn add_publication(").unwrap();
    let body = code[add..].find('{').unwrap() + add;
    let body = body..closing(&code, body);
    for handler in ["publication.on_muted(", "publication.on_unmuted("] {
        let open = code[body.clone()].find(handler).unwrap() + body.start + handler.len() - 1;
        let close = closing(&code, open);
        assert_eq!(
            requests
                .iter()
                .filter(|&&request| open < request && request < close)
                .count(),
            1,
            "{handler} sends one request"
        );
    }
}

/// The room of the call library, which handles the resume of the session.
const ROOM: &str = include_str!("../../../vendor/livekit-0.9.4/src/room/mod.rs");

/// The local participant of the call library, which the room asks to send the mute states.
const LOCAL_PARTICIPANT: &str =
    include_str!("../../../vendor/livekit-0.9.4/src/room/participant/local_participant.rs");

/// After each resume the participant gives a request with the mute state of each local track
/// to its one `InOrder`, and each request reads the state when the requests given before it
/// have ended. A request written to the lost signal connection does not reach the media
/// server, so without the resend the server holds the state from before it. A state read
/// when the request is given would be the state before the changes whose requests still
/// wait, and the last request would send a state that is no longer current.
#[test]
fn the_mute_states_are_sent_after_each_resume_and_read_in_their_turn() {
    let room = without_comments(ROOM);
    assert!(
        room.contains("EngineEvent::Resumed(tx) => self.handle_resumed(tx),"),
        "the room does not handle the resume in handle_resumed"
    );
    let resumed = body(
        &room,
        "fn handle_resumed(self: &Arc<Self>, tx: oneshot::Sender<()>)",
    );
    assert!(
        room[resumed].contains("self.local_participant.send_mute_states();"),
        "the room does not send the mute states after a resume"
    );

    let local = without_comments(LOCAL_PARTICIPANT);
    let send = body(&local, "pub(crate) fn send_mute_states(&self)");
    assert_eq!(
        local[send.start + 1..send.end].trim(),
        "super::send_mute_states(&self.inner);",
        "the local participant does not give the mute states to its participant"
    );

    let code = without_comments(PARTICIPANT);
    let send = body(
        &code,
        "pub(super) fn send_mute_states(inner: &Arc<ParticipantInner>)",
    );
    let line = |at: usize| code[..at].matches('\n').count() + 1;
    let run = code[send.clone()]
        .find("inner.mute_requests.run(")
        .map(|at| send.start + at + "inner.mute_requests.run(".len() - 1)
        .expect("send_mute_states gives no request to the InOrder of the participant");
    let run = run..closing(&code, run);
    let reads: Vec<usize> = code[send.clone()]
        .match_indices("is_muted()")
        .map(|(at, _)| send.start + at)
        .collect();
    assert!(
        !reads.is_empty(),
        "send_mute_states does not read the mute state"
    );
    for read in reads {
        assert!(
            run.contains(&read),
            "the mute state at line {} is read before the requests given before it have ended",
            line(read)
        );
    }
    assert!(
        code[run].contains(".mute_track("),
        "send_mute_states sends no mute request"
    );
}

/// The braces of the body of the function whose signature is `signature`, from the opening
/// brace to the closing one.
fn body(code: &str, signature: &str) -> std::ops::Range<usize> {
    let at = code
        .find(signature)
        .unwrap_or_else(|| panic!("no function {signature}"));
    let open = code[at..].find('{').unwrap() + at;
    open..closing(code, open)
}

/// `source` with every `//` comment removed and every line kept, so that a comment that
/// names code is not read as code.
fn without_comments(source: &str) -> String {
    let mut code = String::with_capacity(source.len());
    for line in source.lines() {
        let mut in_string = false;
        let mut escaped = false;
        let mut end = line.len();
        for (at, character) in line.char_indices() {
            match character {
                _ if escaped => escaped = false,
                '\\' if in_string => escaped = true,
                '"' => in_string = !in_string,
                '/' if !in_string && line[at..].starts_with("//") => {
                    end = at;
                    break;
                }
                _ => {}
            }
        }
        code.push_str(&line[..end]);
        code.push('\n');
    }
    code
}

/// The position of the delimiter that closes the one at `open`. Delimiters inside string
/// literals do not count.
fn closing(code: &str, open: usize) -> usize {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (at, character) in code[open..].char_indices() {
        match character {
            _ if escaped => escaped = false,
            '\\' if in_string => escaped = true,
            '"' => in_string = !in_string,
            _ if in_string => {}
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return open + at;
                }
            }
            _ => {}
        }
    }
    panic!("the delimiter at {open} is not closed");
}
