//! What the state of a server holds of the recordings that a reader has open (ADR 0604,
//! ADR 0607): the recording with its segments, its transcript, and the lines of the
//! transcript that were read.

use std::sync::Arc;

use junto_protocol::ids::{ChannelId, RecordingId};
use junto_protocol::recording::{Recording, RecordingDetail};
use junto_protocol::transcript::{Transcript, TranscriptLine, TranscriptSpeaker};

use super::{Loadable, ReadFailure, ServerModel};

/// How many segments of one recording the state holds at most, in the recording and in
/// its transcript. A server allows 2,000 segments (ADR 0604). What passes a bound is cut,
/// so a server cannot make the state grow without a bound (ADR 0714).
pub const MOST_SEGMENTS: usize = 2_000;
/// How many speaker labels of one transcript the state holds at most. A server stores the
/// labels up to `Speaker 10000` (ADR 0607).
pub const MOST_SPEAKERS: usize = 10_000;
/// How many lines of one transcript the state holds at most. A server stores up to
/// 200,000 lines (ADR 0607); the text form of a transcript has all of them.
pub const MOST_LINES: usize = 20_000;
/// The lines of one page that a reader asks for.
pub const LINES_PAGE: usize = 200;
/// How many glossary terms of each of the two lists of a transcript the state holds at
/// most. A glossary has 200 terms (ADR 0607).
const MOST_TERMS: usize = 200;

/// The lines of the transcript of an open recording: one range without a hole, from the
/// first line of the transcript on, or around a line that a reader asked for, as far as a
/// reader asked for them.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TranscriptLines {
    /// In the order of the timeline. Each line is shared between a copy of the state and
    /// the next one.
    pub lines: Vec<Arc<TranscriptLine>>,
    /// A page has arrived.
    pub loaded: bool,
    /// The transcript has lines after the loaded ones, and the state can take them.
    pub more: bool,
    /// The transcript has lines before the loaded ones.
    pub more_before: bool,
    /// The line around which a reader asked for the lines. The lines are read around it
    /// while the state does not hold it, and again after a reload.
    pub around: Option<u32>,
    /// A page is on its way.
    pub reading: bool,
    /// Why the last read of lines failed. The next read that succeeds clears it.
    pub failed: Option<ReadFailure>,
    /// The state was replaced by a snapshot, and the lines were not read again yet.
    /// Corrections of the time between can be missing.
    pub stale: bool,
}

/// One recording that a reader has open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRecording {
    pub id: RecordingId,
    /// The recording with its segments, in the order in which they were asked for.
    pub detail: Loadable<RecordingDetail>,
    /// The transcript without its lines: its state, its speaker labels and the result for
    /// each audio segment.
    pub transcript: Loadable<Transcript>,
    pub lines: TranscriptLines,
    /// Why the last read of the recording and of its transcript failed. The next read
    /// that succeeds clears it.
    pub failed: Option<ReadFailure>,
    /// The server has no such recording for the account: it was removed, or the account
    /// may not read it any more. Nothing of it is held.
    pub gone: bool,
    /// The state was replaced by a snapshot, and the recording was not read again yet.
    pub stale: bool,
}

impl OpenRecording {
    fn new(id: RecordingId) -> Self {
        Self {
            id,
            detail: Loadable::NotLoaded,
            transcript: Loadable::NotLoaded,
            lines: TranscriptLines::default(),
            failed: None,
            gone: false,
            stale: false,
        }
    }

    /// The line at a position of the transcript, when it is loaded.
    pub fn line(&self, position: u32) -> Option<&Arc<TranscriptLine>> {
        let index = self.line_index(position)?;
        self.lines.lines.get(index)
    }

    fn line_index(&self, position: u32) -> Option<usize> {
        self.lines
            .lines
            .binary_search_by_key(&position, |held| held.line)
            .ok()
    }

    /// The speaker label with a number, when the transcript is loaded and has it.
    pub fn speaker(&self, number: u32) -> Option<&TranscriptSpeaker> {
        self.transcript
            .loaded()?
            .speakers
            .iter()
            .find(|held| held.number == number)
    }
}

/// Where the lines of one read start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LinesFrom {
    /// After this position, or from the first line of the transcript.
    After(Option<u32>),
    /// The lines nearest before this position.
    Before(u32),
    /// A page with the line at this position in its middle.
    Around(u32),
}

