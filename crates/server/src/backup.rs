//! What the server does for a backup and after a restore (ADR 0005, ADR 0018).
//!
//! The copy of the database and of the files is made by the programs of the deployment.
//! The server binary provides what only the server can: it holds back every deletion of a
//! file while a backup runs, it lists the files that the copy of the database needs and
//! the ones that the backup leaves out, and after a restore it brings the restored database
//! into a state that fits the restored files.

use std::path::PathBuf;

use junto_protocol::ids::{AttachmentId, RecordingId};
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::database::Database;
use crate::error::ApiResult;
use crate::owner::{self, HostCommandError};
use crate::{read, transact};

/// A backup that did not report its end holds deletions back for this long.
pub const LIFETIME: Duration = Duration::hours(24);

const PAGE: i64 = 1000;

/// True while a backup runs: the server then deletes no file of an attachment or of a
/// recording. Every task that deletes such files asks this first.
pub async fn held(connection: &mut PgConnection, now: OffsetDateTime) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar("select exists (select 1 from backups where expires_at > $1)")
            .bind(now)
            .fetch_one(connection)
            .await?,
    )
}

/// One line of the list of a backup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A directory that the backup needs, relative to the directory of its kind.
    Needed(String),
    /// A directory that the backup leaves out, because the server still writes into it, and
    /// why.
    LeftOut(String, &'static str),
}

impl std::fmt::Display for Line {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Needed(path) => write!(formatter, "file {path}"),
            Self::LeftOut(path, reason) => write!(formatter, "skip {path} {reason}"),
        }
    }
}

pub struct Begun {
    pub id: Uuid,
    pub expires_at: OffsetDateTime,
}

/// Starts a backup: from now on, and until [`end`] or the end of [`LIFETIME`], the server
/// deletes no file of an attachment or of a recording.
pub async fn begin(database: &Database, now: OffsetDateTime) -> Result<Begun, HostCommandError> {
    let id = Uuid::now_v7();
    let expires_at = now + LIFETIME;
    transact!(database, |connection| {
        owner::require_started(connection).await?;
        sqlx::query("delete from backups where expires_at <= $1")
            .bind(now)
            .execute(&mut *connection)
            .await?;
        sqlx::query("insert into backups (id, started_at, expires_at) values ($1, $2, $3)")
            .bind(id)
            .bind(now)
            .bind(expires_at)
            .execute(&mut *connection)
            .await?;
        Ok::<_, HostCommandError>(())
    })?;
    Ok(Begun { id, expires_at })
}

/// The directories of the files of the server, for the directories that the database
/// states as removed: such a directory is part of the list while it is on the disk.
#[derive(Debug, Default)]
pub struct Roots {
    pub attachments: Option<PathBuf>,
    pub recordings: Option<PathBuf>,
}

fn present(root: Option<&PathBuf>, path: &str) -> bool {
    root.is_some_and(|root| root.join(path).exists())
}

/// How [`list`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listed {
    Done,
    /// No backup with the identifier holds deletions back.
    NoBackup,
    /// The database has rows of this kind, and the command does not see their directory:
    /// the setting is absent, or it names no directory. The list would then leave out what
    /// was removed after the copy of the database.
    NoDirectory(&'static str),
}

