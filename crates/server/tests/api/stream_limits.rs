//! The limits of the event stream, the health route, a stop of the server and two server
//! processes (ADR 0011, ADR 0012, ADR 0116).

use std::time::Duration;

use junto_protocol::error::ErrorCode;
use junto_protocol::events::{ClientMessage, CloseCode, Event, Level, ServerMessage};
use junto_protocol::server::Health;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::devices::link_second_device;
use crate::stream_support::{Frame, Stream};
use crate::support::{Member, TestServer};

fn milliseconds(count: i64) -> time::Duration {
    time::Duration::milliseconds(count)
}

async fn closed_with(stream: &mut Stream) -> Option<CloseCode> {
    stream.until_closed().await.1
}

async fn until_connections(server: &TestServer, member: &Member, count: usize) {
    for _ in 0..500 {
        if server.state.streams.connections_of(member.account_id) == count {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "{} connections stayed open",
        server.state.streams.connections_of(member.account_id)
    );
}

#[tokio::test]
async fn a_device_keeps_its_newest_connections_and_an_account_has_a_ceiling() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.connections_per_device = 2;
        config.limits.stream.connections_per_account = 3;
    })
    .await;
    let owner = server.owner().await;
    let second = link_second_device(&server, &owner).await;
    let other = server.join(&owner, "Other").await;

    let mut oldest = server.subscribed(&owner, Level::Full).await;
    let mut newer = server.stream(&owner).await;
    let mut newest = server.stream(&owner).await;
    assert_eq!(closed_with(&mut oldest).await, Some(CloseCode::Replaced));
    until_connections(&server, &owner, 2).await;

    let mut of_second = server.stream(&second).await;
    until_connections(&server, &owner, 3).await;
    let refused = server.open_stream(Some(&second.token)).await.err().unwrap();
    assert_eq!(
        refused,
        (StatusCode::TOO_MANY_REQUESTS, ErrorCode::TooManyConnections)
    );
    until_connections(&server, &owner, 3).await;
    let _of_other = server.stream(&other).await;

    drop(of_second.send_frame(Message::Close(None)).await);
    assert!(matches!(of_second.next().await, Frame::Closed(_)));
    until_connections(&server, &owner, 2).await;
    let _again = server.stream(&second).await;

    for stream in [&mut newer, &mut newest] {
        stream.subscribe(Level::Background, None).await;
    }
    assert_eq!(server.state.streams.connections(), 4);
}

#[tokio::test]
async fn the_number_of_opened_streams_per_account_is_limited() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.opens.attempts = 3;
    })
    .await;
    let owner = server.owner().await;
    let other = server.join(&owner, "Other").await;
    for _ in 0..3 {
        server.stream(&owner).await;
    }
    let refused = server.open_stream(Some(&owner.token)).await.err().unwrap();
    assert_eq!(
        refused,
        (StatusCode::TOO_MANY_REQUESTS, ErrorCode::RateLimited)
    );
    server.stream(&other).await;
    server.clock.advance(time::Duration::minutes(1));
    server.stream(&owner).await;
}

#[tokio::test]
async fn a_connection_that_does_not_subscribe_in_time_is_closed() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let mut subscribed = server.subscribed(&owner, Level::Full).await;
    server.tighten_streams(|limits| limits.subscribe_timeout = milliseconds(300));
    let mut silent = server.stream(&owner).await;

    assert_eq!(closed_with(&mut silent).await, Some(CloseCode::Idle));
    assert_eq!(subscribed.within(Duration::from_millis(400)).await, None);

    let behind = subscribed.cursor(u64::MAX);
    subscribed
        .send(&ClientMessage::Subscribe {
            level: Level::Full,
            cursor: Some(behind),
        })
        .await;
    assert!(matches!(
        subscribed.next().await,
        Frame::Message(ServerMessage::Resync { .. })
    ));
    assert_eq!(closed_with(&mut subscribed).await, Some(CloseCode::Idle));
}

/// Reads `stream` until `done` resolves; reading answers the pings of the server. Fails
/// when the stream gets a message or ends in that time.
async fn quiet_until<T>(stream: &mut Stream, done: impl Future<Output = T>) -> T {
    tokio::select! {
        biased;
        done = done => done,
        frame = stream.next() => panic!("the answering client got {frame:?}"),
    }
}

