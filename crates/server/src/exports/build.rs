//! The build of the file of one export (ADR 0411).
//!
//! A build reads its place in pages. Each page is one transaction, which decides again
//! whether the account may export the place and reads only what the account reads. The
//! build runs twice over the place: the first run counts the bytes of the file, so that the
//! storage guard reserves them, and the second run writes. Both runs use the same code
//! with another sink. Nothing is held in memory beyond one page and one part of a file.

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use junto_protocol::error::ErrorCode;
use junto_protocol::export::{
    EXPORT_FORMAT, ExportFailure, ExportManifest, ExportedRecording, ExportedSegmentFile,
};
use junto_protocol::ids::{
    AccountId, AttachmentId, ExportId, MessageId, RecordingId, WorkspaceRoleId,
};
use junto_protocol::message::MessagePlace;
use junto_protocol::recording::RecordingDetail;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use super::zip::{self, ZipWriter};
use super::{ExportRow, html};
use crate::access::Access;
use crate::error::{ApiError, ApiResult};
use crate::messages::{self, Place, uploads};
use crate::recordings::files;
use crate::recordings::store as recordings;
use crate::state::AppState;
use crate::storage;
use crate::transact;

/// The attachments and the recordings that one page reads.
const ATTACHMENT_PAGE: i64 = 50;
const RECORDING_PAGE: i64 = 1;
/// The bytes that a build reads of a file at one time.
const CHUNK: usize = 1024 * 1024;
/// Added to the counted bytes, for the differences between the two runs.
pub const MARGIN: u64 = 1024 * 1024;

/// The messages of the place `$1` after the position `($3, $4)` that the export `$5` of the
/// account `$2` holds: the ones that are not newer than the export, that are not deleted
/// and that the account reads. At most `$6`.
pub const MESSAGE_PAGE_QUERY: &str = "select m.id, m.position from messages m
     where m.place_id = $1 and (m.position, m.id) > ($3, $4) and m.id <= $5 and not m.deleted
       and (m.readers is null or $2 = any(m.readers))
     order by m.position, m.id limit $6";

/// The files of the messages of the place `$1` after the attachment `$2` that the export
/// `$3` holds. At most `$4`.
pub const ATTACHMENT_PAGE_QUERY: &str = "select a.id, a.storage_id, a.name, a.bytes
     from attachments a
     join messages m on m.id = a.message_id
     where a.place_id = $1 and a.id > $2 and a.removed_at is null and a.complete and a.attached
       and m.id <= $3 and not m.deleted and m.readers is null
     order by a.id limit $4";

/// The processed recordings of the channel `$1` after the recording `$2` that are not
/// newer than the export `$3`. At most `$4`.
pub const RECORDING_PAGE_QUERY: &str = "select id from recordings
     where channel_id = $1 and id > $2 and id <= $3 and state in ('ready', 'partly_failed')
     order by id limit $4";

/// The export that a build takes next: one that waits, or one whose process has not
/// renewed its lease since `$1`.
pub const NEXT_QUERY: &str = "select id, attempts from exports
     where removed_at is null
       and (state = 'waiting' or (state = 'running' and heartbeat_at <= $1))
     order by id limit 1 for update";

/// Whether a process writes an export now: its lease was renewed after `$1`.
pub const BUSY_QUERY: &str = "select exists (
         select 1 from exports
         where state = 'running' and removed_at is null and heartbeat_at > $1)";

/// What a step of a build left to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Progress {
    More,
    Finished,
}

/// Why a build ends without a file.
enum Stop {
    Failed(ExportFailure),
    /// The export was removed, or another process took it. This process states nothing.
    Gone,
    Error(ApiError),
}

impl From<ApiError> for Stop {
    fn from(error: ApiError) -> Self {
        if error.code() == ErrorCode::StorageLow {
            Self::Failed(ExportFailure::StorageLow)
        } else {
            Self::Error(error)
        }
    }
}

impl From<sqlx::Error> for Stop {
    fn from(error: sqlx::Error) -> Self {
        Self::Error(error.into())
    }
}

impl From<io::Error> for Stop {
    fn from(error: io::Error) -> Self {
        tracing::warn!(%error, "a file of an export cannot be read or written");
        Self::Failed(ExportFailure::Failed)
    }
}

type Stopped<T> = Result<T, Stop>;

/// Why a page ends a build although the server works: the outcome of a transaction that
/// commits. An error of the server is an `ApiError`, so that the transaction runs again
/// when it lost against another one (ADR 0009).
enum Halt {
    Failed(ExportFailure),
    Gone,
}

impl From<Halt> for Stop {
    fn from(halt: Halt) -> Self {
        match halt {
            Halt::Failed(failure) => Self::Failed(failure),
            Halt::Gone => Self::Gone,
        }
    }
}

type Paged<T> = ApiResult<Result<T, Halt>>;

/// The value of a page, or the end of the function with the halt of the page.
macro_rules! unless_halted {
    ($page:expr) => {
        match $page {
            Ok(value) => value,
            Err(halt) => return Ok(Err(halt)),
        }
    };
}

struct AttachmentPlan {
    entry: String,
    storage_id: AttachmentId,
    bytes: u64,
    /// The offset, the length and the checksum of each chunk of the file.
    chunks: Vec<(i64, i32, String)>,
}

