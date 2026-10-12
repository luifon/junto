//! The hand-over of a recording to the transcription worker (ADR 0607, ADR 0611).
//!
//! The worker is a program without a network that reads one job file and writes one
//! transcript file (ADR 0610). The server runs it as a child process: it takes the oldest
//! job that waits, writes the job file, starts the worker, states at intervals that it
//! still runs the job, ends the worker at the time limit, reads and checks the transcript,
//! and stores it. One job runs at a time for all server processes.

use std::io;
use std::path::Path;
use std::process::{ExitStatus, Stdio};

use junto_protocol::ids::RecordingId;
use junto_protocol::transcript::{LineSource, TranscriptFailure};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};

use crate::config::TranscriberSettings;
use crate::error::ApiResult;
use crate::recordings::contract::{self, Job, JobSegment, Participant};
use crate::recordings::files::{self, JOB_FILE, JOB_TRANSCRIPT, Place};
use crate::recordings::store::{self, Of};
use crate::recordings::transcripts::{self, AudioSegment, Ended};
use crate::state::AppState;
use crate::{read, transact};

/// The exit codes of the worker (`workers/transcription/README.md`).
const EXIT_JOB_REFUSED: i32 = 2;
const EXIT_NOT_READY: i32 = 3;

/// The longest line of the worker's log that the server passes on.
const LONGEST_LOG_LINE: usize = 512;
/// The interval at which the server looks whether the worker has ended.
const ENDED_CHECK: std::time::Duration = std::time::Duration::from_millis(50);
/// The time in which the log of a run that has ended closes.
const LOG_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// The job of a recording as the worker reads it. The identifiers of the job are the
/// identifiers of the server; the paths are the names of the audio files in the directory
/// of the recording, which is the audio root of the worker.
fn job(
    recording_id: RecordingId,
    language: &str,
    glossary: Vec<String>,
    separate_speakers: bool,
    timeline: time::OffsetDateTime,
    segments: &[AudioSegment],
) -> Job {
    let segments = segments
        .iter()
        .filter_map(|segment| {
            let since = (segment.starts_at? - timeline).whole_milliseconds();
            let place = Place::track(recording_id, segment.id);
            let name = Some(segment.name.as_str()).filter(|name| contract::is_text(name));
            segment.has_file.then(|| JobSegment {
                id: segment.id.to_string(),
                path: place.parts().last().cloned().unwrap_or_default(),
                kind: segment.source,
                participant: Participant {
                    id: segment.account_id.to_string(),
                    name: name.map_or_else(|| segment.account_id.to_string(), str::to_owned),
                },
                start_ms: u64::try_from(since).unwrap_or(0),
            })
        })
        .collect();
    Job {
        format_version: contract::FORMAT_VERSION,
        recording_id: recording_id.to_string(),
        language: language.to_owned(),
        glossary: glossary
            .into_iter()
            .filter(|term| contract::is_term(term))
            .collect(),
        separate_speakers,
        segments,
    }
}

/// What a run of the worker needs.
struct Prepared {
    job: Job,
    segments: Vec<AudioSegment>,
}

async fn prepare(
    app: &AppState,
    settings: &TranscriberSettings,
    id: RecordingId,
) -> ApiResult<Option<Prepared>> {
    read!(app.background(), |connection| {
        let Some(recording) = store::recording(connection, Of::Id(id), false).await? else {
            return Ok(None);
        };
        let segments = transcripts::audio_segments(connection, id).await?;
        let glossary = transcripts::glossary(connection, recording.workspace_id).await?;
        let Some(timeline) = recording.timeline_started_at else {
            return Ok(None);
        };
        let separate = settings.separate_speakers
            && segments
                .iter()
                .any(|segment| segment.source == LineSource::Shared);
        let job = job(
            id,
            &recording.language,
            glossary,
            separate,
            timeline,
            &segments,
        );
        Ok((!job.segments.is_empty()).then_some(Prepared { job, segments }))
    })
}

