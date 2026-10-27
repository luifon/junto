use crate::messages_support::Files;
use junto_protocol::{
    WorkspacePermission as W,
    error::ErrorCode,
    message::{Message, Upload},
};
use reqwest::StatusCode;
use serde_json::json;

async fn files() -> Files {
    let mut files = Files::open().await;
    files.owner = files.server.join(&files.owner, "Author").await;
    files
}

async fn deny(files: &Files, permissions: &[W]) {
    let bits = permissions.iter().fold(0_i64, |bits, permission| {
        bits | (1_i64 << *permission as u8)
    });
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4) on conflict(channel_id,account_id) do update set deny=excluded.deny")
        .bind(files.channel.id).bind(files.owner.account_id).bind(files.channel.workspace_id).bind(bits).execute(&files.server.db).await.unwrap();
}

#[tokio::test]
async fn starting_an_upload_requires_attach_files_and_a_message_destination() {
    let files = files().await;
    let body = json!({"channel_id":files.channel.id,"name":"file.bin","bytes":1,"sha256":junto_server::messages::uploads::hash(b"x")});
    deny(&files, &[W::AttachFiles]).await;
    files
        .server
        .post(&files.owner, "/api/v1/attachments", &body)
        .await
        .error(ErrorCode::Forbidden);
    deny(&files, &[W::SendMessages, W::SendMessagesInThreads]).await;
    files
        .server
        .post(&files.owner, "/api/v1/attachments", &body)
        .await
        .error(ErrorCode::Forbidden);
    deny(&files, &[W::SendMessages]).await;
    files.start(b"x").await;
}

#[tokio::test]
async fn an_upload_chunk_rechecks_attach_files() {
    let files = files().await;
    let pending = files.start(b"x").await;
    deny(&files, &[W::AttachFiles]).await;
    files
        .server
        .put_bytes(
            &files.owner,
            &format!("/api/v1/attachments/{}/chunks/0", pending.id),
            None,
            b"x".to_vec(),
        )
        .await
        .error(ErrorCode::Forbidden);
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
async fn upload_completion_rechecks_attach_files() {
    let files = files().await;
    let pending = files.start(b"x").await;
    files
        .server
        .put_bytes(
            &files.owner,
            &format!("/api/v1/attachments/{}/chunks/0", pending.id),
            None,
            b"x".to_vec(),
        )
        .await
        .ok::<Upload>(StatusCode::OK);
    deny(&files, &[W::AttachFiles]).await;
    files
        .server
        .post(
            &files.owner,
            &format!("/api/v1/attachments/{}/complete", pending.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn binding_a_completed_upload_rechecks_attach_files() {
    let files = files().await;
    let complete = files.upload(b"x").await;
    deny(&files, &[W::AttachFiles]).await;
    files
        .server
        .post(
            &files.owner,
            &format!("/api/v1/channels/{}/messages", files.channel.id),
            &json!({"document":{"blocks":[]},"attachments":[complete.id]}),
        )
        .await
        .error(ErrorCode::Forbidden);
    let pending: Upload = files
        .server
        .get(
            &files.owner,
            &format!("/api/v1/attachments/{}/upload", complete.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(pending.complete);
}

#[tokio::test]
async fn a_thread_only_writer_can_attach_to_a_thread() {
    let files = files().await;
    let root = files
        .server
        .post(
            &files.owner,
            &format!("/api/v1/channels/{}/messages", files.channel.id),
            &crate::messages_support::text("Root"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    deny(&files, &[W::SendMessages]).await;
    let complete = files.upload(b"x").await;
    files
        .server
        .post(
            &files.owner,
            &format!("/api/v1/messages/{}/thread", root.id),
            &json!({"document":{"blocks":[]},"attachments":[complete.id]}),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
}

#[tokio::test]
async fn starting_an_upload_requires_permission_to_send_somewhere_in_the_channel() {
    let files = files().await;
    deny(&files, &[W::SendMessages, W::SendMessagesInThreads]).await;
    let body = json!({"channel_id":files.channel.id,"name":"file.bin","bytes":1,"sha256":junto_server::messages::uploads::hash(b"x")});
    files
        .server
        .post(&files.owner, "/api/v1/attachments", &body)
        .await
        .error(ErrorCode::Forbidden);
}
