//! Removal of rows that no rule reads any more. It runs apart from the requests, so that a
//! request reads and writes only its own rows (ADR 0009).

use std::time::Duration;

use time::OffsetDateTime;

use crate::calls::store as calls;
use crate::error::ApiResult;
use crate::events;
use crate::state::AppState;
use crate::transact;

pub const INTERVAL: Duration = Duration::from_secs(60);

/// One kind of row that maintenance changes: the statement works on at most `$2` rows that
/// are due at `$1` and returns one row for each row it changed.
const EXPIRED_AT_NOW: [&str; 5] = [
    "delete from challenges where id in (
         select id from challenges where expires_at <= $1 limit $2)
     returning 1",
    "delete from device_links where id in (
         select id from device_links where expires_at <= $1 limit $2)
     returning 1",
    "update join_requests set state = 'expired' where id in (
         select id from join_requests
         where state = 'pending' and expires_at <= $1 limit $2)
     returning 1",
    "delete from access_tokens where id in (
         select id from access_tokens where expires_at <= $1 limit $2)
     returning 1",
    "delete from direct_mutes where (account_id, other_account_id) in (
         select account_id, other_account_id from direct_mutes
         where muted_until <= $1 limit $2)
     returning 1",
];

const EXPIRED_SESSIONS: &str = "delete from sessions where id in (
         select id from sessions where expires_at <= $1 limit $2)
     returning 1";

const RINGS_THAT_NO_LIMIT_COUNTS: &str = "delete from direct_call_rings where id in (
         select id from direct_call_rings where rung_at <= $1 limit $2)
     returning 1";

const PAST_THE_HISTORY_PERIOD: &str = "delete from join_requests where id in (
         select id from join_requests
         where state in ('denied', 'expired') and expires_at <= $1 limit $2)
     returning 1";

const ENDED_SESSIONS_PAST_THEIR_LIFETIME: &str = "delete from ended_sessions
     where session_id in (
         select session_id from ended_sessions where ended_at <= $1 limit $2)
     returning 1";

/// Deletes expired challenges, sessions, access tokens, device links and mutes, marks the join requests that
/// waited too long as expired, deletes denied and expired join requests that are older than
/// the history period, deletes the stored reasons of sessions that would have expired by now
/// and the rings of direct-message calls that no limit counts any more, and removes the
/// events that the log no longer keeps. It applies the retention settings, records the
/// notices of archived channels and deletes the archived channels whose period has ended
/// (ADR 0410), removes the exports whose lifetime has ended (ADR 0411) and the content of
/// deleted channels and workspaces (ADR 0017). The work is done in batches, each in its own
/// transaction.
pub async fn run_once(app: &AppState) -> ApiResult<()> {
    let now = app.now();
    for statement in EXPIRED_AT_NOW {
        in_batches(app, statement, now).await?;
    }
    expire_sessions(app, now).await?;
    let history_start = now - app.config.limits.join_request_history;
    in_batches(app, PAST_THE_HISTORY_PERIOD, history_start).await?;
    let kept_from = now - app.config.limits.session_lifetime;
    in_batches(app, ENDED_SESSIONS_PAST_THEIR_LIFETIME, kept_from).await?;
    let calls = &app.config.limits.calls;
    let counted = calls.rings.window.max(calls.ring_pause_after_decline);
    let rings_from = now - counted - calls.invitation_lifetime;
    in_batches(app, RINGS_THAT_NO_LIMIT_COUNTS, rings_from).await?;
    events::log::trim(app).await?;
    crate::messages::retention::run(app).await?;
    crate::archive::notices(app).await?;
    crate::archive::delete_due(app).await?;
    crate::exports::expire(app).await?;
    crate::purge::run_once(app).await?;
    crate::messages::uploads::cleanup(app).await?;
    Ok(())
}

/// Deletes the expired sessions. The transaction that deletes sessions also ends the places
/// in calls whose session has expired, so that an expired session holds no identity in a
/// room (ADR 0510).
async fn expire_sessions(app: &AppState, now: OffsetDateTime) -> ApiResult<()> {
    let batch = i64::from(app.config.limits.maintenance_batch.max(1));
    loop {
        let expiring = async {
            transact!(app, |connection| {
                let places =
                    calls::end_places_without_a_session(connection, &app.calls, now, batch).await?;
                let sessions: Vec<i32> = sqlx::query_scalar(EXPIRED_SESSIONS)
                    .bind(now)
                    .bind(batch)
                    .fetch_all(connection)
                    .await?;
                Ok(places.max(sessions.len()))
            })
        };
        let changed: usize = crate::calls::following(app, expiring).await?;
        if i64::try_from(changed).unwrap_or(i64::MAX) < batch {
            return Ok(());
        }
    }
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
