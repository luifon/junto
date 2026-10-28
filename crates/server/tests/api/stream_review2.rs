//! The defects that the second review of the event stream found, each with its scenario.

use std::time::{Duration, Instant};

use junto_protocol::account::RecoveryNotice;
use junto_protocol::events::{CloseCode, EndReason, Level, ServerMessage};
use junto_protocol::roles::WorkspaceRole;
use junto_server::events::connection;
use junto_server::events::hub::Arrivals;
use junto_server::events::transport::server_socket;
use reqwest::StatusCode;
use serde_json::{Value, json};
use sqlx::Connection;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::Role;

use crate::stream_review::{
    assign, opened, private_channel, queue_an_event_and_block_its_read, role,
};
use crate::support::{DeviceKey, TestServer, drive_until};

fn milliseconds(count: i64) -> time::Duration {
    time::Duration::milliseconds(count)
}

#[tokio::test]
async fn interrupted_reads_never_use_more_database_backends_than_the_bound() {
    let world = private_channel(|config| {
        config.limits.stream.most_failed_attempts = 10_000;
    })
    .await;
    let (server, owner) = (&world.server, &world.owner);
    let mut streams = Vec::new();
    for index in 0..6 {
        let member = server.join(owner, &format!("Member {index}")).await;
        assign(server, owner, &world.workspace, &member, &world.team, true).await;
        streams.push(server.subscribed(&member, Level::Full).await);
    }
    server.tighten_streams(|limits| {
        limits.read_deadline = milliseconds(300);
        limits.retry_delay = milliseconds(100);
    });
    let mut observer = sqlx::PgConnection::connect(&server.database_url())
        .await
        .unwrap();

    let lock = queue_an_event_and_block_its_read(&world).await;
    let started = Instant::now();
    let mut most = 0;
    while started.elapsed() < Duration::from_millis(2_500) {
        let waiting: i64 = sqlx::query_scalar(
            "select count(*) from pg_stat_activity
             where datname = current_database() and wait_event_type = 'Lock'",
        )
        .fetch_one(&mut observer)
        .await
        .unwrap();
        most = most.max(waiting);
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let request =
        tokio::time::timeout(Duration::from_secs(6), server.get(owner, "/api/v1/account"))
            .await
            .expect("a request waited for a database connection");
    request.ok::<Value>(StatusCode::OK);
    lock.rollback().await.unwrap();

    assert!(most >= 1, "no read waited");
    assert!(
        most <= 4,
        "{most} backends were busy with reads of the event stream"
    );
}

#[tokio::test]
async fn a_connection_that_makes_no_progress_is_closed_and_can_resume() {
    let world = private_channel(|config| {
        config.limits.stream.most_failed_attempts = 3;
    })
    .await;
    let server = &world.server;
    let mut stream = server.stream(&world.insider).await;
    let start = stream.subscribe(Level::Full, None).await;
    let normal = server.state.streams.limits();
    server.tighten_streams(|limits| {
        limits.read_deadline = milliseconds(200);
        limits.retry_delay = milliseconds(100);
    });

    let lock = queue_an_event_and_block_its_read(&world).await;
    let closed = tokio::time::timeout(Duration::from_secs(10), stream.until_closed()).await;
    assert_eq!(
        closed,
        Ok((vec![], Some(CloseCode::Failed))),
        "the connection stayed open without progress"
    );
    lock.rollback().await.unwrap();
    server.state.streams.set_limits((*normal).clone());

    let mut resumed = server.stream(&world.insider).await;
    resumed.subscribe(Level::Full, Some(start)).await;
    assert_eq!(resumed.event().await.1.name(), "message_changed");
    assert_eq!(resumed.event().await.1.name(), "channel_updated");
}

#[tokio::test]
async fn a_connection_whose_session_cannot_be_checked_is_closed() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.heartbeat = milliseconds(100);
        config.limits.stream.most_failed_attempts = 3;
        config.limits.stream.client_frames.attempts = 10_000;
    })
    .await;
    let owner = server.owner().await;
    let mut stream = server.stream(&owner).await;
    assert_eq!(stream.within(Duration::from_millis(400)).await, None);
    server.tighten_streams(|limits| {
        limits.read_deadline = milliseconds(200);
        limits.retry_delay = milliseconds(100);
    });

    let mut lock = server.db.begin().await.unwrap();
    sqlx::query("lock table server_roles in access exclusive mode")
        .execute(&mut *lock)
        .await
        .unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(10), stream.until_closed()).await;
    assert_eq!(
        closed,
        Ok((vec![], Some(CloseCode::Failed))),
        "the connection stayed open without a check of its session"
    );
    lock.rollback().await.unwrap();
}

