//! The recordings in the stream client: the recordings that a reader has open, the reads
//! of their two routes, and the pages of the lines of their transcripts.
//!
//! `recording_updated` carries a recording without its segments and without its
//! transcript, so the client reads both routes after it. The events of a transcript carry
//! what they change, and cost no request.

use std::collections::BTreeMap;
use std::sync::Arc;

use junto_protocol::ids::RecordingId;
use junto_protocol::recording::RecordingDetail;
use junto_protocol::transcript::Transcript;
use tokio::sync::oneshot;

use super::{Client, Load};
use crate::api::{ApiError, ServerApi};
use crate::live::{Command, LoadError};
use crate::model::{LinesAsk, LinesRead, Part, ReadFailure, ServerModel};

/// The most lines that one request asks for. The server gives at most 500 (ADR 0607).
const LARGEST_PAGE: usize = 500;

type Reply = oneshot::Sender<Result<(), LoadError>>;

#[derive(Default)]
pub(super) struct Work {
    /// The recordings that a reader has open, with the number of their readers.
    open: BTreeMap<RecordingId, usize>,
    /// The readers that wait for a recording.
    opening: BTreeMap<RecordingId, Vec<Reply>>,
}

impl Work {
    pub(super) fn wants(&self, recording_id: RecordingId) -> bool {
        self.open.contains_key(&recording_id)
    }
}

/// The recording and its transcript, each from its route.
pub(super) async fn read_recording(
    api: &ServerApi,
    recording_id: RecordingId,
) -> Result<(RecordingDetail, Transcript), ApiError> {
    futures_util::future::try_join(api.recording(recording_id), api.transcript(recording_id)).await
}

/// The lines that `ask` names, in as many requests as their number needs.
pub(super) async fn read_lines(
    api: &ServerApi,
    recording_id: RecordingId,
    ask: LinesAsk,
) -> Result<LinesRead, ApiError> {
    let mut lines = Vec::new();
    let mut after = ask.after;
    let mut more = false;
    for _ in 0..ask.count.div_ceil(LARGEST_PAGE) {
        let wanted = ask.count.saturating_sub(lines.len()).min(LARGEST_PAGE);
        let limit = u32::try_from(wanted).unwrap_or(u32::MAX);
        let page = api
            .transcript_lines(recording_id, after, Some(limit))
            .await?;
        more = page.next_after.is_some();
        // The next request starts after the last line that arrived. A page that does not
        // go on from where it was asked to start ends the read.
        let last = page.lines.last().map(|line| line.line);
        let moved = last.is_some_and(|last| after.is_none_or(|after| last > after));
        lines.extend(page.lines);
        if !more || !moved || lines.len() >= ask.count {
            break;
        }
        after = last;
    }
    Ok(LinesRead { ask, lines, more })
}

impl Client {
    /// The account and its state are gone: no recording stays open.
    pub(super) fn forget_recordings(&mut self) {
        let work = std::mem::take(&mut self.recordings);
        for reply in work.opening.into_values().flatten() {
            let _ = reply.send(Err(LoadError::Off));
        }
    }

    /// Takes a command of a reader about a recording.
    pub(super) fn recording_command(&mut self, command: Command) {
        match command {
            Command::OpenRecording {
                recording_id,
                reply,
            } => self.open_recording(recording_id, reply),
            Command::ReleaseRecording(recording_id) => self.release_recording(recording_id),
            Command::MoreTranscriptLines(recording_id)
                if self.recordings.wants(recording_id) && !self.stale =>
            {
                self.read(Part::TranscriptLines(recording_id));
            }
            _ => {}
        }
    }

