//! The requests of the administration about the records of a server: access tokens
//! (ADR 0210), the exports of the person and their files (ADR 0411), and the reads of the
//! audit log (ADR 0305).

use std::collections::HashMap;
use std::future::Future;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use futures_channel::oneshot;
use gpui::Context;
use junto_client_core::api::{ApiError, ErrorCategory, ServerApi};
use junto_client_core::protocol::access_token::CreateAccessToken;
use junto_client_core::protocol::audit::{AuditEntry, AuditFilterQuery};
use junto_client_core::protocol::export::ExportState;
use junto_client_core::protocol::ids::{
    AccessTokenId, AccountId, AuditEntryId, ExportId, ServerId, WorkspaceId,
};

use super::view::records::{
    AUDIT_PAGE, ExportSave, MOST_AUDIT_ENTRIES, export_file_name, export_of,
};
use super::{AskKind, Made};
use crate::state::AppState;
use crate::text;

/// How many requests for the file of an export in a row may make no progress.
const MOST_TRIES: u32 = 5;
/// The bytes beyond the most that an earlier answer reached that a request must bring to
/// count as progress, when it does not reach the end of the file.
const LEAST_PROGRESS: u64 = 64 * 1024;
/// The longest part of a wait. A save that is stopped ends its wait within one part.
const WAIT_PART: Duration = Duration::from_millis(250);
/// The wait after the first request without an answer. Each further wait is twice as
/// long.
const FIRST_WAIT: Duration = Duration::from_secs(1);
const LONGEST_WAIT: Duration = Duration::from_secs(16);
/// How often the page reads the progress of a file that is saved.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
/// The ending of the name of the unfinished file of a save.
const PART_ENDING: &str = ".part";

/// The number of the last read of the audit log. It does not start again when the log is
/// put away with the administration, so a late answer never has the number of a later
/// read.
static AUDIT_READS: AtomicU64 = AtomicU64::new(0);

/// A part of the audit log: the entries about the server, or the entries of one
/// workspace (ADR 0305).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditScope {
    Server,
    Workspace(WorkspaceId),
}

/// How far back the page reads the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum AuditPeriod {
    #[default]
    Any,
    Day,
    Week,
    Month,
}

impl AuditPeriod {
    pub const ALL: [Self; 4] = [Self::Any, Self::Day, Self::Week, Self::Month];

    /// The time from which a read that starts at `now` asks for entries. Absent: from
    /// the first entry of the log.
    pub fn since(self, now: time::OffsetDateTime) -> Option<time::OffsetDateTime> {
        let days = match self {
            Self::Any => return None,
            Self::Day => 1,
            Self::Week => 7,
            Self::Month => 30,
        };
        Some(now - time::Duration::days(days))
    }
}

/// The filters of the audit log. The server applies them, so a page holds only entries
/// that pass them (ADR 0305).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AuditFilter {
    /// One action, by its name in the protocol. Absent: every action.
    pub action: Option<&'static str>,
    /// The account that acted. Absent: every actor.
    pub actor: Option<AccountId>,
    /// The account that the entries name. Absent: every entry.
    pub about: Option<AccountId>,
    pub period: AuditPeriod,
}

impl AuditFilter {
    /// True when the filter narrows the log.
    pub fn is_set(&self) -> bool {
        *self != Self::default()
    }
}

/// The most accounts that the page keeps as the people of its filters.
pub(crate) const MOST_AUDIT_PEOPLE: usize = 200;

/// Why the last read of the audit log failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AuditFailure {
    pub(crate) sentence: String,
    /// The read asked for the page after the loaded entries.
    pub(crate) older: bool,
}

/// The entries of the audit log that the page has read. The audit log is no collection of
/// the client core: the page asks for pages of it.
#[derive(Debug, Default)]
pub struct AuditLog {
    /// The log that the page asked for.
    pub(crate) shown: Option<(ServerId, AuditScope)>,
    /// The newest first.
    pub(crate) entries: Vec<AuditEntry>,
    /// The server holds entries before this one. Absent after the last page.
    pub(crate) next_before: Option<AuditEntryId>,
    /// A read is on its way.
    pub(crate) reading: bool,
    pub(crate) failed: Option<AuditFailure>,
    /// The filters that the entries were asked for with.
    pub(crate) filter: AuditFilter,
    /// The start of the period that the first page of the entries was asked for with.
    /// The read of an older page repeats it, so that both pages belong to one list.
    pub(crate) since: Option<time::OffsetDateTime>,
    /// The accounts that acted in an entry of a page that was read for the shown log,
    /// with any filter. The filters offer them.
    pub(crate) people: Vec<AccountId>,
    /// The number of the read whose answer the log takes.
    pub(crate) read: u64,
}

/// The request for one page of the audit log: the filters, the start of their period as
/// the first page stated it, and the entry that the page continues after.
pub(crate) fn audit_query(
    filter: &AuditFilter,
    since: Option<time::OffsetDateTime>,
    before: Option<AuditEntryId>,
) -> AuditFilterQuery {
    AuditFilterQuery {
        action: filter.action.map(str::to_owned),
        actor_id: filter.actor,
        target_id: filter.about.map(|account_id| account_id.as_uuid()),
        since,
        until: None,
        before,
        limit: Some(AUDIT_PAGE),
    }
}

/// The files of exports that the person saves, and those that were saved.
#[derive(Debug, Default)]
pub struct ExportFiles {
    pub(crate) files: HashMap<(ServerId, ExportId), ExportSave>,
    /// The saves that run. A save that the state does not hold under its number changes
    /// nothing when it ends.
    pub(crate) running: HashMap<(ServerId, ExportId), RunningSave>,
    pub(crate) saves: u64,
}

/// What a running save tells and is told: the bytes that it has written, and that it
/// must stop.
#[derive(Debug, Default)]
pub(crate) struct Progress {
    written: AtomicU64,
    stopped: AtomicBool,
}

impl Progress {
    fn is_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed)
    }
}

/// One save that runs.
#[derive(Debug)]
pub(crate) struct RunningSave {
    number: u64,
    /// The account that started the save. The file belongs to this account.
    account_id: AccountId,
    progress: Arc<Progress>,
}

/// Why the file of an export was not saved.
#[derive(Debug)]
enum SaveFailure {
    /// The file on this device could not be written.
    File,
    /// The answers of the server ended before the file did, several times in a row.
    Ended,
    /// The server sent more bytes than the export has.
    TooLong,
    Refused(ApiError),
    /// The save was stopped: its server or its account is gone, or the client ends.
    Stopped,
}

