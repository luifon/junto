//! The processing of the files of a finished recording (ADR 0606).
//!
//! The controller leaves a recording in the state `processing` when every job of the
//! recorder has ended. The processing then finds the file of each segment, takes the file
//! that was saved of a lost job, joins the pieces of each composite into one MP4 file,
//! places each file on the timeline, checks each file, and gives the recording its final
//! state. It also deletes the directories of removed recordings (ADR 0608).
//!
//! Every step can run again: a video that exists is not joined again and never replaced,
//! and nothing is stored before every file is examined.

use std::io;
use std::path::Path;

use junto_protocol::ids::RecordingId;
use junto_protocol::recording::{FileProblem, RecordingState, SegmentSource, SegmentState};
use time::{Duration, OffsetDateTime};

use crate::error::{ApiError, ApiResult};
use crate::recordings::check::{self, Heard, Sound, Timed};
use crate::recordings::files::{self, Place};
use crate::recordings::store::{self, FileRow, Of, RecordingRow, SegmentRow};
use crate::recordings::tools::{Container, ToolError, Tools};
use crate::recordings::{Backend, transcripts};
use crate::state::AppState;
use crate::{read, transact};

/// The directories that one sweep deletes.
const REMOVALS_AT_ONCE: i64 = 50;
/// The directories below `rescued/` that one sweep examines.
pub const RESCUED_AT_ONCE: usize = 50;
/// The part of the sound of a composite and of a microphone file that places a composite
/// without a reported end, and how far the two can be apart (ADR 0602).
const PLACED_BY: Duration = Duration::seconds(60);
const SEARCHED: Duration = Duration::seconds(6);
/// Less common sound than this places nothing.
const LEAST_COMMON_SOUND: Duration = Duration::seconds(4);

/// A step that did not run to its end: a program did not start or reached its time limit,
/// or the file system refused. The attempt is repeated.
struct Unfinished;

fn unfinished(what: &str, error: &dyn std::fmt::Display) -> Unfinished {
    tracing::warn!(%error, "{what}");
    Unfinished
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .unwrap_or_else(|error| Err(io::Error::other(error)))
}

/// One file with what the programs measured of it.
struct Measured {
    row: FileRow,
    timed: Option<Timed>,
    sound: Option<Sound>,
}

impl Measured {
    fn add(&mut self, problem: FileProblem) {
        if !self.row.problems.contains(&problem) {
            self.row.problems.push(problem);
        }
    }

    /// The sound of the file on the timeline, when the file has a place and sound.
    fn heard(&self) -> Option<Heard> {
        Some(Heard::of(
            self.row.starts_at?,
            self.timed.as_ref()?,
            self.sound.as_ref()?,
        ))
    }
}

/// A file of a recording as the programs get it: opened by the server at each use, without
/// following a link, and given with its form.
struct Source<'a> {
    root: &'a Path,
    place: Place,
    container: Container,
}

impl<'a> Source<'a> {
    fn of(root: &'a Path, recording: RecordingId, segment: &SegmentRow) -> Self {
        let (place, container) = if segment.is_track() {
            (Place::track(recording, segment.id), Container::Track)
        } else {
            (Place::video(recording, segment.id), Container::Video)
        };
        Self {
            root,
            place,
            container,
        }
    }

    async fn open(&self) -> Result<std::fs::File, Unfinished> {
        let (root, place) = (self.root.to_path_buf(), self.place.clone());
        blocking(move || files::open(&root, &place))
            .await
            .map_err(|error| unfinished("a file of a recording cannot be opened", &error))
    }
}

