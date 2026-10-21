//! `recordings`: the routes of recordings and of their transcripts from a terminal
//! (ADR 0604, ADR 0607, ADR 0608).

use std::path::PathBuf;

use junto_client_core::ServerHandle;
use junto_client_core::protocol::ids::{RecordingId, RecordingSegmentId};
use junto_client_core::protocol::recording::{Recording, RecordingSegment};
use junto_client_core::protocol::transcript::{Transcript, TranscriptLine, TranscriptSpeaker};
use junto_client_core::transfers::RecordingDownload;

use crate::discovery::{identifier, word};
use crate::{Arguments, Client, Failure, end_session, failed, logged_in, shown, usage};

const ACTIONS: [&str; 10] = [
    "list",
    "show",
    "remove",
    "transcript",
    "request-transcript",
    "lines",
    "text",
    "correct",
    "rename-speaker",
    "download",
];

/// The word of the API for a state, a source or a reason.
fn name<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "unknown".to_owned(),
    }
}

fn names<T: serde::Serialize>(values: &[T]) -> String {
    values.iter().map(name).collect::<Vec<_>>().join(", ")
}

/// A time of the timeline as hours, minutes, seconds and milliseconds.
fn clock(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60,
        milliseconds % 1000
    )
}

fn recording_lines(recording: &Recording) -> Vec<String> {
    let stop = match &recording.stop {
        Some(stop) => match stop.by {
            Some(by) => format!("{} by {by}", name(&stop.reason)),
            None => name(&stop.reason),
        },
        None => "none".to_owned(),
    };
    let bytes = recording
        .bytes
        .map_or_else(|| "not processed".to_owned(), |bytes| bytes.to_string());
    let mut lines = vec![
        format!(
            "recording: {}  channel {}  state: {}  started by {}  {}",
            recording.id,
            recording.channel_id,
            name(&recording.state),
            recording.started_by,
            recording.requested_at
        ),
        format!(
            "  segments: {} ({} failed)  bytes: {bytes}  stop: {stop}",
            recording.segments, recording.failed_segments
        ),
        format!(
            "  transcript: {}  lines: {}  language: {}",
            name(&recording.transcript),
            recording.transcript_lines,
            shown(&recording.language)
        ),
    ];
    if let Some(failure) = &recording.failure {
        lines.push(format!("  failure: {}", name(failure)));
    }
    lines
}

fn segment_lines(segment: &RecordingSegment) -> Vec<String> {
    let of = match segment.account_id {
        Some(account_id) => format!("  account {account_id}"),
        None => String::new(),
    };
    let failure = match &segment.failure {
        Some(failure) => format!("  failure: {}", name(failure)),
        None => String::new(),
    };
    let file = match &segment.file {
        Some(file) => {
            let mut text = format!(
                "    file: {}  {} bytes  {} ms  check: {}",
                shown(&file.media_type),
                file.bytes,
                file.duration_ms,
                name(&file.check)
            );
            if !file.problems.is_empty() {
                text.push_str(&format!("  problems: {}", names(&file.problems)));
            }
            if file.silent {
                text.push_str("  silent");
            }
            if file.recovered {
                text.push_str("  recovered");
            }
            text
        }
        None => "    file: none".to_owned(),
    };
    vec![
        format!(
            "  segment: {}  {}{of}  state: {}{failure}",
            segment.id,
            name(&segment.source),
            name(&segment.state)
        ),
        file,
    ]
}

fn speaker_line(speaker: &TranscriptSpeaker) -> String {
    let named = match (&speaker.name, speaker.renamed_by) {
        (Some(name), Some(by)) => format!("name: {} (by {by})", shown(name)),
        (Some(name), None) => format!("name: {}", shown(name)),
        (None, _) => "no name".to_owned(),
    };
    format!(
        "  speaker: {}  shared sound of {}  {named}",
        speaker.number, speaker.account_id
    )
}

fn transcript_lines(transcript: &Transcript) -> Vec<String> {
    let failure = match &transcript.failure {
        Some(failure) => format!("  failure: {}", name(failure)),
        None => String::new(),
    };
    let mut lines = vec![format!(
        "transcript: {}  state: {}  lines: {}  language: {}{failure}",
        transcript.recording_id,
        name(&transcript.state),
        transcript.lines,
        shown(&transcript.language)
    )];
    lines.extend(transcript.speakers.iter().map(speaker_line));
    let terms = |terms: &[String]| {
        let shown: Vec<String> = terms.iter().map(|term| shown(term)).collect();
        shown.join(", ")
    };
    if !transcript.glossary.used.is_empty() || !transcript.glossary.cut.is_empty() {
        lines.push(format!(
            "  glossary used: {}  cut: {}",
            terms(&transcript.glossary.used),
            terms(&transcript.glossary.cut)
        ));
    }
    for segment in &transcript.segments {
        let reason = match &segment.reason {
            Some(reason) => format!(" ({})", name(reason)),
            None => String::new(),
        };
        let speakers_reason = match &segment.speakers_reason {
            Some(reason) => format!(" ({})", name(reason)),
            None => String::new(),
        };
        lines.push(format!(
            "  segment: {}  {}{reason}  lines {}  speakers: {}{speakers_reason}",
            segment.segment_id,
            name(&segment.status),
            segment.lines,
            name(&segment.speakers)
        ));
    }
    lines
}

