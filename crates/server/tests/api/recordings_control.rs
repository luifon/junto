//! The controller of recordings: new tracks, a lost recorder, stops without an answer, the
//! end of a call, a second server process, and the main share (ADR 0601, ADR 0602,
//! ADR 0604). The recorder is the fake that behaves as step 0 measured, and each test runs
//! the passes of the controller itself.

use std::time::{Duration as Wait, Instant};

use junto_protocol::audit::AuditEvent;
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::AccountId;
use junto_protocol::recording::{
    RecordingDetail, RecordingSegment, RecordingState, RecordingStopReason, SegmentFailure,
    SegmentSource, SegmentState,
};
use junto_server::media::{ParticipantKind, TrackKind, TrackSource};
use junto_server::recorder::StartTrack;
use junto_server::recorder::fake::Asked;
use reqwest::{Method, StatusCode};
use serde_json::json;
use time::Duration;

use crate::recordings_support::{Studio, composites, of_source};
use crate::support::TestServer;

async fn until(what: &str, mut holds: impl AsyncFnMut() -> bool) {
    let started = Instant::now();
    while !holds().await {
        assert!(
            started.elapsed() < Wait::from_secs(30),
            "after 30 seconds this did not hold: {what}"
        );
        tokio::time::sleep(Wait::from_millis(20)).await;
    }
}

fn of_person(
    detail: &RecordingDetail,
    account_id: AccountId,
    source: SegmentSource,
) -> Vec<&RecordingSegment> {
    detail
        .segments
        .iter()
        .filter(|segment| segment.account_id == Some(account_id) && segment.source == source)
        .collect()
}

fn states(detail: &RecordingDetail) -> Vec<(SegmentState, Option<SegmentFailure>)> {
    detail
        .segments
        .iter()
        .map(|segment| (segment.state, segment.failure))
        .collect()
}

const RUNNING: (SegmentState, Option<SegmentFailure>) = (SegmentState::Recording, None);
const ENDED: (SegmentState, Option<SegmentFailure>) = (SegmentState::Ended, None);
const LOST: (SegmentState, Option<SegmentFailure>) =
    (SegmentState::Failed, Some(SegmentFailure::RecorderLost));

/// ADR 0601: a new job with a new segment for each audio track that appears later. A mute
/// starts no new segment, and a picture has no file of its own.
#[tokio::test]
async fn a_track_that_appears_later_gets_its_own_segment_and_a_mute_starts_none() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ben) = (&scene.server, &scene.owner, &scene.ben);
    let started = studio.record().await;
    assert_eq!(server.recorder().jobs().len(), 3);

    let seat = &studio.ben_seat;
    server.publish(seat, TrackKind::Video, TrackSource::ScreenShare);
    server.publish(seat, TrackKind::Video, TrackSource::Camera);
    let shared = server.publish(seat, TrackKind::Audio, TrackSource::ScreenShareAudio);
    server.tend().await;
    let jobs = server.recorder().jobs();
    assert_eq!(jobs.len(), 4);
    assert_eq!(jobs[3].track_id(), Some(shared.as_str()));
    let detail = server.recording(owner, started.id).await;
    let of_ben = of_person(&detail, ben.account_id, SegmentSource::ScreenShareAudio);
    assert_eq!(of_ben.len(), 1);
    assert_eq!(of_ben[0].state, SegmentState::Recording);
    assert_eq!(detail.recording.segments, 4);

    let ann = &studio.ann_seat;
    let microphone = TrackSource::Microphone;
    server
        .fake()
        .set_muted(&ann.room, &ann.identity, microphone, true);
    server.clock.advance(Duration::seconds(30));
    server.tend().await;
    server
        .fake()
        .set_muted(&ann.room, &ann.identity, microphone, false);
    server.tend().await;
    assert_eq!(server.recorder().jobs().len(), 4);
    assert_eq!(server.recorder().recording().len(), 4);
    assert_eq!(server.recorder().reports(), 0);
}

