//! Rate limits apply to every message mutation and history, not just edits.
use crate::messages_support::*;
use crate::support::TestServer;
use junto_protocol::{error::ErrorCode, message::*};
use reqwest::StatusCode;
use serde_json::json;
use time::Duration;

#[tokio::test]
async fn every_message_mutation_counts_against_the_account_limit() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let other = server.join(&owner, "Other").await;
    let channel = channel(&server, &owner, "text").await;
    let sent = send(&server, &owner, &channel, "message").await;
    let now = server.clock.now();
    while server
        .state
        .message_writes
        .attempt_of(owner.account_id, now)
        .is_ok()
    {}
    server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &text("send"),
        )
        .await
        .error(ErrorCode::RateLimited);
    server
        .delete(&owner, &message(sent.id))
        .await
        .error(ErrorCode::RateLimited);
    server
        .put(
            &owner,
            &format!("{}/reaction", message(sent.id)),
            &json!({"emoji":"👍"}),
        )
        .await
        .error(ErrorCode::RateLimited);
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/messages/read", channel.id),
            &json!({"position":sent.position}),
        )
        .await
        .error(ErrorCode::RateLimited);
    server
        .post(
            &owner,
            "/api/v1/conversations",
            &json!({"account_id":other.account_id}),
        )
        .await
        .error(ErrorCode::RateLimited);
    server.clock.advance(Duration::minutes(1));
    server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &text("again"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
}

#[tokio::test]
async fn attachment_mutations_have_a_separate_session_limit() {
    let files = Files::open().await;
    let pending = files.start(b"pending").await;
    // Consume attempts through the route, including invalid upload declarations.
    for _ in 0..1199 {
        files.server.post(&files.owner,"/api/v1/attachments",&json!({"channel_id":files.channel.id,"name":"file","bytes":0,"sha256":"00".repeat(32)})).await.error(ErrorCode::InvalidRequest);
    }
    files
        .server
        .put_bytes(
            &files.owner,
            &format!("/api/v1/attachments/{}/chunks/0", pending.id),
            None,
            b"pending".to_vec(),
        )
        .await
        .error(ErrorCode::RateLimited);
    files
        .server
        .post(
            &files.owner,
            &format!("/api/v1/attachments/{}/complete", pending.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::RateLimited);
    files
        .server
        .delete(
            &files.owner,
            &format!("/api/v1/attachments/{}/upload", pending.id),
        )
        .await
        .error(ErrorCode::RateLimited);
    files.server.clock.advance(Duration::minutes(1));
    files
        .server
        .delete(
            &files.owner,
            &format!("/api/v1/attachments/{}/upload", pending.id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn history_reads_are_rate_limited_and_recover_with_the_clock() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let route = format!("/api/v1/channels/{}/messages", channel.id);
    for _ in 0..120 {
        server
            .get(&owner, &route)
            .await
            .ok::<MessagePage>(StatusCode::OK);
    }
    server
        .get(&owner, &route)
        .await
        .error(ErrorCode::RateLimited);
    server.clock.advance(Duration::minutes(1));
    server
        .get(&owner, &route)
        .await
        .ok::<MessagePage>(StatusCode::OK);
}
