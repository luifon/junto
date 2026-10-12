//! The removal of the content of deleted channels and workspaces (ADR 0017).
//!
//! A deletion marks its rows in the transaction of its request and writes one row here for
//! each channel and workspace. This task removes what they hold, in transactions that each
//! remove at most one maintenance batch of rows, so that no deletion holds a transaction
//! open for a time that grows with the content. The rows that depend on a message or on a
//! recording in numbers without a small limit are removed first, in batches of their own,
//! so that the removal of a message or of a recording removes few further rows.

use std::time::Duration;

use junto_protocol::events::Event;
use junto_protocol::export::ExportRemoval;
use junto_protocol::ids::{AccountId, ChannelId, ExportId, RecordingId, WorkspaceId};
use sqlx::PgConnection;
use time::OffsetDateTime;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::recordings::files::Place;
use crate::recordings::store as recordings;
use crate::state::AppState;
use crate::{storage, transact};

/// The task looks for subjects at this interval and when a deletion wakes it.
pub const INTERVAL: Duration = Duration::from_secs(60);

/// A subject that waits for the end of a call or of a recording is taken again after this
/// time.
const WAIT: time::Duration = time::Duration::seconds(30);

/// What a purge removes the content of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subject {
    Channel(ChannelId),
    Workspace(WorkspaceId),
}

impl Subject {
    const fn stored(self) -> (&'static str, Uuid) {
        match self {
            Self::Channel(id) => ("channel", id.as_uuid()),
            Self::Workspace(id) => ("workspace", id.as_uuid()),
        }
    }
}

/// Wakes the task after a deletion.
#[derive(Default)]
pub struct Purges {
    wake: Notify,
}

impl Purges {
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    async fn woken(&self) {
        self.wake.notified().await;
    }
}

/// States that the content of `subject` is to be removed, in the transaction that deletes
/// the subject.
pub async fn queue(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    subject: Subject,
) -> ApiResult<()> {
    let (kind, id) = subject.stored();
    sqlx::query(
        "insert into purges (id, kind, requested_at) values ($1, $2, $3)
         on conflict (id) do nothing",
    )
    .bind(id)
    .bind(kind)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

/// One statement of the purge of a channel: it removes at most `$2` rows of the place `$1`
/// and returns one row for each row that it removed. `$3` is the time.
pub const CHANNEL_STEPS: [(&str, &str); 9] = [
    (
        "attachments",
        "update attachments set removed_at = $3 where id in (
             select id from attachments where place_id = $1 and removed_at is null
             order by id limit $2)
         returning 1",
    ),
    (
        "reply references",
        "update messages set reply_to = null where id in (
             select id from messages where place_id = $1 and reply_to is not null
             order by id limit $2)
         returning 1",
    ),
    (
        "replies",
        "delete from messages where id in (
             select id from messages where place_id = $1 and thread_root is not null
             order by id limit $2)
         returning 1",
    ),
    (
        "messages",
        "delete from messages where id in (
             select id from messages where place_id = $1 order by position, id limit $2)
         returning 1",
    ),
    (
        "read positions",
        "delete from message_reads where (place_id, account_id) in (
             select place_id, account_id from message_reads where place_id = $1
             order by account_id limit $2)
         returning 1",
    ),
    (
        "notification settings",
        "delete from notification_settings where (account_id, scope_id) in (
             select account_id, scope_id from notification_settings where scope_id = $1
             order by account_id limit $2)
         returning 1",
    ),
    (
        "call notes",
        "delete from call_notes where id in (
             select id from call_notes where channel_id = $1 order by id limit $2)
         returning 1",
    ),
    (
        "private call members",
        "delete from call_message_members where (call_id, account_id) in (
             select m.call_id, m.account_id
             from call_message_privacy p
             join call_message_members m on m.call_id = p.call_id
             where p.channel_id = $1 limit $2)
         returning 1",
    ),
    (
        "private call lines",
        "delete from call_message_privacy where call_id in (
             select call_id from call_message_privacy where channel_id = $1
             order by call_id limit $2)
         returning 1",
    ),
];