/// ADR 0601: a participant who reconnects gets new tracks. The file of the old track ends
/// complete, and the server starts a new file for the new track.
#[tokio::test]
async fn a_reconnect_ends_the_file_of_the_old_track_and_starts_a_new_one() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ben) = (&scene.server, &scene.owner, &scene.ben);
    let started = studio.record().await;
    let began = server.clock.now();
    server.clock.advance(Duration::seconds(5));

    let seat = &studio.ben_seat;
    server.fake().leave(&seat.room, &seat.identity);
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    let of_ben = of_person(&detail, ben.account_id, SegmentSource::Microphone);
    assert_eq!(of_ben.len(), 1);
    assert_eq!(of_ben[0].state, SegmentState::Ended);
    assert_eq!(of_ben[0].started_at, Some(began));
    assert_eq!(of_ben[0].duration_ms, Some(5_000));
    assert_eq!(detail.recording.failed_segments, 0);

    server.clock.advance(Duration::seconds(2));
    let returned = server.connect(server.join_call(ben, scene.voice.id).await);
    assert_eq!(returned.identity, seat.identity);
    let track = server.speak(&returned);
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    let of_ben = of_person(&detail, ben.account_id, SegmentSource::Microphone);
    assert_eq!(of_ben.len(), 2);
    assert_eq!(of_ben[1].state, SegmentState::Recording);
    assert_eq!(server.recorder().job_of_track(&track).len(), 1);
    assert_eq!(server.recorder().jobs().len(), 4);
    assert_eq!(detail.recording.state, RecordingState::Recording);
}

/// ADR 0601: the status in the recorder's job list does not decide whether a job is alive.
/// A job that is older than 10 seconds and has no participant in the room is lost: its
/// segment fails with the range that was saved, and a new job starts while the call runs.
#[tokio::test]
async fn a_lost_recorder_is_found_by_the_participants_of_the_room_and_new_jobs_start() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let started = studio.record().await;
    let began = server.clock.now();
    server.clock.advance(Duration::seconds(30));

    server.recorder().kill();
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(states(&detail), [RUNNING; 3]);
    assert_eq!(server.recorder().reports(), 0);

    server.recorder().participants_gone();
    server.recorder().restart();
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(
        states(&detail),
        [LOST, LOST, LOST, RUNNING, RUNNING, RUNNING]
    );
    assert_eq!(detail.recording.state, RecordingState::Recording);
    assert_eq!(
        (detail.recording.segments, detail.recording.failed_segments),
        (6, 3)
    );
    for segment in &detail.segments[..3] {
        assert_eq!(segment.started_at, Some(began));
        assert_eq!(segment.ended_at, None);
    }
    let jobs = server.recorder().jobs();
    assert_eq!(jobs.len(), 6);
    let lost: Vec<String> = jobs[..3].iter().map(|job| job.id.clone()).collect();
    assert_eq!(server.recorder().stops(), lost);
    assert_eq!(server.recorder().recording().len(), 3);
    assert_eq!(composites(&jobs).len(), 2);
    let noted: i64 =
        sqlx::query_scalar("select count(*) from call_notes where kind = 'recording_failed'")
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(noted, 1);

    server.clock.advance(Duration::seconds(5));
    let done = studio.stop(started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(states(&done), [LOST, LOST, LOST, ENDED, ENDED, ENDED]);
    assert_eq!(done.recording.failed_segments, 3);
}

/// The participant of a new job enters the room some seconds after the request. A job
/// without a participant is lost only when it is older than 10 seconds.
#[tokio::test]
async fn a_job_without_a_participant_is_lost_only_after_ten_seconds() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    server.recorder().start_without_participant(true);
    let started = studio.record().await;

    server.tend().await;
    server.clock.advance(Duration::seconds(10));
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(states(&detail), [RUNNING; 3]);
    assert!(server.recorder().reports() >= 6);
    assert!(server.recorder().stops().is_empty());

    server.clock.advance(Duration::seconds(1));
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(
        states(&detail),
        [LOST, LOST, LOST, RUNNING, RUNNING, RUNNING]
    );
}

