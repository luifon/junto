//! What the server does for a backup and after a restore (ADR 0005, ADR 0018).
//!
//! The copy of the database and of the files is made by the programs of the deployment.
//! The server binary provides what only the server can: it holds back every deletion of a
//! file while a backup runs, it lists the files that the copy of the database needs and
//! the ones that the backup leaves out, and after a restore it brings the restored database
//! into a state that fits the restored files. It also lists and removes the directories
//! that no row names, which a restore and the way back from an upgrade leave.

use std::path::{Path, PathBuf};

use junto_protocol::ids::{AttachmentId, RecordingId};
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::database::Database;
use crate::error::ApiResult;
use crate::owner::{self, HostCommandError};
use crate::recordings::files;
use crate::{bootstrap, read, transact};

/// A backup that did not report its end holds deletions back for this long.
pub const LIFETIME: Duration = Duration::hours(24);

const PAGE: i64 = 1000;

/// True while a backup runs: the server then deletes no file of an attachment or of a
/// recording. Every task that deletes such files asks this first.
pub async fn held(connection: &mut PgConnection, now: OffsetDateTime) -> ApiResult<bool> {
    Ok(holds(connection, now).await?)
}

async fn holds(connection: &mut PgConnection, now: OffsetDateTime) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("select exists (select 1 from backups where expires_at > $1)")
        .bind(now)
        .fetch_one(connection)
        .await
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

/// The directories of the files of the server. [`list`] reads the first two for the
/// directories that the database states as removed: such a directory is part of the list
/// while it is on the disk.
#[derive(Debug, Default)]
pub struct Roots {
    pub attachments: Option<PathBuf>,
    pub recordings: Option<PathBuf>,
    pub exports: Option<PathBuf>,
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
                // A failed recording has no file (ADR 0606).
                "failed" => continue,
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

/// How [`end_running`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    Done,
    /// The backup expired at this time and is not ended: the server may have deleted files
    /// before the backup copied them.
    Expired(OffsetDateTime),
    /// No backup has the identifier.
    Unknown,
}

/// Ends a backup that runs: the deletions that waited are done. A backup that has expired
/// stays stored, so that every call for it gives the same answer.
pub async fn end_running(
    database: &Database,
    now: OffsetDateTime,
    id: Uuid,
) -> Result<Ended, HostCommandError> {
    transact!(database, |connection| {
        let expires_at: Option<OffsetDateTime> =
            sqlx::query_scalar("select expires_at from backups where id = $1 for update")
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?;
        match expires_at {
            None => Ok(Ended::Unknown),
            Some(expired_at) if expired_at <= now => Ok(Ended::Expired(expired_at)),
            Some(_) => {
                sqlx::query("delete from backups where id = $1")
                    .bind(id)
                    .execute(connection)
                    .await?;
                Ok(Ended::Done)
            }
        }
    })
}

/// Ends a backup, also one that has expired. False when no backup has the identifier.
pub async fn end(database: &Database, id: Uuid) -> Result<bool, HostCommandError> {
    transact!(database, |connection| {
        let ended = sqlx::query("delete from backups where id = $1")
            .bind(id)
            .execute(connection)
            .await?;
        Ok(ended.rows_affected() > 0)
    })
}

/// The schema of a database that is not the schema of this version of the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OtherSchema {
    pub found: i64,
    pub needed: i64,
}

impl std::fmt::Display for OtherSchema {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "the database has the schema {}, and this version of the server needs the schema {}",
            self.found, self.needed
        )
    }
}

/// The schema of the database when it differs from the schema of this version of the
/// server. A command whose statements are written for that schema asks this first.
async fn other_schema(database: &Database) -> Result<Option<OtherSchema>, HostCommandError> {
    let needed = bootstrap::schema_version();
    let found = read!(database, |connection| {
        Ok::<_, HostCommandError>(bootstrap::schema_of(connection).await?)
    })?
    .ok_or(HostCommandError::NotStarted)?;
    Ok((found != needed).then_some(OtherSchema { found, needed }))
}