struct RecordingPlan {
    directory: String,
    description: Vec<u8>,
    transcript: Option<String>,
    /// The name of the entry, the place of the file and its size.
    files: Vec<(String, files::Place, u64)>,
}

/// What one step adds to the file.
enum Plan {
    /// The start of the entry of the messages.
    Open,
    Messages {
        json: Vec<u8>,
        html: Vec<u8>,
    },
    /// The end of the entry of the messages, and the readable page.
    Page {
        head: String,
    },
    Attachments(Vec<AttachmentPlan>),
    Recordings(Vec<RecordingPlan>),
    Manifest(Vec<u8>),
}

#[derive(Debug, Clone, Copy)]
enum Stage {
    Open,
    Messages { after: (i64, Uuid) },
    Page,
    Attachments { after: Uuid },
    Recordings { after: Uuid },
    Manifest,
    End,
}

/// The files of one attempt of a build, in the directory of its export. Their names state
/// the attempt, so that two processes never write the same file.
struct Writing {
    zip: Option<ZipWriter>,
    /// The readable page, until the entry of the messages has ended.
    page: File,
    attachments: Option<PathBuf>,
    recordings: Option<PathBuf>,
    /// The bytes that are reserved for the files of the build.
    reserved: u64,
    /// The bytes of every plan that was given to the files, with the margin: the most
    /// that the files hold at the end.
    planned: u64,
    /// The bytes that the files hold now. A plan can state bytes that are written later:
    /// the readable page and the directory are copied into the archive at their end.
    written: u64,
}

enum Sink {
    Count(u64),
    Write(Box<Writing>),
}

#[derive(Default, Clone, Copy)]
struct Counts {
    messages: u64,
    attachments: u64,
    recordings: u64,
}

struct Run {
    sink: Option<Sink>,
    stage: Stage,
    counts: Counts,
}

impl Run {
    fn new(sink: Sink) -> Self {
        Self {
            sink: Some(sink),
            stage: Stage::Open,
            counts: Counts::default(),
        }
    }
}

enum Phase {
    Count(Run),
    Reserve(u64),
    Write(Run),
    Seal,
    Done,
}

fn entry_bytes(name: &str, content: u64) -> u64 {
    // The record of the directory is written twice: into its own file, then into the
    // archive.
    content + zip::entry_overhead(name.len()) + zip::directory_record(name.len())
}

impl Plan {
    /// The bytes that the plan adds to the files of a build.
    fn bytes(&self) -> u64 {
        match self {
            Self::Open => entry_bytes("messages.json", 1),
            Self::Messages { json, html } => json.len() as u64 + 2 * html.len() as u64,
            Self::Page { head } => {
                2 + entry_bytes("messages.html", (head.len() + html::TAIL.len()) as u64)
            }
            Self::Attachments(attachments) => attachments
                .iter()
                .map(|attachment| entry_bytes(&attachment.entry, attachment.bytes))
                .sum(),
            Self::Recordings(recordings) => recordings
                .iter()
                .map(|recording| {
                    let description = format!("{}/recording.json", recording.directory);
                    let transcript = format!("{}/transcript.txt", recording.directory);
                    entry_bytes(&description, recording.description.len() as u64)
                        + recording
                            .transcript
                            .as_ref()
                            .map_or(0, |text| entry_bytes(&transcript, text.len() as u64))
                        + recording
                            .files
                            .iter()
                            .map(|(entry, _, bytes)| entry_bytes(entry, *bytes))
                            .sum::<u64>()
                })
                .sum(),
            Self::Manifest(manifest) => entry_bytes("export.json", manifest.len() as u64),
        }
    }
}

fn stopped() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "the build was stopped")
}

/// Copies a file of a recording into the open entry. A file that holds more than the
/// `bytes` that its segment states is not copied: the space for it is not reserved.
fn copy_file(zip: &mut ZipWriter, file: File, bytes: u64, stop: &AtomicBool) -> io::Result<()> {
    let mut buffer = vec![0u8; CHUNK];
    let mut file = file.take(bytes.saturating_add(1));
    let mut copied = 0u64;
    loop {
        if stop.load(Ordering::SeqCst) {
            return Err(stopped());
        }
        let count = file.read(&mut buffer)?;
        if count == 0 {
            return Ok(());
        }
        copied += count as u64;
        if copied > bytes {
            return Err(io::Error::other(
                "a file of a recording is larger than its segment states",
            ));
        }
        zip.write(&buffer[..count])?;
    }
}

impl Writing {
    /// The bytes of the archive, of its directory and of the readable page on the disk.
    fn on_disk(&self) -> io::Result<u64> {
        let archive = self.zip.as_ref().map_or(0, ZipWriter::written);
        Ok(archive + self.page.metadata()?.len())
    }

    fn zip(&mut self) -> io::Result<&mut ZipWriter> {
        self.zip
            .as_mut()
            .ok_or_else(|| io::Error::other("the archive has ended"))
    }

