//! The text of the page of the recordings of a workspace (ADR 0784).

use std::time::Duration;

use junto_client_core::api::{ApiError, ErrorCategory};
use junto_client_core::flow::FlowError;
use junto_client_core::protocol::recording::{RecordingFailure, RecordingState};
use junto_client_core::protocol::transcript::TranscriptState;

use crate::state::admin::view::recordings::RecordingLine;

pub const HELP: &str = "The recordings of the calls of this workspace that your account can see, the newest \
     first. \"Download\" lists the files and the transcript of a recording; each one is saved \
     to your download folder. This page does not play or remove a recording.";
pub const NO_RECORDINGS: &str = "This workspace has no recording that your account can see.";
pub const LOAD_OLDER: &str = "Load older";
pub const READ_AGAIN: &str = "Read again";
pub const BOUND: &str = "The page shows the newest 2,000 recordings. It reads no older one.";
pub const UNKNOWN_CHANNEL: &str = "A channel that is not shown";
pub const AN_ACCOUNT: &str = "an account";

/// The longest name of a channel or of a person that a row shows, in characters. A server
/// allows 64.
const NAME_CHARACTERS: usize = 64;

/// A name of a server as a row shows it: without control characters, and cut.
fn shown_name(name: &str) -> String {
    name.chars()
        .filter(|character| !character.is_control())
        .take(NAME_CHARACTERS)
        .collect()
}

/// The channel of a recording.
pub fn channel(line: &RecordingLine) -> String {
    line.channel
        .as_deref()
        .map(shown_name)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| UNKNOWN_CHANNEL.to_owned())
}

/// A length as hours, minutes and seconds: `1 h 4 min 9 s`, `36 min 5 s`, `42 s`.
pub fn length(length: Duration) -> String {
    let seconds = length.as_secs();
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours} h {minutes} min {seconds} s")
    } else if minutes > 0 {
        format!("{minutes} min {seconds} s")
    } else {
        format!("{seconds} s")
    }
}

/// When the recording started, its length when it ended, and its size when the server
/// knows it.
pub fn when(line: &RecordingLine) -> String {
    let at = super::date_time(line.since);
    let mut parts = vec![if line.started {
        format!("Started {at}")
    } else {
        format!("Asked for {at}")
    }];
    if let Some(held) = line.length {
        parts.push(length(held));
    }
    if let Some(bytes) = line.bytes {
        parts.push(crate::text::file_size(bytes));
    }
    parts.join(" · ")
}

/// The state of a recording as words.
pub fn state(line: &RecordingLine) -> String {
    match line.state {
        RecordingState::Starting => "Starting: the server asks the recorder.".to_owned(),
        RecordingState::Recording => "Recording now.".to_owned(),
        RecordingState::Processing => "Ended. The server checks the files.".to_owned(),
        RecordingState::Ready => "Ready.".to_owned(),
        RecordingState::PartlyFailed => {
            if line.failed_segments > 0 && line.segments > 0 {
                format!(
                    "Saved in part: {} of its {} files failed.",
                    line.failed_segments.min(line.segments),
                    line.segments
                )
            } else {
                "Saved in part: a file is missing or failed its check.".to_owned()
            }
        }
        RecordingState::Failed => match line.failure {
            Some(RecordingFailure::RecorderUnavailable) => {
                "Failed: no recorder took the recording. Nothing was saved.".to_owned()
            }
            Some(RecordingFailure::NothingRecorded) => {
                "Failed: the recorder ended without a file. Nothing was saved.".to_owned()
            }
            Some(RecordingFailure::FilesMissing) => {
                "Failed: the server found no file of the recorder. Nothing was saved.".to_owned()
            }
            Some(RecordingFailure::Unknown) | None => "Failed. Nothing was saved.".to_owned(),
        },
    }
}

/// Who started the recording.
pub fn started_by(line: &RecordingLine) -> String {
    let name = line
        .started_by
        .as_deref()
        .map(shown_name)
        .filter(|name| !name.trim().is_empty());
    format!("Started by {}", name.as_deref().unwrap_or(AN_ACCOUNT))
}

fn lines(count: u32) -> String {
    match count {
        1 => "1 line".to_owned(),
        count => format!("{count} lines"),
    }
}

