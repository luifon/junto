//! The hand-over of a recording to the transcription worker and the routes of transcripts
//! (ADR 0603, ADR 0607, ADR 0611). The worker is a stand-in process that follows the
//! contract of the job file and the transcript file.

use junto_protocol::audit::AuditEvent;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::RecordingId;
use junto_protocol::recording::{
    Recording, RecordingDetail, RecordingSegment, RecordingState, SegmentSource,
};
use junto_protocol::transcript::{
    LineSource, SegmentReason, SegmentTranscription, SpeakerSeparation, SpeakersReason,
    TranscriptFailure, TranscriptLine, TranscriptLinePage, TranscriptSpeaker, TranscriptState,
};
use junto_server::media::{TrackKind, TrackSource};
use junto_server::recorder::samples;
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::Duration;

use crate::recordings_support::{FakeWorker, Said, Studio, parts, transcript_of};
use crate::support::{Member, TestServer};

fn length() -> Duration {
    Duration::seconds(i64::from(samples::SECONDS))
}

/// The audio segments of a recording that have a file, in the order of the job.
fn audio(detail: &RecordingDetail) -> Vec<&RecordingSegment> {
    detail
        .segments
        .iter()
        .filter(|segment| segment.source != SegmentSource::Composite && segment.file.is_some())
        .collect()
}

fn two_lines(detail: &RecordingDetail) -> Value {
    let said = [
        Said(0, 500, 1500, "Good morning everyone.", None),
        Said(1, 700, 1900, "Can you hear me?", None),
    ];
    transcript_of(detail, (&[], &[]), &said)
}

