//! The routes of recordings, their permissions, their events and the storage guard
//! (ADR 0601, ADR 0604, ADR 0605).

use junto_protocol::audit::AuditEvent;
use junto_protocol::error::ErrorCode;
use junto_protocol::recording::{
    Recording, RecordingFailure, RecordingPage, RecordingState, RecordingStopReason,
    SegmentFailure, SegmentSource, SegmentState,
};
use junto_protocol::server::Health;
use junto_protocol::{WorkspacePermission, WorkspacePermission as W};
use junto_server::recorder::fake::Asked;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use time::Duration;

use crate::calls_support::Scene;
use crate::recordings_support::{Studio, composites, of_source};
use crate::support::{Member, TestServer};

const GIB: u64 = 1024 * 1024 * 1024;
const RESERVE: u64 = 5 * GIB;
const RESERVATION: u64 = 8 * GIB;

async fn health(server: &TestServer) -> Health {
    server
        .send::<Value>(Method::GET, "/api/v1/health", None, None, None)
        .await
        .ok(StatusCode::OK)
}

fn states(events: &[(&'static str, Recording)]) -> Vec<(&'static str, RecordingState)> {
    events
        .iter()
        .map(|(name, recording)| (*name, recording.state))
        .collect()
}

#[tokio::test]
async fn a_participant_with_the_permission_starts_a_recording_with_one_job_per_audio_track() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let mut inside = server.full_stream(ben).await;
    let mut outside = server.full_stream(owner).await;

    let started = server.record(ann, voice).await;
    assert_eq!(started.state, RecordingState::Starting);
    assert_eq!(started.started_by, ann.account_id);
    assert_eq!(
        (started.workspace_id, started.channel_id),
        (scene.workspace_id, voice)
    );
    assert_eq!(started.language, "auto");
    assert_eq!((started.segments, started.failed_segments), (0, 0));
    assert_eq!(started.recording_since, None);
    assert_eq!(server.count("storage_reservations").await, 1);
    assert!(server.recorder().jobs().is_empty());

    assert_eq!(server.tend().await, 1);
    let jobs = server.recorder().jobs();
    assert_eq!(jobs.len(), 3);
    let detail = server.recording(ben, started.id).await;
    assert_eq!(detail.recording.state, RecordingState::Recording);
    assert_eq!(detail.recording.recording_since, Some(server.clock.now()));
    assert_eq!(detail.recording.segments, 3);
    let sources: Vec<_> = detail
        .segments
        .iter()
        .map(|segment| (segment.source, segment.account_id, segment.state))
        .collect();
    let running = SegmentState::Recording;
    assert_eq!(sources[0], (SegmentSource::Composite, None, running));
    let mut people: Vec<_> = sources[1..].to_vec();
    people.sort_by_key(|(_, account_id, _)| *account_id);
    let mut expected = vec![
        (SegmentSource::Microphone, Some(ann.account_id), running),
        (SegmentSource::Microphone, Some(ben.account_id), running),
    ];
    expected.sort_by_key(|(_, account_id, _)| *account_id);
    assert_eq!(people, expected);

    let composite = detail.segments[0].id;
    let Asked::Composite(request) = &composites(&jobs)[0].asked else {
        panic!("the first job is the composite");
    };
    assert_eq!(request.room, studio.ann_seat.room);
    assert_eq!(request.layout_url, "http://server.example.test/layout/");
    assert_eq!(request.main_share, None);
    assert_eq!(
        request.directory,
        format!("/out/{}/{composite}", started.id)
    );
    let ann_track = server
        .fake()
        .track_id(
            &studio.ann_seat.room,
            &studio.ann_seat.identity,
            junto_server::media::TrackSource::Microphone,
        )
        .unwrap();
    let of_ann = server.recorder().job_of_track(&ann_track);
    assert_eq!(of_ann.len(), 1);
    let Asked::Track(request) = &of_ann[0].asked else {
        panic!("the job of a track");
    };
    let segment = of_source(&detail, SegmentSource::Microphone)
        .into_iter()
        .find(|segment| segment.account_id == Some(ann.account_id))
        .unwrap();
    assert_eq!(request.path, format!("/out/{}/{}", started.id, segment.id));

    for (member, stream) in [(ben, &mut inside), (owner, &mut outside)] {
        let events = server.recording_events(member, stream).await;
        assert_eq!(
            states(&events),
            [
                ("recording_started", RecordingState::Starting),
                ("recording_updated", RecordingState::Starting),
                ("recording_updated", RecordingState::Recording),
            ]
        );
        assert_eq!(events[2].1, detail.recording);
    }
    let call = server.channel_call(ben, voice).await.call.unwrap();
    assert_eq!(call.recording, Some(detail.recording.clone()));
    let snapshot = server.snapshot(owner).await;
    let shown = snapshot.workspaces[0].calls[0].call.as_ref().unwrap();
    assert_eq!(shown.recording, Some(detail.recording.clone()));

    let again = server.record(ann, voice).await;
    assert_eq!(again, detail.recording);
    server.tend().await;
    assert_eq!(server.recorder().jobs().len(), 3);
    assert_eq!(server.count("recordings").await, 1);

    let entries = server.audit_actions(owner, scene.workspace_id).await;
    let recorded: Vec<_> = entries
        .iter()
        .filter(|entry| matches!(entry.event, AuditEvent::RecordingStarted { .. }))
        .collect();
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].actor_id, Some(ann.account_id));
    assert_eq!(recorded[0].channel_id, Some(voice));
    assert_eq!(
        recorded[0].event,
        AuditEvent::RecordingStarted {
            recording_id: started.id
        }
    );
    let notes: Vec<(String, Option<uuid::Uuid>)> = sqlx::query_as(
        "select kind, recording_id from call_notes where kind like 'recording%' order by id",
    )
    .fetch_all(&server.db)
    .await
    .unwrap();
    assert_eq!(
        notes,
        [("recording_started".to_owned(), Some(started.id.as_uuid()))]
    );
}