impl SaveFailure {
    fn sentence(&self) -> String {
        match self {
            Self::File => text::admin::records::SAVE_FILE_FAILED.to_owned(),
            Self::Ended => text::admin::records::SAVE_ENDED.to_owned(),
            Self::TooLong => text::admin::records::SAVE_TOO_LONG.to_owned(),
            Self::Refused(error) => text::admin::records::save_refused(error),
            Self::Stopped => text::admin::records::SAVE_STOPPED.to_owned(),
        }
    }
}

impl From<std::io::Error> for SaveFailure {
    fn from(_: std::io::Error) -> Self {
        Self::File
    }
}

/// Waits for a time on the runtime of the client. The wait of a save that is stopped ends
/// early.
type Wait = Box<dyn Fn(Duration) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// True when the same request can succeed later: no answer, `busy`, a limit on attempts,
/// or a failure of the server.
fn passes(error: &ApiError) -> bool {
    error.is_unavailable() || error.category() == Some(ErrorCategory::Limited)
}

/// The unfinished file of a save, beside the place of the finished file. It is deleted
/// when the save ends without a finished file, also when the work is stopped.
struct Part {
    path: PathBuf,
    file: std::fs::File,
    settled: bool,
}

impl Part {
    /// Creates the unfinished file. What an earlier save of the same export left under
    /// the name is removed first; a link under the name is removed and not followed.
    fn create(path: PathBuf) -> std::io::Result<Self> {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        Ok(Self {
            path,
            file,
            settled: false,
        })
    }

    /// Gives the finished file a name in `directory` that no file has: `name`, or that
    /// name with a number. The name is taken by creating an empty file, which the
    /// finished file then replaces, so no file of another origin is replaced.
    fn settle(mut self, directory: &Path, name: &str) -> std::io::Result<PathBuf> {
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
                    if let Err(error) = std::fs::rename(&self.path, &candidate) {
                        let _ = std::fs::remove_file(&candidate);
                        return Err(error);
                    }
                    self.settled = true;
                    return Ok(candidate);
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists))
    }
}