fn line_lines(line: &TranscriptLine) -> Vec<String> {
    let voice = match line.speaker {
        Some(number) => format!("  speaker {number}"),
        None => String::new(),
    };
    let mut lines = vec![
        format!(
            "line: {}  {}-{}  {}  account {}{voice}  {}",
            line.line,
            clock(line.start_ms),
            clock(line.end_ms),
            name(&line.source),
            line.account_id,
            shown(&line.language)
        ),
        format!("  {}", shown(&line.text)),
    ];
    if let Some(original) = &line.original_text {
        let by = match (line.corrected_by, line.corrected_at) {
            (Some(by), Some(at)) => format!("corrected by {by} at {at}"),
            _ => "corrected".to_owned(),
        };
        lines.push(format!("  {by}; the worker wrote: {}", shown(original)));
    }
    lines
}

/// The directory of `--to`, or the directory in which the tool runs.
fn directory(arguments: &Arguments) -> PathBuf {
    PathBuf::from(arguments.value("to").unwrap_or("."))
}

async fn saved(download: RecordingDownload) -> Result<Vec<String>, Failure> {
    let saved = download
        .finished()
        .await
        .map_err(|failure| Failure::Failed(format!("{failure:?}")))?;
    Ok(vec![format!("saved: {}", saved.display())])
}

async fn perform(
    server: &ServerHandle,
    words: &[String],
    arguments: &Arguments,
) -> Result<Vec<String>, Failure> {
    let api = server.api();
    let recording = |position| identifier::<RecordingId>(words, position, "the recording");
    Ok(match word(words, 0, "the action of recordings")? {
        "list" => {
            let workspace_id = identifier(words, 1, "the workspace")?;
            let before = arguments
                .value("before")
                .map(|id| id.parse().map_err(|_| usage("--before needs a recording")))
                .transpose()?;
            let page = api
                .recordings(workspace_id, before, arguments.number("limit")?)
                .await
                .map_err(failed)?;
            let mut lines: Vec<String> = page.recordings.iter().flat_map(recording_lines).collect();
            lines.push(format!("recordings: {}", page.recordings.len()));
            if let Some(next) = page.next_before {
                lines.push(format!("continue with: --before {next}"));
            }
            lines
        }
        "show" => {
            let detail = api.recording(recording(1)?).await.map_err(failed)?;
            let mut lines = recording_lines(&detail.recording);
            lines.extend(detail.segments.iter().flat_map(segment_lines));
            lines
        }
        "remove" => {
            api.remove_recording(recording(1)?).await.map_err(failed)?;
            vec!["done".to_owned()]
        }
        "transcript" => transcript_lines(&api.transcript(recording(1)?).await.map_err(failed)?),
        "request-transcript" => {
            let asked = api.request_transcript(recording(1)?).await;
            recording_lines(&asked.map_err(failed)?)
        }
        "lines" => {
            let after = arguments.number("after")?;
            let page = api
                .transcript_lines(recording(1)?, after, arguments.number("limit")?)
                .await
                .map_err(failed)?;
            let mut lines: Vec<String> = page.lines.iter().flat_map(line_lines).collect();
            lines.push(format!("lines: {}", page.lines.len()));
            if let Some(next) = page.next_after {
                lines.push(format!("continue with: --after {next}"));
            }
            lines
        }
        "text" => {
            let recording_id = recording(1)?;
            if arguments.value("to").is_some() {
                let stem = format!("transcript {recording_id}");
                let transfers = server.transfers();
                let download =
                    transfers.download_transcript_text(recording_id, &stem, directory(arguments));
                return saved(download).await;
            }
            let text = api.transcript_text(recording_id).await.map_err(failed)?;
            // The text has one line for each line of the transcript.
            text.lines().map(shown).collect()
        }
        "correct" => {
            let position = word(words, 2, "the line")?
                .parse()
                .map_err(|_| usage("the line is a whole number"))?;
            let text = arguments
                .value("text")
                .ok_or_else(|| usage("--text <corrected text> is missing"))?;
            let corrected = api.correct_transcript_line(recording(1)?, position, text, None);
            line_lines(&corrected.await.map_err(failed)?)
        }
        "rename-speaker" => {
            let number = word(words, 2, "the number of the speaker")?
                .parse()
                .map_err(|_| usage("the number of the speaker is a whole number"))?;
            let name = arguments
                .value("name")
                .ok_or_else(|| usage("--name <name of the voice> is missing"))?;
            let renamed = api.rename_transcript_speaker(recording(1)?, number, name, None);
            vec![speaker_line(&renamed.await.map_err(failed)?)]
        }
        "download" => {
            let recording_id = recording(1)?;
            let segment_id: RecordingSegmentId = identifier(words, 2, "the segment")?;
            let detail = api.recording(recording_id).await.map_err(failed)?;
            let segment = detail
                .segments
                .iter()
                .find(|segment| segment.id == segment_id)
                .ok_or_else(|| Failure::Failed("the recording has no such segment".to_owned()))?;
            let stem = format!("recording {recording_id} {segment_id}");
            let download = server
                .transfers()
                .download_segment(recording_id, segment, &stem, directory(arguments))
                .ok_or_else(|| Failure::Failed("the segment has no file".to_owned()))?;
            saved(download).await?
        }
        other => return Err(usage(format!("unknown action of recordings: {other}"))),
    })
}

