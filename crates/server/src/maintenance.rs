//! Removal of rows that no rule reads any more. It runs apart from the requests, so that a
//! request reads and writes only its own rows (ADR 0009).

use std::time::Duration;

use time::OffsetDateTime;

use crate::error::ApiResult;
use crate::state::AppState;
use crate::transact;

pub const INTERVAL: Duration = Duration::from_secs(60);

/// One kind of row that maintenance changes: the statement works on at most `$2` rows that
/// are due at `$1` and returns one row for each row it changed.
const EXPIRED_AT_NOW: [&str; 4] = [
    "delete from challenges where id in (
         select id from challenges where expires_at <= $1 limit $2)
     returning 1",
    "delete from sessions where id in (
         select id from sessions where expires_at <= $1 limit $2)
     returning 1",
    "delete from device_links where id in (
         select id from device_links where expires_at <= $1 limit $2)
     returning 1",
    "update join_requests set state = 'expired' where id in (
         select id from join_requests
         where state = 'pending' and expires_at <= $1 limit $2)
     returning 1",
];

const PAST_THE_HISTORY_PERIOD: &str = "delete from join_requests where id in (
         select id from join_requests
         where state in ('denied', 'expired') and expires_at <= $1 limit $2)
     returning 1";

/// Deletes expired challenges, sessions and device links, marks the join requests that
/// waited too long as expired, and deletes denied and expired join requests that are older
/// than the history period. The work is done in batches, each in its own transaction.
pub async fn run_once(app: &AppState) -> ApiResult<()> {
    let now = app.now();
    for statement in EXPIRED_AT_NOW {
        in_batches(app, statement, now).await?;
    }
    let history_start = now - app.config.limits.join_request_history;
    in_batches(app, PAST_THE_HISTORY_PERIOD, history_start).await
}

async fn in_batches(app: &AppState, statement: &'static str, due: OffsetDateTime) -> ApiResult<()> {
    let batch = i64::from(app.config.limits.maintenance_batch.max(1));
    loop {
        let changed = transact!(app, |connection| {
            let rows: Vec<i32> = sqlx::query_scalar(statement)
                .bind(due)
                .bind(batch)
                .fetch_all(connection)
                .await?;
            Ok(rows.len())
        })?;
        if i64::try_from(changed).unwrap_or(i64::MAX) < batch {
            return Ok(());
        }
    }
}

/// Runs [`run_once`] at each interval until the task is dropped. A failure is logged and the
/// next interval tries again.
pub async fn run(app: AppState) {
    let mut ticks = tokio::time::interval(INTERVAL);
    loop {
        ticks.tick().await;
        if let Err(error) = run_once(&app).await {
            tracing::warn!(code = ?error.code(), "maintenance failed");
        }
    }
}
