//! A finished recording in the chat of its voice channel (ADR 0744, view G of the design):
//! what the card of the recording shows, derived from the recording that the client core
//! holds open (ADR 0723).

use std::collections::HashMap;
use std::time::Duration;

use junto_client_core::model::{Loadable, OpenRecording, ReadFailure, TranscriptLines};
use junto_client_core::protocol::ids::{
    AccountId, ChannelId, RecordingId, RecordingSegmentId, ServerId,
};
use junto_client_core::protocol::recording::{
    FileCheck, RecordingDetail, RecordingFailure, RecordingSegment, RecordingState, SegmentSource,
    SegmentState,
};
use junto_client_core::protocol::transcript::{
    LineSource, SegmentTranscription, SpeakerSeparation, TranscriptFailure, TranscriptLine,
    TranscriptState,
};
use junto_client_core::transfers::DownloadProgress;
use time::{Date, OffsetDateTime};

use super::feed::Zone;
use super::transcript_work::TranscriptAsk;
use crate::text;

/// How many lines of its transcript a card shows. The side panel shows all of them.
pub const SHOWN_LINES: usize = 6;

/// What a file of a recording holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// The video of the call.
    Video,
    /// The microphone of one person.
    Microphone,
    /// The sound of the screen share of one person.
    ShareSound,
}

/// One file of a recording that the person can save.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub segment_id: RecordingSegmentId,
    pub kind: FileKind,
    /// The person of an audio track. Absent for the video.
    pub person: Option<String>,
    pub bytes: u64,
    /// The name under which the file is saved, without its ending.
    pub stem: String,
    /// What the check of the server found wrong with the file, in words.
    pub problem: Option<String>,
    pub download: Option<DownloadProgress>,
}

/// The state of the job of a recording (ADR 0601).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobShown {
    Starting,
    Recording,
    /// The server checks the files and joins the video.
    Processing,
    Ready,
    /// A part was saved, and a part was not.
    PartlyFailed {
        failed: u32,
        segments: u32,
    },
    /// Nothing was saved.
    Failed(Option<RecordingFailure>),
}

/// The zone and the day by which the window states the time of a change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct When {
    pub zone: Zone,
    pub today: Option<Date>,
}

impl When {
    /// A moment as the window states it: the time of day, and the day.
    fn shown(self, at: OffsetDateTime) -> String {
        let local = self.zone.local(at);
        format!(
            "{} {:02}:{:02}",
            text::day_or_today(local.date(), self.today),
            local.hour(),
            local.minute()
        )
    }
}

/// Who made a change of a transcript, and when (ADR 0603).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Changed {
    pub by: String,
    pub at: String,
}

fn changed(
    by: Option<AccountId>,
    at: Option<OffsetDateTime>,
    names: &HashMap<AccountId, String>,
    when: When,
) -> Option<Changed> {
    let by = by.map(|by| name_of(by, names));
    match (by, at) {
        (None, None) => None,
        (by, at) => Some(Changed {
            by: by.unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned()),
            at: at.map(|at| when.shown(at)).unwrap_or_default(),
        }),
    }
}

/// Where the sound of a transcript line comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineFrom {
    Microphone,
    /// The sound of the screen share of this person.
    Shared {
        of: String,
    },
}

/// One line of a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRow {
    pub line: u32,
    /// The time from the start of the recording, as `00:11:02`.
    pub time: String,
    /// The person of the microphone, or the person who shared the sound.
    pub account_id: AccountId,
    /// The name of the speaker: the person, the name that a voice was given, or
    /// `Speaker N`.
    pub name: String,
    /// The label of a voice that has a name: `Speaker 1`.
    pub was: Option<String>,
    /// The number of the voice in shared sound.
    pub voice: Option<u32>,
    pub source: LineFrom,
    pub text: String,
    /// A person corrected the text of the line.
    pub corrected: bool,
    /// The text that the worker wrote, when a person corrected the line.
    pub original: Option<String>,
    /// Who made the newest correction, and when.
    pub changed: Option<Changed>,
}