/// The next messages of the place `$1` after the position `($2, $3)`. At most `$4`.
pub const MESSAGE_PAGE: &str = "select id, position from messages
     where place_id = $1 and (position, id) > ($2, $3)
     order by position, id limit $4";

/// The tables whose rows name a message and have no small limit for one message, with the
/// column that names the message: one row for each account that follows a thread, is
/// notified, is mentioned or reacted. The messages that answer a message are the step
/// `reply references` of [`CHANNEL_STEPS`].
pub const MESSAGE_DEPENDENTS: [(&str, &str); 6] = [
    ("thread_reads", "root_id"),
    ("notifications", "message_id"),
    ("message_mention_accounts", "message_id"),
    ("message_mentions_notified", "message_id"),
    ("message_mentions", "message_id"),
    ("message_reactions", "message_id"),
];

/// The statement that removes at most `$2` rows of `table` that name one of the messages
/// `$1`, and returns one row for each row that it removed.
pub fn dependents_statement(table: &str, column: &str) -> String {
    format!(
        "delete from {table} where ctid = any (array(
             select ctid from {table} where {column} = any($1) limit $2))
         returning 1"
    )
}

/// One statement of the purge of a recording: it removes at most `$2` rows of the
/// transcript of the recording `$1` and returns one row for each row that it removed.
pub const TRANSCRIPT_STEPS: [(&str, &str); 3] = [
    (
        "speaker names",
        "delete from transcript_speaker_names where id in (
             select id from transcript_speaker_names where recording_id = $1
             order by speaker, id limit $2)
         returning 1",
    ),
    (
        "transcript corrections",
        "delete from transcript_corrections where id in (
             select id from transcript_corrections where recording_id = $1
             order by line, id limit $2)
         returning 1",
    ),
    (
        "transcript lines",
        "delete from transcript_lines where (recording_id, line) in (
             select recording_id, line from transcript_lines where recording_id = $1
             order by line limit $2)
         returning 1",
    ),
];

/// The recordings of a channel that the purge removes next: the ones in a final state.
pub const RECORDINGS_STEP: &str = "select id from recordings
     where channel_id = $1 and state in ('ready', 'partly_failed', 'failed')
     order by id limit $2";

/// The exports of a place that the purge removes next.
pub const EXPORTS_STEP: &str = "update exports set removed_at = $3 where id in (
         select id from exports where place_id = $1 and removed_at is null
         order by id limit $2)
     returning id, account_id";

/// Whether a call or a recording of the channel `$1` still runs. The purge waits for its
/// end, because the recorder writes the files of a recording until then.
pub const RUNS: &str = "select exists (select 1 from calls where channel_id = $1)
     or exists (
         select 1 from recordings
         where channel_id = $1 and state in ('starting', 'recording', 'processing'))";

const NEXT: &str = "select id, kind, requested_at from purges
     where (requested_at, id) > ($1, $2) and (retry_at is null or retry_at <= $3)
     order by requested_at, id limit 1";

enum Outcome {
    Finished,
    /// A call or a recording of the channel still runs.
    Waits,
}

fn batch(app: &AppState) -> i64 {
    i64::from(app.config.limits.maintenance_batch.max(1))
}

/// Runs one statement of [`CHANNEL_STEPS`] or of [`TRANSCRIPT_STEPS`] until it removes
/// less than a batch. With `timed`, the statement takes the time as `$3`.
async fn in_batches(
    app: &AppState,
    statement: &'static str,
    subject: Uuid,
    timed: bool,
) -> ApiResult<()> {
    let batch = batch(app);
    loop {
        let now = app.now();
        let removed = transact!(app, |connection| {
            let mut query = sqlx::query_scalar(statement).bind(subject).bind(batch);
            if timed {
                query = query.bind(now);
            }
            let rows: Vec<i32> = query.fetch_all(connection).await?;
            Ok(rows.len())
        })?;
        if i64::try_from(removed).unwrap_or(i64::MAX) < batch {
            return Ok(());
        }
    }
}

