//! Helpers for the tests of calls: a server with the fake media server, voice channels, and
//! clients of the fake media server.

use junto_protocol::call::{CallJoin, ChannelCall, OwnCallView};
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::{ChannelId, WorkspaceId};
use junto_protocol::structure::Channel;
use junto_server::calls::admission::{self, Outcome};
use junto_server::media::fake::{FakeMedia, SECRET};
use junto_server::media::token;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::stream_support::Stream;
use crate::support::{Member, Reply, TestServer};

/// A person in the room of the fake media server.
pub struct Seat {
    pub join: CallJoin,
    pub room: String,
    pub identity: String,
}

/// A server with an owner, two further members of the default workspace and one voice
/// channel.
pub struct Scene {
    pub server: TestServer,
    pub owner: Member,
    pub ann: Member,
    pub ben: Member,
    pub workspace_id: WorkspaceId,
    pub voice: Channel,
}

impl Scene {
    pub async fn start() -> Self {
        Self::on(TestServer::start_for_calls().await).await
    }

    pub async fn on(server: TestServer) -> Self {
        let owner = server.owner().await;
        let ann = server.join(&owner, "Ann").await;
        let ben = server.join(&owner, "Ben").await;
        let workspace_id = server.default_workspace(&owner).await.id;
        let voice = server.voice(&owner, workspace_id, "lounge").await;
        Self {
            server,
            owner,
            ann,
            ben,
            workspace_id,
            voice,
        }
    }
}

pub fn claims(join: &CallJoin) -> Value {
    token::verify(SECRET, join.token.expose()).expect("a token of the fake media server")
}

impl TestServer {
    pub fn fake(&self) -> &FakeMedia {
        self.media.as_deref().expect("a server started for calls")
    }

    /// Runs one admission check.
    pub async fn check(&self) -> Outcome {
        admission::check(&self.state).await.unwrap()
    }

    /// Waits until the checks that the requests so far started have ended. For a server
    /// whose tasks run.
    pub async fn settled(&self) {
        let started = std::time::Instant::now();
        while !self.state.calls.settled() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(20),
                "the admission check did not settle"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    }

    pub async fn voice(&self, manager: &Member, workspace_id: WorkspaceId, name: &str) -> Channel {
        self.post(
            manager,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
            &json!({ "name": name, "kind": "voice" }),
        )
        .await
        .ok(StatusCode::CREATED)
    }

    pub async fn call_settings(&self, manager: &Member, channel_id: ChannelId, body: Value) {
        self.patch(
            manager,
            &format!("/api/v1/channels/{channel_id}/call/settings"),
            &body,
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    }

    pub async fn call_join_reply(&self, member: &Member, channel_id: ChannelId) -> Reply {
        self.post(
            member,
            &format!("/api/v1/channels/{channel_id}/call/join"),
            &Value::Null,
        )
        .await
    }

    /// Asks for a place in the call of a voice channel. The person is not in the room yet.
    pub async fn join_call(&self, member: &Member, channel_id: ChannelId) -> CallJoin {
        self.call_join_reply(member, channel_id)
            .await
            .ok(StatusCode::OK)
    }

    /// Connects to the fake media server with the token of a join.
    pub fn connect(&self, join: CallJoin) -> Seat {
        let (room, identity) = self.fake().connect(&join.token).expect("an accepted token");
        assert_eq!(identity, join.identity);
        Seat {
            join,
            room,
            identity,
        }
    }

    /// Joins the call of a voice channel, connects to the fake media server and runs a
    /// check, so that the person is in the call for everyone.
    pub async fn enter_call(&self, member: &Member, channel_id: ChannelId) -> Seat {
        let seat = self.connect(self.join_call(member, channel_id).await);
        self.check().await;
        seat
    }

    /// Joins the direct-message call with `other`, connects to the fake media server and
    /// runs a check.
    pub async fn enter_direct(&self, member: &Member, other: &Member) -> Seat {
        let join = self
            .post(
                member,
                &format!("/api/v1/direct-calls/{}/join", other.account_id),
                &Value::Null,
            )
            .await
            .ok(StatusCode::OK);
        let seat = self.connect(join);
        self.check().await;
        seat
    }

    pub async fn leave_call(&self, member: &Member) {
        self.delete(member, "/api/v1/call")
            .await
            .empty(StatusCode::NO_CONTENT);
    }

    pub async fn channel_call(&self, member: &Member, channel_id: ChannelId) -> ChannelCall {
        self.get(member, &format!("/api/v1/channels/{channel_id}/call"))
            .await
            .ok(StatusCode::OK)
    }

    pub async fn own_call(&self, member: &Member) -> OwnCallView {
        self.get(member, "/api/v1/call").await.ok(StatusCode::OK)
    }

    /// The accounts in the call of a voice channel, as `viewer` sees them.
    pub async fn occupants(
        &self,
        viewer: &Member,
        channel_id: ChannelId,
    ) -> Vec<junto_protocol::ids::AccountId> {
        self.channel_call(viewer, channel_id)
            .await
            .call
            .map(|call| {
                call.participants
                    .into_iter()
                    .map(|participant| participant.account_id)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// A full stream of `member` and the types of the events that it receives until now.
    pub async fn full_stream(&self, member: &Member) -> Stream {
        self.subscribed(member, Level::Full).await
    }

    /// The events of calls that `stream` received until now, without the events of the
    /// stream's own account that every device gets.
    pub async fn call_events(&self, member: &Member, stream: &mut Stream) -> Vec<Event> {
        self.traffic(member, stream)
            .await
            .into_iter()
            .map(|(_, event)| event)
            .filter(|event| event.name().starts_with("call_") || event.name() == "own_call_changed")
            .collect()
    }

    pub async fn count(&self, table: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("select count(*) from {table}")))
            .fetch_one(&self.db)
            .await
            .unwrap()
    }
}

pub fn names(events: &[Event]) -> Vec<&'static str> {
    events.iter().map(Event::name).collect()
}
