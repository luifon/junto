//! The files of messages (ADR 0408, ADR 0721): uploads in parts that continue after a cut
//! connection, and downloads to a new file.
//!
//! A file that the server gives comes from another account. Its name is reduced to a file
//! name, its length is bounded by what the message states, and its bytes are compared with
//! the hash of the message before the file gets its name.

mod recordings;

use std::future::Future;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::StreamExt as _;
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{AttachmentId, MessageId};
use junto_protocol::message::{Attachment, Document, StartUpload};
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::api::{ApiError, CHUNK_BYTES, ErrorCategory, FileBody, ServerApi};
use crate::flow::machine::lock;
use crate::live::{Command, LiveServer};
use crate::model::{PendingFile, PlaceId};
use crate::runtime::Runtime;

pub use recordings::{
    RecordingDownload, TRANSCRIPT_TEXT_MAX_BYTES, segment_file_name, transcript_file_name,
};

/// The first wait after a request without an answer. Each further wait is twice as long.
const FIRST_WAIT: Duration = Duration::from_secs(1);
const LONGEST_WAIT: Duration = Duration::from_secs(30);
/// The longest name of a file that the server accepts, in characters.
const LONGEST_NAME: usize = 200;
/// The largest file that the server marks as a picture (ADR 0408).
pub const PICTURE_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Why a file was not uploaded or not downloaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferFailure {
    /// The file on this device could not be read or written.
    File(std::io::ErrorKind),
    /// The file is empty. The server stores no empty file.
    Empty,
    /// The file changed while it was read: its bytes are not the ones that were declared.
    Changed,
    /// The bytes that the server sent are not the file that the message states.
    Corrupt,
    /// The server refused the request.
    Refused { code: ErrorCode, status: u16 },
    /// The device is not logged in any more, or the client ends.
    Stopped,
    /// The server ended its answer before the file was complete, several times in a
    /// row.
    Interrupted,
}

impl TransferFailure {
    fn of(error: &ApiError) -> Self {
        match error.rejection() {
            Some(rejection) => Self::Refused {
                code: rejection.code,
                status: rejection.status,
            },
            None => Self::Stopped,
        }
    }
}

impl From<std::io::Error> for TransferFailure {
    fn from(error: std::io::Error) -> Self {
        Self::File(error.kind())
    }
}

/// True when the same request can succeed later: no answer, `busy`, a limit on attempts,
/// or a failure of the server.
fn passes(error: &ApiError) -> bool {
    error.is_unavailable() || error.category() == Some(ErrorCategory::Limited)
}

/// How many answers in a row may add nothing to a file before its download ends.
const IDLE_TRIES: u8 = 5;

struct Waits {
    next: Duration,
}

impl Waits {
    const fn new() -> Self {
        Self { next: FIRST_WAIT }
    }

    fn reset(&mut self) {
        self.next = FIRST_WAIT;
    }

    /// Waits before the next try: the time that the server names, or the next wait.
    async fn wait(&mut self, error: &ApiError) {
        let wait = error.retry_after().unwrap_or(self.next).min(LONGEST_WAIT);
        self.next = self.next.saturating_mul(2).min(LONGEST_WAIT);
        tokio::time::sleep(wait).await;
    }
}

/// What a person attaches to a message.
#[derive(Debug, Clone)]
pub enum UploadSource {
    /// A file of this device.
    File(PathBuf),
    /// Bytes that the application holds, such as a pasted picture.
    Bytes { name: String, bytes: Arc<Vec<u8>> },
}

impl UploadSource {
    /// The name that the server gets: the last part of the path, without the characters
    /// that the server refuses.
    pub fn name(&self) -> String {
        let name = match self {
            Self::File(path) => path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Self::Bytes { name, .. } => name.clone(),
        };
        upload_name(&name)
    }
}

/// A name that the server accepts for an upload (ADR 0408): 1 to 200 characters, without
/// path separators, control characters, invisible formatting and directional controls.
pub fn upload_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|character| {
            let refused = character.is_control()
                || matches!(character,
                    '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' |
                    '\u{2060}'..='\u{206f}' | '\u{feff}' | '/' | '\\');
            if refused { '_' } else { character }
        })
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return "file".to_owned();
    }
    shorten(cleaned, LONGEST_NAME)
}

/// At most `most` characters of a file name, with its extension kept.
fn shorten(name: &str, most: usize) -> String {
    if name.chars().count() <= most {
        return name.to_owned();
    }
    let extension = name
        .rfind('.')
        .map(|at| &name[at..])
        .filter(|extension| extension.chars().count() <= 16)
        .unwrap_or("");
    let keep = most - extension.chars().count();
    let stem: String = name.chars().take(keep).collect();
    format!("{stem}{extension}")
}

/// The ending of the name of the unfinished file of a download.
const PART_ENDING: &str = ".part";