/// Measures one file and applies the rules that need no other file.
async fn measure(
    tools: &Tools,
    source: &Source<'_>,
    bytes: u64,
    segment: &SegmentRow,
) -> Result<Measured, Unfinished> {
    let picture = !segment.is_track();
    let container = source.container;
    let mut measured = Measured {
        row: FileRow {
            bytes,
            media_ms: 0,
            starts_at: None,
            size: None,
            problems: Vec::new(),
            silent: false,
            recovered: segment.state == SegmentState::Failed,
        },
        timed: None,
        sound: None,
    };
    let unreadable = |error: ToolError| match error {
        ToolError::Unreadable(reason) => {
            tracing::info!(segment = %segment.id, reason, "a file of a recording cannot be read");
            Ok(())
        }
        ToolError::Failed(reason) => Err(unfinished("a file could not be examined", &reason)),
    };
    let timed = match tools.probe(source.open().await?, container).await {
        Ok(probe) => check::timing(&probe, picture),
        Err(error) => {
            unreadable(error)?;
            measured.add(FileProblem::Unreadable);
            return Ok(measured);
        }
    };
    measured.row.media_ms = timed.media_ms;
    measured.row.size = timed.size;
    measured.row.problems.clone_from(&timed.problems);
    if timed.has_sound {
        match tools.sound(source.open().await?, container).await {
            Ok(sound) => {
                measured.row.silent = check::is_silent(&sound);
                measured.sound = Some(sound);
            }
            Err(error) => {
                unreadable(error)?;
                measured.add(FileProblem::Unreadable);
            }
        }
    }
    if picture && timed.has_picture {
        match tools.key_frames(source.open().await?).await {
            Ok(frames) if check::picture_is_blank(&frames) => {
                measured.add(FileProblem::PictureBlank);
            }
            Ok(_) => {}
            Err(error) => {
                unreadable(error)?;
                measured.add(FileProblem::Unreadable);
            }
        }
    }
    let reported = segment
        .duration_ms
        .filter(|_| segment.state == SegmentState::Ended);
    if reported.is_some_and(|reported| check::length_differs(timed.media_ms, reported)) {
        measured.add(FileProblem::LengthMismatch);
    }
    measured.timed = Some(timed);
    Ok(measured)
}

/// Finds the file of an audio track: where the recorder wrote it, or among the files that
/// were saved of its lost job. Returns its bytes.
async fn find_track(
    root: &Path,
    recording: RecordingId,
    segment: &SegmentRow,
) -> io::Result<Option<u64>> {
    let place = Place::track(recording, segment.id);
    let rescued = segment
        .job_id
        .as_deref()
        .and_then(|job_id| Place::rescued_track(job_id, segment.id));
    let root = root.to_path_buf();
    blocking(move || {
        if let Some(bytes) = files::size(&root, &place)? {
            return Ok(Some(bytes));
        }
        let Some(rescued) = rescued else {
            return Ok(None);
        };
        if files::size(&root, &rescued)?.is_none() {
            return Ok(None);
        }
        files::take_rescued(&root, &rescued, &place)?;
        files::size(&root, &place)
    })
    .await
}

/// Finds the video of a composite, and joins it from its pieces when it does not exist.
/// Returns its bytes; `None` when the recorder left no piece.
async fn find_video(
    backend: &Backend,
    recording: RecordingId,
    segment: &SegmentRow,
) -> Result<Option<u64>, Unfinished> {
    let root = backend.settings.directory.clone();
    let video = Place::video(recording, segment.id);
    let pieces = Place::pieces(recording, segment.id);
    let listed = {
        let (root, video, pieces) = (root.clone(), video.clone(), pieces.clone());
        blocking(move || match files::size(&root, &video)? {
            Some(bytes) => Ok((Some(bytes), Vec::new())),
            None => Ok((None, files::pieces(&root, &pieces)?)),
        })
        .await
    };
    let (found, names) =
        listed.map_err(|error| unfinished("the files of a composite cannot be listed", &error))?;
    if found.is_some() || names.is_empty() {
        return Ok(found);
    }
    let unfinished_video = Place::unfinished_video(recording, segment.id);
    let open = |position: usize| files::open(&root, &pieces.with(&names[position]));
    let joined = backend
        .tools
        .join(names.len(), open, &unfinished_video.under(&root))
        .await;
    let finished = {
        let (root, from, to) = (root.clone(), unfinished_video, video.clone());
        blocking(move || match joined {
            Ok(()) => {
                files::finish(&root, &from, &to)?;
                files::size(&root, &to)
            }
            Err(error) => {
                files::remove(&root, &from)?;
                Err(io::Error::other(error))
            }
        })
        .await
    };
    finished.map_err(|error| unfinished("the pieces of a composite could not be joined", &error))
}

