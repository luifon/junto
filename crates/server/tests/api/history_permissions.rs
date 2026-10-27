mod event_times;
mod lists;
mod mentions;
mod notices;
mod recordings;
mod streams;

use crate::messages_support::{channel, message, send, text};
use crate::support::{Member, TestServer};
use junto_protocol::{
    WorkspacePermission as W, error::ErrorCode, events::Level, message::Message, structure::Channel,
};
use reqwest::StatusCode;
use time::Duration;

async fn denied() -> (TestServer, Member, Member, Channel, Message) {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let old = send(&server, &reader, &channel, "Before the connection").await;
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(channel.id).bind(reader.account_id).bind(channel.workspace_id).bind(1_i64 << W::ReadMessageHistory as u8).execute(&server.db).await.unwrap();
    (server, owner, reader, channel, old)
}

#[tokio::test]
async fn without_history_only_messages_after_the_current_live_connection_are_readable() {
    let (server, owner, reader, channel, old) = denied().await;
    server
        .get(&reader, &message(old.id))
        .await
        .error(ErrorCode::Forbidden);
    server.clock.advance(Duration::seconds(1));
    let first = server.subscribed(&reader, Level::Full).await;
    let same_time = send(&server, &owner, &channel, "At the connection start").await;
    server
        .get(&reader, &message(same_time.id))
        .await
        .error(ErrorCode::Forbidden);
    server.clock.advance(Duration::seconds(1));
    let live = send(&server, &owner, &channel, "During the connection").await;
    server
        .get(&reader, &message(live.id))
        .await
        .ok::<Message>(StatusCode::OK);
    server
        .get(&reader, &message(old.id))
        .await
        .error(ErrorCode::Forbidden);
    server.clock.advance(Duration::seconds(1));
    let second = server.subscribed(&reader, Level::Full).await;
    server
        .get(&reader, &message(live.id))
        .await
        .error(ErrorCode::Forbidden);
    server.clock.advance(Duration::seconds(1));
    let later = send(&server, &owner, &channel, "During the new connection").await;
    server
        .get(&reader, &message(later.id))
        .await
        .ok::<Message>(StatusCode::OK);
    sqlx::query(
        "update channel_person_overrides set deny=deny|$3 where channel_id=$1 and account_id=$2",
    )
    .bind(channel.id)
    .bind(reader.account_id)
    .bind(1_i64 << W::ViewChannel as u8)
    .execute(&server.db)
    .await
    .unwrap();
    server
        .get(&reader, &message(later.id))
        .await
        .error(ErrorCode::NotFound);
    drop((first, second));
}

#[tokio::test]
async fn channel_history_requires_history_permission_even_with_a_live_connection() {
    let (server, _, reader, channel, _) = denied().await;
    let stream = server.subscribed(&reader, Level::Full).await;
    server
        .get(
            &reader,
            &format!("/api/v1/channels/{}/messages", channel.id),
        )
        .await
        .error(ErrorCode::Forbidden);
    drop(stream);
}

#[tokio::test]
async fn sending_and_editing_return_the_written_message_without_history_permission() {
    let (server, _, reader, channel, _) = denied().await;
    let sent = send(&server, &reader, &channel, "New message").await;
    let edited: Message = server
        .put(&reader, &message(sent.id), &text("Edited"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(edited.id, sent.id);
    let reply: Message = server
        .post(
            &reader,
            &format!("/api/v1/messages/{}/thread", sent.id),
            &text("Reply"),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put(&reader, &message(reply.id), &text("Edited reply"))
        .await
        .ok::<Message>(StatusCode::OK);
}