/// ADR 0601: a stop request that answers with a timeout was not delivered. The server
/// repeats it while the recorder is alive.
#[tokio::test]
async fn a_stop_without_an_answer_is_repeated_while_the_recorder_lives() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(30));

    server.recorder().set_link(false);
    server
        .stop_recording(ann, scene.voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.tend().await;
    assert_eq!(server.recorder().stops().len(), 3);
    server.tend().await;
    assert_eq!(server.recorder().stops().len(), 6);
    let detail = server.recording(owner, started.id).await;
    assert_eq!(detail.recording.state, RecordingState::Recording);
    assert_eq!(states(&detail), [RUNNING; 3]);
    assert_eq!(server.recorder().recording().len(), 3);

    server.recorder().set_link(true);
    server.tend().await;
    assert_eq!(server.recorder().stops().len(), 9);
    server.tend().await;
    let done = server.recording(owner, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(states(&done), [ENDED; 3]);
    assert_eq!(server.recorder().stops().len(), 9);
}

/// A stop for a job of a lost recorder has no answer either. The participants of the room
/// decide: once they are gone, the segments fail and the recording ends.
#[tokio::test]
async fn a_stop_without_an_answer_ends_with_lost_segments_when_the_recorder_is_lost() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(15));

    server.recorder().kill();
    server
        .stop_recording(ann, scene.voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(detail.recording.state, RecordingState::Recording);
    assert_eq!(states(&detail), [RUNNING; 3]);

    server.recorder().participants_gone();
    server.tend().await;
    let done = server.recording(owner, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(states(&done), [LOST; 3]);
    assert_eq!(done.recording.failure, None);
    assert_eq!(server.recorder().jobs().len(), 3);
    assert_eq!(server.count("storage_reservations").await, 1);
}

/// ADR 0601: after a loss of the private link, a job that had ended was reported as active
/// for 16 seconds. A job whose stop was delivered has 20 seconds to report its end, and the
/// end time comes from the file result.
#[tokio::test]
async fn a_stopped_job_that_reports_its_end_late_is_not_lost() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(15));

    server.recorder().hold_ends(true);
    server
        .stop_recording(ann, scene.voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    let stopped_at = server.clock.now();
    server.tend().await;
    server.tend().await;
    server.clock.advance(Duration::seconds(20));
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(detail.recording.state, RecordingState::Recording);
    assert_eq!(states(&detail), [RUNNING; 3]);
    assert_eq!(server.recorder().stops().len(), 3);

    server.recorder().report_ends();
    server.tend().await;
    let done = server.recording(owner, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(states(&done), [ENDED; 3]);
    for segment in &done.segments {
        assert_eq!(segment.ended_at, Some(stopped_at));
        assert_eq!(segment.duration_ms, Some(15_000));
    }
}

#[tokio::test]
async fn a_stopped_job_that_does_not_report_its_end_in_time_is_lost() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(15));
    server.recorder().hold_ends(true);
    server
        .stop_recording(ann, scene.voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.tend().await;
    server.clock.advance(Duration::seconds(21));
    server.tend().await;
    let done = server.recording(owner, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(states(&done), [LOST; 3]);
}

/// A job that the server declared lost while its recorder lived gets a stop, so that two
/// jobs do not record the same thing. When the recording ends, the file result of the
/// recorder for that job replaces the declaration.
#[tokio::test]
async fn a_job_that_was_declared_lost_and_reports_a_file_is_not_failed() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let started = studio.record().await;
    let began = server.clock.now();
    server.clock.advance(Duration::seconds(11));

    let composite = composites(&server.recorder().jobs())[0].id.clone();
    server.fake().leave(&studio.ann_seat.room, &composite);
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(states(&detail), [LOST, RUNNING, RUNNING, RUNNING]);
    assert_eq!(detail.segments[3].source, SegmentSource::Composite);
    assert_eq!(detail.recording.failed_segments, 1);
    assert_eq!(server.recorder().stops(), std::slice::from_ref(&composite));
    assert_eq!(server.recorder().recording().len(), 3);
    assert_eq!(composites(&server.recorder().recording()).len(), 1);

    server.clock.advance(Duration::seconds(9));
    let done = studio.stop(started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(states(&done), [ENDED; 4]);
    assert_eq!(done.recording.failed_segments, 0);
    assert_eq!(done.segments[0].started_at, Some(began));
    assert_eq!(done.segments[0].duration_ms, Some(11_000));
    assert_eq!(done.segments[3].duration_ms, Some(9_000));
}

#[tokio::test]
async fn a_job_that_the_recorder_ends_with_an_error_fails_its_segment_and_a_new_job_starts() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let started = studio.record().await;
    let began = server.clock.now();
    server.clock.advance(Duration::seconds(8));

    let seat = &studio.ann_seat;
    let track = server
        .fake()
        .track_id(&seat.room, &seat.identity, TrackSource::Microphone)
        .unwrap();
    let job = server.recorder().job_of_track(&track).remove(0);
    server.recorder().fail(&job.id);
    server.tend().await;

    let detail = server.recording(owner, started.id).await;
    let of_ann = of_person(&detail, ann.account_id, SegmentSource::Microphone);
    assert_eq!(of_ann.len(), 2);
    assert_eq!(of_ann[0].state, SegmentState::Failed);
    assert_eq!(of_ann[0].failure, Some(SegmentFailure::RecorderError));
    assert_eq!(of_ann[0].started_at, Some(began));
    assert_eq!(of_ann[0].ended_at, Some(began + Duration::seconds(8)));
    assert_eq!(of_ann[1].state, SegmentState::Recording);
    assert_eq!(server.recorder().job_of_track(&track).len(), 2);
    assert_eq!(detail.recording.failed_segments, 1);
    assert_eq!(detail.recording.state, RecordingState::Recording);
}