#[tokio::test]
async fn only_a_participant_with_the_permission_starts_and_stops_a_recording() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;

    server
        .record_reply(ben, voice)
        .await
        .error(ErrorCode::Forbidden);
    server
        .record_reply(owner, voice)
        .await
        .error(ErrorCode::NotInCall);
    server
        .stop_recording(ben, voice)
        .await
        .error(ErrorCode::Forbidden);
    server
        .stop_recording(owner, voice)
        .await
        .error(ErrorCode::NotInCall);

    let waiting = server.join(owner, "Cid").await;
    let record = [WorkspacePermission::RecordCalls];
    server
        .grant_workspace(owner, &waiting, scene.workspace_id, &record)
        .await;
    server.join_call(&waiting, voice).await;
    server
        .record_reply(&waiting, voice)
        .await
        .error(ErrorCode::NotInCall);

    let text = server.channel(owner, scene.workspace_id, "notes").await;
    server
        .record_reply(owner, text.id)
        .await
        .error(ErrorCode::Conflict);
    let absent = junto_protocol::ids::ChannelId::generate();
    server
        .record_reply(ann, absent)
        .await
        .error(ErrorCode::NotFound);

    let path = format!("/api/v1/channels/{voice}/call/recording");
    for language in ["english", "EN", "e", "pt-BR", ""] {
        server
            .put(ann, &path, &json!({ "language": language }))
            .await
            .error(ErrorCode::InvalidRequest);
    }
    server
        .stop_recording(ann, voice)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.count("recordings").await, 0);
    assert_eq!(server.count("storage_reservations").await, 0);

    let started: Recording = server
        .put(ann, &path, &json!({ "language": "pt" }))
        .await
        .ok(StatusCode::OK);
    assert_eq!(started.language, "pt");
    server
        .stop_recording(ben, voice)
        .await
        .error(ErrorCode::Forbidden);
    assert_eq!(server.recording(ben, started.id).await.recording.stop, None);
}

