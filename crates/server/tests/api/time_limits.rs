use junto_protocol::error::ErrorCode;
use junto_protocol::structure::Workspace;
use reqwest::StatusCode;
use serde_json::json;
use time::Duration;

use crate::support::{TestServer, join_all};

const PATIENCE: std::time::Duration = std::time::Duration::from_secs(8);

/// The connections of this database, other than `holder` and the asking one, that run a
/// statement, wait for a lock or are inside a transaction.
async fn busy_backends(server: &TestServer, holder: i32) -> i64 {
    sqlx::query_scalar(
        "select count(*) from pg_stat_activity
         where datname = current_database()
           and pid <> $1 and pid <> pg_backend_pid()
           and state <> 'idle'",
    )
    .bind(holder)
    .fetch_one(&server.db)
    .await
    .unwrap()
}

async fn hold_workspace_row(
    server: &TestServer,
    workspace: &Workspace,
) -> (sqlx::Transaction<'static, sqlx::Postgres>, i32) {
    let mut holder = server.db.begin().await.unwrap();
    let pid: i32 = sqlx::query_scalar("select pg_backend_pid()")
        .fetch_one(&mut *holder)
        .await
        .unwrap();
    sqlx::query("select id from workspaces where id = $1 for update")
        .bind(workspace.id.as_uuid())
        .execute(&mut *holder)
        .await
        .unwrap();
    (holder, pid)
}

#[tokio::test]
async fn a_request_blocked_inside_a_statement_gets_the_timeout_error_and_frees_its_connection() {
    let server = TestServer::start_with(|config| {
        config.limits.database.lock = Duration::milliseconds(300);
        config.limits.database.idle_in_transaction = Duration::seconds(30);
    })
    .await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let path = format!("/api/v1/workspaces/{}", workspace.id);
    let (holder, holder_pid) = hold_workspace_row(&server, &workspace).await;

    let body = json!({ "name": "Renamed" });
    let blocked = (0..12)
        .map(|_| server.patch(&owner, &path, &body))
        .collect();
    let replies = tokio::time::timeout(PATIENCE, join_all(blocked))
        .await
        .expect("the blocked requests did not return");
    for reply in replies {
        assert_eq!(
            reply.status,
            StatusCode::SERVICE_UNAVAILABLE,
            "{}",
            reply.text()
        );
        reply.error(ErrorCode::Timeout);
        assert!(reply.retry_after.is_some());
    }
    assert_eq!(busy_backends(&server, holder_pid).await, 0);

    let account = tokio::time::timeout(PATIENCE, server.get(&owner, "/api/v1/account"))
        .await
        .expect("the pool did not serve the next request");
    account.ok::<serde_json::Value>(StatusCode::OK);

    holder.rollback().await.unwrap();
    let renamed: Workspace = server.patch(&owner, &path, &body).await.ok(StatusCode::OK);
    assert_eq!(renamed.name, "Renamed");
}

#[tokio::test]
async fn a_request_that_reaches_the_http_time_limit_inside_a_statement_stops_in_the_database() {
    let server = TestServer::start_with(|config| {
        config.limits.request_timeout = Duration::milliseconds(300);
        config.limits.database.lock = Duration::milliseconds(1_500);
        config.limits.database.idle_in_transaction = Duration::seconds(30);
    })
    .await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let path = format!("/api/v1/workspaces/{}", workspace.id);
    let (holder, holder_pid) = hold_workspace_row(&server, &workspace).await;

    let body = json!({ "name": "Renamed" });
    let blocked = (0..12)
        .map(|_| server.patch(&owner, &path, &body))
        .collect();
    let replies = tokio::time::timeout(PATIENCE, join_all(blocked))
        .await
        .expect("the blocked requests did not return");
    for reply in replies {
        reply.error(ErrorCode::Timeout);
    }

    let settled = tokio::time::timeout(PATIENCE, async {
        while busy_backends(&server, holder_pid).await > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        settled.is_ok(),
        "backends still work for requests that ended"
    );

    server
        .get(&owner, "/api/v1/account")
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
    holder.rollback().await.unwrap();
    let name: String = sqlx::query_scalar("select name from workspaces where id = $1")
        .bind(workspace.id.as_uuid())
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(name, "Acme");
}

#[tokio::test]
async fn a_connection_left_inside_a_transaction_is_ended_by_the_database() {
    let server = TestServer::start_with(|config| {
        config.limits.database.idle_in_transaction = Duration::milliseconds(400);
    })
    .await;
    let mut abandoned = server.db.begin().await.unwrap();
    sqlx::query("select 1")
        .execute(&mut *abandoned)
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    let result = sqlx::query("select 1").execute(&mut *abandoned).await;
    assert!(result.is_err(), "the idle transaction is still open");
}

#[tokio::test]
async fn a_statement_that_runs_too_long_is_cancelled() {
    let server = TestServer::start_with(|config| {
        config.limits.database.statement = Duration::milliseconds(300);
    })
    .await;
    let started = std::time::Instant::now();
    let result = sqlx::query("select pg_sleep(5)").execute(&server.db).await;
    assert!(result.is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(3));
}

#[tokio::test]
async fn migrations_are_exempt_from_the_statement_time_limit() {
    let server = TestServer::start_with(|config| {
        config.limits.database.statement = Duration::milliseconds(200);
    })
    .await;
    let directory = std::env::temp_dir().join(format!("junto-migration-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("9999_slow.sql"), "select pg_sleep(0.7);").unwrap();
    let mut migrator = sqlx::migrate::Migrator::new(directory.clone())
        .await
        .unwrap();
    migrator.set_ignore_missing(true);

    let migrated = server.state.database.migrate(&migrator).await;
    std::fs::remove_dir_all(&directory).unwrap();
    migrated.unwrap();

    let applied: i64 =
        sqlx::query_scalar("select count(*) from _sqlx_migrations where version = 9999")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(applied, 1);
    assert!(
        sqlx::query("select pg_sleep(0.7)")
            .execute(&server.db)
            .await
            .is_err()
    );
}
