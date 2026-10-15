//! The recordings in the stream client: the recordings that a reader has open, the reads
//! of their two routes, and the pages of the lines of their transcripts.
//!
//! `recording_updated` carries a recording without its segments and without its
//! transcript, so the client reads both routes after it. Transcript events carry only
//! identifiers; changed content is read through its authorized route.

use std::collections::BTreeMap;
use std::sync::Arc;

use junto_protocol::ids::RecordingId;
use junto_protocol::recording::RecordingDetail;
use junto_protocol::transcript::{Transcript, TranscriptLineQuery};
use tokio::sync::oneshot;

use super::{Client, Load};
use crate::api::{ApiError, ServerApi};
use crate::live::{Command, LoadError};
use crate::model::{LinesAsk, LinesFrom, LinesRead, Part, ReadFailure, ServerModel};

/// The most lines that one request asks for. The server gives at most 500 (ADR 0607).
const LARGEST_PAGE: usize = 500;

type Reply = oneshot::Sender<Result<(), LoadError>>;

#[derive(Default)]
pub(super) struct Work {
    /// The recordings that a reader has open, with the number of their readers.
    open: BTreeMap<RecordingId, usize>,
    /// The readers that wait for a recording.
    opening: BTreeMap<RecordingId, Vec<Reply>>,
    /// The recordings whose lines before the loaded ones a reader asked for.
    earlier: std::collections::BTreeSet<RecordingId>,
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

/// The lines that `ask` names, in as many requests as their number needs. A read before
/// a line or around it is one page.
pub(super) async fn read_lines(
    api: &ServerApi,
    recording_id: RecordingId,
    ask: LinesAsk,
) -> Result<LinesRead, ApiError> {
    let limit = |count: usize| Some(u32::try_from(count.min(LARGEST_PAGE)).unwrap_or(u32::MAX));
    let one_page = |query: TranscriptLineQuery| async move {
        let page = api.transcript_lines_at(recording_id, &query).await?;
        Ok(LinesRead {
            ask,
            more: page.next_after.is_some(),
            more_before: page.previous_before.is_some(),
            lines: page.lines,
        })
    };
    let mut after = match ask.from {
        LinesFrom::After(after) => after,
        LinesFrom::Before(before) => {
            return one_page(TranscriptLineQuery {
                before: Some(before),
                limit: limit(ask.count),
                ..TranscriptLineQuery::default()
            })
            .await;
        }
        LinesFrom::Around(around) => {
            return one_page(TranscriptLineQuery {
                around: Some(around),
                limit: limit(ask.count),
                ..TranscriptLineQuery::default()
            })
            .await;
        }
    };
    let mut lines = Vec::new();
    let mut more = false;
    let mut more_before = false;
    for page_number in 0..ask.count.div_ceil(LARGEST_PAGE) {
        let wanted = ask.count.saturating_sub(lines.len());
        let page = api
            .transcript_lines(recording_id, after, limit(wanted))
            .await?;
        more = page.next_after.is_some();
        if page_number == 0 {
            more_before = page.previous_before.is_some();
        }
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
    Ok(LinesRead {
        ask,
        lines,
        more,
        more_before,
    })
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
            Command::EarlierTranscriptLines(recording_id)
                if self.recordings.wants(recording_id) && !self.stale =>
            {
                self.recordings.earlier.insert(recording_id);
                self.read(Part::TranscriptLines(recording_id));
            }
            Command::TranscriptLinesAround { recording_id, line }
                if self.recordings.wants(recording_id) && !self.stale =>
            {
                let due = self
                    .model
                    .as_mut()
                    .is_some_and(|model| model.want_lines_around(recording_id, line));
                self.model_changed = true;
                if due {
                    self.read(Part::TranscriptLines(recording_id));
                }
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
        self.recordings.earlier.remove(&recording_id);
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
        // A read that replaces the lines comes first; the lines before the loaded ones are
        // asked for when none is due.
        let ask = match model.lines_ask(recording_id) {
            Some(ask) if ask.replace => ask,
            due => match self.recordings.earlier.remove(&recording_id) {
                true => model.earlier_lines_ask(recording_id).or(due)?,
                false => due?,
            },
        };
        model.set_lines_reading(recording_id, true);
        self.model_changed = true;
        Some(ask)
    }

    /// After an answer of lines: a read that a reader asked for while that read was on its
    /// way is due now. A read around a line that was just read around that line is not
    /// repeated: the transcript has no such line.
    pub(super) fn lines_due_after(&mut self, recording_id: RecordingId, answered: LinesAsk) {
        let Some(model) = &self.model else {
            return;
        };
        let around = model
            .lines_ask(recording_id)
            .filter(|ask| matches!(ask.from, LinesFrom::Around(_)) && ask.from != answered.from);
        if around.is_some() || self.recordings.earlier.contains(&recording_id) {
            self.read(Part::TranscriptLines(recording_id));
        }
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
