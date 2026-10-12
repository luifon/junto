//! Exports of a channel or of a direct-message conversation (ADR 0411): the stored
//! exports, their removal, and the task that writes their files.

pub mod build;
pub mod html;
pub mod zip;

use std::path::{Path, PathBuf};
use std::time::Duration;

use junto_authz::{Request, WorkspacePermission};
use junto_protocol::call::CallPlace;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::export::{Export, ExportFailure, ExportRemoval, ExportState};
use junto_protocol::ids::{AccountId, ChannelId, ConversationId, ExportId, WorkspaceId};
use junto_protocol::message::MessagePlace;
use sqlx::PgConnection;
use time::OffsetDateTime;
use tokio::sync::Notify;
use uuid::Uuid;

use crate::access::Access;
use crate::db::StoredText;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::events::audience::Audience;
use crate::messages::Place;
use crate::recordings::files;
use crate::state::AppState;
use crate::storage::{self, Space};
use crate::transact;

/// The name of the file of an export in its directory.
pub const FILE: &str = "export.zip";

macro_rules! stored {
    ($name:ident { $($variant:ident => $text:literal,)+ }) => {
        impl StoredText for $name {
            fn stored(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                }
            }

            fn from_stored(text: &str) -> Option<Self> {
                match text {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

stored!(ExportState {
    Waiting => "waiting",
    Running => "running",
    Ready => "ready",
    Failed => "failed",
});

stored!(ExportFailure {
    StorageLow => "storage_low",
    AccessEnded => "access_ended",
    Interrupted => "interrupted",
    Failed => "failed",
});

/// What a server with a directory of exports has, and what wakes its task.
pub struct Exports {
    directory: Option<PathBuf>,
    space: Space,
    wake: Notify,
    /// Names this server process in the lease of a build.
    instance: Uuid,
}

impl Exports {
    pub fn new(directory: Option<PathBuf>) -> Self {
        Self {
            space: Space::of(directory.clone()),
            directory,
            wake: Notify::new(),
            instance: Uuid::new_v4(),
        }
    }

    pub fn instance(&self) -> Uuid {
        self.instance
    }

    /// The directory of the exports. A server without one makes no export.
    pub fn directory(&self) -> ApiResult<&Path> {
        self.directory.as_deref().ok_or_else(|| {
            ApiError::new(
                ErrorCode::Unavailable,
                "this server has no directory for exports; no export can be made",
            )
        })
    }

    /// The disk of the exports, whose free space the storage guard reads.
    pub fn space(&self) -> &Space {
        &self.space
    }

    /// The free disk space of the exports. Absent on a server without the directory and
    /// when the space cannot be read.
    pub fn free_bytes(&self) -> Option<u64> {
        self.space.free_bytes()
    }

    /// Makes the task look for an export to write and for files to delete now.
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    async fn woken(&self) {
        self.wake.notified().await;
    }
}

/// One stored export that no route hides.
#[derive(Debug, Clone)]
pub struct ExportRow {
    pub export: Export,
    pub account_id: AccountId,
}

type Row = (
    ExportId,
    AccountId,
    String,
    Option<String>,
    OffsetDateTime,
    Option<OffsetDateTime>,
    Option<OffsetDateTime>,
    Option<i64>,
    Option<ChannelId>,
    Option<WorkspaceId>,
    Option<ConversationId>,
    Option<AccountId>,
    Option<AccountId>,
);

const SELECT: &str = "select e.id, e.account_id, e.state, e.failure, e.requested_at, \
     e.finished_at, e.expires_at, e.bytes, ch.id, ch.workspace_id, cv.id, cv.low_account_id, \
     cv.high_account_id \
     from exports e \
     left join channels ch on ch.id = e.place_id \
     left join conversations cv on cv.id = e.place_id";

fn row(row: Row) -> ApiResult<ExportRow> {
    let (
        id,
        account_id,
        state,
        failure,
        requested_at,
        finished_at,
        expires_at,
        bytes,
        channel_id,
        workspace_id,
        conversation_id,
        low,
        high,
    ) = row;
    let place = match (channel_id, workspace_id, conversation_id, low, high) {
        (Some(channel_id), Some(workspace_id), ..) => MessagePlace::Channel {
            workspace_id,
            channel_id,
        },
        (_, _, Some(conversation_id), Some(low_account_id), Some(high_account_id)) => {
            MessagePlace::Direct {
                conversation_id,
                low_account_id,
                high_account_id,
            }
        }
        _ => return Err(ApiError::internal(&"an export without a place")),
    };
    Ok(ExportRow {
        export: Export {
            id,
            place,
            state: ExportState::read(&state)?,
            failure: failure.as_deref().map(ExportFailure::read).transpose()?,
            requested_at,
            finished_at,
            expires_at,
            bytes: bytes.and_then(|bytes| u64::try_from(bytes).ok()),
        },
        account_id,
    })
}

/// One export by its identifier. With `for_change`, its row is locked.
pub async fn export(
    connection: &mut PgConnection,
    id: ExportId,
    for_change: bool,
) -> ApiResult<Option<ExportRow>> {
    let lock = if for_change { " for update of e" } else { "" };
    let found: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} where e.id = $1 and e.removed_at is null{lock}"
    )))
    .bind(id)
    .fetch_optional(connection)
    .await?;
    found.map(row).transpose()
}

