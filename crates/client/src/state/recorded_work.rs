//! What the application does for the recordings of the window (ADR 0744): it holds the
//! recordings of the shown chat open in the client core, saves their files, removes a
//! recording, and shows a transcript in the side panel.

use std::path::PathBuf;

use gpui::Context;
use junto_client_core::api::ErrorCategory;
use junto_client_core::flow::FlowError;
use junto_client_core::live::LoadError;
use junto_client_core::protocol::ids::{
    ChannelId, RecordingId, RecordingSegmentId, ServerId, WorkspaceId,
};
use junto_client_core::protocol::transcript::TranscriptState;
use junto_client_core::transfers::{DownloadProgress, DownloadStage, RecordingDownload};

use super::AppState;
use super::recorded::near_recordings;
use super::view::Shown;
use crate::text;

/// How many recordings the window holds open in the client core: the ones around the
/// place where the list of the shown chat stands, and the one of the transcript panel.
const MOST_OPEN: usize = 24;

/// The file of a recording that a download saves: one segment, or the transcript as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecordedFile {
    Segment(RecordingSegmentId),
    Text,
}

/// What the window shows of the progress of a download: its stage, and the hundredth of
/// the file that has arrived. A file without a stated length counts parts of 256 KiB.
fn progress_step(
    progress: &DownloadProgress,
) -> (std::mem::Discriminant<DownloadStage>, bool, u64) {
    let part = progress
        .received
        .saturating_mul(100)
        .checked_div(progress.bytes)
        .unwrap_or(progress.received / (256 * 1024));
    (
        std::mem::discriminant(&progress.stage),
        progress.interrupted,
        part,
    )
}