/// The state of the transcript with the number of its lines.
pub fn transcript(state: TranscriptState, count: u32) -> String {
    match state {
        TranscriptState::None => "No transcript".to_owned(),
        TranscriptState::Waiting => "Transcript: waits for the transcription".to_owned(),
        TranscriptState::Running => "Transcript: in progress".to_owned(),
        TranscriptState::Ready => format!("Transcript: {}", lines(count)),
        TranscriptState::Partial => {
            format!("Transcript: {}, for a part of the sound", lines(count))
        }
        TranscriptState::Failed => "Transcript: failed".to_owned(),
    }
}

/// The number of loaded recordings.
pub fn count(loaded: usize) -> String {
    match loaded {
        1 => "1 recording is loaded.".to_owned(),
        loaded => format!("{loaded} recordings are loaded."),
    }
}

/// Why the list was not read.
pub fn read_failure(error: &ApiError) -> String {
    match error.category() {
        Some(ErrorCategory::NotAllowed) => {
            "Your account may not read the recordings of this workspace.".to_owned()
        }
        Some(ErrorCategory::NotFound) => {
            "The server has no such workspace for your account.".to_owned()
        }
        _ => crate::text::flow_error(&FlowError::from(error)),
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::api::{Rejection, TransportError, TransportErrorKind};
    use junto_client_core::protocol::error::ErrorCode;
    use junto_client_core::protocol::ids::RecordingId;
    use time::OffsetDateTime;

    use super::*;

    fn line(state: RecordingState) -> RecordingLine {
        RecordingLine {
            id: RecordingId::generate(),
            channel: Some("Daily".to_owned()),
            // 15 January 2027, 08:00 UTC.
            since: OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap(),
            started: true,
            length: None,
            state,
            failure: None,
            failed_segments: 0,
            segments: 3,
            started_by: Some("Ana".to_owned()),
            transcript: TranscriptState::None,
            transcript_lines: 0,
            bytes: None,
            downloads: false,
            files: None,
        }
    }

    #[test]
    fn every_state_of_a_recording_has_its_words() {
        let mut seen = std::collections::BTreeSet::new();
        for held in [
            RecordingState::Starting,
            RecordingState::Recording,
            RecordingState::Processing,
            RecordingState::Ready,
            RecordingState::PartlyFailed,
            RecordingState::Failed,
        ] {
            // A match without a wildcard: a state that the protocol adds does not compile
            // until it is in the list above.
            match held {
                RecordingState::Starting
                | RecordingState::Recording
                | RecordingState::Processing
                | RecordingState::Ready
                | RecordingState::PartlyFailed
                | RecordingState::Failed => {}
            }
            let words = state(&line(held));
            assert!(words.ends_with('.'), "{held:?}: {words}");
            assert!(seen.insert(words), "{held:?}");
        }
        let mut partly = line(RecordingState::PartlyFailed);
        partly.failed_segments = 2;
        partly.segments = 5;
        assert_eq!(state(&partly), "Saved in part: 2 of its 5 files failed.");
        // A count that cannot hold is not written as it is.
        partly.failed_segments = 9;
        assert_eq!(state(&partly), "Saved in part: 5 of its 5 files failed.");
        let mut failures = std::collections::BTreeSet::new();
        for failure in [
            Some(RecordingFailure::RecorderUnavailable),
            Some(RecordingFailure::NothingRecorded),
            Some(RecordingFailure::FilesMissing),
            Some(RecordingFailure::Unknown),
        ] {
            let mut failed = line(RecordingState::Failed);
            failed.failure = failure;
            let words = state(&failed);
            assert!(words.starts_with("Failed"), "{failure:?}");
            assert!(words.ends_with("Nothing was saved."), "{failure:?}");
            assert!(failures.insert(words), "{failure:?}");
        }
        assert_eq!(
            state(&line(RecordingState::Failed)),
            "Failed. Nothing was saved."
        );
    }

    #[test]
    fn a_row_states_the_start_the_length_and_the_size() {
        let mut row = line(RecordingState::Ready);
        assert_eq!(when(&row), "Started 15 January 2027, 08:00");
        row.length = Some(Duration::from_secs(36 * 60 + 5));
        row.bytes = Some(48_200_000);
        assert_eq!(
            when(&row),
            "Started 15 January 2027, 08:00 · 36 min 5 s · 48 MB"
        );
        row.started = false;
        row.length = None;
        row.bytes = None;
        assert_eq!(when(&row), "Asked for 15 January 2027, 08:00");
        assert_eq!(length(Duration::from_secs(42)), "42 s");
        assert_eq!(length(Duration::ZERO), "0 s");
        assert_eq!(length(Duration::from_secs(60)), "1 min 0 s");
        assert_eq!(
            length(Duration::from_secs(3600 + 4 * 60 + 9)),
            "1 h 4 min 9 s"
        );
        assert_eq!(length(Duration::from_secs(26 * 3600)), "26 h 0 min 0 s");
    }

    #[test]
    fn a_name_that_the_state_lacks_has_a_neutral_text_and_a_name_of_a_server_is_cut() {
        let mut row = line(RecordingState::Ready);
        assert_eq!(channel(&row), "Daily");
        assert_eq!(started_by(&row), "Started by Ana");
        row.channel = None;
        row.started_by = None;
        assert_eq!(channel(&row), UNKNOWN_CHANNEL);
        assert_eq!(channel(&row), "A channel that is not shown");
        assert_eq!(started_by(&row), "Started by an account");

        // Text of a server: no control character, and at most 64 characters.
        row.channel = Some(format!("Dai\u{7}ly\n{}", "x".repeat(500)));
        let shown = channel(&row);
        assert!(shown.starts_with("Dailyx"));
        assert_eq!(shown.chars().count(), 64);
        row.started_by = Some("\u{1b}[2J\r\n".to_owned());
        assert_eq!(started_by(&row), "Started by [2J");
        row.started_by = Some("\n\t ".to_owned());
        assert_eq!(started_by(&row), "Started by an account");
        row.channel = Some("\u{0}".to_owned());
        assert_eq!(channel(&row), UNKNOWN_CHANNEL);
    }

    #[test]
    fn every_state_of_a_transcript_has_its_words() {
        let mut seen = std::collections::BTreeSet::new();
        for held in [
            TranscriptState::None,
            TranscriptState::Waiting,
            TranscriptState::Running,
            TranscriptState::Ready,
            TranscriptState::Partial,
            TranscriptState::Failed,
        ] {
            match held {
                TranscriptState::None
                | TranscriptState::Waiting
                | TranscriptState::Running
                | TranscriptState::Ready
                | TranscriptState::Partial
                | TranscriptState::Failed => {}
            }
            assert!(seen.insert(transcript(held, 212)), "{held:?}");
        }
        assert_eq!(
            transcript(TranscriptState::Ready, 212),
            "Transcript: 212 lines"
        );
        assert_eq!(transcript(TranscriptState::Ready, 1), "Transcript: 1 line");
        assert_eq!(
            transcript(TranscriptState::Partial, 40),
            "Transcript: 40 lines, for a part of the sound"
        );
        assert_eq!(transcript(TranscriptState::None, 0), "No transcript");
        assert_eq!(count(1), "1 recording is loaded.");
        assert_eq!(count(50), "50 recordings are loaded.");
    }

    #[test]
    fn a_refusal_and_a_server_that_cannot_be_reached_have_their_sentences() {
        let rejected = |code: ErrorCode| {
            ApiError::Rejected(Rejection {
                code,
                status: code.http_status(),
                retry_after: None,
                message: "a sentence of the server".to_owned(),
                left: None,
            })
        };
        assert_eq!(
            read_failure(&rejected(ErrorCode::Forbidden)),
            "Your account may not read the recordings of this workspace."
        );
        assert_eq!(
            read_failure(&rejected(ErrorCode::NotFound)),
            "The server has no such workspace for your account."
        );
        let unreachable = ApiError::Unreachable(TransportError::new(
            TransportErrorKind::Connect,
            "connection refused",
        ));
        assert_eq!(
            read_failure(&unreachable),
            "No connection to the server. Check the address and the network connection."
        );
        for error in [
            rejected(ErrorCode::RateLimited),
            rejected(ErrorCode::Unavailable),
            rejected(ErrorCode::Unauthenticated),
        ] {
            let sentence = read_failure(&error);
            assert!(sentence.ends_with('.'), "{sentence}");
            // The sentence of a server is not shown.
            assert!(!sentence.contains("a sentence of the server"), "{sentence}");
        }
    }
}