/// The server asks for the job of a track until its requests failed three times in a row.
#[tokio::test]
async fn the_requests_for_a_job_end_after_three_failures_in_a_row() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ben) = (&scene.server, &scene.owner, &scene.ben);
    let started = studio.record().await;

    server.recorder().refuse_starts(true);
    let seat = &studio.ben_seat;
    server.publish(seat, TrackKind::Audio, TrackSource::ScreenShareAudio);
    for _ in 0..5 {
        server.tend().await;
    }
    server.recorder().refuse_starts(false);
    server.tend().await;

    let detail = server.recording(owner, started.id).await;
    let shared = of_person(&detail, ben.account_id, SegmentSource::ScreenShareAudio);
    assert_eq!(shared.len(), 3);
    for segment in shared {
        assert_eq!(segment.state, SegmentState::Failed);
        assert_eq!(segment.failure, Some(SegmentFailure::StartRefused));
    }
    assert_eq!(detail.recording.state, RecordingState::Recording);
    assert_eq!(
        (detail.recording.segments, detail.recording.failed_segments),
        (6, 3)
    );
    assert_eq!(server.recorder().recording().len(), 3);
}

/// Review finding 2: a job that was declared lost while its recorder lived, and whose stop
/// had no answer, gets the stop again when its participant is in the room, so that two jobs
/// do not record the same source.
#[tokio::test]
async fn a_job_that_was_declared_lost_gets_its_stop_until_it_has_left_the_room() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(11));

    let room = &studio.ann_seat.room;
    let composite = composites(&server.recorder().jobs())[0].id.clone();
    server.fake().leave(room, &composite);
    server.recorder().set_link(false);
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(states(&detail)[..3], [LOST, RUNNING, RUNNING]);
    assert_eq!(server.recorder().stops(), std::slice::from_ref(&composite));
    assert_eq!(composites(&server.recorder().recording()).len(), 1);

    server.recorder().set_link(true);
    server
        .fake()
        .enter(room, &composite, ParticipantKind::Recorder, None);
    server.tend().await;
    let running = server.recorder().recording();
    assert_eq!(composites(&running).len(), 1);
    assert_ne!(composites(&running)[0].id, composite);
    assert_eq!(server.recorder().stops().len(), 2);
    assert_eq!(server.recorder().stops()[1], composite);
    server.tend().await;
    assert_eq!(server.recorder().stops().len(), 2);
}

/// Review finding 3: a stop that is asked for after a start whose answer the server did not
/// store. The job of that start is in the room; the recording does not end, and keeps its
/// reservation, until the job has its stop and has left the room.
#[tokio::test]
async fn a_recording_does_not_end_while_a_job_of_the_recorder_is_in_its_room() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let started = server.record(ann, scene.voice.id).await;

    let seat = &studio.ann_seat;
    let track = server
        .fake()
        .track_id(&seat.room, &seat.identity, TrackSource::Microphone)
        .unwrap();
    sqlx::query(
        "insert into recording_segments
             (id, recording_id, source, account_id, track_id, state, requested_path, requested_at)
         values ($1, $2, 'microphone', $3, $4, 'starting', 'interrupted', now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(started.id.as_uuid())
    .bind(ann.account_id.as_uuid())
    .bind(&track)
    .execute(&server.db)
    .await
    .unwrap();
    let unknown = server.recorder().start_unasked(StartTrack {
        room: seat.room.clone(),
        track_id: track,
        path: "/out/interrupted".to_owned(),
    });
    server
        .stop_recording(ann, scene.voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);

    server.recorder().set_link(false);
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(detail.recording.state, RecordingState::Starting);
    assert_eq!(detail.recording.ended_at, None);
    assert_eq!(
        states(&detail),
        [(SegmentState::Failed, Some(SegmentFailure::StartInterrupted))]
    );
    assert_eq!(server.recorder().recording().len(), 1);
    assert_eq!(server.count("storage_reservations").await, 1);

    server.recorder().set_link(true);
    server.tend().await;
    assert!(server.recorder().recording().is_empty());
    assert_eq!(server.recorder().stops(), [unknown.clone(), unknown]);
    server.tend().await;
    let done = server.recording(owner, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(done.recording.failure, None);
    assert_eq!(server.count("storage_reservations").await, 1);
}

/// Review finding 4: the report of the recorder is additional evidence. Without it, a job
/// that is older than 10 seconds and has no participant in the room is lost all the same.
#[tokio::test]
async fn a_job_without_a_participant_is_lost_also_when_the_recorder_reports_nothing() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(30));
    server.recorder().kill();
    server.recorder().participants_gone();
    server.recorder().restart();
    server.recorder().fail_reports(true);

    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(
        states(&detail),
        [LOST, LOST, LOST, RUNNING, RUNNING, RUNNING]
    );
    assert!(server.recorder().reports() >= 3);
    assert_eq!(server.recorder().recording().len(), 3);
}