/// Removes the rows of [`MESSAGE_DEPENDENTS`] for every message of a deleted channel, one
/// page of messages after the other.
async fn remove_dependents(app: &AppState, channel_id: ChannelId) -> ApiResult<()> {
    let batch = batch(app);
    let mut after = (i64::MIN, Uuid::nil());
    loop {
        let page: Vec<(Uuid, i64)> = crate::read!(app, |connection| {
            Ok(sqlx::query_as(MESSAGE_PAGE)
                .bind(channel_id)
                .bind(after.0)
                .bind(after.1)
                .bind(batch)
                .fetch_all(connection)
                .await?)
        })?;
        let Some((last, position)) = page.last() else {
            return Ok(());
        };
        after = (*position, *last);
        let ids: Vec<Uuid> = page.iter().map(|(id, _)| *id).collect();
        for (table, column) in MESSAGE_DEPENDENTS {
            let statement = dependents_statement(table, column);
            loop {
                let removed = transact!(app, |connection| {
                    let rows: Vec<i32> = sqlx::query_scalar(sqlx::AssertSqlSafe(statement.clone()))
                        .bind(&ids)
                        .bind(batch)
                        .fetch_all(connection)
                        .await?;
                    Ok(rows.len())
                })?;
                if i64::try_from(removed).unwrap_or(i64::MAX) < batch {
                    break;
                }
            }
        }
        if i64::try_from(page.len()).unwrap_or(i64::MAX) < batch {
            return Ok(());
        }
    }
}

/// Removes the recordings of a deleted channel with their segments and transcripts, one
/// recording after the other, and puts their directories into the queue of removals
/// (ADR 0608). The lines and the corrections of a transcript are removed in batches before
/// their recording. No event and no audit entry: nobody reads a recording of a deleted
/// channel, and the deletion of the channel has its entry.
async fn remove_recordings(app: &AppState, channel_id: ChannelId) -> ApiResult<()> {
    loop {
        let next: Option<RecordingId> = crate::read!(app, |connection| {
            Ok(sqlx::query_scalar(RECORDINGS_STEP)
                .bind(channel_id)
                .bind(1_i64)
                .fetch_optional(connection)
                .await?)
        })?;
        let Some(id) = next else {
            return Ok(());
        };
        for (_, statement) in TRANSCRIPT_STEPS {
            in_batches(app, statement, id.as_uuid(), false).await?;
        }
        let now = app.now();
        transact!(app, |connection| {
            let lost_jobs: Vec<String> = sqlx::query_scalar(
                "select job_id from recording_segments
                 where recording_id = $1 and failure = 'recorder_lost'
                   and job_id is not null",
            )
            .bind(id)
            .fetch_all(&mut *connection)
            .await?;
            for job_id in lost_jobs {
                if let Some(rescued) = Place::rescued(&job_id) {
                    recordings::queue_removal(&mut *connection, now, &rescued).await?;
                }
            }
            let removed = sqlx::query("delete from recordings where id = $1")
                .bind(id)
                .execute(&mut *connection)
                .await?;
            if removed.rows_affected() == 0 {
                return Ok(());
            }
            sqlx::query("delete from storage_reservations where subject_id = $1")
                .bind(id)
                .execute(&mut *connection)
                .await?;
            recordings::queue_removal(&mut *connection, now, &Place::recording(id)).await
        })?;
    }
}

/// Removes the exports of a deleted channel. Each account is told that its export is gone.
async fn remove_exports(app: &AppState, channel_id: ChannelId) -> ApiResult<()> {
    let batch = batch(app);
    loop {
        let now = app.now();
        let removed = transact!(app, |connection| {
            let rows: Vec<(ExportId, AccountId)> = sqlx::query_as(EXPORTS_STEP)
                .bind(channel_id)
                .bind(batch)
                .bind(now)
                .fetch_all(&mut *connection)
                .await?;
            for (export_id, account_id) in &rows {
                storage::release(&mut *connection, export_id.as_uuid()).await?;
                let event = Event::ExportRemoved {
                    export_id: *export_id,
                    reason: ExportRemoval::PlaceDeleted,
                };
                events::tell_account(&mut *connection, now, *account_id, &event).await?;
            }
            Ok(rows.len())
        })?;
        if i64::try_from(removed).unwrap_or(i64::MAX) < batch {
            return Ok(());
        }
    }
}