#[tokio::test]
async fn a_stop_ends_every_job_and_the_recording_waits_for_its_files() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let started = studio.record().await;
    let began = server.clock.now();
    server.clock.advance(Duration::seconds(30));
    let mut inside = server.full_stream(ben).await;

    server
        .stop_recording(ann, voice)
        .await
        .empty(StatusCode::NO_CONTENT);
    let stopping = server.recording(ben, started.id).await;
    assert_eq!(stopping.recording.state, RecordingState::Recording);
    let stop = stopping.recording.stop.unwrap();
    assert_eq!(stop.reason, RecordingStopReason::Stopped);
    assert_eq!(stop.by, Some(ann.account_id));
    assert_eq!(stop.at, server.clock.now());
    server
        .record_reply(ann, voice)
        .await
        .error(ErrorCode::Conflict);
    server
        .stop_recording(ann, voice)
        .await
        .empty(StatusCode::NO_CONTENT);

    server.tend().await;
    assert_eq!(server.recorder().stops().len(), 3);
    assert!(server.recorder().recording().is_empty());
    let delivered = server.recording(ben, started.id).await;
    assert_eq!(delivered.recording.state, RecordingState::Recording);

    server.tend().await;
    let done = server.recording(owner, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert_eq!(done.recording.ended_at, Some(server.clock.now()));
    assert_eq!(done.recording.failed_segments, 0);
    for segment in &done.segments {
        assert_eq!(segment.state, SegmentState::Ended);
        assert_eq!(segment.failure, None);
        assert_eq!(segment.started_at, Some(began));
        assert_eq!(segment.ended_at, Some(began + Duration::seconds(30)));
        assert_eq!(segment.duration_ms, Some(30_000));
    }
    let stored: Vec<Option<String>> =
        sqlx::query_scalar("select file_name from recording_segments order by id")
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert_eq!(
        stored[0],
        Some(format!(
            "/out/{}/{}/playlist.m3u8",
            started.id, done.segments[0].id
        ))
    );
    assert_eq!(
        stored[1],
        Some(format!("/out/{}/{}.ogg", started.id, done.segments[1].id))
    );
    assert_eq!(server.tend().await, 0);
    assert_eq!(server.recorder().stops().len(), 3);

    let events = server.recording_events(ben, &mut inside).await;
    assert_eq!(
        states(&events),
        [
            ("recording_updated", RecordingState::Recording),
            ("recording_updated", RecordingState::Processing),
        ]
    );
    assert!(events[0].1.stop.is_some());
    assert_eq!(events[1].1, done.recording);
    let call = server.channel_call(ben, voice).await.call.unwrap();
    assert_eq!(call.recording, None);
    assert_eq!(server.count("storage_reservations").await, 1);

    let entries = server.audit_actions(owner, scene.workspace_id).await;
    let stopped: Vec<_> = entries
        .iter()
        .filter(|entry| matches!(entry.event, AuditEvent::RecordingStopped { .. }))
        .collect();
    assert_eq!(stopped.len(), 1);
    assert_eq!(stopped[0].actor_id, Some(ann.account_id));
    assert_eq!(
        stopped[0].event,
        AuditEvent::RecordingStopped {
            recording_id: started.id,
            reason: RecordingStopReason::Stopped
        }
    );

    let second = studio.record().await;
    assert_ne!(second.id, started.id);
    assert_eq!(server.recorder().recording().len(), 3);
    let page = server.recordings(ben, scene.workspace_id).await;
    let listed: Vec<_> = page.recordings.iter().map(|listed| listed.id).collect();
    assert_eq!(listed, [second.id, started.id]);
}

#[tokio::test]
async fn without_a_recorder_no_recording_starts() {
    let server = TestServer::start_for_calls_with(|config| config.recorder = None).await;
    let studio = Studio::on(server).await;
    let scene = &studio.scene;
    let reply = scene.server.record_reply(&scene.ann, scene.voice.id).await;
    reply.error(ErrorCode::RecorderUnavailable);
    assert_eq!(scene.server.count("recordings").await, 0);
    assert_eq!(health(&scene.server).await.storage_low, None);
    assert_eq!(scene.server.tend().await, 0);
}