/// Review finding 7: the reports about the jobs that record have one time limit together.
/// A recorder that does not answer does not keep a pass from deciding: the jobs are decided
/// by the participants of the room alone.
#[tokio::test]
async fn reports_that_do_not_arrive_do_not_keep_a_pass_from_deciding() {
    let server = TestServer::start_for_calls_with(|config| {
        config.limits.recordings.report_time_limit = Duration::milliseconds(200);
    })
    .await;
    let studio = Studio::on(server).await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    server.recorder().start_without_participant(true);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(11));
    server.recorder().start_without_participant(false);

    server.recorder().delay_reports(Some(Wait::from_secs(3600)));
    let pass = tokio::time::timeout(Wait::from_secs(60), server.tend());
    assert!(pass.await.is_ok(), "the pass waited for the reports");
    assert_eq!(server.recorder().reports(), 3);
    let detail = server.recording(owner, started.id).await;
    assert_eq!(
        states(&detail),
        [LOST, LOST, LOST, RUNNING, RUNNING, RUNNING]
    );
}

/// Review 2, finding 2: before a recording ends, the recorder is asked about every job that
/// was declared lost, without the time limit of the other reports. A complete file result
/// that the recorder gives slowly still replaces the declaration.
#[tokio::test]
async fn a_slow_report_about_a_job_that_was_declared_lost_is_waited_for() {
    let server = TestServer::start_for_calls_with(|config| {
        config.limits.recordings.report_time_limit = Duration::milliseconds(100);
    })
    .await;
    let studio = Studio::on(server).await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(11));
    let composite = composites(&server.recorder().jobs())[0].id.clone();
    server.fake().leave(&studio.ann_seat.room, &composite);
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(states(&detail), [LOST, RUNNING, RUNNING, RUNNING]);

    server
        .recorder()
        .delay_reports_of(&composite, Wait::from_millis(400));
    let done = studio.stop(started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(states(&done), [ENDED; 4]);
    assert_eq!(done.recording.failed_segments, 0);
}

/// Review 2, finding 3: the age of a job is taken at the moment at which the participants of
/// the room were read. A job that was 9 seconds old and absent then is not lost because the
/// reports took a further 5 seconds.
#[tokio::test]
async fn the_age_of_a_job_counts_at_the_moment_the_room_was_read() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    server.recorder().start_without_participant(true);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(9));

    let asked = server.recorder().reports();
    server
        .recorder()
        .delay_reports(Some(Wait::from_millis(300)));
    let later = async {
        until("the pass asks the recorder", async || {
            server.recorder().reports() > asked
        })
        .await;
        server.clock.advance(Duration::seconds(5));
    };
    tokio::join!(server.tend(), later);
    let detail = server.recording(owner, started.id).await;
    assert_eq!(states(&detail), [RUNNING; 3]);

    server.recorder().delay_reports(None);
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(states(&detail)[..3], [LOST; 3]);
}

/// An identifier of a job that the record cannot store: the start counts as refused. When
/// all the first starts are refused, the recording fails, but only when the jobs have had
/// their stop and have left the room; until then it keeps its reservation.
#[tokio::test]
async fn a_job_whose_identifier_cannot_be_stored_counts_as_refused() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    server.recorder().long_identifiers(true);
    let started = server.record(ann, scene.voice.id).await;
    server.tend().await;

    let refused = (SegmentState::Failed, Some(SegmentFailure::StartRefused));
    let stopping = server.recording(owner, started.id).await;
    assert_eq!(stopping.recording.state, RecordingState::Starting);
    assert_eq!(
        stopping.recording.stop.unwrap().reason,
        RecordingStopReason::RecorderLost
    );
    assert_eq!(states(&stopping), [refused; 3]);
    assert_eq!(server.recorder().recording().len(), 3);
    assert_eq!(server.count("storage_reservations").await, 1);

    server.tend().await;
    assert!(server.recorder().recording().is_empty());
    assert_eq!(server.recorder().stops().len(), 3);
    let stopping = server.recording(owner, started.id).await;
    assert_eq!(stopping.recording.state, RecordingState::Starting);

    server.tend().await;
    let failed = server.recording(owner, started.id).await;
    assert_eq!(failed.recording.state, RecordingState::Failed);
    assert_eq!(server.count("storage_reservations").await, 0);
}

