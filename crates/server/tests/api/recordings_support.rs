//! Helpers for the tests of recordings: a call with a person who may record, the fake
//! recorder, and the passes of the controller.

use junto_protocol::WorkspacePermission;
use junto_protocol::events::Event;
use junto_protocol::ids::{ChannelId, RecordingId, WorkspaceId};
use junto_protocol::recording::{Recording, RecordingDetail, RecordingPage, RecordingSegment};
use junto_server::media::{TrackKind, TrackSource};
use junto_server::recorder::fake::{FakeJob, FakeRecorder};
use junto_server::recordings::control;
use junto_server::storage::FakeDisk;
use reqwest::StatusCode;
use serde_json::json;

use crate::calls_support::{Scene, Seat};
use crate::stream_support::Stream;
use crate::support::{Member, Reply, TestServer};

/// A scene in which Ann holds `record_calls`, and Ann and Ben are in the call of the voice
/// channel, each with a microphone track.
pub struct Studio {
    pub scene: Scene,
    pub ann_seat: Seat,
    pub ben_seat: Seat,
}

impl Studio {
    pub async fn open() -> Self {
        Self::on(TestServer::start_for_calls().await).await
    }

    pub async fn on(server: TestServer) -> Self {
        let scene = Scene::on(server).await;
        let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
        let record = [WorkspacePermission::RecordCalls];
        server
            .grant_workspace(owner, ann, scene.workspace_id, &record)
            .await;
        let ann_seat = server.enter_call(ann, scene.voice.id).await;
        let ben_seat = server.enter_call(ben, scene.voice.id).await;
        server.speak(&ann_seat);
        server.speak(&ben_seat);
        Self {
            scene,
            ann_seat,
            ben_seat,
        }
    }

    /// Ann starts a recording, and the controller runs one pass: the jobs run.
    pub async fn record(&self) -> Recording {
        let server = &self.scene.server;
        let started = server.record(&self.scene.ann, self.scene.voice.id).await;
        server.tend().await;
        started
    }

    /// Ann stops the recording, and the controller runs three passes: the stops, the ends of
    /// the jobs, and the last questions about jobs that were declared lost.
    pub async fn stop(&self, recording_id: RecordingId) -> RecordingDetail {
        let server = &self.scene.server;
        server
            .stop_recording(&self.scene.ann, self.scene.voice.id)
            .await
            .empty(StatusCode::NO_CONTENT);
        for _ in 0..3 {
            server.tend().await;
        }
        server.recording(&self.scene.ann, recording_id).await
    }
}

impl TestServer {
    pub fn recorder(&self) -> &FakeRecorder {
        self.recorder
            .as_deref()
            .expect("a server started for calls")
    }

    pub fn disk(&self) -> &FakeDisk {
        self.disk.as_deref().expect("a server started for calls")
    }

    /// Runs one pass of the controller over every active recording. Before it, the fake
    /// recorder ends the files of the tracks that have ended, as a recorder does by itself.
    pub async fn tend(&self) -> usize {
        self.recorder().work();
        control::tend_all(&self.state).await.unwrap()
    }

    /// Publishes a microphone track and returns its identifier.
    pub fn speak(&self, seat: &Seat) -> String {
        self.publish(seat, TrackKind::Audio, TrackSource::Microphone)
    }

    pub fn publish(&self, seat: &Seat, kind: TrackKind, source: TrackSource) -> String {
        let fake = self.fake();
        assert!(fake.publish(&seat.room, &seat.identity, kind, source));
        fake.track_id(&seat.room, &seat.identity, source).unwrap()
    }

    pub async fn record_reply(&self, member: &Member, channel_id: ChannelId) -> Reply {
        let path = format!("/api/v1/channels/{channel_id}/call/recording");
        self.put(member, &path, &json!({})).await
    }

    /// Asks for a recording. No pass of the controller has run for it yet.
    pub async fn record(&self, member: &Member, channel_id: ChannelId) -> Recording {
        self.record_reply(member, channel_id)
            .await
            .ok(StatusCode::OK)
    }

    pub async fn stop_recording(&self, member: &Member, channel_id: ChannelId) -> Reply {
        let path = format!("/api/v1/channels/{channel_id}/call/recording");
        self.delete(member, &path).await
    }

    pub async fn recording_reply(&self, member: &Member, recording_id: RecordingId) -> Reply {
        self.get(member, &format!("/api/v1/recordings/{recording_id}"))
            .await
    }

    pub async fn recording(&self, member: &Member, recording_id: RecordingId) -> RecordingDetail {
        self.recording_reply(member, recording_id)
            .await
            .ok(StatusCode::OK)
    }

