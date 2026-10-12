//! The controller of recordings (ADR 0601, ADR 0604).
//!
//! The recorder reports by notifications that can be lost, and its list of jobs reports the
//! jobs of a lost recorder as running without end. The server therefore compares each
//! recording with the participants of its room again and again: every job of the recorder is
//! a participant of the room. One pass over a recording reads the room, changes the record in
//! one transaction, sends the requests that follow to the recorder outside of any
//! transaction, and stores their answers in a second transaction.

use std::collections::{HashMap, HashSet};

use futures_util::StreamExt;
use futures_util::future::join_all;
use junto_protocol::ids::{AccountId, RecordingId, RecordingSegmentId};
use junto_protocol::recording::{
    RecordingFailure, RecordingState, RecordingStopReason, SegmentFailure, SegmentSource,
    SegmentState,
};
use sqlx::PgConnection;
use time::OffsetDateTime;
use tokio::task::JoinSet;

use crate::error::ApiResult;
use crate::media::{MediaServer, Participant, ParticipantKind, TrackKind, TrackSource};
use crate::recorder::{Job, JobStatus, RecorderError, RecorderResult, StartComposite, StartTrack};
use crate::recordings::store::{self, Of, RecordingRow, SegmentRow};
use crate::recordings::{Backend, ToldLayout};
use crate::state::AppState;
use crate::storage;
use crate::{read, transact};

/// The reports of jobs that one pass asks the recorder for at one time.
const CONCURRENT_REPORTS: usize = 8;

/// What one pass saw of the room and of the recorder.
struct Observed {
    /// The time at which the participants were read. The age of a job counts at this time,
    /// not at the later time at which the reports are there.
    at: OffsetDateTime,
    participants: Vec<Participant>,
    /// What the recorder reports for the jobs that the pass asked about. `None` for a job
    /// that the recorder does not know.
    reports: HashMap<String, Option<Job>>,
    /// The jobs that the pass asked about and that the recorder gave no report for.
    unreported: HashSet<String>,
    /// The participants of the recorder in the room that are no job of the record.
    unknown_jobs: Vec<String>,
}

impl Observed {
    fn recorder_participants(&self) -> HashSet<&str> {
        recorder_participants(&self.participants)
    }
}

fn recorder_participants(participants: &[Participant]) -> HashSet<&str> {
    participants
        .iter()
        .filter(|participant| {
            participant.kind == ParticipantKind::Recorder && participant.connected
        })
        .map(|participant| participant.identity.as_str())
        .collect()
}

enum Start {
    Composite(StartComposite),
    Track(StartTrack),
}

/// The requests to the recorder that follow from the record after one pass changed it.
#[derive(Default)]
struct Plan {
    starts: Vec<(RecordingSegmentId, Start)>,
    /// The jobs to stop, each with its segment when the record waits for the answer.
    stops: Vec<(Option<RecordingSegmentId>, String)>,
    /// The job of the composite that runs, and whether its participant is in the room.
    composite: Option<(String, bool)>,
    main_share: Option<String>,
    /// The recording left the active states with files to process.
    ended: bool,
    /// The recording ends, and the stop of a job that was declared lost is owed. The
    /// recorder is asked about the job after the stop of this plan, in a pass that follows
    /// at once.
    asks_again: bool,
}

#[derive(Default)]
struct Answers {
    started: Vec<(RecordingSegmentId, RecorderResult<String>)>,
    stopped: Vec<(RecordingSegmentId, RecorderResult<()>)>,
}

/// Asks the recorder about `jobs`, a bounded number at one time, until `deadline` when there
/// is one. Returns the reports, and the jobs for which the recorder gave none.
async fn reports_of(
    backend: &Backend,
    jobs: Vec<String>,
    deadline: Option<tokio::time::Instant>,
) -> (HashMap<String, Option<Job>>, HashSet<String>) {
    let recorder = backend.recorder.clone();
    let mut answers = futures_util::stream::iter(jobs)
        .map(|job_id| {
            let recorder = recorder.clone();
            async move {
                let report = recorder.job(&job_id).await;
                (job_id, report)
            }
        })
        .buffer_unordered(CONCURRENT_REPORTS);
    let (mut reports, mut unreported) = (HashMap::new(), HashSet::new());
    loop {
        let answer = match deadline {
            Some(deadline) => match tokio::time::timeout_at(deadline, answers.next()).await {
                Ok(answer) => answer,
                Err(_) => break,
            },
            None => answers.next().await,
        };
        match answer {
            Some((job_id, Ok(report))) => {
                reports.insert(job_id, report);
            }
            Some((job_id, Err(error))) => {
                tracing::debug!(%error, "the recorder did not report a job");
                unreported.insert(job_id);
            }
            None => break,
        }
    }
    (reports, unreported)
}