/// The processes of one run of the worker. The command of the worker can be a script that
/// starts the worker, so the run is a process group of its own, and the server ends the
/// group, not one process. Dropping the value ends what is left of the group.
struct Group {
    #[cfg(unix)]
    leader: Option<rustix::process::Pid>,
}

impl Group {
    /// Resolves when the worker has ended. The worker is not collected: its process
    /// identifier, which is the identifier of the group, stays taken until the caller has
    /// ended the group and waits for the worker.
    async fn worker_ended(&self, child: &mut Child) {
        #[cfg(unix)]
        if let Some(leader) = self.leader {
            use rustix::process::{WaitId, WaitIdOptions, waitid};
            let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
            // An error means that the worker is no child of this process any more.
            while let Ok(None) = waitid(WaitId::Pid(leader), options) {
                tokio::time::sleep(ENDED_CHECK).await;
            }
            return;
        }
        let _ = child.wait().await;
    }

    fn of(child: &Child) -> Self {
        #[cfg(not(unix))]
        let _ = child;
        Self {
            #[cfg(unix)]
            leader: child
                .id()
                .and_then(|id| i32::try_from(id).ok())
                .and_then(rustix::process::Pid::from_raw),
        }
    }

    #[cfg(unix)]
    fn signal(&self, signal: rustix::process::Signal) {
        if let Some(leader) = self.leader {
            // No process of the group is left: nothing to end.
            let _ = rustix::process::kill_process_group(leader, signal);
        }
    }

    /// Asks every process of the run to end, and ends them when the worker does not end
    /// within `grace`.
    async fn end(&mut self, child: &mut Child, grace: std::time::Duration) {
        #[cfg(unix)]
        self.signal(rustix::process::Signal::TERM);
        let _ = tokio::time::timeout(grace, self.worker_ended(child)).await;
        self.finish();
        if let Err(error) = child.kill().await {
            tracing::warn!(%error, "the transcription worker could not be ended");
        }
    }

    /// Ends every process that the run left behind.
    fn finish(&mut self) {
        #[cfg(unix)]
        {
            self.signal(rustix::process::Signal::KILL);
            self.leader = None;
        }
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Passes the log of the worker on. Its lines hold positions, counts and fixed codes, and
/// no value of the job (ADR 0610).
async fn pass_log_on(errors: tokio::process::ChildStderr) {
    let mut reader = BufReader::new(errors);
    let mut line = Vec::new();
    loop {
        line.clear();
        // A line that is longer than the bound is passed on in parts.
        let mut bounded = (&mut reader).take(8 * LONGEST_LOG_LINE as u64);
        match bounded.read_until(b'\n', &mut line).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let text = String::from_utf8_lossy(&line);
                let text: String = text.trim_end().chars().take(LONGEST_LOG_LINE).collect();
                tracing::info!(target: "junto_server::transcription_worker", "{text}");
            }
        }
    }
}

async fn renew(app: &AppState, id: RecordingId) -> ApiResult<bool> {
    let now = app.now();
    transact!(app.background(), |connection| {
        transcripts::renew(connection, now, id, app.recordings.instance()).await
    })
}

/// How a run of the worker ended.
enum Run {
    Exited(ExitStatus),
    TimeLimit,
    /// The job is no longer the job of this server process.
    Abandoned,
    NotStarted(io::Error),
}