/// Gives every directory of an attachment and of a recording to `line`: the ones that the
/// backup `id` needs, and the ones that it leaves out because an upload or a recording has
/// not ended.
///
/// The list is made after the copy of the database. Every file that the copy states as
/// complete was written before the copy and is not deleted while the backup runs, so it is
/// in the list: as a row that is still complete, or as a row or a queued removal whose
/// directory is still on the disk, because the content was removed after the copy. The
/// rows are read in pages, so the list is not held in memory.
pub async fn list(
    database: &Database,
    now: OffsetDateTime,
    id: Uuid,
    roots: &Roots,
    mut line: impl FnMut(Line),
) -> Result<Listed, HostCommandError> {
    let (runs, attachments, recordings): (bool, bool, bool) = read!(database, |connection| {
        Ok::<_, HostCommandError>(
            sqlx::query_as(
                "select exists (select 1 from backups where id = $1 and expires_at > $2),
                        exists (select 1 from attachments),
                        exists (select 1 from recordings)
                            or exists (select 1 from file_removals)",
            )
            .bind(id)
            .bind(now)
            .fetch_one(connection)
            .await?,
        )
    })?;
    if !runs {
        return Ok(Listed::NoBackup);
    }
    let seen = |root: &Option<PathBuf>| root.as_ref().is_some_and(|root| root.is_dir());
    if attachments && !seen(&roots.attachments) {
        return Ok(Listed::NoDirectory("JUNTO_ATTACHMENTS_DIR"));
    }
    if recordings && !seen(&roots.recordings) {
        return Ok(Listed::NoDirectory("JUNTO_RECORDINGS_DIR"));
    }
    let mut after = Uuid::nil();
    loop {
        let page: Vec<(AttachmentId, bool, bool, bool)> = read!(database, |connection| {
            Ok::<_, HostCommandError>(
                sqlx::query_as(
                    "select storage_id, bool_or(complete and removed_at is null),
                            bool_or(complete), bool_or(removed_at is null)
                     from attachments where storage_id > $1
                     group by storage_id order by storage_id limit $2",
                )
                .bind(after)
                .bind(PAGE)
                .fetch_all(connection)
                .await?,
            )
        })?;
        let Some((last, ..)) = page.last() else {
            break;
        };
        after = last.as_uuid();
        for (storage_id, live, complete, open) in page {
            let directory = storage_id.to_string();
            let path = format!("attachments/{directory}");
            if live || (complete && present(roots.attachments.as_ref(), &directory)) {
                line(Line::Needed(path));
            } else if open && !complete {
                line(Line::LeftOut(path, "upload_not_complete"));
            }
        }
    }
    let mut after = Uuid::nil();
    loop {
        let page: Vec<(RecordingId, String)> = read!(database, |connection| {
            Ok::<_, HostCommandError>(
                sqlx::query_as(
                    "select id, state from recordings where id > $1 order by id limit $2",
                )
                .bind(after)
                .bind(PAGE)
                .fetch_all(connection)
                .await?,
            )
        })?;
        let Some((last, _)) = page.last() else {
            break;
        };
        after = last.as_uuid();
        for (id, state) in page {
            let path = format!("recordings/{id}");
            line(match state.as_str() {
                "starting" | "recording" => Line::LeftOut(path, "recording_runs"),
                "processing" => Line::LeftOut(path, "files_are_processed"),
                _ => Line::Needed(path),
            });
        }
    }
    // What was removed while the backup runs waits in the queue of removals.
    let mut after = Uuid::nil();
    loop {
        let page: Vec<(Uuid, String)> = read!(database, |connection| {
            Ok::<_, HostCommandError>(
                sqlx::query_as(
                    "select id, path from file_removals where id > $1 order by id limit $2",
                )
                .bind(after)
                .bind(PAGE)
                .fetch_all(connection)
                .await?,
            )
        })?;
        let Some((last, _)) = page.last() else {
            break;
        };
        after = *last;
        for (_, path) in page {
            if present(roots.recordings.as_ref(), &path) {
                line(Line::Needed(format!("recordings/{path}")));
            }
        }
    }
    Ok(Listed::Done)
}

/// Ends a backup: the deletions that waited are done. False when no backup has the
/// identifier.
pub async fn end(database: &Database, id: Uuid) -> Result<bool, HostCommandError> {
    transact!(database, |connection| {
        let ended = sqlx::query("delete from backups where id = $1")
            .bind(id)
            .execute(connection)
            .await?;
        Ok(ended.rows_affected() > 0)
    })
}

/// What [`restored`] changed.
pub struct Restored {
    pub exports_removed: u64,
}

/// Brings a database that was restored from a backup into a state that fits the restored
/// files: the event stream gets a new epoch, because the clients hold cursors that the
/// restored database does not have (ADR 0010); the backup that was running when the copy
/// was made no longer holds deletions back; and every export is removed, because exports
/// are not part of a backup.
pub async fn restored(
    database: &Database,
    now: OffsetDateTime,
) -> Result<Restored, HostCommandError> {
    owner::renew_event_epoch(database, now).await?;
    transact!(database, |connection| {
        sqlx::query("delete from backups")
            .execute(&mut *connection)
            .await?;
        sqlx::query(
            "delete from storage_reservations where subject_id in (select id from exports)",
        )
        .execute(&mut *connection)
        .await?;
        let removed = sqlx::query("update exports set removed_at = $1 where removed_at is null")
            .bind(now)
            .execute(connection)
            .await?;
        Ok(Restored {
            exports_removed: removed.rows_affected(),
        })
    })
}