/// Reads the participants of the room, and asks the recorder about the jobs whose state the
/// participants do not settle.
///
/// The jobs that record are asked about when they have no participant or their stop was
/// delivered. These reports have one time limit together: a report that is not there by
/// then is left out, and the pass decides about that job from the participants alone.
///
/// The jobs that were declared lost are asked about when a stop of the recording is asked
/// for and the record has closed every segment: then no job waits for a request of this
/// pass, and the reports have no time limit together. The recording does not end before the
/// recorder was asked about each of them.
///
/// `None` when the participants cannot be read; the pass then decides nothing that depends
/// on them.
async fn observe(
    backend: &Backend,
    media: &dyn MediaServer,
    now: OffsetDateTime,
    report_time_limit: time::Duration,
    recording: &RecordingRow,
    segments: &[SegmentRow],
) -> Option<Observed> {
    let participants = match media.list_participants(&recording.room).await {
        Ok(participants) => participants,
        Err(error) => {
            tracing::debug!(%error, "the room of a recording could not be read");
            return None;
        }
    };
    let present = recorder_participants(&participants);
    let jobs_of = |wanted: &dyn Fn(&SegmentRow, &str) -> bool| -> Vec<String> {
        segments
            .iter()
            .filter_map(|segment| {
                let job_id = segment.job_id.as_deref()?;
                wanted(segment, job_id).then(|| job_id.to_owned())
            })
            .collect()
    };
    let recording_jobs = jobs_of(&|segment, job_id| {
        segment.state == SegmentState::Recording
            && (!present.contains(job_id) || segment.stop_delivered_at.is_some())
    });
    let deadline = tokio::time::Instant::now() + report_time_limit.unsigned_abs();
    let (mut reports, mut unreported) = reports_of(backend, recording_jobs, Some(deadline)).await;

    let every_segment_is_closed = segments
        .iter()
        .all(|segment| matches!(segment.state, SegmentState::Ended | SegmentState::Failed));
    if recording.stop.is_some() && every_segment_is_closed {
        let lost_jobs =
            jobs_of(&|segment, _| segment.failure == Some(SegmentFailure::RecorderLost));
        let (of_lost, without) = reports_of(backend, lost_jobs, None).await;
        reports.extend(of_lost);
        unreported.extend(without);
    }
    let known: HashSet<&str> = segments
        .iter()
        .filter_map(|segment| segment.job_id.as_deref())
        .collect();
    let unknown_jobs = present
        .iter()
        .filter(|job_id| !known.contains(**job_id))
        .map(|job_id| (*job_id).to_owned())
        .collect();
    Some(Observed {
        at: now,
        participants,
        reports,
        unreported,
        unknown_jobs,
    })
}

/// True while the server still asks for a job of the subject that `of` selects: the last
/// requests for it did not fail in a row as often as the limit states.
fn may_ask(segments: &[SegmentRow], of: impl Fn(&SegmentRow) -> bool, attempts: u32) -> bool {
    let failed_in_a_row = segments
        .iter()
        .rev()
        .filter(|segment| of(segment))
        .take_while(|segment| {
            matches!(
                segment.failure,
                Some(
                    SegmentFailure::RecorderUnavailable
                        | SegmentFailure::StartRefused
                        | SegmentFailure::StartInterrupted
                )
            )
        })
        .count();
    failed_in_a_row < usize::try_from(attempts).unwrap_or(usize::MAX)
}