/// The exports of an account, the newest first. Their number has a ceiling.
pub async fn of_account(
    connection: &mut PgConnection,
    account_id: AccountId,
) -> ApiResult<Vec<ExportRow>> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} where e.account_id = $1 and e.removed_at is null order by e.id desc"
    )))
    .bind(account_id)
    .fetch_all(connection)
    .await?;
    rows.into_iter().map(row).collect()
}

/// The place of an export when the account may export it now: for a channel, the account
/// sees it and holds `export_channel` in it; for a direct-message conversation, the account
/// is a participant and may reach the other one (ADR 0302). `None` otherwise. Every
/// request for an export, every page of its build and every download ask this.
pub async fn permitted(
    connection: &mut PgConnection,
    access: &Access,
    place: MessagePlace,
) -> ApiResult<Option<Place>> {
    let opened = match place {
        MessagePlace::Channel { channel_id, .. } => {
            Place::channel(connection, access, channel_id).await
        }
        MessagePlace::Direct {
            conversation_id, ..
        } => Place::direct(connection, access, conversation_id).await,
    };
    let opened = match opened {
        Ok(opened) => opened,
        Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::Forbidden) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    if let Some(scope) = &opened.scope
        && !access.allows(Request::Channel {
            scope,
            permission: WorkspacePermission::ExportChannel,
        })
    {
        return Ok(None);
    }
    Ok(Some(opened))
}

/// Whether the file of the export `$1` holds a recording of a call that still runs and
/// hides who is inside, while the account `$2` is not inside (ADR 0517).
pub const HIDDEN_RECORDING: &str = "select exists (
         select 1 from exports e
         where e.id = $1 and e.held_call_id is not null
           and not reads_call(e.held_call_id, $2))";

/// Whether the file of an export holds a recording that the account may not read now
/// (ADR 0604): a recording of a call that still runs in a channel that hides who is
/// inside, while the account is not inside. The file is not sent until the call has ended
/// or the account is inside again.
pub async fn holds_hidden_recording(
    connection: &mut PgConnection,
    access: &Access,
    export_id: ExportId,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(HIDDEN_RECORDING)
        .bind(export_id)
        .bind(access.account_id)
        .fetch_one(connection)
        .await?)
}

fn call_place(place: MessagePlace) -> CallPlace {
    match place {
        MessagePlace::Channel {
            workspace_id,
            channel_id,
        } => CallPlace::Channel {
            workspace_id,
            channel_id,
        },
        MessagePlace::Direct {
            low_account_id,
            high_account_id,
            ..
        } => CallPlace::Direct {
            low_account_id,
            high_account_id,
        },
    }
}

