//! The client of LiveKit Egress (ADR 0604): five calls of the Egress service of the LiveKit
//! server API over Twirp with JSON bodies.

use std::sync::Arc;

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use time::{Duration, OffsetDateTime};

use crate::config::RecordingLimits;
use crate::media::livekit::Named;
use crate::media::token::ApiGrant;
use crate::media::twirp::{Answer, Failure, Twirp};
use crate::recorder::{
    COMPOSITE_LIVE_PLAYLIST, COMPOSITE_PIECE_PREFIX, COMPOSITE_PIECE_SECONDS, COMPOSITE_PLAYLIST,
    FileResult, Job, JobStatus, Recorder, RecorderError, RecorderResult, StartComposite,
    StartTrack, layout,
};

const SERVICE: &str = "livekit.Egress";

/// The picture of the composite. ADR 0602 states the measured cost of this preset.
const PRESET: &str = "H264_1080P_30";

pub struct Egress {
    api: Arc<Twirp>,
    start_timeout: std::time::Duration,
    request_timeout: std::time::Duration,
}

/// A 64-bit number, which the API writes as text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Number(i64);

impl<'de> Deserialize<'de> for Number {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Text(String),
            Number(i64),
        }
        Ok(Self(match Written::deserialize(deserializer)? {
            Written::Text(text) => text.parse().unwrap_or(0),
            Written::Number(number) => number,
        }))
    }
}

impl Number {
    /// A time in nanoseconds since 1970. Zero means that the API states no time.
    fn time(self) -> Option<OffsetDateTime> {
        (self.0 > 0)
            .then(|| OffsetDateTime::from_unix_timestamp_nanos(i128::from(self.0)).ok())
            .flatten()
    }

    fn duration(self) -> Option<Duration> {
        (self.0 > 0).then(|| Duration::nanoseconds(self.0))
    }
}

#[derive(Debug, Default, Deserialize)]
struct EgressList {
    #[serde(default)]
    items: Vec<EgressInfo>,
}

#[derive(Debug, Default, Deserialize)]
struct EgressInfo {
    #[serde(default, alias = "egressId")]
    egress_id: String,
    #[serde(default)]
    status: Named,
    #[serde(default, alias = "fileResults")]
    file_results: Vec<FileInfo>,
    #[serde(default, alias = "segmentResults")]
    segment_results: Vec<PiecesInfo>,
}

#[derive(Debug, Default, Deserialize)]
struct FileInfo {
    #[serde(default)]
    filename: String,
    #[serde(default, alias = "startedAt")]
    started_at: Number,
    #[serde(default, alias = "endedAt")]
    ended_at: Number,
    #[serde(default)]
    duration: Number,
}

#[derive(Debug, Default, Deserialize)]
struct PiecesInfo {
    #[serde(default, alias = "playlistName")]
    playlist_name: String,
    #[serde(default, alias = "startedAt")]
    started_at: Number,
    #[serde(default, alias = "endedAt")]
    ended_at: Number,
    #[serde(default)]
    duration: Number,
}

fn result(name: String, started_at: Number, ended_at: Number, duration: Number) -> FileResult {
    FileResult {
        file_name: Some(name).filter(|name| !name.is_empty()),
        started_at: started_at.time(),
        ended_at: ended_at.time(),
        duration: duration.duration(),
    }
}

fn job(info: EgressInfo) -> Job {
    let status =
        if info.status.is("EGRESS_COMPLETE", 3) || info.status.is("EGRESS_LIMIT_REACHED", 6) {
            JobStatus::Complete
        } else if info.status.is("EGRESS_FAILED", 4) || info.status.is("EGRESS_ABORTED", 5) {
            JobStatus::Failed
        } else {
            JobStatus::Running
        };
    let file = info
        .file_results
        .into_iter()
        .next()
        .map(|file| result(file.filename, file.started_at, file.ended_at, file.duration))
        .or_else(|| {
            info.segment_results.into_iter().next().map(|pieces| {
                result(
                    pieces.playlist_name,
                    pieces.started_at,
                    pieces.ended_at,
                    pieces.duration,
                )
            })
        })
        .unwrap_or_default();
    Job {
        id: info.egress_id,
        status,
        file,
    }
}

fn composite_body(request: &StartComposite) -> Value {
    json!({
        "room_name": request.room,
        "layout": layout(request.main_share.as_deref()),
        "custom_base_url": request.layout_url,
        "preset": PRESET,
        "segment_outputs": [{
            "filename_prefix": format!("{}/{COMPOSITE_PIECE_PREFIX}", request.directory),
            "playlist_name": COMPOSITE_PLAYLIST,
            "live_playlist_name": COMPOSITE_LIVE_PLAYLIST,
            "segment_duration": COMPOSITE_PIECE_SECONDS,
        }],
    })
}

fn track_body(request: &StartTrack) -> Value {
    json!({
        "room_name": request.room,
        "track_id": request.track_id,
        "file": { "filepath": request.path },
    })
}

