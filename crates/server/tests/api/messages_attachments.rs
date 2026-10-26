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
    // Above 16,777,216 pixels, and a side above 16,384 pixels.
    for bytes in [
        large,
        animated,
        junto_server::image::samples::png(4097, 4096),
        junto_server::image::samples::png(16_385, 1),
    ] {
        let upload = files.upload(&bytes).await;
        let sent = files.attach(&[upload.id]).await;
        assert_eq!(sent.attachments[0].width, None);
        assert_eq!(sent.attachments[0].media_type, "application/octet-stream");
    }
}

/// A picture larger than 1,024 pixels per side gets its size, up to 16,777,216 pixels: a
/// client shows a reduced copy in its lists and the picture at its own size in its viewer.
#[tokio::test]
async fn previews_state_the_size_of_a_picture_larger_than_1024_pixels() {
    let files = Files::open().await;
    for (width, height) in [(4096, 4096), (2560, 1440), (16_384, 1)] {
        let upload = files
            .upload(&junto_server::image::samples::png(width, height))
            .await;
        let sent = files.attach(&[upload.id]).await;
        assert_eq!(
            (sent.attachments[0].width, sent.attachments[0].height),
            (Some(width), Some(height))
        );
        assert_eq!(sent.attachments[0].media_type, "image/png");
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

#[cfg(unix)]
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
    let response = strict
        .post(
            &files.owner,
            &format!("/api/v1/attachments/{}/complete", upload.id),
            &json!({}),
        )
        .await;
    lock.commit().await.unwrap();
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    response.error(ErrorCode::Unavailable);
    assert!(response.retry_after.is_none(), "{}", response.text());
    assert_eq!(
        response.ok::<serde_json::Value>(StatusCode::SERVICE_UNAVAILABLE)["error"]["message"],
        "the request could not be confirmed; state may have changed; read current state before sending it again"
    );
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

#[tokio::test]
async fn a_chunk_with_a_committed_intent_does_not_claim_that_nothing_changed() {
    let files = Files::open().await;
    let upload = files.start(b"content").await;
    let route = format!("/api/v1/attachments/{}/chunks/0", upload.id);
    files.server.gate().arm(1);
    let mut request = std::pin::pin!(files.server.put_bytes(
        &files.owner,
        &route,
        None,
        b"content".to_vec(),
    ));
    crate::support::drive_until(&mut request, files.server.gate().arrived()).await;
    files.server.gate().lose_next(8);
    files.server.gate().release();
    // The intent transaction was already allowed through its loss check.
    // Each attempt of the final transaction now loses.
    let reply = request.await;
    let intent: bool = sqlx::query_scalar(
        "select stored from attachment_chunks where attachment_id=$1 and offset_bytes=0",
    )
    .bind(upload.id)
    .fetch_one(&files.server.db)
    .await
    .unwrap();
    assert!(!intent);
    assert_eq!(
        std::fs::read(files.directory.0.join(upload.id.to_string()).join("0")).unwrap(),
        b"content"
    );
    assert_eq!(reply.status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(reply.retry_after.is_none(), "{}", reply.text());
    reply.error(ErrorCode::Unavailable);
    let resumed: Upload = files
        .server
        .put_bytes(&files.owner, &route, None, b"content".to_vec())
        .await
        .ok(StatusCode::OK);
    assert_eq!(resumed.received, 7);
    let repeated: Upload = files
        .server
        .put_bytes(&files.owner, &route, None, b"content".to_vec())
        .await
        .ok(StatusCode::OK);
    assert_eq!(repeated.received, 7);
}

#[tokio::test]
async fn a_chunk_deadline_after_its_intent_does_not_claim_that_nothing_changed() {
    let files = Files::open().await;
    let upload = files.start(b"content").await;
    let strict = files
        .server
        .strict(|config| {
            config.limits.request_timeout = Duration::seconds(1);
        })
        .await;
    let route = format!("/api/v1/attachments/{}/chunks/0", upload.id);
    strict.gate().arm(2);
    let mut request =
        std::pin::pin!(strict.put_bytes(&files.owner, &route, None, b"content".to_vec(),));
    crate::support::drive_until(&mut request, strict.gate().arrived()).await;
    let reply = request.await;
    strict.gate().release();
    assert_eq!(
        std::fs::read(files.directory.0.join(upload.id.to_string()).join("0")).unwrap(),
        b"content"
    );
    assert!(reply.retry_after.is_none(), "{}", reply.text());
    reply.error(ErrorCode::Unavailable);
    let resumed: Upload = files
        .server
        .put_bytes(&files.owner, &route, None, b"content".to_vec())
        .await
        .ok(StatusCode::OK);
    assert_eq!(resumed.received, 7);
}

#[tokio::test]
async fn a_chunk_acceptance_lock_timeout_does_not_claim_that_nothing_changed() {
    let files = Files::open().await;
    let upload = files.start(b"content").await;
    sqlx::raw_sql(
        "create function hold_chunk_acceptance() returns trigger language plpgsql as $$
         begin perform pg_advisory_xact_lock(1); return new; end $$;
         create trigger hold_chunk_acceptance before update of stored on attachment_chunks
         for each row execute function hold_chunk_acceptance()",
    )
    .execute(&files.server.db)
    .await
    .unwrap();
    let strict = files
        .server
        .strict(|config| {
            config.limits.database.lock = Duration::milliseconds(300);
        })
        .await;
    let mut holder = files.server.db.begin().await.unwrap();
    sqlx::query("select pg_advisory_xact_lock(1)")
        .execute(&mut *holder)
        .await
        .unwrap();
    let route = format!("/api/v1/attachments/{}/chunks/0", upload.id);
    let reply = strict
        .put_bytes(&files.owner, &route, None, b"content".to_vec())
        .await;
    assert_eq!(
        std::fs::read(files.directory.0.join(upload.id.to_string()).join("0")).unwrap(),
        b"content"
    );
    assert!(reply.retry_after.is_none(), "{}", reply.text());
    reply.error(ErrorCode::Unavailable);
    holder.rollback().await.unwrap();
    let resumed: Upload = files
        .server
        .put_bytes(&files.owner, &route, None, b"content".to_vec())
        .await
        .ok(StatusCode::OK);
    assert_eq!(resumed.received, 7);
}

#[tokio::test]
async fn the_files_of_a_message_keep_the_order_of_the_send_request() {
    let files = Files::open().await;
    let mut uploads = Vec::new();
    for bytes in [b"zero".as_slice(), b"one", b"two", b"three"] {
        uploads.push(files.upload(bytes).await);
    }
    let order = [2, 0, 3, 1];
    let ids: Vec<_> = order.iter().map(|index| uploads[*index].id).collect();
    let hashes = |message: &Message| -> Vec<String> {
        message
            .attachments
            .iter()
            .map(|file| file.sha256.clone())
            .collect()
    };
    let expected: Vec<String> = ["two".as_bytes(), b"zero", b"three", b"one"]
        .iter()
        .map(|bytes| junto_server::messages::uploads::hash(bytes))
        .collect();
    let sent = files.attach(&ids).await;
    assert_eq!(hashes(&sent), expected);
    let read: Message = files
        .server
        .get(&files.owner, &format!("/api/v1/messages/{}", sent.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(hashes(&read), expected);
    let page: MessagePage = files
        .server
        .get(
            &files.owner,
            &format!("/api/v1/channels/{}/messages", files.channel.id),
        )
        .await
        .ok(StatusCode::OK);
    let listed = page
        .messages
        .iter()
        .find(|message| message.id == sent.id)
        .expect("the sent message in the history");
    assert_eq!(hashes(listed), expected);
    let fork: junto_protocol::structure::Channel = files
        .server
        .post(
            &files.owner,
            &format!("/api/v1/messages/{}/fork", sent.id),
            &json!({"name":"fork","kind":"text"}),
        )
        .await
        .ok(StatusCode::CREATED);
    let copied: MessagePage = files
        .server
        .get(
            &files.owner,
            &format!("/api/v1/channels/{}/messages", fork.id),
        )
        .await
        .ok(StatusCode::OK);
    let copy = copied
        .messages
        .iter()
        .find(|message| !message.attachments.is_empty())
        .expect("the copy of the message with its files");
    assert_eq!(hashes(copy), expected);
}

#[tokio::test]
async fn migration_numbers_the_files_of_earlier_messages_in_the_order_of_their_ids() {
    let files = Files::open().await;
    let mut uploads = Vec::new();
    for bytes in [b"zero".as_slice(), b"one", b"two"] {
        uploads.push(files.upload(bytes).await);
    }
    let sent = files
        .attach(&[uploads[2].id, uploads[0].id, uploads[1].id])
        .await;
    let unattached = files.upload(b"later").await;
    // The table as it was before migration 0025, then the migration.
    sqlx::raw_sql(
        "alter table attachments drop constraint attachments_attached_have_a_position;
         alter table attachments drop column position",
    )
    .execute(&files.server.db)
    .await
    .unwrap();
    sqlx::raw_sql(include_str!(
        "../../migrations/0025_attachment_positions.sql"
    ))
    .execute(&files.server.db)
    .await
    .unwrap();
    let numbered: Vec<(junto_protocol::ids::AttachmentId, Option<i32>)> =
        sqlx::query_as("select id, position from attachments order by id")
            .fetch_all(&files.server.db)
            .await
            .unwrap();
    assert_eq!(
        numbered,
        [
            (uploads[0].id, Some(0)),
            (uploads[1].id, Some(1)),
            (uploads[2].id, Some(2)),
            (unattached.id, None),
        ]
    );
    let read: Message = files
        .server
        .get(&files.owner, &format!("/api/v1/messages/{}", sent.id))
        .await
        .ok(StatusCode::OK);
    let ids: Vec<_> = read.attachments.iter().map(|file| file.id).collect();
    assert_eq!(ids, [uploads[0].id, uploads[1].id, uploads[2].id]);
}