/// The name under which a file of the server is stored on this device: one file name
/// without a path, without a leading dot and without characters that a file system
/// refuses.
pub fn local_name(name: &str) -> String {
    let last = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = last
        .chars()
        .map(|character| {
            let refused = character.is_control()
                || matches!(character,
                    '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' |
                    '\u{2060}'..='\u{206f}' | '\u{feff}' |
                    '<' | '>' | ':' | '"' | '|' | '?' | '*');
            if refused { '_' } else { character }
        })
        .collect();
    let cleaned = cleaned
        .trim_start_matches(['.', ' '])
        .trim_end_matches(['.', ' ']);
    if cleaned.is_empty() {
        return "file".to_owned();
    }
    let stem = cleaned.split('.').next().unwrap_or_default();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let reserved = RESERVED.iter().any(|name| name.eq_ignore_ascii_case(stem));
    let named = if reserved {
        format!("_{cleaned}")
    } else {
        cleaned.to_owned()
    };
    let named = shorten(&named, LONGEST_NAME);
    // The ending of the unfinished file of a download belongs to this client: no saved
    // file has it, so a download never continues in a file that a person saved.
    let ending = named.len().saturating_sub(PART_ENDING.len());
    if named
        .get(ending..)
        .is_some_and(|end| end.eq_ignore_ascii_case(PART_ENDING))
    {
        return format!("{}_", shorten(&named, LONGEST_NAME - 1));
    }
    named
}

fn hex(digest: &[u8]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The length and the SHA-256 hash of what a reader gives.
fn measure(mut reader: impl Read) -> std::io::Result<(u64, String)> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut length = 0u64;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        length += count as u64;
    }
    Ok((length, hex(&hasher.finalize())))
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Result<T, TransferFailure> {
    match tokio::task::spawn_blocking(work).await {
        Ok(result) => result.map_err(TransferFailure::from),
        Err(_) => Err(TransferFailure::Stopped),
    }
}

/// Where an upload stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadStage {
    /// The file is read for its length and its hash.
    Reading,
    Sending,
    /// The server compares the file with its hash.
    Checking,
    Complete(AttachmentId),
    Failed(TransferFailure),
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UploadProgress {
    pub stage: UploadStage,
    /// The bytes that the server holds.
    pub sent: u64,
    /// The length of the file. Zero until the file was read.
    pub bytes: u64,
    /// A request got no answer, and the client waits to continue.
    pub interrupted: bool,
}

struct UploadShared {
    progress: watch::Sender<UploadProgress>,
    /// The upload on the server, once it was started there.
    attachment_id: Mutex<Option<AttachmentId>>,
}

/// One upload. The value can be copied; every copy follows and controls the same upload.
#[derive(Clone)]
pub struct Upload {
    /// The number of the upload on this client.
    pub id: u64,
    pub name: String,
    shared: Arc<UploadShared>,
    /// Only the copies of this value hold the sender: the task of the upload learns from
    /// its end that no copy is left.
    cancelled: Arc<watch::Sender<bool>>,
}

impl Upload {
    pub fn progress(&self) -> UploadProgress {
        *self.shared.progress.borrow()
    }

    /// A value that changes with the progress.
    pub fn watch(&self) -> watch::Receiver<UploadProgress> {
        self.shared.progress.subscribe()
    }

    /// Ends the upload. What the server holds of it is deleted: the parts of an upload
    /// that runs, and the file of a complete upload that no message has.
    pub fn cancel(&self) {
        self.cancelled.send_replace(true);
    }

    /// Waits for the end of the upload.
    pub fn finished(
        &self,
    ) -> impl Future<Output = Result<AttachmentId, TransferFailure>> + Send + 'static + use<> {
        let mut watch = self.watch();
        async move {
            let ended = watch
                .wait_for(|progress| {
                    matches!(
                        progress.stage,
                        UploadStage::Complete(_) | UploadStage::Failed(_) | UploadStage::Cancelled
                    )
                })
                .await
                .map(|progress| progress.stage);
            match ended {
                Ok(UploadStage::Complete(attachment_id)) => Ok(attachment_id),
                Ok(UploadStage::Failed(failure)) => Err(failure),
                _ => Err(TransferFailure::Stopped),
            }
        }
    }
}

fn read_part(source: &UploadSource, offset: u64, length: usize) -> std::io::Result<Vec<u8>> {
    match source {
        UploadSource::File(path) => {
            let mut file = std::fs::File::open(path)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut part = vec![0u8; length];
            file.read_exact(&mut part)?;
            Ok(part)
        }
        UploadSource::Bytes { bytes, .. } => {
            let start = usize::try_from(offset).unwrap_or(usize::MAX);
            bytes
                .get(start..start.saturating_add(length))
                .map(<[u8]>::to_vec)
                .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::UnexpectedEof))
        }
    }
}