    fn open_recording(&mut self, recording_id: RecordingId, reply: Reply) {
        if self.target.is_none() {
            let _ = reply.send(Err(LoadError::Off));
            return;
        }
        let ready = match &mut self.model {
            Some(model) if !self.stale => {
                model.open_recording(recording_id);
                self.model_changed = true;
                Some(model.recording(recording_id).is_some_and(|open| {
                    open.detail.loaded().is_some()
                        && !open.stale
                        && !open.gone
                        && open.failed.is_none()
                }))
            }
            // The recording is read when the state has arrived.
            _ => None,
        };
        *self.recordings.open.entry(recording_id).or_default() += 1;
        match ready {
            Some(true) => self.answers.push((reply, Ok(()))),
            Some(false) => {
                self.recordings
                    .opening
                    .entry(recording_id)
                    .or_default()
                    .push(reply);
                self.read(Part::Recording(recording_id));
            }
            None => self
                .recordings
                .opening
                .entry(recording_id)
                .or_default()
                .push(reply),
        }
    }

    fn release_recording(&mut self, recording_id: RecordingId) {
        let Some(readers) = self.recordings.open.get_mut(&recording_id) else {
            return;
        };
        *readers = readers.saturating_sub(1);
        if *readers > 0 {
            return;
        }
        self.recordings.open.remove(&recording_id);
        self.recordings.opening.remove(&recording_id);
        let of_recording = |part: &Part| matches!(part, Part::Recording(of) | Part::TranscriptLines(of) if *of == recording_id);
        self.messages.drop_retries(of_recording);
        self.messages.drop_queued(of_recording);
        self.resume.retain(|part| !of_recording(part));
        self.pending
            .retain(|pending| !matches!(&pending.load, Load::Part(part) if of_recording(part)));
        if let Some(model) = &mut self.model {
            model.close_recording(recording_id);
            self.model_changed = true;
        }
    }

    /// What a read of lines asks for now, with the mark in the state that it is on its
    /// way. `None` when there is nothing to read.
    pub(super) fn lines_ask(&mut self, recording_id: RecordingId) -> Option<LinesAsk> {
        let model = self.model.as_mut()?;
        let ask = model.lines_ask(recording_id)?;
        model.set_lines_reading(recording_id, true);
        self.model_changed = true;
        Some(ask)
    }

    pub(super) fn answer_recording(
        &mut self,
        recording_id: RecordingId,
        answer: impl Fn() -> Result<(), LoadError>,
    ) {
        let waiting = self.recordings.opening.remove(&recording_id);
        for waiting in waiting.unwrap_or_default() {
            self.answers.push((waiting, answer()));
        }
    }

    /// A read of a recording ended without the recording. The state tells why, or that
    /// the server has no such recording for the account, and the readers that wait get
    /// the error.
    pub(super) fn recording_unread(&mut self, recording_id: RecordingId, error: ApiError) {
        if let Some(model) = &mut self.model {
            model.fail_recording(recording_id, ReadFailure::of(&error));
            self.model_changed = true;
        }
        let error = Arc::new(error);
        self.answer_recording(recording_id, || Err(LoadError::Request(error.clone())));
    }

    /// The answer names another recording than the one that was asked for (ADR 0714). It
    /// is not used, and the recording is not asked for again.
    pub(super) fn recording_answered_wrongly(&mut self, recording_id: RecordingId) {
        self.recording_unread(
            recording_id,
            ApiError::InvalidResponse {
                status: 200,
                reason: "another recording than the one that was asked for",
            },
        );
    }

    /// After a snapshot: the recordings that readers hold stay until their routes are
    /// read again, and each is read again, with the lines that were loaded.
    pub(super) fn carry_recordings(&mut self, before: Option<&ServerModel>) {
        let wanted: Vec<RecordingId> = self.recordings.open.keys().copied().collect();
        let Some(model) = &mut self.model else {
            return;
        };
        model.carry_recordings(before, &wanted);
        let with_lines: Vec<RecordingId> = wanted
            .iter()
            .copied()
            .filter(|recording_id| {
                model
                    .recording(*recording_id)
                    .is_some_and(|open| open.lines.stale)
            })
            .collect();
        for recording_id in wanted {
            self.read(Part::Recording(recording_id));
        }
        for recording_id in with_lines {
            self.read(Part::TranscriptLines(recording_id));
        }
    }
}