#[tokio::test]
async fn a_client_that_answers_no_ping_is_closed_as_idle() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let idle = milliseconds(2_000);
    server.tighten_streams(|limits| {
        limits.heartbeat = milliseconds(100);
        limits.idle_timeout = idle;
        limits.close_timeout = milliseconds(300);
        limits.client_frames.attempts = 10_000;
    });
    let mut silent = server.subscribed(&owner, Level::Full).await;
    silent.answer_nothing();
    let mut answering = server.subscribed(&owner, Level::Full).await;
    let subscribed = tokio::time::Instant::now();

    let closed = quiet_until(&mut answering, silent.until_closed()).await;
    assert_eq!(closed, (vec![], Some(CloseCode::Idle)));
    quiet_until(&mut answering, until_connections(&server, &owner, 1)).await;
    let twice_the_idle_time = tokio::time::sleep_until(subscribed + 2 * idle.unsigned_abs());
    quiet_until(&mut answering, twice_the_idle_time).await;
    answering.subscribe(Level::Full, None).await;
}

#[tokio::test]
async fn a_checkpoint_moves_the_cursor_past_events_that_the_connection_may_not_receive() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.heartbeat = milliseconds(100);
        config.limits.stream.client_frames.attempts = 10_000;
        config.limits.stream.replay_most_events = 3;
    })
    .await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let mut stream = server.subscribed(&plain, Level::Full).await;
    assert_eq!(stream.within(Duration::from_millis(300)).await, None);

    server.state.streams.hold();
    for _ in 0..5 {
        server.invite(&owner, json!({})).await;
    }
    let head = server.snapshot(&plain).await.cursor;
    server.state.streams.release();
    assert_eq!(
        stream.next().await,
        Frame::Message(ServerMessage::Checkpoint { cursor: head })
    );
    assert_eq!(stream.within(Duration::from_millis(300)).await, None);

    let mut again = server.stream(&plain).await;
    assert_eq!(again.subscribe(Level::Full, Some(head)).await, head);
    assert!(server.traffic(&plain, &mut again).await.is_empty());
}

#[tokio::test]
async fn a_client_message_that_is_not_known_or_too_large_closes_the_connection() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let large = format!(
        r#"{{"type":"subscribe","level":"full","padding":"{}"}}"#,
        "x".repeat(5_000)
    );
    for (text, code) in [
        ("not json", CloseCode::InvalidMessage),
        (r#"{"type":"listen"}"#, CloseCode::InvalidMessage),
        (
            r#"{"type":"subscribe","level":"loud"}"#,
            CloseCode::InvalidMessage,
        ),
        (large.as_str(), CloseCode::MessageTooBig),
    ] {
        let mut stream = server.stream(&owner).await;
        stream.send_text(text).await;
        assert_eq!(closed_with(&mut stream).await, Some(code), "{text:.40}");
    }
    let mut stream = server.stream(&owner).await;
    stream
        .send_frame(Message::Binary(vec![1, 2, 3].into()))
        .await
        .unwrap();
    assert_eq!(
        closed_with(&mut stream).await,
        Some(CloseCode::InvalidMessage)
    );

    let mut tolerant = server.stream(&owner).await;
    tolerant
        .send_text(r#"{"type":"subscribe","level":"full","new_field":1}"#)
        .await;
    assert!(matches!(
        tolerant.next().await,
        Frame::Message(ServerMessage::Subscribed { .. })
    ));
    until_connections(&server, &owner, 1).await;
}

#[tokio::test]
async fn a_client_that_sends_too_many_frames_is_closed() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.client_frames.attempts = 5;
    })
    .await;
    let owner = server.owner().await;
    let mut stream = server.stream(&owner).await;
    for _ in 0..5 {
        stream.subscribe(Level::Full, None).await;
    }
    stream
        .send(&ClientMessage::Subscribe {
            level: Level::Full,
            cursor: None,
        })
        .await;
    assert_eq!(
        closed_with(&mut stream).await,
        Some(CloseCode::TooManyMessages)
    );
}

