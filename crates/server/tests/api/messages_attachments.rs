//! Attachment bounds, capacity, storage reservations, transport and cleanup.
use crate::messages_support::*;
use junto_protocol::{error::ErrorCode, message::*};
use reqwest::StatusCode;
use serde_json::json;
use time::Duration;

#[tokio::test]
async fn sending_refuses_excess_and_repeated_attachments() {
    let files = Files::open().await;
    let first = files.upload(b"first").await;
    let second = files.upload(b"second").await;
    files
        .server
        .put(
            &files.owner,
            "/api/v1/attachment-limits",
            &json!({"bytes_per_file":100000000,"files_per_message":1}),
        )
        .await
        .ok::<AttachmentLimits>(StatusCode::OK);
    let route = format!("/api/v1/channels/{}/messages", files.channel.id);
    files
        .server
        .post(
            &files.owner,
            &route,
            &json!({"document":{"blocks":[]},"attachments":[first.id,second.id]}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    files
        .server
        .put(
            &files.owner,
            "/api/v1/attachment-limits",
            &json!({"bytes_per_file":100000000,"files_per_message":10}),
        )
        .await
        .ok::<AttachmentLimits>(StatusCode::OK);
    files
        .server
        .post(
            &files.owner,
            &route,
            &json!({"document":{"blocks":[]},"attachments":[first.id,first.id]}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    assert_eq!(
        files.attach(&[first.id, second.id]).await.attachments.len(),
        2
    );
}

async fn removed_rows(files: &Files) -> i64 {
    sqlx::query_scalar("select count(*) from attachments where removed_at is not null")
        .fetch_one(&files.server.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn an_account_has_at_most_twenty_pending_uploads() {
    let files = Files::open().await;
    for _ in 0..20 {
        files.start(b"x").await;
    }
    files.server.post(&files.owner,"/api/v1/attachments",&json!({"channel_id":files.channel.id,"name":"file","bytes":1,"sha256":"00".repeat(32)})).await.error(ErrorCode::LimitReached);
}

#[tokio::test]
async fn upload_capacity_is_four_and_download_capacity_is_independent() {
    let files = Files::open().await;
    let complete = files.upload(b"complete").await;
    files.attach(&[complete.id]).await;
    let pending = files.start(b"pending").await;
    let mut held = Vec::new();
    for account in [
        junto_protocol::ids::AccountId::generate(),
        junto_protocol::ids::AccountId::generate(),
    ] {
        for _ in 0..2 {
            held.push(
                files
                    .server
                    .state
                    .attachment_transfers
                    .acquire(account)
                    .unwrap(),
            );
        }
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
        .error(ErrorCode::Busy);
    assert_eq!(
        files
            .server
            .get(
                &files.owner,
                &format!("/api/v1/attachments/{}/file", complete.id)
            )
            .await
            .bytes(),
        b"complete"
    );
    drop(held);
    files
        .server
        .put_bytes(
            &files.owner,
            &format!("/api/v1/attachments/{}/chunks/0", pending.id),
            None,
            b"pending".to_vec(),
        )
        .await
        .ok::<Upload>(StatusCode::OK);
}

#[tokio::test]
async fn every_new_chunk_checks_the_storage_reserve() {
    let files = Files::open().await;
    let upload = files.start(b"content").await;
    let low = files
        .server
        .strict(|config| config.limits.recordings.reserve_bytes = u64::MAX)
        .await;
    low.put_bytes(
        &files.owner,
        &format!("/api/v1/attachments/{}/chunks/0", upload.id),
        None,
        b"content".to_vec(),
    )
    .await
    .error(ErrorCode::StorageLow);
    let stored: Upload = files
        .server
        .get(
            &files.owner,
            &format!("/api/v1/attachments/{}/upload", upload.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(stored.received, 0);
}

#[tokio::test]
async fn completion_cancellation_and_expiry_release_their_reservations() {
    let files = Files::open().await;
    let count = || async {
        sqlx::query_scalar::<_, i64>(
            "select count(*) from storage_reservations where kind='attachment'",
        )
        .fetch_one(&files.server.db)
        .await
        .unwrap()
    };
    let complete = files.upload(b"complete").await;
    assert_eq!(count().await, 0);
    let cancelled = files.start(b"cancelled").await;
    assert_eq!(count().await, 1);
    files
        .server
        .delete(
            &files.owner,
            &format!("/api/v1/attachments/{}/upload", cancelled.id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    files
        .server
        .get(
            &files.owner,
            &format!("/api/v1/attachments/{}/upload", cancelled.id),
        )
        .await
        .error(ErrorCode::NotFound);
    // The cancel ends the reservation in its transaction; the cleanup removes the row
    // one minute later.
    assert_eq!(count().await, 0);
    files.server.clock.advance(Duration::seconds(59));
    junto_server::maintenance::run_once(&files.server.state)
        .await
        .unwrap();
    assert_eq!(removed_rows(&files).await, 1);
    files.server.clock.advance(Duration::seconds(2));
    junto_server::maintenance::run_once(&files.server.state)
        .await
        .unwrap();
    assert_eq!(removed_rows(&files).await, 0);
    assert_eq!(count().await, 0);
    let expired = files.start(b"expired").await;
    assert_eq!(count().await, 1);
    files.server.clock.advance(Duration::hours(24));
    junto_server::maintenance::run_once(&files.server.state)
        .await
        .unwrap();
    assert_eq!(count().await, 0);
    for upload in [complete, expired] {
        files
            .server
            .get(
                &files.owner,
                &format!("/api/v1/attachments/{}/upload", upload.id),
            )
            .await
            .error(ErrorCode::NotFound);
    }
}

#[tokio::test]
async fn chunk_lengths_match_the_declared_file_and_chunk_bound() {
    let files = Files::open().await;
    let upload = files.start(&vec![0; 1024 * 1024 + 3]).await;
    let route = format!("/api/v1/attachments/{}/chunks/0", upload.id);
    files
        .server
        .put_bytes(&files.owner, &route, None, vec![0; 1023])
        .await
        .error(ErrorCode::Conflict);
    files
        .server
        .put_bytes(&files.owner, &route, None, vec![0; 1024 * 1024 + 1])
        .await
        .error(ErrorCode::InvalidRequest);
    files
        .server
        .put_bytes(&files.owner, &route, None, vec![0; 1024 * 1024])
        .await
        .ok::<Upload>(StatusCode::OK);
    files
        .server
        .put_bytes(
            &files.owner,
            &format!("/api/v1/attachments/{}/chunks/1048576", upload.id),
            None,
            vec![0; 4],
        )
        .await
        .error(ErrorCode::Conflict);
}

#[tokio::test]
async fn downloads_state_safe_headers_and_accept_one_byte_range() {
    let files = Files::open().await;
    let bytes = b"<html>untrusted</html>";
    let upload = files.upload(bytes).await;
    files.attach(&[upload.id]).await;
    let route = format!("/api/v1/attachments/{}/file", upload.id);
    let reply = files.server.get(&files.owner, &route).await;
    assert_eq!(reply.header("content-disposition"), Some("attachment"));
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert_eq!(
        reply.header("content-type"),
        Some("application/octet-stream")
    );
    assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(
        reply.header("etag"),
        Some(format!("\"{}\"", junto_server::messages::uploads::hash(bytes)).as_str())
    );
    for (range, status, expected) in [
        ("bytes=2-5", StatusCode::PARTIAL_CONTENT, &bytes[2..6]),
        (
            "bytes=-3",
            StatusCode::PARTIAL_CONTENT,
            &bytes[bytes.len() - 3..],
        ),
        ("bytes=0-1,4-5", StatusCode::OK, &bytes[..]),
    ] {
        let response = reqwest::Client::new()
            .get(format!("{}{route}", files.server.base))
            .bearer_auth(&files.owner.token)
            .header("Range", range)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), status);
        assert_eq!(response.bytes().await.unwrap().as_ref(), expected);
    }
    let response = reqwest::Client::new()
        .get(format!("{}{route}", files.server.base))
        .bearer_auth(&files.owner.token)
        .header("Range", "bytes=999-")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
}

#[tokio::test]
async fn previews_refuse_large_or_animated_images() {
    let files = Files::open().await;
    let mut large = junto_server::image::samples::png(2, 2);
    large.resize(8 * 1024 * 1024 + 1, 0);
    let animated = junto_server::image::samples::animated_png(2, 2);
    for bytes in [large, animated, junto_server::image::samples::png(8193, 1)] {
        let upload = files.upload(&bytes).await;
        let sent = files.attach(&[upload.id]).await;
        assert_eq!(sent.attachments[0].width, None);
        assert_eq!(sent.attachments[0].media_type, "application/octet-stream");
    }
}

#[tokio::test]
async fn channel_deletion_removes_messages_and_queues_attachment_cleanup() {
    let files = Files::open().await;
    let upload = files.upload(b"file").await;
    let sent = files.attach(&[upload.id]).await;
    files
        .server
        .delete(
            &files.owner,
            &format!("/api/v1/channels/{}", files.channel.id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    files
        .server
        .get(&files.owner, &message(sent.id))
        .await
        .error(ErrorCode::NotFound);
    files
        .server
        .get(
            &files.owner,
            &format!("/api/v1/attachments/{}/file", upload.id),
        )
        .await
        .error(ErrorCode::NotFound);
    files.server.clock.advance(Duration::minutes(2));
    junto_server::maintenance::run_once(&files.server.state)
        .await
        .unwrap();
    assert!(!files.directory.0.join(upload.id.to_string()).exists());
}

#[tokio::test]
async fn a_symbolic_link_cannot_supply_an_uploaded_chunk() {
    let files = Files::open().await;
    let upload = files.upload(b"file").await;
    let chunk = files.directory.0.join(upload.id.to_string()).join("0");
    std::fs::remove_file(&chunk).unwrap();
    let outside = files.directory.0.join("outside");
    std::fs::write(&outside, b"file").unwrap();
    std::os::unix::fs::symlink(&outside, &chunk).unwrap();
    files.attach(&[upload.id]).await;
    let response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/attachments/{}/file",
            files.server.base, upload.id
        ))
        .bearer_auth(&files.owner.token)
        .send()
        .await;
    assert!(match response {
        Err(_) => true,
        Ok(response) => response.bytes().await.is_err(),
    });
    assert_eq!(std::fs::read(outside).unwrap(), b"file");
}

#[tokio::test]
async fn changing_attachment_limits_is_audited_and_unchanged_limits_publish_nothing() {
    let files = Files::open().await;
    let mut stream = files
        .server
        .subscribed(&files.owner, junto_protocol::events::Level::Background)
        .await;
    let limits = json!({"bytes_per_file":5000,"files_per_message":2});
    files
        .server
        .put(&files.owner, "/api/v1/attachment-limits", &limits)
        .await
        .ok::<AttachmentLimits>(StatusCode::OK);
    assert!(matches!(
        stream.event().await.1,
        junto_protocol::events::Event::AttachmentLimitsChanged { .. }
    ));
    let count: i64 = sqlx::query_scalar(
        "select count(*) from audit_entries where action='attachment_limits_changed'",
    )
    .fetch_one(&files.server.db)
    .await
    .unwrap();
    assert_eq!(count, 1);
    files
        .server
        .put(&files.owner, "/api/v1/attachment-limits", &limits)
        .await
        .ok::<AttachmentLimits>(StatusCode::OK);
    assert!(
        files
            .server
            .traffic(&files.owner, &mut stream)
            .await
            .is_empty()
    );
    let count: i64 = sqlx::query_scalar(
        "select count(*) from audit_entries where action='attachment_limits_changed'",
    )
    .fetch_one(&files.server.db)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn completion_keeps_running_after_the_request_deadline_and_a_retry_reuses_it() {
    let files = Files::open().await;
    let upload = files.start(b"complete after deadline").await;
    files
        .server
        .put_bytes(
            &files.owner,
            &format!("/api/v1/attachments/{}/chunks/0", upload.id),
            None,
            b"complete after deadline".to_vec(),
        )
        .await
        .ok::<Upload>(StatusCode::OK);
    let strict = files
        .server
        .strict(|config| config.limits.request_timeout = Duration::seconds(1))
        .await;
    let mut lock = files.server.db.begin().await.unwrap();
    sqlx::query("select id from attachments where id=$1 for update")
        .bind(upload.id)
        .fetch_one(&mut *lock)
        .await
        .unwrap();
    strict
        .post(
            &files.owner,
            &format!("/api/v1/attachments/{}/complete", upload.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Timeout);
    lock.commit().await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let state: Upload = files
                .server
                .get(
                    &files.owner,
                    &format!("/api/v1/attachments/{}/upload", upload.id),
                )
                .await
                .ok(StatusCode::OK);
            if state.complete {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("verification persisted after the HTTP deadline");
    // A complete retry must not reread bytes; a later download still verifies them.
    std::fs::remove_file(files.directory.0.join(upload.id.to_string()).join("0")).unwrap();
    let state: Upload = files
        .server
        .post(
            &files.owner,
            &format!("/api/v1/attachments/{}/complete", upload.id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    assert!(state.complete);
}

#[tokio::test]
async fn failed_hash_is_recorded_and_completion_retry_does_not_read_files() {
    let files = Files::open().await;
    let pending = files.start(b"correct").await;
    let route = format!("/api/v1/attachments/{}/complete", pending.id);
    files
        .server
        .put_bytes(
            &files.owner,
            &format!("/api/v1/attachments/{}/chunks/0", pending.id),
            None,
            b"corrupt".to_vec(),
        )
        .await
        .ok::<Upload>(StatusCode::OK);
    files
        .server
        .post(&files.owner, &route, &json!({}))
        .await
        .error(ErrorCode::InvalidRequest);
    let directory = junto_server::messages::uploads::directory(pending.id);
    junto_server::recordings::files::remove(&files.directory.0, &directory).unwrap();
    files
        .server
        .post(&files.owner, &route, &json!({}))
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn completing_an_upload_requires_an_upload_place() {
    let files = Files::open().await;
    let pending = files.start(b"content").await;
    files
        .server
        .put_bytes(
            &files.owner,
            &format!("/api/v1/attachments/{}/chunks/0", pending.id),
            None,
            b"content".to_vec(),
        )
        .await
        .ok::<Upload>(StatusCode::OK);
    let mut held = Vec::new();
    for account in [
        junto_protocol::ids::AccountId::generate(),
        junto_protocol::ids::AccountId::generate(),
    ] {
        for _ in 0..2 {
            held.push(
                files
                    .server
                    .state
                    .attachment_transfers
                    .acquire(account)
                    .unwrap(),
            );
        }
    }
    files
        .server
        .post(
            &files.owner,
            &format!("/api/v1/attachments/{}/complete", pending.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Busy);
    drop(held);
    let complete: Upload = files
        .server
        .post(
            &files.owner,
            &format!("/api/v1/attachments/{}/complete", pending.id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    assert!(complete.complete);
}

#[tokio::test]
async fn absent_attachment_storage_is_unavailable() {
    let server = crate::support::TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    server
        .post(
            &owner,
            "/api/v1/attachments",
            &json!({"channel_id":channel.id,"name":"file","bytes":1,"sha256":"00".repeat(32)}),
        )
        .await
        .error(ErrorCode::Unavailable);
}

#[tokio::test]
async fn one_account_can_hold_only_two_upload_places_and_other_accounts_can_upload() {
    let files = Files::open().await;
    let pending = files.start(b"content").await;
    files
        .server
        .put_bytes(
            &files.owner,
            &format!("/api/v1/attachments/{}/chunks/0", pending.id),
            None,
            b"content".to_vec(),
        )
        .await
        .ok::<Upload>(StatusCode::OK);
    let chunk = files.start(b"next").await;
    let first = files
        .server
        .state
        .attachment_transfers
        .acquire(files.owner.account_id)
        .unwrap();
    let second = files
        .server
        .state
        .attachment_transfers
        .acquire(files.owner.account_id)
        .unwrap();
    files
        .server
        .put_bytes(
            &files.owner,
            &format!("/api/v1/attachments/{}/chunks/0", chunk.id),
            None,
            b"next".to_vec(),
        )
        .await
        .error(ErrorCode::Busy);
    files
        .server
        .post(
            &files.owner,
            &format!("/api/v1/attachments/{}/complete", pending.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Busy);
    let other = files.server.join(&files.owner, "Other").await;
    let other_upload = upload(
        &files.server,
        &other,
        json!({"channel_id":files.channel.id}),
        b"other",
    )
    .await;
    assert!(other_upload.complete);
    drop((first, second));
    let complete: Upload = files
        .server
        .post(
            &files.owner,
            &format!("/api/v1/attachments/{}/complete", pending.id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    assert!(complete.complete);
}
