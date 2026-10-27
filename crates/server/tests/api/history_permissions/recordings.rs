use super::*;
use crate::recordings_support::{FakeWorker, Said, Studio, transcript_of};
use junto_protocol::recording::{RecordingDetail, SegmentSource};
use junto_server::media::{TrackKind, TrackSource};
use serde_json::json;

async fn denied_recording() -> (Studio, RecordingDetail) {
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    studio.scene.server.publish(
        &studio.ann_seat,
        TrackKind::Audio,
        TrackSource::ScreenShareAudio,
    );
    let done = studio.finished_after(Duration::seconds(1)).await;
    let shared = done
        .segments
        .iter()
        .filter(|s| s.source != SegmentSource::Composite && s.file.is_some())
        .position(|s| s.source == SegmentSource::ScreenShareAudio)
        .unwrap();
    let scene = &studio.scene;
    worker.give(&transcript_of(
        &done,
        (&[], &[]),
        &[
            Said(0, 100, 900, "Recorded words.", None),
            Said(shared, 100, 900, "Shared words.", Some(1)),
        ],
    ));
    assert!(scene.server.transcribe().await);
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(scene.voice.id).bind(scene.ben.account_id).bind(scene.workspace_id).bind(1_i64 << W::ReadMessageHistory as u8).execute(&scene.server.db).await.unwrap();
    (studio, done)
}

#[tokio::test]
async fn recording_detail_requires_history_permission() {
    let (studio, done) = denied_recording().await;
    let scene = &studio.scene;
    scene
        .server
        .recording_reply(&scene.ben, done.recording.id)
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn recording_lists_exclude_channels_without_history_permission() {
    let (studio, _) = denied_recording().await;
    let scene = &studio.scene;
    let list = scene
        .server
        .recordings(&scene.ben, scene.workspace_id)
        .await;
    assert!(list.recordings.is_empty());
}

#[tokio::test]
async fn recording_files_require_history_permission() {
    let (studio, done) = denied_recording().await;
    let scene = &studio.scene;
    let segment = done
        .segments
        .iter()
        .find(|segment| segment.file.is_some())
        .unwrap();
    scene
        .server
        .get(
            &scene.ben,
            &format!(
                "/api/v1/recordings/{}/segments/{}/file",
                done.recording.id, segment.id
            ),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn transcript_header_lines_and_text_require_history_permission() {
    let (studio, done) = denied_recording().await;
    let scene = &studio.scene;
    let mut statuses = Vec::new();
    for suffix in ["", "/lines", "/text"] {
        let path = format!(
            "/api/v1/recordings/{}/transcript{suffix}",
            done.recording.id
        );
        assert_eq!(
            scene.server.get(&scene.ann, &path).await.status,
            StatusCode::OK
        );
        statuses.push(scene.server.get(&scene.ben, &path).await.status);
    }
    assert_eq!(statuses, vec![StatusCode::FORBIDDEN; 3]);
}

#[tokio::test]
async fn changing_a_transcript_line_requires_history_permission() {
    let (studio, done) = denied_recording().await;
    let scene = &studio.scene;
    scene
        .server
        .put(
            &scene.ben,
            &format!(
                "/api/v1/recordings/{}/transcript/lines/0",
                done.recording.id
            ),
            &json!({"text":"Changed"}),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn changing_a_speaker_name_requires_history_permission() {
    let (studio, done) = denied_recording().await;
    let scene = &studio.scene;
    scene
        .server
        .put(
            &scene.ben,
            &format!(
                "/api/v1/recordings/{}/transcript/speakers/1",
                done.recording.id
            ),
            &json!({"name":"Speaker"}),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn changing_a_recording_by_removing_it_requires_history_permission() {
    let (studio, done) = denied_recording().await;
    let scene = &studio.scene;
    scene
        .server
        .grant_workspace(
            &scene.owner,
            &scene.ben,
            scene.workspace_id,
            &[W::ManageRecordings],
        )
        .await;
    scene
        .server
        .delete(
            &scene.ben,
            &format!("/api/v1/recordings/{}", done.recording.id),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn changing_a_recording_by_requesting_a_transcript_requires_history_permission() {
    let (studio, done) = denied_recording().await;
    let scene = &studio.scene;
    scene
        .server
        .grant_workspace(
            &scene.owner,
            &scene.ben,
            scene.workspace_id,
            &[W::ManageRecordings],
        )
        .await;
    scene
        .server
        .post(
            &scene.ben,
            &format!("/api/v1/recordings/{}/transcript", done.recording.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn recording_control_keeps_its_record_calls_permission_without_history() {
    let studio = Studio::open().await;
    let scene = &studio.scene;
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(scene.voice.id).bind(scene.ann.account_id).bind(scene.workspace_id).bind(1_i64 << W::ReadMessageHistory as u8).execute(&scene.server.db).await.unwrap();
    studio.record().await;
    scene
        .server
        .stop_recording(&scene.ann, scene.voice.id)
        .await
        .empty(StatusCode::NO_CONTENT);
    scene.server.tend().await;
}
