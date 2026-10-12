//! Speaking marks for the people outside a voice channel (ADR 0507, ADR 0513). The clients
//! inside report who speaks; the server keeps the reports in memory and records an event when
//! the set of speakers of a call has changed, at most once per interval.

use std::collections::HashMap;
use std::sync::PoisonError;

use junto_protocol::call::CallPlace;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, CallId};
use time::OffsetDateTime;

use crate::calls::store::{self, By};
use crate::error::ApiResult;
use crate::events;
use crate::state::AppState;
use crate::transact;

#[derive(Default)]
struct CallMarks {
    /// The accounts that reported that they speak, each with the time at which the report
    /// ends unless it is renewed.
    until: HashMap<AccountId, OffsetDateTime>,
    /// The set of speakers that the last event of the call carried.
    sent: Vec<AccountId>,
    sent_at: Option<OffsetDateTime>,
}

#[derive(Default)]
pub struct Marks {
    calls: HashMap<CallId, CallMarks>,
}

fn marks(app: &AppState) -> std::sync::MutexGuard<'_, Marks> {
    app.calls
        .speaking
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
}

/// Keeps the report of one account. The caller has checked that the account is in the call.
pub fn report(app: &AppState, call_id: CallId, account_id: AccountId, speaking: bool) {
    let until = app.now() + app.config.limits.calls.speaking_mark_lifetime;
    let mut marks = marks(app);
    let call = marks.calls.entry(call_id).or_default();
    if speaking {
        call.until.insert(account_id, until);
    } else {
        call.until.remove(&account_id);
    }
}

/// The calls that can need an event now, each with the accounts that report that they speak
/// and the set that the last event carried: the calls with a report or with a sent set that
/// is not empty, whose last event is at least one interval old.
fn candidates(
    app: &AppState,
    now: OffsetDateTime,
) -> Vec<(CallId, Vec<AccountId>, Vec<AccountId>)> {
    let limits = &app.config.limits.calls;
    let mut marks = marks(app);
    marks.calls.retain(|_, call| {
        call.until.retain(|_, until| *until > now);
        !(call.until.is_empty() && call.sent.is_empty())
    });
    marks
        .calls
        .iter()
        .filter(|(_, call)| {
            call.sent_at
                .is_none_or(|at| now - at >= limits.speaking_interval)
        })
        .map(|(call_id, call)| {
            let reported = call.until.keys().copied().collect();
            (*call_id, reported, call.sent.clone())
        })
        .collect()
}

/// What the look at one call found.
enum Found {
    /// The call has ended, or it is a direct-message call.
    NoMarks,
    /// The set of speakers is the one that was sent.
    Unchanged,
    Sent(Vec<AccountId>),
}

/// Records one speaking event for each call whose set of speakers differs from the set that
/// its last event carried. The set is computed from the record at this moment: the people
/// who report that they speak, whom the media server lists, whose own microphone setting is
/// not muted and who have no microphone block. A block, a mute, a leave and a hidden channel
/// therefore change the set at the next interval, also while the reports stay the same. A
/// call that hides who is inside has the empty set. Returns the number of events.
pub async fn publish(app: &AppState) -> ApiResult<usize> {
    let now = app.now();
    let most = i64::try_from(app.config.limits.calls.most_speaking_marks).unwrap_or(i64::MAX);
    let mut recorded = 0;
    for (call_id, reported, sent) in candidates(app, now) {
        let found = transact!(app.background(), |connection| {
            let Some(call) = store::call(connection, By::Id(call_id), false).await? else {
                return Ok(Found::NoMarks);
            };
            let CallPlace::Channel { .. } = call.place else {
                return Ok(Found::NoMarks);
            };
            let speaking: Vec<AccountId> = sqlx::query_scalar(
                "select m.account_id from call_members m
                 where m.call_id = $1 and m.account_id = any($2)
                   and m.connected_at is not null and m.absent_since is null
                   and not m.self_muted
                   and not exists (
                       select 1 from call_microphone_blocks b
                       where b.call_id = m.call_id and b.account_id = m.account_id)
                   and not $3
                 order by m.account_id
                 limit $4",
            )
            .bind(call_id)
            .bind(&reported)
            .bind(call.hides_occupants)
            .bind(most)
            .fetch_all(&mut *connection)
            .await?;
            if speaking == sent {
                return Ok(Found::Unchanged);
            }
            let event = Event::CallSpeaking {
                call_id,
                place: call.place,
                account_ids: speaking.clone(),
            };
            events::publish(connection, now, &event).await?;
            Ok(Found::Sent(speaking))
        })?;
        let mut marks = marks(app);
        match found {
            Found::NoMarks => {
                marks.calls.remove(&call_id);
            }
            Found::Unchanged => {}
            Found::Sent(speaking) => {
                if let Some(call) = marks.calls.get_mut(&call_id) {
                    call.sent = speaking;
                    call.sent_at = Some(now);
                }
                recorded += 1;
            }
        }
    }
    Ok(recorded)
}

/// Runs [`publish`] at every interval until the task is dropped.
pub async fn run(app: AppState) {
    let interval = app.config.limits.calls.speaking_interval.unsigned_abs();
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        if let Err(error) = publish(&app).await {
            tracing::warn!(code = ?error.code(), "the speaking marks could not be published");
        }
    }
}