/// How [`restored`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Restored {
    Done {
        exports_removed: u64,
    },
    /// Nothing was changed.
    OtherSchema(OtherSchema),
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
    if let Some(other) = other_schema(database).await? {
        return Ok(Restored::OtherSchema(other));
    }
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
        Ok(Restored::Done {
            exports_removed: removed.rows_affected(),
        })
    })
}

/// A directory that no row names stays while its last change is younger than this. The
/// rows alone tell a directory that the server writes; the age does not depend on the
/// order in which the server writes a row and a directory.
pub const UNNAMED_AGE: Duration = Duration::hours(24);

/// One kind of directory that the server names by the identifier of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Attachments,
    Recordings,
    Exports,
}

impl Kind {
    fn setting(self) -> &'static str {
        match self {
            Self::Attachments => "JUNTO_ATTACHMENTS_DIR",
            Self::Recordings => "JUNTO_RECORDINGS_DIR",
            Self::Exports => "JUNTO_EXPORTS_DIR",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Attachments => "attachments",
            Self::Recordings => "recordings",
            Self::Exports => "exports",
        }
    }

    /// The statement that selects, of the identifiers `$1`, the ones that no row of this
    /// kind names. A removed recording has no row; its directory waits in the queue of
    /// removals.
    fn unnamed(self) -> &'static str {
        match self {
            Self::Attachments => {
                "select candidate from unnest($1::uuid[]) as candidate
                 where not exists (select 1 from attachments where storage_id = candidate)"
            }
            Self::Recordings => {
                "select candidate from unnest($1::uuid[]) as candidate
                 where not exists (select 1 from recordings where id = candidate)
                   and not exists (
                       select 1 from file_removals
                       where split_part(path, '/', 1) = candidate::text)"
            }
            Self::Exports => {
                "select candidate from unnest($1::uuid[]) as candidate
                 where not exists (select 1 from exports where id = candidate)"
            }
        }
    }
}

/// One line of [`unnamed`] about a directory that no row names, as `<kind>/<identifier>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnnamedLine {
    /// The directory is old enough. Nothing was asked to remove it.
    Found(String),
    Removed(String),
    /// The directory stays, and why.
    Kept(String, &'static str),
}

impl std::fmt::Display for UnnamedLine {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Found(path) => write!(formatter, "unnamed {path}"),
            Self::Removed(path) => write!(formatter, "removed {path}"),
            Self::Kept(path, reason) => write!(formatter, "skip {path} {reason}"),
        }
    }
}

/// Why [`unnamed`] did not run to its end.
#[derive(Debug, thiserror::Error)]
pub enum UnnamedError {
    #[error(transparent)]
    Command(#[from] HostCommandError),
    #[error("{0}. Nothing was changed")]
    OtherSchema(OtherSchema),
    #[error(
        "none of JUNTO_ATTACHMENTS_DIR, JUNTO_RECORDINGS_DIR and JUNTO_EXPORTS_DIR is set; \
         run the command with the settings and the directories of the server"
    )]
    NoDirectory,
    #[error(
        "{0} and {1} name the same directory, so the command cannot tell which rows name \
         its directories. Nothing was changed"
    )]
    SameDirectory(&'static str, &'static str),
    #[error(
        "a backup holds deletions back; no further directory was removed. Run the command \
         again after the end of the backup"
    )]
    Held,
    #[error("the directory {0} could not be read: {1}")]
    Unreadable(String, std::io::Error),
    #[error("{0} could not be removed: {1}")]
    NotRemoved(String, std::io::Error),
}

/// The identifier that `name` is, when it has the form in which the server writes one.
fn identifier(name: &str) -> Option<Uuid> {
    let id = Uuid::parse_str(name).ok()?;
    (id.hyphenated().to_string() == name).then_some(id)
}

/// The identifiers that are the names of entries of `root`. An entry of another name is
/// not listed.
fn identifiers(root: &Path) -> std::io::Result<Vec<Uuid>> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(root)? {
        if let Some(id) = entry?.file_name().to_str().and_then(identifier) {
            found.push(id);
        }
    }
    found.sort_unstable();
    Ok(found)
}