/// ADR 0601 and ADR 0604: when no job of the first requests starts, the server asks for the
/// stop of the recording at once, and the recording fails with the reason that no recorder
/// is available when its room holds no job.
#[tokio::test]
async fn a_recording_fails_when_no_recorder_takes_its_first_requests() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let mut inside = server.full_stream(ben).await;
    server.recorder().set_running(false);

    let started = server.record(ann, voice).await;
    assert_eq!(started.state, RecordingState::Starting);
    server.tend().await;
    let stopping = server.recording(owner, started.id).await;
    assert_eq!(stopping.recording.state, RecordingState::Starting);
    let stop = stopping.recording.stop.unwrap();
    assert_eq!(stop.reason, RecordingStopReason::RecorderLost);
    assert_eq!(stop.by, None);
    assert_eq!(server.count("storage_reservations").await, 1);

    server.tend().await;
    let failed = server.recording(owner, started.id).await;
    assert_eq!(failed.recording.state, RecordingState::Failed);
    assert_eq!(
        failed.recording.failure,
        Some(RecordingFailure::RecorderUnavailable)
    );
    assert_eq!(failed.recording.ended_at, Some(server.clock.now()));
    assert_eq!(failed.recording.recording_since, None);
    assert_eq!(
        (failed.recording.segments, failed.recording.failed_segments),
        (3, 3)
    );
    for segment in &failed.segments {
        assert_eq!(segment.state, SegmentState::Failed);
        assert_eq!(segment.failure, Some(SegmentFailure::RecorderUnavailable));
        assert_eq!((segment.started_at, segment.ended_at), (None, None));
    }
    assert_eq!(server.count("storage_reservations").await, 0);
    let events = server.recording_events(ben, &mut inside).await;
    assert_eq!(
        states(&events),
        [
            ("recording_started", RecordingState::Starting),
            ("recording_updated", RecordingState::Starting),
            ("recording_updated", RecordingState::Starting),
            ("recording_updated", RecordingState::Failed),
        ]
    );
    assert_eq!(events[2].1.stop, Some(stop));
    let failures: i64 = sqlx::query_scalar(
        "select count(*) from call_notes where kind = 'recording_failed' and recording_id = $1",
    )
    .bind(started.id.as_uuid())
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(failures, 1);
    assert_eq!(server.tend().await, 0);

    server.recorder().set_running(true);
    let second = studio.record().await;
    assert_ne!(second.id, started.id);
    let detail = server.recording(owner, second.id).await;
    assert_eq!(detail.recording.state, RecordingState::Recording);
}

/// ADR 0006 and ADR 0605: a recording starts only when the free space holds the reserve,
/// every reservation and its own reservation, and the number of recordings at one time is
/// limited.
#[tokio::test]
async fn the_storage_guard_refuses_a_recording_without_space_and_limits_their_number() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let record = [WorkspacePermission::RecordCalls];
    for member in [ann, ben] {
        server
            .grant_workspace(owner, member, scene.workspace_id, &record)
            .await;
    }
    let second = server.voice(owner, scene.workspace_id, "second").await.id;
    let third = server.voice(owner, scene.workspace_id, "third").await.id;
    let first = scene.voice.id;
    server.enter_call(ann, first).await;
    server.enter_call(ben, second).await;
    server.enter_call(owner, third).await;

    server.disk().set_free(RESERVE + RESERVATION - 1);
    server
        .record_reply(ann, first)
        .await
        .error(ErrorCode::StorageLow);
    server.disk().set_unreadable();
    server
        .record_reply(ann, first)
        .await
        .error(ErrorCode::StorageLow);
    assert_eq!(server.count("recordings").await, 0);
    assert_eq!(server.count("storage_reservations").await, 0);

    server.disk().set_free(RESERVE + RESERVATION);
    server.record(ann, first).await;
    server
        .record_reply(ben, second)
        .await
        .error(ErrorCode::StorageLow);
    server.disk().set_free(RESERVE + 2 * RESERVATION);
    server.record(ben, second).await;
    let reserved: i64 = sqlx::query_scalar("select sum(bytes)::bigint from storage_reservations")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(u64::try_from(reserved).unwrap(), 2 * RESERVATION);

    server.disk().set_free(1 << 50);
    let full = server.record_reply(owner, third).await;
    full.error(ErrorCode::RecordingsFull);
    assert_eq!(full.retry_after.as_deref(), Some("30"));
    assert_eq!(server.count("recordings").await, 2);

    server
        .stop_recording(ann, first)
        .await
        .empty(StatusCode::NO_CONTENT);
    full_is_still(server, owner, third).await;
    server.tend().await;
    server.tend().await;
    assert_eq!(
        server.record(owner, third).await.state,
        RecordingState::Starting
    );
    assert_eq!(server.count("storage_reservations").await, 2);
    let first_state: String =
        sqlx::query_scalar("select state from recordings where channel_id = $1")
            .bind(first.as_uuid())
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(first_state, "failed");
}

