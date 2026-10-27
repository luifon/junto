use crate::messages_support::{channel, message, send, text};
use crate::support::{Member, TestServer};
use junto_protocol::{
    WorkspacePermission as W, error::ErrorCode, message::Message, structure::Channel,
};
use reqwest::StatusCode;
use serde_json::json;

async fn denied(permission: W) -> (TestServer, Member, Member, Channel, Message) {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &reader, &channel, "Root").await;
    server
        .put(
            &reader,
            &format!("{}/reaction", message(root.id)),
            &json!({"emoji":"👍"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(channel.id).bind(reader.account_id).bind(channel.workspace_id).bind(1_i64 << permission as u8).execute(&server.db).await.unwrap();
    (server, owner, reader, channel, root)
}

#[tokio::test]
async fn a_person_who_cannot_send_cannot_create_or_edit_channel_messages() {
    let (server, _, reader, channel, root) = denied(W::SendMessages).await;
    server
        .post(
            &reader,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &text("Blocked"),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .put(&reader, &message(root.id), &text("Changed"))
        .await
        .error(ErrorCode::Forbidden);
    let unchanged: Message = server
        .get(&reader, &message(root.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(unchanged.document, root.document);
}

#[tokio::test]
async fn a_person_who_cannot_add_reactions_can_remove_their_existing_reaction() {
    let (server, _, reader, _, root) = denied(W::AddReactions).await;
    server
        .put(
            &reader,
            &format!("{}/reaction", message(root.id)),
            &json!({"emoji":"❤️"}),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&reader, &format!("{}/reaction/👍", message(root.id)))
        .await
        .empty(StatusCode::NO_CONTENT);
    let changed: Message = server
        .get(&reader, &message(root.id))
        .await
        .ok(StatusCode::OK);
    assert!(changed.reactions.is_empty());
}

#[tokio::test]
async fn sending_in_threads_is_independent_of_sending_in_the_channel() {
    let (server, _, reader, channel, root) = denied(W::SendMessages).await;
    let reply: Message = server
        .post(
            &reader,
            &format!("{}/thread", message(root.id)),
            &text("Reply"),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put(&reader, &message(reply.id), &text("Edited reply"))
        .await
        .ok::<Message>(StatusCode::OK);
    sqlx::query("update channel_person_overrides set deny=$2 where channel_id=$1")
        .bind(channel.id)
        .bind(1_i64 << W::SendMessagesInThreads as u8)
        .execute(&server.db)
        .await
        .unwrap();
    server
        .post(
            &reader,
            &format!("{}/thread", message(root.id)),
            &text("Blocked"),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .put(&reader, &message(reply.id), &text("Blocked edit"))
        .await
        .error(ErrorCode::Forbidden);
    server
        .post(
            &reader,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &text("Allowed"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
}

#[tokio::test]
async fn creating_a_thread_is_required_only_for_its_first_reply() {
    let (server, _, reader, channel, root) = denied(W::CreateThreads).await;
    server
        .put(
            &reader,
            &format!("{}/thread/follow", message(root.id)),
            &json!({"following":true}),
        )
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
    server
        .post(
            &reader,
            &format!("{}/thread", message(root.id)),
            &text("Blocked"),
        )
        .await
        .error(ErrorCode::Forbidden);
    sqlx::query("delete from channel_person_overrides where channel_id=$1")
        .bind(channel.id)
        .execute(&server.db)
        .await
        .unwrap();
    let first: Message = server
        .post(
            &reader,
            &format!("{}/thread", message(root.id)),
            &text("First"),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .delete(&reader, &message(first.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(channel.id).bind(reader.account_id).bind(channel.workspace_id).bind(1_i64 << W::CreateThreads as u8).execute(&server.db).await.unwrap();
    server
        .post(
            &reader,
            &format!("{}/thread", message(root.id)),
            &text("Existing thread"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
}

#[tokio::test]
async fn role_mentions_require_permission_when_added_to_a_message() {
    let (server, owner, reader, channel, root) = denied(W::MentionRoles).await;
    let role = server
        .grant_workspace(&owner, &reader, channel.workspace_id, &[W::AddReactions])
        .await;
    let body = json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"role","role_id":role}]}]}});
    server
        .post(
            &reader,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &body,
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .put(&reader, &message(root.id), &body)
        .await
        .error(ErrorCode::Forbidden);
    sqlx::query("delete from channel_person_overrides where channel_id=$1")
        .bind(channel.id)
        .execute(&server.db)
        .await
        .unwrap();
    let sent: Message = server
        .post(
            &reader,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &body,
        )
        .await
        .ok(StatusCode::CREATED);
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(channel.id).bind(reader.account_id).bind(channel.workspace_id).bind(1_i64 << W::MentionRoles as u8).execute(&server.db).await.unwrap();
    let mut edited = body;
    edited["document"]["blocks"][0]["content"]
        .as_array_mut()
        .unwrap()
        .push(json!({"kind":"text","text":" unchanged mention"}));
    server
        .put(&reader, &message(sent.id), &edited)
        .await
        .ok::<Message>(StatusCode::OK);
}

#[tokio::test]
async fn editing_a_channel_message_rechecks_send_permission() {
    let (server, _, reader, _, root) = denied(W::SendMessages).await;
    server
        .put(&reader, &message(root.id), &text("Blocked edit"))
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn editing_a_thread_reply_rechecks_thread_send_permission() {
    let (server, _, reader, channel, root) = denied(W::SendMessages).await;
    let reply: Message = server
        .post(
            &reader,
            &format!("{}/thread", message(root.id)),
            &text("Reply"),
        )
        .await
        .ok(StatusCode::CREATED);
    sqlx::query("update channel_person_overrides set deny=$2 where channel_id=$1")
        .bind(channel.id)
        .bind(1_i64 << W::SendMessagesInThreads as u8)
        .execute(&server.db)
        .await
        .unwrap();
    server
        .put(&reader, &message(reply.id), &text("Blocked edit"))
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn adding_a_role_mention_by_edit_rechecks_mention_permission() {
    let (server, owner, reader, channel, root) = denied(W::MentionRoles).await;
    let role = server
        .grant_workspace(&owner, &reader, channel.workspace_id, &[W::AddReactions])
        .await;
    let body = json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"role","role_id":role}]}]}});
    server
        .put(&reader, &message(root.id), &body)
        .await
        .error(ErrorCode::Forbidden);
}