async fn upload(
    api: &ServerApi,
    place: PlaceId,
    source: UploadSource,
    name: String,
    shared: &UploadShared,
) -> Result<AttachmentId, TransferFailure> {
    let tell = |change: &dyn Fn(&mut UploadProgress)| {
        shared.progress.send_modify(|progress| change(progress));
    };
    let measured = source.clone();
    let (bytes, sha256) = blocking(move || match &measured {
        UploadSource::File(path) => measure(std::fs::File::open(path)?),
        UploadSource::Bytes { bytes, .. } => measure(bytes.as_slice()),
    })
    .await?;
    if bytes == 0 {
        return Err(TransferFailure::Empty);
    }
    tell(&|progress| {
        progress.bytes = bytes;
        progress.stage = UploadStage::Sending;
    });
    // The file of a reply in a thread belongs to the channel of the thread.
    let (channel_id, conversation_id) = match place {
        PlaceId::Channel(channel_id) | PlaceId::Thread { channel_id, .. } => {
            (Some(channel_id), None)
        }
        PlaceId::Direct(conversation_id) => (None, Some(conversation_id)),
    };
    let declared = StartUpload {
        channel_id,
        conversation_id,
        name,
        bytes,
        sha256,
    };
    let mut waits = Waits::new();
    let started = loop {
        match api.start_upload(&declared).await {
            Ok(started) => break started,
            // A start whose answer was lost leaves an upload without a file on the
            // server. It counts as one of the 20 of the account until it expires.
            Err(error) if passes(&error) => {
                tell(&|progress| progress.interrupted = true);
                waits.wait(&error).await;
            }
            Err(error) => return Err(TransferFailure::of(&error)),
        }
    };
    let attachment_id = started.id;
    *lock(&shared.attachment_id) = Some(attachment_id);
    waits.reset();
    let mut offset = started.received;
    let mut refusals = 0u8;
    while offset < bytes {
        tell(&|progress| {
            progress.sent = offset;
            progress.interrupted = false;
        });
        let length = usize::try_from(bytes - offset)
            .unwrap_or(usize::MAX)
            .min(CHUNK_BYTES);
        let part_of = source.clone();
        let part = blocking(move || read_part(&part_of, offset, length)).await?;
        match api.put_chunk(attachment_id, offset, part).await {
            Ok(held) => {
                offset = held.received;
                waits.reset();
                refusals = 0;
            }
            Err(error) => {
                let out_of_step = error.category() == Some(ErrorCategory::Conflict) && refusals < 2;
                if !passes(&error) && !out_of_step {
                    return Err(TransferFailure::of(&error));
                }
                refusals += u8::from(out_of_step);
                tell(&|progress| progress.interrupted = true);
                // The server tells how many bytes it holds: the answer to a part can be
                // lost after the part was stored.
                loop {
                    waits.wait(&error).await;
                    match api.upload_status(attachment_id).await {
                        Ok(held) => {
                            offset = held.received;
                            break;
                        }
                        Err(error) if passes(&error) => {}
                        Err(error) => return Err(TransferFailure::of(&error)),
                    }
                }
            }
        }
    }
    tell(&|progress| {
        progress.sent = bytes;
        progress.interrupted = false;
        progress.stage = UploadStage::Checking;
    });
    waits.reset();
    loop {
        match api.complete_upload(attachment_id).await {
            Ok(held) if held.complete => return Ok(attachment_id),
            Ok(_) => tokio::time::sleep(FIRST_WAIT).await,
            Err(error) if passes(&error) => waits.wait(&error).await,
            Err(error) if error.code() == Some(ErrorCode::InvalidRequest) => {
                // The bytes that the server holds do not have the declared hash.
                return Err(TransferFailure::Changed);
            }
            Err(error) => return Err(TransferFailure::of(&error)),
        }
    }
}

/// Where a download stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DownloadStage {
    Receiving,
    /// The file is compared with the hash of the message.
    Checking,
    Saved(PathBuf),
    Failed(TransferFailure),
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadProgress {
    pub stage: DownloadStage,
    pub received: u64,
    pub bytes: u64,
    /// The connection was cut, and the client waits to continue.
    pub interrupted: bool,
}

struct DownloadShared {
    progress: watch::Sender<DownloadProgress>,
    cancelled: watch::Sender<bool>,
    /// This download opened the unfinished file, so the file is its own.
    opened: AtomicBool,
}

/// One download to a file. The value can be copied.
#[derive(Clone)]
pub struct Download {
    pub attachment_id: AttachmentId,
    shared: Arc<DownloadShared>,
}

impl Download {
    pub fn progress(&self) -> DownloadProgress {
        self.shared.progress.borrow().clone()
    }

    pub fn watch(&self) -> watch::Receiver<DownloadProgress> {
        self.shared.progress.subscribe()
    }

    /// Ends the download and deletes the part of the file that arrived.
    pub fn cancel(&self) {
        self.shared.cancelled.send_replace(true);
    }

    /// Waits for the end of the download and returns the file.
    pub fn finished(
        &self,
    ) -> impl Future<Output = Result<PathBuf, TransferFailure>> + Send + 'static + use<> {
        finished(self.watch())
    }
}

