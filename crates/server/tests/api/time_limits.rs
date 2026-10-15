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

/// Waits until no backend of this database works. With [`reads_the_log_after_writes_only`],
/// none starts to work afterwards unless a request arrives.
async fn until_no_backend_is_busy(server: &TestServer, holder: i32) {
    let idle = async {
        while busy_backends(server, holder).await > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(PATIENCE, idle)
        .await
        .expect("a backend works while no request runs");
}

/// The server reads the event log in the background at an interval (ADR 0011). A test that
/// counts the backends that work switches the interval off, so that it counts the backends
/// of its requests only.
fn reads_the_log_after_writes_only(config: &mut junto_server::config::Config) {
    config.limits.stream.poll_interval = Duration::hours(1);
}

#[tokio::test]
async fn a_request_blocked_inside_a_statement_gets_the_timeout_error_and_frees_its_connection() {
    let prepared = TestServer::start_with(reads_the_log_after_writes_only).await;
    let owner = prepared.owner().await;
    let workspace = prepared.default_workspace(&owner).await;
    let server = prepared
        .strict(|config| {
            config.limits.database.lock = Duration::milliseconds(300);
            config.limits.database.idle_in_transaction = Duration::seconds(30);
        })
        .await;
    let path = format!("/api/v1/workspaces/{}", workspace.id);
    let (holder, holder_pid) = hold_workspace_row(&server, &workspace).await;
    until_no_backend_is_busy(&server, holder_pid).await;

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
    // Cancellation must release its backend within twice the lock timeout after the replies.
    tokio::time::timeout(
        server.state.config.limits.database.lock.unsigned_abs() * 2,
        until_no_backend_is_busy(&server, holder_pid),
    )
    .await
    .expect("a cancelled statement held its backend beyond twice the lock timeout");
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
    let prepared = TestServer::start().await;
    let owner = prepared.owner().await;
    let workspace = prepared.default_workspace(&owner).await;
    // The request after the blocked ones runs under the same time limit and waits for
    // nothing, so the limit is long enough for it on a slow machine.
    let server = prepared
        .strict(|config| {
            config.limits.request_timeout = Duration::seconds(5);
            config.limits.database.lock = Duration::seconds(10);
            config.limits.database.idle_in_transaction = Duration::minutes(2);
        })
        .await;
    let patience = std::time::Duration::from_secs(30);
    let path = format!("/api/v1/workspaces/{}", workspace.id);
    let (holder, holder_pid) = hold_workspace_row(&server, &workspace).await;

    let body = json!({ "name": "Renamed" });
    let blocked = (0..12)
        .map(|_| server.patch(&owner, &path, &body))
        .collect();
    let replies = tokio::time::timeout(patience, join_all(blocked))
        .await
        .expect("the blocked requests did not return");
    for reply in replies {
        reply.error(ErrorCode::Timeout);
    }

    let settled = tokio::time::timeout(patience, async {
        while busy_backends(&prepared, holder_pid).await > 0 {
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
    let server = TestServer::start().await;
    let database = server
        .database_with(|config| {
            config.limits.database.idle_in_transaction = Duration::milliseconds(400);
        })
        .await;
    let mut abandoned = database.pool().begin().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1_200)).await;
    let result = sqlx::query("select 1").execute(&mut *abandoned).await;
    assert!(result.is_err(), "the idle transaction is still open");
}

#[tokio::test]
async fn a_statement_that_runs_too_long_is_cancelled() {
    let server = TestServer::start().await;
    let database = server
        .database_with(|config| {
            config.limits.database.statement = Duration::milliseconds(300);
        })
        .await;
    let mut limited = database.pool().acquire().await.unwrap();
    let mut reference = server.db.acquire().await.unwrap();

    // The reference starts second and runs for ten times the limit: the statement under
    // the limit has to end before it.
    let cancelled = tokio::select! {
        biased;
        cancelled = sqlx::query("select pg_sleep(20)").execute(&mut *limited) => cancelled,
        _ = sqlx::query("select pg_sleep(3)").execute(&mut *reference) => {
            panic!("the statement ran for ten times its time limit")
        }
    };
    let error = cancelled.expect_err("the statement ran to its end");
    assert!(junto_server::database::is_timeout(&error), "{error}");
}

#[tokio::test]
async fn migrations_are_exempt_from_the_statement_time_limit() {
    const SLOW: &str = "select pg_sleep(0.7)";
    let server = TestServer::start().await;
    let database = server
        .database_with(|config| {
            config.limits.database.statement = Duration::milliseconds(200);
        })
        .await;
    let directory = std::env::temp_dir().join(format!("junto-migration-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("9999_slow.sql"), format!("{SLOW};")).unwrap();
    let mut migrator = sqlx::migrate::Migrator::new(directory.clone())
        .await
        .unwrap();
    migrator.set_ignore_missing(true);

    let migrated = database.migrate(&migrator).await;
    std::fs::remove_dir_all(&directory).unwrap();
    migrated.unwrap();

    let applied: i64 =
        sqlx::query_scalar("select count(*) from _sqlx_migrations where version = 9999")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(applied, 1);
    let error = sqlx::query(SLOW)
        .execute(database.pool())
        .await
        .expect_err("the statement of the migration ran to its end on the pool");
    assert!(junto_server::database::is_timeout(&error), "{error}");
}

/// When PostgreSQL stops answering for a while, every connection of the pool stays busy or
/// fails its check, and a request gets no connection within the wait for one. That request
/// changed nothing, so it gets `busy` with a time to wait, not an internal error (ADR 0009).
#[tokio::test]
async fn a_request_that_gets_no_connection_of_the_pool_gets_the_busy_error() {
    let prepared = TestServer::start().await;
    let owner = prepared.owner().await;
    let server = prepared
        .strict(|config| config.limits.database.acquire = Duration::milliseconds(300))
        .await;
    let connections = server.db.options().get_max_connections();
    // A task of the server can hold a connection for a moment; each is taken when it is free.
    let take_all = async {
        let mut taken = Vec::new();
        while taken.len() < connections as usize {
            if let Ok(connection) = server.db.acquire().await {
                taken.push(connection);
            }
        }
        taken
    };
    let taken = tokio::time::timeout(PATIENCE, take_all)
        .await
        .expect("the connections of the pool did not become free");

    let refused =
        tokio::time::timeout(PATIENCE, server.post(&owner, "/api/v1/invites", &json!({})))
            .await
            .expect("the request did not return");
    assert_eq!(
        refused.status,
        StatusCode::SERVICE_UNAVAILABLE,
        "{}",
        refused.text()
    );
    refused.error(ErrorCode::Busy);
    assert!(refused.retry_after.is_some());

    drop(taken);
    server
        .post(&owner, "/api/v1/invites", &json!({}))
        .await
        .ok::<serde_json::Value>(StatusCode::CREATED);
}