/// ADR 0604: a participant of the recorder in a room that has no recording which is
/// starting or runs is not allowed there. The admission check asks the recorder to stop it.
#[tokio::test]
async fn the_admission_check_stops_a_job_of_the_recorder_in_a_room_without_a_recording() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let seat = &studio.ann_seat;
    let track = server
        .fake()
        .track_id(&seat.room, &seat.identity, TrackSource::Microphone)
        .unwrap();
    let request = StartTrack {
        room: seat.room.clone(),
        track_id: track,
        path: "/out/unknown".to_owned(),
    };
    let unknown = server.recorder().start_unasked(request.clone());
    server.check().await;
    assert_eq!(server.recorder().stops(), std::slice::from_ref(&unknown));
    assert!(server.recorder().recording().is_empty());
    assert_eq!(server.occupants(ben, scene.voice.id).await.len(), 2);

    let started = studio.record().await;
    server.check().await;
    assert_eq!(server.recorder().stops().len(), 1);
    assert_eq!(server.recorder().recording().len(), 3);
    studio.stop(started.id).await;

    server.leave_call(ann).await;
    server.leave_call(ben).await;
    server.check().await;
    let late = server.recorder().start_unasked(request);
    server.check().await;
    assert_eq!(server.recorder().stops().last(), Some(&late));
    assert!(server.recorder().recording().is_empty());
}

/// ADR 0604: a recording of which no job runs, and for which the requests to start new jobs
/// failed three times in a row, is stopped: the indicator does not stay while nothing is
/// saved.
#[tokio::test]
async fn a_recording_whose_jobs_cannot_be_started_again_is_stopped() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(11));
    server.recorder().kill();
    server.recorder().participants_gone();

    for attempt in 1..=3 {
        server.tend().await;
        let detail = server.recording(owner, started.id).await;
        assert_eq!(detail.recording.stop, None, "attempt {attempt}");
        assert_eq!(detail.recording.segments, 3 + 3 * attempt);
        assert_eq!(detail.recording.failed_segments, 3 + 3 * attempt);
    }
    server.tend().await;
    let stopping = server.recording(owner, started.id).await;
    let stop = stopping.recording.stop.unwrap();
    assert_eq!(stop.reason, RecordingStopReason::RecorderLost);
    assert_eq!(stop.by, None);
    assert_eq!(stopping.recording.state, RecordingState::Recording);
    assert_eq!(stopping.recording.segments, 12);

    server.tend().await;
    let done = server.recording(owner, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(states(&done)[..3], [LOST; 3]);
    let unavailable = (
        SegmentState::Failed,
        Some(SegmentFailure::RecorderUnavailable),
    );
    assert_eq!(states(&done)[3..], [unavailable; 9]);
    assert_eq!(server.recorder().jobs().len(), 3);
}

/// ADR 0604: the transaction that ends a call asks for the stop of its recording.
#[tokio::test]
async fn the_end_of_a_call_stops_its_recording() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let started = studio.record().await;
    server.clock.advance(Duration::seconds(12));
    let room = studio.ann_seat.room.clone();

    server.leave_call(ann).await;
    assert_eq!(
        server.recording(owner, started.id).await.recording.stop,
        None
    );
    server.leave_call(ben).await;
    let stopping = server.recording(owner, started.id).await;
    let stop = stopping.recording.stop.unwrap();
    assert_eq!(stop.reason, RecordingStopReason::CallEnded);
    assert_eq!(stop.by, None);
    assert_eq!(stopping.recording.state, RecordingState::Recording);

    server.check().await;
    assert_eq!(server.fake().identities(&room).len(), 3);
    server.tend().await;
    assert!(server.fake().identities(&room).is_empty());
    server.tend().await;
    let done = server.recording(owner, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(states(&done), [ENDED; 3]);
    assert_eq!(done.segments[0].duration_ms, Some(12_000));

    let entries = server.audit_actions(owner, scene.workspace_id).await;
    let stopped = entries
        .iter()
        .find(|entry| matches!(entry.event, AuditEvent::RecordingStopped { .. }))
        .unwrap();
    assert_eq!(stopped.actor_id, None);
    assert_eq!(
        stopped.event,
        AuditEvent::RecordingStopped {
            recording_id: started.id,
            reason: RecordingStopReason::CallEnded
        }
    );
}