/// One voice in the shared sound of a person (ADR 0614).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceRow {
    pub number: u32,
    /// `Speaker N`.
    pub label: String,
    /// The name that a person gave the voice.
    pub name: Option<String>,
    /// The person whose shared sound holds the voice.
    pub of: String,
    /// Who gave the name, and when.
    pub changed: Option<Changed>,
}

/// What the transcript holds of one audio file of the recording (ADR 0611).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentResult {
    /// The file: the microphone or the shared sound of a person.
    pub title: String,
    pub sentence: String,
    /// The file was transcribed in part or not at all, or lines have no voice.
    pub failed: bool,
}

/// One part of a recording: a file of the composite or of one track, with the time range
/// that the recorder reported for it (ADR 0601).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartRow {
    pub title: String,
    pub state: String,
    /// The times of day of its start and of its end. Absent when the recorder reported
    /// neither.
    pub span: Option<String>,
    pub failed: bool,
}

/// The transcript of a recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptShown {
    pub state: TranscriptState,
    pub failure: Option<TranscriptFailure>,
    /// The number of lines that the transcript has.
    pub total: u32,
    pub language: String,
    /// How many lines were read.
    pub held: usize,
    /// The first lines that were read, at most [`SHOWN_LINES`].
    pub lines: Vec<LineRow>,
    /// The transcript has lines after the read ones.
    pub more: bool,
    /// The transcript has lines before the read ones: the panel was opened at a line.
    pub more_before: bool,
    pub reading: bool,
    /// The last read of lines or transcript metadata failed.
    pub failed: bool,
    pub refresh_failed: bool,
    /// The voices of shared sound, in the order of their numbers.
    pub voices: Vec<VoiceRow>,
    /// The glossary terms that the speech model received, and those that did not fit in
    /// its limit (ADR 0613).
    pub used: Vec<String>,
    pub cut: Vec<String>,
    /// The result of each audio file.
    pub segments: Vec<SegmentResult>,
    /// The person can correct a line and name a voice: the channel is not archived. The
    /// server decides (ADR 0603).
    pub may_change: bool,
    /// The correction or the name that the person sent, while it is on its way or after
    /// the server refused it.
    pub ask: Option<TranscriptAsk>,
}

/// What the derivation of a transcript reads beside the recording.
#[derive(Clone, Copy)]
pub struct TranscriptSource<'a> {
    pub names: &'a HashMap<AccountId, String>,
    pub when: When,
    pub may_change: bool,
    pub ask: Option<&'a TranscriptAsk>,
}

/// A recording as its card shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardDetail {
    /// The name of the voice channel.
    pub channel: String,
    /// The day of the recording in words.
    pub day: String,
    /// The times of day at which the recording started and ended.
    pub span: Option<(String, String)>,
    pub length: Option<Duration>,
    pub started_by: Option<String>,
    /// The size of the picture of the video.
    pub picture: Option<(u32, u32)>,
    /// The saved microphone files, and the saved files of shared sound.
    pub microphones: usize,
    pub share_sounds: usize,
    pub bytes: Option<u64>,
    pub job: JobShown,
    pub files: Vec<FileRow>,
    /// Every part of the recording with its time range.
    pub parts: Vec<PartRow>,
    /// The video that "Open" plays: the first video file.
    pub video: Option<RecordingSegmentId>,
    pub transcript: TranscriptShown,
    /// The name under which the transcript is saved as text, without its ending.
    pub text_stem: String,
    pub text_download: Option<DownloadProgress>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CardState {
    Reading,
    /// The recording could not be read.
    Failed,
    /// The server has no such recording for the account: it was removed, or the account
    /// may not read it any more.
    Gone,
    Shown(Box<CardDetail>),
}

