//! Storage and model regressions found during independent review.
use crate::messages_support::*;
use crate::support::{Scratch, TestServer};
use junto_protocol::{
    error::ErrorCode,
    message::{Message, Upload},
};
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn deleting_a_root_preserves_its_replies_and_removes_its_content() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &writer, &channel, "root secret").await;
    let reply: Message = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &json!({"document":text("reply")["document"],"reply_to":root.id}),
        )
        .await
        .ok(StatusCode::CREATED);
    sqlx::query("update messages set thread_root=$1,permanent=true where id=$2")
        .bind(root.id)
        .bind(reply.id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("update channels set permanent_message_count=1 where id=$1")
        .bind(channel.id)
        .execute(&server.db)
        .await
        .unwrap();
    server
        .delete(&writer, &message(root.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let marker: Message = server
        .get(&owner, &message(root.id))
        .await
        .ok(StatusCode::OK);
    assert!(marker.deleted);
    assert!(marker.document.is_none());
    assert!(marker.system.is_none());
    assert!(marker.author_id.is_none());
    let kept: Message = server
        .get(&owner, &message(reply.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(kept.reply_to, Some(root.id));
    assert!(kept.permanent);
    let count: i64 = sqlx::query_scalar("select permanent_message_count from channels where id=$1")
        .bind(channel.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn attachment_names_refuse_invisible_and_direction_controls() {
    let directory = Scratch::new();
    let server = TestServer::start_with(|config| {
        config.attachments_directory = Some(directory.0.clone());
        config.limits.recordings.reserve_bytes = 0;
    })
    .await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    for name in [
        "image\u{202e}gnp.exe",
        "image\u{200b}.exe",
        "image\u{2066}txt.exe",
        "image\u{feff}.exe",
    ] {
        server
            .post(
                &owner,
                "/api/v1/attachments",
                &json!({"channel_id":channel.id,"name":name,"bytes":1,"sha256":"00".repeat(32)}),
            )
            .await
            .error(ErrorCode::InvalidRequest);
    }
}

#[tokio::test]
async fn images_larger_than_the_safe_decode_bound_are_generic_files() {
    let directory = Scratch::new();
    let server = TestServer::start_with(|config| {
        config.attachments_directory = Some(directory.0.clone());
        config.limits.recordings.reserve_bytes = 0;
    })
    .await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let bytes = junto_server::image::samples::png(8192, 8192);
    let upload = upload(&server, &owner, json!({"channel_id":channel.id}), &bytes).await;
    let message: Message = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &json!({"document":{"blocks":[]},"attachments":[upload.id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(
        message.attachments[0].media_type,
        "application/octet-stream"
    );
    assert_eq!(message.attachments[0].width, None);
}

#[tokio::test]
async fn an_absent_staging_directory_does_not_stop_later_cleanup() {
    let directory = Scratch::new();
    let server = TestServer::start_with(|config| {
        config.attachments_directory = Some(directory.0.clone());
        config.limits.recordings.reserve_bytes = 0;
    })
    .await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let first = upload(&server, &owner, json!({"channel_id":channel.id}), b"first").await;
    let second = upload(&server, &owner, json!({"channel_id":channel.id}), b"second").await;
    std::fs::remove_dir_all(directory.0.join(first.id.to_string())).unwrap();
    let partial = directory
        .0
        .join(second.id.to_string())
        .join("staging-test.partial");
    std::fs::write(&partial, b"unused").unwrap();
    server.clock.advance(time::Duration::minutes(2));
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    assert!(!partial.exists());
    server
        .get(&owner, &format!("/api/v1/attachments/{}/upload", second.id))
        .await
        .ok::<Upload>(StatusCode::OK);
}

#[cfg(unix)]
#[tokio::test]
async fn one_unreadable_directory_does_not_block_later_staging_cleanup() {
    use std::os::unix::fs::PermissionsExt;
    let files = Files::open().await;
    let first = files.upload(b"first").await;
    let second = files.upload(b"second").await;
    let inaccessible = files.directory.0.join(first.id.to_string());
    let permissions = std::fs::metadata(&inaccessible).unwrap().permissions();
    std::fs::set_permissions(&inaccessible, std::fs::Permissions::from_mode(0o0)).unwrap();
    let partial = files
        .directory
        .0
        .join(second.id.to_string())
        .join("staging-test.partial");
    std::fs::write(&partial, b"unused").unwrap();
    files.server.clock.advance(time::Duration::minutes(2));
    let result = junto_server::maintenance::run_once(&files.server.state).await;
    std::fs::set_permissions(&inaccessible, permissions).unwrap();
    assert!(result.is_ok(), "{result:?}");
    assert!(!partial.exists());
}