    pub async fn recordings(&self, member: &Member, workspace_id: WorkspaceId) -> RecordingPage {
        self.get(
            member,
            &format!("/api/v1/workspaces/{workspace_id}/recordings"),
        )
        .await
        .ok(StatusCode::OK)
    }

    /// The recordings in the events of recordings that `stream` received until now, each
    /// with the name of its event.
    pub async fn recording_events(
        &self,
        member: &Member,
        stream: &mut Stream,
    ) -> Vec<(&'static str, Recording)> {
        self.traffic(member, stream)
            .await
            .into_iter()
            .filter_map(|(_, event)| {
                let name = event.name();
                match event {
                    Event::RecordingStarted { recording }
                    | Event::RecordingUpdated { recording } => Some((name, *recording)),
                    _ => None,
                }
            })
            .collect()
    }

    /// The actions of the audit entries of a workspace, oldest first, each with its actor.
    pub async fn audit_actions(
        &self,
        reader: &Member,
        workspace_id: WorkspaceId,
    ) -> Vec<junto_protocol::audit::AuditEntry> {
        let page: junto_protocol::audit::AuditPage = self
            .get(
                reader,
                &format!("/api/v1/workspaces/{workspace_id}/audit-log?limit=200"),
            )
            .await
            .ok(StatusCode::OK);
        page.entries.into_iter().rev().collect()
    }
}

/// The segment of a recording that the job of the recorder with `job_id` writes, found by
/// the order of the requests: the fake recorder numbers its jobs in the order of the
/// segments of one pass.
pub fn of_source(
    detail: &RecordingDetail,
    source: junto_protocol::recording::SegmentSource,
) -> Vec<&RecordingSegment> {
    detail
        .segments
        .iter()
        .filter(|segment| segment.source == source)
        .collect()
}

pub fn composites(jobs: &[FakeJob]) -> Vec<&FakeJob> {
    jobs.iter().filter(|job| job.is_composite()).collect()
}

impl Studio {
    /// Ann records and stops, and the server processes the files: the recording is in a
    /// final state.
    pub async fn finished(&self) -> RecordingDetail {
        let started = self.record().await;
        self.stop(started.id).await;
        let server = &self.scene.server;
        assert_eq!(server.process().await, 1);
        server.recording(&self.scene.ann, started.id).await
    }

    /// Like [`Studio::finished`], with `length` between the start and the stop, so that
    /// the recorder reports that length for each file.
    pub async fn finished_after(&self, length: time::Duration) -> RecordingDetail {
        let started = self.record().await;
        self.scene.server.clock.advance(length);
        self.stop(started.id).await;
        let server = &self.scene.server;
        assert_eq!(server.process().await, 1);
        server.recording(&self.scene.ann, started.id).await
    }
}

/// The problems of the file of a segment. Panics for a segment without a file.
pub fn problems(segment: &RecordingSegment) -> &[junto_protocol::recording::FileProblem] {
    &segment.file.as_ref().expect("a file").problems
}

/// The segment of the composite and the segments of the audio tracks of a recording with
/// one composite.
pub fn parts(detail: &RecordingDetail) -> (&RecordingSegment, Vec<&RecordingSegment>) {
    use junto_protocol::recording::SegmentSource;
    let composite = of_source(detail, SegmentSource::Composite);
    assert_eq!(composite.len(), 1, "{detail:?}");
    let tracks = detail
        .segments
        .iter()
        .filter(|segment| segment.source != SegmentSource::Composite)
        .collect();
    (composite[0], tracks)
}

impl TestServer {
    /// Runs one attempt of the processing for every recording that waits, and deletes the
    /// directories that wait. Returns the number of recordings.
    pub async fn process(&self) -> usize {
        junto_server::recordings::process::run_once(&self.state)
            .await
            .unwrap()
    }

    /// Takes one transcription job and runs it to its end. False when none was taken.
    pub async fn transcribe(&self) -> bool {
        junto_server::recordings::transcribe::run_once(&self.state)
            .await
            .unwrap()
    }

    /// The directory of the recordings of this server.
    pub fn recordings_dir(&self) -> std::path::PathBuf {
        self.state
            .recordings
            .backend()
            .expect("a server started for calls")
            .settings
            .directory
            .clone()
    }

    /// The kinds of the notes of a voice channel, oldest first.
    pub async fn notes(&self, channel_id: ChannelId) -> Vec<String> {
        sqlx::query_scalar("select kind from call_notes where channel_id = $1 order by id")
            .bind(channel_id.as_uuid())
            .fetch_all(&self.db)
            .await
            .unwrap()
    }
}