/// Runs the worker for one job and waits for its end, within the time limit of the server.
/// While it waits, it renews the heartbeat of the job. When it returns, no process of the
/// run is left.
async fn run_worker(
    app: &AppState,
    settings: &TranscriberSettings,
    id: RecordingId,
    job_directory: &Path,
    audio_root: &Path,
) -> Run {
    let limits = &app.config.limits.recordings.transcription;
    let Some((program, arguments)) = settings.command.split_first() else {
        return Run::NotStarted(io::Error::other("the command of the worker is empty"));
    };
    let mut command = Command::new(program);
    command
        .args(arguments)
        .arg("run")
        .arg(job_directory.join(JOB_FILE))
        .arg("--out")
        .arg(job_directory.join(JOB_TRANSCRIPT))
        .arg("--audio-root")
        .arg(audio_root)
        .args(
            settings
                .batch_size
                .map(|size| ["--batch-size".to_owned(), size.to_string()])
                .into_iter()
                .flatten(),
        )
        .env_clear()
        .envs(
            settings
                .environment
                .iter()
                .map(|(name, value)| (name, value)),
        )
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    if let Some(path) = std::env::var_os("PATH") {
        command.env("PATH", path);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return Run::NotStarted(error),
    };
    let mut group = Group::of(&child);
    let log = child
        .stderr
        .take()
        .map(|errors| tokio::spawn(pass_log_on(errors)));
    let mut heartbeat = tokio::time::interval(limits.heartbeat.unsigned_abs());
    heartbeat.tick().await;
    let time_limit = tokio::time::sleep(limits.time_limit.unsigned_abs());
    tokio::pin!(time_limit);
    let grace = limits.end_grace.unsigned_abs();
    let run = loop {
        tokio::select! {
            () = group.worker_ended(&mut child) => {
                // The group is ended while the identifier of the worker is still taken.
                group.finish();
                match child.wait().await {
                    Ok(status) => break Run::Exited(status),
                    Err(error) => break Run::NotStarted(error),
                }
            }
            () = &mut time_limit => {
                group.end(&mut child, grace).await;
                break Run::TimeLimit;
            }
            _ = heartbeat.tick() => {
                // A failure to reach the database does not end the worker: the job stays
                // the job of this process until another process takes it.
                if matches!(renew(app, id).await, Ok(false)) {
                    group.end(&mut child, grace).await;
                    break Run::Abandoned;
                }
            }
        }
    };
    group.finish();
    if let Some(mut log) = log {
        // Every process that held the log is ended. A log that still does not close does
        // not hold the server.
        if tokio::time::timeout(LOG_GRACE, &mut log).await.is_err() {
            log.abort();
        }
    }
    run
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|error| Err(io::Error::other(error)))
}

/// Reads the transcript that the worker wrote. A file with more bytes than the server
/// accepts is an error, also when it grows while it is read.
async fn read_transcript(root: &Path, place: Place, most: u64) -> io::Result<Vec<u8>> {
    let root = root.to_path_buf();
    blocking(move || {
        let file = files::open(&root, &place)?;
        let mut bytes = Vec::new();
        io::Read::read_to_end(
            &mut io::Read::take(file, most.saturating_add(1)),
            &mut bytes,
        )?;
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > most {
            return Err(io::Error::other(
                "the transcript is larger than the server reads",
            ));
        }
        Ok(bytes)
    })
    .await
}

/// Records the event of a recording whose transcript changed its state, in the transaction
/// of the change (ADR 0011).
async fn announce(
    connection: &mut sqlx::PgConnection,
    now: time::OffsetDateTime,
    id: RecordingId,
) -> ApiResult<()> {
    match store::recording(&mut *connection, Of::Id(id), false).await? {
        Some(recording) => store::announce(connection, now, &recording, false).await,
        None => Ok(()),
    }
}