fn transcript_states(events: &[(&'static str, Recording)]) -> Vec<TranscriptState> {
    events
        .iter()
        .map(|(_, recording)| recording.transcript)
        .collect()
}

async fn state_of(server: &TestServer, member: &Member, id: RecordingId) -> TranscriptState {
    server.recording(member, id).await.recording.transcript
}

fn path(id: RecordingId, rest: &str) -> String {
    format!("/api/v1/recordings/{id}/transcript{rest}")
}

#[tokio::test]
async fn a_processed_recording_is_handed_to_the_worker_and_its_transcript_is_stored() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let glossary = format!("/api/v1/workspaces/{}/glossary", scene.workspace_id);
    server
        .put(
            owner,
            &glossary,
            &json!({ "terms": ["Quorvex", "Lumitrax"] }),
        )
        .await
        .ok::<Value>(StatusCode::OK);

    let done = studio.finished_after(length()).await;
    assert_eq!(done.recording.state, RecordingState::Ready);
    assert_eq!(done.recording.transcript, TranscriptState::Waiting);
    assert_eq!(done.recording.transcript_lines, 0);
    let id = done.recording.id;
    let said = [
        Said(0, 500, 1500, "Good morning everyone.", None),
        Said(1, 700, 1900, "Can you hear me?", None),
    ];
    let written = transcript_of(&done, (&["Quorvex"], &["Lumitrax"]), &said);
    worker.give(&written);
    let mut stream = server.full_stream(owner).await;

    assert!(server.transcribe().await);
    assert!(!server.transcribe().await);

    // The end of the transcript has one line in the chat of the voice channel, and the card
    // of the recording and that line count as unread for the person who started the
    // recording (ADR 0408).
    let notes = server.notes(scene.voice.id).await;
    assert_eq!(
        notes
            .iter()
            .filter(|kind| kind.as_str() == "transcript_ready")
            .count(),
        1,
        "{notes:?}"
    );
    let counted: Vec<String> = sqlx::query_scalar(
        "select system->>'action' from messages where notifies = $1 and place_id = $2
         order by position",
    )
    .bind(done.recording.started_by.as_uuid())
    .bind(scene.voice.id.as_uuid())
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert_eq!(counted, ["recording_ready", "transcript_ready"]);

    // The job file (ADR 0611).
    let tracks = audio(&done);
    let timeline = done.recording.timeline_started_at.unwrap();
    let job = worker.job();
    assert_eq!(job["format_version"], 1);
    assert_eq!(job["recording_id"], json!(id));
    assert_eq!(job["language"], "auto");
    assert_eq!(job["glossary"], json!(["Quorvex", "Lumitrax"]));
    assert_eq!(job["separate_speakers"], false);
    let segments = job["segments"].as_array().unwrap();
    assert_eq!(segments.len(), 2);
    for (segment, track) in segments.iter().zip(&tracks) {
        let name = if track.account_id == Some(ann.account_id) {
            "Ann"
        } else {
            "Ben"
        };
        let starts_at = track.file.as_ref().unwrap().starts_at.unwrap();
        assert_eq!(
            *segment,
            json!({
                "id": track.id,
                "path": format!("{}.ogg", track.id),
                "kind": "microphone",
                "participant": {"id": track.account_id.unwrap(), "name": name},
                "start_ms": (starts_at - timeline).whole_milliseconds(),
            })
        );
    }
    assert!(segments[0]["start_ms"].as_u64().unwrap() > 0);

    // The run of the worker.
    let directory = server.recordings_dir().join(id.to_string());
    let directory = directory.to_str().unwrap();
    // Each run has a directory of its own below `<recording>/transcription/`.
    let runs = worker.runs();
    assert_eq!(runs.len(), 1);
    let arguments: Vec<&str> = runs[0].split(' ').collect();
    let run_directory = arguments[1].strip_suffix("/job.json").unwrap();
    let run = run_directory
        .strip_prefix(&format!("{directory}/transcription/"))
        .unwrap();
    // The name states the attempt, here the first.
    let unique = run.strip_prefix("1-").unwrap();
    assert!(unique.len() == 32 && unique.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert_eq!(
        arguments,
        [
            "run",
            &format!("{run_directory}/job.json"),
            "--out",
            &format!("{run_directory}/transcript.json"),
            "--audio-root",
            directory,
        ]
    );
    let variables = worker.variables();
    assert!(variables.contains(&"JUNTO_TRANSCRIBE_MODELS".to_owned()));
    assert!(variables.contains(&"PATH".to_owned()));
    assert!(std::env::var_os("JUNTO_TEST_DATABASE_URL").is_some());
    for kept_from_the_worker in ["JUNTO_TEST_DATABASE_URL", "HOME", "USER"] {
        assert!(
            !variables.contains(&kept_from_the_worker.to_owned()),
            "{variables:?}"
        );
    }
    let heard = worker.audio_root();
    for track in &tracks {
        assert!(heard.contains(&format!("{}.ogg", track.id)), "{heard:?}");
    }

    // The states of the transcript are events of the recording.
    let events = server.recording_events(owner, &mut stream).await;
    assert_eq!(
        transcript_states(&events),
        [TranscriptState::Running, TranscriptState::Ready]
    );
    assert!(events.iter().all(|(name, _)| *name == "recording_updated"));
    assert_eq!(events[1].1.transcript_lines, 2);
    let read = server.recording(ann, id).await;
    assert_eq!(read.recording, events[1].1);
    assert_eq!(read.recording.state, RecordingState::Ready);

    let transcript = server.transcript(ann, id).await;
    assert_eq!(transcript.recording_id, id);
    assert_eq!(transcript.state, TranscriptState::Ready);
    assert_eq!(transcript.failure, None);
    assert_eq!(transcript.language, "auto");
    assert_eq!(transcript.lines, 2);
    assert_eq!(transcript.glossary.used, ["Quorvex"]);
    assert_eq!(transcript.glossary.cut, ["Lumitrax"]);
    assert_eq!(transcript.speakers, []);
    assert_eq!(transcript.segments.len(), 2);
    for (result, track) in transcript.segments.iter().zip(&tracks) {
        assert_eq!(result.segment_id, track.id);
        assert_eq!(
            (result.status, result.reason),
            (SegmentTranscription::Ok, None)
        );
        assert_eq!(result.lines, 1);
        assert_eq!(result.speakers, SpeakerSeparation::NotApplicable);
        assert_eq!(
            result.audio_ms,
            Some(track.file.as_ref().unwrap().duration_ms)
        );
    }

    let lines = server.transcript_lines(ann, id, 200).await;
    assert_eq!(lines.len(), 2);
    for (position, (line, Said(segment, start_ms, end_ms, text, _))) in
        lines.iter().zip(&said).enumerate()
    {
        assert_eq!(
            *line,
            TranscriptLine {
                line: u32::try_from(position).unwrap(),
                start_ms: *start_ms,
                end_ms: *end_ms,
                segment_id: tracks[*segment].id,
                source: LineSource::Microphone,
                account_id: tracks[*segment].account_id.unwrap(),
                speaker: None,
                text: (*text).to_owned(),
                language: "en".to_owned(),
                original_text: None,
                corrected_by: None,
                corrected_at: None,
            }
        );
    }

    // The file of the worker is kept with the recording; the directory of the job is gone.
    let kept: Value = serde_json::from_slice(
        &std::fs::read(
            server
                .recordings_dir()
                .join(id.to_string())
                .join("transcript.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(kept, written);
    assert!(
        !server
            .recordings_dir()
            .join(id.to_string())
            .join("transcription")
            .exists()
    );
}

#[tokio::test]
async fn voices_in_shared_sound_get_labels_that_a_person_renames() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    server.publish(
        &studio.ann_seat,
        TrackKind::Audio,
        TrackSource::ScreenShareAudio,
    );
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    let tracks = audio(&done);
    let shared = tracks
        .iter()
        .position(|track| track.source == SegmentSource::ScreenShareAudio)
        .unwrap();
    let microphone = tracks
        .iter()
        .position(|track| track.account_id == Some(ben.account_id))
        .unwrap();
    worker.give(&transcript_of(
        &done,
        (&[], &[]),
        &[
            Said(shared, 100, 900, "Thank you all for joining.", Some(1)),
            Said(microphone, 300, 800, "Hello.", None),
            Said(shared, 1000, 1400, "Can we see the timings?", Some(2)),
            Said(shared, 1500, 1600, "Yes.", None),
        ],
    ));
    assert!(server.transcribe().await);

    let job = worker.job();
    assert_eq!(job["separate_speakers"], true);
    assert_eq!(job["segments"][shared]["kind"], "shared");
    assert_eq!(
        job["segments"][shared]["participant"],
        json!({"id": ann.account_id, "name": "Ann"})
    );

    let transcript = server.transcript(ben, id).await;
    assert_eq!(transcript.state, TranscriptState::Ready);
    let label = |number| TranscriptSpeaker {
        number,
        account_id: ann.account_id,
        name: None,
        renamed_by: None,
        renamed_at: None,
    };
    assert_eq!(transcript.speakers, [label(1), label(2)]);
    let result = &transcript.segments[shared];
    assert_eq!(result.speakers, SpeakerSeparation::Unassigned);
    assert_eq!(result.speakers_reason, Some(SpeakersReason::NoVoiceWindow));
    let lines = server.transcript_lines(ben, id, 200).await;
    let speakers: Vec<_> = lines
        .iter()
        .map(|line| (line.source, line.speaker))
        .collect();
    assert_eq!(
        speakers,
        [
            (LineSource::Shared, Some(1)),
            (LineSource::Microphone, None),
            (LineSource::Shared, Some(2)),
            (LineSource::Shared, None),
        ]
    );
    assert!(
        lines
            .iter()
            .filter(|line| line.source == LineSource::Shared)
            .all(|line| line.account_id == ann.account_id)
    );

    // A rename applies to one label of one recording, names its author and can be repeated
    // (ADR 0603).
    let mut stream = server.full_stream(owner).await;
    let rename = path(id, "/speakers/1");
    let named: TranscriptSpeaker = server
        .put(ben, &rename, &json!({ "name": " Noor " }))
        .await
        .ok(StatusCode::OK);
    assert_eq!(named.name.as_deref(), Some("Noor"));
    assert_eq!(named.renamed_by, Some(ben.account_id));
    assert_eq!(named.renamed_at, Some(server.clock.now()));
    assert_eq!((named.number, named.account_id), (1, ann.account_id));
    let events: Vec<Event> = server
        .traffic(owner, &mut stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .collect();
    assert_eq!(
        events,
        [Event::TranscriptSpeakerRenamed {
            recording_id: id,
            workspace_id: scene.workspace_id,
            channel_id: scene.voice.id,
            speaker: 1,
        }]
    );
    let again: TranscriptSpeaker = server
        .put(ann, &rename, &json!({ "name": "Noor Example" }))
        .await
        .ok(StatusCode::OK);
    assert_eq!(again.renamed_by, Some(ann.account_id));
    let transcript = server.transcript(ben, id).await;
    assert_eq!(transcript.speakers, [again, label(2)]);
    assert_eq!(server.count("transcript_speaker_names").await, 2);

    server
        .put(ben, &path(id, "/speakers/3"), &json!({ "name": "Nobody" }))
        .await
        .error(ErrorCode::NotFound);
    for refused in ["", "   ", &"a".repeat(65), "a\nb"] {
        server
            .put(ben, &rename, &json!({ "name": refused }))
            .await
            .error(ErrorCode::InvalidRequest);
    }

    // A name made from an older name, or from no name when the voice has one, is refused
    // and stores nothing; one made from the name of now is stored.
    let names = server.count("transcript_speaker_names").await;
    for older in [json!("Noor"), json!(null)] {
        server
            .put(ben, &rename, &json!({ "name": "Other", "replaces": older }))
            .await
            .error(ErrorCode::Conflict);
    }
    server
        .put(
            ben,
            &path(id, "/speakers/2"),
            &json!({ "name": "Other", "replaces": "Someone" }),
        )
        .await
        .error(ErrorCode::Conflict);
    assert_eq!(server.count("transcript_speaker_names").await, names);
    let unnamed: TranscriptSpeaker = server
        .put(
            ben,
            &path(id, "/speakers/2"),
            &json!({ "name": "Rafa", "replaces": null }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(unnamed.name.as_deref(), Some("Rafa"));
    server
        .put(
            ben,
            &path(id, "/speakers/2"),
            &json!({ "name": "Speaker 2", "replaces": "Rafa" }),
        )
        .await
        .ok::<TranscriptSpeaker>(StatusCode::OK);

    let text = server.get(ben, &path(id, "/text")).await;
    assert_eq!(text.status, StatusCode::OK);
    assert_eq!(
        text.header("content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(
        text.header("content-disposition"),
        Some(format!("attachment; filename=\"{id}.txt\"").as_str())
    );
    assert_eq!(
        text.text(),
        "[00:00:00] Noor Example (shared audio of Ann): Thank you all for joining.\n\
         [00:00:00] Ben: Hello.\n\
         [00:00:01] Speaker 2 (shared audio of Ann): Can we see the timings?\n\
         [00:00:01] Shared audio of Ann: Yes.\n"
    );
}

#[tokio::test]
async fn a_transcript_of_a_part_of_the_audio_is_partial() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    // The worker read a part of the first file, and reports less audio for the second file
    // than the file holds without calling it damaged (ADR 0611).
    let mut written = two_lines(&done);
    written["segments"][0]["status"] = json!("damaged");
    written["segments"][0]["reason"] =
        json!({"code": "decode_incomplete", "message": "decoding stopped"});
    written["segments"][1]["audio_ms"] = json!(500);
    written["segments"][1]["processed_ms"] = json!(500);
    worker.give(&written);

    assert!(server.transcribe().await);
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Partial);
    let transcript = server.transcript(ann, id).await;
    assert_eq!(transcript.state, TranscriptState::Partial);
    assert_eq!(transcript.lines, 2);
    let results: Vec<_> = transcript
        .segments
        .iter()
        .map(|segment| (segment.status, segment.reason, segment.lines))
        .collect();
    assert_eq!(
        results,
        [
            (
                SegmentTranscription::Damaged,
                Some(SegmentReason::DecodeIncomplete),
                1
            ),
            (
                SegmentTranscription::Damaged,
                Some(SegmentReason::ShorterThanFile),
                1
            ),
        ]
    );
    assert_eq!(server.transcript_lines(ann, id, 200).await.len(), 2);
}

#[tokio::test]
async fn an_audio_segment_without_a_file_is_not_transcribed_and_the_transcript_is_partial() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let started = studio.record().await;
    server.clock.advance(length());
    let stopped = studio.stop(started.id).await;
    let (_, tracks) = parts(&stopped);
    let missing = tracks[0].id;
    let directory = server.recordings_dir().join(started.id.to_string());
    std::fs::remove_file(directory.join(format!("{missing}.ogg"))).unwrap();
    assert_eq!(server.process().await, 1);
    let done = server.recording(ann, started.id).await;
    assert_eq!(done.recording.transcript, TranscriptState::Waiting);

    worker.give(&transcript_of(
        &done,
        (&[], &[]),
        &[Said(0, 500, 1500, "Only one of us is heard.", None)],
    ));
    assert!(server.transcribe().await);
    assert_eq!(worker.job()["segments"].as_array().unwrap().len(), 1);
    let transcript = server.transcript(ann, started.id).await;
    assert_eq!(transcript.state, TranscriptState::Partial);
    let of_missing = transcript
        .segments
        .iter()
        .find(|segment| segment.segment_id == missing)
        .unwrap();
    assert_eq!(of_missing.status, SegmentTranscription::NotTranscribed);
    assert_eq!(of_missing.reason, Some(SegmentReason::NoFile));
    assert_eq!((of_missing.audio_ms, of_missing.lines), (None, 0));
    assert_eq!(transcript.segments.len(), 2);
}

#[tokio::test]
async fn a_refused_job_and_a_worker_that_is_not_ready_fail_without_another_run() {
    for (code, failure) in [
        (2, TranscriptFailure::JobRefused),
        (3, TranscriptFailure::WorkerNotReady),
    ] {
        let worker = FakeWorker::new();
        let studio = Studio::on(worker.server().await).await;
        let scene = &studio.scene;
        let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
        let done = studio.finished_after(length()).await;
        let id = done.recording.id;
        worker.give(&two_lines(&done));
        worker.behave(&format!("exit {code}"));
        let mut stream = server.full_stream(owner).await;

        assert!(server.transcribe().await);
        assert!(!server.transcribe().await);
        assert_eq!(worker.runs().len(), 1);
        let transcript = server.transcript(ann, id).await;
        assert_eq!(transcript.state, TranscriptState::Failed);
        assert_eq!(transcript.failure, Some(failure));
        assert_eq!((transcript.lines, transcript.segments.len()), (0, 0));
        let events = server.recording_events(owner, &mut stream).await;
        assert_eq!(
            transcript_states(&events),
            [TranscriptState::Running, TranscriptState::Failed]
        );
        // The recording keeps its state: the transcript has its own.
        assert_eq!(events[1].1.state, RecordingState::Ready);
        server.clock.advance(Duration::hours(1));
        assert!(!server.transcribe().await);
        let job_directory = server
            .recordings_dir()
            .join(id.to_string())
            .join("transcription");
        assert!(!job_directory.exists());
    }
}

#[tokio::test]
async fn a_worker_that_fails_is_run_three_times_with_a_wait_between_the_runs() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    worker.give(&two_lines(&done));
    worker.behave("exit 1");
    let mut stream = server.full_stream(owner).await;

    assert!(server.transcribe().await);
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Waiting);
    assert!(!server.transcribe().await);
    server.clock.advance(Duration::seconds(59));
    assert!(!server.transcribe().await);
    server.clock.advance(Duration::seconds(2));
    assert!(server.transcribe().await);
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Waiting);
    // The wait grows with each run that failed.
    server.clock.advance(Duration::seconds(61));
    assert!(!server.transcribe().await);
    server.clock.advance(Duration::seconds(60));
    assert!(server.transcribe().await);
    assert_eq!(worker.runs().len(), 3);
    let transcript = server.transcript(ann, id).await;
    assert_eq!(transcript.state, TranscriptState::Failed);
    assert_eq!(transcript.failure, Some(TranscriptFailure::WorkerFailed));
    server.clock.advance(Duration::hours(1));
    assert!(!server.transcribe().await);
    let events = server.recording_events(owner, &mut stream).await;
    use TranscriptState::{Failed, Running, Waiting};
    assert_eq!(
        transcript_states(&events),
        [Running, Waiting, Running, Waiting, Running, Failed]
    );

    // A person with `manage_recordings` asks for the transcript again.
    server
        .post(ben, &path(id, ""), &Value::Null)
        .await
        .error(ErrorCode::Forbidden);
    worker.behave("");
    let asked: Recording = server
        .post(owner, &path(id, ""), &Value::Null)
        .await
        .ok(StatusCode::OK);
    assert_eq!(asked.transcript, TranscriptState::Waiting);
    server
        .post(owner, &path(id, ""), &Value::Null)
        .await
        .error(ErrorCode::Conflict);
    let entries = server.audit_actions(owner, scene.workspace_id).await;
    let last = entries.last().unwrap();
    assert_eq!(
        last.event,
        AuditEvent::TranscriptRequested { recording_id: id }
    );
    assert_eq!(last.actor_id, Some(owner.account_id));
    assert!(server.transcribe().await);
    assert_eq!(worker.runs().len(), 4);
    let transcript = server.transcript(ann, id).await;
    assert_eq!(transcript.state, TranscriptState::Ready);
    assert_eq!((transcript.failure, transcript.lines), (None, 2));
    server
        .post(owner, &path(id, ""), &Value::Null)
        .await
        .error(ErrorCode::Conflict);
}

#[tokio::test]
async fn a_transcript_that_breaks_the_contract_is_not_stored() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;

    // No JSON, a transcript of another recording, and a line of an account that is not the
    // account of its segment.
    worker.give_text("{\"format_version\": 1");
    assert!(server.transcribe().await);
    let mut other = two_lines(&done);
    other["recording_id"] = json!(RecordingId::generate());
    worker.give(&other);
    server.clock.advance(Duration::seconds(61));
    assert!(server.transcribe().await);
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Waiting);
    let mut foreign = two_lines(&done);
    foreign["lines"][0]["source"]["participant_id"] = json!(scene.owner.account_id);
    worker.give(&foreign);
    server.clock.advance(Duration::seconds(121));
    assert!(server.transcribe().await);

    assert_eq!(worker.runs().len(), 3);
    let transcript = server.transcript(ann, id).await;
    assert_eq!(transcript.state, TranscriptState::Failed);
    assert_eq!(
        transcript.failure,
        Some(TranscriptFailure::TranscriptInvalid)
    );
    assert_eq!(server.count("transcript_lines").await, 0);
    assert_eq!(server.count("transcript_segments").await, 0);
    let directory = server.recordings_dir().join(id.to_string());
    assert!(!directory.join("transcript.json").exists());
    assert!(!directory.join("transcription").exists());
}