/// Waits for the end of a download and returns the file.
async fn finished(
    mut watch: watch::Receiver<DownloadProgress>,
) -> Result<PathBuf, TransferFailure> {
    let ended = watch
        .wait_for(|progress| {
            matches!(
                progress.stage,
                DownloadStage::Saved(_) | DownloadStage::Failed(_) | DownloadStage::Cancelled
            )
        })
        .await
        .map(|progress| progress.stage.clone());
    match ended {
        Ok(DownloadStage::Saved(path)) => Ok(path),
        Ok(DownloadStage::Failed(failure)) => Err(failure),
        _ => Err(TransferFailure::Stopped),
    }
}

/// How long a file is that a download saves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Length {
    /// The server states the length. A download that was cut continues in its unfinished
    /// file.
    Exactly(u64),
    /// The server makes the file for each request and states no length. The client reads
    /// at most this many bytes, from the start at every try.
    AtMost(u64),
}

/// A file of the server that a download saves.
struct Wanted {
    /// The name of the saved file, as [`local_name`] gives it.
    name: String,
    /// The path of the unfinished file.
    part: PathBuf,
    length: Length,
    /// The SHA-256 hash of the file, when the server states one.
    sha256: Option<String>,
}

/// The path of the unfinished file of a download in `directory`: the name of the saved
/// file and the identifier of what is saved.
fn part_file(directory: &Path, name: &str, id: uuid::Uuid) -> PathBuf {
    directory.join(format!(
        "{}.{}{PART_ENDING}",
        shorten(name, 120),
        id.simple()
    ))
}