    /// Writes what a plan holds. It reads the files of attachments and recordings, so it
    /// runs on a thread for blocking work.
    fn apply(&mut self, plan: Plan, stop: &AtomicBool) -> io::Result<()> {
        match plan {
            Plan::Open => {
                let zip = self.zip()?;
                zip.start("messages.json")?;
                zip.write(b"[")
            }
            Plan::Messages { json, html } => {
                self.zip()?.write(&json)?;
                self.page.write_all(&html)
            }
            Plan::Page { head } => {
                let zip = self.zip()?;
                zip.write(b"]\n")?;
                zip.end()?;
                zip.start("messages.html")?;
                zip.write(head.as_bytes())?;
                self.page.seek(SeekFrom::Start(0))?;
                let mut buffer = vec![0u8; CHUNK];
                loop {
                    let count = self.page.read(&mut buffer)?;
                    if count == 0 {
                        break;
                    }
                    self.zip()?.write(&buffer[..count])?;
                }
                let zip = self.zip()?;
                zip.write(html::TAIL.as_bytes())?;
                zip.end()
            }
            Plan::Attachments(attachments) => {
                let root = self
                    .attachments
                    .clone()
                    .ok_or_else(|| io::Error::other("no directory of attachments"))?;
                for attachment in attachments {
                    self.zip()?.start(&attachment.entry)?;
                    for (offset, length, expected) in attachment.chunks {
                        if stop.load(Ordering::SeqCst) {
                            return Err(stopped());
                        }
                        let offset = u64::try_from(offset).map_err(io::Error::other)?;
                        let chunk = uploads::chunk(attachment.storage_id, offset);
                        let mut bytes = Vec::new();
                        files::open(&root, &chunk)?
                            .take(uploads::CHUNK as u64 + 1)
                            .read_to_end(&mut bytes)?;
                        if i64::try_from(bytes.len()).ok() != Some(i64::from(length))
                            || uploads::hash(&bytes) != expected
                        {
                            return Err(io::Error::other("a chunk of an attachment is damaged"));
                        }
                        self.zip()?.write(&bytes)?;
                    }
                    self.zip()?.end()?;
                }
                Ok(())
            }
            Plan::Recordings(recordings) => {
                for recording in recordings {
                    let description = format!("{}/recording.json", recording.directory);
                    self.zip()?.entry(&description, &recording.description)?;
                    if let Some(text) = &recording.transcript {
                        let transcript = format!("{}/transcript.txt", recording.directory);
                        self.zip()?.entry(&transcript, text.as_bytes())?;
                    }
                    let root = self
                        .recordings
                        .clone()
                        .ok_or_else(|| io::Error::other("no directory of recordings"))?;
                    for (entry, place, bytes) in recording.files {
                        let file = files::open(&root, &place)?;
                        self.zip()?.start(&entry)?;
                        copy_file(self.zip()?, file, bytes, stop)?;
                        self.zip()?.end()?;
                    }
                }
                Ok(())
            }
            Plan::Manifest(manifest) => self.zip()?.entry("export.json", &manifest),
        }
    }
}

/// One build: the export that a server process has taken, and how far its file is.
pub struct Job {
    id: ExportId,
    account_id: AccountId,
    place: MessagePlace,
    requested_at: OffsetDateTime,
    attempt: i32,
    phase: Phase,
    /// The bytes of the finished file.
    bytes: u64,
    counts: Counts,
}

fn file_name(attempt: i32, kind: &str) -> String {
    format!("{attempt}.{kind}")
}

/// The name of the file of an export that its build `attempt` finished.
pub fn finished_name(attempt: i32) -> String {
    file_name(attempt, "zip")
}

impl Job {
    /// Takes the next export for this server process. `None` when no export waits, and
    /// while another process writes one: the server writes one export at a time. An export
    /// that was taken three times without an end fails.
    pub async fn claim(app: &AppState) -> ApiResult<Option<Self>> {
        let limits = &app.config.limits.exports;
        let now = app.now();
        let instance = app.exports.instance();
        let stale = now - limits.lease;
        let most = i32::try_from(limits.attempts).unwrap_or(i32::MAX);
        let taken = transact!(app.background(), |connection| {
            let busy: bool = sqlx::query_scalar(BUSY_QUERY)
                .bind(stale)
                .fetch_one(&mut *connection)
                .await?;
            if busy {
                return Ok(None);
            }
            let next: Option<(ExportId, i32)> = sqlx::query_as(NEXT_QUERY)
                .bind(stale)
                .fetch_optional(&mut *connection)
                .await?;
            let Some((id, attempts)) = next else {
                return Ok(None);
            };
            if attempts >= most {
                let failure = ExportFailure::Interrupted;
                finish_failed(&mut *connection, app, now, id, None, failure).await?;
                return Ok(Some((id, None)));
            }
            sqlx::query(
                "update exports
                 set state = 'running', attempts = attempts + 1, claimed_by = $2,
                     heartbeat_at = $3
                 where id = $1",
            )
            .bind(id)
            .bind(instance)
            .bind(now)
            .execute(&mut *connection)
            .await?;
            let stored = super::export(&mut *connection, id, false)
                .await?
                .ok_or_else(ApiError::not_found)?;
            super::announce(&mut *connection, now, &stored).await?;
            Ok(Some((id, Some((stored, attempts + 1)))))
        })?;
        match taken {
            None => Ok(None),
            // The export failed in the transaction above; the next one is taken.
            Some((_, None)) => Box::pin(Self::claim(app)).await,
            Some((id, Some((stored, attempt)))) => Ok(Some(Self {
                id,
                account_id: stored.account_id,
                place: stored.export.place,
                requested_at: stored.export.requested_at,
                attempt,
                phase: Phase::Count(Run::new(Sink::Count(zip::END_BYTES + MARGIN))),
                bytes: 0,
                counts: Counts::default(),
            })),
        }
    }

