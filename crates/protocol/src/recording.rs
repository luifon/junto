//! Recordings of calls in voice channels (ADR 0601, ADR 0604).
//!
//! A recording is a tracked job of the server. The recorder writes one file per audio track
//! and the composite video; each file is a segment of the recording.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::{AccountId, CallId, ChannelId, RecordingId, RecordingSegmentId, WorkspaceId};

/// The state of a recording. The states follow each other in this order; the last three are
/// final.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RecordingState {
    /// The server has reserved disk space and asks the recorder to start.
    Starting,
    /// At least one job of the recorder runs. A stop that was asked for is in `stop`; the
    /// state stays until every job has ended.
    Recording,
    /// Every job has ended. The server checks the files and builds the results.
    Processing,
    Ready,
    /// A part of the recording was saved and a part was not.
    PartlyFailed,
    /// Nothing was saved.
    Failed,
}

impl RecordingState {
    /// True while the recorder is asked to record: the people in the call see the indicator.
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Recording)
    }
}

/// Why a recording was stopped. A server can add reasons within one API version; a reason
/// that this build does not know is read as [`RecordingStopReason::Unknown`], which is never
/// written (ADR 0209).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RecordingStopReason {
    /// A person in the call stopped it.
    Stopped,
    /// The call ended.
    CallEnded,
    /// The free disk space reached the reserve of the server (ADR 0006).
    StorageReserve,
    /// The recording holds as many segments as a recording can have.
    SegmentLimit,
    /// No job of the recording ran any more, and the requests to start new ones failed.
    RecorderLost,
    #[serde(other, skip_serializing)]
    Unknown,
}

/// Why nothing of a recording was saved. Read with an unknown value as
/// [`RecordingStopReason`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum RecordingFailure {
    /// No recorder took the first requests of the recording.
    RecorderUnavailable,
    /// Every job of the recording ended without a file.
    NothingRecorded,
    #[serde(other, skip_serializing)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RecordingStop {
    pub reason: RecordingStopReason,
    /// The person who stopped the recording. Absent when the server stopped it.
    pub by: Option<AccountId>,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
}

/// A recording without its segments. Events and lists carry this form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Recording {
    pub id: RecordingId,
    pub workspace_id: WorkspaceId,
    pub channel_id: ChannelId,
    pub call_id: CallId,
    pub state: RecordingState,
    pub started_by: AccountId,
    /// When the recording was asked for.
    #[serde(with = "time::serde::rfc3339")]
    pub requested_at: OffsetDateTime,
    /// When the first job of the recorder was started. Absent before.
    #[serde(with = "time::serde::rfc3339::option")]
    pub recording_since: Option<OffsetDateTime>,
    /// Present from the moment a stop is asked for.
    pub stop: Option<RecordingStop>,
    /// When the last job of the recorder had ended. Absent while the recording is active.
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
    /// Present with the state `failed`.
    pub failure: Option<RecordingFailure>,
    /// `auto`, or the language code that the person who started the recording set for the
    /// transcript (ADR 0603).
    pub language: String,
    /// The number of segments of the recording.
    pub segments: u32,
    /// The number of segments that failed. The segments state what was saved of each.
    pub failed_segments: u32,
}

/// What a segment records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SegmentSource {
    /// The video of the call with the sound of the main share and of the microphones.
    Composite,
    Microphone,
    /// The sound of a screen share.
    ScreenShareAudio,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SegmentState {
    /// The server asks the recorder to start the job.
    Starting,
    Recording,
    /// The job ended and the recorder reported its file.
    Ended,
    Failed,
}

/// Why a segment failed. Read with an unknown value as [`RecordingStopReason`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum SegmentFailure {
    /// No recorder took the request to start the job.
    RecorderUnavailable,
    /// The recorder refused the request to start the job.
    StartRefused,
    /// The server stopped before it had the answer to the request to start the job.
    StartInterrupted,
    /// The recorder was lost while the job ran (ADR 0601).
    RecorderLost,
    /// The recorder ended the job with an error.
    RecorderError,
    #[serde(other, skip_serializing)]
    Unknown,
}

/// One file of a recording: the composite, or one audio track of one person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RecordingSegment {
    pub id: RecordingSegmentId,
    pub source: SegmentSource,
    /// The person whose track the segment records. Absent for the composite.
    pub account_id: Option<AccountId>,
    pub state: SegmentState,
    /// Present with the state `failed`.
    pub failure: Option<SegmentFailure>,
    /// When the server asked the recorder for the job.
    #[serde(with = "time::serde::rfc3339")]
    pub requested_at: OffsetDateTime,
    /// The start of the file, as the recorder reports it. Absent until the recorder has
    /// reported it. Of a failed segment, `started_at` and `ended_at` state the time range
    /// that was saved, as far as the recorder reported it.
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub ended_at: Option<OffsetDateTime>,
    /// The length of the file in milliseconds, as the recorder reports it.
    pub duration_ms: Option<u64>,
}

/// A recording with its segments, in the order in which they were asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RecordingDetail {
    pub recording: Recording,
    pub segments: Vec<RecordingSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StartRecording {
    /// The language of the speech, as a Whisper language code of two or three lower-case
    /// letters. Absent or `auto`: the language of each speech clip is detected (ADR 0603).
    #[serde(default)]
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RecordingPage {
    /// Newest first.
    pub recordings: Vec<Recording>,
    /// Pass as `before` to read the next page. Absent on the last page.
    pub next_before: Option<RecordingId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct RecordingQuery {
    /// Return recordings older than this one.
    #[serde(default)]
    pub before: Option<RecordingId>,
    /// The largest number of recordings to return, from 1 to 200. Absent: 50.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states_and_sources_have_their_wire_names() {
        let states = [
            (RecordingState::Starting, "starting"),
            (RecordingState::Recording, "recording"),
            (RecordingState::Processing, "processing"),
            (RecordingState::Ready, "ready"),
            (RecordingState::PartlyFailed, "partly_failed"),
            (RecordingState::Failed, "failed"),
        ];
        for (state, name) in states {
            assert_eq!(serde_json::to_value(state).unwrap(), name);
            assert_eq!(
                state.is_active(),
                matches!(name, "starting" | "recording"),
                "{name}"
            );
        }
        assert_eq!(
            serde_json::to_value(SegmentSource::ScreenShareAudio).unwrap(),
            "screen_share_audio"
        );
        assert!(serde_json::from_str::<RecordingState>("\"paused\"").is_err());
    }

    #[test]
    fn a_reason_of_a_newer_server_is_read_as_unknown_and_never_written() {
        assert_eq!(
            serde_json::from_str::<RecordingStopReason>("\"retention\"").unwrap(),
            RecordingStopReason::Unknown
        );
        assert_eq!(
            serde_json::from_str::<RecordingFailure>("\"check_failed\"").unwrap(),
            RecordingFailure::Unknown
        );
        assert_eq!(
            serde_json::from_str::<SegmentFailure>("\"file_damaged\"").unwrap(),
            SegmentFailure::Unknown
        );
        assert!(serde_json::to_string(&RecordingStopReason::Unknown).is_err());
        assert!(serde_json::to_string(&RecordingFailure::Unknown).is_err());
        assert!(serde_json::to_string(&SegmentFailure::Unknown).is_err());
        assert_eq!(
            serde_json::to_value(SegmentFailure::RecorderLost).unwrap(),
            "recorder_lost"
        );
    }
}