/// The card of one recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordingCard {
    pub server_id: ServerId,
    pub recording_id: RecordingId,
    pub state: CardState,
    /// The roles of the person allow the removal: "manage recordings". The server
    /// decides.
    pub may_remove: bool,
    /// The removal is on its way.
    pub removing: bool,
    /// The person can ask for the transcript again: the files are processed, and the
    /// recording has no transcript or a failed one (ADR 0607).
    pub request: TranscriptRequest,
    /// Why the server did not do what the person asked for with the recording.
    pub refused: Option<String>,
}

/// Whether a card offers a new request for its transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRequest {
    /// The recording has a transcript, one is on its way, or its files are not processed.
    NotOffered,
    /// The roles of the person do not hold "manage recordings", or the channel is
    /// archived.
    NotAllowed,
    Allowed {
        /// The request is on its way.
        sending: bool,
    },
}

/// What the derivation of a card reads.
pub struct CardSource<'a> {
    pub server_id: ServerId,
    pub recording_id: RecordingId,
    /// The recording as the client core holds it. Absent until the window opened it.
    pub open: Option<&'a OpenRecording>,
    /// The name of the voice channel.
    pub channel: &'a str,
    /// The voice channel whose chat shows the card.
    pub channel_id: ChannelId,
    pub names: &'a HashMap<AccountId, String>,
    pub zone: Zone,
    pub today: Option<Date>,
    pub may_remove: bool,
    /// The roles of the person allow a new request for a transcript, and the channel is
    /// not archived.
    pub may_request: bool,
    pub downloads: &'a HashMap<(ServerId, RecordingSegmentId), DownloadProgress>,
    /// The downloads of transcripts as text.
    pub texts: &'a HashMap<(ServerId, RecordingId), DownloadProgress>,
    pub removing: bool,
    /// The new request for the transcript is on its way.
    pub requesting: bool,
    pub refused: Option<&'a str>,
}

fn name_of(account_id: AccountId, names: &HashMap<AccountId, String>) -> String {
    names
        .get(&account_id)
        .cloned()
        .unwrap_or_else(|| text::UNKNOWN_PERSON.to_owned())
}

/// The recordings that the window holds open, out of the ones that the loaded messages
/// of the shown chat name with the positions of their messages, in the order of the
/// chat.
pub fn near_recordings(
    named: &[(u64, RecordingId)],
    anchor: Option<u64>,
    most: usize,
) -> Vec<RecordingId> {
    // A list at its end shows the newest messages. A list elsewhere shows the messages
    // from its first row on: the recordings from there on come first, three quarters of
    // the count at most, then the ones before, the nearest first, for a list that moves
    // towards them.
    let first = match anchor {
        None => named.len(),
        Some(position) => named.partition_point(|(at, _)| *at < position),
    };
    let (before, after) = named.split_at(first);
    let ahead = after.len().min(most - most / 4);
    let order = after[..ahead]
        .iter()
        .chain(before.iter().rev())
        .chain(after[ahead..].iter());
    let mut near: Vec<RecordingId> = Vec::with_capacity(most.min(named.len()));
    for (_, recording_id) in order {
        if near.len() >= most {
            break;
        }
        if !near.contains(recording_id) {
            near.push(*recording_id);
        }
    }
    near
}