/// Runs the job of one recording, which this server process has claimed, to its end.
async fn transcribe(
    app: &AppState,
    settings: &TranscriberSettings,
    id: RecordingId,
    attempt: u32,
) -> ApiResult<()> {
    let Some(backend) = app.recordings.backend() else {
        return Ok(());
    };
    let limits = &app.config.limits.recordings.transcription;
    let instance = app.recordings.instance();
    let root = backend.settings.directory.clone();
    let job_place = Place::job_run(id, attempt);
    let ended = |ended: Ended| async move {
        let now = app.now();
        transact!(app.background(), |connection| {
            let (attempts, wait) = (limits.attempts, limits.retry_wait);
            let state =
                transcripts::ended_without(connection, now, id, instance, ended, attempts, wait)
                    .await?;
            if state.is_some() {
                announce(connection, now, id).await?;
            }
            Ok(())
        })
    };

    let Some(Prepared { job, segments }) = prepare(app, settings, id).await? else {
        return ended(Ended::Failed(TranscriptFailure::JobRefused)).await;
    };
    // What the runs of earlier attempts left is deleted: a process of such a run has lost
    // the job, and its result is not stored. The directory of a later attempt belongs to a
    // process that took the job from this one, and stays.
    let written = {
        let (root, job_place) = (root.clone(), job_place.clone());
        let document = serde_json::to_vec(&job);
        blocking(move || {
            for earlier in files::earlier_runs(&root, id, attempt)? {
                files::remove(&root, &earlier)?;
            }
            files::create_directory(&root, &job_place)?;
            let document = document.map_err(io::Error::other)?;
            files::write_new(&root, &job_place.with(JOB_FILE), &document)
        })
        .await
    };
    let run = match written {
        Ok(()) => {
            let audio_root = Place::recording(id).under(&root);
            run_worker(app, settings, id, &job_place.under(&root), &audio_root).await
        }
        Err(error) => Run::NotStarted(error),
    };
    let outcome = match run {
        Run::Abandoned => Ok(None),
        Run::NotStarted(error) => {
            tracing::warn!(%error, "the transcription worker did not start");
            Err(Ended::TryAgain(TranscriptFailure::WorkerFailed))
        }
        Run::TimeLimit => Err(Ended::Failed(TranscriptFailure::TimeLimit)),
        Run::Exited(status) if status.success() => {
            let most = limits.most_transcript_bytes;
            let place = job_place.with(JOB_TRANSCRIPT);
            let read = read_transcript(&root, place, most)
                .await
                .map_err(|error| error.to_string())
                .and_then(|bytes| contract::read(&bytes, &job).map(|read| (read, bytes)));
            match read {
                Ok(read) => Ok(Some(read)),
                Err(reason) => {
                    tracing::warn!(reason, "the transcript of the worker was refused");
                    Err(Ended::TryAgain(TranscriptFailure::TranscriptInvalid))
                }
            }
        }
        Run::Exited(status) => {
            tracing::warn!(%status, "the transcription worker ended with an error");
            Err(match status.code() {
                Some(EXIT_JOB_REFUSED) => Ended::Failed(TranscriptFailure::JobRefused),
                Some(EXIT_NOT_READY) => Ended::Failed(TranscriptFailure::WorkerNotReady),
                _ => Ended::TryAgain(TranscriptFailure::WorkerFailed),
            })
        }
    };
    // The file of the worker is kept beside the files of the recording in the transaction
    // that stores the transcript, after the check that this process holds the job: a stored
    // transcript has its file, and a process that lost the job writes nothing. A
    // transaction that runs again writes the same bytes again.
    let finished = match outcome {
        Ok(Some((transcript, bytes))) => {
            let now = app.now();
            let bytes = std::sync::Arc::new(bytes);
            let kept = transact!(app.background(), |connection| {
                if !transcripts::holds(connection, id, instance).await? {
                    return Ok(true);
                }
                let (root, kept, bytes) = (root.clone(), Place::transcript(id), bytes.clone());
                let written = blocking(move || {
                    files::remove(&root, &kept)?;
                    files::write_new(&root, &kept, &bytes)
                })
                .await;
                if let Err(error) = written {
                    tracing::warn!(%error, "the transcript file could not be kept");
                    return Ok(false);
                }
                transcripts::store(connection, now, id, instance, &segments, &transcript).await?;
                announce(connection, now, id).await?;
                Ok(true)
            })?;
            if kept {
                Ok(())
            } else {
                ended(Ended::TryAgain(TranscriptFailure::WorkerFailed)).await
            }
        }
        Ok(None) => Ok(()),
        Err(without) => ended(without).await,
    };
    // Only the directory of this run is deleted: another server process can have taken the
    // job and run its own worker.
    let cleared = blocking(move || {
        files::remove(&root, &job_place)?;
        files::remove_if_empty(&root, &Place::jobs(id))
    })
    .await;
    if let Err(error) = cleared {
        tracing::warn!(%error, "the directory of a transcription job could not be deleted");
    }
    finished
}

