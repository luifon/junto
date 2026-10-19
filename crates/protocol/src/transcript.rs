//! Transcripts of recordings and the glossary of a workspace (ADR 0603, ADR 0607).
//!
//! The transcription worker writes the lines of a transcript. People correct a line and
//! rename a speaker label; the text that the worker wrote stays readable.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::{AccountId, RecordingId, RecordingSegmentId, WorkspaceId};

/// The state of the transcript of a recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TranscriptState {
    /// The recording has no transcript: its files are not processed yet, it has no audio
    /// file, or the server has no transcription worker.
    None,
    /// The job waits for the worker.
    Waiting,
    /// The worker runs.
    Running,
    /// Every audio file of the recording was transcribed whole.
    Ready,
    /// The transcript covers a part of the audio: an audio file is missing, or the worker
    /// could read a file only in part. The segments of the transcript state which.
    Partial,
    /// No transcript was built.
    Failed,
}

/// Why no transcript was built. A server can add reasons within one API version; a reason
/// that this build does not know is read as [`TranscriptFailure::Unknown`], which is never
/// written (ADR 0209).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum TranscriptFailure {
    /// The worker ended with an error at every attempt.
    WorkerFailed,
    /// The worker refused the job.
    JobRefused,
    /// The worker refused to start: a model is missing or differs, or its device is absent.
    WorkerNotReady,
    /// The worker did not end within the time limit of the server.
    TimeLimit,
    /// The worker wrote a transcript that does not follow the contract.
    TranscriptInvalid,
    /// The server stopped during every attempt.
    Interrupted,
    #[serde(other, skip_serializing)]
    Unknown,
}

/// How much of one audio file the transcript covers (ADR 0611).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SegmentTranscription {
    /// The whole file was processed and has at least one line.
    Ok,
    /// The whole file was processed and has no line.
    Empty,
    /// A part of the file was processed; the lines of that part exist.
    Damaged,
    /// Nothing of the file was processed.
    Failed,
    /// The file was not given to the worker.
    NotTranscribed,
}

/// Why a segment is damaged, failed or not transcribed. Read with an unknown value as
/// [`TranscriptFailure`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SegmentReason {
    FileMissing,
    NotARegularFile,
    DecodeFailed,
    NotOneAudioStream,
    DecodeIncomplete,
    TimestampGap,
    TimestampBackward,
    SegmentDurationLimit,
    JobDurationLimit,
    TimeLimit,
    ProcessingFailed,
    /// The worker decoded less audio than the file holds.
    ShorterThanFile,
    /// The server found no file of the segment.
    NoFile,
    /// The recorder reported no start time for the file, so it has no place on the timeline.
    NoStartTime,
    #[serde(other, skip_serializing)]
    Unknown,
}

/// Whether the lines of a segment of shared sound have speaker labels (ADR 0614).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SpeakerSeparation {
    /// A microphone: the speaker is the person of the track.
    NotApplicable,
    NotRequested,
    /// Every line has a label.
    Separated,
    /// At least one line has no label.
    Unassigned,
    /// No line has a label.
    Failed,
}

/// Why labels are missing. Read with an unknown value as [`TranscriptFailure`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SpeakersReason {
    SegmentFailed,
    EmbeddingFailed,
    ClusteringFailed,
    NoVoiceWindow,
    SpeakerWindowLimit,
    TimeLimit,
    #[serde(other, skip_serializing)]
    Unknown,
}

/// The result of the transcript for one audio segment of the recording.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TranscriptSegment {
    pub segment_id: RecordingSegmentId,
    pub status: SegmentTranscription,
    /// Present with the status `damaged`, `failed` or `not_transcribed`.
    pub reason: Option<SegmentReason>,
    /// The length of the audio that the worker decoded, in milliseconds.
    pub audio_ms: Option<u64>,
    /// The lines cover the audio from the start of the file to this point.
    pub processed_ms: Option<u64>,
    pub lines: u32,
    pub speakers: SpeakerSeparation,
    /// Present when labels are missing.
    pub speakers_reason: Option<SpeakersReason>,
}

/// One voice in the shared sound of one person. The worker numbers the voices of a
/// recording; a person can give a voice a name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TranscriptSpeaker {
    /// N of the label `Speaker N`.
    pub number: u32,
    /// The person whose shared sound holds the voice.
    pub account_id: AccountId,
    /// The name that a person gave to the voice. Absent: the label `Speaker N` is shown.
    pub name: Option<String>,
    /// The person who gave the name.
    pub renamed_by: Option<AccountId>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub renamed_at: Option<OffsetDateTime>,
}

/// The terms of the glossary that the worker gave to the speech model, and those that did
/// not fit in its limit (ADR 0613).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct GlossaryUse {
    pub used: Vec<String>,
    pub cut: Vec<String>,
}

