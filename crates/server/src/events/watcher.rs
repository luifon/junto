//! Tells the connections of this process that the log has grown. It reads the log after
//! every committed write of this process and at a fixed interval, which covers the writes of
//! other processes (ADR 0011).

use junto_protocol::ids::AccountId;

use crate::db::StoredText;
use crate::error::ApiResult;
use crate::events::audience::Class;
use crate::events::hub::Arrivals;
use crate::events::log;
use crate::read;
use crate::state::AppState;

const PAGE: i64 = 500;

type Arrival = (i64, String, Option<AccountId>, Option<AccountId>);

/// Reads the events after `seen` and wakes the connections they can concern. Returns the new
/// value of `seen`. On the first call it wakes every connection, because a connection can
/// have read the log before the watcher did.
pub async fn advance(app: &AppState, seen: Option<i64>) -> ApiResult<i64> {
    let Some(mut seen) = seen else {
        let head = read!(app.unattended(), |connection| {
            Ok(log::state(connection).await?.head)
        })?;
        app.streams.wake(&Arrivals::everything());
        return Ok(head);
    };
    loop {
        let rows: Vec<Arrival> = read!(app.unattended(), |connection| {
            Ok(sqlx::query_as(
                "select position, class, audience_account_id, revalidates_account_id
                 from events
                 where position > $1
                 order by position
                 limit $2",
            )
            .bind(seen)
            .bind(PAGE)
            .fetch_all(connection)
            .await?)
        })?;
        let Some(last) = rows.last() else {
            return Ok(seen);
        };
        seen = last.0;
        let full_page = i64::try_from(rows.len()).unwrap_or(i64::MAX) >= PAGE;
        let mut arrivals = Arrivals::default();
        for (_, class, addressed_to, revalidates) in rows {
            match addressed_to {
                Some(account_id) => {
                    arrivals.accounts.insert(account_id);
                }
                None if Class::from_stored(&class) == Some(Class::General) => {
                    arrivals.general = true;
                }
                None => arrivals.other = true,
            }
            arrivals.revalidate.extend(revalidates);
        }
        app.streams.wake(&arrivals);
        if !full_page {
            return Ok(seen);
        }
    }
}

/// Runs [`advance`] until the task is dropped. A failure is logged and the next round tries
/// again.
pub async fn run(app: AppState) {
    let interval = app.config.limits.stream.poll_interval.unsigned_abs();
    let mut seen = None;
    loop {
        match advance(&app, seen).await {
            Ok(position) => seen = Some(position),
            Err(error) => tracing::warn!(code = ?error.code(), "the event log could not be read"),
        }
        tokio::select! {
            () = app.database.written() => {}
            () = tokio::time::sleep(interval) => {}
        }
    }
}