/// Gives the finished file a name in `directory` that no file has: the name of the
/// attachment, or that name with a number. The name is taken by creating an empty file,
/// which the finished file then replaces, so no file of another origin is replaced.
fn settle(part: &Path, directory: &Path, name: &str) -> std::io::Result<PathBuf> {
    let (stem, extension) = match name.rfind('.') {
        Some(at) if at > 0 => name.split_at(at),
        _ => (name, ""),
    };
    for number in 0..10_000u32 {
        let candidate = if number == 0 {
            directory.join(name)
        } else {
            directory.join(format!("{stem} ({number}){extension}"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(_) => {
                if let Err(error) = std::fs::rename(part, &candidate) {
                    let _ = std::fs::remove_file(&candidate);
                    return Err(error);
                }
                return Ok(candidate);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
}

/// Receives a file into its unfinished file and gives it its name in `directory`. `open`
/// asks the server for the file from a byte on.
async fn save<Open, Opened>(
    wanted: &Wanted,
    open: &Open,
    directory: &Path,
    shared: &DownloadShared,
) -> Result<PathBuf, TransferFailure>
where
    Open: Fn(u64) -> Opened,
    Opened: Future<Output = Result<FileBody, ApiError>>,
{
    let part = &wanted.part;
    let result = match wanted.length {
        Length::Exactly(bytes) => {
            receive(bytes, wanted.sha256.as_deref(), open, part, shared).await
        }
        Length::AtMost(most) => receive_whole(most, open, part, shared).await,
    };
    match result {
        Ok(file) => {
            let (part, directory) = (part.clone(), directory.to_path_buf());
            let name = wanted.name.clone();
            // The file stays locked until it has its name.
            blocking(move || {
                let settled = settle(&part, &directory, &name);
                drop(file);
                settled
            })
            .await
        }
        Err(failure) => {
            // A part that a cut connection left stays for the next try. A part that no
            // later try can continue does not: its bytes are not the file that the
            // server states, or the server has no such file or no byte after them.
            let ended = match failure {
                TransferFailure::Corrupt | TransferFailure::Interrupted => true,
                TransferFailure::Refused { status, .. } => matches!(status, 404 | 410 | 416),
                _ => false,
            };
            if ended && shared.opened.load(Ordering::Relaxed) {
                discard_part(part);
            }
            Err(failure)
        }
    }
}

/// Deletes an unfinished file that no later try can continue. The file is locked for the
/// deletion: a file that another download holds at that moment stays.
fn discard_part(part: &Path) {
    let Ok(file) = std::fs::OpenOptions::new().read(true).open(part) else {
        return;
    };
    if file.try_lock().is_ok() {
        let _ = std::fs::remove_file(part);
    }
}

/// True when `named` and `held` are one file that has one name. A file with a second
/// name can be a file of another origin under the name of an unfinished download.
#[cfg(unix)]
fn own_file(named: &std::fs::Metadata, held: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    named.dev() == held.dev() && named.ino() == held.ino() && held.nlink() == 1
}

#[cfg(not(unix))]
fn own_file(_: &std::fs::Metadata, _: &std::fs::Metadata) -> bool {
    true
}

/// Opens the unfinished file of a download of `bytes` bytes, and returns it with the
/// count of bytes that it holds. The path must name a regular file with one name: a link
/// is refused before anything is written, so no file of another origin changes. The file
/// is locked until it is closed, so a second download of the same file is refused while
/// this one runs.
fn open_part(part: &Path, bytes: u64) -> std::io::Result<(std::fs::File, u64)> {
    let regular = |path: &Path| match std::fs::symlink_metadata(path) {
        Ok(named) if named.file_type().is_file() => Ok(Some(named)),
        Ok(_) => Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    };
    regular(part)?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .create(true)
        .append(true)
        .open(part)?;
    // Opening wrote nothing. The path is read again: it still names the opened file.
    let held = file.metadata()?;
    let named = regular(part)?.ok_or(std::io::ErrorKind::NotFound)?;
    if !own_file(&named, &held) {
        return Err(std::io::ErrorKind::AlreadyExists.into());
    }
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => std::io::ErrorKind::ResourceBusy.into(),
        std::fs::TryLockError::Error(error) => error,
    })?;
    if held.len() > bytes {
        file.set_len(0)?;
        return Ok((file, 0));
    }
    Ok((file, held.len()))
}

/// Receives the `bytes` bytes of a file into its unfinished file, from the byte after the
/// ones that the file holds, and compares the file with the stated length, and with the
/// hash when the server states one. Returns the file, which is locked while it is open.
async fn receive<Open, Opened>(
    bytes: u64,
    sha256: Option<&str>,
    open: &Open,
    part: &Path,
    shared: &DownloadShared,
) -> Result<std::fs::File, TransferFailure>
where
    Open: Fn(u64) -> Opened,
    Opened: Future<Output = Result<FileBody, ApiError>>,
{
    let opened = part.to_path_buf();
    let (mut file, mut received) = blocking(move || open_part(&opened, bytes)).await?;
    shared.opened.store(true, Ordering::Relaxed);
    let mut waits = Waits::new();
    // The answers in a row after which the file holds no more bytes than it held at
    // most before. A server that answers without the file, or that starts the file again
    // at every answer, does not make the client ask without an end (ADR 0714).
    let mut idle = 0u8;
    let mut most = received;
    while received < bytes {
        shared.progress.send_modify(|progress| {
            progress.received = received;
        });
        let opened = match open(received).await {
            Ok(opened) => opened,
            Err(error) if passes(&error) => {
                shared
                    .progress
                    .send_modify(|progress| progress.interrupted = true);
                waits.wait(&error).await;
                continue;
            }
            Err(error) => return Err(TransferFailure::of(&error)),
        };
        // A part of the file that starts at another byte than the one that was asked
        // for is not the rest of the file.
        if opened.partial && opened.starts_at.is_some_and(|at| at != received) {
            return Err(TransferFailure::Corrupt);
        }
        if !opened.partial && received > 0 {
            // The answer is the whole file: what is held is replaced.
            file.set_len(0)?;
            received = 0;
        }
        let mut body = opened.body;
        let mut cut = None;
        while let Some(chunk) = body.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    cut = Some(ApiError::Unreachable(error));
                    break;
                }
            };
            if received + chunk.len() as u64 > bytes {
                return Err(TransferFailure::Corrupt);
            }
            file.write_all(&chunk)?;
            received += chunk.len() as u64;
            waits.reset();
            shared.progress.send_modify(|progress| {
                progress.received = received;
                progress.interrupted = false;
            });
        }
        idle = if received > most { 0 } else { idle + 1 };
        most = most.max(received);
        if received < bytes && idle >= IDLE_TRIES {
            return Err(TransferFailure::Interrupted);
        }
        if let Some(error) = cut {
            shared
                .progress
                .send_modify(|progress| progress.interrupted = true);
            waits.wait(&error).await;
        } else if received < bytes {
            // The body ended before the file did. The next request asks for the rest.
            tokio::time::sleep(FIRST_WAIT).await;
        }
    }
    file.flush()?;
    shared.progress.send_modify(|progress| {
        progress.received = received;
        progress.interrupted = false;
        progress.stage = DownloadStage::Checking;
    });
    let stated = sha256.map(str::to_owned);
    let hashed = stated.is_some();
    let (file, length, measured) = blocking(move || {
        if !hashed {
            let length = file.metadata()?.len();
            return Ok((file, length, None));
        }
        file.seek(SeekFrom::Start(0))?;
        let (length, sha256) = measure(&mut file)?;
        Ok((file, length, Some(sha256)))
    })
    .await?;
    if length != bytes || measured != stated {
        return Err(TransferFailure::Corrupt);
    }
    Ok(file)
}