#[tokio::test]
async fn a_transcript_file_above_the_size_that_the_server_reads_is_not_stored() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let server = &scene.server;
    let done = studio.finished_after(length()).await;
    worker.give(&two_lines(&done));
    let small = server
        .strict(|config| config.limits.recordings.transcription.most_transcript_bytes = 100)
        .await;

    assert!(small.transcribe().await);
    assert_eq!(
        state_of(server, &scene.ann, done.recording.id).await,
        TranscriptState::Waiting
    );
    assert_eq!(server.count("transcript_lines").await, 0);
    server.clock.advance(Duration::seconds(61));
    assert!(server.transcribe().await);
    assert_eq!(
        state_of(server, &scene.ann, done.recording.id).await,
        TranscriptState::Ready
    );
}

/// ADR 0610: the server needs its own time limit for the worker. The limit is short for
/// the one process that runs the worker; the preparation runs under the normal limits.
#[tokio::test]
async fn a_worker_that_does_not_end_is_ended_at_the_time_limit() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let server = &scene.server;
    let done = studio.finished_after(length()).await;
    worker.give(&two_lines(&done));
    worker.behave("touch \"$here/started\"\nexec sleep 600");
    let hasty = server
        .strict(|config| {
            let limits = &mut config.limits.recordings.transcription;
            limits.time_limit = Duration::milliseconds(1500);
        })
        .await;

    let began = std::time::Instant::now();
    assert!(hasty.transcribe().await);
    assert!(began.elapsed() < std::time::Duration::from_secs(120));
    let transcript = server.transcript(&scene.ann, done.recording.id).await;
    assert_eq!(transcript.state, TranscriptState::Failed);
    assert_eq!(transcript.failure, Some(TranscriptFailure::TimeLimit));
    assert_eq!(worker.runs().len(), 1);
    hasty.clock.advance(Duration::hours(1));
    assert!(!hasty.transcribe().await);
}

