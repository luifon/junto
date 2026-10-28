use super::*;
use crate::discovery_support::{mention, notices};
use junto_protocol::{discovery::NotificationsRead, events::Event};
use serde_json::json;

#[tokio::test]
async fn saved_notices_require_history_or_a_message_from_the_current_connection() {
    let (server, owner, reader, channel, _) = denied().await;
    mention(&server, &owner, &channel, &reader).await;
    assert!(notices(&server, &reader).await.is_empty());
}

#[tokio::test]
async fn a_live_notice_can_be_read_and_marked_until_reconnect() {
    let (server, owner, reader, channel, _) = denied().await;
    let old = mention(&server, &owner, &channel, &reader).await;
    let first = server.subscribed(&reader, Level::Full).await;
    server.clock.advance(Duration::seconds(1));
    let live = mention(&server, &owner, &channel, &reader).await;
    let listed = notices(&server, &reader).await;
    assert_eq!(
        listed.iter().map(|n| n.message_id).collect::<Vec<_>>(),
        vec![live.id]
    );
    server
        .put_empty(&reader, &format!("/api/v1/notifications/{}/read", old.id))
        .await
        .error(ErrorCode::NotFound);
    server
        .put_empty(&reader, &format!("/api/v1/notifications/{}/read", live.id))
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
    server.clock.advance(Duration::seconds(1));
    let second = server.subscribed(&reader, Level::Full).await;
    assert!(notices(&server, &reader).await.is_empty());
    server
        .put_empty(&reader, &format!("/api/v1/notifications/{}/read", live.id))
        .await
        .error(ErrorCode::NotFound);
    drop((first, second));
}

#[tokio::test]
async fn mark_all_read_counts_only_notices_readable_in_the_current_connection() {
    let (server, owner, reader, channel, _) = denied().await;
    let old = mention(&server, &owner, &channel, &reader).await;
    let stream = server.subscribed(&reader, Level::Full).await;
    server.clock.advance(Duration::seconds(1));
    mention(&server, &owner, &channel, &reader).await;
    let marked: NotificationsRead = server
        .put(&reader, "/api/v1/notifications/read", &json!({}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(marked.marked, 1);
    let unread: bool = sqlx::query_scalar(
        "select read_at is null from notifications where account_id=$1 and message_id=$2",
    )
    .bind(reader.account_id)
    .bind(old.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(unread);
    drop(stream);
}

#[tokio::test]
async fn notification_events_do_not_replay_old_notices_but_deliver_live_notices() {
    let (server, owner, reader, channel, _) = denied().await;
    let cursor = server.snapshot(&reader).await.cursor;
    let old = mention(&server, &owner, &channel, &reader).await;
    server.clock.advance(Duration::seconds(1));
    let mut stream = server.stream(&reader).await;
    stream.subscribe(Level::Full, Some(cursor)).await;
    server.clock.advance(Duration::seconds(1));
    let live = mention(&server, &owner, &channel, &reader).await;
    let events = server.traffic(&reader, &mut stream).await;
    assert!(events.iter().any(
        |(_, event)| matches!(event,Event::NotificationChanged{message_id, ..} if *message_id==live.id)
    ));
    assert!(!events.iter().any(
        |(_, event)| matches!(event,Event::NotificationChanged{message_id, ..} if *message_id==old.id)
    ));
}

#[tokio::test]
async fn snapshots_keep_live_notices_and_settings_without_exposing_old_notices() {
    let (server, owner, reader, channel, _) = denied().await;
    mention(&server, &owner, &channel, &reader).await;
    server
        .put(
            &reader,
            &format!("/api/v1/notification-settings/{}", channel.id),
            &json!({"level":"mentions"}),
        )
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
    let stream = server.subscribed(&reader, Level::Full).await;
    server.clock.advance(Duration::seconds(1));
    let live = mention(&server, &owner, &channel, &reader).await;
    let snapshot = server.snapshot(&reader).await;
    assert_eq!(
        snapshot
            .notifications
            .iter()
            .map(|n| n.message_id)
            .collect::<Vec<_>>(),
        vec![live.id]
    );
    assert!(
        snapshot
            .notification_settings
            .iter()
            .any(|s| s.scope_id == channel.id.as_uuid())
    );
    drop(stream);
}