fn new_segment(
    now: OffsetDateTime,
    recording: &RecordingRow,
    source: SegmentSource,
    account_id: Option<AccountId>,
    track_id: Option<&str>,
) -> SegmentRow {
    let id = RecordingSegmentId::generate();
    SegmentRow {
        id,
        source,
        account_id,
        track_id: track_id.map(str::to_owned),
        state: SegmentState::Starting,
        failure: None,
        job_id: None,
        requested_path: format!("{}/{id}", recording.id),
        requested_at: now,
        job_started_at: None,
        stop_delivered_at: None,
        stop_owed: false,
        started_at: None,
        ended_at: None,
        duration_ms: None,
        file: None,
    }
}

/// The members of the call by the identity that each may use in the room, and the track of
/// the share that started last among the shares that the media server lists (ADR 0602).
async fn people(
    connection: &mut PgConnection,
    recording: &RecordingRow,
    participants: &[Participant],
) -> ApiResult<(HashMap<String, AccountId>, Option<String>)> {
    let members: Vec<(String, AccountId, Option<OffsetDateTime>)> = sqlx::query_as(
        "select identity, account_id, share_started_at from call_members
         where call_id = $1
         order by share_started_at desc nulls last, account_id",
    )
    .bind(recording.call_id)
    .fetch_all(connection)
    .await?;
    let screen_track = |identity: &str| {
        participants
            .iter()
            .filter(|participant| participant.identity == identity && participant.connected)
            .flat_map(|participant| &participant.tracks)
            .find(|track| {
                track.kind == TrackKind::Video
                    && track.source == TrackSource::ScreenShare
                    && !track.id.is_empty()
            })
            .map(|track| track.id.clone())
    };
    let main_share = members
        .iter()
        .filter(|(_, _, share_started_at)| share_started_at.is_some())
        .find_map(|(identity, _, _)| screen_track(identity));
    let by_identity = members
        .into_iter()
        .map(|(identity, account_id, _)| (identity, account_id))
        .collect();
    Ok((by_identity, main_share))
}

/// What one pass knows when it changes the record.
#[derive(Clone, Copy)]
struct Sight<'a> {
    now: OffsetDateTime,
    observed: Option<&'a Observed>,
    /// The free disk space of the recordings, when it could be read.
    free: Option<u64>,
}