/// A deleted channel ends its call at the next admission check, and with it the recording.
/// The recording of a deleted channel is not read through the API (ADR 0407).
#[tokio::test]
async fn a_deleted_channel_ends_the_call_and_stops_its_recording() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let started = studio.record().await;

    server
        .delete(owner, &format!("/api/v1/channels/{}", scene.voice.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.check().await;
    assert_eq!(server.count("calls").await, 0);
    server.tend().await;
    server.tend().await;
    assert!(server.recorder().recording().is_empty());
    let (state, reason): (String, Option<String>) =
        sqlx::query_as("select state, stop_reason from recordings where id = $1")
            .bind(started.id.as_uuid())
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("processing", Some("call_ended"))
    );
    server
        .recording_reply(owner, started.id)
        .await
        .error(ErrorCode::NotFound);
    assert!(
        server
            .recordings(owner, scene.workspace_id)
            .await
            .recordings
            .is_empty()
    );
}

/// ADR 0601: after its own restart the server compares its records with the room. A request
/// whose answer the first process did not store leaves a segment without a job and a job
/// without a segment; the second process fails the one and stops the other.
#[tokio::test]
async fn a_second_server_process_continues_a_recording() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let started = studio.record().await;

    let seat = &studio.ben_seat;
    let shared = server.publish(seat, TrackKind::Audio, TrackSource::ScreenShareAudio);
    let interrupted = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into recording_segments
             (id, recording_id, source, account_id, track_id, state, requested_path, requested_at)
         values ($1, $2, 'screen_share_audio', $3, $4, 'starting', 'interrupted', now())",
    )
    .bind(interrupted)
    .bind(started.id.as_uuid())
    .bind(ben.account_id.as_uuid())
    .bind(&shared)
    .execute(&server.db)
    .await
    .unwrap();
    let unknown = server.recorder().start_unasked(StartTrack {
        room: seat.room.clone(),
        track_id: shared.clone(),
        path: "/out/interrupted".to_owned(),
    });
    assert_eq!(server.recorder().recording().len(), 4);

    let second = server.restart().await;
    second.tend().await;
    let detail = second.recording(owner, started.id).await;
    let of_ben = of_person(&detail, ben.account_id, SegmentSource::ScreenShareAudio);
    assert_eq!(of_ben.len(), 2);
    assert_eq!(of_ben[0].id.as_uuid(), interrupted);
    assert_eq!(of_ben[0].failure, Some(SegmentFailure::StartInterrupted));
    assert_eq!(of_ben[1].state, SegmentState::Recording);
    assert_eq!(second.recorder().stops(), std::slice::from_ref(&unknown));
    let jobs = second.recorder().jobs();
    assert_eq!(jobs.len(), 5);
    let running = second.recorder().recording();
    assert_eq!(running.len(), 4);
    assert!(running.iter().all(|job| job.id != unknown));
    assert_eq!(second.recorder().job_of_track(&shared).len(), 2);

    second.tend().await;
    assert_eq!(second.recorder().jobs().len(), 5);
    second
        .stop_recording(ann, scene.voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    second.tend().await;
    second.tend().await;
    let done = second.recording(owner, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(
        (done.recording.segments, done.recording.failed_segments),
        (5, 1)
    );
}

/// ADR 0604: a recording has a bound on its segments, which is the bound of the
/// transcription job. At the bound it is stopped in order.
#[tokio::test]
async fn a_recording_is_stopped_at_the_bound_of_its_segments() {
    let server = TestServer::start_for_calls_with(|config| {
        config.limits.recordings.most_segments = 4;
    })
    .await;
    let studio = Studio::on(server).await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let started = studio.record().await;

    let shared = TrackSource::ScreenShareAudio;
    server.publish(&studio.ben_seat, TrackKind::Audio, shared);
    server.tend().await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(detail.recording.segments, 4);
    assert_eq!(detail.recording.stop, None);

    server.publish(&studio.ann_seat, TrackKind::Audio, shared);
    server.tend().await;
    let stopping = server.recording(owner, started.id).await;
    let stop = stopping.recording.stop.unwrap();
    assert_eq!(stop.reason, RecordingStopReason::SegmentLimit);
    assert_eq!(stopping.recording.segments, 4);
    assert_eq!(server.recorder().jobs().len(), 4);
    assert_eq!(server.recorder().stops().len(), 4);

    server.tend().await;
    let done = server.recording(owner, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(states(&done), [ENDED; 4]);
}

/// ADR 0602: the server names the main share in the start request and with a layout update
/// whenever the main share changes, and sends the current value again after the recorder
/// has returned to the room.
#[tokio::test]
async fn the_composite_is_told_the_main_share() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let started = studio.record().await;
    let composite = composites(&server.recorder().jobs())[0].clone();
    let Asked::Composite(request) = &composite.asked else {
        panic!("a composite");
    };
    assert_eq!(request.main_share, None);
    let told = || {
        let jobs = server.recorder().jobs();
        let job = jobs.iter().find(|job| job.id == composite.id).unwrap();
        job.main_shares.clone()
    };
    server.tend().await;
    assert!(told().is_empty());

    let share = json!({ "audio": "none" });
    let screen = (TrackKind::Video, TrackSource::ScreenShare);
    server
        .put(ann, "/api/v1/call/screen-share", &share)
        .await
        .empty(StatusCode::NO_CONTENT);
    let of_ann = server.publish(&studio.ann_seat, screen.0, screen.1);
    server.check().await;
    server.tend().await;
    assert_eq!(told(), [Some(of_ann.clone())]);
    server.tend().await;
    assert_eq!(told().len(), 1);

    server.clock.advance(Duration::seconds(1));
    server
        .put(ben, "/api/v1/call/screen-share", &share)
        .await
        .empty(StatusCode::NO_CONTENT);
    let of_ben = server.publish(&studio.ben_seat, screen.0, screen.1);
    server.check().await;
    server.tend().await;
    assert_eq!(told(), [Some(of_ann.clone()), Some(of_ben.clone())]);

    server
        .delete(ben, "/api/v1/call/screen-share")
        .await
        .empty(StatusCode::NO_CONTENT);
    let seat = &studio.ben_seat;
    server
        .fake()
        .unpublish(&seat.room, &seat.identity, screen.1);
    server.tend().await;
    assert_eq!(told().len(), 3);
    assert_eq!(told()[2], Some(of_ann.clone()));

    server.fake().leave(&seat.room, &composite.id);
    server.tend().await;
    assert_eq!(told().len(), 3);
    let recorder = ParticipantKind::Recorder;
    server
        .fake()
        .enter(&seat.room, &composite.id, recorder, None);
    server.tend().await;
    assert_eq!(told().len(), 4);
    assert_eq!(told()[3], Some(of_ann.clone()));
    server.tend().await;
    assert_eq!(told().len(), 4);

    studio.stop(started.id).await;
    studio.record().await;
    let jobs = server.recorder().recording();
    let Asked::Composite(request) = &composites(&jobs)[0].asked else {
        panic!("a composite");
    };
    assert_eq!(request.main_share, Some(of_ann));
}

/// With its task running, the controller looks at a recording at once after the request
/// that started it and after a notification of the media server (ADR 0604). The interval of
/// the controller is one hour here, so only those two make it run.
#[tokio::test]
async fn the_controller_runs_after_a_request_and_after_a_notification() {
    let server = TestServer::start_with_tasks(|config| {
        config.limits.recordings.check_interval = Duration::hours(1);
    })
    .await;
    let studio = Studio::on(server).await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);

    let started = server.record(ann, scene.voice.id).await;
    until("the jobs of the recording run", async || {
        server.recorder().recording().len() == 3
    })
    .await;

    let seat = &studio.ann_seat;
    let track = server
        .fake()
        .track_id(&seat.room, &seat.identity, TrackSource::Microphone)
        .unwrap();
    let job = server.recorder().job_of_track(&track).remove(0);
    server.recorder().fail(&job.id);
    let body = json!({
        "event": "egress_ended",
        "id": "EV_1",
        "egressInfo": { "egressId": job.id, "roomName": seat.room },
    })
    .to_string();
    let request = reqwest::Client::new()
        .request(
            Method::POST,
            format!("{}/internal/media/notifications", server.base),
        )
        .header("content-type", "application/webhook+json")
        .header(
            "authorization",
            server.fake().sign_notification(body.as_bytes()),
        )
        .body(body);
    TestServer::reply(request)
        .await
        .empty(StatusCode::NO_CONTENT);
    until("the failed job is in the record", async || {
        let detail = server.recording(owner, started.id).await;
        detail.recording.failed_segments == 1
    })
    .await;
    until("a new job records the track", async || {
        server.recorder().job_of_track(&track).len() == 2
    })
    .await;
    let detail = server.recording(owner, started.id).await;
    assert_eq!(of_source(&detail, SegmentSource::Microphone).len(), 3);
}