/// A recording whose stop is asked for still runs: it counts until its jobs have ended.
async fn full_is_still(
    server: &TestServer,
    member: &Member,
    channel_id: junto_protocol::ids::ChannelId,
) {
    server
        .record_reply(member, channel_id)
        .await
        .error(ErrorCode::RecordingsFull);
}

#[tokio::test]
async fn a_recording_that_reaches_the_reserve_is_stopped_in_order() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner) = (&scene.server, &scene.owner);
    let started = studio.record().await;
    assert_eq!(health(server).await.storage_low, Some(false));

    server.disk().set_free(RESERVE + 1);
    server.tend().await;
    assert_eq!(
        server.recording(owner, started.id).await.recording.stop,
        None
    );

    server.disk().set_free(RESERVE);
    assert_eq!(health(server).await.storage_low, Some(true));
    server
        .record_reply(&scene.ann, scene.voice.id)
        .await
        .ok::<Recording>(StatusCode::OK);
    server.tend().await;
    let stopping = server.recording(owner, started.id).await;
    let stop = stopping.recording.stop.unwrap();
    assert_eq!(stop.reason, RecordingStopReason::StorageReserve);
    assert_eq!(stop.by, None);
    assert_eq!(server.recorder().stops().len(), 3);

    server.tend().await;
    let done = server.recording(owner, started.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    assert!(
        done.segments
            .iter()
            .all(|segment| segment.state == SegmentState::Ended)
    );
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
            reason: RecordingStopReason::StorageReserve
        }
    );
    server
        .record_reply(&scene.ann, scene.voice.id)
        .await
        .error(ErrorCode::StorageLow);
    server.disk().set_unreadable();
    assert_eq!(health(server).await.storage_low, None);
}