/// Receives a file without a stated length into its unfinished file: at most `most`
/// bytes, from the first byte at every try. What an earlier try left is not kept, because
/// the server makes the file for each request. Returns the file, which is locked while it
/// is open.
async fn receive_whole<Open, Opened>(
    most: u64,
    open: &Open,
    part: &Path,
    shared: &DownloadShared,
) -> Result<std::fs::File, TransferFailure>
where
    Open: Fn(u64) -> Opened,
    Opened: Future<Output = Result<FileBody, ApiError>>,
{
    let opened = part.to_path_buf();
    let (mut file, _) = blocking(move || open_part(&opened, most)).await?;
    shared.opened.store(true, Ordering::Relaxed);
    let mut waits = Waits::new();
    // The answers in a row that were cut. Each try reads the file from its start, so a
    // connection that is cut at every try never completes it.
    let mut cuts = 0u8;
    loop {
        file.set_len(0)?;
        let mut received = 0u64;
        shared
            .progress
            .send_modify(|progress| progress.received = received);
        let opened = match open(0).await {
            Ok(opened) => opened,
            Err(error) if passes(&error) => {
                shared
                    .progress
                    .send_modify(|progress| progress.interrupted = true);
                waits.wait(&error).await;
                continue;
            }
            Err(error) => return Err(TransferFailure::of(&error)),
        };
        let mut body = opened.body;
        let mut cut = None;
        while let Some(chunk) = body.next().await {
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(error) => {
                    cut = Some(ApiError::Unreachable(error));
                    break;
                }
            };
            if received + chunk.len() as u64 > most {
                return Err(TransferFailure::Corrupt);
            }
            file.write_all(&chunk)?;
            received += chunk.len() as u64;
            shared.progress.send_modify(|progress| {
                progress.received = received;
                progress.interrupted = false;
            });
        }
        let Some(error) = cut else {
            file.flush()?;
            shared.progress.send_modify(|progress| {
                progress.bytes = received;
                progress.stage = DownloadStage::Checking;
            });
            return Ok(file);
        };
        cuts += 1;
        if cuts >= IDLE_TRIES {
            return Err(TransferFailure::Interrupted);
        }
        shared
            .progress
            .send_modify(|progress| progress.interrupted = true);
        waits.wait(&error).await;
    }
}

/// Reads a whole file of a message into memory: a picture that the window draws. A file
/// above `most` bytes, and a file with other bytes than the message states, is refused.
pub async fn read_whole(
    api: &ServerApi,
    attachment: &Attachment,
    most: u64,
) -> Result<Vec<u8>, TransferFailure> {
    if attachment.bytes > most {
        return Err(TransferFailure::Corrupt);
    }
    let length = usize::try_from(attachment.bytes).map_err(|_| TransferFailure::Corrupt)?;
    let mut waits = Waits::new();
    for _ in 0..4 {
        let opened = match api.attachment_file(attachment.id, 0).await {
            Ok(opened) => opened,
            Err(error) if passes(&error) => {
                waits.wait(&error).await;
                continue;
            }
            Err(error) => return Err(TransferFailure::of(&error)),
        };
        let mut body = opened.body;
        let mut held = Vec::with_capacity(length);
        let mut cut = None;
        while let Some(chunk) = body.next().await {
            match chunk {
                Ok(chunk) if held.len() + chunk.len() <= length => held.extend_from_slice(&chunk),
                Ok(_) => return Err(TransferFailure::Corrupt),
                Err(error) => {
                    cut = Some(ApiError::Unreachable(error));
                    break;
                }
            }
        }
        if let Some(error) = cut {
            waits.wait(&error).await;
            continue;
        }
        if hex(&Sha256::digest(&held)) != attachment.sha256 {
            return Err(TransferFailure::Corrupt);
        }
        return Ok(held);
    }
    Err(TransferFailure::Stopped)
}

/// The files of the messages of one saved server.
#[derive(Clone)]
pub struct Transfers {
    pub(crate) api: ServerApi,
    pub(crate) live: LiveServer,
    pub(crate) runtime: Runtime,
}

impl Transfers {
    /// Starts the upload of a file for a message of `place`. The upload runs until it is
    /// complete, fails or is cancelled, also when the returned value is dropped.
    pub fn upload(&self, place: PlaceId, source: UploadSource) -> Upload {
        let name = source.name();
        let shared = Arc::new(UploadShared {
            progress: watch::Sender::new(UploadProgress {
                stage: UploadStage::Reading,
                sent: 0,
                bytes: 0,
                interrupted: false,
            }),
            attachment_id: Mutex::new(None),
        });
        let handle = Upload {
            id: self.live.shared.next_number(),
            name: name.clone(),
            shared: shared.clone(),
            cancelled: Arc::new(watch::Sender::new(false)),
        };
        let mut cancelled = handle.cancelled.subscribe();
        let api = self.api.clone();
        drop(self.runtime.run(async move {
            let tell = |stage: UploadStage| {
                shared.progress.send_modify(|progress| {
                    progress.stage = stage;
                    progress.interrupted = false;
                });
            };
            let work = upload(&api, place, source, name, &shared);
            tokio::pin!(work);
            let stage = tokio::select! {
                result = &mut work => match result {
                    Ok(attachment_id) => UploadStage::Complete(attachment_id),
                    Err(failure) => UploadStage::Failed(failure),
                },
                // No copy of the value is left: nobody can cancel, and the upload runs
                // to its end.
                asked = async { cancelled.wait_for(|cancelled| *cancelled).await.is_ok() } => {
                    if asked {
                        UploadStage::Cancelled
                    } else {
                        match work.await {
                            Ok(attachment_id) => UploadStage::Complete(attachment_id),
                            Err(failure) => UploadStage::Failed(failure),
                        }
                    }
                }
            };
            tell(stage);
            let started = *lock(&shared.attachment_id);
            match (started, stage) {
                (Some(attachment_id), UploadStage::Complete(_)) => {
                    // The file waits on the server for its message and counts as an
                    // upload of the account until then (ADR 0408). A cancel deletes it.
                    // The server refuses that for a file that a message has.
                    if cancelled.wait_for(|cancelled| *cancelled).await.is_ok() {
                        let _ = api.cancel_upload(attachment_id).await;
                        tell(UploadStage::Cancelled);
                    }
                }
                (Some(attachment_id), _) => {
                    // What the server holds of an upload that ended without a file is
                    // deleted. A failure leaves it to its expiry (ADR 0408).
                    let _ = api.cancel_upload(attachment_id).await;
                }
                (None, _) => {}
            }
        }));
        handle
    }

