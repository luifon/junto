//! A recorder in memory for tests. It behaves as step 0 measured LiveKit Egress (ADR 0601):
//! each job is a participant of the room, a job of a lost recorder is reported as running
//! without end, a stop for such a job has no answer, and a recorder that starts again
//! resumes nothing.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use time::OffsetDateTime;

use crate::clock::Clock;
use crate::media::ParticipantKind;
use crate::media::fake::FakeMedia;
use crate::recorder::{
    COMPOSITE_PLAYLIST, FileResult, Job, JobStatus, Recorder, RecorderError, RecorderResult,
    StartComposite, StartTrack,
};

/// What a job of the fake recorder was asked to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asked {
    Composite(StartComposite),
    Track(StartTrack),
}

/// One job of the fake recorder, as a test reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeJob {
    pub id: String,
    pub asked: Asked,
    pub status: JobStatus,
    /// False for a job whose recorder was lost.
    pub alive: bool,
    pub started_at: OffsetDateTime,
    pub ended_at: Option<OffsetDateTime>,
    /// The main shares that the job was told after its start, in order.
    pub main_shares: Vec<Option<String>>,
    /// The end of the job is not reported yet.
    held: bool,
}

impl FakeJob {
    pub fn room(&self) -> &str {
        match &self.asked {
            Asked::Composite(request) => &request.room,
            Asked::Track(request) => &request.room,
        }
    }

    pub fn track_id(&self) -> Option<&str> {
        match &self.asked {
            Asked::Composite(_) => None,
            Asked::Track(request) => Some(&request.track_id),
        }
    }

    pub fn is_composite(&self) -> bool {
        matches!(self.asked, Asked::Composite(_))
    }

    fn file(&self) -> FileResult {
        let file_name = match &self.asked {
            Asked::Composite(request) => format!("{}/{COMPOSITE_PLAYLIST}", request.directory),
            Asked::Track(request) => format!("{}.ogg", request.path),
        };
        FileResult {
            file_name: Some(file_name),
            started_at: Some(self.started_at),
            ended_at: self.ended_at,
            duration: self.ended_at.map(|ended_at| ended_at - self.started_at),
        }
    }

    fn report(&self) -> Job {
        let reported = if self.held || !self.alive {
            Self {
                status: JobStatus::Running,
                ended_at: None,
                ..self.clone()
            }
        } else {
            self.clone()
        };
        Job {
            id: self.id.clone(),
            status: reported.status,
            file: reported.file(),
        }
    }
}

struct State {
    jobs: Vec<FakeJob>,
    next: u64,
    /// A recorder process exists and takes requests.
    running: bool,
    /// The path between the media server and the recorder is up.
    link: bool,
    refuses_starts: bool,
    holds_ends: bool,
    /// The jobs whose participant does not enter the room.
    without_participant: bool,
    stops: Vec<String>,
    reports: u64,
    fails_reports: bool,
    report_delay: Option<std::time::Duration>,
    slow_reports: Vec<(String, std::time::Duration)>,
    long_identifiers: bool,
}

pub struct FakeRecorder {
    clock: Arc<dyn Clock>,
    media: Arc<FakeMedia>,
    state: Mutex<State>,
}