#[tokio::test]
async fn recordings_are_read_by_the_people_who_see_the_voice_channel() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let cid = server.join(owner, "Cid").await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        let started = studio.record().await;
        studio.stop(started.id).await;
        ids.push(started.id);
        server.clock.advance(Duration::seconds(1));
    }
    ids.reverse();

    let detail = server.recording(&cid, ids[0]).await;
    assert_eq!(detail.recording.state, RecordingState::Processing);
    assert_eq!(detail.segments.len(), 3);
    let list = format!("/api/v1/workspaces/{}/recordings", scene.workspace_id);
    let first: RecordingPage = server
        .get(&cid, &format!("{list}?limit=2"))
        .await
        .ok(StatusCode::OK);
    let listed: Vec<_> = first.recordings.iter().map(|listed| listed.id).collect();
    assert_eq!(listed, ids[..2]);
    assert_eq!(first.next_before, Some(ids[1]));
    let rest: RecordingPage = server
        .get(&cid, &format!("{list}?limit=2&before={}", ids[1]))
        .await
        .ok(StatusCode::OK);
    assert_eq!(rest.recordings.len(), 1);
    assert_eq!(rest.recordings[0].id, ids[2]);
    assert_eq!(rest.next_before, None);
    for limit in [0, 201] {
        server
            .get(&cid, &format!("{list}?limit={limit}"))
            .await
            .error(ErrorCode::InvalidRequest);
    }

    let mut hidden = server.full_stream(&cid).await;
    let role = server
        .grant_workspace(owner, ann, scene.workspace_id, &[W::RecordCalls])
        .await;
    server
        .put_empty(
            owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{role}",
                scene.workspace_id, ben.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .make_private(owner, scene.workspace_id, voice, role)
        .await;
    server.check().await;
    let absent = junto_protocol::ids::RecordingId::generate();
    assert_eq!(
        server.recording_reply(&cid, ids[0]).await.observable(),
        server.recording_reply(&cid, absent).await.observable()
    );
    server
        .recording_reply(&cid, ids[0])
        .await
        .error(ErrorCode::NotFound);
    assert!(
        server
            .recordings(&cid, scene.workspace_id)
            .await
            .recordings
            .is_empty()
    );
    assert_eq!(
        server
            .recordings(ben, scene.workspace_id)
            .await
            .recordings
            .len(),
        3
    );
    let fourth = studio.record().await;
    studio.stop(fourth.id).await;
    assert!(server.recording_events(&cid, &mut hidden).await.is_empty());
    server
        .recording_reply(&cid, fourth.id)
        .await
        .error(ErrorCode::NotFound);

    let other = server
        .post(owner, "/api/v1/workspaces", &json!({ "name": "Closed" }))
        .await
        .ok::<junto_protocol::structure::Workspace>(StatusCode::CREATED);
    server
        .get(ben, &format!("/api/v1/workspaces/{}/recordings", other.id))
        .await
        .error(ErrorCode::NotFound);
    assert!(
        server
            .recordings(owner, other.id)
            .await
            .recordings
            .is_empty()
    );
}

/// ADR 0514 and ADR 0604: a recording tells who is in its call, so in a channel that hides
/// who is inside it is shown to the people inside only, for as long as the call runs: also
/// after the recording itself has ended. When the call has ended, the people who see the
/// channel read it and are told of it.
#[tokio::test]
async fn a_recording_in_a_hidden_channel_is_shown_to_the_people_outside_when_its_call_has_ended() {
    let studio = Studio::hidden().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let mut inside = server.full_stream(ben).await;
    let mut outside = server.full_stream(owner).await;
    let absent = junto_protocol::ids::RecordingId::generate();
    let hidden_from_the_owner = async |id| {
        assert_eq!(
            server.recording_reply(owner, id).await.observable(),
            server.recording_reply(owner, absent).await.observable()
        );
        assert!(
            server
                .recordings(owner, scene.workspace_id)
                .await
                .recordings
                .is_empty()
        );
    };

    let first = studio.record().await;
    assert_eq!(server.recording_events(ben, &mut inside).await.len(), 3);
    assert!(
        server
            .recording_events(owner, &mut outside)
            .await
            .is_empty()
    );
    hidden_from_the_owner(first.id).await;
    assert_eq!(
        server
            .recordings(ben, scene.workspace_id)
            .await
            .recordings
            .len(),
        1
    );

    let done = studio.stop(first.id).await;
    assert_eq!(done.recording.state, RecordingState::Processing);
    let events = server.recording_events(ben, &mut inside).await;
    assert_eq!(events.last().unwrap().1, done.recording);
    assert!(
        server
            .recording_events(owner, &mut outside)
            .await
            .is_empty()
    );
    hidden_from_the_owner(first.id).await;
    assert_eq!(server.recording(ben, first.id).await, done);

    let second = studio.record().await;
    hidden_from_the_owner(second.id).await;
    assert!(
        server
            .recording_events(owner, &mut outside)
            .await
            .is_empty()
    );
    server.leave_call(ann).await;
    server.leave_call(ben).await;
    let events = server.recording_events(owner, &mut outside).await;
    assert_eq!(
        states(&events),
        [
            ("recording_updated", RecordingState::Processing),
            ("recording_updated", RecordingState::Recording),
        ]
    );
    assert_eq!(events[0].1, done.recording);
    assert_eq!(events[1].1.id, second.id);
    assert_eq!(
        events[1].1.stop.unwrap().reason,
        RecordingStopReason::CallEnded
    );
    assert_eq!(server.recording(owner, first.id).await, done);
    assert_eq!(
        server
            .recordings(owner, scene.workspace_id)
            .await
            .recordings
            .len(),
        2
    );

    server.tend().await;
    server.tend().await;
    let events = server.recording_events(owner, &mut outside).await;
    assert_eq!(
        states(&events),
        [("recording_updated", RecordingState::Processing)]
    );
    assert_eq!(
        server.recording(owner, second.id).await.recording,
        events[0].1
    );
}

