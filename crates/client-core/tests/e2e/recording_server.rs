//! The server of this checkout inside the test process, with a media server and a recorder
//! in memory and a stand-in for the transcription worker. The server binary cannot use the
//! recorder in memory, so a test that needs a finished recording with real files starts
//! the server with this file.
//!
//! The server runs on a runtime of its own. A test awaits the functions of this file from
//! any executor. The tasks of calls and of recordings do not run by themselves: each
//! function of this file runs the passes that it needs, so the order of things is fixed.
//! The work of the server is not handed to a task, because its futures are not `Send` for
//! every lifetime: a thread runs each piece of work on the runtime of the server.
//!
//! The tests of recordings need `ffmpeg` and `ffprobe` with the encoders `libopus` and
//! `libx264` on the path, or named by `JUNTO_FFMPEG` and `JUNTO_FFPROBE` (ADR 0606).

use std::future::Future;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use junto_client_core::ServerHandle;
use junto_client_core::protocol::ids::{AccountId, ChannelId, RecordingId};
use junto_client_core::protocol::origin::Origin;
use junto_client_core::protocol::recording::{
    RecordingDetail, RecordingSegment, RecordingState, SegmentSource,
};
use junto_server::Tasks;
use junto_server::calls::admission;
use junto_server::clock::Clock;
use junto_server::config::{
    Config, Limits, LogFormat, PublicOrigins, RecorderSettings, TranscriberSettings,
};
use junto_server::database::Database;
use junto_server::media::fake::FakeMedia;
use junto_server::media::{MediaServer, TrackKind, TrackSource};
use junto_server::recorder::fake::FakeRecorder;
use junto_server::recorder::samples;
use junto_server::recordings::{Backend, control, process, transcribe};
use junto_server::state::AppState;
use junto_server::storage::FakeDisk;
use serde_json::{Value, json};
use sqlx::ConnectOptions;
use sqlx::postgres::PgConnectOptions;
use time::OffsetDateTime;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

const DATABASE_URL: &str = "JUNTO_TEST_DATABASE_URL";

/// The stand-in for the transcription worker (ADR 0611). The server runs it as
/// `<script> run <job> --out <transcript> --audio-root <directory of the recording>`. It
/// writes the transcript that the test gave for that recording, and fails without one.
const WORKER_SCRIPT: &str = r#"#!/bin/sh
here=$(dirname "$0")
given="$here/$(basename "$6").json"
[ -f "$given" ] || exit 1
cp "$given" "$4.tmp" && mv "$4.tmp" "$4"
"#;

/// The time of the machine, and ahead of it by what a test added. A recording gets the
/// length of its files in this way.
#[derive(Default)]
struct TestClock {
    ahead: Mutex<time::Duration>,
}

impl TestClock {
    fn advance(&self, duration: time::Duration) {
        *self.ahead.lock().unwrap() += duration;
    }
}

impl Clock for TestClock {
    fn now(&self) -> OffsetDateTime {
        let now = OffsetDateTime::now_utc() + *self.ahead.lock().unwrap();
        // The server stores times to the microsecond.
        now.replace_nanosecond(now.microsecond() * 1_000).unwrap()
    }
}