#[tokio::test]
async fn the_reason_of_an_ended_session_is_stored_with_the_session() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let renumber = |id: &'static str| {
        let pool = server.db.clone();
        let device_id = guest.device_id.as_uuid();
        async move {
            sqlx::query("update sessions set id = $1::uuid where device_id = $2")
                .bind(id)
                .bind(device_id)
                .execute(&pool)
                .await
                .unwrap();
        }
    };

    renumber("ffffffff-ffff-7fff-bfff-ffffffffffff").await;
    let mut old = server.stream(&guest).await;
    let notice: RecoveryNotice = server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/recovery", guest.account_id),
            &json!({ "public_key": DeviceKey::generate().public() }),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(
        old.until_closed().await,
        (
            vec![ServerMessage::Ended {
                reason: EndReason::Recovered,
                recovery: Some(notice.clone()),
            }],
            Some(CloseCode::Ended)
        )
    );

    let session = server.login(&guest.key).await;
    renumber("00000000-0000-7000-8000-000000000001").await;
    let mut later = server
        .open_stream(Some(session.token.expose()))
        .await
        .unwrap();
    server
        .send::<Value>(
            reqwest::Method::DELETE,
            "/api/v1/auth/session",
            Some(session.token.expose()),
            None,
            None,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        later.until_closed().await,
        (
            vec![ServerMessage::Ended {
                reason: EndReason::SessionEnded,
                recovery: None,
            }],
            Some(CloseCode::Ended)
        )
    );

    let stored: Vec<(String, Option<uuid::Uuid>)> =
        sqlx::query_as("select reason, recovery_id from ended_sessions order by ended_at, reason")
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert_eq!(
        stored,
        [
            ("logged_out".to_owned(), None),
            ("recovered".to_owned(), Some(notice.id.as_uuid())),
        ]
    );
}

#[tokio::test]
async fn the_time_limit_of_a_write_is_not_renewed_by_interruptions() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.send_timeout = milliseconds(600);
    })
    .await;
    let owner = server.owner().await;
    let epoch = server.snapshot(&owner).await.cursor.epoch;
    let streams = server.state.streams.clone();

    let (near, far) = tokio::io::duplex(64);
    let socket = server_socket(near, &server.state.config.limits.stream).await;
    let registration = streams
        .open(
            owner.account_id,
            owner.device_id,
            uuid::Uuid::nil(),
            time::OffsetDateTime::UNIX_EPOCH,
        )
        .unwrap();
    let app = server.state.clone();
    let serving = tokio::spawn(connection::run(
        app,
        socket,
        opened(&owner),
        epoch,
        registration,
    ));
    let _client = WebSocketStream::from_raw_socket(far, Role::Client, None).await;

    let started = Instant::now();
    while !serving.is_finished() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "a client that reads nothing kept its connection through interruptions"
        );
        let mut arrivals = Arrivals::default();
        arrivals.revalidate.insert(owner.account_id);
        streams.wake(&arrivals);
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
    assert!(started.elapsed() >= Duration::from_millis(500));
}

#[tokio::test]
async fn two_requests_that_together_pass_a_ceiling_do_not_both_commit() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let workspace = server.default_workspace(&owner).await;
    let first: WorkspaceRole = role(&server, &owner, &workspace, "first")
        .await
        .ok(StatusCode::CREATED);
    let second: WorkspaceRole = role(&server, &owner, &workspace, "second")
        .await
        .ok(StatusCode::CREATED);
    let most_held = i64::from(server.state.config.limits.most_roles_per_member);
    let most_roles = i64::from(server.state.config.limits.most_roles_per_workspace);
    sqlx::query(
        "with added as (
             insert into workspace_roles (id, workspace_id, name, permissions, created_at, position)
             select gen_random_uuid(), $1, 'held', 0, now(), (1000 + n)::integer
             from generate_series(1, $3) n returning id
         ) insert into member_roles (workspace_id, account_id, role_id)
           select $1, $2, id from added",
    )
    .bind(workspace.id)
    .bind(guest.account_id)
    .bind(most_held - 1)
    .execute(&mut server.test_data().await)
    .await
    .unwrap();
    let path = |role: &WorkspaceRole| {
        format!(
            "/api/v1/workspaces/{}/members/{}/roles/{}",
            workspace.id, guest.account_id, role.id
        )
    };
    let limit_reached = |reply: &crate::support::Reply| {
        assert_eq!(reply.status, StatusCode::CONFLICT, "{}", reply.text());
        assert!(reply.text().contains("limit_reached"), "{}", reply.text());
    };

    server.gate().arm(0);
    let first_path = path(&first);
    let held = server.put_empty(&owner, &first_path);
    tokio::pin!(held);
    drive_until(&mut held, server.gate().arrived()).await;
    server
        .put_empty(&owner, &path(&second))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.gate().release();
    limit_reached(&held.await);
    let held_roles: i64 = sqlx::query_scalar("select count(*) from member_roles")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(held_roles, most_held);
    let existing: i64 =
        sqlx::query_scalar("select count(*) from workspace_roles where workspace_id = $1")
            .bind(workspace.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    sqlx::query(
        "insert into workspace_roles (id, workspace_id, name, permissions, created_at, position)
         select gen_random_uuid(), $1, 'filler', 0, now(), (2000 + n)::integer
         from generate_series(1, $2) n",
    )
    .bind(workspace.id)
    .bind(most_roles - existing - 1)
    .execute(&mut server.test_data().await)
    .await
    .unwrap();

    server.gate().arm(0);
    let roles_path = format!("/api/v1/workspaces/{}/roles", workspace.id);
    let body = json!({ "name": "third" });
    let held = server.post(&owner, &roles_path, &body);
    tokio::pin!(held);
    drive_until(&mut held, server.gate().arrived()).await;
    let second_body = json!({ "name": "fourth" });
    let second = server.post(&owner, &roles_path, &second_body);
    tokio::pin!(second);
    drive_until(&mut second, server.until_blocked()).await;
    server.gate().release();
    held.await.ok::<WorkspaceRole>(StatusCode::CREATED);
    limit_reached(&second.await);
    let roles: i64 = sqlx::query_scalar("select count(*) from workspace_roles")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(roles, most_roles);
}