/// ADR 0517: a channel that stops hiding who is inside while its call runs shows nothing of
/// the recordings of that call. The people outside read them, and are told of them, when
/// the call has ended.
#[tokio::test]
async fn a_channel_that_stops_hiding_who_is_inside_shows_the_recordings_when_its_call_has_ended() {
    let studio = Studio::hidden().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let first = studio.record().await;
    let done = studio.stop(first.id).await;
    let second = studio.record().await;
    let mut outside = server.full_stream(owner).await;

    server
        .call_settings(owner, voice, json!({ "hides_occupants": false }))
        .await;
    assert!(
        server
            .recording_events(owner, &mut outside)
            .await
            .is_empty()
    );
    for hidden in [first.id, second.id] {
        server
            .recording_reply(owner, hidden)
            .await
            .error(ErrorCode::NotFound);
    }

    server.leave_call(ann).await;
    server.leave_call(ben).await;
    let events = server.recording_events(owner, &mut outside).await;
    assert_eq!(
        states(&events),
        [
            ("recording_updated", RecordingState::Processing),
            ("recording_updated", RecordingState::Recording),
        ]
    );
    assert_eq!(events[0].1, done.recording);
    assert_eq!(events[1].1.id, second.id);
    assert_eq!(server.recording(owner, first.id).await, done);
}

/// ADR 0604: the end of a call in a hidden channel tells the people outside of at most 50
/// recordings of that call, and the one that still runs is among them.
#[tokio::test]
async fn the_end_of_a_hidden_call_tells_of_at_most_fifty_recordings() {
    let studio = Studio::hidden().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let first = studio.record().await;
    studio.stop(first.id).await;
    sqlx::query(
        "insert into recordings
             (id, workspace_id, channel_id, call_id, room_name, state, started_by, language,
              requested_at, recording_since, stop_reason, stop_requested_at, ended_at)
         select gen_random_uuid(), workspace_id, channel_id, call_id, room_name, 'processing',
                started_by, 'auto', requested_at, requested_at, 'stopped', requested_at,
                requested_at
         from recordings, generate_series(1, 60)
         where id = $1",
    )
    .bind(first.id.as_uuid())
    .execute(&server.db)
    .await
    .unwrap();
    let last = studio.record().await;
    let mut outside = server.full_stream(owner).await;

    server.leave_call(ann).await;
    server.leave_call(ben).await;
    let events = server.recording_events(owner, &mut outside).await;
    assert_eq!(events.len(), 50);
    let running: Vec<_> = events
        .iter()
        .filter(|(_, recording)| recording.state == RecordingState::Recording)
        .collect();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].1.id, last.id);
    let page: RecordingPage = server
        .get(
            owner,
            &format!(
                "/api/v1/workspaces/{}/recordings?limit=200",
                scene.workspace_id
            ),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(page.recordings.len(), 62);
}