    pub fn export_id(&self) -> ExportId {
        self.id
    }

    fn directory(&self) -> files::Place {
        files::Place::export(self.id)
    }

    fn part(&self, kind: &str) -> files::Place {
        self.directory().with(&file_name(self.attempt, kind))
    }

    /// Does the next part of the build: one page of the count or of the file, the
    /// reservation of the space, or the end. A failure of the export is stored, and the
    /// step then answers `Finished`. An error is a failure of the server: the export waits
    /// for another attempt.
    pub async fn step(&mut self, app: &AppState) -> ApiResult<Progress> {
        match self.advance(app).await {
            Ok(progress) => Ok(progress),
            Err(Stop::Gone) => {
                self.discard(app).await;
                Ok(Progress::Finished)
            }
            Err(Stop::Failed(failure)) => {
                self.discard(app).await;
                let now = app.now();
                let instance = Some(app.exports.instance());
                let id = self.id;
                transact!(app.background(), |connection| {
                    finish_failed(connection, app, now, id, instance, failure).await
                })?;
                Ok(Progress::Finished)
            }
            Err(Stop::Error(error)) => {
                self.discard(app).await;
                Err(error)
            }
        }
    }

    /// Deletes the files of this attempt.
    async fn discard(&mut self, app: &AppState) {
        self.phase = Phase::Done;
        let Ok(root) = app.exports.directory().map(Path::to_path_buf) else {
            return;
        };
        let places = [
            self.part("zip.partial"),
            self.part("directory.partial"),
            self.part("html.partial"),
            self.part("zip"),
        ];
        let removed = tokio::task::spawn_blocking(move || {
            for place in places {
                files::remove(&root, &place)?;
            }
            Ok::<_, io::Error>(())
        })
        .await;
        if !matches!(removed, Ok(Ok(()))) {
            tracing::warn!(export = %self.id, "the files of a build cannot be deleted");
        }
    }

    async fn advance(&mut self, app: &AppState) -> Stopped<Progress> {
        match std::mem::replace(&mut self.phase, Phase::Done) {
            Phase::Count(mut run) => {
                let ended = self.run(app, &mut run).await?;
                self.phase = match (ended, run.sink) {
                    (true, Some(Sink::Count(bytes))) => Phase::Reserve(bytes),
                    (_, sink) => {
                        run.sink = sink;
                        Phase::Count(run)
                    }
                };
                Ok(Progress::More)
            }
            Phase::Reserve(bytes) => {
                let writing = self.reserve(app, bytes).await?;
                self.phase = Phase::Write(Run::new(Sink::Write(Box::new(writing))));
                Ok(Progress::More)
            }
            Phase::Write(mut run) => {
                let ended = self.run(app, &mut run).await?;
                if ended {
                    self.counts = run.counts;
                    let Some(Sink::Write(mut writing)) = run.sink.take() else {
                        return Err(Stop::Error(ApiError::internal(&"a build without a file")));
                    };
                    let zip = writing.zip.take();
                    let finished = tokio::task::spawn_blocking(move || match zip {
                        Some(zip) => zip.finish(),
                        None => Err(io::Error::other("the archive has ended")),
                    })
                    .await
                    .map_err(|error| Stop::Error(ApiError::internal(&error)))?;
                    self.bytes = finished?;
                    self.phase = Phase::Seal;
                } else {
                    self.phase = Phase::Write(run);
                }
                Ok(Progress::More)
            }
            Phase::Seal => {
                self.seal(app).await?;
                Ok(Progress::Finished)
            }
            Phase::Done => Ok(Progress::Finished),
        }
    }

    /// The transaction of every page starts with this: the export still belongs to this
    /// process, whose lease is renewed, and its account may still export its place.
    async fn permitted(
        &self,
        connection: &mut PgConnection,
        app: &AppState,
        now: OffsetDateTime,
    ) -> Paged<(Access, Place)> {
        let renewed = sqlx::query(
            "update exports set heartbeat_at = $3
             where id = $1 and claimed_by = $2 and state = 'running' and removed_at is null",
        )
        .bind(self.id)
        .bind(app.exports.instance())
        .bind(now)
        .execute(&mut *connection)
        .await?;
        if renewed.rows_affected() == 0 {
            return Ok(Err(Halt::Gone));
        }
        let Some(access) = Access::of_account(&mut *connection, self.account_id).await? else {
            return Ok(Err(Halt::Failed(ExportFailure::AccessEnded)));
        };
        Ok(
            match super::permitted(connection, &access, self.place).await? {
                Some(place) => Ok((access, place)),
                None => Err(Halt::Failed(ExportFailure::AccessEnded)),
            },
        )
    }

    /// Renews the lease while a file is copied. False when the export is gone.
    async fn renew(&self, app: &AppState) -> ApiResult<bool> {
        let now = app.now();
        let (id, instance) = (self.id, app.exports.instance());
        transact!(app.background(), |connection| {
            let renewed = sqlx::query(
                "update exports set heartbeat_at = $3
                 where id = $1 and claimed_by = $2 and state = 'running'
                   and removed_at is null",
            )
            .bind(id)
            .bind(instance)
            .bind(now)
            .execute(connection)
            .await?;
            Ok(renewed.rows_affected() > 0)
        })
    }