/// One job runs at a time for all server processes, and a job whose process is gone is
/// taken by another process (ADR 0607).
#[tokio::test]
async fn a_job_of_a_server_process_that_is_gone_is_taken_by_another() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    worker.give(&two_lines(&done));
    // The first run waits until the test lets it end; later runs end at once.
    worker.behave(
        "if [ \"$(wc -l < \"$here/runs\")\" -eq 1 ]; then\n\
         touch \"$here/started\"\n\
         while [ ! -f \"$here/go\" ]; do sleep 0.05; done\nfi",
    );
    let second = server.restart().await;

    let state = server.state.clone();
    let first_run = tokio::spawn(async move {
        junto_server::recordings::transcribe::run_once(&state)
            .await
            .unwrap()
    });
    let waited = std::time::Instant::now();
    while worker.runs().is_empty() {
        assert!(waited.elapsed() < std::time::Duration::from_secs(60));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Running);

    // The other process takes nothing while the heartbeat of the job is young.
    assert!(!second.transcribe().await);
    second.clock.advance(Duration::seconds(89));
    assert!(!second.transcribe().await);
    assert_eq!(worker.runs().len(), 1);
    // After the lease it takes the job, and its run is the second attempt.
    second.clock.advance(Duration::seconds(2));
    assert!(second.transcribe().await);
    assert_eq!(worker.runs().len(), 2);
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Ready);
    assert_eq!(server.count("transcript_lines").await, 2);

    // The first process stores nothing when its worker ends later.
    worker.signal("go");
    assert!(first_run.await.unwrap());
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Ready);
    assert_eq!(server.count("transcript_lines").await, 2);
    let attempts: i32 = sqlx::query_scalar("select attempts from transcripts")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(attempts, 2);
}