/// Records the state of an export for its account, while the account sees the channel of
/// the export or may reach the other account of its conversation.
pub async fn announce(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    stored: &ExportRow,
) -> ApiResult<()> {
    let audience = Audience::addressed_at(stored.account_id, &[call_place(stored.export.place)]);
    let event = Event::ExportUpdated {
        export: stored.export.clone(),
    };
    events::log::record(connection, now, &audience, &event).await
}

/// The rows that a removal statement returns: each removed export with its account.
async fn removed(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    rows: Vec<(ExportId, AccountId)>,
    reason: ExportRemoval,
) -> ApiResult<usize> {
    for (export_id, account_id) in &rows {
        storage::release(&mut *connection, export_id.as_uuid()).await?;
        let event = Event::ExportRemoved {
            export_id: *export_id,
            reason,
        };
        events::tell_account(&mut *connection, now, *account_id, &event).await?;
    }
    Ok(rows.len())
}

/// Removes one export: no route shows it from now on, and its file is deleted afterwards.
pub async fn remove(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    id: ExportId,
    reason: ExportRemoval,
) -> ApiResult<bool> {
    let rows: Vec<(ExportId, AccountId)> = sqlx::query_as(
        "update exports set removed_at = $2 where id = $1 and removed_at is null
         returning id, account_id",
    )
    .bind(id)
    .bind(now)
    .fetch_all(&mut *connection)
    .await?;
    Ok(removed(connection, now, rows, reason).await? > 0)
}

/// The exports of the place `$1` that hold, or can hold, the item `$2`. `$3` is the time.
pub const OF_ITEM: &str = "update exports set removed_at = $3
     where place_id = $1 and id >= $2 and removed_at is null
     returning id, account_id";

/// Removes every export that holds, or can hold, a message that is deleted now or whose
/// files are deleted now. It runs in the transaction of the deletion (ADR 0406).
pub async fn message_deleted(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    message_id: junto_protocol::ids::MessageId,
) -> ApiResult<()> {
    let place: Option<Uuid> = sqlx::query_scalar("select place_id from messages where id = $1")
        .bind(message_id)
        .fetch_optional(&mut *connection)
        .await?;
    let Some(place) = place else {
        return Ok(());
    };
    let rows: Vec<(ExportId, AccountId)> = sqlx::query_as(OF_ITEM)
        .bind(place)
        .bind(message_id)
        .bind(now)
        .fetch_all(&mut *connection)
        .await?;
    removed(connection, now, rows, ExportRemoval::ContentDeleted).await?;
    Ok(())
}

/// Removes every export of a channel that holds, or can hold, a recording that is removed
/// now. It runs in the transaction of the removal.
pub async fn recording_removed(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    channel_id: ChannelId,
    recording_id: junto_protocol::ids::RecordingId,
) -> ApiResult<()> {
    let rows: Vec<(ExportId, AccountId)> = sqlx::query_as(OF_ITEM)
        .bind(channel_id)
        .bind(recording_id)
        .bind(now)
        .fetch_all(&mut *connection)
        .await?;
    removed(connection, now, rows, ExportRemoval::ContentDeleted).await?;
    Ok(())
}

/// Removes every export of an account whose access to the server has ended.
pub async fn account_ended(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
) -> ApiResult<()> {
    sqlx::query(
        "delete from storage_reservations where subject_id in (
             select id from exports where account_id = $1 and removed_at is null)",
    )
    .bind(account_id)
    .execute(&mut *connection)
    .await?;
    sqlx::query("update exports set removed_at = $2 where account_id = $1 and removed_at is null")
        .bind(account_id)
        .bind(now)
        .execute(connection)
        .await?;
    Ok(())
}

/// The exports whose lifetime has ended, at most `$2`. `$1` is the time.
pub const EXPIRED: &str = "update exports set removed_at = $1 where id in (
         select id from exports
         where expires_at <= $1 and removed_at is null
         order by expires_at, id limit $2)
     returning id, account_id";