impl Drop for Part {
    fn drop(&mut self) {
        if !self.settled {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Waits for `time` in parts of at most [`WAIT_PART`], and ends after the part in which
/// the save was stopped.
async fn in_parts<F: Future<Output = ()>>(
    time: Duration,
    progress: &Progress,
    sleep: impl Fn(Duration) -> F,
) {
    let mut left = time;
    while !left.is_zero() && !progress.is_stopped() {
        let part = left.min(WAIT_PART);
        sleep(part).await;
        left = left.saturating_sub(part);
    }
}

/// Receives the `bytes` bytes of the file of an export into `file` and tells the count
/// of written bytes in `progress`.
///
/// The server ends an answer when the access of the account to the export ends
/// (ADR 0411), and a cut connection ends one too. A file with fewer bytes than the export
/// has is never complete: the rest is asked for from the byte that was reached, and the
/// answer to that request tells a refusal.
///
/// The server does not decide how long the save runs. A request makes progress when it
/// reaches the end of the file, or when it brings at least [`LEAST_PROGRESS`] bytes
/// beyond the most that any earlier answer reached; an answer that is the whole file
/// although a part was asked for makes none. After [`MOST_TRIES`] requests in a row
/// without progress the save fails. A download that makes progress has no bound on its
/// requests, so a large file arrives over a connection that is cut again and again.
/// Every request after the first one waits.
///
/// A save that is stopped ends before its next request, before its next write, and
/// after its wait.
async fn receive(
    api: &ServerApi,
    export_id: ExportId,
    bytes: u64,
    file: &mut std::fs::File,
    progress: &Progress,
    wait: &Wait,
) -> Result<(), SaveFailure> {
    let mut received = 0u64;
    // The most bytes that the file held after any answer.
    let mut reached = 0u64;
    let mut tries = 0u32;
    let mut first = true;
    let mut next_wait = FIRST_WAIT;
    // The wait that the server named, and why the last request got no answer.
    let mut named_wait: Option<Duration> = None;
    let mut unanswered: Option<ApiError> = None;
    while received < bytes {
        if progress.is_stopped() {
            return Err(SaveFailure::Stopped);
        }
        if tries >= MOST_TRIES {
            return Err(unanswered.map_or(SaveFailure::Ended, SaveFailure::Refused));
        }
        if !first {
            let time = named_wait.take().unwrap_or(next_wait).min(LONGEST_WAIT);
            next_wait = next_wait.saturating_mul(2).min(LONGEST_WAIT);
            wait(time).await;
            if progress.is_stopped() {
                return Err(SaveFailure::Stopped);
            }
        }
        first = false;
        let opened = match api.export_file(export_id, received).await {
            Ok(opened) => opened,
            Err(error) if passes(&error) => {
                tries += 1;
                named_wait = error.retry_after();
                unanswered = Some(error);
                continue;
            }
            Err(error) => return Err(SaveFailure::Refused(error)),
        };
        unanswered = None;
        let whole_again = !opened.partial && received > 0;
        if whole_again {
            // The answer is the whole file: what is held is replaced.
            file.set_len(0)?;
            file.seek(SeekFrom::Start(0))?;
            received = 0;
            progress.written.store(0, Ordering::Relaxed);
        }
        let mut body = opened.body;
        loop {
            let next = std::future::poll_fn(|context| body.as_mut().poll_next(context)).await;
            match next {
                Some(Ok(chunk)) => {
                    if progress.is_stopped() {
                        return Err(SaveFailure::Stopped);
                    }
                    let length = chunk.len() as u64;
                    if length > bytes - received {
                        return Err(SaveFailure::TooLong);
                    }
                    file.write_all(&chunk)?;
                    received += length;
                    progress.written.store(received, Ordering::Relaxed);
                }
                // A cut connection and the end of the answer are told apart by the
                // next request.
                Some(Err(_)) | None => break,
            }
        }
        let further = received >= bytes || received >= reached.saturating_add(LEAST_PROGRESS);
        reached = reached.max(received);
        if further && !whole_again {
            tries = 0;
            next_wait = FIRST_WAIT;
        } else {
            tries += 1;
        }
    }
    file.flush()?;
    Ok(())
}

/// Saves the file of an export into `directory` under `name`, or under that name with a
/// number when a file has it. The bytes go to an unfinished file beside it first. A save
/// that is stopped gives the file no name.
async fn save(
    api: &ServerApi,
    export_id: ExportId,
    bytes: u64,
    directory: &Path,
    name: &str,
    progress: &Progress,
    wait: &Wait,
) -> Result<PathBuf, SaveFailure> {
    let part = directory.join(format!(
        "{name}.{}{PART_ENDING}",
        export_id.as_uuid().simple()
    ));
    let mut part = Part::create(part)?;
    receive(api, export_id, bytes, &mut part.file, progress, wait).await?;
    if progress.is_stopped() {
        return Err(SaveFailure::Stopped);
    }
    Ok(part.settle(directory, name)?)
}

impl AppState {
    /// Creates an access token. The answer is the only one that carries the token, so
    /// the token is kept until the person puts it away (ADR 0210).
    pub fn create_access_token(
        &mut self,
        server_id: ServerId,
        token: CreateAccessToken,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::CreateToken,
            |api| async move { api.create_access_token(&token).await },
            |this, created, _| {
                this.admin.made = Some(Made::Token {
                    access_token_id: created.access_token.id,
                    token: created.token,
                });
            },
            cx,
        );
    }

    /// The text of the new access token with this identifier, while the person has not
    /// put it away. The caller gives it to the clipboard and keeps no copy.
    pub(crate) fn new_token_text(&self, access_token_id: AccessTokenId) -> Option<String> {
        match &self.admin.made {
            Some(Made::Token {
                access_token_id: made,
                token,
            }) if *made == access_token_id => Some(token.expose().to_owned()),
            _ => None,
        }
    }

    /// Revokes an access token. The server refuses every further request with it.
    pub fn revoke_access_token(
        &mut self,
        server_id: ServerId,
        access_token_id: AccessTokenId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::RevokeToken(access_token_id),
            |api| async move { api.revoke_access_token(access_token_id).await },
            |_, (), _| {},
            cx,
        );
    }

    /// Removes an export of the person with its file on the server. A file that the
    /// person saved stays on this device.
    pub fn remove_export(
        &mut self,
        server_id: ServerId,
        export_id: ExportId,
        cx: &mut Context<Self>,
    ) {
        self.admin_ask(
            server_id,
            AskKind::RemoveExport(export_id),
            |api| async move { api.remove_export(export_id).await },
            move |this, (), _| {
                let key = (server_id, export_id);
                // A save that runs ends by itself: the server ends its answer.
                if !this.admin.exports.running.contains_key(&key) {
                    this.admin.exports.files.remove(&key);
                }
            },
            cx,
        );
    }

    /// Saves the file of an export into the download directory of the person, with its
    /// progress. The file gets a name that no file has. The work runs on the runtime of
    /// the client, as the downloads of the files of messages do. The save belongs to the
    /// account that started it: its requests are made for that account only, and
    /// [`AppState::end_stale_export_saves`] stops it when the account or the server is
    /// gone. It goes on when the administration is closed.
    pub fn save_export(
        &mut self,
        server_id: ServerId,
        export_id: ExportId,
        cx: &mut Context<Self>,
    ) {
        let key = (server_id, export_id);
        if self.admin.exports.running.contains_key(&key) {
            return;
        }
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let Some(account_id) = server.account() else {
            return;
        };
        let Some(model) = server.live().view().model else {
            return;
        };
        let Some(export) = model
            .admin
            .exports
            .loaded()
            .and_then(|exports| exports.iter().find(|held| held.id == export_id))
        else {
            return;
        };
        let (ExportState::Ready, Some(bytes)) = (export.state, export.bytes) else {
            return;
        };
        self.forget_gone_export_files();
        let Some(directory) = self.download_directory() else {
            self.admin.exports.files.insert(
                key,
                ExportSave::Failed(text::NO_DOWNLOAD_DIRECTORY.to_owned()),
            );
            cx.notify();
            return;
        };
        let of = export_of(server_id, export, &model, &self.known);
        let requested = self.known.zone.local(export.requested_at).date();
        let name = export_file_name(&of, requested);

        let progress = Arc::new(Progress::default());
        let exports = &mut self.admin.exports;
        exports.saves += 1;
        let number = exports.saves;
        exports.running.insert(
            key,
            RunningSave {
                number,
                account_id,
                progress: progress.clone(),
            },
        );
        exports.files.insert(key, ExportSave::Saving { written: 0 });

        let (ended, end) = oneshot::channel();
        // A request of the save fails when the device has another account by then.
        let api = server.api_for(account_id);
        let handle = self.client.runtime().handle().clone();
        // The window names no timer of the runtime: a wait is a sleep on one of its
        // threads that may block.
        let pool = handle.clone();
        let told = progress.clone();
        let wait: Wait = Box::new(
            move |time: Duration| -> Pin<Box<dyn Future<Output = ()> + Send>> {
                let (pool, told) = (pool.clone(), told.clone());
                Box::pin(async move {
                    in_parts(time, &told, |part| {
                        let sleeping = pool.spawn_blocking(move || std::thread::sleep(part));
                        async move {
                            let _ = sleeping.await;
                        }
                    })
                    .await;
                })
            },
        );
        let saving = progress.clone();
        drop(handle.spawn(async move {
            let result = save(&api, export_id, bytes, &directory, &name, &saving, &wait).await;
            let _ = ended.send(result);
        }));
        cx.spawn(async move |this, cx| {
            let result = end.await.unwrap_or(Err(SaveFailure::Stopped));
            let _ = this.update(cx, |this, cx| {
                this.export_file_ended(key, number, result, cx);
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(PROGRESS_INTERVAL).await;
                let now = progress.written.load(Ordering::Relaxed);
                let runs = this.update(cx, |this, cx| {
                    this.export_file_progress(key, number, now, cx)
                });
                if !matches!(runs, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    /// True while the save with this number runs.
    fn export_file_progress(
        &mut self,
        key: (ServerId, ExportId),
        number: u64,
        written: u64,
        cx: &mut Context<Self>,
    ) -> bool {
        let exports = &mut self.admin.exports;
        if exports.running.get(&key).map(|held| held.number) != Some(number) {
            return false;
        }
        let state = ExportSave::Saving { written };
        if exports.files.get(&key) != Some(&state) {
            exports.files.insert(key, state);
            cx.notify();
        }
        true
    }

    fn export_file_ended(
        &mut self,
        key: (ServerId, ExportId),
        number: u64,
        result: Result<PathBuf, SaveFailure>,
        cx: &mut Context<Self>,
    ) {
        let exports = &mut self.admin.exports;
        // A save that was stopped is not held any more: nothing of it is shown.
        if exports.running.get(&key).map(|held| held.number) != Some(number) {
            return;
        }
        exports.running.remove(&key);
        let state = match result {
            Ok(path) => ExportSave::Saved(path),
            Err(failure) => ExportSave::Failed(failure.sentence()),
        };
        exports.files.insert(key, state);
        self.forget_gone_export_files();
        cx.notify();
    }

    /// Stops every save whose server this device holds no longer, and every save whose
    /// server has another account now than the one that started it, or none. Such a save
    /// writes no further byte and gives its file no name, and nothing of it is shown to
    /// the next account. It runs with every change of the state.
    pub(super) fn end_stale_export_saves(&mut self) {
        let stale: Vec<(ServerId, ExportId)> = self
            .admin
            .exports
            .running
            .iter()
            .filter(|(key, held)| {
                let account = self
                    .client
                    .server(key.0)
                    .and_then(|server| server.account());
                account != Some(held.account_id)
            })
            .map(|(key, _)| *key)
            .collect();
        for key in stale {
            if let Some(held) = self.admin.exports.running.remove(&key) {
                held.progress.stopped.store(true, Ordering::Relaxed);
            }
            self.admin.exports.files.remove(&key);
        }
    }

    /// What the page told about the file of an export belongs to a row of the page. The
    /// file of an export that its server lists no longer, and of a server that this
    /// device has no longer, is put away.
    fn forget_gone_export_files(&mut self) {
        let servers = self.client.servers();
        let exports = &mut self.admin.exports;
        let running = &exports.running;
        exports.files.retain(|key, _| {
            if running.contains_key(key) {
                return true;
            }
            let Some(server) = servers.iter().find(|server| server.id() == key.0) else {
                return false;
            };
            let model = server.live().view().model;
            match model
                .as_ref()
                .and_then(|model| model.admin.exports.loaded())
            {
                Some(listed) => listed.iter().any(|held| held.id == key.1),
                // The list is not read now: nothing is known about the export.
                None => true,
            }
        });
    }

    /// Shows the audit log of a scope and reads its first page. A second call for the
    /// same scope reads it again from the start; the loaded entries stay until the
    /// answer arrives.
    pub fn show_audit_log(
        &mut self,
        server_id: ServerId,
        scope: AuditScope,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let audit = &mut self.admin.audit;
        if audit.shown != Some((server_id, scope)) {
            // The entries, the filters and their people belong to the log that was shown
            // before.
            audit.shown = Some((server_id, scope));
            audit.entries.clear();
            audit.next_before = None;
            audit.filter = AuditFilter::default();
            audit.since = None;
            audit.people.clear();
        }
        self.read_audit(server.api(), server_id, scope, None, cx);
    }

    /// Reads the shown log of a server again from its start with these filters. The
    /// entries of the filters before leave the page at once: they are not the answer to
    /// what the person asked for now.
    pub fn set_audit_filter(
        &mut self,
        server_id: ServerId,
        filter: AuditFilter,
        cx: &mut Context<Self>,
    ) {
        let Some((shown, scope)) = self.admin.audit.shown else {
            return;
        };
        if shown != server_id || self.admin.audit.filter == filter {
            return;
        }
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let audit = &mut self.admin.audit;
        audit.filter = filter;
        audit.entries.clear();
        audit.next_before = None;
        self.read_audit(server.api(), server_id, scope, None, cx);
    }

    /// Reads the page of entries after the loaded ones. Nothing is read while a read is
    /// on its way, after the last page, and when the page holds the most entries that it
    /// keeps.
    pub fn read_older_audit(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        let audit = &self.admin.audit;
        let Some((shown, scope)) = audit.shown else {
            return;
        };
        if shown != server_id || audit.reading || audit.entries.len() >= MOST_AUDIT_ENTRIES {
            return;
        }
        let Some(before) = audit.next_before else {
            return;
        };
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        self.read_audit(server.api(), server_id, scope, Some(before), cx);
    }

    fn read_audit(
        &mut self,
        api: ServerApi,
        server_id: ServerId,
        scope: AuditScope,
        before: Option<AuditEntryId>,
        cx: &mut Context<Self>,
    ) {
        let number = AUDIT_READS.fetch_add(1, Ordering::Relaxed) + 1;
        let audit = &mut self.admin.audit;
        audit.read = number;
        audit.reading = true;
        audit.failed = None;
        let filter = audit.filter;
        let since = match before {
            Some(_) => audit.since,
            None => filter.period.since(time::OffsetDateTime::now_utc()),
        };
        let query = audit_query(&filter, since, before);
        cx.spawn(async move |this, cx| {
            let read = match scope {
                AuditScope::Server => api.server_audit_log(&query).await,
                AuditScope::Workspace(workspace_id) => {
                    api.workspace_audit_log(workspace_id, &query).await
                }
            };
            let _ = this.update(cx, |this, cx| {
                let audit = &mut this.admin.audit;
                // A late answer to an earlier read, or to a read of another log, is not
                // the one that the page waits for.
                if audit.read != number || audit.shown != Some((server_id, scope)) {
                    return;
                }
                audit.reading = false;
                match read {
                    Ok(page) => {
                        if before.is_none() {
                            audit.entries.clear();
                            audit.since = since;
                        }
                        for actor in page.entries.iter().filter_map(|entry| entry.actor_id) {
                            if audit.people.len() < MOST_AUDIT_PEOPLE
                                && !audit.people.contains(&actor)
                            {
                                audit.people.push(actor);
                            }
                        }
                        audit.entries.extend(page.entries);
                        audit.entries.truncate(MOST_AUDIT_ENTRIES);
                        audit.next_before = page.next_before;
                    }
                    Err(error) => {
                        audit.failed = Some(AuditFailure {
                            sentence: text::admin::records::audit_failure(&error),
                            older: before.is_some(),
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Draws this audit log and these files of exports in place of what a server
    /// answered and of what a save tells. The page starts no read for a log that names
    /// its scope. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_records_fixture(
        &mut self,
        audit: AuditLog,
        exports: ExportFiles,
        cx: &mut Context<Self>,
    ) {
        self.admin.audit = audit;
        self.admin.exports = exports;
        self.changed(cx);
    }
}

#[cfg(feature = "test-support")]
impl AuditLog {
    /// The log of a scope as a read left it: these entries, the newest first. With
    /// `older` the server holds entries after them.
    pub fn loaded(
        server_id: ServerId,
        scope: AuditScope,
        entries: Vec<AuditEntry>,
        older: bool,
    ) -> Self {
        Self {
            shown: Some((server_id, scope)),
            next_before: entries.last().filter(|_| older).map(|entry| entry.id),
            entries,
            ..Self::default()
        }
    }

    /// A read is on its way: the first page for a log without entries, and the page
    /// after the loaded entries otherwise.
    pub fn with_reading(mut self) -> Self {
        self.reading = true;
        self.failed = None;
        self
    }

    /// The last read failed, with the sentence that the page states.
    pub fn with_failure(mut self, sentence: impl Into<String>) -> Self {
        self.reading = false;
        self.failed = Some(AuditFailure {
            sentence: sentence.into(),
            older: !self.entries.is_empty(),
        });
        self
    }

    /// The entries were asked for with these filters, and the server returned the
    /// entries that `passes` keeps.
    pub fn with_filter(
        mut self,
        filter: AuditFilter,
        passes: impl Fn(&AuditEntry) -> bool,
    ) -> Self {
        self.filter = filter;
        self.entries.retain(passes);
        self.next_before = None;
        self
    }

    /// These accounts acted in an entry of a page that was read before.
    pub fn with_people(mut self, people: Vec<AccountId>) -> Self {
        self.people = people;
        self
    }
}

#[cfg(feature = "test-support")]
impl ExportFiles {
    /// The files of these exports: one that is saved now with its written bytes, one
    /// that was saved with its path, and one that failed with its sentence.
    pub fn of(files: impl IntoIterator<Item = ((ServerId, ExportId), ExportSave)>) -> Self {
        Self {
            files: files.into_iter().collect(),
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;
    use std::sync::atomic::AtomicU32;

    use futures_channel::mpsc;
    use junto_client_core::address::ServerAddress;
    use junto_client_core::api::{
        ApiClient, BodyStream, HttpRequest, OpenFuture, OpenedResponse, RetryPolicy, Transport,
        TransportError, TransportErrorKind, TransportFuture,
    };
    use junto_client_core::secret::SecretToken;
    use junto_client_core::version::SupportedVersions;

    use super::*;

    #[test]
    fn a_period_starts_a_number_of_days_before_the_read_and_any_time_has_no_start() {
        let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        assert_eq!(AuditPeriod::Any.since(now), None);
        for (period, days) in [
            (AuditPeriod::Day, 1),
            (AuditPeriod::Week, 7),
            (AuditPeriod::Month, 30),
        ] {
            assert_eq!(period.since(now), Some(now - time::Duration::days(days)));
        }
        assert_eq!(AuditPeriod::ALL[0], AuditPeriod::default());
        assert!(!AuditFilter::default().is_set());
        let narrowed = AuditFilter {
            period: AuditPeriod::Day,
            ..AuditFilter::default()
        };
        assert!(narrowed.is_set());
    }

    #[test]
    fn the_request_of_a_page_states_the_filters_and_an_older_page_repeats_their_start() {
        let (actor, about) = (AccountId::generate(), AccountId::generate());
        let now = time::OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        let filter = AuditFilter {
            action: Some("account_banned"),
            actor: Some(actor),
            about: Some(about),
            period: AuditPeriod::Week,
        };
        let since = filter.period.since(now);
        let first = audit_query(&filter, since, None);
        assert_eq!(first.action.as_deref(), Some("account_banned"));
        assert_eq!(first.actor_id, Some(actor));
        // The person concerned is an identifier that the entry names.
        assert_eq!(first.target_id, Some(about.as_uuid()));
        assert_eq!(first.since, Some(now - time::Duration::days(7)));
        assert_eq!((first.until, first.before), (None, None));
        assert_eq!(first.limit, Some(AUDIT_PAGE));
        // The page after it names the last entry and the same filters, with the start of
        // the period as the first page stated it.
        let last = AuditEntryId::generate();
        let older = audit_query(&filter, since, Some(last));
        assert_eq!(older.before, Some(last));
        assert_eq!(
            (older.action, older.actor_id, older.target_id, older.since),
            (first.action, first.actor_id, first.target_id, first.since)
        );
        // No filter: the request of before.
        let plain = audit_query(&AuditFilter::default(), None, None);
        assert_eq!(
            plain,
            AuditFilterQuery {
                limit: Some(AUDIT_PAGE),
                ..AuditFilterQuery::default()
            }
        );
    }

    /// What the server of a test answers to one request for the file: a status, and the
    /// parts of the body in the order in which they arrive.
    struct Answer {
        status: u16,
        parts: Vec<Result<Vec<u8>, TransportError>>,
    }

    impl Answer {
        /// The answer to a request without a range: these bytes as the whole file.
        fn whole(bytes: &[u8]) -> Self {
            Self {
                status: 200,
                parts: vec![Ok(bytes.to_vec())],
            }
        }

        /// The answer to a request with a range: these bytes from the byte that was asked
        /// for.
        fn part(bytes: &[u8]) -> Self {
            Self {
                status: 206,
                parts: vec![Ok(bytes.to_vec())],
            }
        }

        /// The connection fails after the parts of the answer.
        fn cut(mut self) -> Self {
            self.parts.push(Err(TransportError::new(
                TransportErrorKind::Interrupted,
                "cut by the test",
            )));
            self
        }
    }

    /// The answer to a request, from the byte that it asks for and its number, which
    /// starts at 1.
    type Script = Box<dyn Fn(u64, usize) -> Answer + Send + Sync>;

    /// A server that gives the file of an export as its script says, and keeps the byte
    /// from which each request asked.
    struct Server {
        script: Script,
        asked: Mutex<Vec<u64>>,
    }

    impl Server {
        fn asked(&self) -> Vec<u64> {
            self.asked.lock().unwrap().clone()
        }
    }

    fn stream(parts: Vec<Result<Vec<u8>, TransportError>>) -> BodyStream {
        let (sender, receiver) = mpsc::unbounded();
        for part in parts {
            let _ = sender.unbounded_send(part.map(Into::into));
        }
        Box::pin(receiver)
    }

    impl Transport for Server {
        fn send(&self, _: HttpRequest) -> TransportFuture {
            Box::pin(async {
                Err(TransportError::new(
                    TransportErrorKind::Connect,
                    "the server of the test has no such route",
                ))
            })
        }

        fn open(&self, _: HttpRequest, from: u64) -> OpenFuture {
            let number = {
                let mut asked = self.asked.lock().unwrap();
                asked.push(from);
                asked.len()
            };
            let answer = (self.script)(from, number);
            Box::pin(async move {
                Ok(OpenedResponse {
                    range_start: None,
                    status: answer.status,
                    retry_after: None,
                    body: stream(answer.parts),
                })
            })
        }
    }

    fn server(
        script: impl Fn(u64, usize) -> Answer + Send + Sync + 'static,
    ) -> (Arc<Server>, ServerApi) {
        let server = Arc::new(Server {
            script: Box::new(script),
            asked: Mutex::default(),
        });
        let api = ApiClient::new(
            server.clone(),
            ServerAddress::parse("chat.example.test").unwrap(),
            ServerId::generate(),
            SupportedVersions::THIS_BUILD.negotiate(&[1]).unwrap(),
            RetryPolicy::default(),
        )
        .with_token(SecretToken::new("jst_test".to_owned()));
        (server, api)
    }

    /// A wait that returns at once, and the number of waits that were asked for.
    fn no_wait() -> (Arc<AtomicU32>, Wait) {
        let waits = Arc::new(AtomicU32::new(0));
        let counted = waits.clone();
        let wait: Wait = Box::new(
            move |_: Duration| -> Pin<Box<dyn Future<Output = ()> + Send>> {
                counted.fetch_add(1, Ordering::Relaxed);
                Box::pin(async {})
            },
        );
        (waits, wait)
    }

    fn run<F: Future>(work: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(work)
    }

    /// The bytes of a file of a test.
    fn content(length: usize) -> Vec<u8> {
        (0..length).map(|at| (at % 251) as u8).collect()
    }

    fn names_in(directory: &Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|held| held.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    const NAME: &str = "junto-export-general-2027-01-15.zip";

    #[test]
    fn a_server_that_answers_every_request_with_one_byte_of_the_whole_file_ends_the_save() {
        // The export has 100 bytes. Every answer is the whole file, holds one byte and
        // ends without a failure.
        let (server, api) = server(|_, _| Answer::whole(b"x"));
        let (waits, wait) = no_wait();
        let directory = tempfile::tempdir().unwrap();
        let progress = Progress::default();
        let saved = run(save(
            &api,
            ExportId::generate(),
            100,
            directory.path(),
            NAME,
            &progress,
            &wait,
        ));
        assert!(matches!(saved, Err(SaveFailure::Ended)), "{saved:?}");
        // One byte is no progress, and no later answer brings a byte beyond the first.
        let asked = server.asked();
        assert_eq!(asked, [0, 1, 1, 1, 1]);
        assert_eq!(asked.len(), MOST_TRIES as usize);
        // Every request after the first one waited.
        assert_eq!(waits.load(Ordering::Relaxed) as usize, asked.len() - 1);
        assert!(names_in(directory.path()).is_empty());
    }

    #[test]
    fn a_whole_file_that_grows_by_one_byte_with_each_answer_does_not_count_as_progress() {
        // Each answer is the whole file again and holds one byte more than the one
        // before: the most that was reached grows, and the save still ends.
        let file = content(100);
        let (server, api) = server(move |_, number| Answer::whole(&file[..number]));
        let (_, wait) = no_wait();
        let mut held = tempfile::tempfile().unwrap();
        let progress = Progress::default();
        let received = run(receive(
            &api,
            ExportId::generate(),
            100,
            &mut held,
            &progress,
            &wait,
        ));
        assert!(matches!(received, Err(SaveFailure::Ended)), "{received:?}");
        assert_eq!(server.asked(), [0, 1, 2, 3, 4]);
    }

    /// A server whose connection fails after `parts` parts of `part` bytes of each
    /// answer. An answer starts at the byte that was asked for.
    fn interrupted(file: Vec<u8>, part: usize, parts: usize) -> impl Fn(u64, usize) -> Answer {
        move |from, _| {
            let from = from as usize;
            let end = file.len().min(from + part * parts);
            let answer = Answer {
                status: if from == 0 { 200 } else { 206 },
                parts: file[from..end]
                    .chunks(part)
                    .map(|bytes| Ok(bytes.to_vec()))
                    .collect(),
            };
            if end < file.len() {
                answer.cut()
            } else {
                answer
            }
        }
    }

    #[test]
    fn a_large_file_arrives_over_a_connection_that_is_cut_again_and_again() {
        // The export has 10 MiB. Each answer brings 8 parts of 100 KiB, and then the
        // connection fails.
        let file = content(10 * 1024 * 1024);
        let (server, api) = server(interrupted(file.clone(), 100 * 1024, 8));
        let (waits, wait) = no_wait();
        let directory = tempfile::tempdir().unwrap();
        let progress = Progress::default();
        let saved = run(save(
            &api,
            ExportId::generate(),
            file.len() as u64,
            directory.path(),
            NAME,
            &progress,
            &wait,
        ))
        .unwrap();
        assert!(std::fs::read(&saved).unwrap() == file);
        let asked = server.asked();
        // 12 answers of 800 KiB, and one with the rest.
        assert_eq!(asked.len(), 13);
        assert_eq!(asked[1], 800 * 1024);
        // Every request after the first one waited.
        assert_eq!(waits.load(Ordering::Relaxed), 12);
        assert_eq!(progress.written.load(Ordering::Relaxed), file.len() as u64);
        assert_eq!(names_in(directory.path()), [NAME]);
    }

    #[test]
    fn a_save_has_no_bound_on_requests_that_make_progress() {
        // Each of 70 answers brings the least bytes that count as progress.
        let step = LEAST_PROGRESS as usize;
        assert_eq!(step, 64 * 1024);
        let file = content(70 * step);
        let (server, api) = server(interrupted(file.clone(), step, 1));
        let (_, wait) = no_wait();
        let mut held = tempfile::tempfile().unwrap();
        let progress = Progress::default();
        run(receive(
            &api,
            ExportId::generate(),
            file.len() as u64,
            &mut held,
            &progress,
            &wait,
        ))
        .unwrap();
        assert_eq!(server.asked().len(), 70);
        assert_eq!(held.metadata().unwrap().len(), file.len() as u64);
    }

    #[test]
    fn a_server_that_gives_1024_bytes_with_each_answer_ends_the_save_after_five_requests() {
        let file = content(1024 * 1024);
        let (server, api) = server(interrupted(file.clone(), 1024, 1));
        let (waits, wait) = no_wait();
        let directory = tempfile::tempdir().unwrap();
        let progress = Progress::default();
        let saved = run(save(
            &api,
            ExportId::generate(),
            file.len() as u64,
            directory.path(),
            NAME,
            &progress,
            &wait,
        ));
        assert!(matches!(saved, Err(SaveFailure::Ended)), "{saved:?}");
        assert_eq!(server.asked(), [0, 1024, 2048, 3072, 4096]);
        assert_eq!(waits.load(Ordering::Relaxed), 4);
        assert!(names_in(directory.path()).is_empty());
    }

    #[test]
    fn the_last_bytes_of_a_file_count_as_progress_also_when_they_are_few() {
        // After four answers without progress the fifth one brings the rest of the file.
        let file = content(4 * 1024 + 100);
        let (server, api) = server(interrupted(file.clone(), 1024, 1));
        let (_, wait) = no_wait();
        let mut held = tempfile::tempfile().unwrap();
        let progress = Progress::default();
        run(receive(
            &api,
            ExportId::generate(),
            file.len() as u64,
            &mut held,
            &progress,
            &wait,
        ))
        .unwrap();
        assert_eq!(server.asked(), [0, 1024, 2048, 3072, 4096]);
        assert_eq!(held.metadata().unwrap().len(), file.len() as u64);
    }

    #[test]
    fn a_save_that_is_stopped_before_its_first_request_asks_for_nothing_and_leaves_no_file() {
        let file = content(100);
        let (server, api) = server(move |_, _| Answer::whole(&file));
        let (waits, wait) = no_wait();
        let directory = tempfile::tempdir().unwrap();
        let progress = Progress::default();
        progress.stopped.store(true, Ordering::Relaxed);
        let saved = run(save(
            &api,
            ExportId::generate(),
            100,
            directory.path(),
            NAME,
            &progress,
            &wait,
        ));
        assert!(matches!(saved, Err(SaveFailure::Stopped)), "{saved:?}");
        assert!(server.asked().is_empty());
        assert_eq!(waits.load(Ordering::Relaxed), 0);
        assert!(names_in(directory.path()).is_empty());
    }

    #[test]
    fn a_save_that_is_stopped_while_an_answer_arrives_writes_nothing_and_asks_no_more() {
        // The save is stopped when the server serves the first answer, which is the whole
        // file.
        let file = content(100);
        let progress = Arc::new(Progress::default());
        let stopping = progress.clone();
        let (server, api) = server(move |_, _| {
            stopping.stopped.store(true, Ordering::Relaxed);
            Answer::whole(&file)
        });
        let (_, wait) = no_wait();
        let directory = tempfile::tempdir().unwrap();
        let saved = run(save(
            &api,
            ExportId::generate(),
            100,
            directory.path(),
            NAME,
            &progress,
            &wait,
        ));
        assert!(matches!(saved, Err(SaveFailure::Stopped)), "{saved:?}");
        assert_eq!(server.asked(), [0]);
        assert_eq!(progress.written.load(Ordering::Relaxed), 0);
        assert!(names_in(directory.path()).is_empty());
    }

    #[test]
    fn a_save_that_is_stopped_between_two_requests_makes_no_further_request() {
        // The first answer holds 40 of 100 bytes and ends. The save is stopped while it
        // waits for its second request.
        let file = content(100);
        let (server, api) = server(move |_, _| Answer::whole(&file[..40]));
        let progress = Arc::new(Progress::default());
        let stopping = progress.clone();
        let wait: Wait = Box::new(
            move |_: Duration| -> Pin<Box<dyn Future<Output = ()> + Send>> {
                stopping.stopped.store(true, Ordering::Relaxed);
                Box::pin(async {})
            },
        );
        let directory = tempfile::tempdir().unwrap();
        let saved = run(save(
            &api,
            ExportId::generate(),
            100,
            directory.path(),
            NAME,
            &progress,
            &wait,
        ));
        assert!(matches!(saved, Err(SaveFailure::Stopped)), "{saved:?}");
        assert_eq!(server.asked(), [0]);
        assert_eq!(progress.written.load(Ordering::Relaxed), 40);
        assert!(names_in(directory.path()).is_empty());
    }

    #[test]
    fn a_wait_in_parts_ends_after_the_part_in_which_the_save_was_stopped() {
        let progress = Progress::default();
        let parts = AtomicU32::new(0);
        // The whole wait is made of 8 parts.
        run(in_parts(WAIT_PART * 8, &progress, |part| {
            assert_eq!(part, WAIT_PART);
            parts.fetch_add(1, Ordering::Relaxed);
            async {}
        }));
        assert_eq!(parts.load(Ordering::Relaxed), 8);
        // The save is stopped in the third part.
        parts.store(0, Ordering::Relaxed);
        run(in_parts(WAIT_PART * 8, &progress, |_| {
            if parts.fetch_add(1, Ordering::Relaxed) == 2 {
                progress.stopped.store(true, Ordering::Relaxed);
            }
            async {}
        }));
        assert_eq!(parts.load(Ordering::Relaxed), 3);
        // A stopped save does not wait.
        run(in_parts(WAIT_PART * 8, &progress, |_| {
            parts.fetch_add(1, Ordering::Relaxed);
            async {}
        }));
        assert_eq!(parts.load(Ordering::Relaxed), 3);
        // A wait that is no multiple of a part ends with a shorter part.
        let waited = Mutex::new(Vec::new());
        run(in_parts(
            WAIT_PART * 2 + Duration::from_millis(10),
            &Progress::default(),
            |part| {
                waited.lock().unwrap().push(part);
                async {}
            },
        ));
        assert_eq!(
            *waited.lock().unwrap(),
            [WAIT_PART, WAIT_PART, Duration::from_millis(10)]
        );
    }

    #[test]
    fn a_body_with_more_bytes_than_the_export_has_fails_before_the_excess_is_written() {
        // The export has 100 bytes. The body holds 150 bytes in three parts.
        let file = content(150);
        let (server, api) = server(move |_, _| Answer {
            status: 200,
            parts: vec![
                Ok(file[..60].to_vec()),
                Ok(file[60..120].to_vec()),
                Ok(file[120..].to_vec()),
            ],
        });
        let (_, wait) = no_wait();
        let mut held = tempfile::tempfile().unwrap();
        let progress = Progress::default();
        let received = run(receive(
            &api,
            ExportId::generate(),
            100,
            &mut held,
            &progress,
            &wait,
        ));
        assert!(
            matches!(received, Err(SaveFailure::TooLong)),
            "{received:?}"
        );
        let length = held.metadata().unwrap().len();
        assert_eq!(length, 60);
        assert!(length <= 100);
        assert_eq!(progress.written.load(Ordering::Relaxed), 60);
        assert_eq!(server.asked(), [0]);
    }

    #[test]
    fn a_body_with_one_byte_too_many_at_its_end_fails() {
        let file = content(101);
        let (_, api) = server(move |_, _| Answer {
            status: 200,
            parts: vec![Ok(file[..100].to_vec()), Ok(file[100..].to_vec())],
        });
        let (_, wait) = no_wait();
        let directory = tempfile::tempdir().unwrap();
        let progress = Progress::default();
        let saved = run(save(
            &api,
            ExportId::generate(),
            100,
            directory.path(),
            NAME,
            &progress,
            &wait,
        ));
        assert!(matches!(saved, Err(SaveFailure::TooLong)), "{saved:?}");
        assert!(names_in(directory.path()).is_empty());
    }

    #[test]
    fn a_body_that_is_cut_is_continued_from_the_byte_that_was_reached() {
        let file = content(100);
        for cut in [true, false] {
            // The first answer holds 40 bytes: then the connection fails, or the answer
            // ends. The second answer starts at the byte that was asked for.
            let served = file.clone();
            let (server, api) = server(move |from, _| match from {
                0 if cut => Answer::whole(&served[..40]).cut(),
                0 => Answer::whole(&served[..40]),
                reached => Answer::part(&served[reached as usize..]),
            });
            let (waits, wait) = no_wait();
            let directory = tempfile::tempdir().unwrap();
            let progress = Progress::default();
            let saved = run(save(
                &api,
                ExportId::generate(),
                100,
                directory.path(),
                NAME,
                &progress,
                &wait,
            ))
            .unwrap();
            assert_eq!(saved, directory.path().join(NAME));
            assert_eq!(std::fs::read(&saved).unwrap(), file);
            assert_eq!(server.asked(), [0, 40]);
            assert_eq!(waits.load(Ordering::Relaxed), 1);
            assert_eq!(progress.written.load(Ordering::Relaxed), 100);
            assert_eq!(names_in(directory.path()), [NAME]);
        }
    }

    #[test]
    fn an_answer_that_is_the_whole_file_again_replaces_what_was_held() {
        // The server knows no ranges: the answer to the second request is the whole
        // file, although the request asked for the bytes from 40 on.
        let file = content(100);
        let served = file.clone();
        let (server, api) = server(move |_, number| match number {
            1 => Answer::whole(&served[..40]).cut(),
            _ => Answer::whole(&served),
        });
        let (_, wait) = no_wait();
        let directory = tempfile::tempdir().unwrap();
        let progress = Progress::default();
        let saved = run(save(
            &api,
            ExportId::generate(),
            100,
            directory.path(),
            NAME,
            &progress,
            &wait,
        ))
        .unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), file);
        assert_eq!(server.asked(), [0, 40]);
    }

    #[test]
    fn a_file_is_saved_under_a_name_that_no_file_has_and_a_second_save_gets_a_number() {
        let file = content(100);
        let served = file.clone();
        let (_, api) = server(move |_, _| Answer::whole(&served));
        let (waits, wait) = no_wait();
        let directory = tempfile::tempdir().unwrap();
        let export_id = ExportId::generate();
        let save_again = || {
            let progress = Progress::default();
            let saved = run(save(
                &api,
                export_id,
                100,
                directory.path(),
                NAME,
                &progress,
                &wait,
            ))
            .unwrap();
            assert_eq!(progress.written.load(Ordering::Relaxed), 100);
            saved
        };
        let first = save_again();
        assert_eq!(first, directory.path().join(NAME));
        assert_eq!(std::fs::read(&first).unwrap(), file);
        // The person changes the first file. The second save does not replace it.
        std::fs::write(&first, b"mine").unwrap();
        let second = save_again();
        let numbered = "junto-export-general-2027-01-15 (1).zip";
        assert_eq!(second, directory.path().join(numbered));
        assert_eq!(std::fs::read(&second).unwrap(), file);
        assert_eq!(std::fs::read(&first).unwrap(), b"mine");
        let third = save_again();
        assert_eq!(
            third,
            directory
                .path()
                .join("junto-export-general-2027-01-15 (2).zip")
        );
        // No unfinished file stays, and a whole answer needs no wait.
        assert_eq!(names_in(directory.path()).len(), 3);
        assert!(
            names_in(directory.path())
                .iter()
                .all(|name| !name.ends_with(PART_ENDING))
        );
        assert_eq!(waits.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_save_that_the_server_refuses_after_a_cut_leaves_no_unfinished_file() {
        // The first answer is cut after 40 bytes. The server then refuses the request for
        // the rest, as it does when the access of the account to the export has ended.
        let file = content(100);
        let (server, api) = server(move |from, _| match from {
            0 => Answer::whole(&file[..40]).cut(),
            _ => Answer {
                status: 404,
                parts: vec![Ok(b"gone".to_vec())],
            },
        });
        let (_, wait) = no_wait();
        let directory = tempfile::tempdir().unwrap();
        let progress = Progress::default();
        let saved = run(save(
            &api,
            ExportId::generate(),
            100,
            directory.path(),
            NAME,
            &progress,
            &wait,
        ));
        assert!(matches!(saved, Err(SaveFailure::Refused(_))), "{saved:?}");
        // A refusal is final: the file is not asked for again.
        assert_eq!(server.asked(), [0, 40]);
        assert!(names_in(directory.path()).is_empty());
    }

    #[test]
    fn a_server_that_cannot_be_reached_ends_the_save_after_five_requests() {
        let directory = tempfile::tempdir().unwrap();
        let (server, api) = server(|_, _| Answer {
            status: 503,
            parts: vec![Ok(b"down".to_vec())],
        });
        let (waits, wait) = no_wait();
        let progress = Progress::default();
        let saved = run(save(
            &api,
            ExportId::generate(),
            100,
            directory.path(),
            NAME,
            &progress,
            &wait,
        ));
        assert!(matches!(saved, Err(SaveFailure::Refused(_))), "{saved:?}");
        assert_eq!(server.asked(), [0, 0, 0, 0, 0]);
        assert_eq!(waits.load(Ordering::Relaxed), 4);
        assert!(names_in(directory.path()).is_empty());
    }
}