    /// Reads the next page and adds it to the sink of the run. True when the run has ended.
    async fn run(&self, app: &AppState, run: &mut Run) -> Stopped<bool> {
        let writes = matches!(run.sink, Some(Sink::Write(_)));
        let (plan, next, counted) = self.read(app, run.stage, run.counts, writes).await?;
        run.counts = counted;
        match run.sink.take() {
            Some(Sink::Count(bytes)) => {
                run.sink = Some(Sink::Count(bytes + plan.as_ref().map_or(0, Plan::bytes)));
            }
            Some(Sink::Write(mut writing)) => {
                if let Some(plan) = plan {
                    // The content can have grown since it was counted. The files get no
                    // byte that is not reserved.
                    let planned = writing.planned + plan.bytes();
                    if planned > writing.reserved {
                        let pending = planned.saturating_sub(writing.written);
                        self.reserve_more(app, planned, pending).await?;
                        writing.reserved = planned;
                    }
                    writing.planned = planned;
                    let stop = Arc::new(AtomicBool::new(false));
                    let stopper = stop.clone();
                    let mut work = tokio::task::spawn_blocking(move || {
                        let applied = writing.apply(plan, &stopper);
                        let written = applied.and_then(|()| writing.on_disk());
                        (writing, written)
                    });
                    let heartbeat = app
                        .config
                        .limits
                        .exports
                        .heartbeat
                        .try_into()
                        .unwrap_or(std::time::Duration::from_secs(15));
                    let (returned, written) = loop {
                        tokio::select! {
                            joined = &mut work => {
                                break joined
                                    .map_err(|error| Stop::Error(ApiError::internal(&error)))?;
                            }
                            () = tokio::time::sleep(heartbeat) => {
                                if !self.renew(app).await? {
                                    stop.store(true, Ordering::SeqCst);
                                }
                            }
                        }
                    };
                    if stop.load(Ordering::SeqCst) {
                        return Err(Stop::Gone);
                    }
                    writing = returned;
                    writing.written = written?;
                }
                run.sink = Some(Sink::Write(writing));
            }
            None => return Err(Stop::Error(ApiError::internal(&"a run without a sink"))),
        }
        run.stage = next;
        Ok(matches!(next, Stage::End))
    }

    /// One page: what it adds to the file, the stage that follows, and the counts.
    async fn read(
        &self,
        app: &AppState,
        stage: Stage,
        mut counts: Counts,
        writes: bool,
    ) -> Stopped<(Option<Plan>, Stage, Counts)> {
        let start = (-1, Uuid::nil());
        let is_channel = matches!(self.place, MessagePlace::Channel { .. });
        let after_messages = Stage::Attachments { after: Uuid::nil() };
        let now = app.now();
        let page = i64::from(app.config.limits.exports.page.max(1));
        let read = match stage {
            Stage::Open => (Some(Plan::Open), Stage::Messages { after: start }),
            Stage::Messages { after } => {
                let first = after == start;
                let read = transact!(app.background(), |connection| {
                    self.messages(connection, app, now, after, page, first)
                        .await
                })?;
                let (plan, last, read_messages) = read?;
                counts.messages += read_messages;
                let next = match last {
                    Some(after) => Stage::Messages { after },
                    None => Stage::Page,
                };
                (Some(plan), next)
            }
            Stage::Page => {
                let read = transact!(app.background(), |connection| {
                    self.title(connection, app, now).await
                })?;
                let head = html::head(&read?, self.requested_at);
                (Some(Plan::Page { head }), after_messages)
            }
            Stage::Attachments { after } => {
                let read = transact!(app.background(), |connection| {
                    self.attachments(connection, app, now, after).await
                })?;
                let (plans, last) = read?;
                counts.attachments += plans.len() as u64;
                let next = match (last, is_channel) {
                    (Some(after), _) => Stage::Attachments { after },
                    (None, true) => Stage::Recordings { after: Uuid::nil() },
                    (None, false) => Stage::Manifest,
                };
                (Some(Plan::Attachments(plans)), next)
            }
            Stage::Recordings { after } => {
                let read = transact!(app.background(), |connection| {
                    self.recordings(connection, app, now, after, writes).await
                })?;
                let (plans, last) = read?;
                counts.recordings += plans.len() as u64;
                let next = match last {
                    Some(after) => Stage::Recordings { after },
                    None => Stage::Manifest,
                };
                (Some(Plan::Recordings(plans)), next)
            }
            Stage::Manifest => {
                let read = transact!(app.background(), |connection| {
                    self.title(connection, app, now).await
                })?;
                let channel_name = is_channel.then_some(read?);
                let manifest = ExportManifest {
                    format: EXPORT_FORMAT,
                    export_id: self.id,
                    place: self.place,
                    channel_name,
                    requested_by: self.account_id,
                    requested_at: self.requested_at,
                    messages: counts.messages,
                    attachments: counts.attachments,
                    recordings: counts.recordings,
                };
                let manifest = serde_json::to_vec_pretty(&manifest)
                    .map_err(|error| Stop::Error(ApiError::internal(&error)))?;
                (Some(Plan::Manifest(manifest)), Stage::End)
            }
            Stage::End => (None, Stage::End),
        };
        Ok((read.0, read.1, counts))
    }