/// The time of the last change of the directory `id` in `root`. `None` for a file and for
/// a link, and when nothing is there.
fn changed_at(root: &Path, id: Uuid) -> std::io::Result<Option<OffsetDateTime>> {
    match std::fs::symlink_metadata(root.join(id.to_string())) {
        Ok(found) if found.is_dir() => Ok(Some(found.modified()?.into())),
        Ok(_) => Ok(None),
        Err(error) if files::is_absent(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// Whether a backup holds deletions back, and the identifiers of `page` that no row of
/// `kind` names, read in one transaction.
async fn unnamed_of(
    database: &Database,
    now: OffsetDateTime,
    kind: Kind,
    page: &[Uuid],
) -> Result<(bool, Vec<Uuid>), HostCommandError> {
    read!(database, |connection| {
        let held = holds(&mut *connection, now).await?;
        let unnamed = sqlx::query_scalar(kind.unnamed())
            .bind(page)
            .fetch_all(connection)
            .await?;
        Ok((held, unnamed))
    })
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> std::io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(std::io::Error::other)?
}

/// Gives to `line` every directory of an attachment, of a recording and of an export that
/// no row names, and with `remove` deletes the ones that were last changed more than
/// [`UNNAMED_AGE`] ago. A restore and the way back from an upgrade leave such directories.
///
/// The server writes the row of an upload, of a recording and of an export before it
/// creates the directory, and deletes a directory before the last row that names it. The
/// names of a directory are read first and the rows afterwards, so a directory that the
/// server writes is named by a row that this reads. Each directory is read whole, and the
/// rows are read in pages. Nothing is removed while a backup holds deletions back.
pub async fn unnamed(
    database: &Database,
    now: OffsetDateTime,
    roots: &Roots,
    remove: bool,
    mut line: impl FnMut(UnnamedLine),
) -> Result<(), UnnamedError> {
    let mut places: Vec<(Kind, PathBuf, PathBuf)> = Vec::new();
    for (kind, root) in [
        (Kind::Attachments, &roots.attachments),
        (Kind::Recordings, &roots.recordings),
        (Kind::Exports, &roots.exports),
    ] {
        let Some(root) = root else {
            continue;
        };
        let real = std::fs::canonicalize(root)
            .map_err(|error| UnnamedError::Unreadable(root.display().to_string(), error))?;
        if let Some((other, ..)) = places.iter().find(|(_, _, taken)| *taken == real) {
            return Err(UnnamedError::SameDirectory(other.setting(), kind.setting()));
        }
        places.push((kind, root.clone(), real));
    }
    if places.is_empty() {
        return Err(UnnamedError::NoDirectory);
    }
    if let Some(other) = other_schema(database).await? {
        return Err(UnnamedError::OtherSchema(other));
    }
    let old_before = now - UNNAMED_AGE;
    let mut not_removed = None;
    for (kind, root, _) in places {
        let unreadable = |error| UnnamedError::Unreadable(root.display().to_string(), error);
        let listed = root.clone();
        let found = blocking(move || identifiers(&listed))
            .await
            .map_err(unreadable)?;
        for page in found.chunks(PAGE as usize) {
            let (held, unnamed) = unnamed_of(database, now, kind, page).await?;
            if remove && held {
                return Err(UnnamedError::Held);
            }
            for id in unnamed {
                let path = format!("{}/{id}", kind.label());
                let Some(changed) = changed_at(&root, id).map_err(unreadable)? else {
                    continue;
                };
                if changed > old_before {
                    line(UnnamedLine::Kept(path, "changed_recently"));
                } else if !remove {
                    line(UnnamedLine::Found(path));
                } else {
                    let Some(place) = files::Place::from_stored(&id.to_string()) else {
                        continue;
                    };
                    let within = root.clone();
                    match blocking(move || files::remove(&within, &place)).await {
                        Ok(()) => line(UnnamedLine::Removed(path)),
                        Err(error) => {
                            not_removed.get_or_insert(UnnamedError::NotRemoved(path, error));
                        }
                    }
                }
            }
        }
    }
    not_removed.map_or(Ok(()), Err)
}