/// Takes the oldest job that waits and runs it to its end. Returns false when no job was
/// taken: none waits, another runs, a recording runs, or the server has no worker. Tests
/// run the transcription with this, so that the order of things is fixed.
pub async fn run_once(app: &AppState) -> ApiResult<bool> {
    let settings = app
        .recordings
        .backend()
        .and_then(|backend| backend.settings.transcriber.as_ref());
    let Some(settings) = settings else {
        return Ok(false);
    };
    let limits = &app.config.limits.recordings.transcription;
    let now = app.now();
    let claimed = transact!(app.background(), |connection| {
        let instance = app.recordings.instance();
        let claim =
            transcripts::claim(connection, now, instance, limits.lease, limits.attempts).await?;
        for id in &claim.changed {
            announce(connection, now, *id).await?;
        }
        Ok(claim.claimed)
    })?;
    let Some((id, attempt)) = claimed else {
        return Ok(false);
    };
    transcribe(app, settings, id, attempt).await?;
    Ok(true)
}

/// Runs [`run_once`] at each interval, and at once when a job was stored, until the task is
/// dropped. A job that ended is followed by the next one at once. The worker process of a
/// task that is dropped is ended; its job is taken again when its heartbeat is old.
pub async fn run(app: AppState) {
    let interval = app
        .config
        .limits
        .recordings
        .transcription
        .check_interval
        .unsigned_abs();
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticks.tick() => {}
            () = app.recordings.asked_to_transcribe() => {}
        }
        loop {
            match run_once(&app).await {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => {
                    tracing::warn!(code = ?error.code(), "a transcription job failed");
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use junto_protocol::ids::{AccountId, RecordingSegmentId};
    use time::{Duration, OffsetDateTime};

    use super::*;

    fn audio(
        source: LineSource,
        name: &str,
        starts_at: Option<i64>,
        has_file: bool,
    ) -> AudioSegment {
        AudioSegment {
            id: RecordingSegmentId::generate(),
            source,
            account_id: AccountId::generate(),
            name: name.to_owned(),
            starts_at: starts_at
                .map(|after| OffsetDateTime::UNIX_EPOCH + Duration::milliseconds(after)),
            has_file,
            media_ms: Some(2000),
        }
    }

    #[test]
    fn the_job_names_the_audio_files_that_have_a_place_on_the_timeline() {
        let recording_id = RecordingId::generate();
        let segments = [
            audio(LineSource::Microphone, "Ann", Some(1500), true),
            audio(LineSource::Shared, "Ann", Some(76_700), true),
            audio(LineSource::Microphone, "Ben", None, true),
            audio(LineSource::Microphone, "Cy", Some(900), false),
            audio(LineSource::Microphone, "line\nbreak", Some(1500), true),
        ];
        let glossary = vec![
            "Quorvex".to_owned(),
            "<|endoftext|>".to_owned(),
            "Lumitrax".to_owned(),
        ];
        let timeline = OffsetDateTime::UNIX_EPOCH + Duration::milliseconds(1500);
        let job = job(recording_id, "pt", glossary, true, timeline, &segments);
        assert_eq!(job.format_version, 1);
        assert_eq!(job.recording_id, recording_id.to_string());
        assert_eq!(job.language, "pt");
        assert_eq!(job.glossary, ["Quorvex", "Lumitrax"]);
        assert!(job.separate_speakers);
        assert_eq!(job.segments.len(), 3);
        let first = &job.segments[0];
        assert_eq!(first.id, segments[0].id.to_string());
        assert_eq!(first.path, format!("{}.ogg", segments[0].id));
        assert_eq!(first.kind, LineSource::Microphone);
        assert_eq!(first.participant.id, segments[0].account_id.to_string());
        assert_eq!(first.participant.name, "Ann");
        assert_eq!(first.start_ms, 0);
        assert_eq!(
            (job.segments[1].kind, job.segments[1].start_ms),
            (LineSource::Shared, 75_200)
        );
        // A name that the job format refuses is replaced by the identifier of the account.
        assert_eq!(
            job.segments[2].participant.name,
            segments[4].account_id.to_string()
        );
        let in_job: Vec<bool> = segments.iter().map(AudioSegment::is_transcribed).collect();
        assert_eq!(in_job, [true, true, false, false, true]);
    }
}