/// The time by which the sound is later in a composite than the reported start times of
/// the composite and of a microphone file state, from the loudness of both.
async fn shift_by_sound(
    tools: &Tools,
    composite: (&Source<'_>, OffsetDateTime, i64),
    reference: (&Source<'_>, OffsetDateTime, i64),
) -> Result<Option<Duration>, Unfinished> {
    let (video, video_start, video_ms) = composite;
    let (track, track_start, track_ms) = reference;
    let video_from = (track_start - video_start).max(Duration::ZERO);
    let track_from = (video_start - track_start).max(Duration::ZERO);
    let common = PLACED_BY
        .min(Duration::milliseconds(video_ms) - video_from)
        .min(Duration::milliseconds(track_ms) - track_from);
    if common < LEAST_COMMON_SOUND {
        return Ok(None);
    }
    let loudness = |source: &Source<'_>, file, from: Duration| {
        let (from, length) = (from.as_seconds_f64(), common.as_seconds_f64());
        tools.loudness(file, source.container, from, length)
    };
    let (video_file, track_file) = (video.open().await?, track.open().await?);
    let (of_video, of_track) = tokio::join!(
        loudness(video, video_file, video_from),
        loudness(track, track_file, track_from)
    );
    match (of_video, of_track) {
        (Ok(of_video), Ok(of_track)) => Ok(check::best_shift(&of_video, &of_track, SEARCHED)),
        (Err(ToolError::Failed(reason)), _) | (_, Err(ToolError::Failed(reason))) => Err(
            unfinished("the loudness of a file could not be read", &reason),
        ),
        _ => Ok(None),
    }
}

/// What one attempt found of the files of a recording, one entry for each segment.
struct Examined {
    files: Vec<Option<FileRow>>,
    /// A step did not run to its end, so the attempt decides nothing.
    incomplete: bool,
}

/// Finds, joins, places and checks the files of a recording. Nothing is stored.
async fn examine(backend: &Backend, recording: &RecordingRow, segments: &[SegmentRow]) -> Examined {
    let root = &backend.settings.directory;
    let tools = &backend.tools;
    let mut measured: Vec<Option<Measured>> = Vec::with_capacity(segments.len());
    let mut incomplete = false;

    for segment in segments {
        let found = if segment.is_track() {
            find_track(root, recording.id, segment)
                .await
                .map_err(|error| unfinished("the file of a track cannot be read", &error))
        } else {
            find_video(backend, recording.id, segment).await
        };
        let source = Source::of(root, recording.id, segment);
        let examined = match found {
            Ok(Some(bytes)) => measure(tools, &source, bytes, segment).await.map(Some),
            Ok(None) => Ok(None),
            Err(Unfinished) => Err(Unfinished),
        };
        match examined {
            Ok(Some(mut file)) => {
                if segment.is_track() {
                    file.row.starts_at = segment.started_at.map(check::track_start);
                } else if let Some(ended_at) = segment.ended_at {
                    file.row.starts_at = Some(check::composite_start(ended_at, file.row.media_ms));
                }
                measured.push(Some(file));
            }
            Ok(None) => measured.push(None),
            Err(Unfinished) => {
                incomplete = true;
                measured.push(None);
            }
        }
    }

    // The microphone files that can place a composite and that its sound is compared with:
    // those that passed their own check, have sound and have a place.
    let microphones: Vec<usize> = segments
        .iter()
        .enumerate()
        .filter(|(position, segment)| {
            segment.source == SegmentSource::Microphone
                && measured[*position].as_ref().is_some_and(|file| {
                    file.row.problems.is_empty()
                        && !file.row.silent
                        && file.sound.is_some()
                        && file.row.starts_at.is_some()
                })
        })
        .map(|(position, _)| position)
        .collect();

    for (position, segment) in segments.iter().enumerate() {
        if segment.is_track() {
            continue;
        }
        let needs_place = measured[position]
            .as_ref()
            .is_some_and(|file| file.row.starts_at.is_none() && file.sound.is_some());
        if let (true, Some(video_start)) = (needs_place, segment.started_at) {
            let longest = microphones.iter().copied().max_by_key(|microphone| {
                measured[*microphone].as_ref().map(|file| file.row.media_ms)
            });
            let reference = longest.and_then(|microphone| {
                let start = segments[microphone].started_at?;
                let length = measured[microphone].as_ref()?.row.media_ms;
                let source = Source::of(root, recording.id, &segments[microphone]);
                Some((source, start, length))
            });
            let video_ms = measured[position]
                .as_ref()
                .map_or(0, |file| file.row.media_ms);
            if let Some((track, track_start, track_ms)) = reference {
                let video = Source::of(root, recording.id, segment);
                let shift = shift_by_sound(
                    tools,
                    (&video, video_start, video_ms),
                    (&track, track_start, track_ms),
                )
                .await;
                match shift {
                    Ok(Some(shift)) => {
                        if let Some(file) = &mut measured[position] {
                            file.row.starts_at =
                                Some(check::composite_start_by_sound(video_start, shift));
                        }
                    }
                    Ok(None) => {}
                    Err(Unfinished) => incomplete = true,
                }
            }
        }
        let heard = measured[position].as_ref().and_then(Measured::heard);
        let lacks = heard.is_some_and(|composite| {
            microphones.iter().any(|microphone| {
                measured[*microphone]
                    .as_ref()
                    .and_then(Measured::heard)
                    .is_some_and(|reference| check::lacks_sound_of(&composite, &reference))
            })
        });
        if let (true, Some(file)) = (lacks, &mut measured[position]) {
            file.add(FileProblem::SoundMissing);
        }
    }

    Examined {
        files: measured
            .into_iter()
            .map(|file| file.map(|file| file.row))
            .collect(),
        incomplete,
    }
}

/// The files that exist of the segments whose examination did not end, each with the
/// problem that it was not checked. Used when no further attempt follows.
async fn unchecked(
    backend: &Backend,
    recording: RecordingId,
    segments: &[SegmentRow],
    files: &mut [Option<FileRow>],
) {
    for (segment, file) in segments.iter().zip(files) {
        if file.is_some() {
            continue;
        }
        let place = if segment.is_track() {
            Place::track(recording, segment.id)
        } else {
            Place::video(recording, segment.id)
        };
        let root = backend.settings.directory.clone();
        let bytes = blocking(move || files::size(&root, &place)).await;
        if let Ok(Some(bytes)) = bytes {
            *file = Some(FileRow {
                bytes,
                media_ms: 0,
                starts_at: None,
                size: None,
                problems: vec![FileProblem::NotChecked],
                silent: false,
                recovered: segment.state == SegmentState::Failed,
            });
        }
    }
}

/// The state that follows from the files of a recording: `ready` when every segment ended
/// and has a file that passed its check, `failed` without any file, and `partly_failed`
/// otherwise.
fn final_state(segments: &[SegmentRow], files: &[Option<FileRow>]) -> RecordingState {
    let whole = segments.iter().zip(files).all(|(segment, file)| {
        segment.state == SegmentState::Ended
            && file.as_ref().is_some_and(|file| file.problems.is_empty())
    });
    if files.iter().all(Option::is_none) {
        RecordingState::Failed
    } else if whole {
        RecordingState::Ready
    } else {
        RecordingState::PartlyFailed
    }
}

/// This server process as the holder of an attempt.
fn worker(app: &AppState) -> store::Worker {
    store::Worker {
        instance: app.recordings.instance(),
        lease: app.config.limits.recordings.processing_lease,
    }
}

/// Runs one attempt to process the files of one recording. Without a recorder, and for a
/// recording that does not wait for its processing, it does nothing.
pub async fn process(app: &AppState, id: RecordingId) -> ApiResult<()> {
    let Some(backend) = app.recordings.backend() else {
        return Ok(());
    };
    let limits = &app.config.limits.recordings;
    let worker = worker(app);
    let now = app.now();
    let begun = transact!(app.background(), |connection| {
        store::begin_processing(connection, now, id, worker).await
    })?;
    let Some(store::Begun {
        recording,
        segments,
        attempt,
    }) = begun
    else {
        return Ok(());
    };
    // A recording whose attempts were all used, by a server that stopped during each, is
    // finished with what is on disk. No program runs for it again.
    let (last_attempt, exhausted) = (
        attempt >= limits.processing_attempts,
        attempt > limits.processing_attempts,
    );
    // This process holds the attempt, so an unfinished video in the directory was left by
    // an attempt that is over.
    {
        let (root, directory) = (
            backend.settings.directory.clone(),
            Place::recording(recording.id),
        );
        let cleared = blocking(move || {
            for name in files::unfinished(&root, &directory)? {
                files::remove(&root, &directory.with(&name))?;
            }
            Ok(())
        })
        .await;
        if let Err(error) = cleared {
            tracing::debug!(%error, "an unfinished video of an earlier attempt stays");
        }
    }
    let Examined {
        mut files,
        incomplete,
    } = if exhausted {
        Examined {
            files: vec![None; segments.len()],
            incomplete: true,
        }
    } else {
        // While the files are examined, this process states that it still holds the
        // attempt. An attempt that another process took is given up.
        let examining = examine(backend, &recording, &segments);
        tokio::pin!(examining);
        let mut heartbeat = tokio::time::interval(limits.processing_heartbeat.unsigned_abs());
        heartbeat.tick().await;
        loop {
            tokio::select! {
                examined = &mut examining => break examined,
                _ = heartbeat.tick() => {
                    let now = app.now();
                    let renewed = transact!(app.background(), |connection| {
                        store::renew_processing(connection, now, id, worker.instance).await
                    });
                    if matches!(renewed, Ok(false)) {
                        return Ok(());
                    }
                }
            }
        }
    };
    if incomplete && !last_attempt {
        let again = app.now() + limits.processing_retry;
        return transact!(app.background(), |connection| {
            store::retry_processing(connection, id, worker.instance, again).await
        });
    }
    if incomplete {
        unchecked(backend, id, &segments, &mut files).await;
    }

    // The pieces of a composite are deleted when its video passed the check (ADR 0602).
    for (segment, file) in segments.iter().zip(&files) {
        let passed = file.as_ref().is_some_and(|file| file.problems.is_empty());
        if !segment.is_track() && passed {
            let (root, pieces) = (
                backend.settings.directory.clone(),
                Place::pieces(id, segment.id),
            );
            if let Err(error) = blocking(move || files::remove(&root, &pieces)).await {
                tracing::warn!(%error, "the pieces of a composite could not be deleted");
            }
        }
    }

    let state = final_state(&segments, &files);
    let timeline = files
        .iter()
        .filter_map(|file| file.as_ref()?.starts_at)
        .min();
    let transcribes = backend.settings.transcriber.is_some();
    let now = app.now();
    let stored = transact!(app.background(), |connection| {
        let of_id = Of::Id(id);
        let Some(mut recording) = store::recording(connection, of_id, true).await? else {
            return Ok(false);
        };
        if !store::holds_processing(connection, id, worker.instance).await? {
            return Ok(false);
        }
        for (segment, file) in segments.iter().zip(&files) {
            store::store_file(connection, segment.id, file.as_ref()).await?;
        }
        store::processed(connection, now, &mut recording, state, timeline).await?;
        if transcribes {
            transcripts::request(connection, now, id).await?;
        }
        store::announce(connection, now, &recording, false).await?;
        Ok(true)
    })?;
    if stored {
        app.recordings.process_now();
        app.recordings.transcribe_now();
    }
    Ok(())
}

/// Deletes the directories that wait in the queue of removals (ADR 0608). A directory that
/// cannot be deleted stays in the queue and is tried again after a wait. Returns the number
/// of directories that it deleted.
pub async fn sweep(app: &AppState) -> ApiResult<usize> {
    let Some(backend) = app.recordings.backend() else {
        return Ok(0);
    };
    let now = app.now();
    let waiting = read!(app.background(), |connection| {
        // A backup that runs still copies the files (ADR 0018).
        if crate::backup::held(connection, now).await? {
            return Ok(Vec::new());
        }
        store::removals(connection, now, REMOVALS_AT_ONCE).await
    })?;
    let mut deleted = 0;
    for (removal_id, path) in waiting {
        let Some(place) = Place::from_stored(&path) else {
            return Err(ApiError::internal(
                &"a stored removal names no path below the directory of the recordings",
            ));
        };
        let root = backend.settings.directory.clone();
        match blocking(move || files::remove(&root, &place)).await {
            Ok(()) => {
                transact!(app.background(), |connection| {
                    store::removal_done(connection, removal_id).await
                })?;
                deleted += 1;
            }
            Err(error) => {
                tracing::warn!(%error, "a directory of a removed recording could not be deleted");
                let again = now + app.config.limits.recordings.removal_retry;
                transact!(app.background(), |connection| {
                    store::retry_removal(connection, removal_id, again).await
                })?;
            }
        }
    }
    Ok(deleted)
}

/// Deletes the entries below `rescued/` that are the job of no segment and were last
/// changed longer ago than the age of the limits (ADR 0606): the audio that the start
/// script of the recorder saved for a recording that was removed before, or for a job
/// whose start the record never stored. One call examines at most [`RESCUED_AT_ONCE`]
/// entries in the order of their names, and the next call continues after them. Nothing is
/// deleted while a backup holds deletions (ADR 0018). Returns the number of entries that
/// it deleted.
pub async fn sweep_rescued(app: &AppState) -> ApiResult<usize> {
    let Some(backend) = app.recordings.backend() else {
        return Ok(0);
    };
    let now = app.now();
    let (root, after) = (
        backend.settings.directory.clone(),
        app.recordings.rescued_after(),
    );
    let listed =
        blocking(move || files::rescued_entries(&root, after.as_deref(), RESCUED_AT_ONCE)).await;
    let entries = match listed {
        Ok(entries) => entries,
        Err(error) => {
            tracing::warn!(%error, "the saved files of lost jobs cannot be listed");
            return Ok(0);
        }
    };
    let more_follow = entries.len() == RESCUED_AT_ONCE;
    let last = entries.last().map(|(name, _)| name.clone());
    app.recordings
        .continue_rescued_after(last.filter(|_| more_follow));
    let old_before = (now - app.config.limits.recordings.rescued_age).unix_timestamp();
    let old: Vec<String> = entries
        .into_iter()
        .filter(|(_, changed)| *changed <= old_before)
        .map(|(name, _)| name)
        .collect();
    if old.is_empty() {
        return Ok(0);
    }
    let of_no_segment = read!(app.background(), |connection| {
        // A backup that runs still copies the files (ADR 0018).
        if crate::backup::held(connection, now).await? {
            return Ok(Vec::new());
        }
        store::jobs_without_segment(connection, &old).await
    })?;
    let mut deleted = 0;
    for name in of_no_segment {
        let Some(place) = Place::rescued(&name) else {
            continue;
        };
        let root = backend.settings.directory.clone();
        match blocking(move || files::remove(&root, &place)).await {
            Ok(()) => deleted += 1,
            Err(error) => {
                tracing::warn!(%error, "saved files of a job of no segment could not be deleted");
            }
        }
    }
    Ok(deleted)
}

/// Deletes the directories that wait, and runs one attempt for every recording whose files
/// wait for their processing, one after the other. Returns the number of recordings. Tests
/// run the processing with this, so that the order of things is fixed.
pub async fn run_once(app: &AppState) -> ApiResult<usize> {
    sweep(app).await?;
    let (now, worker) = (app.now(), worker(app));
    let ids = read!(app.background(), |connection| {
        store::processing(connection, now, worker).await
    })?;
    for id in &ids {
        process(app, *id).await?;
    }
    Ok(ids.len())
}

/// Runs [`run_once`] at each interval, and at once when a recording leaves the active
/// states or is removed, until the task is dropped. One recording is processed at a time:
/// the check of a long video uses a CPU core for minutes. A failure is logged and the next
/// interval tries again.
pub async fn run(app: AppState) {
    let interval = app.config.limits.recordings.process_interval.unsigned_abs();
    let mut ticks = tokio::time::interval(interval);
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = ticks.tick() => {}
            () = app.recordings.asked_to_process() => {}
        }
        if let Err(error) = run_once(&app).await {
            tracing::warn!(code = ?error.code(), "the processing of recordings failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use junto_protocol::ids::RecordingSegmentId;

    use super::*;

    fn segment(state: SegmentState) -> SegmentRow {
        SegmentRow {
            id: RecordingSegmentId::generate(),
            source: SegmentSource::Microphone,
            account_id: None,
            track_id: None,
            state,
            failure: None,
            job_id: None,
            requested_path: String::new(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
            job_started_at: None,
            stop_delivered_at: None,
            stop_owed: false,
            started_at: None,
            ended_at: None,
            duration_ms: None,
            file: None,
        }
    }

    fn file(problems: &[FileProblem]) -> Option<FileRow> {
        Some(FileRow {
            bytes: 1,
            media_ms: 1000,
            starts_at: None,
            size: None,
            problems: problems.to_vec(),
            silent: false,
            recovered: false,
        })
    }

    #[test]
    fn the_final_state_follows_from_the_segments_and_their_files() {
        let (ended, failed) = (segment(SegmentState::Ended), segment(SegmentState::Failed));
        let state = |segments: &[&SegmentRow], files: &[Option<FileRow>]| {
            let segments: Vec<SegmentRow> = segments.iter().map(|row| (*row).clone()).collect();
            final_state(&segments, files)
        };
        let good = file(&[]);
        assert_eq!(
            state(&[&ended, &ended], &[good.clone(), good.clone()]),
            RecordingState::Ready
        );
        assert_eq!(
            state(&[&ended, &ended], &[good.clone(), None]),
            RecordingState::PartlyFailed
        );
        assert_eq!(
            state(
                &[&ended, &ended],
                &[good.clone(), file(&[FileProblem::TimestampHole])]
            ),
            RecordingState::PartlyFailed
        );
        assert_eq!(
            state(&[&ended, &failed], &[good.clone(), good.clone()]),
            RecordingState::PartlyFailed
        );
        assert_eq!(
            state(&[&ended, &failed], &[good, None]),
            RecordingState::PartlyFailed
        );
        assert_eq!(
            state(&[&ended, &failed], &[None, None]),
            RecordingState::Failed
        );
        assert_eq!(state(&[], &[]), RecordingState::Failed);
    }
}