/// One line of a transcript with the name of its speaker.
pub fn line_row(
    open: &OpenRecording,
    line: &TranscriptLine,
    names: &HashMap<AccountId, String>,
    when: When,
) -> LineRow {
    let person = name_of(line.account_id, names);
    let voice = line.speaker.filter(|_| line.source == LineSource::Shared);
    let label = voice.map(text::speaker_label);
    let given = voice
        .and_then(|number| open.speaker(number))
        .and_then(|speaker| speaker.name.clone());
    let (name, was) = match (line.source, given, label) {
        (LineSource::Microphone, _, _) | (LineSource::Shared, None, None) => (person.clone(), None),
        (LineSource::Shared, Some(given), label) => (given, label),
        (LineSource::Shared, None, Some(label)) => (label, None),
    };
    LineRow {
        line: line.line,
        time: text::clock_of(line.start_ms),
        account_id: line.account_id,
        name,
        was,
        voice,
        source: match line.source {
            LineSource::Microphone => LineFrom::Microphone,
            LineSource::Shared => LineFrom::Shared { of: person },
        },
        text: line.text.clone(),
        corrected: line.original_text.is_some(),
        original: line.original_text.clone(),
        changed: line
            .original_text
            .as_ref()
            .and_then(|_| changed(line.corrected_by, line.corrected_at, names, when)),
    }
}

/// The first lines of a transcript, as a card shows them.
fn first_lines(
    open: &OpenRecording,
    held: &TranscriptLines,
    names: &HashMap<AccountId, String>,
    when: When,
) -> Vec<LineRow> {
    held.lines
        .iter()
        .take(SHOWN_LINES)
        .map(|line| line_row(open, line, names, when))
        .collect()
}

/// The title of an audio file of a recording, by its person.
fn segment_title(segment: Option<&RecordingSegment>, names: &HashMap<AccountId, String>) -> String {
    let person = segment
        .and_then(|segment| segment.account_id)
        .map(|account_id| name_of(account_id, names));
    match segment.map(|segment| segment.source) {
        Some(SegmentSource::Composite) => text::RECORDING_VIDEO.to_owned(),
        Some(SegmentSource::ScreenShareAudio) => text::recording_share_sound(person.as_deref()),
        Some(SegmentSource::Microphone) => text::recording_microphone(person.as_deref()),
        None => text::transcripts::UNKNOWN_FILE.to_owned(),
    }
}

/// The result of each audio file of a transcript.
fn segment_results(
    open: &OpenRecording,
    transcript: &junto_client_core::protocol::transcript::Transcript,
    names: &HashMap<AccountId, String>,
) -> Vec<SegmentResult> {
    let segments = open
        .detail
        .loaded()
        .map(|detail| detail.segments.as_slice());
    transcript
        .segments
        .iter()
        .map(|result| {
            let segment = segments
                .and_then(|segments| segments.iter().find(|held| held.id == result.segment_id));
            let lines_missing_voice = matches!(
                result.speakers,
                SpeakerSeparation::Unassigned | SpeakerSeparation::Failed
            );
            SegmentResult {
                title: segment_title(segment, names),
                sentence: text::transcripts::segment_result(result),
                failed: !matches!(
                    result.status,
                    SegmentTranscription::Ok | SegmentTranscription::Empty
                ) || lines_missing_voice,
            }
        })
        .collect()
}

/// The voices of shared sound of a transcript.
fn voices(
    transcript: &junto_client_core::protocol::transcript::Transcript,
    names: &HashMap<AccountId, String>,
    when: When,
) -> Vec<VoiceRow> {
    let mut voices: Vec<VoiceRow> = transcript
        .speakers
        .iter()
        .map(|speaker| VoiceRow {
            number: speaker.number,
            label: text::speaker_label(speaker.number),
            name: speaker.name.clone(),
            of: name_of(speaker.account_id, names),
            changed: speaker
                .name
                .as_ref()
                .and_then(|_| changed(speaker.renamed_by, speaker.renamed_at, names, when)),
        })
        .collect();
    voices.sort_by_key(|voice| voice.number);
    voices
}