/// A process that lost its job cleans up after its own run only. The run of the process
/// that took the job is not disturbed.
#[tokio::test]
async fn a_process_that_lost_its_job_does_not_disturb_the_run_of_its_successor() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    worker.give(&two_lines(&done));
    // Each run waits until the test lets it end.
    worker.behave(
        "run=$(wc -l < \"$here/runs\" | tr -d ' ')\n\
         while [ ! -f \"$here/go$run\" ]; do sleep 0.05; done",
    );
    let second = server.restart().await;
    let spawn = |state: junto_server::state::AppState| {
        tokio::spawn(async move {
            junto_server::recordings::transcribe::run_once(&state)
                .await
                .unwrap()
        })
    };
    let runs = async |count: usize| {
        let waited = std::time::Instant::now();
        while worker.runs().len() < count {
            assert!(waited.elapsed() < std::time::Duration::from_secs(60));
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    };

    let first_run = spawn(server.state.clone());
    runs(1).await;
    second.clock.advance(Duration::seconds(91));
    let second_run = spawn(second.state.clone());
    runs(2).await;

    // The first process ends while the worker of the second still runs.
    worker.signal("go1");
    assert!(first_run.await.unwrap());
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Running);
    worker.signal("go2");
    assert!(second_run.await.unwrap());
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Ready);
    assert_eq!(worker.runs().len(), 2);
    assert_eq!(server.count("transcript_lines").await, 2);
    let directory = server.recordings_dir().join(id.to_string());
    assert!(directory.join("transcript.json").is_file());
    assert!(!directory.join("transcription").exists());
}

/// The late end of a run whose process lost the job changes neither the stored transcript
/// nor its file. Two rules give this: the process that took the job deletes the directory
/// of the earlier run, and a process writes the file only while it holds the job. The test
/// does not tell the two apart.
#[tokio::test]
async fn the_late_end_of_a_run_that_lost_its_job_changes_nothing() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    let stored = two_lines(&done);
    worker.give(&stored);
    // The first run waits; the second ends at once.
    worker.behave(
        "if [ \"$(wc -l < \"$here/runs\" | tr -d ' ')\" -eq 1 ]; then\n\
         while [ ! -f \"$here/go\" ]; do sleep 0.05; done\nfi",
    );
    let second = server.restart().await;
    let state = server.state.clone();
    let first_run = tokio::spawn(async move {
        junto_server::recordings::transcribe::run_once(&state)
            .await
            .unwrap()
    });
    let waited = std::time::Instant::now();
    while worker.runs().is_empty() {
        assert!(waited.elapsed() < std::time::Duration::from_secs(60));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    second.clock.advance(Duration::seconds(91));
    assert!(second.transcribe().await);
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Ready);

    // The worker of the first process ends with another valid transcript of the same job.
    let late = transcript_of(
        &done,
        (&[], &[]),
        &[Said(0, 500, 1500, "A late run wrote this.", None)],
    );
    worker.give(&late);
    worker.signal("go");
    assert!(first_run.await.unwrap());
    let kept: Value = serde_json::from_slice(
        &std::fs::read(
            server
                .recordings_dir()
                .join(id.to_string())
                .join("transcript.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(kept, stored);
    let lines = server.transcript_lines(ann, id, 200).await;
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0].text, "Good morning everyone.");
}

/// A run deletes the directories of earlier runs of its job. It never deletes the directory
/// of a later run: a process that was held up after it took the job can find that another
/// process has taken the job since.
#[tokio::test]
async fn a_run_deletes_the_directories_of_earlier_runs_only() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let server = &scene.server;
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    worker.give(&two_lines(&done));
    let runs = server
        .recordings_dir()
        .join(id.to_string())
        .join("transcription");
    let later = runs.join("2-0123456789abcdef0123456789abcdef");
    std::fs::create_dir_all(&later).unwrap();
    std::fs::write(later.join("job.json"), b"{}").unwrap();
    // The first run fails, so that a second one follows.
    worker.behave("if [ \"$(wc -l < \"$here/runs\" | tr -d ' ')\" -eq 1 ]; then exit 1; fi");

    assert!(server.transcribe().await);
    assert!(later.join("job.json").is_file());
    // A directory that the first run left behind, as a server that stopped leaves it.
    let earlier = runs.join("1-fedcba9876543210fedcba9876543210");
    std::fs::create_dir_all(&earlier).unwrap();
    std::fs::write(earlier.join("job.json"), b"{}").unwrap();
    server.clock.advance(Duration::seconds(61));
    assert!(server.transcribe().await);
    assert_eq!(
        state_of(server, &scene.ann, id).await,
        TranscriptState::Ready
    );
    assert!(!earlier.exists());
    assert!(later.join("job.json").is_file());
}