    /// The name of the channel of the export, or a name for its conversation.
    async fn title(
        &self,
        connection: &mut PgConnection,
        app: &AppState,
        now: OffsetDateTime,
    ) -> Paged<String> {
        unless_halted!(self.permitted(&mut *connection, app, now).await?);
        Ok(Ok(match self.place {
            MessagePlace::Channel { channel_id, .. } => {
                sqlx::query_scalar("select name from channels where id = $1")
                    .bind(channel_id)
                    .fetch_one(connection)
                    .await?
            }
            MessagePlace::Direct { .. } => "Direct messages".to_owned(),
        }))
    }

    /// One page of messages as JSON and as HTML, the position of its last message when a
    /// further page can follow, and its number of messages.
    async fn messages(
        &self,
        connection: &mut PgConnection,
        app: &AppState,
        now: OffsetDateTime,
        after: (i64, Uuid),
        page: i64,
        first: bool,
    ) -> Paged<(Plan, Option<(i64, Uuid)>, u64)> {
        let (access, place) = unless_halted!(self.permitted(&mut *connection, app, now).await?);
        let rows: Vec<(MessageId, i64)> = sqlx::query_as(MESSAGE_PAGE_QUERY)
            .bind(place.id)
            .bind(self.account_id)
            .bind(after.0)
            .bind(after.1)
            .bind(self.id)
            .bind(page)
            .fetch_all(&mut *connection)
            .await?;
        let ids: Vec<MessageId> = rows.iter().map(|(id, _)| *id).collect();
        let read = messages::load_messages(&mut *connection, &access, &place, &ids).await?;
        let names = self.names(&mut *connection, &read).await?;
        let mut json = Vec::new();
        let mut page_html = String::new();
        for (index, message) in read.iter().enumerate() {
            if !(first && index == 0) {
                json.extend_from_slice(b",\n");
            }
            serde_json::to_writer(&mut json, message)
                .map_err(|error| ApiError::internal(&error))?;
            page_html.push_str(&html::message(message, &names));
        }
        let full = i64::try_from(rows.len()).unwrap_or(i64::MAX) >= page;
        let last = rows
            .last()
            .filter(|_| full)
            .map(|(id, position)| (*position, id.as_uuid()));
        let plan = Plan::Messages {
            json,
            html: page_html.into_bytes(),
        };
        Ok(Ok((plan, last, read.len() as u64)))
    }

    /// The names of the accounts and roles that a page mentions, as far as the place shows
    /// them: the members of the workspace of a channel, or the two accounts of a
    /// conversation.
    async fn names(
        &self,
        connection: &mut PgConnection,
        read: &[junto_protocol::message::Message],
    ) -> ApiResult<html::Names> {
        let (accounts, roles) = html::mentioned(read);
        let mut names = html::Names::default();
        if accounts.is_empty() && roles.is_empty() {
            return Ok(names);
        }
        match self.place {
            MessagePlace::Channel { workspace_id, .. } => {
                let found: Vec<(AccountId, String)> = sqlx::query_as(
                    "select a.id, a.name from accounts a
                     join workspace_members m on m.account_id = a.id and m.workspace_id = $2
                     where a.id = any($1)",
                )
                .bind(&accounts)
                .bind(workspace_id)
                .fetch_all(&mut *connection)
                .await?;
                names.accounts = found.into_iter().collect();
                let found: Vec<(WorkspaceRoleId, String)> = sqlx::query_as(
                    "select id, name from workspace_roles
                     where id = any($1) and workspace_id = $2",
                )
                .bind(&roles)
                .bind(workspace_id)
                .fetch_all(connection)
                .await?;
                names.roles = found.into_iter().collect();
            }
            MessagePlace::Direct {
                low_account_id,
                high_account_id,
                ..
            } => {
                let found: Vec<(AccountId, String)> = sqlx::query_as(
                    "select id, name from accounts where id = any($1) and id = any($2)",
                )
                .bind(&accounts)
                .bind([low_account_id, high_account_id].as_slice())
                .fetch_all(connection)
                .await?;
                names.accounts = found.into_iter().collect();
            }
        }
        Ok(names)
    }

    async fn attachments(
        &self,
        connection: &mut PgConnection,
        app: &AppState,
        now: OffsetDateTime,
        after: Uuid,
    ) -> Paged<(Vec<AttachmentPlan>, Option<Uuid>)> {
        let (_, place) = unless_halted!(self.permitted(&mut *connection, app, now).await?);
        let rows: Vec<(AttachmentId, AttachmentId, String, i64)> =
            sqlx::query_as(ATTACHMENT_PAGE_QUERY)
                .bind(place.id)
                .bind(after)
                .bind(self.id)
                .bind(ATTACHMENT_PAGE)
                .fetch_all(&mut *connection)
                .await?;
        let full = i64::try_from(rows.len()).unwrap_or(i64::MAX) >= ATTACHMENT_PAGE;
        let last = rows.last().filter(|_| full).map(|row| row.0.as_uuid());
        let mut plans = Vec::with_capacity(rows.len());
        for (id, storage_id, name, bytes) in rows {
            plans.push(AttachmentPlan {
                entry: format!("attachments/{id}/{}", html::file_name(&name)),
                storage_id,
                bytes: u64::try_from(bytes).unwrap_or(0),
                chunks: uploads::chunks(&mut *connection, id).await?,
            });
        }
        Ok(Ok((plans, last)))
    }