impl FakeRecorder {
    pub fn new(clock: Arc<dyn Clock>, media: Arc<FakeMedia>) -> Self {
        Self {
            clock,
            media,
            state: Mutex::new(State {
                jobs: Vec::new(),
                next: 0,
                running: true,
                link: true,
                refuses_starts: false,
                holds_ends: false,
                without_participant: false,
                stops: Vec::new(),
                reports: 0,
                fails_reports: false,
                report_delay: None,
                slow_reports: Vec::new(),
                long_identifiers: false,
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Ends the jobs of tracks that no participant of the room publishes any more, as the
    /// recorder ends the file of a track that ended.
    fn settle(&self, state: &mut State) {
        let now = self.clock.now();
        let holds = state.holds_ends;
        for job in &mut state.jobs {
            let Asked::Track(request) = &job.asked else {
                continue;
            };
            if !job.alive || job.status != JobStatus::Running {
                continue;
            }
            let published = self
                .media
                .participants(&request.room)
                .iter()
                .any(|participant| {
                    participant
                        .tracks
                        .iter()
                        .any(|track| track.id == request.track_id)
                });
            if !published {
                job.status = JobStatus::Complete;
                job.ended_at = Some(now);
                job.held = holds;
                self.media.leave(&request.room, &job.id);
            }
        }
    }

    fn start(&self, asked: Asked) -> RecorderResult<String> {
        let mut state = self.state();
        self.settle(&mut state);
        if !state.running || !state.link {
            return Err(RecorderError::Unavailable(
                "no response from servers".to_owned(),
            ));
        }
        if state.refuses_starts {
            return Err(RecorderError::Refused(
                "the fake refuses to start".to_owned(),
            ));
        }
        state.next += 1;
        let width = if state.long_identifiers { 140 } else { 8 };
        let job = FakeJob {
            id: format!("EG_{:0width$}", state.next),
            asked,
            status: JobStatus::Running,
            alive: true,
            started_at: self.clock.now(),
            ended_at: None,
            main_shares: Vec::new(),
            held: false,
        };
        if !state.without_participant {
            self.media
                .enter(job.room(), &job.id, ParticipantKind::Recorder, None);
        }
        let id = job.id.clone();
        state.jobs.push(job);
        Ok(id)
    }

    /// What the recorder does by itself: it ends the file of every track that has ended.
    /// The fake does this whenever it is asked something; a test calls this for the time in
    /// between.
    pub fn work(&self) {
        let mut state = self.state();
        self.settle(&mut state);
    }

    /// Every job of the fake, in the order of its start.
    pub fn jobs(&self) -> Vec<FakeJob> {
        let mut state = self.state();
        self.settle(&mut state);
        state.jobs.clone()
    }

    /// The jobs that record now.
    pub fn recording(&self) -> Vec<FakeJob> {
        self.jobs()
            .into_iter()
            .filter(|job| job.alive && job.status == JobStatus::Running)
            .collect()
    }

    pub fn job_of_track(&self, track_id: &str) -> Vec<FakeJob> {
        self.jobs()
            .into_iter()
            .filter(|job| job.track_id() == Some(track_id))
            .collect()
    }

    /// The identifiers of the jobs that the server asked to stop, in order.
    pub fn stops(&self) -> Vec<String> {
        self.state().stops.clone()
    }

    /// The number of times the server asked for the report of a job.
    pub fn reports(&self) -> u64 {
        self.state().reports
    }

    /// The recorder process dies. Its jobs are lost: they are reported as running without
    /// end, and their participants stay in the room until [`FakeRecorder::participants_gone`].
    pub fn kill(&self) {
        let mut state = self.state();
        self.settle(&mut state);
        state.running = false;
        for job in &mut state.jobs {
            if job.status == JobStatus::Running {
                job.alive = false;
            }
        }
    }

    /// The media server no longer lists the participants of the lost jobs.
    pub fn participants_gone(&self) {
        let state = self.state();
        for job in state.jobs.iter().filter(|job| !job.alive) {
            self.media.leave(job.room(), &job.id);
        }
    }

    /// A recorder process starts. It resumes nothing.
    pub fn restart(&self) {
        self.state().running = true;
    }

    /// Takes the recorder away without a lost job: no request to start is taken.
    pub fn set_running(&self, running: bool) {
        self.state().running = running;
    }

    /// Opens or closes the path between the media server and the recorder. While it is
    /// closed, a stop has no answer and the jobs keep recording.
    pub fn set_link(&self, up: bool) {
        self.state().link = up;
    }

    pub fn refuse_starts(&self, refuses: bool) {
        self.state().refuses_starts = refuses;
    }

    /// While this is on, a job that ends is still reported as running, as after a loss of
    /// the path between the hosts. [`FakeRecorder::report_ends`] reports the ends.
    pub fn hold_ends(&self, holds: bool) {
        self.state().holds_ends = holds;
    }

    pub fn report_ends(&self) {
        let mut state = self.state();
        state.holds_ends = false;
        for job in &mut state.jobs {
            job.held = false;
        }
    }

    /// While this is on, the recorder does not answer a request for the report of a job.
    pub fn fail_reports(&self, fails: bool) {
        self.state().fails_reports = fails;
    }

    /// Makes every report of a job take `delay` from now on.
    pub fn delay_reports(&self, delay: Option<std::time::Duration>) {
        self.state().report_delay = delay;
    }

    /// Makes every report of one job take `delay` from now on.
    pub fn delay_reports_of(&self, job_id: &str, delay: std::time::Duration) {
        self.state().slow_reports.push((job_id.to_owned(), delay));
    }

    /// While this is on, a new job gets an identifier of more than 128 characters.
    pub fn long_identifiers(&self, long: bool) {
        self.state().long_identifiers = long;
    }

    /// While this is on, the participant of a new job does not enter the room.
    pub fn start_without_participant(&self, without: bool) {
        self.state().without_participant = without;
    }

    /// The recorder ends a job with an error.
    pub fn fail(&self, job_id: &str) {
        let mut state = self.state();
        let now = self.clock.now();
        if let Some(job) = state.jobs.iter_mut().find(|job| job.id == job_id) {
            job.status = JobStatus::Failed;
            job.ended_at = Some(now);
            self.media.leave(job.room(), &job.id);
        }
    }

    /// A job that the server did not start: what a request leaves whose answer was lost.
    pub fn start_unasked(&self, request: StartTrack) -> String {
        self.start(Asked::Track(request)).unwrap_or_default()
    }
}

impl Recorder for FakeRecorder {
    fn start_composite<'a>(
        &'a self,
        request: &'a StartComposite,
    ) -> BoxFuture<'a, RecorderResult<String>> {
        let started = self.start(Asked::Composite(request.clone()));
        async move { started }.boxed()
    }

    fn start_track<'a>(&'a self, request: &'a StartTrack) -> BoxFuture<'a, RecorderResult<String>> {
        let started = self.start(Asked::Track(request.clone()));
        async move { started }.boxed()
    }

    fn stop<'a>(&'a self, job_id: &'a str) -> BoxFuture<'a, RecorderResult<()>> {
        let stopped = {
            let mut state = self.state();
            self.settle(&mut state);
            state.stops.push(job_id.to_owned());
            let (link, holds) = (state.link, state.holds_ends);
            let now = self.clock.now();
            match state.jobs.iter_mut().find(|job| job.id == job_id) {
                None => Ok(()),
                Some(job) if !link || !job.alive => {
                    Err(RecorderError::NoAnswer("request timed out".to_owned()))
                }
                Some(job) if job.status != JobStatus::Running => Err(RecorderError::Refused(
                    "the job has ended and cannot be stopped".to_owned(),
                )),
                Some(job) => {
                    job.status = JobStatus::Complete;
                    job.ended_at = Some(now);
                    job.held = holds;
                    self.media.leave(job.room(), &job.id);
                    Ok(())
                }
            }
        };
        async move { stopped }.boxed()
    }

    fn set_main_share<'a>(
        &'a self,
        job_id: &'a str,
        main_share: Option<&'a str>,
    ) -> BoxFuture<'a, RecorderResult<()>> {
        let set = {
            let mut state = self.state();
            let link = state.link;
            match state.jobs.iter_mut().find(|job| job.id == job_id) {
                Some(job) if link && job.alive && job.status == JobStatus::Running => {
                    job.main_shares.push(main_share.map(str::to_owned));
                    Ok(())
                }
                Some(_) => Err(RecorderError::NoAnswer("request timed out".to_owned())),
                None => Err(RecorderError::Refused("the job does not exist".to_owned())),
            }
        };
        async move { set }.boxed()
    }

    fn job<'a>(&'a self, job_id: &'a str) -> BoxFuture<'a, RecorderResult<Option<Job>>> {
        let (report, delay) = {
            let mut state = self.state();
            self.settle(&mut state);
            state.reports += 1;
            let report = if state.fails_reports {
                Err(RecorderError::NoAnswer("request timed out".to_owned()))
            } else {
                Ok(state
                    .jobs
                    .iter()
                    .find(|job| job.id == job_id)
                    .map(FakeJob::report))
            };
            let of_job = state
                .slow_reports
                .iter()
                .find(|(slow, _)| slow == job_id)
                .map(|(_, delay)| *delay);
            (report, state.report_delay.or(of_job))
        };
        async move {
            if let Some(delay) = delay {
                tokio::time::sleep(delay).await;
            }
            report
        }
        .boxed()
    }
}