/// Brings the record of one recording up to date with what the pass observed, in one
/// transaction, and returns the requests that follow for the recorder.
async fn decide(
    connection: &mut PgConnection,
    app: &AppState,
    backend: &Backend,
    id: RecordingId,
    sight: &Sight<'_>,
) -> ApiResult<Plan> {
    let Sight {
        now,
        observed,
        free,
    } = *sight;
    let limits = &app.config.limits.recordings;
    let mut plan = Plan::default();
    let Some(mut recording) = store::recording(&mut *connection, Of::Id(id), true).await? else {
        return Ok(plan);
    };
    if !recording.state.is_active() {
        return Ok(plan);
    }
    let mut segments = store::segments(&mut *connection, id).await?;
    let mut changed = false;
    let mut failed = false;
    // A file result replaced the declaration of a lost job.
    let mut replaced = false;

    let call_runs: bool = sqlx::query_scalar("select exists (select 1 from calls where id = $1)")
        .bind(recording.call_id)
        .fetch_one(&mut *connection)
        .await?;
    let reason = if !call_runs {
        Some(RecordingStopReason::CallEnded)
    } else if free.is_some_and(|free| storage::at_reserve(free, limits.reserve_bytes)) {
        Some(RecordingStopReason::StorageReserve)
    } else {
        None
    };
    if let Some(reason) = reason {
        changed |= store::request_stop(&mut *connection, now, &mut recording, reason, None).await?;
    }

    for segment in &mut segments {
        if segment.state == SegmentState::Starting {
            let interrupted = Some(SegmentFailure::StartInterrupted);
            store::close_segment(&mut *connection, segment, interrupted, None).await?;
            (changed, failed) = (true, true);
        }
    }

    if let Some(observed) = observed {
        let present = observed.recorder_participants();
        for segment in &mut segments {
            let Some(job_id) = segment.job_id.clone() else {
                continue;
            };
            let is_present = present.contains(job_id.as_str());
            let is_closed = matches!(segment.state, SegmentState::Ended | SegmentState::Failed);
            if is_closed && is_present && !segment.stop_owed {
                // The record has closed the segment and its job is still in the room: a job
                // that was declared lost while its recorder lived, and whose stop had no
                // answer. It gets the stop until it has left.
                plan.stops.push((None, job_id.clone()));
            }
            let job = observed.reports.get(&job_id).and_then(Option::as_ref);
            let file = job.map(|job| &job.file);
            let recorded = file.is_some_and(|file| file.started_at.is_some());
            let status = job.map(|job| job.status);
            match (segment.state, status) {
                (SegmentState::Recording, Some(JobStatus::Complete)) if recorded => {
                    store::close_segment(&mut *connection, segment, None, file).await?;
                    changed = true;
                }
                (SegmentState::Recording, Some(JobStatus::Complete | JobStatus::Failed)) => {
                    let error = Some(SegmentFailure::RecorderError);
                    store::close_segment(&mut *connection, segment, error, file).await?;
                    (changed, failed) = (true, true);
                }
                (SegmentState::Recording, Some(JobStatus::Running) | None) => {
                    let old = segment
                        .job_started_at
                        .is_some_and(|at| observed.at - at > limits.lost_after);
                    let ending = segment
                        .stop_delivered_at
                        .is_some_and(|at| observed.at - at <= limits.ending_grace);
                    if old && !ending && !is_present {
                        let lost = Some(SegmentFailure::RecorderLost);
                        store::close_segment(&mut *connection, segment, lost, file).await?;
                        (changed, failed) = (true, true);
                        if segment.stop_delivered_at.is_none() {
                            // The stop follows this transaction. It is stored as owed, so
                            // that a pass of another server process sends it as well.
                            store::owe_stop(&mut *connection, segment).await?;
                        } else {
                            plan.stops.push((Some(segment.id), job_id));
                        }
                    }
                }
                (SegmentState::Failed, Some(JobStatus::Complete))
                    if recorded && segment.failure == Some(SegmentFailure::RecorderLost) =>
                {
                    store::close_segment(&mut *connection, segment, None, file).await?;
                    (changed, replaced) = (true, true);
                }
                _ => {}
            }
        }
        for job_id in &observed.unknown_jobs {
            plan.stops.push((None, job_id.clone()));
        }
    }
    // A stop that is owed is sent by every pass until it has had an end: the pass that
    // declared the job lost, or a later one when that pass did not store an answer.
    for segment in &segments {
        if let (true, Some(job_id)) = (segment.stop_owed, &segment.job_id) {
            plan.stops.push((Some(segment.id), job_id.clone()));
        }
    }

    if recording.stop.is_none()
        && let Some(observed) = observed
    {
        let (members, main_share) =
            people(&mut *connection, &recording, &observed.participants).await?;
        plan.main_share = main_share;
        let attempts = limits.start_attempts;
        let mut wanted: Vec<SegmentRow> = Vec::new();
        let is_composite = |segment: &SegmentRow| segment.source == SegmentSource::Composite;
        let composite_runs = segments
            .iter()
            .any(|segment| is_composite(segment) && segment.state == SegmentState::Recording);
        if !composite_runs && may_ask(&segments, is_composite, attempts) {
            let composite = SegmentSource::Composite;
            wanted.push(new_segment(now, &recording, composite, None, None));
        }
        for participant in &observed.participants {
            let Some(account_id) = members.get(&participant.identity) else {
                continue;
            };
            if participant.kind != ParticipantKind::Standard || !participant.connected {
                continue;
            }
            for track in &participant.tracks {
                let source = match (track.kind, track.source) {
                    (TrackKind::Audio, TrackSource::Microphone) => SegmentSource::Microphone,
                    (TrackKind::Audio, TrackSource::ScreenShareAudio) => {
                        SegmentSource::ScreenShareAudio
                    }
                    _ => continue,
                };
                if !store::fits(&track.id) {
                    continue;
                }
                let of_track =
                    |segment: &SegmentRow| segment.track_id.as_deref() == Some(&track.id);
                let recorded = segments
                    .iter()
                    .chain(&wanted)
                    .any(|segment| of_track(segment) && segment.failure.is_none());
                if !recorded && may_ask(&segments, of_track, attempts) {
                    let (account_id, track_id) = (Some(*account_id), Some(track.id.as_str()));
                    wanted.push(new_segment(now, &recording, source, account_id, track_id));
                }
            }
        }
        let most = usize::try_from(limits.most_segments).unwrap_or(usize::MAX);
        if segments.len() + wanted.len() > most {
            let limit = RecordingStopReason::SegmentLimit;
            changed |=
                store::request_stop(&mut *connection, now, &mut recording, limit, None).await?;
        } else {
            for segment in wanted {
                let segment = store::add_segment(&mut *connection, now, id, segment).await?;
                let room = recording.room.clone();
                let path = format!(
                    "{}/{}",
                    backend.settings.recorder_directory, segment.requested_path
                );
                let start = match &segment.track_id {
                    Some(track_id) => Start::Track(StartTrack {
                        room,
                        track_id: track_id.clone(),
                        path,
                    }),
                    None => Start::Composite(StartComposite {
                        room,
                        layout_url: backend.settings.layout_url.clone(),
                        main_share: plan.main_share.clone(),
                        directory: path,
                    }),
                };
                plan.starts.push((segment.id, start));
                segments.push(segment);
                changed = true;
            }
        }
    }

    // A recording of which nothing records, and for which the server asks for no further
    // job, would show an indicator while nothing is saved. It is stopped.
    let runs = segments.iter().any(|segment| {
        matches!(
            segment.state,
            SegmentState::Starting | SegmentState::Recording
        )
    });
    if recording.stop.is_none() && observed.is_some() && !runs {
        let lost = RecordingStopReason::RecorderLost;
        changed |= store::request_stop(&mut *connection, now, &mut recording, lost, None).await?;
    }

    if recording.stop.is_some() {
        let present = observed.map(Observed::recorder_participants);
        for segment in &segments {
            let Some(job_id) = &segment.job_id else {
                continue;
            };
            // A job that still has its participant after the time in which a stopped job
            // reports its end did not end: the stop is sent again.
            let ignored = segment.stop_delivered_at.is_some_and(|at| {
                now - at > limits.ending_grace
                    && present
                        .as_ref()
                        .is_some_and(|present| present.contains(job_id.as_str()))
            });
            let undelivered = segment.stop_delivered_at.is_none() || ignored;
            if segment.state == SegmentState::Recording && undelivered {
                plan.stops.push((Some(segment.id), job_id.clone()));
            }
        }
    } else if let Some(observed) = observed {
        let present = observed.recorder_participants();
        plan.composite = segments
            .iter()
            .filter(|segment| {
                segment.source == SegmentSource::Composite
                    && segment.state == SegmentState::Recording
            })
            .find_map(|segment| segment.job_id.clone())
            .map(|job_id| {
                let is_present = present.contains(job_id.as_str());
                (job_id, is_present)
            });
    }

    // The recording leaves the active states when a stop is asked for, the record has closed
    // every segment, the room holds no participant of the recorder, so that no job of it,
    // known or not, records, and the recorder was asked in this pass about every job that
    // was declared lost. A job that was declared lost while its recorder lived ends by the
    // stop that follows the declaration: while that stop is owed the recording stays, the
    // recorder is asked after the stop, and a job whose stop the recorder took has the time
    // of a stopped job to report its end. In that time only a report of an end, or of a job
    // that the recorder does not know, settles it.
    let every_job_ended = segments
        .iter()
        .all(|segment| matches!(segment.state, SegmentState::Ended | SegmentState::Failed));
    let reconciled = observed.is_some_and(|observed| {
        let settled = |segment: &SegmentRow| {
            let Some(job_id) = &segment.job_id else {
                return true;
            };
            let report = observed.reports.get(job_id);
            let asked = report.is_some() || observed.unreported.contains(job_id);
            let can_still_end = match report {
                Some(Some(job)) => job.status == JobStatus::Running,
                Some(None) => false,
                None => true,
            };
            let ending = can_still_end
                && segment
                    .stop_delivered_at
                    .is_some_and(|at| observed.at - at <= limits.ending_grace);
            asked && !ending && !segment.stop_owed
        };
        observed.recorder_participants().is_empty()
            && segments
                .iter()
                .filter(|segment| segment.failure == Some(SegmentFailure::RecorderLost))
                .all(settled)
    });
    if let Some(stop) = recording.stop
        && every_job_ended
        && reconciled
    {
        // A job ran for a segment with a job identifier, and can have run for a segment whose
        // start was interrupted. Only without both is nothing on disk.
        let recorded = segments.iter().any(|segment| {
            segment.job_id.is_some() || segment.failure == Some(SegmentFailure::StartInterrupted)
        });
        let failure = match (recorded, stop.reason) {
            (true, _) => None,
            (false, RecordingStopReason::RecorderLost) => {
                Some(RecordingFailure::RecorderUnavailable)
            }
            (false, _) => Some(RecordingFailure::NothingRecorded),
        };
        store::finish(&mut *connection, now, &mut recording, failure).await?;
        plan.ended = failure.is_none();
        changed = true;
    }
    plan.asks_again = recording.stop.is_some() && segments.iter().any(|segment| segment.stop_owed);
    if replaced && segments.iter().all(|segment| segment.failure.is_none()) {
        store::failure_replaced(&mut *connection, now, &recording).await?;
    }
    if failed {
        store::note_failure(&mut *connection, now, &recording).await?;
    }
    if changed {
        store::announce(connection, now, &recording, false).await?;
    }
    Ok(plan)
}