/// True while the process with the identifier in the file `name` of the worker exists.
#[cfg(unix)]
fn alive(worker: &FakeWorker, name: &str) -> bool {
    let process = worker.note(name);
    std::process::Command::new("kill")
        .args(["-0", process.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

/// A command of the worker can be a script that starts further processes. A process that
/// outlives the script does not hold the server, and is ended with the run.
#[cfg(unix)]
#[tokio::test]
async fn a_process_that_the_worker_leaves_behind_is_ended_with_the_run() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let server = &scene.server;
    let done = studio.finished_after(length()).await;
    worker.give(&two_lines(&done));
    // The left process keeps the log of the worker open.
    worker.behave("sleep 600 &\necho $! > \"$here/child\"");

    let run = tokio::time::timeout(std::time::Duration::from_secs(120), server.transcribe());
    assert!(
        run.await
            .expect("the run ends although a process holds the log")
    );
    assert_eq!(
        state_of(server, &scene.ann, done.recording.id).await,
        TranscriptState::Ready
    );
    let waited = std::time::Instant::now();
    while alive(&worker, "child") {
        assert!(waited.elapsed() < std::time::Duration::from_secs(30));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// At the time limit the server ends every process of the run, also when the command is a
/// script that waits for the worker.
#[cfg(unix)]
#[tokio::test]
async fn the_time_limit_ends_every_process_of_the_run() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let server = &scene.server;
    let done = studio.finished_after(length()).await;
    worker.give(&two_lines(&done));
    worker.behave("sleep 600 &\necho $! > \"$here/child\"\nwait");
    let hasty = server
        .strict(|config| {
            config.limits.recordings.transcription.time_limit = Duration::seconds(4);
        })
        .await;

    let run = tokio::time::timeout(std::time::Duration::from_secs(120), hasty.transcribe());
    assert!(run.await.expect("the run ends at its time limit"));
    let transcript = server.transcript(&scene.ann, done.recording.id).await;
    assert_eq!(transcript.failure, Some(TranscriptFailure::TimeLimit));
    let waited = std::time::Instant::now();
    while alive(&worker, "child") {
        assert!(waited.elapsed() < std::time::Duration::from_secs(30));
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// The file of the worker is kept before the transcript is stored. A transcript whose file
/// cannot be kept is not stored; the run counts as failed and is repeated.
#[cfg(unix)]
#[tokio::test]
async fn a_transcript_whose_file_cannot_be_kept_is_not_stored() {
    use std::os::unix::fs::PermissionsExt;

    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    worker.give(&two_lines(&done));
    // The worker takes the right to write into the directory of the recording away. Its
    // own directory stays writable.
    worker.behave("chmod 500 \"$6\"");
    let directory = server.recordings_dir().join(id.to_string());

    assert!(server.transcribe().await);
    std::fs::set_permissions(&directory, PermissionsExt::from_mode(0o700)).unwrap();
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Waiting);
    assert_eq!(server.count("transcript_lines").await, 0);
    assert!(!directory.join("transcript.json").exists());

    worker.behave("");
    server.clock.advance(Duration::seconds(61));
    assert!(server.transcribe().await);
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Ready);
    assert!(directory.join("transcript.json").is_file());
}

#[tokio::test]
async fn a_job_that_lost_its_process_three_times_fails_as_interrupted() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    // Three server processes took the job and are gone.
    sqlx::query(
        "update transcripts set state = 'running', attempts = 3, claimed_by = $1,
             claimed_at = $2, heartbeat_at = $2",
    )
    .bind(uuid::Uuid::new_v4())
    .bind(server.clock.now())
    .execute(&server.db)
    .await
    .unwrap();
    let mut stream = server.full_stream(owner).await;

    assert!(!server.transcribe().await);
    assert_eq!(state_of(server, ann, id).await, TranscriptState::Running);
    server.clock.advance(Duration::seconds(91));
    assert!(!server.transcribe().await);
    let transcript = server.transcript(ann, id).await;
    assert_eq!(transcript.state, TranscriptState::Failed);
    assert_eq!(transcript.failure, Some(TranscriptFailure::Interrupted));
    assert!(worker.runs().is_empty());
    let events = server.recording_events(owner, &mut stream).await;
    assert_eq!(transcript_states(&events), [TranscriptState::Failed]);
}

/// A recording can be removed while its transcript is built. The process that runs the
/// worker learns it at its next heartbeat and ends the worker.
#[tokio::test]
async fn the_worker_is_ended_when_its_recording_is_removed() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    worker.give(&two_lines(&done));
    worker.behave("touch \"$here/started\"\nexec sleep 600");
    // Give deletion time to commit between lease renewals under CPU load.
    let quick = server
        .strict(|config| {
            config.limits.recordings.transcription.heartbeat = Duration::seconds(10);
        })
        .await;

    let state = quick.state.clone();
    let run = tokio::spawn(async move {
        junto_server::recordings::transcribe::run_once(&state)
            .await
            .unwrap()
    });
    let waited = std::time::Instant::now();
    while worker.runs().is_empty() {
        assert!(waited.elapsed() < std::time::Duration::from_secs(60));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    server
        .delete(owner, &format!("/api/v1/recordings/{id}"))
        .await
        .empty(StatusCode::NO_CONTENT);

    let ended = tokio::time::timeout(std::time::Duration::from_secs(120), run).await;
    assert!(ended.unwrap().unwrap());
    assert_eq!(server.count("transcripts").await, 0);
    assert_eq!(server.count("recordings").await, 0);
    server.process().await;
    assert!(!server.recordings_dir().join(id.to_string()).exists());
}

/// ADR 0003: the worker must not take the CPU cores of the recorder during a recording.
#[tokio::test]
async fn no_job_starts_while_a_recording_runs() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let done = studio.finished_after(length()).await;
    worker.give(&two_lines(&done));

    let next = studio.record().await;
    assert!(!server.transcribe().await);
    assert_eq!(
        state_of(server, ann, done.recording.id).await,
        TranscriptState::Waiting
    );
    assert!(worker.runs().is_empty());

    studio.stop(next.id).await;
    assert!(server.transcribe().await);
    assert_eq!(
        state_of(server, ann, done.recording.id).await,
        TranscriptState::Ready
    );
}

#[tokio::test]
async fn jobs_run_one_after_the_other_in_the_order_of_their_requests() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let first = studio.finished_after(length()).await;
    server.clock.advance(Duration::seconds(1));
    let second = studio.finished_after(length()).await;

    worker.give(&two_lines(&first));
    assert!(server.transcribe().await);
    assert_eq!(
        state_of(server, ann, first.recording.id).await,
        TranscriptState::Ready
    );
    assert_eq!(
        state_of(server, ann, second.recording.id).await,
        TranscriptState::Waiting
    );
    worker.give(&two_lines(&second));
    assert!(server.transcribe().await);
    assert_eq!(
        state_of(server, ann, second.recording.id).await,
        TranscriptState::Ready
    );
    assert!(!server.transcribe().await);
    assert_eq!(server.count("transcript_lines").await, 4);
}

#[tokio::test]
async fn a_server_without_a_worker_builds_no_transcript() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let done = studio.finished().await;
    let id = done.recording.id;
    assert_eq!(done.recording.transcript, TranscriptState::None);
    assert!(!server.transcribe().await);
    let transcript = server.transcript(ann, id).await;
    assert_eq!(transcript.state, TranscriptState::None);
    assert_eq!((transcript.lines, transcript.segments.len()), (0, 0));
    assert_eq!(transcript.language, "auto");
    let lines: TranscriptLinePage = server
        .get(ann, &path(id, "/lines"))
        .await
        .ok(StatusCode::OK);
    assert_eq!((lines.lines.len(), lines.next_after), (0, None));
    server
        .get(ann, &path(id, "/text"))
        .await
        .error(ErrorCode::NotFound);
    server
        .post(owner, &path(id, ""), &Value::Null)
        .await
        .error(ErrorCode::Conflict);
    assert_eq!(server.count("transcripts").await, 0);
}