/// One line that a test lets the transcription worker write. The times count in
/// milliseconds from the start of the timeline of the recording, and lie inside the two
/// seconds that each file of a recording of this server has.
#[derive(Debug, Clone)]
pub struct Spoken {
    /// The person of the microphone, or the person whose screen share holds the sound.
    pub account_id: AccountId,
    /// N of the label `Speaker N`: the line is one of the shared sound of the person.
    /// Absent: the line is one of the microphone of the person.
    pub speaker: Option<u32>,
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

pub struct RecordingServer {
    address: SocketAddr,
    owner_code: String,
    state: AppState,
    clock: Arc<TestClock>,
    media: Arc<FakeMedia>,
    recorder: Arc<FakeRecorder>,
    /// The directory of the recordings.
    directory: tempfile::TempDir,
    /// The script of the worker, and the transcripts that the tests gave.
    worker: tempfile::TempDir,
    _attachments: tempfile::TempDir,
    /// The recordings that [`RecordingServer::record`] made.
    recorded: Mutex<Vec<RecordingDetail>>,
    runtime: Option<tokio::runtime::Runtime>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl Drop for RecordingServer {
    fn drop(&mut self) {
        drop(self.shutdown.take());
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

/// Runs work to its end on a runtime, from a thread of its own, and returns its result.
async fn on_runtime<T, F>(
    runtime: tokio::runtime::Handle,
    work: impl FnOnce() -> F + Send + 'static,
) -> T
where
    F: Future<Output = T>,
    T: Send + 'static,
{
    let (done, result) = oneshot::channel();
    std::thread::spawn(move || {
        let _ = done.send(runtime.block_on(work()));
    });
    result.await.expect("the work of the test server ended")
}

fn program(variable: &str, name: &str) -> PathBuf {
    std::env::var_os(variable).map_or_else(|| name.into(), PathBuf::from)
}

impl RecordingServer {
    /// Starts a server on a new database, with a recorder and a transcription worker. The
    /// server has no owner yet: the first device claims it with the owner code.
    pub async fn start(name: &str) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("junto-test-server")
            .enable_all()
            .build()
            .unwrap();
        let directory = tempfile::tempdir().unwrap();
        let worker = tempfile::tempdir().unwrap();
        let attachments = tempfile::tempdir().unwrap();
        let script = worker.path().join("worker.sh");
        std::fs::write(&script, WORKER_SCRIPT).unwrap();
        let recorder_settings = RecorderSettings {
            directory: directory.path().to_path_buf(),
            recorder_directory: "/out".to_owned(),
            layout_url: "http://server.example.test/layout/".to_owned(),
            ffmpeg: program("JUNTO_FFMPEG", "ffmpeg"),
            ffprobe: program("JUNTO_FFPROBE", "ffprobe"),
            transcriber: Some(TranscriberSettings {
                batch_size: None,
                command: vec!["/bin/sh".to_owned(), script.to_str().unwrap().to_owned()],
                environment: Vec::new(),
                separate_speakers: true,
            }),
        };
        let attachments_directory = attachments.path().canonicalize().unwrap();
        let name = name.to_owned();
        let clock = Arc::new(TestClock::default());
        let server_clock = clock.clone();
        let (shutdown, stopped) = oneshot::channel::<()>();
        let started = on_runtime(runtime.handle().clone(), move || async move {
            let admin_url = std::env::var(DATABASE_URL).unwrap_or_else(|_| {
                panic!(
                    "{DATABASE_URL} is not set. Run the tests through tools/with-test-postgres.sh."
                )
            });
            let admin = PgConnectOptions::from_str(&admin_url).unwrap();
            let database = format!("junto_client_{}", uuid::Uuid::new_v4().simple());
            let mut connection = admin.connect().await.unwrap();
            sqlx::query(sqlx::AssertSqlSafe(format!("create database {database}")))
                .execute(&mut connection)
                .await
                .unwrap();
            drop(connection);

            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let origin = Origin::parse(&format!("http://{address}")).unwrap();
            let config = Config {
                exports_directory: None,
                migration_copy: junto_server::bootstrap::MigrationCopy::None,
                recorder_layout_directory: None,
                attachments_directory: Some(attachments_directory),
                database_url: String::new(),
                listen_address: address,
                public_origins: PublicOrigins::of(vec![origin]),
                server_name: name,
                trusted_proxies: Vec::new(),
                log_format: LogFormat::Text,
                log_level: tracing::Level::ERROR,
                media: None,
                recorder: Some(recorder_settings.clone()),
                limits: Limits::default(),
            };
            let limits = &config.limits;
            let connected = Database::connect(
                admin.database(&database),
                8,
                &limits.database,
                &limits.stream.pool(),
            )
            .await
            .unwrap();
            let media = Arc::new(FakeMedia::new(server_clock.clone()));
            let recorder = Arc::new(FakeRecorder::new(server_clock.clone(), media.clone()));
            recorder.write_files(
                &recorder_settings.recorder_directory,
                &recorder_settings.directory,
            );
            let backend = Backend::new(
                recorder.clone(),
                Arc::new(FakeDisk::new(1 << 40)),
                recorder_settings,
                &limits.recordings,
            );
            let media_server: Arc<dyn MediaServer> = media.clone();
            let prepared = junto_server::prepare_with(
                connected,
                config,
                server_clock,
                Some(media_server),
                Some(backend),
            )
            .await
            .unwrap();
            let state = prepared.state.clone();
            let owner_code = prepared
                .owner_code
                .map(|code| code.expose().to_owned())
                .unwrap_or_default();
            let stop = async move {
                let _ = stopped.await;
            };
            let tasks = Tasks { calls: false };
            tokio::spawn(junto_server::serve_with(
                listener,
                prepared.state,
                stop,
                tasks,
            ));
            (address, owner_code, state, media, recorder)
        });
        let (address, owner_code, state, media, recorder) = started.await;
        Self {
            address,
            owner_code,
            state,
            clock,
            media,
            recorder,
            directory,
            worker,
            _attachments: attachments,
            recorded: Mutex::default(),
            runtime: Some(runtime),
            shutdown: Some(shutdown),
        }
    }

    pub fn address(&self) -> String {
        format!("http://{}", self.address)
    }

    /// The owner code of the first start of the server.
    pub fn owner_code(&self) -> String {
        self.owner_code.clone()
    }

    /// The directory of the recordings. The file of a segment is
    /// `<recording>/<segment>.mp4` for the video and `<recording>/<segment>.ogg` for an
    /// audio track (ADR 0606).
    pub fn recordings_dir(&self) -> PathBuf {
        self.directory.path().to_path_buf()
    }

    /// Runs work of the server on the runtime of the server.
    async fn on<T, F>(&self, work: impl FnOnce(AppState) -> F + Send + 'static) -> T
    where
        F: Future<Output = T>,
        T: Send + 'static,
    {
        let runtime = self.runtime.as_ref().expect("a running server");
        let state = self.state.clone();
        on_runtime(runtime.handle().clone(), move || work(state)).await
    }

    /// One admission check: the server takes what the media server holds.
    async fn check(&self) {
        self.on(|state| async move {
            admission::check(&state).await.unwrap();
        })
        .await;
    }

    /// One pass of the controller of recordings. Before it, the recorder ends the files
    /// of the tracks that have ended.
    async fn tend(&self) {
        self.recorder.work();
        self.on(|state| async move {
            control::tend_all(&state).await.unwrap();
        })
        .await;
    }

    /// Makes a finished recording of the call of a voice channel and returns its
    /// identifier. Each person enters the call with a microphone; the first one also
    /// shares the sound of a screen, starts the recording and stops it, so that account
    /// needs "record calls" in the channel. The files are processed, every person leaves
    /// the call, and the transcript of the recording waits for
    /// [`RecordingServer::transcribe`].
    ///
    /// The recording has the video of the composite, one audio file for each microphone,
    /// and one for the shared sound, each of two seconds.
    pub async fn record(&self, channel_id: ChannelId, people: &[&ServerHandle]) -> RecordingId {
        let starter = people.first().expect("a person who records");
        // The server lets a person into a call only while its checks reach the media
        // server (ADR 0509).
        self.check().await;
        let mut seats = Vec::new();
        for (position, person) in people.iter().enumerate() {
            let join = person.api().join_channel_call(channel_id).await.unwrap();
            let (room, identity) = self
                .media
                .connect(&join.token)
                .expect("a token that the media server accepts");
            self.check().await;
            let publish = |source| {
                let published = self
                    .media
                    .publish(&room, &identity, TrackKind::Audio, source);
                assert!(published, "the person may not publish {source:?}");
            };
            publish(TrackSource::Microphone);
            if position == 0 {
                publish(TrackSource::ScreenShareAudio);
            }
            seats.push((room, identity));
        }

        let started = starter
            .api()
            .start_recording(channel_id, None)
            .await
            .unwrap();
        self.tend().await;
        // The recorder reports the length of the files that it writes.
        self.clock
            .advance(time::Duration::seconds(i64::from(samples::SECONDS)));
        starter.api().stop_recording(channel_id).await.unwrap();
        // The stops, the ends of the jobs, and the last questions about them.
        for _ in 0..3 {
            self.tend().await;
        }
        let processed = self
            .on(|state| async move { process::run_once(&state).await.unwrap() })
            .await;
        assert_eq!(processed, 1, "the files of one recording were processed");

        for (person, (room, identity)) in people.iter().zip(&seats) {
            person.api().leave_call().await.unwrap();
            self.media.leave(room, identity);
        }
        self.check().await;

        let detail = starter.api().recording(started.id).await.unwrap();
        assert_eq!(detail.recording.state, RecordingState::Ready, "{detail:?}");
        self.recorded.lock().unwrap().push(detail);
        started.id
    }

    /// Gives a recording of [`RecordingServer::record`] its transcript: the worker writes
    /// these lines, and the server stores them. The lines are stored in the order of
    /// their start times.
    ///
    /// The server runs one transcription job at a time, the oldest first. A test that
    /// makes several recordings gives them their transcripts in the order in which it
    /// made them.
    pub async fn transcribe(&self, recording_id: RecordingId, lines: &[Spoken]) {
        let detail = self
            .recorded
            .lock()
            .unwrap()
            .iter()
            .find(|detail| detail.recording.id == recording_id)
            .cloned()
            .expect("a recording that this server made");
        let written = serde_json::to_vec(&transcript_of(&detail, lines)).unwrap();
        let given = self.worker.path().join(format!("{recording_id}.json"));
        std::fs::write(given, written).unwrap();
        let state: String = self
            .on(move |state| async move {
                assert!(
                    transcribe::run_once(&state).await.unwrap(),
                    "the server took no transcription job"
                );
                sqlx::query_scalar("select state::text from transcripts where recording_id = $1")
                    .bind(recording_id.as_uuid())
                    .fetch_one(state.database.pool())
                    .await
                    .unwrap()
            })
            .await;
        assert_eq!(
            state, "ready",
            "the transcript of the recording was not stored. The server runs the oldest job \
             first: give the transcripts in the order of the recordings."
        );
    }
}

/// The audio segments of a recording that have a file, in the order of the job that the
/// server writes for the worker.
fn audio(detail: &RecordingDetail) -> Vec<&RecordingSegment> {
    detail
        .segments
        .iter()
        .filter(|segment| segment.source != SegmentSource::Composite && segment.file.is_some())
        .collect()
}

/// A transcript in the form of ADR 0611 for the job that the server writes for `detail`:
/// one segment for each audio segment with a file, in their order, and the given lines.
fn transcript_of(detail: &RecordingDetail, lines: &[Spoken]) -> Value {
    let audio = audio(detail);
    let shared = |segment: &RecordingSegment| segment.source == SegmentSource::ScreenShareAudio;
    let participant = |segment: &RecordingSegment| segment.account_id.unwrap().to_string();
    // The position of the audio segment that holds a line.
    let segment_of = |line: &Spoken| {
        audio
            .iter()
            .position(|segment| {
                segment.account_id == Some(line.account_id)
                    && shared(segment) == line.speaker.is_some()
            })
            .unwrap_or_else(|| panic!("the recording has no audio for the line {line:?}"))
    };
    let mut ordered: Vec<&Spoken> = lines.iter().collect();
    ordered.sort_by_key(|line| line.start_ms);
    let written: Vec<Value> = ordered
        .iter()
        .map(|line| {
            let segment = audio[segment_of(line)];
            let source = match line.speaker {
                Some(number) => json!({
                    "kind": "shared",
                    "participant_id": participant(segment),
                    "speaker": format!("Speaker {number}"),
                }),
                None => json!({"kind": "microphone", "participant_id": participant(segment)}),
            };
            json!({
                "start_ms": line.start_ms, "end_ms": line.end_ms, "segment_id": segment.id,
                "source": source, "text": line.text, "language": "en", "words": null,
            })
        })
        .collect();
    let segments: Vec<Value> = audio
        .iter()
        .enumerate()
        .map(|(position, segment)| {
            let own = lines
                .iter()
                .filter(|line| segment_of(line) == position)
                .count();
            let length = segment.file.as_ref().unwrap().duration_ms;
            json!({
                "id": segment.id,
                "status": if own == 0 { "empty" } else { "ok" },
                "reason": null, "audio_ms": length, "processed_ms": length,
                "speech_ms": 0, "clips": own, "lines": own,
                "speakers": if shared(segment) { "separated" } else { "not_applicable" },
                "speakers_reason": null,
            })
        })
        .collect();
    let mut labels: Vec<(u32, String)> = lines
        .iter()
        .filter_map(|line| Some((line.speaker?, participant(audio[segment_of(line)]))))
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
        "lines": written,
        "segments": segments,
        "glossary": {"token_limit": 200, "token_count": 0, "used": [], "cut": []},
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
