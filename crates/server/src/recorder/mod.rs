//! The recorder as the server uses it (ADR 0601, ADR 0604): it starts one job for the
//! composite and one for each audio track, stops a job, names the main share of a composite,
//! and reports the file of a job. The files are written by the recorder; no media passes
//! through the server.

pub mod fake;
pub mod livekit;
pub mod samples;

use std::fmt;

use futures_util::future::BoxFuture;
use time::{Duration, OffsetDateTime};

/// The length of one piece of the composite, in seconds. Every finished piece survives a
/// loss of the recorder (ADR 0602).
pub const COMPOSITE_PIECE_SECONDS: u32 = 2;

/// The names of the files of a composite inside its directory.
pub const COMPOSITE_PIECE_PREFIX: &str = "part";
pub const COMPOSITE_PLAYLIST: &str = "playlist.m3u8";
pub const COMPOSITE_LIVE_PLAYLIST: &str = "live.m3u8";

/// The request for the composite of a room: the video that the layout page composes, written
/// as pieces with a playlist into `directory`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartComposite {
    pub room: String,
    pub layout_url: String,
    /// The track of the share that the layout page shows as the main picture. Absent: the
    /// page chooses by its own order.
    pub main_share: Option<String>,
    /// The directory of the pieces, as the recorder sees it.
    pub directory: String,
}

/// The request for the file of one audio track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartTrack {
    pub room: String,
    pub track_id: String,
    /// The path of the file without its extension, as the recorder sees it.
    pub path: String,
}

/// The value of the layout of a composite that names `main_share`.
pub fn layout(main_share: Option<&str>) -> String {
    main_share.map_or_else(String::new, |track_id| format!("main:{track_id}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    /// The recorder reports the job as starting, active or ending. This is also what it
    /// reports without end for a job of a lost recorder (ADR 0601).
    Running,
    /// The job ended and its file is reported as complete.
    Complete,
    /// The job ended with an error.
    Failed,
}

/// What the recorder reports for the file of a job.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileResult {
    pub file_name: Option<String>,
    pub started_at: Option<OffsetDateTime>,
    pub ended_at: Option<OffsetDateTime>,
    pub duration: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub status: JobStatus,
    pub file: FileResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecorderError {
    /// No recorder took the request.
    Unavailable(String),
    /// The request has no answer within its time limit. A stop without an answer was not
    /// delivered (ADR 0601).
    NoAnswer(String),
    /// The recorder, or the media server for it, refused the request.
    Refused(String),
}

impl fmt::Display for RecorderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(formatter, "no recorder is available: {reason}"),
            Self::NoAnswer(reason) => write!(formatter, "the recorder did not answer: {reason}"),
            Self::Refused(reason) => write!(formatter, "the recorder refused a request: {reason}"),
        }
    }
}

impl std::error::Error for RecorderError {}

pub type RecorderResult<T> = Result<T, RecorderError>;

/// What the server needs of a recorder. The product uses [`livekit::Egress`]; tests use
/// [`fake::FakeRecorder`]. Each start returns the identifier of the job, which is also the
/// identity of the participant of the recorder in the room.
pub trait Recorder: Send + Sync + 'static {
    fn start_composite<'a>(
        &'a self,
        request: &'a StartComposite,
    ) -> BoxFuture<'a, RecorderResult<String>>;

    fn start_track<'a>(&'a self, request: &'a StartTrack) -> BoxFuture<'a, RecorderResult<String>>;

    /// Ends a job so that its file is complete. A job that the recorder does not know is no
    /// error.
    fn stop<'a>(&'a self, job_id: &'a str) -> BoxFuture<'a, RecorderResult<()>>;

    /// Names the main share of a running composite.
    fn set_main_share<'a>(
        &'a self,
        job_id: &'a str,
        main_share: Option<&'a str>,
    ) -> BoxFuture<'a, RecorderResult<()>>;

    /// What the recorder reports about one job. `None` for a job that it does not know.
    fn job<'a>(&'a self, job_id: &'a str) -> BoxFuture<'a, RecorderResult<Option<Job>>>;
}