#[tokio::test]
async fn a_client_that_does_not_read_is_disconnected_and_resumes_from_its_cursor() {
    const CHANGES: usize = 400;
    let server = TestServer::start_tuned(
        |config| {
            config.limits.stream.send_timeout = milliseconds(300);
            config.limits.stream.poll_interval = milliseconds(50);
        },
        Some(8 * 1024),
    )
    .await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let mut slow = server.narrow_stream(&owner, 8 * 1024).await;
    let start = slow.subscribe(Level::Full, None).await;

    let name = |change: usize| format!("{change:0>8000}");
    server
        .large_events(
            json!({ "to": "workspace", "workspace_id": workspace.id }),
            json!({ "type": "workspace_updated", "workspace": workspace }),
            &["workspace", "name"],
            CHANGES,
        )
        .await;
    until_connections(&server, &owner, 0).await;

    let mut received: Vec<(u64, String)> = Vec::new();
    let read = |event: Event, id: u64, received: &mut Vec<(u64, String)>| {
        if let Event::WorkspaceUpdated { workspace } = event {
            received.push((id, workspace.name));
        }
    };
    let end = loop {
        match slow.next().await {
            Frame::Message(ServerMessage::Change { events }) => {
                for event in events {
                    read(event.event, event.id, &mut received);
                }
            }
            Frame::Message(other) => panic!("unexpected {other:?}"),
            Frame::Closed(code) => break code,
        }
    };
    assert!(
        end.is_none() || end == Some(CloseCode::Slow.code()),
        "{end:?}"
    );
    assert!(
        !received.is_empty() && received.len() < CHANGES,
        "{}",
        received.len()
    );

    let cursor = received.last().map_or(start, |(id, _)| slow.cursor(*id));
    let mut resumed = server.stream(&owner).await;
    resumed.subscribe(Level::Full, Some(cursor)).await;
    while received.len() < CHANGES {
        let (id, event) = resumed.event().await;
        read(event, id, &mut received);
    }
    let names: Vec<String> = received.iter().map(|(_, name)| name.clone()).collect();
    let expected: Vec<String> = (0..CHANGES).map(name).collect();
    assert_eq!(names, expected);
    assert!(received.windows(2).all(|pair| pair[0].0 < pair[1].0));
}

#[tokio::test]
async fn the_health_route_reports_the_database() {
    let server = TestServer::start().await;
    let reply = server
        .send::<Value>(Method::GET, "/api/v1/health", None, None, None)
        .await;
    assert_eq!(
        reply.ok::<Health>(StatusCode::OK),
        Health {
            database: true,
            media_server: None,
            failing_call_rooms: 0,
            storage_low: None,
        }
    );

    server.state.database.close().await;
    let reply = server
        .send::<Value>(Method::GET, "/api/v1/health", None, None, None)
        .await;
    reply.error(ErrorCode::Unavailable);
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(reply.retry_after.as_deref(), Some("1"));
}

fn category_names(events: Vec<(u64, Event)>) -> Vec<String> {
    events
        .into_iter()
        .filter_map(|(_, event)| match event {
            Event::CategoryCreated { category } => Some(category.name),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_stop_between_a_commit_and_its_delivery_loses_no_event() {
    let mut server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let mut stream = server.subscribed(&owner, Level::Full).await;
    server.category(&owner, workspace.id, "delivered", 0).await;
    let (delivered, _) = stream.event().await;

    server.state.streams.hold();
    server.category(&owner, workspace.id, "committed", 1).await;
    server
        .category(&owner, workspace.id, "committed too", 2)
        .await;
    assert_eq!(stream.within(Duration::from_millis(200)).await, None);

    server.stop().await;
    assert_eq!(
        stream.until_closed().await,
        (vec![], Some(CloseCode::Stopping))
    );
    assert_eq!(server.state.streams.connections(), 0);

    let restarted = server.restart().await;
    let mut resumed = restarted.stream(&owner).await;
    assert_eq!(resumed.epoch, stream.epoch);
    resumed
        .subscribe(Level::Full, Some(stream.cursor(delivered)))
        .await;
    assert_eq!(
        category_names(restarted.traffic(&owner, &mut resumed).await),
        ["committed", "committed too"]
    );
}

#[tokio::test]
async fn two_server_processes_deliver_the_events_of_each_other_in_one_order() {
    let first = TestServer::start_with(|config| {
        config.limits.stream.poll_interval = milliseconds(50);
        config.limits.stream.connections_per_device = 1;
    })
    .await;
    let owner = first.owner().await;
    let guest = first.join(&owner, "Guest").await;
    let workspace = first.default_workspace(&owner).await;
    let second = first.restart().await;

    let mut on_first = first.subscribed(&owner, Level::Full).await;
    let mut on_second = second.subscribed(&owner, Level::Full).await;
    assert_eq!(first.state.streams.connections(), 1);
    assert_eq!(second.state.streams.connections(), 1);

    let mut expected = Vec::new();
    for change in 0..12 {
        let through = if change % 2 == 0 { &first } else { &second };
        let name = format!("category {change}");
        through.category(&owner, workspace.id, &name, change).await;
        expected.push(name);
    }
    let seen_on_first = first.traffic(&owner, &mut on_first).await;
    let seen_on_second = second.traffic(&owner, &mut on_second).await;
    assert_eq!(category_names(seen_on_first.clone()), expected);
    assert_eq!(seen_on_first, seen_on_second[..seen_on_first.len()]);

    let mut of_guest = second.subscribed(&guest, Level::Full).await;
    first
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", guest.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(of_guest.until_closed().await.1, Some(CloseCode::Ended));
}