/// Which lines one read asks the server for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LinesAsk {
    pub(crate) from: LinesFrom,
    /// How many lines are asked for.
    pub(crate) count: usize,
    /// The lines replace the loaded ones.
    pub(crate) replace: bool,
}

/// Puts a page of the lines before the first loaded line in front of the loaded ones.
/// Each line must come before the next one and before the first loaded line; the first
/// line that does not ends the page (ADR 0714). Over the bound the state drops lines at
/// the end, which the next read after them asks for again.
fn add_earlier_lines(
    lines: &mut TranscriptLines,
    before: u32,
    page: Vec<TranscriptLine>,
    more_before: bool,
) {
    if lines.lines.first().map(|first| first.line) != Some(before) {
        return;
    }
    let mut taken: Vec<Arc<TranscriptLine>> = Vec::with_capacity(page.len());
    let mut foreign = false;
    for line in page {
        let next = taken.last().map_or(before, |held| held.line);
        if !taken.is_empty() && line.line <= next || line.line >= before {
            foreign = true;
            break;
        }
        taken.push(Arc::new(line));
    }
    let added = !taken.is_empty();
    taken.append(&mut lines.lines);
    if taken.len() > MOST_LINES {
        taken.truncate(MOST_LINES);
        lines.more = true;
    }
    lines.lines = taken;
    lines.more_before = more_before && added && !foreign;
    lines.failed = foreign.then_some(ReadFailure::NoAnswer);
}

/// The answer to a read of lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LinesRead {
    pub(crate) ask: LinesAsk,
    pub(crate) lines: Vec<TranscriptLine>,
    /// The server states that lines follow.
    pub(crate) more: bool,
    /// The server states that lines come before the page.
    pub(crate) more_before: bool,
}

impl ServerModel {
    /// A recording, while a reader has it open.
    pub fn recording(&self, recording_id: RecordingId) -> Option<&Arc<OpenRecording>> {
        self.recordings.iter().find(|held| held.id == recording_id)
    }

    pub(super) fn recording_mut(
        &mut self,
        recording_id: RecordingId,
    ) -> Option<&mut OpenRecording> {
        self.recordings
            .iter_mut()
            .find(|held| held.id == recording_id)
            .map(Arc::make_mut)
    }

    /// Starts to hold a recording for a reader.
    pub(crate) fn open_recording(&mut self, recording_id: RecordingId) {
        if self.recording(recording_id).is_none() {
            self.recordings
                .push(Arc::new(OpenRecording::new(recording_id)));
        }
    }

    pub(crate) fn close_recording(&mut self, recording_id: RecordingId) {
        self.recordings.retain(|held| held.id != recording_id);
    }

    /// Marks a recording that is not loaded as on its way.
    pub(crate) fn set_recording_reading(&mut self, recording_id: RecordingId) {
        if let Some(open) = self.recording_mut(recording_id) {
            if open.detail.loaded().is_none() {
                open.detail = Loadable::Loading;
            }
            if open.transcript.loaded().is_none() {
                open.transcript = Loadable::Loading;
            }
        }
    }

    /// Puts the answers of the routes of a recording and of its transcript into the
    /// state. False for an answer that names another recording: a server is not trusted
    /// to answer for what was asked (ADR 0714), and nothing of that answer is taken.
    pub(crate) fn set_recording(
        &mut self,
        recording_id: RecordingId,
        mut detail: RecordingDetail,
        mut transcript: Transcript,
    ) -> bool {
        if detail.recording.id != recording_id || transcript.recording_id != recording_id {
            return false;
        }
        let Some(open) = self.recording_mut(recording_id) else {
            return true;
        };
        detail.segments.truncate(MOST_SEGMENTS);
        transcript.segments.truncate(MOST_SEGMENTS);
        transcript.speakers.truncate(MOST_SPEAKERS);
        transcript.glossary.used.truncate(MOST_TERMS);
        transcript.glossary.cut.truncate(MOST_TERMS);
        // A transcript that states another number of lines than before has lines that
        // the state does not hold: a reader asks for them.
        let stated = open.transcript.loaded().map(|held| held.lines);
        let held = open.lines.lines.len();
        if stated != Some(transcript.lines)
            && usize::try_from(transcript.lines).is_ok_and(|lines| lines > held)
            && held < MOST_LINES
        {
            open.lines.more = true;
        }
        open.detail = Loadable::Loaded(detail);
        open.transcript = Loadable::Loaded(transcript);
        open.failed = None;
        open.gone = false;
        open.stale = false;
        true
    }