/// Every part of a recording, in the order in which they were asked for, with the time
/// range that the recorder reported.
fn parts(detail: &RecordingDetail, names: &HashMap<AccountId, String>, zone: Zone) -> Vec<PartRow> {
    let clock = |at: OffsetDateTime| {
        let at = zone.local(at);
        format!("{:02}:{:02}:{:02}", at.hour(), at.minute(), at.second())
    };
    detail
        .segments
        .iter()
        .map(|segment| {
            let span = match (segment.started_at, segment.ended_at) {
                (None, None) => None,
                (started, ended) => Some(text::transcripts::part_span(
                    started.map(clock).as_deref(),
                    ended.map(clock).as_deref(),
                )),
            };
            let failed = segment.state == SegmentState::Failed
                || segment
                    .file
                    .as_ref()
                    .is_some_and(|file| file.check == FileCheck::Failed)
                || (segment.state == SegmentState::Ended
                    && segment.file.is_none()
                    && detail.recording.bytes.is_some());
            PartRow {
                title: segment_title(Some(segment), names),
                state: text::transcripts::part_state(segment, detail.recording.bytes.is_some()),
                span,
                failed,
            }
        })
        .collect()
}

/// The transcript of an open recording as the card and the side panel show it, with
/// its first lines. The side panel derives each further line when it draws it.
pub fn transcript(open: &OpenRecording, source: TranscriptSource<'_>) -> TranscriptShown {
    let names = source.names;
    let held = open.transcript.loaded();
    TranscriptShown {
        state: held.map_or(TranscriptState::None, |held| held.state),
        failure: held.and_then(|held| held.failure),
        total: held.map_or(0, |held| held.lines),
        language: held.map(|held| held.language.clone()).unwrap_or_default(),
        held: open.lines.lines.len(),
        lines: first_lines(open, &open.lines, names, source.when),
        more: open.lines.more,
        more_before: open.lines.more_before,
        reading: open.lines.reading,
        failed: open.lines.failed.is_some() || open.failed.is_some(),
        refresh_failed: open.failed.is_some(),
        voices: held.map_or_else(Vec::new, |held| voices(held, names, source.when)),
        used: held.map_or_else(Vec::new, |held| held.glossary.used.clone()),
        cut: held.map_or_else(Vec::new, |held| held.glossary.cut.clone()),
        segments: held.map_or_else(Vec::new, |held| segment_results(open, held, names)),
        may_change: source.may_change,
        ask: source.ask.cloned(),
    }
}

/// The card of a recording.
pub fn card(source: &CardSource<'_>) -> RecordingCard {
    let state = match source.open {
        None => CardState::Reading,
        Some(open) if open.gone => CardState::Gone,
        Some(open) => match (&open.detail, open.failed) {
            // A server is not trusted to name a recording of the shown channel
            // (ADR 0714): the names and the permissions of the card are the ones of
            // that channel.
            (Loadable::Loaded(detail), _) if detail.recording.channel_id != source.channel_id => {
                CardState::Gone
            }
            (Loadable::Loaded(detail), _) => {
                CardState::Shown(Box::new(shown(source, open, detail)))
            }
            (_, Some(ReadFailure::NotFound)) => CardState::Gone,
            (_, Some(_)) => CardState::Failed,
            (Loadable::Loading | Loadable::NotLoaded, None) => CardState::Reading,
        },
    };
    let request = match &state {
        CardState::Shown(detail)
            if matches!(detail.job, JobShown::Ready | JobShown::PartlyFailed { .. })
                && matches!(
                    detail.transcript.state,
                    TranscriptState::None | TranscriptState::Failed
                ) =>
        {
            if source.may_request {
                TranscriptRequest::Allowed {
                    sending: source.requesting,
                }
            } else {
                TranscriptRequest::NotAllowed
            }
        }
        _ => TranscriptRequest::NotOffered,
    };
    RecordingCard {
        server_id: source.server_id,
        recording_id: source.recording_id,
        state,
        may_remove: source.may_remove,
        removing: source.removing,
        request,
        refused: source.refused.map(str::to_owned),
    }
}