    /// One page of the recordings of the channel that the account may read now, by the
    /// rule of the routes of recordings (ADR 0604).
    async fn recordings(
        &self,
        connection: &mut PgConnection,
        app: &AppState,
        now: OffsetDateTime,
        after: Uuid,
        writes: bool,
    ) -> Paged<(Vec<RecordingPlan>, Option<Uuid>)> {
        let (access, place) = unless_halted!(self.permitted(&mut *connection, app, now).await?);
        let ids: Vec<RecordingId> = sqlx::query_scalar(RECORDING_PAGE_QUERY)
            .bind(place.id)
            .bind(after)
            .bind(self.id)
            .bind(RECORDING_PAGE)
            .fetch_all(&mut *connection)
            .await?;
        let full = i64::try_from(ids.len()).unwrap_or(i64::MAX) >= RECORDING_PAGE;
        let last = ids.last().filter(|_| full).map(|id| id.as_uuid());
        let mut plans = Vec::new();
        for id in ids {
            let readable =
                crate::routes::recordings::readable(&mut *connection, &access, id, false).await;
            let row = match readable {
                Ok((row, _)) => row,
                Err(error) if error.code() == ErrorCode::NotFound => continue,
                Err(error) => return Err(error),
            };
            // The recording of a call that still runs is for the people inside while the
            // channel hides them. The download asks this again (ADR 0604). Only the run
            // that writes the file states it: the file holds what that run read.
            if writes && recordings::call_runs(&mut *connection, row.call_id).await? {
                sqlx::query("update exports set held_call_id = $2 where id = $1")
                    .bind(self.id)
                    .bind(row.call_id)
                    .execute(&mut *connection)
                    .await?;
            }
            let segments = recordings::segments(&mut *connection, id).await?;
            let directory = format!("recordings/{id}");
            let mut files = Vec::new();
            let mut listed = Vec::new();
            for segment in &segments {
                let Some(file) = &segment.file else {
                    continue;
                };
                let (place, name) = if segment.is_track() {
                    (
                        files::Place::track(id, segment.id),
                        format!("{}.ogg", segment.id),
                    )
                } else {
                    (
                        files::Place::video(id, segment.id),
                        format!("{}.mp4", segment.id),
                    )
                };
                files.push((format!("{directory}/{name}"), place, file.bytes));
                listed.push(ExportedSegmentFile {
                    segment_id: segment.id,
                    name,
                });
            }
            let transcript = crate::routes::transcripts::text_of(&mut *connection, id).await?;
            let description = ExportedRecording {
                recording: RecordingDetail {
                    recording: recordings::summary(&mut *connection, &row).await?,
                    segments: segments.iter().map(recordings::SegmentRow::view).collect(),
                },
                files: listed,
                transcript: transcript.is_some(),
            };
            plans.push(RecordingPlan {
                directory,
                description: serde_json::to_vec_pretty(&description)
                    .map_err(|error| ApiError::internal(&error))?,
                transcript,
                files,
            });
        }
        Ok(Ok((plans, last)))
    }

    /// Reserves the counted bytes and creates the files of this attempt. The files of
    /// earlier attempts, which no process finishes any more, are deleted.
    async fn reserve(&self, app: &AppState, bytes: u64) -> Stopped<Writing> {
        let root = app.exports.directory()?.to_path_buf();
        let free = app.exports.free_bytes();
        let reserve = app.storage_reserve_bytes();
        let now = app.now();
        let id = self.id;
        let reserved: Result<(), Halt> = transact!(app.background(), |connection| {
            unless_halted!(self.permitted(&mut *connection, app, now).await?);
            sqlx::query("update exports set held_call_id = null where id = $1")
                .bind(id)
                .execute(&mut *connection)
                .await?;
            storage::release(&mut *connection, id.as_uuid()).await?;
            let kind = storage::Kind::Export;
            storage::reserve(connection, now, kind, id.as_uuid(), bytes, free, reserve).await?;
            Ok(Ok(()))
        })?;
        reserved?;

        let directory = self.directory();
        let attempt = self.attempt;
        let parts = [
            self.part("zip.partial"),
            self.part("directory.partial"),
            self.part("html.partial"),
        ];
        let at = self.requested_at;
        let attachments = app.config.attachments_directory.clone();
        let recordings = app
            .recordings
            .backend()
            .map(|backend| backend.settings.directory.clone());
        let created = tokio::task::spawn_blocking(move || {
            files::create_directory(&root, &directory)?;
            for name in files::unfinished(&root, &directory)? {
                let earlier = name
                    .split('.')
                    .next()
                    .and_then(|number| number.parse::<i32>().ok())
                    .is_some_and(|number| number < attempt);
                if earlier {
                    files::remove(&root, &directory.with(&name))?;
                }
            }
            let [archive, records, page] = parts;
            let zip = ZipWriter::new(
                files::create_file(&root, &archive)?,
                files::create_file(&root, &records)?,
                at,
            );
            Ok::<_, io::Error>(Writing {
                zip: Some(zip),
                page: files::create_file(&root, &page)?,
                attachments,
                recordings,
                reserved: bytes,
                planned: zip::END_BYTES + MARGIN,
                written: 0,
            })
        })
        .await
        .map_err(|error| Stop::Error(ApiError::internal(&error)))?;
        Ok(created?)
    }