/// Sends the requests of a plan to the recorder: the starts, the stops and the main share,
/// each kind at the same time as the others. A request that fails does not stop the others.
async fn act(app: &AppState, backend: &Backend, id: RecordingId, plan: Plan) -> Answers {
    let recorder = &backend.recorder;
    let starting = join_all(plan.starts.iter().map(|(segment_id, start)| async move {
        let answer = match start {
            Start::Composite(request) => recorder.start_composite(request).await,
            Start::Track(request) => recorder.start_track(request).await,
        };
        (*segment_id, answer)
    }));
    let stopping = join_all(
        plan.stops
            .iter()
            .map(|(segment_id, job_id)| async move { (*segment_id, recorder.stop(job_id).await) }),
    );
    let naming = async {
        let Some((job_id, present)) = &plan.composite else {
            return;
        };
        let told = app
            .recordings
            .told_layout(id)
            .filter(|told| told.job_id == *job_id);
        let returned = told.as_ref().is_none_or(|told| !told.present);
        let differs = told
            .as_ref()
            .is_none_or(|told| told.main_share != plan.main_share);
        if *present && (returned || differs) {
            let main_share = plan.main_share.as_deref();
            let told = match recorder.set_main_share(job_id, main_share).await {
                Ok(()) => Some(ToldLayout {
                    job_id: job_id.clone(),
                    main_share: plan.main_share.clone(),
                    present: true,
                }),
                Err(error) => {
                    tracing::debug!(%error, "the main share of a composite could not be named");
                    None
                }
            };
            app.recordings.tell_layout(id, told);
        } else if let Some(told) = told
            && !*present
        {
            let absent = ToldLayout {
                present: false,
                ..told
            };
            app.recordings.tell_layout(id, Some(absent));
        }
    };
    let (started, stopped, ()) = tokio::join!(starting, stopping, naming);
    let started: Vec<(RecordingSegmentId, RecorderResult<String>)> = started
        .into_iter()
        .map(|(segment_id, answer)| {
            let answer = answer.and_then(|job_id| {
                if store::fits(&job_id) {
                    Ok(job_id)
                } else {
                    Err(RecorderError::Refused(
                        "the identifier of the job cannot be stored".to_owned(),
                    ))
                }
            });
            (segment_id, answer)
        })
        .collect();

    for ((segment_id, start), (_, answer)) in plan.starts.iter().zip(&started) {
        match (start, answer) {
            (Start::Composite(request), Ok(job_id)) => {
                let told = ToldLayout {
                    job_id: job_id.clone(),
                    main_share: request.main_share.clone(),
                    present: true,
                };
                app.recordings.tell_layout(id, Some(told));
            }
            (_, Err(error)) => {
                tracing::warn!(segment = %segment_id, %error, "a job of a recording did not start");
            }
            (Start::Track(_), Ok(_)) => {}
        }
    }
    Answers {
        started,
        stopped: stopped
            .into_iter()
            .filter_map(|(segment_id, answer)| Some((segment_id?, answer)))
            .collect(),
    }
}