/// ADR 0604: the caller is in the call when the media server lists the seat. A person whose
/// connection to the media server ended, and whom the admission check saw absent, does not
/// start or stop a recording in the time in which the place is kept.
#[tokio::test]
async fn a_person_whom_the_media_server_no_longer_lists_does_not_start_or_stop_a_recording() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, ann) = (&scene.server, &scene.ann);
    let voice = scene.voice.id;
    let started = studio.record().await;

    let seat = &studio.ann_seat;
    server.fake().leave(&seat.room, &seat.identity);
    server.check().await;
    assert!(server.own_call(ann).await.call.is_some());
    server
        .stop_recording(ann, voice)
        .await
        .error(ErrorCode::NotInCall);
    assert_eq!(
        server
            .recording(&scene.ben, started.id)
            .await
            .recording
            .stop,
        None
    );

    let returned = server.connect(server.join_call(ann, voice).await);
    assert_eq!(returned.identity, seat.identity);
    server
        .stop_recording(ann, voice)
        .await
        .error(ErrorCode::NotInCall);
    server.check().await;
    server
        .stop_recording(ann, voice)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.tend().await;
    server.tend().await;

    server.fake().leave(&seat.room, &seat.identity);
    server.check().await;
    server
        .record_reply(ann, voice)
        .await
        .error(ErrorCode::NotInCall);
    assert_eq!(server.count("recordings").await, 1);
}

#[tokio::test]
async fn two_requests_at_the_same_time_start_one_recording() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let record = [WorkspacePermission::RecordCalls];
    server
        .grant_workspace(owner, ben, scene.workspace_id, &record)
        .await;
    let voice = scene.voice.id;
    let (first, second) = tokio::join!(server.record(ann, voice), server.record(ben, voice));
    assert_eq!(first.id, second.id);
    assert_eq!(server.count("recordings").await, 1);
    assert_eq!(server.count("storage_reservations").await, 1);
    let entries = server.audit_actions(owner, scene.workspace_id).await;
    let started = entries
        .iter()
        .filter(|entry| matches!(entry.event, AuditEvent::RecordingStarted { .. }))
        .count();
    assert_eq!(started, 1);
}

/// ADR 0904: the database itself refuses the states that break the rules of a recording.
#[tokio::test]
async fn the_database_refuses_inconsistent_recordings() {
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    let started = studio.record().await;
    let refused = |statement: String| async move {
        let result = sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&server.db)
            .await;
        let error = result.expect_err("the statement must be refused");
        error
            .as_database_error()
            .unwrap()
            .code()
            .unwrap()
            .into_owned()
    };
    let id = started.id;
    let second = uuid::Uuid::now_v7();
    assert_eq!(
        refused(format!(
            "insert into recordings
                 (id, workspace_id, channel_id, call_id, room_name, state, started_by, language,
                  requested_at)
             select '{second}', workspace_id, channel_id, call_id, room_name, 'starting',
                    started_by, 'auto', requested_at
             from recordings where id = '{id}'"
        ))
        .await,
        "23505"
    );
    for change in [
        "state = 'failed'",
        "state = 'processing'",
        "failure = 'nothing_recorded'",
        "stop_reason = 'stopped'",
        "stopped_by = started_by",
        "language = 'English'",
        "state = 'paused'",
        "state = 'starting'",
    ] {
        let code = refused(format!("update recordings set {change} where id = '{id}'")).await;
        assert_eq!(code, "23514", "{change}");
    }
    for change in [
        "state = 'failed'",
        "failure = 'recorder_lost'",
        "state = 'starting'",
        "job_id = null",
        "account_id = null where source = 'microphone'",
        "track_id = 'TR_x' where source = 'composite'",
        "duration_ms = -1",
        "source = 'camera'",
    ] {
        let code = refused(format!("update recording_segments set {change}")).await;
        assert_eq!(code, "23514", "{change}");
    }
    assert_eq!(
        refused("update recording_segments set job_id = 'EG_same'".to_owned()).await,
        "23505"
    );
    assert_eq!(
        refused("update storage_reservations set bytes = 0".to_owned()).await,
        "23514"
    );
}
