//! The tier with the real transcription worker of `workers/transcription` (ADR 0607,
//! ADR 0902). The test is ignored in the plain run. It needs the virtual environment of the
//! worker and the models of its development tier:
//!
//! ```sh
//! (cd workers/transcription && uv sync && uv run junto-transcribe models fetch --dir .models --whisper small)
//! tools/with-test-postgres.sh tools/serial.sh \
//!     cargo test -p junto-server --test api transcriber_real:: -- --ignored
//! ```
//!
//! The speech is the generated speech of the worker's own model tests
//! (`workers/transcription/README.md`).

use std::path::Path;
use std::sync::Arc;

use junto_protocol::recording::SegmentSource;
use junto_protocol::transcript::{LineSource, SegmentTranscription, TranscriptState};
use junto_server::config::TranscriberSettings;
use junto_server::media::{TrackKind, TrackSource};
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::Duration;

use crate::recordings_support::Studio;
use crate::support::{TestServer, package_directory};

#[tokio::test]
#[ignore = "needs the virtual environment and the models of workers/transcription"]
async fn the_real_worker_transcribes_the_microphone_and_the_shared_sound_of_a_recording() {
    let worker = package_directory().join("../../workers/transcription");
    let program = worker.join(".venv/bin/junto-transcribe");
    let models = worker.join(".models");
    let speech = worker.join(".audio/en_quick.ogg");
    let voices = worker.join(".audio/trio_first.ogg");
    for needed in [
        &program,
        &models.join("faster-whisper-small"),
        &speech,
        &voices,
    ] {
        assert!(
            needed.exists(),
            "{} is missing. This tier needs the virtual environment, the models and the \
             generated speech of workers/transcription; see its README.",
            needed.display()
        );
    }
    let text = |path: &Path| path.to_str().unwrap().to_owned();
    let settings = TranscriberSettings {
        command: vec![text(&program)],
        environment: vec![
            ("JUNTO_TRANSCRIBE_MODELS".to_owned(), text(&models)),
            (
                "JUNTO_TRANSCRIBE_WHISPER_MODEL".to_owned(),
                "small".to_owned(),
            ),
            ("JUNTO_TRANSCRIBE_DEVICE".to_owned(), "cpu".to_owned()),
        ],
        separate_speakers: true,
        batch_size: None,
    };
    let server = TestServer::start_for_calls_with(|config| {
        config.recorder.as_mut().unwrap().transcriber = Some(settings);
    })
    .await;
    let studio = Studio::on(server).await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let glossary = format!("/api/v1/workspaces/{}/glossary", scene.workspace_id);
    server
        .put(owner, &glossary, &json!({ "terms": ["billing service"] }))
        .await
        .ok::<Value>(StatusCode::OK);

    // Ann speaks and shares a window with three voices. Ben is muted.
    let shared = server.publish(
        &studio.ann_seat,
        TrackKind::Audio,
        TrackSource::ScreenShareAudio,
    );
    let microphone = |seat: &crate::calls_support::Seat| {
        server
            .fake()
            .track_id(&seat.room, &seat.identity, TrackSource::Microphone)
            .unwrap()
    };
    let recorder = server.recorder();
    recorder.set_track_of(
        &microphone(&studio.ann_seat),
        Arc::new(std::fs::read(&speech).unwrap()),
    );
    recorder.set_track_of(&shared, Arc::new(std::fs::read(&voices).unwrap()));
    recorder.set_track_of(
        &microphone(&studio.ben_seat),
        junto_server::recorder::samples::silent_track(),
    );
    let done = studio.finished_after(Duration::seconds(32)).await;
    let id = done.recording.id;
    assert_eq!(done.recording.transcript, TranscriptState::Waiting);

    assert!(server.transcribe().await);
    let transcript = server.transcript(ben, id).await;
    assert_eq!(transcript.state, TranscriptState::Ready, "{transcript:?}");
    assert_eq!(transcript.glossary.used, ["billing service"]);
    let of = |account, source| {
        let segment = done
            .segments
            .iter()
            .find(|segment| segment.account_id == Some(account) && segment.source == source)
            .unwrap();
        transcript
            .segments
            .iter()
            .find(|result| result.segment_id == segment.id)
            .unwrap()
    };
    let spoken = of(ann.account_id, SegmentSource::Microphone);
    assert_eq!(spoken.status, SegmentTranscription::Ok);
    assert!(spoken.lines >= 3, "{spoken:?}");
    assert_eq!(
        of(ben.account_id, SegmentSource::Microphone).status,
        SegmentTranscription::Empty
    );
    let shown = of(ann.account_id, SegmentSource::ScreenShareAudio);
    assert_eq!(shown.status, SegmentTranscription::Ok);

    let lines = server.transcript_lines(ben, id, 500).await;
    assert_eq!(lines.len() as u32, transcript.lines);
    assert!(lines.iter().all(|line| line.account_id == ann.account_id));
    assert!(
        lines
            .windows(2)
            .all(|pair| pair[0].start_ms <= pair[1].start_ms)
    );
    let said = |source| {
        lines
            .iter()
            .filter(|line| line.source == source)
            .map(|line| line.text.to_lowercase())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let own = said(LineSource::Microphone);
    for word in ["planning", "migration", "database", "billing service"] {
        assert!(own.contains(word), "{word:?} is not in {own:?}");
    }
    let shared_sound = said(LineSource::Shared);
    for word in ["customer", "spreadsheet", "onboarding"] {
        assert!(
            shared_sound.contains(word),
            "{word:?} is not in {shared_sound:?}"
        );
    }
    // The three voices of the shared sound have labels.
    assert!(!transcript.speakers.is_empty(), "{transcript:?}");
    assert!(
        transcript
            .speakers
            .iter()
            .all(|speaker| speaker.account_id == ann.account_id)
    );
    assert!(
        lines
            .iter()
            .any(|line| line.source == LineSource::Shared && line.speaker.is_some())
    );
    let text = server
        .get(ben, &format!("/api/v1/recordings/{id}/transcript/text"))
        .await
        .text();
    assert!(text.contains("] Ann: "), "{text}");
    assert!(text.contains("(shared audio of Ann): "), "{text}");
}
