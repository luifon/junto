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

#[derive(sqlx::FromRow)]
struct Arrival {
    id: uuid::Uuid,
    position: i64,
    ordinal: i32,
    class: String,
    addressed_to: Option<AccountId>,
    revalidates: Option<AccountId>,
    audience_kind: Option<String>,
    low: Option<String>,
    high: Option<String>,
}

/// Reads new events and wakes the connections of their recorded recipients. Current
/// authorization is still checked by delivery. The first call covers a racing subscription.
pub async fn advance(app: &AppState, seen: Option<i64>) -> ApiResult<i64> {
    let Some(seen) = seen else {
        let head = read!(app.unattended(), |connection| {
            Ok(log::state(connection).await?.head)
        })?;
        app.streams.wake(&Arrivals::everything());
        return Ok(head);
    };
    // The rows after the whole change at `seen`, read in pages by position and ordinal; a
    // page can end inside a change.
    let mut after = (seen, i32::MAX);
    loop {
        let (rows, receivers) = read!(app.unattended(), |connection| {
            let rows: Vec<Arrival> = sqlx::query_as(
                "select id, position, ordinal, class, audience_account_id as addressed_to,
                        revalidates_account_id as revalidates, audience->>'to' as audience_kind,
                        audience->>'low_account_id' as low, audience->>'high_account_id' as high
                 from events where (position, ordinal) > ($1, $3)
                 order by position, ordinal limit $2",
            )
            .bind(after.0)
            .bind(PAGE)
            .bind(after.1)
            .fetch_all(&mut *connection)
            .await?;
            let notices: Vec<_> = rows
                .iter()
                .filter(|row| row.audience_kind.as_deref() == Some("notification"))
                .map(|row| row.id)
                .collect();
            let connected = app.streams.connected_accounts();
            let receivers: Vec<(AccountId, bool)> = if notices.is_empty() || connected.is_empty() {
                Vec::new()
            } else {
                sqlx::query_as("select account_id, background from notification_receivers where event_id=any($1) and account_id=any($2)")
                    .bind(notices).bind(&connected).fetch_all(&mut *connection).await?
            };
            Ok((rows, receivers))
        })?;
        let Some(last) = rows.last() else {
            return Ok(after.0);
        };
        after = (last.position, last.ordinal);
        let full_page = i64::try_from(rows.len()).unwrap_or(i64::MAX) >= PAGE;
        let mut arrivals = Arrivals::default();
        for (account, background) in receivers {
            if background {
                arrivals.accounts.insert(account);
            } else {
                arrivals.full_accounts.insert(account);
            }
        }
        for row in rows {
            match row.audience_kind.as_deref() {
                Some("notification") => continue,
                Some("direct") => {
                    if let (Some(low), Some(high)) = (
                        row.low.and_then(|id| id.parse::<AccountId>().ok()),
                        row.high.and_then(|id| id.parse::<AccountId>().ok()),
                    ) {
                        arrivals.accounts.extend([low, high]);
                    } else {
                        arrivals.other = true;
                    }
                }
                _ => match row.addressed_to {
                    Some(account_id) => {
                        arrivals.accounts.insert(account_id);
                    }
                    None if Class::from_stored(&row.class) == Some(Class::General) => {
                        arrivals.general = true;
                    }
                    None => arrivals.other = true,
                },
            }
            arrivals.revalidate.extend(row.revalidates);
        }
        app.streams.wake(&arrivals);
        if !full_page {
            return Ok(after.0);
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