fn shown(source: &CardSource<'_>, open: &OpenRecording, detail: &RecordingDetail) -> CardDetail {
    let recording = &detail.recording;
    let names = source.names;
    let local = |at: OffsetDateTime| source.zone.local(at);
    let clock = |at: OffsetDateTime| {
        let at = local(at);
        format!("{:02}:{:02}", at.hour(), at.minute())
    };
    // The recording starts with its first sample when the files are processed, and with
    // the first job of the recorder before.
    let began = recording
        .timeline_started_at
        .or(recording.recording_since)
        .unwrap_or(recording.requested_at);
    let ended = recording
        .stop
        .as_ref()
        .map(|stop| stop.at)
        .or(recording.ended_at);
    // The files of one recording are named alike, by the channel and the start.
    let stamp = {
        let at = local(began);
        format!(
            "{} {:04}-{:02}-{:02} {:02}.{:02}",
            source.channel,
            at.year(),
            u8::from(at.month()),
            at.day(),
            at.hour(),
            at.minute()
        )
    };
    let mut files = Vec::new();
    let mut picture = None;
    for segment in &detail.segments {
        let Some(file) = &segment.file else {
            continue;
        };
        let kind = match segment.source {
            SegmentSource::Composite => FileKind::Video,
            SegmentSource::Microphone => FileKind::Microphone,
            SegmentSource::ScreenShareAudio => FileKind::ShareSound,
        };
        let person = segment
            .account_id
            .map(|account_id| name_of(account_id, names));
        if kind == FileKind::Video && picture.is_none() {
            picture = file.width.zip(file.height);
        }
        // A person has one file for each time that the track started: a second file of
        // the same kind and person gets its number.
        let same = files
            .iter()
            .filter(|held: &&FileRow| held.kind == kind && held.person == person)
            .count();
        files.push(FileRow {
            segment_id: segment.id,
            kind,
            stem: text::recording_file(&stamp, kind, person.as_deref(), same),
            person,
            bytes: file.bytes,
            problem: (file.check == FileCheck::Failed).then(|| text::file_problems(&file.problems)),
            download: source
                .downloads
                .get(&(source.server_id, segment.id))
                .cloned(),
        });
    }
    let count = |kind| files.iter().filter(|file| file.kind == kind).count();
    CardDetail {
        channel: source.channel.to_owned(),
        day: text::day_or_today(local(began).date(), source.today),
        span: ended.map(|ended| (clock(began), clock(ended))),
        length: ended.and_then(|ended| Duration::try_from(ended - began).ok()),
        started_by: names.get(&recording.started_by).cloned(),
        picture,
        microphones: count(FileKind::Microphone),
        share_sounds: count(FileKind::ShareSound),
        // The sum of the files that a person can save. The size that the recording
        // states is not shown: a download writes what its file states.
        bytes: (!files.is_empty()).then(|| {
            files
                .iter()
                .fold(0u64, |sum, file| sum.saturating_add(file.bytes))
        }),
        job: match recording.state {
            RecordingState::Starting => JobShown::Starting,
            RecordingState::Recording => JobShown::Recording,
            RecordingState::Processing => JobShown::Processing,
            RecordingState::Ready => JobShown::Ready,
            RecordingState::PartlyFailed => JobShown::PartlyFailed {
                failed: recording.failed_segments,
                segments: recording.segments,
            },
            RecordingState::Failed => JobShown::Failed(recording.failure),
        },
        video: files
            .iter()
            .find(|file| file.kind == FileKind::Video)
            .map(|file| file.segment_id),
        parts: parts(detail, names, source.zone),
        files,
        transcript: transcript(
            open,
            TranscriptSource {
                names,
                when: When {
                    zone: source.zone,
                    today: source.today,
                },
                // The card shows no control that changes the transcript: the side panel
                // has them.
                may_change: false,
                ask: None,
            },
        ),
        text_stem: format!("{stamp} transcript"),
        text_download: source
            .texts
            .get(&(source.server_id, source.recording_id))
            .cloned(),
    }
}

#[cfg(test)]
mod tests;
