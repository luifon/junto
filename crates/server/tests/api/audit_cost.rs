//! Proofs of the audit of the whole server: work and storage that one member controls.
//! Each test fails while its defect exists.

use std::sync::Arc;

use junto_protocol::error::ErrorCode;
use junto_protocol::message::Upload;
use junto_server::storage::FakeDisk;
use reqwest::StatusCode;
use serde_json::json;

use crate::messages_support::{Files, channel, send, text};
use crate::recordings_support::Studio;
use crate::support::{FREE_DISK, Scratch, TestServer, fixture_batches};

/// ADR 0408: an account has at most 20 pending uploads, and each one reserves its declared
/// size in the storage guard. A cancelled upload no longer counts as pending, and its
/// reservation stays until the cleanup removes its directory. One account that starts and
/// cancels uploads holds more reservations than the limit allows, without a bound, and the
/// reservations stop the uploads and the recordings of every account.
#[tokio::test]
async fn audit_cancelled_uploads_hold_no_more_reservations_than_the_limit_of_pending_uploads() {
    let files = Files::open().await;
    for _ in 0..25 {
        let upload = files.start(b"x").await;
        files
            .server
            .delete(
                &files.owner,
                &format!("/api/v1/attachments/{}/upload", upload.id),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let reserved: i64 =
        sqlx::query_scalar("select count(*) from storage_reservations where kind = 'attachment'")
            .fetch_one(&files.server.db)
            .await
            .unwrap();
    assert!(
        reserved <= 20,
        "one account holds {reserved} reservations of uploads at one time"
    );
}

/// ADR 0605: a recording starts when the free space holds the reserve, every reservation
/// and its own reservation. One account starts 100 uploads of the largest size and cancels
/// each one. No reservation of them is left, so a recording starts on a disk that holds
/// the reserve and one reservation of a recording, and nothing more.
#[tokio::test]
async fn a_recording_starts_after_one_account_started_and_cancelled_100_uploads() {
    let attachments = Scratch::new();
    let directory = attachments.0.clone();
    let server = TestServer::start_for_calls_with(move |config| {
        config.attachments_directory = Some(directory);
    })
    .await;
    server
        .state
        .attachment_space
        .use_disk(Arc::new(FakeDisk::new(FREE_DISK)));
    let studio = Studio::on(server).await;
    let (server, ben) = (&studio.scene.server, &studio.scene.ben);
    let limits = &server.state.config.limits;
    let largest = limits.attachment_bytes;
    let start = json!({
        "channel_id": studio.scene.voice.id,
        "name": "file.bin",
        "bytes": largest,
        "sha256": "00".repeat(32),
    });

    for _ in 0..100 {
        let upload: Upload = server
            .post(ben, "/api/v1/attachments", &start)
            .await
            .ok(StatusCode::CREATED);
        server
            .delete(ben, &format!("/api/v1/attachments/{}/upload", upload.id))
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    assert_eq!(server.count("storage_reservations").await, 0);

    let recordings = &limits.recordings;
    server
        .disk()
        .set_free(recordings.reserve_bytes + recordings.reservation_bytes);
    server
        .record(&studio.scene.ann, studio.scene.voice.id)
        .await;

    // The uploads that wait still count: the account holds 20 and no more, and each one
    // keeps its reservation until it is completed, cancelled or expired.
    for _ in 0..20 {
        server
            .post(ben, "/api/v1/attachments", &start)
            .await
            .ok::<Upload>(StatusCode::CREATED);
    }
    server
        .post(ben, "/api/v1/attachments", &start)
        .await
        .error(ErrorCode::LimitReached);
    let held: i64 =
        sqlx::query_scalar("select count(*) from storage_reservations where kind = 'attachment'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(held, 20);
}

/// An upload whose row is removed and whose reservation still exists counts against the
/// limit of its account: the purge of a channel removes the uploads of the channel, and
/// their reservations end when the cleanup has removed their files.
#[tokio::test]
async fn a_removed_upload_counts_while_its_reservation_exists() {
    let files = Files::open().await;
    for _ in 0..20 {
        files.start(b"x").await;
    }
    sqlx::query("update attachments set removed_at = $1")
        .bind(files.server.clock.now())
        .execute(&files.server.db)
        .await
        .unwrap();
    let refused = files
        .server
        .post(
            &files.owner,
            "/api/v1/attachments",
            &json!({
                "channel_id": files.channel.id,
                "name": "file.bin",
                "bytes": 1,
                "sha256": "00".repeat(32),
            }),
        )
        .await;
    refused.error(ErrorCode::LimitReached);

    // The cleanup releases the reservations one minute after the removal.
    files.server.clock.advance(time::Duration::minutes(2));
    junto_server::messages::uploads::cleanup(&files.server.state)
        .await
        .unwrap();
    files.start(b"x").await;
}

/// How long a request of the test took, in milliseconds.
async fn timed<T>(request: impl Future<Output = T>) -> (T, u128) {
    let started = std::time::Instant::now();
    let output = request.await;
    (output, started.elapsed().as_millis())
}

/// A claim that the audit did not confirm: a search ranks every match before it takes its
/// page, so 120 searches per minute of one account could occupy the 16 database
/// connections. That needs 8 seconds per search. This test measures a search of a word
/// that 20,000 messages of one channel contain, against a search of a word that one
/// message contains. The time grows with the matches, and stays far below 8 seconds.
#[tokio::test]
async fn a_search_over_20000_matches_stays_below_the_time_that_fills_the_connections() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    send(&server, &owner, &channel, "single basalt").await;
    let document = text("placeholder")["document"].clone();
    let messages_before = server.count("messages").await;
    for (first, last) in fixture_batches(20_000) {
        sqlx::query(
            "insert into messages (id, place_id, position, author_id, created_at, document)
         select gen_random_uuid(), $1, 1000000 + n, $2, $3,
                jsonb_set($4, '{blocks,0,content,0,text}', to_jsonb('quartz report ' || n::text))
         from generate_series($5::int, $6::int) n",
        )
        .bind(channel.id)
        .bind(owner.account_id)
        .bind(server.clock.now())
        .bind(sqlx::types::Json(&document))
        .bind(first)
        .bind(last)
        .execute(&server.db)
        .await
        .unwrap();
    }
    assert_eq!(server.count("messages").await, messages_before + 20_000);
    sqlx::query("analyze messages")
        .execute(&server.db)
        .await
        .unwrap();

    let search = |word: &'static str| {
        let (server, owner) = (&server, &owner);
        async move {
            let page: junto_protocol::discovery::SearchPage = server
                .get(owner, &format!("/api/v1/search?q={word}&limit=1"))
                .await
                .ok(StatusCode::OK);
            page
        }
    };
    let (one, of_one) = timed(search("basalt")).await;
    let (many, of_many) = timed(search("quartz")).await;
    assert_eq!(one.results.len(), 1);
    assert_eq!(many.results.len(), 1);
    assert!(many.next.is_some());
    eprintln!("search: 1 match {of_one} ms, 20000 matches {of_many} ms");
    assert!(
        of_many < 8_000,
        "a search over 20000 matches took {of_many} ms; 120 of them in one minute fill 16 \
         connections"
    );
}

/// A claim that the audit did not confirm: a message that mentions a role writes one row
/// per recipient in several tables, and the trim of the event log deletes them by cascade
/// in one statement. This test measures both with 1,000 recipients: the send of one
/// message that mentions everyone, and one statement of the form of the trim that deletes
/// the events of 20 such messages. Both stay far below the 10 seconds that a statement may
/// take (ADR 0009).
#[tokio::test]
async fn a_mention_of_1000_recipients_and_the_trim_of_its_events_stay_below_the_statement_limit() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    sqlx::query(
        "with added as (
             insert into accounts (id, name, status, created_at)
             select gen_random_uuid(), 'Reader ' || n, 'active', $2
             from generate_series(1, 1000) n
             returning id)
         insert into workspace_members (workspace_id, account_id, joined_at)
         select $1, id, $2 from added",
    )
    .bind(channel.workspace_id)
    .bind(server.clock.now())
    .execute(&server.db)
    .await
    .unwrap();
    let route = format!("/api/v1/channels/{}/messages", channel.id);
    let body =
        json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"everyone"}]}]}});

    let mut slowest = 0;
    for _ in 0..20 {
        let (sent, took) = timed(server.post(&owner, &route, &body)).await;
        sent.ok::<junto_protocol::message::Message>(StatusCode::CREATED);
        slowest = slowest.max(took);
    }
    let per_event: Vec<(String, i64)> = sqlx::query_as(
        "select 'message_mention_receivers', count(*) from message_mention_receivers
         union all select 'notification_receivers', count(*) from notification_receivers
         union all select 'notifications', count(*) from notifications
         order by 1",
    )
    .fetch_all(&server.db)
    .await
    .unwrap();
    for (table, written) in &per_event {
        assert_eq!(*written, 20 * 1000, "{table}");
    }

    let (deleted, trim) = timed(
        sqlx::query(
            "delete from events where id in (
                 select id from events
                 where kind in ('message_mentioned', 'notification_changed') limit 500)",
        )
        .execute(&server.db),
    )
    .await;
    assert_eq!(deleted.unwrap().rows_affected(), 40);
    assert_eq!(server.count("message_mention_receivers").await, 0);
    assert_eq!(server.count("notification_receivers").await, 0);
    eprintln!("mention of 1000: slowest send {slowest} ms, delete of 40 events {trim} ms");
    assert!(slowest < 10_000, "a send took {slowest} ms");
    assert!(trim < 10_000, "the delete took {trim} ms");
}

/// ADR 0006: below the reserve the server refuses new uploads and warns the administrators.
/// A server without a recorder read no free space for the warning: its health route stated
/// nothing while every upload was refused with `storage_low`.
#[tokio::test]
async fn audit_a_server_without_a_recorder_states_low_storage_of_its_attachments() {
    use junto_protocol::server::Health;
    use junto_server::storage::FakeDisk;

    let directory = Scratch::new();
    let server = &TestServer::start_with(|config| {
        config.attachments_directory = Some(directory.0.clone());
    })
    .await;
    let reserve = server.state.storage_reserve_bytes();
    assert!(reserve > 0);
    let disk = std::sync::Arc::new(FakeDisk::new(reserve + 1));
    server.state.attachment_space.use_disk(disk.clone());
    let health = || async {
        server
            .send::<serde_json::Value>(reqwest::Method::GET, "/api/v1/health", None, None, None)
            .await
            .ok::<Health>(StatusCode::OK)
    };
    assert_eq!(health().await.storage_low, Some(false));

    disk.set_free(reserve);
    assert_eq!(health().await.storage_low, Some(true));
    disk.set_unreadable();
    assert_eq!(health().await.storage_low, None);
}