/// A request that reached its time limit has no answer: the media server states this with
/// the status 408 for a request that the recorder did not take in time (ADR 0601). A status
/// of 500 and above, as the 503 without a recorder, means that no recorder took the request.
fn error(method: &str, failure: Failure) -> RecorderError {
    let reason = format!("{method}: {failure}");
    match failure {
        Failure::NoAnswer(_) | Failure::Malformed(_) => RecorderError::NoAnswer(reason),
        Failure::Status { status, ref code } if status == 408 || code == "deadline_exceeded" => {
            RecorderError::NoAnswer(reason)
        }
        Failure::Status { status, ref code } if status >= 500 || code == "unavailable" => {
            RecorderError::Unavailable(reason)
        }
        Failure::Status { .. } | Failure::Unsigned => RecorderError::Refused(reason),
    }
}

impl Egress {
    pub fn new(api: Arc<Twirp>, limits: &RecordingLimits) -> Self {
        Self {
            api,
            start_timeout: limits.start_timeout.unsigned_abs(),
            request_timeout: limits.request_timeout.unsigned_abs(),
        }
    }

    async fn call(
        &self,
        method: &str,
        body: Value,
        time_limit: std::time::Duration,
    ) -> RecorderResult<Answer> {
        self.api
            .call(SERVICE, method, ApiGrant::Record, body, Some(time_limit))
            .await
            .map_err(|failure| error(method, failure))
    }

    async fn start(&self, method: &str, body: Value) -> RecorderResult<String> {
        let unreadable = || RecorderError::NoAnswer(format!("{method}: the answer names no job"));
        match self.call(method, body, self.start_timeout).await? {
            Answer::NotFound => Err(RecorderError::Refused(format!("{method}: not found"))),
            Answer::Body(body) => {
                let info: EgressInfo = serde_json::from_value(body).map_err(|_| unreadable())?;
                Some(info.egress_id)
                    .filter(|id| !id.is_empty())
                    .ok_or_else(unreadable)
            }
        }
    }
}