    /// Sends a message with files. The message is in the state as one that waits from this
    /// moment, and is sent when every upload is complete. Returns the number of the
    /// message on this client.
    pub fn send(
        &self,
        place: PlaceId,
        document: Document,
        reply_to: Option<MessageId>,
        uploads: &[Upload],
    ) -> Option<u64> {
        let files = uploads
            .iter()
            .map(|upload| {
                let progress = upload.progress();
                PendingFile {
                    upload: upload.id,
                    name: upload.name.clone(),
                    bytes: progress.bytes,
                    attachment_id: match progress.stage {
                        UploadStage::Complete(attachment_id) => Some(attachment_id),
                        _ => None,
                    },
                }
            })
            .collect::<Vec<_>>();
        let waits = files.iter().any(|file| file.attachment_id.is_none());
        let id = self
            .live
            .enqueue_message(place, document, reply_to, files)?;
        if waits {
            let uploads = uploads.to_vec();
            let live = self.live.clone();
            drop(self.runtime.run(async move {
                // The first upload that fails ends the wait, also while another one
                // still runs.
                let ends = uploads.iter().map(|upload| {
                    let (number, ended) = (upload.id, upload.finished());
                    async move { ended.await.map(|attachment_id| (number, attachment_id)) }
                });
                match futures_util::future::try_join_all(ends).await {
                    Ok(files) => live.shared.send(Command::FilesReady { id, files }),
                    Err(_) => live.shared.send(Command::FilesFailed(id)),
                }
            }));
        }
        Some(id)
    }

    /// Starts the download of a file of a message into `directory`. The file gets the
    /// name of the attachment, or that name with a number when a file has it. Nothing is
    /// replaced.
    pub fn download(&self, attachment: &Attachment, directory: PathBuf) -> Download {
        let name = local_name(&attachment.name);
        let wanted = Wanted {
            part: part_file(&directory, &name, attachment.id.as_uuid()),
            name,
            length: Length::Exactly(attachment.bytes),
            sha256: Some(attachment.sha256.clone()),
        };
        let (api, attachment_id) = (self.api.clone(), attachment.id);
        let open = move |from| {
            let api = api.clone();
            async move { api.attachment_file(attachment_id, from).await }
        };
        Download {
            attachment_id,
            shared: self.start_download(wanted, directory, open),
        }
    }

    /// Starts a download into `directory`. It runs until the file is saved, the download
    /// fails or it is cancelled, also when the returned value is dropped.
    fn start_download<Open, Opened>(
        &self,
        wanted: Wanted,
        directory: PathBuf,
        open: Open,
    ) -> Arc<DownloadShared>
    where
        Open: Fn(u64) -> Opened + Send + Sync + 'static,
        Opened: Future<Output = Result<FileBody, ApiError>> + Send + 'static,
    {
        let shared = Arc::new(DownloadShared {
            progress: watch::Sender::new(DownloadProgress {
                stage: DownloadStage::Receiving,
                received: 0,
                bytes: match wanted.length {
                    Length::Exactly(bytes) => bytes,
                    Length::AtMost(_) => 0,
                },
                interrupted: false,
            }),
            cancelled: watch::Sender::new(false),
            opened: AtomicBool::new(false),
        });
        let task = shared.clone();
        drop(self.runtime.run(async move {
            let shared = task;
            let mut cancelled = shared.cancelled.subscribe();
            let stage = tokio::select! {
                result = save(&wanted, &open, &directory, &shared) => match result {
                    Ok(path) => DownloadStage::Saved(path),
                    Err(failure) => DownloadStage::Failed(failure),
                },
                _ = cancelled.wait_for(|cancelled| *cancelled) => {
                    // The unfinished file of another download of the same file stays.
                    if shared.opened.load(Ordering::Relaxed) {
                        let _ = std::fs::remove_file(&wanted.part);
                    }
                    DownloadStage::Cancelled
                }
            };
            shared.progress.send_modify(|progress| {
                progress.stage = stage;
                progress.interrupted = false;
            });
        }));
        shared
    }

