use super::*;
use junto_protocol::ids::MessageId;
use serde_json::Value;

async fn recorded_times(server: &TestServer, id: MessageId, expected: time::OffsetDateTime) {
    let audiences: Vec<Value> = sqlx::query_scalar("select audience from events where kind='message_changed' and payload->>'message_id'=$1 order by position,ordinal")
        .bind(id.to_string()).fetch_all(&server.db).await.unwrap();
    assert!(!audiences.is_empty());
    for audience in audiences {
        assert_eq!(
            audience["created_at"],
            Value::String(
                expected
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap()
            )
        );
    }
}

#[tokio::test]
async fn message_events_keep_creation_time_through_edits_and_deletion() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "Root").await;
    server.clock.advance(Duration::seconds(1));
    server
        .put(&owner, &message(root.id), &text("Edited"))
        .await
        .ok::<Message>(StatusCode::OK);
    server
        .delete(&owner, &message(root.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    recorded_times(&server, root.id, root.created_at).await;
}

#[tokio::test]
async fn reply_deletion_keeps_creation_time_after_its_row_is_erased() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "Root").await;
    let reply: Message = server
        .post(
            &owner,
            &format!("{}/thread", message(root.id)),
            &text("Reply"),
        )
        .await
        .ok(StatusCode::CREATED);
    server.clock.advance(Duration::seconds(1));
    server
        .delete(&owner, &message(reply.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let present: bool = sqlx::query_scalar("select exists(select 1 from messages where id=$1)")
        .bind(reply.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(!present);
    recorded_times(&server, reply.id, reply.created_at).await;
}

#[tokio::test]
async fn recording_system_message_events_also_keep_their_creation_time() {
    let studio = crate::recordings_support::Studio::open().await;
    let done = studio.finished_after(Duration::seconds(1)).await;
    let server = &studio.scene.server;
    let lines: Vec<(MessageId, time::OffsetDateTime)> =
        sqlx::query_as("select id,created_at from messages where recording_id=$1")
            .bind(done.recording.id)
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert!(!lines.is_empty());
    for (id, created_at) in lines {
        recorded_times(server, id, created_at).await;
    }
}