/// A server that starts the worker itself gives the batch size of its setting to each job,
/// as the loop of the worker container does (ADR 0607, ADR 0016).
#[tokio::test]
async fn the_batch_size_of_the_setting_is_an_option_of_each_run_of_the_worker() {
    let worker = FakeWorker::new();
    let mut settings = worker.settings();
    settings.batch_size = Some(2);
    let server = TestServer::start_for_calls_with(|config| {
        config.recorder.as_mut().unwrap().transcriber = Some(settings);
    })
    .await;
    let studio = Studio::on(server).await;
    let server = &studio.scene.server;
    let done = studio.finished_after(length()).await;
    worker.give(&two_lines(&done));
    assert!(server.transcribe().await);
    let runs = worker.runs();
    assert_eq!(runs.len(), 1);
    let directory = server.recordings_dir().join(done.recording.id.to_string());
    let expected = format!("--audio-root {} --batch-size 2", directory.display());
    assert!(runs[0].ends_with(&expected), "{}", runs[0]);
    assert_eq!(
        server
            .transcript(&studio.scene.ann, done.recording.id)
            .await
            .state,
        TranscriptState::Ready
    );
}

#[tokio::test]
async fn a_line_is_corrected_by_every_person_who_reads_the_recording() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    worker.give(&two_lines(&done));
    assert!(server.transcribe().await);
    let original = server.transcript_lines(ann, id, 200).await;
    let mut stream = server.full_stream(owner).await;

    let first_at = server.clock.now();
    let corrected: TranscriptLine = server
        .put(
            ben,
            &path(id, "/lines/1"),
            &json!({ "text": "  Can you all hear me?  " }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        corrected,
        TranscriptLine {
            text: "Can you all hear me?".to_owned(),
            original_text: Some("Can you hear me?".to_owned()),
            corrected_by: Some(ben.account_id),
            corrected_at: Some(first_at),
            ..original[1].clone()
        }
    );
    let events: Vec<Event> = server
        .traffic(owner, &mut stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .collect();
    assert_eq!(
        events,
        [Event::TranscriptLineCorrected {
            recording_id: id,
            workspace_id: scene.workspace_id,
            channel_id: scene.voice.id,
            line: 1,
        }]
    );
    // The event states which line changed. The text is read from the route of the lines.
    let read: TranscriptLinePage = server
        .get(owner, &path(id, "/lines?after=0&limit=1"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(read.lines, std::slice::from_ref(&corrected));

    // A later correction is shown with its author; the text of the worker stays.
    server.clock.advance(Duration::seconds(5));
    let again: TranscriptLine = server
        .put(
            ann,
            &path(id, "/lines/1"),
            &json!({ "text": "Can everyone hear me?" }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(again.corrected_by, Some(ann.account_id));
    assert_eq!(again.original_text.as_deref(), Some("Can you hear me?"));
    let lines = server.transcript_lines(owner, id, 200).await;
    assert_eq!(lines, [original[0].clone(), again]);
    assert_eq!(server.count("transcript_corrections").await, 2);
    let stored: String = sqlx::query_scalar("select text from transcript_lines where line = 1")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(stored, "Can you hear me?");
    // A correction changes no state of the transcript.
    assert_eq!(server.transcript(ann, id).await.lines, 2);

    server
        .put(
            ann,
            &path(id, "/lines/2"),
            &json!({ "text": "No such line." }),
        )
        .await
        .error(ErrorCode::NotFound);
    let long = "a".repeat(2001);
    for refused in ["", "   ", "two\nlines", long.as_str()] {
        server
            .put(ann, &path(id, "/lines/0"), &json!({ "text": refused }))
            .await
            .error(ErrorCode::InvalidRequest);
    }
    assert_eq!(server.count("transcript_corrections").await, 2);

    // The text form carries the newest correction.
    let text = server.get(ben, &path(id, "/text")).await.text();
    assert!(text.contains(": Can everyone hear me?\n"), "{text}");
    assert!(text.contains(": Good morning everyone.\n"), "{text}");
}

#[tokio::test]
async fn a_correction_made_from_an_older_text_is_refused_and_changes_nothing() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    worker.give(&two_lines(&done));
    assert!(server.transcribe().await);
    let line = path(id, "/lines/1");

    // A correction from the text of the worker, as the line shows it, is stored.
    server
        .put(
            ben,
            &line,
            &json!({ "text": "Can you all hear me?", "replaces": "Can you hear me?" }),
        )
        .await
        .ok::<TranscriptLine>(StatusCode::OK);
    // A second correction from the same older text would hide the first one without a
    // word: it is refused, and nothing is stored or told.
    let (events, corrections) = (
        server.count("events").await,
        server.count("transcript_corrections").await,
    );
    server
        .put(
            ann,
            &line,
            &json!({ "text": "Can everyone hear me?", "replaces": "Can you hear me?" }),
        )
        .await
        .error(ErrorCode::Conflict);
    assert_eq!(server.count("events").await, events);
    assert_eq!(server.count("transcript_corrections").await, corrections);
    // From the text that the line shows now, it is stored.
    let stored: TranscriptLine = server
        .put(
            ann,
            &line,
            &json!({ "text": "Can everyone hear me?", "replaces": "Can you all hear me?" }),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(stored.text, "Can everyone hear me?");
}

#[tokio::test]
async fn a_line_keeps_a_bounded_number_of_corrections() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    worker.give(&two_lines(&done));
    assert!(server.transcribe().await);
    let narrow = server
        .strict(|config| config.limits.recordings.most_corrections = 2)
        .await;

    for text in ["one", "two"] {
        narrow
            .put(ann, &path(id, "/lines/0"), &json!({ "text": text }))
            .await
            .ok::<TranscriptLine>(StatusCode::OK);
    }
    narrow
        .put(ann, &path(id, "/lines/0"), &json!({ "text": "three" }))
        .await
        .error(ErrorCode::LimitReached);
    narrow
        .put(
            ann,
            &path(id, "/lines/1"),
            &json!({ "text": "another line" }),
        )
        .await
        .ok::<TranscriptLine>(StatusCode::OK);
    let lines = server.transcript_lines(ann, id, 200).await;
    assert_eq!(lines[0].text, "two");
}

#[tokio::test]
async fn the_lines_are_read_in_pages() {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    let said: Vec<Said> = (0..7)
        .map(|number| {
            Said(
                number % 2,
                100 * number as u64,
                100 * number as u64 + 50,
                "A line.",
                None,
            )
        })
        .collect();
    worker.give(&transcript_of(&done, (&[], &[]), &said));
    assert!(server.transcribe().await);

    let page = |query: &'static str| async move {
        server
            .get(ann, &path(id, &format!("/lines{query}")))
            .await
            .ok::<TranscriptLinePage>(StatusCode::OK)
    };
    let first = page("?limit=3").await;
    let positions = |page: &TranscriptLinePage| -> Vec<u32> {
        page.lines.iter().map(|line| line.line).collect()
    };
    assert_eq!(
        (positions(&first), first.next_after),
        (vec![0, 1, 2], Some(2))
    );
    let second = page("?limit=3&after=2").await;
    assert_eq!(
        (positions(&second), second.next_after),
        (vec![3, 4, 5], Some(5))
    );
    let last = page("?limit=3&after=5").await;
    assert_eq!((positions(&last), last.next_after), (vec![6], None));
    let exact = page("?limit=7").await;
    assert_eq!((exact.lines.len(), exact.next_after), (7, None));
    assert_eq!(page("").await.lines.len(), 7);
    assert_eq!(page("?after=99").await.lines.len(), 0);
    assert_eq!(server.transcript_lines(ann, id, 2).await.len(), 7);
    for refused in ["?limit=0", "?limit=501", "?after=-1", "?limit=many"] {
        server
            .get(ann, &path(id, &format!("/lines{refused}")))
            .await
            .error(ErrorCode::InvalidRequest);
    }
}

/// A transcript names the people of a call. It is read by the rule of its recording
/// (ADR 0604): a person without sight of the voice channel gets the answer of an absent
/// recording, and while the call runs in a channel that hides who is inside, only the
/// people inside read it.
#[tokio::test]
async fn a_transcript_is_read_and_changed_by_the_rule_of_its_recording() {
    let worker = FakeWorker::new();
    let studio = Studio::hidden_on(worker.server().await).await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    server.publish(
        &studio.ann_seat,
        TrackKind::Audio,
        TrackSource::ScreenShareAudio,
    );
    let done = studio.finished_after(length()).await;
    let id = done.recording.id;
    let shared = audio(&done)
        .iter()
        .position(|track| track.source == SegmentSource::ScreenShareAudio)
        .unwrap();
    worker.give(&transcript_of(
        &done,
        (&[], &[]),
        &[Said(shared, 100, 900, "Thank you.", Some(1))],
    ));
    assert!(server.transcribe().await);
    let outsider = server.join(owner, "Cy").await;

    let absent = RecordingId::generate();
    let requests = |member: &'static str, id: RecordingId| {
        let member = match member {
            "outsider" => &outsider,
            "owner" => owner,
            _ => ann,
        };
        async move {
            vec![
                server.get(member, &path(id, "")).await,
                server.get(member, &path(id, "/lines")).await,
                server.get(member, &path(id, "/text")).await,
                server
                    .put(
                        member,
                        &path(id, "/lines/0"),
                        &json!({ "text": "Changed." }),
                    )
                    .await,
                server
                    .put(
                        member,
                        &path(id, "/speakers/1"),
                        &json!({ "name": "Changed" }),
                    )
                    .await,
            ]
        }
    };
    let observable = |replies: Vec<crate::support::Reply>| -> Vec<_> {
        replies
            .iter()
            .map(crate::support::Reply::observable)
            .collect()
    };
    // The requests that need `manage_recordings`, by a person who may not read the recording.
    let managing = async |id: RecordingId| {
        let removal = format!("/api/v1/recordings/{id}");
        vec![
            server.post(&outsider, &path(id, ""), &Value::Null).await,
            server.delete(&outsider, &removal).await,
        ]
    };
    let not_found = observable(requests("outsider", absent).await);
    assert!(not_found.iter().all(|reply| reply.0 == 404));
    let absent_managed = observable(managing(absent).await);
    assert!(absent_managed.iter().all(|reply| reply.0 == 404));

    // A member of another workspace of the server gets the answer of an absent recording.
    let closed: junto_protocol::structure::Workspace = server
        .post(owner, "/api/v1/workspaces", &json!({ "name": "Closed" }))
        .await
        .ok(StatusCode::CREATED);
    let stranger = server.join_into(owner, "Dee", &[closed.id]).await;
    let by_the_stranger = vec![
        server.get(&stranger, &path(id, "")).await,
        server.get(&stranger, &path(id, "/lines")).await,
        server.get(&stranger, &path(id, "/text")).await,
        server
            .put(
                &stranger,
                &path(id, "/lines/0"),
                &json!({ "text": "Changed." }),
            )
            .await,
        server
            .put(
                &stranger,
                &path(id, "/speakers/1"),
                &json!({ "name": "Changed" }),
            )
            .await,
    ];
    assert_eq!(observable(by_the_stranger), not_found);
    assert_eq!(server.count("transcript_corrections").await, 0);

    // While the call runs and hides who is inside, a person outside gets the answer of an
    // absent recording, for every route. Turning the setting off changes nothing for that
    // call (ADR 0517).
    for turn in 0..2 {
        assert_eq!(observable(requests("outsider", id).await), not_found);
        assert_eq!(observable(requests("owner", id).await), not_found);
        assert_eq!(observable(managing(id).await), absent_managed);
        if turn == 0 {
            server
                .call_settings(owner, scene.voice.id, json!({ "hides_occupants": false }))
                .await;
        }
    }
    let inside = requests("ann", id).await;
    assert!(inside.iter().all(|reply| reply.status == StatusCode::OK));
    assert_eq!(server.count("transcript_corrections").await, 1);

    // When the call has ended, every member of the workspace sees the channel and reads
    // and changes the transcript.
    server.leave_call(ann).await;
    server.leave_call(&scene.ben).await;
    server.check().await;
    let seen = requests("outsider", id).await;
    assert!(seen.iter().all(|reply| reply.status == StatusCode::OK));
    assert_eq!(server.count("transcript_corrections").await, 2);

    // A person who does not see the channel reads nothing of it.
    let role: junto_protocol::roles::WorkspaceRole = server
        .post(
            owner,
            &format!("/api/v1/workspaces/{}/roles", scene.workspace_id),
            &json!({ "name": "insiders" }),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .make_private(owner, scene.workspace_id, scene.voice.id, role.id)
        .await;
    assert_eq!(observable(requests("outsider", id).await), not_found);
    assert_eq!(observable(managing(id).await), absent_managed);
    assert_eq!(server.count("transcript_corrections").await, 2);

    // A recording of a deleted channel is read and changed by nobody (ADR 0604).
    server.leave_call(ann).await;
    server.leave_call(&scene.ben).await;
    server
        .delete(owner, &format!("/api/v1/channels/{}", scene.voice.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(observable(requests("owner", id).await), not_found);
    let removal = format!("/api/v1/recordings/{id}");
    let by_the_owner = vec![
        server.post(owner, &path(id, ""), &Value::Null).await,
        server.delete(owner, &removal).await,
    ];
    assert_eq!(observable(by_the_owner), absent_managed);
    assert_eq!(server.count("recordings").await, 1);
    assert_eq!(server.count("transcript_corrections").await, 2);
}