    /// Reads a picture of a message into memory, at most [`PICTURE_MAX_BYTES`].
    pub fn picture(
        &self,
        attachment: &Attachment,
    ) -> impl Future<Output = Result<Vec<u8>, TransferFailure>> + Send + 'static + use<> {
        let (api, attachment) = (self.api.clone(), attachment.clone());
        let work = self
            .runtime
            .run(async move { read_whole(&api, &attachment, PICTURE_MAX_BYTES).await });
        async move { work.await.unwrap_or(Err(TransferFailure::Stopped)) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_name_of_an_upload_is_one_that_the_server_accepts() {
        assert_eq!(upload_name("notes.pdf"), "notes.pdf");
        assert_eq!(upload_name("a/b\\c.txt"), "a_b_c.txt");
        assert_eq!(upload_name("  "), "file");
        assert_eq!(upload_name("x\u{202e}fdp.exe"), "x_fdp.exe");
        let long = format!("{}.tar", "n".repeat(300));
        let name = upload_name(&long);
        assert_eq!(name.chars().count(), 200);
        assert!(name.ends_with(".tar"));
        let source = UploadSource::File(PathBuf::from("/tmp/some dir/report final.pdf"));
        assert_eq!(source.name(), "report final.pdf");
    }

    #[test]
    fn the_name_of_a_download_is_one_file_name_without_a_path() {
        assert_eq!(local_name("notes.pdf"), "notes.pdf");
        assert_eq!(local_name("../../etc/passwd"), "passwd");
        assert_eq!(local_name("..\\..\\boot.ini"), "boot.ini");
        assert_eq!(local_name(".bashrc"), "bashrc");
        assert_eq!(local_name("..."), "file");
        assert_eq!(local_name("a:b|c?.txt"), "a_b_c_.txt");
        assert_eq!(local_name("con.txt"), "_con.txt");
        assert_eq!(local_name("report. "), "report");
        assert_eq!(local_name("x\u{202e}fdp.exe"), "x_fdp.exe");
    }

    #[test]
    fn a_finished_file_gets_a_name_that_no_file_has() {
        let directory = tempfile::tempdir().unwrap();
        let existing = directory.path().join("notes.pdf");
        std::fs::write(&existing, b"mine").unwrap();
        let part = directory.path().join("notes.pdf.0123.part");
        std::fs::write(&part, b"theirs").unwrap();
        let settled = settle(&part, directory.path(), "notes.pdf").unwrap();
        assert_eq!(settled, directory.path().join("notes (1).pdf"));
        assert_eq!(std::fs::read(&existing).unwrap(), b"mine");
        assert_eq!(std::fs::read(&settled).unwrap(), b"theirs");
        assert!(!part.exists());

        let part = directory.path().join("README.part");
        std::fs::write(&part, b"text").unwrap();
        let settled = settle(&part, directory.path(), "README").unwrap();
        assert_eq!(settled, directory.path().join("README"));
    }

    #[cfg(unix)]
    #[test]
    fn a_part_file_that_is_a_link_is_refused_and_its_target_stays() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("mine.txt");
        std::fs::write(&target, b"mine").unwrap();
        let part = directory.path().join("notes.pdf.0123.part");
        std::os::unix::fs::symlink(&target, &part).unwrap();
        // The file of the message is shorter than the target of the link.
        assert!(open_part(&part, 3).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"mine");
        // A link to a file that does not exist creates no file.
        let absent = directory.path().join("absent.txt");
        let part = directory.path().join("other.pdf.0123.part");
        std::os::unix::fs::symlink(&absent, &part).unwrap();
        assert!(open_part(&part, 3).is_err());
        assert!(!absent.exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_part_file_that_is_a_second_name_of_another_file_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("mine.txt");
        std::fs::write(&target, b"mine").unwrap();
        let part = directory.path().join("notes.pdf.0123.part");
        std::fs::hard_link(&target, &part).unwrap();
        assert!(open_part(&part, 3).is_err());
        assert_eq!(std::fs::read(&target).unwrap(), b"mine");
    }

    #[test]
    fn one_part_file_serves_one_download_at_a_time() {
        let directory = tempfile::tempdir().unwrap();
        let part = directory.path().join("notes.pdf.0123.part");
        let (first, held) = open_part(&part, 10).unwrap();
        assert_eq!(held, 0);
        assert!(open_part(&part, 10).is_err());
        drop(first);
        std::fs::write(&part, b"abc").unwrap();
        let (_second, held) = open_part(&part, 10).unwrap();
        assert_eq!(held, 3);
    }

    #[test]
    fn no_saved_file_has_the_name_of_a_part_file() {
        assert_eq!(local_name("notes.pdf.0123.part"), "notes.pdf.0123.part_");
        assert_eq!(local_name("a.PART"), "a.PART_");
        assert_eq!(local_name("part"), "part");
    }

    #[test]
    fn a_file_is_measured_as_the_server_measures_it() {
        let (length, sha256) = measure(&b"abc"[..]).unwrap();
        assert_eq!(length, 3);
        assert_eq!(
            sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