/// Stores the answers of the recorder, in one transaction. A recording that is starting runs
/// from the first job that started. When no job of its first requests started, its stop is
/// asked for: the next pass fails it with the reason that no recorder is available, when the
/// room holds no job (ADR 0601). Returns true when the next pass has something to do at
/// once.
async fn record(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    id: RecordingId,
    answers: &Answers,
) -> ApiResult<bool> {
    if answers.started.is_empty() && answers.stopped.is_empty() {
        return Ok(false);
    }
    let Some(mut recording) = store::recording(&mut *connection, Of::Id(id), true).await? else {
        return Ok(false);
    };
    for (segment_id, answer) in &answers.started {
        match answer {
            Ok(job_id) => store::job_started(&mut *connection, now, *segment_id, job_id).await?,
            Err(error) => {
                let failure = match error {
                    RecorderError::Refused(_) => SegmentFailure::StartRefused,
                    RecorderError::Unavailable(_) | RecorderError::NoAnswer(_) => {
                        SegmentFailure::RecorderUnavailable
                    }
                };
                store::start_failed(&mut *connection, *segment_id, failure).await?;
            }
        }
    }
    for (segment_id, answer) in &answers.stopped {
        let delivered = matches!(answer, Ok(()) | Err(RecorderError::Refused(_)));
        store::stop_answered(&mut *connection, now, *segment_id, delivered).await?;
    }
    if answers.started.is_empty() {
        return Ok(false);
    }
    let any_started = answers.started.iter().any(|(_, answer)| answer.is_ok());
    let mut gave_up = false;
    if recording.state == RecordingState::Starting {
        if any_started {
            store::began(&mut *connection, now, &mut recording).await?;
        } else {
            let lost = RecordingStopReason::RecorderLost;
            gave_up =
                store::request_stop(&mut *connection, now, &mut recording, lost, None).await?;
        }
    }
    if answers.started.iter().any(|(_, answer)| answer.is_err()) {
        store::note_failure(&mut *connection, now, &recording).await?;
    }
    store::announce(connection, now, &recording, false).await?;
    Ok(gave_up)
}