    /// Raises the reservation to `total` bytes when the next plan needs more than was
    /// counted. The free space must hold the `pending` bytes that the files do not hold
    /// yet; otherwise the export fails with `storage_low`.
    async fn reserve_more(&self, app: &AppState, total: u64, pending: u64) -> Stopped<()> {
        let free = app.exports.free_bytes();
        let reserve = app.storage_reserve_bytes();
        let now = app.now();
        let id = self.id.as_uuid();
        let raised: Result<(), Halt> = transact!(app.background(), |connection| {
            unless_halted!(self.permitted(&mut *connection, app, now).await?);
            storage::reserve_more(connection, id, total, pending, free, reserve).await?;
            Ok(Ok(()))
        })?;
        Ok(raised?)
    }

    /// Gives the finished file its name and states that the export is ready, when its
    /// account may still export the place. When the export is gone in the meantime, the
    /// file is deleted.
    async fn seal(&mut self, app: &AppState) -> Stopped<()> {
        let root = app.exports.directory()?.to_path_buf();
        let (from, to) = (self.part("zip.partial"), self.part("zip"));
        let spare = [self.part("directory.partial"), self.part("html.partial")];
        tokio::task::spawn_blocking(move || {
            files::finish(&root, &from, &to)?;
            for place in spare {
                files::remove(&root, &place)?;
            }
            Ok::<_, io::Error>(())
        })
        .await
        .map_err(|error| Stop::Error(ApiError::internal(&error)))??;

        let now = app.now();
        let lifetime = app.config.limits.exports.lifetime;
        let (id, instance) = (self.id, app.exports.instance());
        let bytes = i64::try_from(self.bytes).map_err(|error| ApiError::internal(&error))?;
        let sealed: Result<bool, Halt> = transact!(app.background(), |connection| {
            unless_halted!(self.permitted(&mut *connection, app, now).await?);
            let ready = sqlx::query(
                "update exports
                 set state = 'ready', bytes = $3, finished_at = $4, expires_at = $5,
                     claimed_by = null, heartbeat_at = null
                 where id = $1 and claimed_by = $2 and state = 'running'
                   and removed_at is null",
            )
            .bind(id)
            .bind(instance)
            .bind(bytes)
            .bind(now)
            .bind(now + lifetime)
            .execute(&mut *connection)
            .await?;
            if ready.rows_affected() == 0 {
                return Ok(Ok(false));
            }
            storage::release(&mut *connection, id.as_uuid()).await?;
            let stored = super::export(&mut *connection, id, false)
                .await?
                .ok_or_else(ApiError::not_found)?;
            super::announce(connection, now, &stored).await?;
            Ok(Ok(true))
        })?;
        if sealed? { Ok(()) } else { Err(Stop::Gone) }
    }
}

/// States that an export has failed. With `instance`, only when that process still holds
/// the export.
async fn finish_failed(
    connection: &mut PgConnection,
    app: &AppState,
    now: OffsetDateTime,
    id: ExportId,
    instance: Option<Uuid>,
    failure: ExportFailure,
) -> ApiResult<()> {
    use crate::db::StoredText;
    let failed = sqlx::query(
        "update exports
         set state = 'failed', failure = $3, finished_at = $4, expires_at = $5,
             claimed_by = null, heartbeat_at = null
         where id = $1 and ($2::uuid is null or claimed_by = $2)
           and state in ('waiting', 'running') and removed_at is null",
    )
    .bind(id)
    .bind(instance)
    .bind(failure.stored())
    .bind(now)
    .bind(now + app.config.limits.exports.lifetime)
    .execute(&mut *connection)
    .await?;
    if failed.rows_affected() == 0 {
        return Ok(());
    }
    storage::release(&mut *connection, id.as_uuid()).await?;
    let stored: Option<ExportRow> = super::export(&mut *connection, id, false).await?;
    if let Some(stored) = stored {
        // The account that lost the access is told as well: the event names the export.
        let event = junto_protocol::events::Event::ExportUpdated {
            export: stored.export,
        };
        crate::events::tell_account(connection, now, stored.account_id, &event).await?;
    }
    Ok(())
}

/// Takes the next export and builds its file to the end. False when no export waits. After
/// an error the export waits again, and its attempt stays counted.
pub async fn run_next(app: &AppState) -> ApiResult<bool> {
    let Some(mut job) = Job::claim(app).await? else {
        return Ok(false);
    };
    loop {
        match job.step(app).await {
            Ok(Progress::More) => {}
            Ok(Progress::Finished) => return Ok(true),
            Err(error) => {
                let (id, instance) = (job.id, app.exports.instance());
                transact!(app.background(), |connection| {
                    sqlx::query(
                        "update exports
                         set state = 'waiting', claimed_by = null, heartbeat_at = null
                         where id = $1 and claimed_by = $2 and state = 'running'",
                    )
                    .bind(id)
                    .bind(instance)
                    .execute(connection)
                    .await?;
                    Ok(())
                })?;
                return Err(error);
            }
        }
    }
}
