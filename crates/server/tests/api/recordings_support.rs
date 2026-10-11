//! Helpers for the tests of recordings: a call with a person who may record, the fake
//! recorder, and the passes of the controller.

use junto_protocol::WorkspacePermission;
use junto_protocol::events::Event;
use junto_protocol::ids::{ChannelId, RecordingId, WorkspaceId};
use junto_protocol::recording::{Recording, RecordingDetail, RecordingPage, RecordingSegment};
use junto_server::media::{TrackKind, TrackSource};
use junto_server::recorder::fake::{FakeJob, FakeRecorder};
use junto_server::recordings::control;
use junto_server::storage::FakeDisk;
use reqwest::StatusCode;
use serde_json::json;

use crate::calls_support::{Scene, Seat};
use crate::stream_support::Stream;
use crate::support::{Member, Reply, TestServer};

/// A scene in which Ann holds `record_calls`, and Ann and Ben are in the call of the voice
/// channel, each with a microphone track.
pub struct Studio {
    pub scene: Scene,
    pub ann_seat: Seat,
    pub ben_seat: Seat,
}

impl Studio {
    pub async fn open() -> Self {
        Self::on(TestServer::start_for_calls().await).await
    }

    pub async fn on(server: TestServer) -> Self {
        let scene = Scene::on(server).await;
        let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
        let record = [WorkspacePermission::RecordCalls];
        server
            .grant_workspace(owner, ann, scene.workspace_id, &record)
            .await;
        let ann_seat = server.enter_call(ann, scene.voice.id).await;
        let ben_seat = server.enter_call(ben, scene.voice.id).await;
        server.speak(&ann_seat);
        server.speak(&ben_seat);
        Self {
            scene,
            ann_seat,
            ben_seat,
        }
    }

    /// Ann starts a recording, and the controller runs one pass: the jobs run.
    pub async fn record(&self) -> Recording {
        let server = &self.scene.server;
        let started = server.record(&self.scene.ann, self.scene.voice.id).await;
        server.tend().await;
        started
    }

    /// Ann stops the recording, and the controller runs three passes: the stops, the ends of
    /// the jobs, and the last questions about jobs that were declared lost.
    pub async fn stop(&self, recording_id: RecordingId) -> RecordingDetail {
        let server = &self.scene.server;
        server
            .stop_recording(&self.scene.ann, self.scene.voice.id)
            .await
            .empty(StatusCode::NO_CONTENT);
        for _ in 0..3 {
            server.tend().await;
        }
        server.recording(&self.scene.ann, recording_id).await
    }
}

impl TestServer {
    pub fn recorder(&self) -> &FakeRecorder {
        self.recorder
            .as_deref()
            .expect("a server started for calls")
    }

    pub fn disk(&self) -> &FakeDisk {
        self.disk.as_deref().expect("a server started for calls")
    }

    /// Runs one pass of the controller over every active recording. Before it, the fake
    /// recorder ends the files of the tracks that have ended, as a recorder does by itself.
    pub async fn tend(&self) -> usize {
        self.recorder().work();
        control::tend_all(&self.state).await.unwrap()
    }

    /// Publishes a microphone track and returns its identifier.
    pub fn speak(&self, seat: &Seat) -> String {
        self.publish(seat, TrackKind::Audio, TrackSource::Microphone)
    }

    pub fn publish(&self, seat: &Seat, kind: TrackKind, source: TrackSource) -> String {
        let fake = self.fake();
        assert!(fake.publish(&seat.room, &seat.identity, kind, source));
        fake.track_id(&seat.room, &seat.identity, source).unwrap()
    }

    pub async fn record_reply(&self, member: &Member, channel_id: ChannelId) -> Reply {
        let path = format!("/api/v1/channels/{channel_id}/call/recording");
        self.put(member, &path, &json!({})).await
    }

    /// Asks for a recording. No pass of the controller has run for it yet.
    pub async fn record(&self, member: &Member, channel_id: ChannelId) -> Recording {
        self.record_reply(member, channel_id)
            .await
            .ok(StatusCode::OK)
    }

    pub async fn stop_recording(&self, member: &Member, channel_id: ChannelId) -> Reply {
        let path = format!("/api/v1/channels/{channel_id}/call/recording");
        self.delete(member, &path).await
    }

    pub async fn recording_reply(&self, member: &Member, recording_id: RecordingId) -> Reply {
        self.get(member, &format!("/api/v1/recordings/{recording_id}"))
            .await
    }

    pub async fn recording(&self, member: &Member, recording_id: RecordingId) -> RecordingDetail {
        self.recording_reply(member, recording_id)
            .await
            .ok(StatusCode::OK)
    }

    pub async fn recordings(&self, member: &Member, workspace_id: WorkspaceId) -> RecordingPage {
        self.get(
            member,
            &format!("/api/v1/workspaces/{workspace_id}/recordings"),
        )
        .await
        .ok(StatusCode::OK)
    }

    /// The recordings in the events of recordings that `stream` received until now, each
    /// with the name of its event.
    pub async fn recording_events(
        &self,
        member: &Member,
        stream: &mut Stream,
    ) -> Vec<(&'static str, Recording)> {
        self.traffic(member, stream)
            .await
            .into_iter()
            .filter_map(|(_, event)| {
                let name = event.name();
                match event {
                    Event::RecordingStarted { recording }
                    | Event::RecordingUpdated { recording } => Some((name, *recording)),
                    _ => None,
                }
            })
            .collect()
    }

    /// The actions of the audit entries of a workspace, oldest first, each with its actor.
    pub async fn audit_actions(
        &self,
        reader: &Member,
        workspace_id: WorkspaceId,
    ) -> Vec<junto_protocol::audit::AuditEntry> {
        let page: junto_protocol::audit::AuditPage = self
            .get(
                reader,
                &format!("/api/v1/workspaces/{workspace_id}/audit-log?limit=200"),
            )
            .await
            .ok(StatusCode::OK);
        page.entries.into_iter().rev().collect()
    }
}

/// The segment of a recording that the job of the recorder with `job_id` writes, found by
/// the order of the requests: the fake recorder numbers its jobs in the order of the
/// segments of one pass.
pub fn of_source(
    detail: &RecordingDetail,
    source: junto_protocol::recording::SegmentSource,
) -> Vec<&RecordingSegment> {
    detail
        .segments
        .iter()
        .filter(|segment| segment.source == source)
        .collect()
}

pub fn composites(jobs: &[FakeJob]) -> Vec<&FakeJob> {
    jobs.iter().filter(|job| job.is_composite()).collect()
}