/// Removes the exports that finished 7 days ago, in batches.
pub async fn expire(app: &AppState) -> ApiResult<()> {
    let batch = i64::from(app.config.limits.maintenance_batch.max(1));
    loop {
        let now = app.now();
        let count = transact!(app.background(), |connection| {
            let rows: Vec<(ExportId, AccountId)> = sqlx::query_as(EXPIRED)
                .bind(now)
                .bind(batch)
                .fetch_all(&mut *connection)
                .await?;
            removed(connection, now, rows, ExportRemoval::Expired).await
        })?;
        if i64::try_from(count).unwrap_or(i64::MAX) < batch {
            app.exports.wake();
            return Ok(());
        }
    }
}

/// The removed exports whose files are deleted next, at most `$2`. `$1` is the time.
pub const REMOVALS: &str = "select id from exports
     where removed_at is not null and (retry_at is null or retry_at <= $1)
     order by removed_at, id limit $2";

/// Deletes the directory of every removed export, and then its row. A directory that
/// cannot be deleted is tried again later. A removed export that a server process still
/// writes is left until that process has seen the removal or its lease has ended.
pub async fn sweep(app: &AppState) -> ApiResult<usize> {
    let Ok(root) = app.exports.directory().map(Path::to_path_buf) else {
        return Ok(0);
    };
    let limits = &app.config.limits.exports;
    let batch = i64::from(app.config.limits.maintenance_batch.max(1));
    let now = app.now();
    let waiting: Vec<ExportId> = crate::read!(app.background(), |connection| {
        Ok(sqlx::query_scalar(REMOVALS)
            .bind(now)
            .bind(batch)
            .fetch_all(connection)
            .await?)
    })?;
    let mut deleted = 0;
    for id in waiting {
        let written_now: bool = crate::read!(app.background(), |connection| {
            Ok(sqlx::query_scalar(
                "select exists (
                     select 1 from exports
                     where id = $1 and claimed_by is not null and claimed_by <> $2
                       and heartbeat_at > $3)",
            )
            .bind(id)
            .bind(app.exports.instance())
            .bind(now - limits.lease)
            .fetch_one(connection)
            .await?)
        })?;
        if written_now {
            continue;
        }
        let directory = root.clone();
        let gone = tokio::task::spawn_blocking(move || {
            files::remove(&directory, &files::Place::export(id))
        })
        .await
        .map_err(|error| ApiError::internal(&error))?;
        match gone {
            Ok(()) => {
                transact!(app.background(), |connection| {
                    storage::release(&mut *connection, id.as_uuid()).await?;
                    sqlx::query("delete from exports where id = $1 and removed_at is not null")
                        .bind(id)
                        .execute(connection)
                        .await?;
                    Ok(())
                })?;
                deleted += 1;
            }
            Err(error) => {
                tracing::warn!(export = %id, %error, "the file of an export cannot be deleted");
                let again = now + limits.removal_retry;
                transact!(app.background(), |connection| {
                    sqlx::query("update exports set retry_at = $2 where id = $1")
                        .bind(id)
                        .bind(again)
                        .execute(connection)
                        .await?;
                    Ok(())
                })?;
            }
        }
    }
    Ok(deleted)
}

/// Writes the files of the exports that wait, one after the other, and deletes the files of
/// the removed ones.
pub async fn run_once(app: &AppState) -> ApiResult<()> {
    sweep(app).await?;
    while build::run_next(app).await? {}
    sweep(app).await?;
    Ok(())
}

/// Runs [`run_once`] when an export is asked for or removed and at each interval, until the
/// task is dropped.
pub async fn run(app: AppState) {
    let interval = app
        .config
        .limits
        .exports
        .check_interval
        .try_into()
        .unwrap_or(Duration::from_secs(5));
    loop {
        if let Err(error) = run_once(&app).await {
            tracing::warn!(code = ?error.code(), "the work on exports failed");
        }
        tokio::select! {
            () = app.exports.woken() => {}
            () = tokio::time::sleep(interval) => {}
        }
    }
}
