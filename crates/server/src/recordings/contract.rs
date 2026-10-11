//! The job file that the server writes for the transcription worker and the transcript file
//! that it reads (ADR 0611). The schemas in `workers/transcription` are the reference; the
//! worker and the server change in the same release.
//!
//! The worker runs apart from the server and reads audio that people sent. The server
//! therefore accepts a transcript only when every part of it agrees with the job.

use std::collections::HashMap;

use junto_protocol::transcript::{
    LineSource, SegmentReason, SegmentTranscription, SpeakerSeparation, SpeakersReason,
};
use serde::de::IgnoredAny;
use serde::{Deserialize, Serialize};

pub const FORMAT_VERSION: u32 = 1;

/// The longest text of a line that the server stores.
pub const LONGEST_LINE: usize = 4000;
/// The most lines of one transcript that the server stores.
pub const MOST_LINES: usize = 200_000;
/// The highest number of a speaker label that the server accepts.
const MOST_SPEAKERS: u32 = 10_000;
/// The latest time on the timeline, and the longest audio of one segment, that the server
/// accepts, in milliseconds: the latest start of a segment in a job (seven days) and the
/// longest audio of a job of the worker (one day).
const LATEST_MS: u64 = 8 * 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Participant {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobSegment {
    pub id: String,
    pub path: String,
    pub kind: LineSource,
    pub participant: Participant,
    pub start_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Job {
    pub format_version: u32,
    pub recording_id: String,
    pub language: String,
    pub glossary: Vec<String>,
    pub separate_speakers: bool,
    pub segments: Vec<JobSegment>,
}

/// True for a glossary term that the job format accepts: 1 to 200 characters, no control
/// character, no space at its start or end, and not the written form of a control token of
/// the speech model.
pub fn is_term(term: &str) -> bool {
    let outer_space = |character: Option<char>| character.is_none_or(char::is_whitespace);
    (1..=200).contains(&term.chars().count())
        && !term.chars().any(is_control)
        && !outer_space(term.chars().next())
        && !outer_space(term.chars().next_back())
        && !term.contains("<|")
        && !term.contains("|>")
}

/// The control characters that the job format refuses in every string.
fn is_control(character: char) -> bool {
    matches!(character, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}')
}

/// True for a text that the job format accepts as an identifier or a name.
pub fn is_text(text: &str) -> bool {
    (1..=200).contains(&text.chars().count()) && !text.chars().any(is_control)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenSource {
    kind: LineSource,
    participant_id: String,
    #[serde(default)]
    speaker: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenLine {
    start_ms: u64,
    end_ms: u64,
    segment_id: String,
    source: WrittenSource,
    text: String,
    language: String,
    #[allow(dead_code)]
    words: IgnoredAny,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenReason<Code> {
    code: Code,
    #[allow(dead_code)]
    message: IgnoredAny,
}

/// The four states of a segment that the worker writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum WrittenStatus {
    Ok,
    Empty,
    Damaged,
    Failed,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenSegment {
    id: String,
    status: WrittenStatus,
    reason: Option<WrittenReason<SegmentReason>>,
    audio_ms: u64,
    processed_ms: u64,
    #[allow(dead_code)]
    speech_ms: u64,
    #[allow(dead_code)]
    clips: u64,
    lines: u32,
    speakers: SpeakerSeparation,
    speakers_reason: Option<WrittenReason<SpeakersReason>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenGlossary {
    #[allow(dead_code)]
    token_limit: u64,
    #[allow(dead_code)]
    token_count: u64,
    used: Vec<String>,
    cut: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenSpeaker {
    label: String,
    participant_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WrittenSeparation {
    #[allow(dead_code)]
    requested: bool,
    speakers: Vec<WrittenSpeaker>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Written {
    format_version: u32,
    recording_id: String,
    language: String,
    #[allow(dead_code)]
    participants: Vec<Participant>,
    lines: Vec<WrittenLine>,
    segments: Vec<WrittenSegment>,
    glossary: WrittenGlossary,
    speaker_separation: WrittenSeparation,
    #[allow(dead_code)]
    engine: IgnoredAny,
    #[allow(dead_code)]
    timing: IgnoredAny,
}

/// One line of an accepted transcript. `segment` is the position of its segment in the job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub start_ms: u64,
    pub end_ms: u64,
    pub segment: usize,
    pub speaker: Option<u32>,
    pub text: String,
    pub language: String,
}

/// The result of one segment of the job, in the order of the job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentResult {
    pub status: SegmentTranscription,
    pub reason: Option<SegmentReason>,
    pub audio_ms: u64,
    pub processed_ms: u64,
    pub lines: u32,
    pub speakers: SpeakerSeparation,
    pub speakers_reason: Option<SpeakersReason>,
}

/// A transcript that agrees with its job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transcript {
    pub lines: Vec<Line>,
    pub segments: Vec<SegmentResult>,
    pub glossary_used: Vec<String>,
    pub glossary_cut: Vec<String>,
    /// Each speaker label with the position, in the job, of a segment of the participant
    /// whose shared sound it belongs to.
    pub speakers: Vec<(u32, usize)>,
}

/// The number of the label `Speaker N`.
fn speaker_number(label: &str) -> Option<u32> {
    let digits = label.strip_prefix("Speaker ")?;
    let plain = !digits.starts_with('0') && digits.bytes().all(|byte| byte.is_ascii_digit());
    plain
        .then(|| digits.parse::<u32>().ok())
        .flatten()
        .filter(|number| (1..=MOST_SPEAKERS).contains(number))
}

fn is_language(text: &str) -> bool {
    (2..=3).contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_lowercase())
}

/// Reads the transcript that the worker wrote for `job`. The error states the first rule
/// that the file breaks, without a value of the file.
pub fn read(bytes: &[u8], job: &Job) -> Result<Transcript, String> {
    let written: Written = serde_json::from_slice(bytes)
        .map_err(|error| format!("not a transcript: {:?} error", error.classify()))?;
    let refuse = |rule: &str| Err(rule.to_owned());
    if written.format_version != FORMAT_VERSION {
        return refuse("another format version");
    }
    if written.recording_id != job.recording_id || written.language != job.language {
        return refuse("the transcript of another job");
    }
    if written.segments.len() != job.segments.len() {
        return refuse("another number of segments than the job");
    }
    let mut glossary = written.glossary.used.clone();
    glossary.extend(written.glossary.cut.iter().cloned());
    if glossary != job.glossary {
        return refuse("another glossary than the job");
    }

    let mut segments = Vec::with_capacity(written.segments.len());
    for (result, asked) in written.segments.iter().zip(&job.segments) {
        if result.id != asked.id {
            return refuse("segments in another order than the job");
        }
        let reason = result.reason.as_ref().map(|reason| reason.code);
        let speakers_reason = result.speakers_reason.as_ref().map(|reason| reason.code);
        let unknown = reason == Some(SegmentReason::Unknown)
            || speakers_reason == Some(SpeakersReason::Unknown);
        let own_reasons = [
            SegmentReason::ShorterThanFile,
            SegmentReason::NoFile,
            SegmentReason::NoStartTime,
        ];
        if unknown || reason.is_some_and(|reason| own_reasons.contains(&reason)) {
            return refuse("a reason that the contract does not have");
        }
        let (status, needs_reason) = match result.status {
            WrittenStatus::Ok => (SegmentTranscription::Ok, false),
            WrittenStatus::Empty => (SegmentTranscription::Empty, false),
            WrittenStatus::Damaged => (SegmentTranscription::Damaged, true),
            WrittenStatus::Failed => (SegmentTranscription::Failed, true),
        };
        let labels_missing = matches!(
            result.speakers,
            SpeakerSeparation::Unassigned | SpeakerSeparation::Failed
        );
        if reason.is_some() != needs_reason || speakers_reason.is_some() != labels_missing {
            return refuse("a status without its reason, or a reason without its status");
        }
        let microphone = asked.kind == LineSource::Microphone;
        if microphone != (result.speakers == SpeakerSeparation::NotApplicable) {
            return refuse("a speaker result that does not fit the kind of the segment");
        }
        if result.processed_ms > result.audio_ms {
            return refuse("lines beyond the decoded audio");
        }
        if result.audio_ms > LATEST_MS {
            return refuse("more audio than a job can hold");
        }
        segments.push(SegmentResult {
            status,
            reason,
            audio_ms: result.audio_ms,
            processed_ms: result.processed_ms,
            lines: result.lines,
            speakers: result.speakers,
            speakers_reason,
        });
    }

    let position: HashMap<&str, usize> = job
        .segments
        .iter()
        .enumerate()
        .map(|(position, segment)| (segment.id.as_str(), position))
        .collect();
    let mut speakers: Vec<(u32, usize)> = Vec::new();
    for speaker in &written.speaker_separation.speakers {
        let Some(number) = speaker_number(&speaker.label) else {
            return refuse("a speaker label of another form");
        };
        let shared = job.segments.iter().position(|segment| {
            segment.kind == LineSource::Shared && segment.participant.id == speaker.participant_id
        });
        let Some(shared) = shared else {
            return refuse("a speaker of a participant without shared sound");
        };
        if speakers.iter().any(|(known, _)| *known == number) {
            return refuse("a speaker label twice");
        }
        speakers.push((number, shared));
    }

    if written.lines.len() > MOST_LINES {
        return refuse("more lines than the server stores");
    }
    let mut counted = vec![0u32; job.segments.len()];
    let mut lines = Vec::with_capacity(written.lines.len());
    let mut latest = 0;
    for line in written.lines {
        let Some(&segment) = position.get(line.segment_id.as_str()) else {
            return refuse("a line of a segment that the job does not have");
        };
        let asked = &job.segments[segment];
        if line.source.kind != asked.kind || line.source.participant_id != asked.participant.id {
            return refuse("a line whose source is not the source of its segment");
        }
        let speaker = match line.source.speaker.as_deref() {
            None => None,
            Some(label) => {
                let number = speaker_number(label);
                let listed =
                    number.and_then(|number| speakers.iter().find(|(known, _)| *known == number));
                let of_participant = listed.is_some_and(|(_, shared)| {
                    job.segments[*shared].participant.id == asked.participant.id
                });
                if asked.kind != LineSource::Shared || !of_participant {
                    return refuse("a line with a speaker label that the transcript does not list");
                }
                number
            }
        };
        if line.end_ms < line.start_ms || line.start_ms < latest {
            return refuse("lines that are not in the order of the timeline");
        }
        if line.end_ms > LATEST_MS {
            return refuse("a line later than a job can hold");
        }
        latest = line.start_ms;
        let length = line.text.chars().count();
        if length == 0 || length > LONGEST_LINE || line.text.chars().any(is_control) {
            return refuse("a line whose text the server does not store");
        }
        if !is_language(&line.language) {
            return refuse("a line without a language code");
        }
        counted[segment] += 1;
        lines.push(Line {
            start_ms: line.start_ms,
            end_ms: line.end_ms,
            segment,
            speaker,
            text: line.text,
            language: line.language,
        });
    }
    for (result, counted) in segments.iter().zip(&counted) {
        let has_lines = *counted > 0;
        let may_have_lines = matches!(
            result.status,
            SegmentTranscription::Ok | SegmentTranscription::Damaged
        );
        let must_have_lines = result.status == SegmentTranscription::Ok;
        if result.lines != *counted
            || (has_lines && !may_have_lines)
            || (must_have_lines && !has_lines)
        {
            return refuse("a segment whose lines do not agree with its result");
        }
    }
    Ok(Transcript {
        lines,
        segments,
        glossary_used: written.glossary.used,
        glossary_cut: written.glossary.cut,
        speakers,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::{Value, json};

    use super::*;

    pub(crate) fn job() -> Job {
        let participant = |id: &str| Participant {
            id: id.to_owned(),
            name: format!("Name {id}"),
        };
        Job {
            format_version: FORMAT_VERSION,
            recording_id: "rec-1".to_owned(),
            language: "auto".to_owned(),
            glossary: vec!["Quorvex".to_owned(), "Lumitrax".to_owned()],
            separate_speakers: true,
            segments: vec![
                JobSegment {
                    id: "seg-1".to_owned(),
                    path: "seg-1.ogg".to_owned(),
                    kind: LineSource::Microphone,
                    participant: participant("ann"),
                    start_ms: 0,
                },
                JobSegment {
                    id: "seg-2".to_owned(),
                    path: "seg-2.ogg".to_owned(),
                    kind: LineSource::Shared,
                    participant: participant("ann"),
                    start_ms: 75_200,
                },
            ],
        }
    }

    pub(crate) fn transcript() -> Value {
        json!({
            "format_version": 1,
            "recording_id": "rec-1",
            "language": "auto",
            "participants": [{"id": "ann", "name": "Name ann"}],
            "lines": [
                {"start_ms": 1180, "end_ms": 3240, "segment_id": "seg-1",
                 "source": {"kind": "microphone", "participant_id": "ann"},
                 "text": "Can everyone see my screen now?", "language": "en",
                 "words": [{"start_ms": 1180, "end_ms": 1400, "text": " Can"}]},
                {"start_ms": 76020, "end_ms": 80110, "segment_id": "seg-2",
                 "source": {"kind": "shared", "participant_id": "ann", "speaker": "Speaker 1"},
                 "text": "Thanks for joining.", "language": "en", "words": null},
                {"start_ms": 81000, "end_ms": 82000, "segment_id": "seg-2",
                 "source": {"kind": "shared", "participant_id": "ann", "speaker": null},
                 "text": "Yes.", "language": "pt", "words": null}
            ],
            "segments": [
                {"id": "seg-1", "status": "ok", "reason": null, "audio_ms": 60000,
                 "processed_ms": 60000, "speech_ms": 2100, "clips": 1, "lines": 1,
                 "speakers": "not_applicable", "speakers_reason": null},
                {"id": "seg-2", "status": "damaged",
                 "reason": {"code": "decode_incomplete", "message": "decoding stopped"},
                 "audio_ms": 30000, "processed_ms": 30000, "speech_ms": 4000, "clips": 2,
                 "lines": 2, "speakers": "unassigned",
                 "speakers_reason": {"code": "no_voice_window", "message": "too short"}}
            ],
            "glossary": {"token_limit": 200, "token_count": 9, "used": ["Quorvex"],
                         "cut": ["Lumitrax"]},
            "speaker_separation": {"requested": true,
                "speakers": [{"label": "Speaker 1", "participant_id": "ann"}]},
            "engine": {"worker": "0.1.0", "device": "cpu", "compute_type": "int8", "threads": 4,
                       "models": [], "libraries": {}, "settings": {}},
            "timing": {"total_ms": 9100, "audio_ms": 90000, "speech_ms": 6100, "stages_ms": {},
                       "peak_memory_mb": null}
        })
    }

    fn reading(document: &Value) -> Result<Transcript, String> {
        read(&serde_json::to_vec(document).unwrap(), &job())
    }

    #[test]
    fn the_job_is_written_in_the_form_of_the_schema() {
        let written = serde_json::to_value(job()).unwrap();
        assert_eq!(
            written,
            json!({
                "format_version": 1,
                "recording_id": "rec-1",
                "language": "auto",
                "glossary": ["Quorvex", "Lumitrax"],
                "separate_speakers": true,
                "segments": [
                    {"id": "seg-1", "path": "seg-1.ogg", "kind": "microphone",
                     "participant": {"id": "ann", "name": "Name ann"}, "start_ms": 0},
                    {"id": "seg-2", "path": "seg-2.ogg", "kind": "shared",
                     "participant": {"id": "ann", "name": "Name ann"}, "start_ms": 75200}
                ]
            })
        );
    }

    #[test]
    fn the_example_of_the_contract_is_read() {
        let read = reading(&transcript()).unwrap();
        assert_eq!(read.lines.len(), 3);
        assert_eq!(
            read.lines[1],
            Line {
                start_ms: 76_020,
                end_ms: 80_110,
                segment: 1,
                speaker: Some(1),
                text: "Thanks for joining.".to_owned(),
                language: "en".to_owned(),
            }
        );
        assert_eq!((read.lines[0].segment, read.lines[0].speaker), (0, None));
        assert_eq!(read.lines[2].speaker, None);
        assert_eq!(read.segments[0].status, SegmentTranscription::Ok);
        assert_eq!(
            (read.segments[1].status, read.segments[1].reason),
            (
                SegmentTranscription::Damaged,
                Some(SegmentReason::DecodeIncomplete)
            )
        );
        assert_eq!(
            (read.segments[1].speakers, read.segments[1].speakers_reason),
            (
                SpeakerSeparation::Unassigned,
                Some(SpeakersReason::NoVoiceWindow)
            )
        );
        assert_eq!(read.glossary_used, ["Quorvex"]);
        assert_eq!(read.glossary_cut, ["Lumitrax"]);
        assert_eq!(read.speakers, [(1, 1)]);
    }

    type Change = Box<dyn Fn(&mut Value)>;

    #[test]
    fn a_transcript_that_does_not_agree_with_its_job_is_refused() {
        let cases: Vec<(&str, Change)> = vec![
            ("version", Box::new(|t| t["format_version"] = json!(2))),
            (
                "recording",
                Box::new(|t| t["recording_id"] = json!("rec-2")),
            ),
            ("language", Box::new(|t| t["language"] = json!("en"))),
            ("unknown field", Box::new(|t| t["extra"] = json!(1))),
            (
                "unknown field of a line",
                Box::new(|t| t["lines"][0]["extra"] = json!(1)),
            ),
            (
                "segment missing",
                Box::new(|t| {
                    t["segments"].as_array_mut().unwrap().pop();
                }),
            ),
            (
                "segment order",
                Box::new(|t| t["segments"].as_array_mut().unwrap().reverse()),
            ),
            (
                "line of another segment",
                Box::new(|t| t["lines"][0]["segment_id"] = json!("seg-9")),
            ),
            (
                "line of another participant",
                Box::new(|t| t["lines"][0]["source"]["participant_id"] = json!("ben")),
            ),
            (
                "kind of the source",
                Box::new(|t| t["lines"][0]["source"]["kind"] = json!("shared")),
            ),
            (
                "label on a microphone",
                Box::new(|t| t["lines"][0]["source"]["speaker"] = json!("Speaker 1")),
            ),
            (
                "label that is not listed",
                Box::new(|t| t["lines"][1]["source"]["speaker"] = json!("Speaker 2")),
            ),
            (
                "label of another form",
                Box::new(|t| t["speaker_separation"]["speakers"][0]["label"] = json!("Speaker 01")),
            ),
            (
                "speaker of a participant without shared sound",
                Box::new(|t| {
                    t["speaker_separation"]["speakers"][0]["participant_id"] = json!("ben");
                }),
            ),
            (
                "order of the lines",
                Box::new(|t| t["lines"][2]["start_ms"] = json!(100)),
            ),
            (
                "end before start",
                Box::new(|t| t["lines"][0]["end_ms"] = json!(10)),
            ),
            (
                "empty text",
                Box::new(|t| t["lines"][0]["text"] = json!("")),
            ),
            (
                "control character",
                Box::new(|t| t["lines"][0]["text"] = json!("a\u{7}b")),
            ),
            (
                "long text",
                Box::new(|t| t["lines"][0]["text"] = json!("a".repeat(LONGEST_LINE + 1))),
            ),
            (
                "language of a line",
                Box::new(|t| t["lines"][0]["language"] = json!("English")),
            ),
            (
                "line count",
                Box::new(|t| t["segments"][0]["lines"] = json!(2)),
            ),
            (
                "ok without a line",
                Box::new(|t| {
                    t["lines"].as_array_mut().unwrap().remove(0);
                    t["segments"][0]["lines"] = json!(0);
                }),
            ),
            (
                "failed with a line",
                Box::new(|t| {
                    t["segments"][0]["status"] = json!("failed");
                    t["segments"][0]["reason"] = json!({"code": "decode_failed", "message": "x"});
                }),
            ),
            (
                "status without reason",
                Box::new(|t| t["segments"][1]["reason"] = Value::Null),
            ),
            (
                "reason without status",
                Box::new(|t| {
                    t["segments"][0]["reason"] = json!({"code": "time_limit", "message": "x"});
                }),
            ),
            (
                "unknown reason",
                Box::new(|t| t["segments"][1]["reason"]["code"] = json!("too_quiet")),
            ),
            (
                "reason of the server",
                Box::new(|t| t["segments"][1]["reason"]["code"] = json!("no_file")),
            ),
            (
                "speaker result of a microphone",
                Box::new(|t| t["segments"][0]["speakers"] = json!("not_requested")),
            ),
            (
                "labels missing without reason",
                Box::new(|t| t["segments"][1]["speakers_reason"] = Value::Null),
            ),
            (
                "lines beyond the audio",
                Box::new(|t| t["segments"][1]["processed_ms"] = json!(30001)),
            ),
            ("glossary", Box::new(|t| t["glossary"]["cut"] = json!([]))),
            (
                "audio longer than a job can hold",
                Box::new(|t| t["segments"][0]["audio_ms"] = json!(i64::MAX)),
            ),
            (
                "a line later than a job can hold",
                Box::new(|t| {
                    t["lines"][2]["start_ms"] = json!(u64::MAX - 1);
                    t["lines"][2]["end_ms"] = json!(u64::MAX);
                }),
            ),
        ];
        for (name, change) in cases {
            let mut document = transcript();
            change(&mut document);
            assert!(reading(&document).is_err(), "{name}");
        }
        assert!(read(b"{", &job()).is_err());
        assert!(read(b"[]", &job()).is_err());
    }

    #[test]
    fn a_refusal_states_no_value_of_the_file() {
        let mut document = transcript();
        document["lines"][0]["text"] = json!(12);
        let refusal = reading(&document).unwrap_err();
        assert_eq!(refusal, "not a transcript: Data error");
        assert_eq!(
            read(b"{\"secret", &job()).unwrap_err(),
            "not a transcript: Eof error"
        );
    }

    #[test]
    fn glossary_terms_and_names_follow_the_job_schema() {
        for term in ["Quorvex", "Ysolde Varkonen", "é", &"a".repeat(200)] {
            assert!(is_term(term), "{term:?}");
        }
        for term in [
            "",
            " Quorvex",
            "Quorvex ",
            "a\tb",
            "a\u{85}b",
            "<|endoftext|>",
            "a|>b",
            &"a".repeat(201),
        ] {
            assert!(!is_term(term), "{term:?}");
        }
        assert!(is_text("Ann Example"));
        assert!(!is_text(""));
        assert!(!is_text("line\nbreak"));
        assert_eq!(speaker_number("Speaker 12"), Some(12));
        for label in [
            "Speaker 0",
            "Speaker 01",
            "speaker 1",
            "Speaker",
            "Speaker 1 ",
        ] {
            assert_eq!(speaker_number(label), None, "{label:?}");
        }
    }
}