/// Runs one pass over one recording. Without a recorder or a media server, while another
/// pass over the recording runs, and for a recording that is no longer active, it does
/// nothing. A pass that sends the owed stop of a job of an ending recording is followed by
/// a second one, which asks the recorder about that job after its stop.
pub async fn tend(app: &AppState, id: RecordingId) -> ApiResult<()> {
    let (Some(backend), Some(media)) = (app.recordings.backend(), app.calls.media()) else {
        return Ok(());
    };
    let Some(_pass) = app.recordings.pass(id) else {
        return Ok(());
    };
    if pass(app, backend, media.as_ref(), id).await? {
        pass(app, backend, media.as_ref(), id).await?;
    }
    Ok(())
}

/// One pass over one recording. True when the recorder is to be asked again at once.
async fn pass(
    app: &AppState,
    backend: &Backend,
    media: &dyn MediaServer,
    id: RecordingId,
) -> ApiResult<bool> {
    let stored = read!(app.background(), |connection| {
        let Some(recording) = store::recording(&mut *connection, Of::Id(id), false).await? else {
            return Ok(None);
        };
        if !recording.state.is_active() {
            return Ok(None);
        }
        let segments = store::segments(connection, id).await?;
        Ok(Some((recording, segments)))
    })?;
    let Some((recording, segments)) = stored else {
        app.recordings.tell_layout(id, None);
        return Ok(false);
    };
    let report_time_limit = app.config.limits.recordings.report_time_limit;
    let observed = observe(
        backend,
        media,
        app.now(),
        report_time_limit,
        &recording,
        &segments,
    )
    .await;
    let sight = Sight {
        now: app.now(),
        observed: observed.as_ref(),
        free: app.recordings.free_bytes(),
    };
    let plan = transact!(app.background(), |connection| {
        decide(connection, app, backend, id, &sight).await
    })?;
    if plan.ended {
        app.recordings.process_now();
    }
    let asks_again = plan.asks_again;
    let answers = act(app, backend, id, plan).await;
    let now = app.now();
    let gave_up = transact!(app.background(), |connection| {
        record(connection, now, id, &answers).await
    })?;
    if gave_up {
        app.recordings.poke();
    }
    Ok(asks_again)
}