impl Recorder for Egress {
    fn start_composite<'a>(
        &'a self,
        request: &'a StartComposite,
    ) -> BoxFuture<'a, RecorderResult<String>> {
        self.start("StartRoomCompositeEgress", composite_body(request))
            .boxed()
    }

    fn start_track<'a>(&'a self, request: &'a StartTrack) -> BoxFuture<'a, RecorderResult<String>> {
        self.start("StartTrackEgress", track_body(request)).boxed()
    }

    fn stop<'a>(&'a self, job_id: &'a str) -> BoxFuture<'a, RecorderResult<()>> {
        async move {
            let body = json!({ "egress_id": job_id });
            self.call("StopEgress", body, self.request_timeout).await?;
            Ok(())
        }
        .boxed()
    }

    fn set_main_share<'a>(
        &'a self,
        job_id: &'a str,
        main_share: Option<&'a str>,
    ) -> BoxFuture<'a, RecorderResult<()>> {
        async move {
            let body = json!({ "egress_id": job_id, "layout": layout(main_share) });
            self.call("UpdateLayout", body, self.request_timeout)
                .await?;
            Ok(())
        }
        .boxed()
    }

    fn job<'a>(&'a self, job_id: &'a str) -> BoxFuture<'a, RecorderResult<Option<Job>>> {
        async move {
            let body = json!({ "egress_id": job_id });
            let answer = self.call("ListEgress", body, self.request_timeout).await?;
            let Answer::Body(body) = answer else {
                return Ok(None);
            };
            let list: EgressList = serde_json::from_value(body)
                .map_err(|error| RecorderError::NoAnswer(format!("ListEgress: {error}")))?;
            Ok(list.items.into_iter().map(job).find(|job| job.id == job_id))
        }
        .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requests_have_the_bodies_that_step_0_tested() {
        let composite = StartComposite {
            room: "jr-a-b".to_owned(),
            layout_url: "http://server.example.test/layout/".to_owned(),
            main_share: Some("TR_share".to_owned()),
            directory: "/out/recording/segment".to_owned(),
        };
        assert_eq!(
            composite_body(&composite),
            json!({
                "room_name": "jr-a-b",
                "layout": "main:TR_share",
                "custom_base_url": "http://server.example.test/layout/",
                "preset": "H264_1080P_30",
                "segment_outputs": [{
                    "filename_prefix": "/out/recording/segment/part",
                    "playlist_name": "playlist.m3u8",
                    "live_playlist_name": "live.m3u8",
                    "segment_duration": 2,
                }],
            })
        );
        let unnamed = StartComposite {
            main_share: None,
            ..composite
        };
        assert_eq!(composite_body(&unnamed)["layout"], "");

        let track = StartTrack {
            room: "jr-a-b".to_owned(),
            track_id: "TR_microphone".to_owned(),
            path: "/out/recording/segment".to_owned(),
        };
        assert_eq!(
            track_body(&track),
            json!({
                "room_name": "jr-a-b",
                "track_id": "TR_microphone",
                "file": { "filepath": "/out/recording/segment" },
            })
        );
    }

    #[test]
    fn a_job_is_read_from_the_report_of_the_recorder() {
        let list: EgressList = serde_json::from_value(json!({ "items": [
            {
                "egress_id": "EG_track", "status": "EGRESS_COMPLETE",
                "started_at": "1791528562892419611", "ended_at": "1791528630005065480",
                "file_results": [{
                    "filename": "/out/r/s.ogg", "started_at": "1791528564061439741",
                    "ended_at": "1791528629981774743", "duration": "65920335002", "size": "1059773"
                }],
                "segment_results": [], "error": ""
            },
            {
                "egress_id": "EG_composite", "status": "EGRESS_COMPLETE",
                "file_results": [],
                "segment_results": [{
                    "playlist_name": "/out/r/c/playlist.m3u8", "duration": "63542599481",
                    "segment_count": "33", "started_at": "1791528566344385890",
                    "ended_at": "1791528629886985371"
                }]
            },
            {
                "egress_id": "EG_lost", "status": "EGRESS_ACTIVE", "ended_at": "0",
                "file_results": [{
                    "filename": "/out/r/l.ogg", "started_at": "1791528520600424022",
                    "ended_at": "0", "duration": "0"
                }]
            },
            { "egressId": "EG_failed", "status": 4, "fileResults": [{ "startedAt": 5 }] },
            { "egress_id": "EG_aborted", "status": "EGRESS_ABORTED" },
            { "egress_id": "EG_limit", "status": "EGRESS_LIMIT_REACHED" },
            { "egress_id": "EG_ending", "status": "EGRESS_ENDING" },
            { "egress_id": "EG_later", "status": "EGRESS_LATER_STATUS" }
        ] }))
        .unwrap();
        let jobs: Vec<Job> = list.items.into_iter().map(job).collect();

        let track = &jobs[0];
        assert_eq!(track.id, "EG_track");
        assert_eq!(track.status, JobStatus::Complete);
        assert_eq!(track.file.file_name.as_deref(), Some("/out/r/s.ogg"));
        let started = track.file.started_at.unwrap();
        assert_eq!(started.unix_timestamp(), 1_791_528_564);
        assert_eq!(started.nanosecond(), 61_439_741);
        let length = track.file.ended_at.unwrap() - started;
        assert_eq!(length.whole_milliseconds(), 65_920);
        assert_eq!(track.file.duration.unwrap().whole_milliseconds(), 65_920);

        let composite = &jobs[1];
        assert_eq!(composite.status, JobStatus::Complete);
        assert_eq!(
            composite.file.file_name.as_deref(),
            Some("/out/r/c/playlist.m3u8")
        );
        assert_eq!(composite.file.duration.unwrap().whole_seconds(), 63);

        let lost = &jobs[2];
        assert_eq!(lost.status, JobStatus::Running);
        assert!(lost.file.started_at.is_some());
        assert_eq!(lost.file.ended_at, None);
        assert_eq!(lost.file.duration, None);

        assert_eq!(jobs[3].id, "EG_failed");
        assert_eq!(jobs[3].status, JobStatus::Failed);
        assert_eq!(jobs[3].file.file_name, None);
        assert!(jobs[3].file.started_at.is_some());
        assert_eq!(jobs[4].status, JobStatus::Failed);
        assert_eq!(jobs[5].status, JobStatus::Complete);
        assert_eq!(jobs[6].status, JobStatus::Running);
        assert_eq!(jobs[7].status, JobStatus::Running);
        assert_eq!(jobs[7].file, FileResult::default());

        let empty: EgressList = serde_json::from_value(json!({})).unwrap();
        assert!(empty.items.is_empty());
    }

    #[test]
    fn an_answer_that_reached_its_time_limit_is_no_answer() {
        let status = |status: u16, code: &str| Failure::Status {
            status,
            code: code.to_owned(),
        };
        let kind = |failure: Failure| match error("StopEgress", failure) {
            RecorderError::NoAnswer(_) => "no_answer",
            RecorderError::Unavailable(_) => "unavailable",
            RecorderError::Refused(_) => "refused",
        };
        assert_eq!(kind(status(408, "deadline_exceeded")), "no_answer");
        assert_eq!(kind(Failure::NoAnswer("timed out".to_owned())), "no_answer");
        assert_eq!(kind(Failure::Malformed("not JSON".to_owned())), "no_answer");
        assert_eq!(kind(status(503, "unavailable")), "unavailable");
        assert_eq!(kind(status(500, "internal")), "unavailable");
        assert_eq!(kind(status(412, "failed_precondition")), "refused");
        assert_eq!(kind(status(401, "unauthenticated")), "refused");
        assert_eq!(kind(Failure::Unsigned), "refused");
    }
}