    /// A read of a recording failed. An answer `not_found` states that the server has no
    /// such recording for the account.
    pub(crate) fn fail_recording(&mut self, recording_id: RecordingId, failure: ReadFailure) {
        if failure == ReadFailure::NotFound {
            self.lose_recording(recording_id);
            return;
        }
        if let Some(open) = self.recording_mut(recording_id) {
            if open.detail.loaded().is_none() {
                open.detail = Loadable::NotLoaded;
            }
            if open.transcript.loaded().is_none() {
                open.transcript = Loadable::NotLoaded;
            }
            open.failed = Some(failure);
        }
    }

    /// The server has no such recording for the account. Nothing of it stays.
    pub(super) fn lose_recording(&mut self, recording_id: RecordingId) -> bool {
        let lost = OpenRecording {
            gone: true,
            ..OpenRecording::new(recording_id)
        };
        if self
            .recording(recording_id)
            .is_none_or(|open| **open == lost)
        {
            return false;
        }
        if let Some(open) = self.recording_mut(recording_id) {
            *open = lost;
        }
        true
    }

    /// A channel is gone, and its recordings with it: nobody reads them (ADR 0604).
    pub(super) fn lose_recordings_of(&mut self, channel_id: ChannelId) {
        let inside: Vec<RecordingId> = self
            .recordings
            .iter()
            .filter(|open| {
                open.detail
                    .loaded()
                    .is_some_and(|detail| detail.recording.channel_id == channel_id)
            })
            .map(|open| open.id)
            .collect();
        for recording_id in inside {
            self.lose_recording(recording_id);
        }
    }

    /// Takes the recording of an event. True when a reader has the recording open: the
    /// event carries no segment, and its recording can state another transcript, so the
    /// recording and its transcript are read from their routes.
    pub(super) fn update_open_recording(&mut self, recording: &Recording) -> bool {
        let Some(open) = self.recording(recording.id) else {
            return false;
        };
        let changed = open
            .detail
            .loaded()
            .is_some_and(|detail| detail.recording != *recording);
        if changed
            && let Some(detail) = self
                .recording_mut(recording.id)
                .and_then(|open| open.detail.loaded_mut())
        {
            detail.recording = recording.clone();
        }
        true
    }

    /// A correction carries no text; read the held lines through their authorized route.
    pub(super) fn invalidate_line(&mut self, recording_id: RecordingId, position: u32) -> bool {
        let Some(open) = self.recording_mut(recording_id).filter(|open| !open.gone) else {
            return false;
        };
        if open.line(position).is_none() {
            return false;
        }
        open.lines.stale = true;
        true
    }

    /// What a read of lines asks for now. `None` when there is nothing to read: no reader
    /// holds the recording, or the state holds every line that it can take.
    pub(crate) fn lines_ask(&self, recording_id: RecordingId) -> Option<LinesAsk> {
        let open = self.recording(recording_id).filter(|open| !open.gone)?;
        let lines = &open.lines;
        let held = lines.lines.len();
        // A line that a reader asked for and that the state does not hold, and the lines
        // around such a line after a reload, are read as one page around it.
        if let Some(line) = lines.around
            && (lines.stale || !lines.loaded || open.line(line).is_none())
        {
            return Some(LinesAsk {
                from: LinesFrom::Around(line),
                count: LINES_PAGE,
                replace: true,
            });
        }
        if lines.stale || !lines.loaded {
            // The loaded lines are read again from the first one, as many as were held.
            return Some(LinesAsk {
                from: LinesFrom::After(None),
                count: held.max(LINES_PAGE),
                replace: true,
            });
        }
        lines.more.then(|| LinesAsk {
            from: LinesFrom::After(lines.lines.last().map(|last| last.line)),
            count: LINES_PAGE.min(MOST_LINES.saturating_sub(held)),
            replace: false,
        })
    }

    /// What a read of the lines before the loaded ones asks for. `None` when the state
    /// holds the first line, or no line.
    pub(crate) fn earlier_lines_ask(&self, recording_id: RecordingId) -> Option<LinesAsk> {
        let open = self.recording(recording_id).filter(|open| !open.gone)?;
        let lines = &open.lines;
        if lines.stale || !lines.loaded || !lines.more_before {
            return None;
        }
        Some(LinesAsk {
            from: LinesFrom::Before(lines.lines.first()?.line),
            count: LINES_PAGE,
            replace: false,
        })
    }