async fn active(app: &AppState) -> ApiResult<Vec<RecordingId>> {
    let ids = read!(app.background(), |connection| {
        store::active(connection).await
    })?;
    app.recordings.keep_layouts_of(&ids);
    Ok(ids)
}

/// Runs one pass over every recording that is starting or runs, one after the other. Returns
/// the number of recordings. Tests run the passes with this, so that the order of things is
/// fixed.
pub async fn tend_all(app: &AppState) -> ApiResult<usize> {
    let ids = active(app).await?;
    for id in &ids {
        tend(app, *id).await?;
    }
    Ok(ids.len())
}

/// Runs a pass over every active recording at each interval, and at once when a request or a
/// notification of the media server asks for it, until the task is dropped. The passes of
/// different recordings run at the same time, so that a recorder that answers a start after
/// 22 seconds delays one recording only. A pass that fails is logged and the next one tries
/// again.
pub async fn run(app: AppState) {
    let limits = &app.config.limits.recordings;
    let mut ticks = tokio::time::interval(limits.check_interval.unsigned_abs());
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut passes = JoinSet::new();
    loop {
        tokio::select! {
            _ = ticks.tick() => {}
            () = app.recordings.poked() => {}
        }
        while passes.try_join_next().is_some() {}
        app.recordings.storage_low(limits.reserve_bytes);
        match active(&app).await {
            Ok(ids) => {
                for id in ids {
                    let app = app.clone();
                    passes.spawn(async move {
                        if let Err(error) = tend(&app, id).await {
                            tracing::warn!(code = ?error.code(), "a pass over a recording failed");
                        }
                    });
                }
            }
            Err(error) => {
                tracing::warn!(code = ?error.code(), "the active recordings could not be read");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(track_id: Option<&str>, failure: Option<SegmentFailure>) -> SegmentRow {
        let recording = RecordingRow {
            id: RecordingId::generate(),
            workspace_id: junto_protocol::ids::WorkspaceId::generate(),
            channel_id: junto_protocol::ids::ChannelId::generate(),
            call_id: junto_protocol::ids::CallId::generate(),
            room: String::new(),
            state: RecordingState::Recording,
            started_by: AccountId::generate(),
            language: "auto".to_owned(),
            requested_at: OffsetDateTime::UNIX_EPOCH,
            recording_since: None,
            stop: None,
            ended_at: None,
            failure: None,
            processed_at: None,
            timeline_started_at: None,
        };
        let source = match track_id {
            Some(_) => SegmentSource::Microphone,
            None => SegmentSource::Composite,
        };
        let account_id = track_id.map(|_| AccountId::generate());
        let now = OffsetDateTime::UNIX_EPOCH;
        let mut segment = new_segment(now, &recording, source, account_id, track_id);
        assert_eq!(
            segment.requested_path,
            format!("{}/{}", recording.id, segment.id)
        );
        segment.failure = failure;
        segment
    }

    #[test]
    fn a_job_is_asked_for_until_its_requests_fail_three_times_in_a_row() {
        let unavailable = Some(SegmentFailure::RecorderUnavailable);
        let refused = Some(SegmentFailure::StartRefused);
        let lost = Some(SegmentFailure::RecorderLost);
        let composite = |segment: &SegmentRow| segment.source == SegmentSource::Composite;
        let of_track = |segment: &SegmentRow| segment.track_id.as_deref() == Some("TR_a");

        let mut segments = vec![segment(None, unavailable), segment(None, refused)];
        assert!(may_ask(&segments, composite, 3));
        assert!(may_ask(&segments, of_track, 3));
        segments.push(segment(Some("TR_a"), unavailable));
        segments.push(segment(None, Some(SegmentFailure::StartInterrupted)));
        assert!(!may_ask(&segments, composite, 3));
        assert!(may_ask(&segments, of_track, 3));

        segments.push(segment(None, lost));
        assert!(may_ask(&segments, composite, 3));
        segments.push(segment(None, None));
        segments.push(segment(None, unavailable));
        assert!(may_ask(&segments, composite, 2));
        assert!(!may_ask(&segments, composite, 1));
    }
}