impl AppState {
    /// The recording whose transcript the side panel shows, and the recordings that the
    /// cards of the shown chat name, from the first row of its list on.
    fn wanted_recordings(&self, shown: Shown) -> Vec<(ServerId, RecordingId)> {
        let Some(server_id) = shown.server else {
            return Vec::new();
        };
        let model = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model);
        let mut wanted: Vec<(ServerId, RecordingId)> = self
            .selection
            .transcript
            .filter(|(of, _, _)| *of == server_id && shown.transcript.is_some())
            .map(|(_, _, recording_id)| (server_id, recording_id))
            .into_iter()
            .collect();
        let timeline = model
            .as_ref()
            .zip(shown.place())
            .and_then(|(model, place)| model.timeline(place));
        if let Some((timeline, place)) = timeline.zip(shown.place()) {
            let named: Vec<(u64, RecordingId)> = timeline
                .messages
                .iter()
                .filter_map(|message| {
                    let system = message.system.as_ref()?;
                    (system.action == "recording_ready")
                        .then_some(system.recording_id)
                        .flatten()
                        .map(|recording_id| (message.position, recording_id))
                })
                .collect();
            // Where the list stands: at a position of the history, or at its end.
            let anchor = self.anchors.get(&(server_id, place)).copied().flatten();
            let room = MOST_OPEN.saturating_sub(wanted.len());
            for recording_id in near_recordings(&named, anchor, room) {
                if !wanted.contains(&(server_id, recording_id)) {
                    wanted.push((server_id, recording_id));
                }
            }
        }
        wanted
    }

    /// The list of the shown chat moved: the recordings around its new place are held
    /// open.
    pub fn follow_shown_recordings(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown;
        self.follow_recordings(shown, cx);
    }

    /// Holds the recordings that the window shows open in the client core, releases the
    /// others, and reads the first lines of a transcript that has some.
    pub(super) fn follow_recordings(&mut self, shown: Shown, cx: &mut Context<Self>) {
        let wanted = self.wanted_recordings(shown);
        let gone: Vec<(ServerId, RecordingId)> = self
            .open_recordings
            .keys()
            .filter(|key| !wanted.contains(key))
            .copied()
            .collect();
        for key in gone {
            self.open_recordings.remove(&key);
            // A recording that is opened again is read around its line again.
            if self
                .lines_asked_around
                .is_some_and(|(server_id, recording_id, _, _)| (server_id, recording_id) == key)
            {
                self.lines_asked_around = None;
            }
            if let Some(server) = self.client.server(key.0) {
                server.live().release_recording(key.1);
            }
        }
        for key in &wanted {
            if self.open_recordings.contains_key(key) {
                continue;
            }
            let Some(server) = self.client.server(key.0) else {
                continue;
            };
            let opening = server.live().open_recording(key.1);
            let key = *key;
            let task = cx.spawn(async move |this, cx| {
                let result = opening.await;
                let _ = this.update(cx, |this, cx| {
                    // The client core did not take the recording: the next change asks
                    // again.
                    if matches!(result, Err(LoadError::Off | LoadError::NotInState)) {
                        this.open_recordings.remove(&key);
                    }
                    cx.notify();
                });
            });
            self.open_recordings.insert(key, task);
        }
        // The lines of a transcript are read when a reader asks for them: the first page
        // of each shown transcript, once, and again after the state was replaced.
        for (server_id, recording_id) in wanted {
            let Some(server) = self.client.server(server_id) else {
                continue;
            };
            let Some(model) = server.live().view().model else {
                continue;
            };
            let Some(open) = model.recording(recording_id) else {
                continue;
            };
            let has_lines = open.transcript.loaded().is_some_and(|transcript| {
                matches!(
                    transcript.state,
                    TranscriptState::Ready | TranscriptState::Partial
                ) && transcript.lines > 0
            });
            let lines = &open.lines;
            // The transcript that the side panel opened at a line is read around that
            // line, once for each opening; the client core reads it there again after a
            // reload (ADR 0744).
            let at_line = self
                .selection
                .transcript
                .filter(|(of, _, shown)| *of == server_id && *shown == recording_id)
                .and(self.known.transcript_line);
            if let Some(line) = at_line.filter(|_| has_lines) {
                let asked = (server_id, recording_id, line, self.known.transcript_jump);
                if self.lines_asked_around != Some(asked) {
                    self.lines_asked_around = Some(asked);
                    server
                        .live()
                        .read_transcript_lines_around(recording_id, line);
                }
                continue;
            }
            if has_lines
                && (!lines.loaded || lines.stale)
                && !lines.reading
                && lines.failed.is_none()
            {
                server.live().read_transcript_lines(recording_id);
            }
        }
    }

    /// Reads the lines of a transcript after the read ones, and again after a failure.
    pub fn read_transcript_lines(&mut self, server_id: ServerId, recording_id: RecordingId) {
        if let Some(server) = self.client.server(server_id) {
            server.live().read_transcript_lines(recording_id);
        }
    }

    /// Reads the lines of a transcript before the read ones.
    pub fn read_earlier_transcript_lines(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
    ) {
        if let Some(server) = self.client.server(server_id) {
            server.live().read_earlier_transcript_lines(recording_id);
        }
    }

    /// Reads a recording again after a read that failed.
    pub fn reload_recording(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        cx: &mut Context<Self>,
    ) {
        let key = (server_id, recording_id);
        let Some(server) = self
            .client
            .server(server_id)
            .filter(|_| self.open_recordings.contains_key(&key))
        else {
            self.changed(cx);
            return;
        };
        let opening = server.live().reload_recording(recording_id);
        let task = cx.spawn(async move |this, cx| {
            let result = opening.await;
            let _ = this.update(cx, |this, cx| {
                if matches!(result, Err(LoadError::Off | LoadError::NotInState)) {
                    this.open_recordings.remove(&key);
                }
                cx.notify();
            });
        });
        self.open_recordings.insert(key, task);
        self.changed(cx);
    }

    /// Saves a file of a recording to the download directory, under a name that no file
    /// has. `stem` is the name without its ending. With `open` the saved file is opened
    /// with the application of the system for it: the video player of the system cannot
    /// read the file from the server (ADR 0608).
    pub fn save_recording_file(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        file: RecordedFile,
        stem: &str,
        open: bool,
        cx: &mut Context<Self>,
    ) {
        // A file that was saved is checked on the file system before it is opened.
        let saved = self
            .recording_downloads
            .get(&(server_id, recording_id, file))
            .map(RecordingDownload::progress)
            .filter(|_| open)
            .and_then(|progress| match progress.stage {
                DownloadStage::Saved(path) => Some((path, progress.bytes)),
                _ => None,
            });
        let stem = stem.to_owned();
        self.find_for_save(server_id, saved, cx, move |this, found, cx| {
            this.save_recording_file_found(server_id, recording_id, file, &stem, open, found, cx);
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn save_recording_file_found(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        file: RecordedFile,
        stem: &str,
        open: bool,
        found: super::messages::Found,
        cx: &mut Context<Self>,
    ) {
        // What the window holds of the downloads of a server belongs to its present owner.
        self.follow_owners();
        if !self.owners.contains_key(&server_id) {
            return;
        }
        let key = (server_id, recording_id, file);
        if let Some(held) = self.recording_downloads.get(&key) {
            let progress = held.progress();
            match (open, progress.stage) {
                // A file that was saved is opened without a second download, while the
                // path still names a file of the saved length. A file that the person
                // deleted or replaced is saved again.
                (true, DownloadStage::Saved(path)) => {
                    let kept = found
                        .saved
                        .as_ref()
                        .is_some_and(|(checked, kept)| *kept && *checked == path);
                    if kept {
                        // The task that follows the download has nothing left to open.
                        self.opening.remove(&key);
                        self.open_file(path, cx);
                        return;
                    }
                }
                (_, DownloadStage::Failed(_) | DownloadStage::Cancelled) => {}
                (_, DownloadStage::Saved(_)) => return,
                _ => {
                    if open {
                        self.opening.insert(key);
                    }
                    return;
                }
            }
        }
        let (Some(server), Some(directory)) = (self.client.server(server_id), found.directory)
        else {
            self.known.recording_refused.insert(
                (server_id, recording_id),
                text::NO_DOWNLOAD_DIRECTORY.to_owned(),
            );
            self.changed(cx);
            return;
        };
        let transfers = server.transfers();
        let download = match file {
            RecordedFile::Text => {
                Some(transfers.download_transcript_text(recording_id, stem, directory))
            }
            RecordedFile::Segment(segment_id) => server
                .live()
                .view()
                .model
                .and_then(|model| {
                    let open = model.recording(recording_id)?;
                    let detail = open.detail.loaded()?;
                    detail
                        .segments
                        .iter()
                        .find(|segment| segment.id == segment_id)
                        .cloned()
                })
                .and_then(|segment| {
                    transfers.download_segment(recording_id, &segment, stem, directory)
                }),
        };
        let Some(download) = download else {
            return;
        };
        if open {
            self.opening.insert(key);
        } else {
            self.opening.remove(&key);
        }
        self.known
            .recording_refused
            .remove(&(server_id, recording_id));
        self.watch_recording_download(key, &download, cx);
        self.recording_downloads.insert(key, download);
        self.show_recording_downloads();
        self.changed(cx);
    }

    fn watch_recording_download(
        &mut self,
        key: (ServerId, RecordingId, RecordedFile),
        download: &RecordingDownload,
        cx: &mut Context<Self>,
    ) {
        // A later download of the same file has another number: the task of this one
        // changes nothing then.
        self.download_asks += 1;
        let number = self.download_asks;
        self.recording_download_numbers.insert(key, number);
        let mut watch = download.watch();
        cx.spawn(async move |this, cx| {
            let mut told_step = None;
            let mut closed = false;
            loop {
                // The stage of this moment is read before the first wait: a download
                // can end before this task runs.
                let progress = watch.borrow_and_update().clone();
                let ended = matches!(
                    progress.stage,
                    DownloadStage::Saved(_) | DownloadStage::Failed(_) | DownloadStage::Cancelled
                );
                // The window is told a change of the stage and each further hundredth
                // of the file, not each part that arrives.
                let step = progress_step(&progress);
                if told_step != Some(step) {
                    told_step = Some(step);
                    let told = this.update(cx, |this, cx| {
                        if this.recording_download_numbers.get(&key) != Some(&number) {
                            return false;
                        }
                        if ended
                            && this.opening.remove(&key)
                            && let DownloadStage::Saved(path) = progress.stage
                        {
                            this.open_file(path, cx);
                        }
                        this.show_recording_downloads();
                        this.changed(cx);
                        true
                    });
                    if !told.unwrap_or(false) {
                        break;
                    }
                }
                if ended || closed {
                    break;
                }
                closed = watch.changed().await.is_err();
            }
        })
        .detach();
    }

    /// Ends a download of a file of a recording, or puts away what the window shows of
    /// one that ended.
    pub fn forget_recording_download(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        file: RecordedFile,
        cx: &mut Context<Self>,
    ) {
        let key = (server_id, recording_id, file);
        if let Some(download) = self.recording_downloads.remove(&key) {
            download.cancel();
        }
        self.recording_download_numbers.remove(&key);
        self.opening.remove(&key);
        self.show_recording_downloads();
        self.changed(cx);
    }

    /// Ends the downloads of files of recordings of a server and puts away what the
    /// window shows of them: the server has another owner than the one that started them
    /// (ADR 0745).
    pub(super) fn forget_recording_downloads_of(&mut self, server_id: ServerId) {
        self.recording_downloads.retain(|key, download| {
            if key.0 == server_id {
                download.cancel();
            }
            key.0 != server_id
        });
        self.recording_download_numbers
            .retain(|key, _| key.0 != server_id);
        self.opening.retain(|key| key.0 != server_id);
        self.show_recording_downloads();
    }

    /// Puts the progress of every download of a file of a recording into what the view
    /// is derived from.
    fn show_recording_downloads(&mut self) {
        self.known.segment_downloads.clear();
        self.known.text_downloads.clear();
        for ((server_id, recording_id, file), download) in &self.recording_downloads {
            match file {
                RecordedFile::Segment(segment_id) => {
                    self.known
                        .segment_downloads
                        .insert((*server_id, *segment_id), download.progress());
                }
                RecordedFile::Text => {
                    self.known
                        .text_downloads
                        .insert((*server_id, *recording_id), download.progress());
                }
            }
        }
    }

    /// Opens a saved file with the application of the system for it. A start of the
    /// application state without that, such as a test, keeps the path.
    fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.opens_files {
            cx.open_with_system(&path);
        } else {
            self.kept_opened.push(path);
        }
    }

    /// Lets the application open saved files with the applications of the system.
    pub fn set_opens_files(&mut self, opens: bool) {
        self.opens_files = opens;
    }

    /// The files that a start without the applications of the system would have opened.
    pub fn kept_opened(&self) -> &[PathBuf] {
        &self.kept_opened
    }

    /// Removes a recording with its files and its transcript. The roles of the person
    /// predict the permission "manage recordings"; the server decides. The card leaves
    /// the chat when the event of the server was followed.
    pub fn remove_recording(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        // Archived channels are read-only even when an old confirmation remains.
        // For an active channel the server decides permission and the card tells its
        // refusal, including when roles changed after the control was drawn.
        let active = server.live().view().model.is_some_and(|model| {
            model
                .recording(recording_id)
                .and_then(|open| open.detail.loaded())
                .is_some_and(|detail| model.channel(detail.recording.channel_id).is_some())
        });
        if !active {
            return;
        }
        let key = (server_id, recording_id);
        if !self.known.recordings_removing.insert(key) {
            return;
        }
        self.known.recording_refused.remove(&key);
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let removed = api.remove_recording(recording_id).await;
            let _ = this.update(cx, |this, cx| {
                this.known.recordings_removing.remove(&key);
                if let Err(error) = removed {
                    let reason = match error.category() {
                        Some(ErrorCategory::NotAllowed) => {
                            text::RECORDING_REMOVE_NOT_ALLOWED.to_owned()
                        }
                        Some(ErrorCategory::Conflict) => {
                            text::RECORDING_REMOVE_TOO_EARLY.to_owned()
                        }
                        // The recording is gone already: it is read again, and its
                        // card says so.
                        Some(ErrorCategory::NotFound) => {
                            this.reload_recording(server_id, recording_id, cx);
                            String::new()
                        }
                        _ => text::flow_error(&FlowError::from(&error)),
                    };
                    if !reason.is_empty() {
                        this.known.recording_refused.insert(key, reason);
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
        self.changed(cx);
    }

    pub fn dismiss_recording_refusal(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        cx: &mut Context<Self>,
    ) {
        self.known
            .recording_refused
            .remove(&(server_id, recording_id));
        self.changed(cx);
    }

    /// Shows the transcript of a recording of a voice channel in the side panel, at a
    /// line when one is named. The voice channel is shown in the main area.
    pub fn open_transcript(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        recording_id: RecordingId,
        line: Option<u32>,
        cx: &mut Context<Self>,
    ) {
        if self.shown.server != Some(server_id) || self.shown.channel != Some(channel_id) {
            self.end_jump();
            self.navigations += 1;
            self.selection.server = Some(server_id);
            self.selection.workspaces.insert(server_id, workspace_id);
            self.selection
                .channels
                .insert((server_id, workspace_id), channel_id);
            self.selection.conversations.remove(&server_id);
        }
        self.selection.transcript = Some((server_id, channel_id, recording_id));
        self.selection.thread = None;
        self.selection.pins = None;
        self.known.fork = None;
        self.known.transcript_line = line;
        // Each opening at a line moves the list of the panel once.
        if line.is_some() {
            self.known.transcript_jump += 1;
        }
        self.changed(cx);
    }

    pub fn close_transcript(&mut self, cx: &mut Context<Self>) {
        self.selection.transcript = None;
        self.known.transcript_line = None;
        self.changed(cx);
    }

    /// Draws these downloads and this state of a removal in place of what the transfers
    /// of the client tell. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_recordings_fixture(
        &mut self,
        change: impl FnOnce(&mut super::view::Known),
        cx: &mut Context<Self>,
    ) {
        change(&mut self.known);
        self.changed(cx);
    }
}