/// The transcript of a recording without its lines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Transcript {
    pub recording_id: RecordingId,
    pub state: TranscriptState,
    /// Present with the state `failed`.
    pub failure: Option<TranscriptFailure>,
    /// `auto`, or the language that the person who started the recording set.
    pub language: String,
    pub lines: u32,
    pub speakers: Vec<TranscriptSpeaker>,
    pub glossary: GlossaryUse,
    /// One result for each audio segment of the recording. Empty until the transcript is
    /// built.
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum LineSource {
    Microphone,
    /// The sound of a screen share.
    Shared,
}

/// One line of a transcript: one speech clip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TranscriptLine {
    /// The position of the line in the transcript, from 0.
    pub line: u32,
    /// Milliseconds from the start of the timeline of the recording.
    pub start_ms: u64,
    pub end_ms: u64,
    pub segment_id: RecordingSegmentId,
    pub source: LineSource,
    /// The person of the microphone, or the person whose screen share holds the sound.
    pub account_id: AccountId,
    /// The number of the voice in shared sound. Absent for a microphone and for a line
    /// without a label.
    pub speaker: Option<u32>,
    /// The newest correction, or the text of the worker.
    pub text: String,
    /// The language that the speech model used for the line.
    pub language: String,
    /// The text of the worker. Present when the line was corrected.
    pub original_text: Option<String>,
    /// The person who made the newest correction.
    pub corrected_by: Option<AccountId>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub corrected_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct TranscriptLinePage {
    /// In the order of the timeline.
    pub lines: Vec<TranscriptLine>,
    /// Pass as `after` to read the next page. Absent on the last page.
    pub next_after: Option<u32>,
    /// Pass as `before` to read the page before this one. Absent when the page starts at
    /// the first line.
    #[serde(default)]
    pub previous_before: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct TranscriptLineQuery {
    /// Return the lines after this position.
    #[serde(default)]
    pub after: Option<u32>,
    /// Return the lines before this position, the ones nearest to it.
    #[serde(default)]
    pub before: Option<u32>,
    /// Return a page with the line at this position in its middle. At most one of `after`,
    /// `before` and `around` is given.
    #[serde(default)]
    pub around: Option<u32>,
    /// The largest number of lines to return, from 1 to 500. Absent: 200.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CorrectLine {
    /// The corrected text: 1 to 2000 characters without control characters.
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RenameSpeaker {
    /// The name of the voice: 1 to 64 characters.
    pub name: String,
}

/// The glossary of a workspace: names and terms that the speech model receives before it
/// transcribes, most important first (ADR 0603).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Glossary {
    pub workspace_id: WorkspaceId,
    pub terms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetGlossary {
    /// At most 200 terms of 1 to 200 characters each, most important first, without
    /// duplicates.
    pub terms: Vec<String>,
    /// The terms that the caller read and changed. When given, the server replaces the
    /// glossary only while it still holds exactly these terms in this order, and answers
    /// `conflict` otherwise, so that a change made from an older list does not undo a
    /// change of another person (ADR 0607).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replaces: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_have_their_wire_names_and_are_a_closed_set() {
        let states = [
            (TranscriptState::None, "none"),
            (TranscriptState::Waiting, "waiting"),
            (TranscriptState::Running, "running"),
            (TranscriptState::Ready, "ready"),
            (TranscriptState::Partial, "partial"),
            (TranscriptState::Failed, "failed"),
        ];
        for (state, name) in states {
            assert_eq!(serde_json::to_value(state).unwrap(), name);
        }
        assert!(serde_json::from_str::<TranscriptState>("\"queued\"").is_err());
        assert_eq!(
            serde_json::to_value(SegmentTranscription::NotTranscribed).unwrap(),
            "not_transcribed"
        );
        assert_eq!(
            serde_json::to_value(SegmentReason::NotARegularFile).unwrap(),
            "not_a_regular_file"
        );
    }

    #[test]
    fn a_reason_of_a_newer_server_is_read_as_unknown_and_never_written() {
        assert_eq!(
            serde_json::from_str::<TranscriptFailure>("\"out_of_memory\"").unwrap(),
            TranscriptFailure::Unknown
        );
        assert_eq!(
            serde_json::from_str::<SegmentReason>("\"too_quiet\"").unwrap(),
            SegmentReason::Unknown
        );
        assert_eq!(
            serde_json::from_str::<SpeakersReason>("\"too_many\"").unwrap(),
            SpeakersReason::Unknown
        );
        assert!(serde_json::to_string(&TranscriptFailure::Unknown).is_err());
        assert!(serde_json::to_string(&SegmentReason::Unknown).is_err());
        assert!(serde_json::to_string(&SpeakersReason::Unknown).is_err());
    }
}