    /// A reader asks for the lines around a line of the transcript of an open recording.
    /// True when the state does not hold the line and a read is due.
    pub(crate) fn want_lines_around(&mut self, recording_id: RecordingId, line: u32) -> bool {
        let Some(open) = self.recording_mut(recording_id).filter(|open| !open.gone) else {
            return false;
        };
        open.lines.around = Some(line);
        open.line(line).is_none()
    }

    /// Marks in the state whether lines of a recording are on their way.
    pub(crate) fn set_lines_reading(&mut self, recording_id: RecordingId, reading: bool) {
        if let Some(open) = self.recording_mut(recording_id) {
            open.lines.reading = reading;
        }
    }

    /// Puts the answer to a read of lines into the state. The page of lines names no
    /// recording, so the lines are taken as far as they continue what was asked for: each
    /// position is above the one before. The first line that is not ends the page, and
    /// the lines say that their read failed (ADR 0714). Lines that continue a list that
    /// the state does not hold any more are not taken, and neither are the lines of a
    /// recording that is gone.
    pub(crate) fn add_transcript_lines(&mut self, recording_id: RecordingId, read: LinesRead) {
        let Some(lines) = self
            .recording_mut(recording_id)
            .filter(|open| !open.gone)
            .map(|open| &mut open.lines)
        else {
            return;
        };
        lines.reading = false;
        let LinesRead {
            ask,
            lines: page,
            more,
            more_before,
        } = read;
        if let LinesFrom::Before(before) = ask.from {
            add_earlier_lines(lines, before, page, more_before);
            return;
        }
        let previous = if ask.replace {
            std::mem::take(&mut lines.lines)
        } else {
            let LinesFrom::After(after) = ask.from else {
                return;
            };
            if lines.lines.last().map(|last| last.line) != after {
                return;
            }
            Vec::new()
        };
        let mut last = match ask.from {
            LinesFrom::After(after) => after,
            LinesFrom::Before(_) | LinesFrom::Around(_) => None,
        };
        let mut added = false;
        let mut foreign = false;
        for line in page {
            if last.is_some_and(|last| line.line <= last) {
                foreign = true;
                break;
            }
            if lines.lines.len() >= MOST_LINES {
                break;
            }
            last = Some(line.line);
            let unchanged = previous
                .binary_search_by_key(&line.line, |held| held.line)
                .ok()
                .and_then(|index| previous.get(index))
                .filter(|held| ***held == line);
            lines
                .lines
                .push(unchanged.cloned().unwrap_or_else(|| Arc::new(line)));
            added = true;
        }
        lines.loaded = true;
        lines.stale = false;
        // A page that adds no line does not promise more lines: the next read would start
        // at the same line.
        lines.more = more && added && !foreign && lines.lines.len() < MOST_LINES;
        if ask.replace {
            lines.more_before = more_before && added && !foreign;
        }
        lines.failed = foreign.then_some(ReadFailure::NoAnswer);
    }

    /// A read of lines failed.
    pub(crate) fn fail_transcript_lines(
        &mut self,
        recording_id: RecordingId,
        failure: ReadFailure,
    ) {
        if failure == ReadFailure::NotFound {
            self.lose_recording(recording_id);
            return;
        }
        if let Some(open) = self.recording_mut(recording_id) {
            open.lines.reading = false;
            open.lines.failed = Some(failure);
        }
    }

    /// After a snapshot replaced the state: the recordings that readers hold stay, with
    /// the mark that they are not current. Each is read again.
    pub(crate) fn carry_recordings(
        &mut self,
        before: Option<&ServerModel>,
        wanted: &[RecordingId],
    ) {
        let before = before.filter(|before| before.account.id == self.account.id);
        for recording_id in wanted {
            let carried = before
                .and_then(|before| before.recording(*recording_id))
                .map(|held| {
                    let mut carried = held.as_ref().clone();
                    carried.stale = true;
                    carried.failed = None;
                    carried.lines.stale = carried.lines.loaded;
                    carried.lines.reading = false;
                    carried.lines.failed = None;
                    carried
                })
                .unwrap_or_else(|| OpenRecording::new(*recording_id));
            self.recordings.push(Arc::new(carried));
        }
    }
}