/// `recordings`: one action in one process.
pub(crate) async fn run(client: &Client, arguments: &Arguments) -> Result<(), Failure> {
    let words = arguments.positional.get(1..).unwrap_or_default();
    // An action that the tool does not know is told before the server is contacted.
    let action = word(words, 0, "the action of recordings")?;
    if !ACTIONS.contains(&action) {
        return Err(usage(format!("unknown action of recordings: {action}")));
    }
    let server = logged_in(client, arguments).await?;
    let lines = perform(&server, words, arguments).await;
    end_session(&server).await;
    for line in lines? {
        crate::live::say(&line);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use junto_client_core::protocol::ids::{AccountId, CallId, ChannelId, WorkspaceId};
    use junto_client_core::protocol::recording::{
        FileCheck, FileProblem, RecordingState, SegmentFile, SegmentSource, SegmentState,
    };
    use junto_client_core::protocol::transcript::{LineSource, TranscriptState};
    use time::OffsetDateTime;

    use super::*;

    #[test]
    fn a_time_of_the_timeline_is_written_with_its_milliseconds() {
        assert_eq!(clock(0), "00:00:00.000");
        assert_eq!(clock(3_671_900), "01:01:11.900");
    }

    #[test]
    fn a_recording_and_a_segment_are_written_with_what_was_saved() {
        let recording = Recording {
            id: RecordingId::generate(),
            workspace_id: WorkspaceId::generate(),
            channel_id: ChannelId::generate(),
            call_id: CallId::generate(),
            state: RecordingState::PartlyFailed,
            started_by: AccountId::generate(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
            recording_since: None,
            stop: None,
            ended_at: None,
            failure: None,
            language: "auto".to_owned(),
            segments: 3,
            failed_segments: 1,
            bytes: Some(4_096),
            timeline_started_at: None,
            transcript: TranscriptState::Waiting,
            transcript_lines: 0,
        };
        let lines = recording_lines(&recording);
        assert!(lines[0].contains("state: partly_failed"), "{lines:?}");
        assert_eq!(
            lines[1],
            "  segments: 3 (1 failed)  bytes: 4096  stop: none"
        );
        assert_eq!(lines[2], "  transcript: waiting  lines: 0  language: auto");

        let mut segment = RecordingSegment {
            id: RecordingSegmentId::generate(),
            source: SegmentSource::Composite,
            account_id: None,
            state: SegmentState::Ended,
            failure: None,
            requested_at: OffsetDateTime::UNIX_EPOCH,
            started_at: None,
            ended_at: None,
            duration_ms: None,
            file: Some(SegmentFile {
                media_type: "video/mp4".to_owned(),
                bytes: 2_048,
                duration_ms: 2_000,
                starts_at: None,
                width: Some(320),
                height: Some(180),
                check: FileCheck::Failed,
                problems: vec![FileProblem::PictureBlank, FileProblem::Unknown],
                silent: true,
                recovered: false,
            }),
        };
        let lines = segment_lines(&segment);
        assert!(lines[0].ends_with("composite  state: ended"), "{lines:?}");
        assert_eq!(
            lines[1],
            "    file: video/mp4  2048 bytes  2000 ms  check: failed  \
             problems: picture_blank, unknown  silent"
        );
        segment.file = None;
        assert_eq!(segment_lines(&segment)[1], "    file: none");
    }

    #[test]
    fn a_corrected_line_names_who_corrected_it_and_the_text_of_the_worker() {
        let by = AccountId::generate();
        let mut line = TranscriptLine {
            line: 4,
            start_ms: 1_000,
            end_ms: 1_900,
            segment_id: RecordingSegmentId::generate(),
            source: LineSource::Shared,
            account_id: AccountId::generate(),
            speaker: Some(2),
            text: "eleven\u{1b}[2J seconds".to_owned(),
            language: "en".to_owned(),
            original_text: None,
            corrected_by: None,
            corrected_at: None,
        };
        let lines = line_lines(&line);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with("line: 4  00:00:01.000-00:00:01.900  shared  account "));
        assert!(lines[0].ends_with("  speaker 2  en"), "{lines:?}");
        // A control character of a text from the server is not printed.
        assert_eq!(lines[1], "  eleven[2J seconds");

        line.original_text = Some("a leaven seconds".to_owned());
        line.corrected_by = Some(by);
        line.corrected_at = Some(OffsetDateTime::UNIX_EPOCH);
        let lines = line_lines(&line);
        assert_eq!(
            lines[2],
            format!(
                "  corrected by {by} at {}; the worker wrote: a leaven seconds",
                OffsetDateTime::UNIX_EPOCH
            )
        );
    }
}