/// A stand-in for the transcription worker: a script that the server runs as
/// `<script> run <job> --out <transcript> --audio-root <directory>`. It keeps the job that
/// it got, writes the transcript that the test gave it, and does what the test tells it
/// before that. The contract between the server and the worker is ADR 0611.
pub struct FakeWorker {
    scratch: crate::support::Scratch,
}

const WORKER_SCRIPT: &str = r#"#!/bin/sh
here=$(dirname "$0")
echo "$*" >> "$here/runs"
env > "$here/environment"
cp "$2" "$here/job.json"
ls "$6" > "$here/audio-root"
if [ -f "$here/behaviour" ]; then . "$here/behaviour"; fi
cp "$here/transcript.json" "$4.tmp" && mv "$4.tmp" "$4"
"#;

impl FakeWorker {
    pub fn new() -> Self {
        let scratch = crate::support::Scratch::new();
        std::fs::write(scratch.0.join("worker.sh"), WORKER_SCRIPT).unwrap();
        Self { scratch }
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self.scratch.0.join(name)
    }

    /// The settings of a server that runs this worker.
    pub fn settings(&self) -> junto_server::config::TranscriberSettings {
        junto_server::config::TranscriberSettings {
            command: vec![
                "/bin/sh".to_owned(),
                self.path("worker.sh").to_str().unwrap().to_owned(),
            ],
            environment: vec![(
                "JUNTO_TRANSCRIBE_MODELS".to_owned(),
                "/srv/junto/models".to_owned(),
            )],
            separate_speakers: true,
        }
    }

    /// A server for calls that runs this worker.
    pub async fn server(&self) -> TestServer {
        let settings = self.settings();
        TestServer::start_for_calls_with(|config| {
            config.recorder.as_mut().unwrap().transcriber = Some(settings);
        })
        .await
    }

    /// The transcript that the worker writes at its next runs.
    pub fn give(&self, transcript: &serde_json::Value) {
        self.give_text(&serde_json::to_string(transcript).unwrap());
    }

    pub fn give_text(&self, transcript: &str) {
        std::fs::write(self.path("transcript.json"), transcript).unwrap();
    }

    /// Shell text that the worker runs before it writes its transcript, at every run from
    /// now on. `$here` is the directory of the worker.
    pub fn behave(&self, script: &str) {
        std::fs::write(self.path("behaviour"), script).unwrap();
    }

    /// Creates a file that a behaviour waits for.
    pub fn signal(&self, name: &str) {
        std::fs::write(self.path(name), b"").unwrap();
    }

    /// The text of a file that a behaviour wrote into the directory of the worker.
    pub fn note(&self, name: &str) -> String {
        std::fs::read_to_string(self.path(name)).unwrap()
    }