async fn purge_channel(app: &AppState, channel_id: ChannelId) -> ApiResult<Outcome> {
    let runs: bool = crate::read!(app, |connection| {
        Ok(sqlx::query_scalar(RUNS)
            .bind(channel_id)
            .fetch_one(connection)
            .await?)
    })?;
    if runs {
        return Ok(Outcome::Waits);
    }
    remove_recordings(app, channel_id).await?;
    remove_exports(app, channel_id).await?;
    remove_dependents(app, channel_id).await?;
    for (_, statement) in CHANNEL_STEPS {
        in_batches(app, statement, channel_id.as_uuid(), true).await?;
    }
    transact!(app, |connection| {
        // A recording that started while the purge ran is found by the next pass.
        let runs: bool = sqlx::query_scalar(RUNS)
            .bind(channel_id)
            .fetch_one(&mut *connection)
            .await?;
        let left: bool = sqlx::query_scalar(
            "select exists (select 1 from recordings where channel_id = $1)
                 or exists (select 1 from messages where place_id = $1)",
        )
        .bind(channel_id)
        .fetch_one(&mut *connection)
        .await?;
        if runs || left {
            return Ok(Outcome::Waits);
        }
        sqlx::query("update channels set permanent_message_count = 0 where id = $1")
            .bind(channel_id)
            .execute(&mut *connection)
            .await?;
        sqlx::query("delete from purges where id = $1")
            .bind(channel_id)
            .execute(&mut *connection)
            .await?;
        Ok(Outcome::Finished)
    })
}

/// The rows of a deleted workspace that its deletion left: the personal notification
/// settings for the workspace.
async fn purge_workspace(app: &AppState, workspace_id: WorkspaceId) -> ApiResult<Outcome> {
    let settings = CHANNEL_STEPS
        .iter()
        .find(|(name, _)| *name == "notification settings")
        .map(|(_, statement)| *statement)
        .ok_or_else(|| ApiError::internal(&"the purge has no statement for the settings"))?;
    in_batches(app, settings, workspace_id.as_uuid(), true).await?;
    transact!(app, |connection| {
        sqlx::query("delete from purges where id = $1")
            .bind(workspace_id)
            .execute(connection)
            .await?;
        Ok(Outcome::Finished)
    })
}

/// Removes the content of every subject that waits, oldest first. A subject whose call or
/// recording still runs stays and is taken again later. Returns the number of subjects
/// that it finished.
pub async fn run_once(app: &AppState) -> ApiResult<usize> {
    let mut finished = 0;
    let mut after = (OffsetDateTime::UNIX_EPOCH, Uuid::nil());
    loop {
        let now = app.now();
        let next: Option<(Uuid, String, OffsetDateTime)> = crate::read!(app, |connection| {
            Ok(sqlx::query_as(NEXT)
                .bind(after.0)
                .bind(after.1)
                .bind(now)
                .fetch_optional(connection)
                .await?)
        })?;
        let Some((id, kind, requested_at)) = next else {
            break;
        };
        after = (requested_at, id);
        let outcome = match kind.as_str() {
            "channel" => purge_channel(app, ChannelId::from_uuid(id)).await?,
            "workspace" => purge_workspace(app, WorkspaceId::from_uuid(id)).await?,
            other => {
                return Err(ApiError::internal(&format_args!(
                    "unknown kind {other:?} of a purge"
                )));
            }
        };
        match outcome {
            Outcome::Finished => finished += 1,
            Outcome::Waits => {
                let again = app.now() + WAIT;
                transact!(app, |connection| {
                    sqlx::query("update purges set retry_at = $2 where id = $1")
                        .bind(id)
                        .bind(again)
                        .execute(connection)
                        .await?;
                    Ok(())
                })?;
            }
        }
    }
    if finished > 0 {
        // The directories of the removed recordings wait in the queue of removals.
        app.recordings.process_now();
        app.exports.wake();
    }
    Ok(finished)
}

/// Runs [`run_once`] after each deletion and at each interval until the task is dropped.
pub async fn run(app: AppState) {
    loop {
        if let Err(error) = run_once(&app).await {
            tracing::warn!(code = ?error.code(), "the removal of deleted content failed");
        }
        tokio::select! {
            () = app.purges.woken() => {}
            () = tokio::time::sleep(INTERVAL) => {}
        }
    }
}