    /// The arguments of every run, oldest first.
    pub fn runs(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("runs"))
            .map(|runs| runs.lines().map(str::to_owned).collect())
            .unwrap_or_default()
    }

    /// The job file of the last run.
    pub fn job(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.path("job.json")).unwrap()).unwrap()
    }

    /// The names of the variables that the last run got.
    pub fn variables(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_to_string(self.path("environment"))
            .unwrap()
            .lines()
            .filter_map(|line| line.split_once('=').map(|(name, _)| name.to_owned()))
            .collect();
        names.sort();
        names
    }

    /// The names of the files in the audio root of the last run.
    pub fn audio_root(&self) -> Vec<String> {
        std::fs::read_to_string(self.path("audio-root"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

/// One line that a test lets the worker write: the position of its segment among the audio
/// segments of the recording, its time, its text and its speaker label.
pub struct Said(
    pub usize,
    pub u64,
    pub u64,
    pub &'static str,
    pub Option<u32>,
);

/// A transcript in the form of ADR 0611 for the job that the server writes for `detail`:
/// one segment for each audio segment with a file, in their order, and the lines of `said`.
/// Each segment is `ok` when it has a line and `empty` otherwise.
pub fn transcript_of(
    detail: &RecordingDetail,
    glossary: (&[&str], &[&str]),
    said: &[Said],
) -> serde_json::Value {
    use junto_protocol::recording::SegmentSource;
    let audio: Vec<&RecordingSegment> = detail
        .segments
        .iter()
        .filter(|segment| segment.source != SegmentSource::Composite && segment.file.is_some())
        .collect();
    let kind = |segment: &RecordingSegment| match segment.source {
        SegmentSource::Microphone => "microphone",
        _ => "shared",
    };
    let participant = |segment: &RecordingSegment| segment.account_id.unwrap().to_string();
    let lines: Vec<serde_json::Value> = said
        .iter()
        .map(|Said(position, start_ms, end_ms, text, speaker)| {
            let segment = audio[*position];
            let mut source = json!({
                "kind": kind(segment),
                "participant_id": participant(segment),
            });
            if kind(segment) == "shared" {
                source["speaker"] = json!(speaker.map(|number| format!("Speaker {number}")));
            }
            json!({
                "start_ms": start_ms, "end_ms": end_ms, "segment_id": segment.id,
                "source": source, "text": text, "language": "en", "words": null,
            })
        })
        .collect();
    let segments: Vec<serde_json::Value> = audio
        .iter()
        .enumerate()
        .map(|(position, segment)| {
            let own: Vec<&Said> = said.iter().filter(|line| line.0 == position).collect();
            let shared = kind(segment) == "shared";
            let labelled = own.iter().filter(|line| line.4.is_some()).count();
            let (speakers, speakers_reason) = match (shared, labelled == own.len()) {
                (false, _) => ("not_applicable", serde_json::Value::Null),
                (true, true) => ("separated", serde_json::Value::Null),
                (true, false) => (
                    "unassigned",
                    json!({"code": "no_voice_window", "message": "too short"}),
                ),
            };
            let length = segment.file.as_ref().unwrap().duration_ms;
            json!({
                "id": segment.id,
                "status": if own.is_empty() { "empty" } else { "ok" },
                "reason": null, "audio_ms": length, "processed_ms": length,
                "speech_ms": 0, "clips": own.len(), "lines": own.len(),
                "speakers": speakers, "speakers_reason": speakers_reason,
            })
        })
        .collect();
    let mut labels: Vec<(u32, String)> = said
        .iter()
        .filter_map(|line| Some((line.4?, participant(audio[line.0]))))
        .collect();
    labels.sort();
    labels.dedup();
    let mut participants: Vec<String> = audio.iter().map(|segment| participant(segment)).collect();
    participants.sort();
    participants.dedup();
    json!({
        "format_version": 1,
        "recording_id": detail.recording.id,
        "language": detail.recording.language,
        "participants": participants
            .iter()
            .map(|id| json!({"id": id, "name": "A name"}))
            .collect::<Vec<_>>(),
        "lines": lines,
        "segments": segments,
        "glossary": {"token_limit": 200, "token_count": 0, "used": glossary.0, "cut": glossary.1},
        "speaker_separation": {
            "requested": !labels.is_empty(),
            "speakers": labels
                .iter()
                .map(|(number, id)| json!({"label": format!("Speaker {number}"), "participant_id": id}))
                .collect::<Vec<_>>(),
        },
        "engine": {"worker": "0.1.0", "device": "cpu", "compute_type": "int8", "threads": 4,
                   "models": [], "libraries": {}, "settings": {}},
        "timing": {"total_ms": 1, "audio_ms": 1, "speech_ms": 1, "stages_ms": {},
                   "peak_memory_mb": null},
    })
}

impl TestServer {
    pub async fn transcript(
        &self,
        member: &Member,
        recording_id: RecordingId,
    ) -> junto_protocol::transcript::Transcript {
        self.get(
            member,
            &format!("/api/v1/recordings/{recording_id}/transcript"),
        )
        .await
        .ok(StatusCode::OK)
    }

    /// Every line of a transcript, read in pages of `page` lines.
    pub async fn transcript_lines(
        &self,
        member: &Member,
        recording_id: RecordingId,
        page: u32,
    ) -> Vec<junto_protocol::transcript::TranscriptLine> {
        let mut lines = Vec::new();
        let mut after: Option<u32> = None;
        loop {
            let mut path =
                format!("/api/v1/recordings/{recording_id}/transcript/lines?limit={page}");
            if let Some(after) = after {
                path.push_str(&format!("&after={after}"));
            }
            let read: junto_protocol::transcript::TranscriptLinePage =
                self.get(member, &path).await.ok(StatusCode::OK);
            lines.extend(read.lines);
            match read.next_after {
                Some(next) => after = Some(next),
                None => return lines,
            }
        }
    }
}

/// Waits until `holds`, for a server whose tasks run by themselves.
pub async fn until(what: &str, mut holds: impl AsyncFnMut() -> bool) {
    let started = std::time::Instant::now();
    while !holds().await {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(120),
            "after 120 seconds this did not hold: {what}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}
