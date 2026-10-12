//! The defects that the review of the event stream found, each with its scenario.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use junto_protocol::ServerPermission;
use junto_protocol::account::RecoveryNotice;
use junto_protocol::audit::AuditPage;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{
    ClientMessage, CloseCode, EndReason, Event, Level, ResyncReason, ServerMessage,
};
use junto_protocol::roles::{ServerRole, WorkspaceRole};
use junto_protocol::structure::{Channel, Workspace};
use junto_server::events::connection;
use junto_server::events::delivery::{self, Batch, Opened, Subscription};
use junto_server::events::hub::Arrivals;
use junto_server::events::transport::{Violation, server_socket};
use junto_server::extract::{Bearer, Credential};
use junto_server::secrets::hash_session_token;
use reqwest::StatusCode;
use serde_json::{Value, json};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::Role;
use tokio_tungstenite::tungstenite::protocol::frame::Frame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};

use crate::stream::{apply, sorted};
use crate::stream_support::Frame as Received;
use crate::support::{DeviceKey, Member, TestServer};

fn milliseconds(count: i64) -> time::Duration {
    time::Duration::milliseconds(count)
}

fn types(events: &[(u64, Event)]) -> Vec<&'static str> {
    events.iter().map(|(_, event)| event.name()).collect()
}

pub async fn role(server: &TestServer, owner: &Member, workspace: &Workspace, name: &str) -> Reply {
    server
        .post(
            owner,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
            &json!({ "name": name }),
        )
        .await
}

pub type Reply = crate::support::Reply;

pub async fn assign(
    server: &TestServer,
    owner: &Member,
    workspace: &Workspace,
    member: &Member,
    role: &WorkspaceRole,
    held: bool,
) {
    let path = format!(
        "/api/v1/workspaces/{}/members/{}/roles/{}",
        workspace.id, member.account_id, role.id
    );
    let reply = if held {
        server.put_empty(owner, &path).await
    } else {
        server.delete(owner, &path).await
    };
    reply.empty(StatusCode::NO_CONTENT);
}

pub struct Private {
    pub server: TestServer,
    pub owner: Member,
    pub insider: Member,
    pub workspace: Workspace,
    pub team: WorkspaceRole,
    pub channel: Channel,
}

/// A channel that only the holders of the role `team` see. The insider holds the role.
pub async fn private_channel(configure: impl FnOnce(&mut junto_server::config::Config)) -> Private {
    let server = TestServer::start_with(configure).await;
    let owner = server.owner().await;
    let insider = server.join(&owner, "Insider").await;
    let workspace = server.default_workspace(&owner).await;
    let team: WorkspaceRole = role(&server, &owner, &workspace, "team")
        .await
        .ok(StatusCode::CREATED);
    assign(&server, &owner, &workspace, &insider, &team, true).await;
    let channel = server.channel(&owner, workspace.id, "secret-plans").await;
    server
        .make_private(&owner, workspace.id, channel.id, team.id)
        .await;
    Private {
        server,
        owner,
        insider,
        workspace,
        team,
        channel,
    }
}

async fn rename(server: &TestServer, owner: &Member, channel: &Channel, name: &str) {
    server
        .patch(
            owner,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({ "name": name }),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
}

/// Makes every statement that reads `table` wait until the returned transaction ends.
async fn block_reads_of(
    server: &TestServer,
    table: &str,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let mut transaction = server.db.begin().await.unwrap();
    let lock = format!("lock table {table} in access exclusive mode");
    sqlx::query(sqlx::AssertSqlSafe(lock))
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction
}

/// Records an event about the private channel and makes the read of the batch that holds it
/// wait in the database, after the transaction of the batch has taken its snapshot.
pub async fn queue_an_event_and_block_its_read(
    world: &Private,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    queue_an_event_and_block_its_read_of(world, "channel_overrides").await
}

/// Like [`queue_an_event_and_block_its_read`], with the lock on `table`. The removal of a
/// role reads the channel overrides of the role (ADR 0304), so a test that takes a role
/// while the read waits locks the state of the log, which a delivery reads after it has
/// taken its snapshot and which no request reads.
async fn queue_an_event_and_block_its_read_of(
    world: &Private,
    table: &str,
) -> sqlx::Transaction<'static, sqlx::Postgres> {
    let server = &world.server;
    server.state.streams.hold();
    rename(server, &world.owner, &world.channel, "secret-renamed").await;
    let lock = block_reads_of(server, table).await;
    server.state.streams.release();
    server.until_blocked().await;
    lock
}

#[tokio::test]
async fn a_batch_that_was_read_from_an_old_snapshot_is_discarded() {
    let world = private_channel(|config| {
        config.limits.stream.authorization_validity = milliseconds(2_000);
    })
    .await;
    let (server, owner) = (&world.server, &world.owner);
    let mut stream = server.subscribed(&world.insider, Level::Full).await;

    let lock = queue_an_event_and_block_its_read_of(&world, "event_stream").await;

    assign(
        server,
        owner,
        &world.workspace,
        &world.insider,
        &world.team,
        false,
    )
    .await;
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    lock.rollback().await.unwrap();

    let received = server.traffic(&world.insider, &mut stream).await;
    assert_eq!(
        types(&received),
        ["workspace_member_roles_changed", "permissions_changed"]
    );
    assert!(!stream.raw.join("\n").contains("secret"));
}

#[tokio::test]
async fn the_replay_decision_counts_only_the_events_that_the_account_may_receive() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.replay_most_events = 3;
    })
    .await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let workspace = server.default_workspace(&owner).await;
    let closed: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Closed" }))
        .await
        .ok(StatusCode::CREATED);
    let start = server.snapshot(&plain).await.cursor;

    for index in 0..5 {
        server.invite(&owner, json!({})).await;
        server
            .category(&owner, closed.id, &format!("hidden {index}"), index)
            .await;
    }
    server.category(&owner, workspace.id, "seen 0", 0).await;
    server.category(&owner, workspace.id, "seen 1", 1).await;
    server.category(&owner, workspace.id, "seen 2", 2).await;

    for level in [Level::Full, Level::Background] {
        let mut stream = server.stream(&plain).await;
        assert_eq!(stream.subscribe(level, Some(start)).await, start);
        let received = server.traffic(&plain, &mut stream).await;
        let expected = match level {
            Level::Full => 3,
            Level::Background => 0,
        };
        assert_eq!(
            types(&received)
                .iter()
                .filter(|name| **name == "category_created")
                .count(),
            expected
        );
    }

    server.category(&owner, workspace.id, "seen 3", 3).await;
    let mut full = server.stream(&plain).await;
    full.send(&ClientMessage::Subscribe {
        level: Level::Full,
        cursor: Some(start),
    })
    .await;
    assert_eq!(
        full.next().await,
        Received::Message(ServerMessage::Resync {
            reason: ResyncReason::TooFarBehind
        })
    );
    let mut background = server.stream(&plain).await;
    assert_eq!(
        background.subscribe(Level::Background, Some(start)).await,
        start
    );
}

#[tokio::test]
async fn a_replaced_connection_that_waits_for_the_database_closes_at_once() {
    let world = private_channel(|_| {}).await;
    let server = &world.server;
    let mut waiting = server.subscribed(&world.insider, Level::Full).await;

    let lock = queue_an_event_and_block_its_read(&world).await;

    let _second = server.stream(&world.insider).await;
    let _third = server.stream(&world.insider).await;
    let closed = tokio::time::timeout(Duration::from_millis(3_500), waiting.until_closed()).await;
    assert_eq!(
        closed.map(|(_, code)| code),
        Ok(Some(CloseCode::Replaced)),
        "the connection stayed in its database read"
    );
    lock.rollback().await.unwrap();
}

#[tokio::test]
async fn a_ban_closes_a_connection_that_waits_for_the_database() {
    let world = private_channel(|_| {}).await;
    let server = &world.server;
    let mut waiting = server.subscribed(&world.insider, Level::Full).await;

    let lock = queue_an_event_and_block_its_read(&world).await;

    server
        .put_empty(
            &world.owner,
            &format!("/api/v1/accounts/{}/ban", world.insider.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let closed = tokio::time::timeout(Duration::from_millis(3_500), waiting.until_closed()).await;
    assert_eq!(
        closed,
        Ok((
            vec![ServerMessage::Ended {
                reason: EndReason::Banned,
                recovery: None
            }],
            Some(CloseCode::Ended)
        ))
    );
    lock.rollback().await.unwrap();
}

#[tokio::test]
async fn a_read_that_takes_too_long_is_given_up_and_the_connection_stays_responsive() {
    let world = private_channel(|_| {}).await;
    let server = &world.server;
    let mut stream = server.subscribed(&world.insider, Level::Full).await;
    server.tighten_streams(|limits| limits.read_deadline = milliseconds(300));

    let lock = queue_an_event_and_block_its_read(&world).await;

    stream
        .send(&ClientMessage::Subscribe {
            level: Level::Background,
            cursor: None,
        })
        .await;
    let answer = stream.within(Duration::from_millis(4_000)).await;
    assert!(
        matches!(
            answer,
            Some(Received::Message(ServerMessage::Subscribed {
                level: Level::Background,
                ..
            }))
        ),
        "{answer:?}"
    );
    lock.rollback().await.unwrap();
}

#[tokio::test]
async fn roles_and_role_assignments_have_ceilings() {
    let server = TestServer::start_with(|config| {
        config.limits.most_roles_per_workspace = 4;
        config.limits.most_roles_per_member = 2;
        config.limits.most_server_roles_per_account = 2;
    })
    .await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    let workspace = server.default_workspace(&owner).await;
    let limit_reached = |reply: &Reply| {
        assert_eq!(reply.status, StatusCode::CONFLICT, "{}", reply.text());
        let body: Value = serde_json::from_str(&reply.text()).unwrap();
        assert_eq!(body["error"]["code"], "limit_reached");
    };

    let mut roles: Vec<WorkspaceRole> = Vec::new();
    for index in 0..3 {
        roles.push(
            role(&server, &owner, &workspace, &format!("role {index}"))
                .await
                .ok(StatusCode::CREATED),
        );
    }
    limit_reached(&role(&server, &owner, &workspace, "one too many").await);

    assign(&server, &owner, &workspace, &guest, &roles[0], true).await;
    assign(&server, &owner, &workspace, &guest, &roles[1], true).await;
    assign(&server, &owner, &workspace, &guest, &roles[1], true).await;
    let third = format!(
        "/api/v1/workspaces/{}/members/{}/roles/{}",
        workspace.id, guest.account_id, roles[2].id
    );
    limit_reached(&server.put_empty(&owner, &third).await);
    assign(&server, &owner, &workspace, &guest, &roles[0], false).await;
    server
        .put_empty(&owner, &third)
        .await
        .empty(StatusCode::NO_CONTENT);

    let mut server_roles: Vec<ServerRole> = Vec::new();
    for index in 0..3 {
        server_roles.push(
            server
                .post(
                    &owner,
                    "/api/v1/server-roles",
                    &json!({ "name": format!("role {index}") }),
                )
                .await
                .ok(StatusCode::CREATED),
        );
    }
    let path = |role: &ServerRole| {
        format!(
            "/api/v1/accounts/{}/server-roles/{}",
            guest.account_id, role.id
        )
    };
    for role in &server_roles[..2] {
        server
            .put_empty(&owner, &path(role))
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    limit_reached(&server.put_empty(&owner, &path(&server_roles[2])).await);
}

#[tokio::test]
async fn an_invite_names_a_bounded_number_of_workspaces() {
    let server = TestServer::start_with(|config| {
        config.limits.invite_most_workspaces = 2;
    })
    .await;
    let owner = server.owner().await;
    let mut workspace_ids = vec![server.default_workspace(&owner).await.id];
    for name in ["Second", "Third"] {
        let workspace: Workspace = server
            .post(&owner, "/api/v1/workspaces", &json!({ "name": name }))
            .await
            .ok(StatusCode::CREATED);
        workspace_ids.push(workspace.id);
    }
    server
        .post(
            &owner,
            "/api/v1/invites",
            &json!({ "workspace_ids": workspace_ids }),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .invite(&owner, json!({ "workspace_ids": workspace_ids[..2] }))
        .await;

    let proof = server
        .proof(
            &DeviceKey::generate(),
            junto_protocol::auth::ChallengeOperation::RequestToJoin,
        )
        .await;
    let request: junto_protocol::invite::JoinRequest = server
        .open(
            "/api/v1/join-requests",
            &json!({ "proof": proof, "name": "Asking" }),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .post(
            &owner,
            &format!("/api/v1/join-requests/{}/approval", request.id),
            &json!({ "workspace_ids": workspace_ids }),
        )
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn a_ban_records_one_event_for_all_the_invites_that_it_cancels() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let guest = server.join(&owner, "Guest").await;
    server
        .grant_server(&owner, &guest, &[ServerPermission::CreateInvites])
        .await;
    for _ in 0..5 {
        server.invite(&guest, json!({})).await;
    }
    let mut stream = server.subscribed(&owner, Level::Full).await;
    let before = server.log().await.len();
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", guest.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);

    let recorded: Vec<String> = server.log().await[before..]
        .iter()
        .map(|(_, kind)| kind.clone())
        .collect();
    assert_eq!(
        recorded,
        ["account_status_changed", "invites_of_creator_revoked"]
    );
    let received = server.traffic(&owner, &mut stream).await;
    let frame: Value = serde_json::from_str(&stream.raw[stream.raw.len() - 2]).unwrap();
    assert_eq!(received.len(), 2);
    assert_eq!(frame["event"]["created_by"], guest.account_id.to_string());
    assert_eq!(frame["event"]["revocation"], "creator_banned");
}

#[tokio::test]
async fn the_database_refuses_an_event_that_is_too_large() {
    let server = TestServer::start().await;
    let insert = |payload_bytes: usize, audience_bytes: usize| {
        let statement = format!(
            "insert into events (id, recorded_at, class, kind, audience, payload)
             values (gen_random_uuid(), now(), 'general', 'a',
                     jsonb_build_object('to', 'workspace', 'pad', repeat('x', {audience_bytes})),
                     jsonb_build_object('type', 'a', 'pad', repeat('x', {payload_bytes})))"
        );
        let pool = server.db.clone();
        async move {
            sqlx::query(sqlx::AssertSqlSafe(statement))
                .execute(&pool)
                .await
        }
    };
    assert!(insert(1_000, 1_000).await.is_ok());
    assert!(insert(20_000, 1_000).await.is_err());
    assert!(insert(1_000, 70_000).await.is_err());
}

pub fn opened(member: &Member) -> Opened {
    Opened {
        bearer: Bearer(hash_session_token(&member.token), Credential::Session),
        account_id: member.account_id,
        device_id: member.device_id,
        session_id: uuid::Uuid::nil(),
    }
}

#[tokio::test]
async fn a_batch_has_a_budget_of_bytes() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let start = server.snapshot(&owner).await.cursor;
    server
        .large_events(
            json!({ "to": "workspace", "workspace_id": workspace.id }),
            json!({ "type": "workspace_updated", "workspace": workspace }),
            &["workspace", "name"],
            64,
        )
        .await;

    let subscription = Subscription {
        level: Level::Full,
        position: i64::try_from(start.position).unwrap(),
        epoch: start.epoch,
    };
    let batch = delivery::next_batch(&server.state, &opened(&owner), subscription, 64)
        .await
        .unwrap();
    let Batch::Events {
        deliveries, more, ..
    } = batch
    else {
        panic!("{batch:?}");
    };
    let bytes: usize = deliveries
        .iter()
        .map(|delivery| serde_json::to_string(&delivery.message).unwrap().len())
        .sum();
    assert!(more);
    assert!(!deliveries.is_empty());
    assert!(bytes <= 256 * 1024 + 16 * 1024, "{bytes}");
}

#[tokio::test]
async fn a_stored_event_that_is_too_large_is_skipped_and_the_client_is_told_to_resynchronise() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.poll_interval = milliseconds(50);
    })
    .await;
    let owner = server.owner().await;
    let plain = server.join(&owner, "Plain").await;
    let workspace = server.default_workspace(&owner).await;
    let mut stream = server.stream(&owner).await;
    let start = stream.subscribe(Level::Full, None).await;
    let mut unaffected = server.subscribed(&plain, Level::Full).await;

    server.category(&owner, workspace.id, "before", 0).await;
    for constraint in ["events_payload_size", "events_audience_size"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "alter table events drop constraint if exists {constraint}"
        )))
        .execute(&server.db)
        .await
        .unwrap();
    }
    sqlx::query(
        "insert into events (id, recorded_at, class, kind, audience, payload)
         values (gen_random_uuid(), now(), 'general', 'invite_revoked',
                 jsonb_build_object('to', 'server_permission', 'permission', 'create_invites'),
                 jsonb_build_object('type', 'invite_revoked', 'pad', repeat('x', 2000000)))",
    )
    .execute(&server.db)
    .await
    .unwrap();

    let expect_skip = |stream: &crate::stream_support::Stream| {
        let last: Value = serde_json::from_str(stream.raw.last().unwrap()).unwrap();
        assert_eq!(last["type"], "resync");
        assert_eq!(last["reason"], "skipped_event");
    };
    assert_eq!(stream.event().await.1.name(), "category_created");
    assert!(matches!(
        stream.next().await,
        Received::Message(ServerMessage::Resync { .. })
    ));
    expect_skip(&stream);

    let snapshot = server.snapshot(&owner).await;
    stream.subscribe(Level::Full, Some(snapshot.cursor)).await;
    server.category(&owner, workspace.id, "after", 1).await;
    assert_eq!(stream.event().await.1.name(), "category_created");

    let mut again = server.stream(&owner).await;
    again.subscribe(Level::Full, Some(start)).await;
    assert_eq!(again.event().await.1.name(), "category_created");
    assert!(matches!(
        again.next().await,
        Received::Message(ServerMessage::Resync { .. })
    ));
    expect_skip(&again);

    let seen = server.traffic(&plain, &mut unaffected).await;
    assert_eq!(types(&seen), ["category_created", "category_created"]);
}

#[tokio::test]
async fn a_fragmented_client_message_closes_the_connection() {
    let server = TestServer::start().await;
    let owner = server.owner().await;

    let mut stream = server.stream(&owner).await;
    let first = Frame::message(
        r#"{"type":"subscribe","#.as_bytes().to_vec(),
        OpCode::Data(Data::Text),
        false,
    );
    stream.send_frame(Message::Frame(first)).await.unwrap();
    let closed = tokio::time::timeout(Duration::from_secs(4), stream.until_closed()).await;
    assert_eq!(
        closed.map(|(_, code)| code),
        Ok(Some(CloseCode::InvalidMessage)),
        "the server waits for the rest of a fragmented message"
    );

    let mut flooding = server.stream(&owner).await;
    let opening = Frame::message(Vec::new(), OpCode::Data(Data::Text), false);
    let _ = flooding.send_frame(Message::Frame(opening)).await;
    for _ in 0..200 {
        let empty = Frame::message(Vec::new(), OpCode::Data(Data::Continue), false);
        if flooding.send_frame(Message::Frame(empty)).await.is_err() {
            break;
        }
    }
    let closed = tokio::time::timeout(Duration::from_secs(4), flooding.until_closed()).await;
    assert!(
        closed.is_ok(),
        "the server accepted the continuation frames"
    );
}

#[tokio::test]
async fn the_frame_limit_applies_before_the_library_hands_out_a_message() {
    let mut limits = junto_server::config::StreamLimits::default();
    limits.client_frames.attempts = 5;
    for (frames, refused) in [(5, false), (6, true)] {
        let (near, far) = tokio::io::duplex(64 * 1024);
        let mut client = WebSocketStream::from_raw_socket(far, Role::Client, None).await;
        for _ in 0..frames {
            client.send(Message::Ping(Vec::new().into())).await.unwrap();
        }
        let mut server = server_socket(near, &limits).await;
        let inbound = server.get_ref().inbound();
        let first = server.next().await;
        if refused {
            assert!(
                matches!(first, Some(Err(_))),
                "a message of a burst above the limit was handed out: {first:?}"
            );
            assert_eq!(inbound.violation(), Some(Violation::TooManyFrames));
        } else {
            assert!(matches!(first, Some(Ok(Message::Ping(_)))), "{first:?}");
            assert_eq!(inbound.violation(), None);
        }
    }
}

#[tokio::test]
async fn a_burst_of_control_frames_closes_the_connection() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.client_frames.attempts = 5;
    })
    .await;
    let owner = server.owner().await;
    let mut stream = server.stream(&owner).await;
    for _ in 0..10 {
        if stream
            .send_frame(Message::Ping(Vec::new().into()))
            .await
            .is_err()
        {
            break;
        }
    }
    assert_eq!(
        stream.until_closed().await.1,
        Some(CloseCode::TooManyMessages)
    );
}

#[tokio::test]
async fn a_workspace_that_another_account_creates_reaches_an_administrator_whole() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    let mut snapshot = server.snapshot(&administrator).await;
    let mut stream = server.stream(&administrator).await;
    stream.subscribe(Level::Full, Some(snapshot.cursor)).await;

    let created: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Second" }))
        .await
        .ok(StatusCode::CREATED);
    server.category(&owner, created.id, "Projects", 0).await;
    server.channel(&owner, created.id, "plans").await;
    role(&server, &owner, &created, "helpers")
        .await
        .ok::<WorkspaceRole>(StatusCode::CREATED);

    for (_, event) in server.traffic(&administrator, &mut stream).await {
        apply(&server, &administrator, &mut snapshot, event).await;
    }
    let live = sorted(server.snapshot(&administrator).await);
    snapshot.cursor = live.cursor;
    assert_eq!(sorted(snapshot), live);
    assert_eq!(live.workspaces.len(), 2);
}

type Pipe = WebSocketStream<tokio::io::DuplexStream>;

async fn text_within(client: &mut Pipe, wait: Duration) -> Option<ServerMessage> {
    let read = async {
        loop {
            match client.next().await {
                Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).ok(),
                Some(Ok(_)) => {}
                _ => return None,
            }
        }
    };
    tokio::time::timeout(wait, read).await.ok().flatten()
}

#[tokio::test]
async fn a_control_message_survives_a_check_of_the_session() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let epoch = server.snapshot(&owner).await.cursor.epoch;
    let streams = &server.state.streams;
    let interrupt = || {
        let mut arrivals = Arrivals::default();
        arrivals.revalidate.insert(owner.account_id);
        streams.wake(&arrivals);
    };
    let pause = || tokio::time::sleep(Duration::from_millis(200));

    let (near, far) = tokio::io::duplex(64);
    let socket = server_socket(near, &server.state.config.limits.stream).await;
    let registration = streams.open(owner.account_id, owner.device_id).unwrap();
    let app = server.state.clone();
    let who = opened(&owner);
    tokio::spawn(connection::run(app, socket, who, epoch, registration));
    let mut client = WebSocketStream::from_raw_socket(far, Role::Client, None).await;

    pause().await;
    interrupt();
    pause().await;
    let hello = text_within(&mut client, Duration::from_secs(2)).await;
    assert!(
        matches!(hello, Some(ServerMessage::Hello { .. })),
        "{hello:?}"
    );

    let subscribe = |cursor| {
        let message = ClientMessage::Subscribe {
            level: Level::Full,
            cursor,
        };
        Message::Text(serde_json::to_string(&message).unwrap().into())
    };
    client.send(subscribe(None)).await.unwrap();
    pause().await;
    interrupt();
    pause().await;
    let subscribed = text_within(&mut client, Duration::from_secs(2)).await;
    assert!(
        matches!(subscribed, Some(ServerMessage::Subscribed { .. })),
        "{subscribed:?}"
    );

    server.category(&owner, workspace.id, "Projects", 0).await;
    let event = text_within(&mut client, Duration::from_secs(2)).await;
    assert!(
        matches!(event, Some(ServerMessage::Event { .. })),
        "{event:?}"
    );

    let unknown = junto_protocol::events::Cursor {
        epoch,
        position: u64::MAX,
    };
    client.send(subscribe(Some(unknown))).await.unwrap();
    pause().await;
    interrupt();
    pause().await;
    let resync = text_within(&mut client, Duration::from_secs(2)).await;
    assert!(
        matches!(resync, Some(ServerMessage::Resync { .. })),
        "{resync:?}"
    );
}

#[tokio::test]
async fn a_recovery_is_the_reason_also_after_the_device_has_logged_in_again() {
    let first = TestServer::start_with(|config| {
        config.limits.stream.poll_interval = time::Duration::seconds(60);
    })
    .await;
    let owner = first.owner().await;
    let guest = first.join(&owner, "Guest").await;
    let workspace = first.default_workspace(&owner).await;
    let second = first.restart().await;
    let mut old = second.stream(&guest).await;
    assert_eq!(old.within(Duration::from_millis(300)).await, None);

    let notice: RecoveryNotice = first
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/recovery", guest.account_id),
            &json!({ "public_key": DeviceKey::generate().public() }),
        )
        .await
        .ok(StatusCode::CREATED);
    let session = first.login(&guest.key).await;
    assert_eq!(session.recoveries, std::slice::from_ref(&notice));

    second.category(&owner, workspace.id, "wake", 0).await;
    assert_eq!(
        old.until_closed().await,
        (
            vec![ServerMessage::Ended {
                reason: EndReason::Recovered,
                recovery: Some(notice.clone()),
            }],
            Some(CloseCode::Ended)
        )
    );
    async fn pending(server: &TestServer, token: &str) -> Vec<RecoveryNotice> {
        server
            .send::<Value>(
                reqwest::Method::GET,
                "/api/v1/account/recovery-notices",
                Some(token),
                None,
                None,
            )
            .await
            .ok(StatusCode::OK)
    }
    let listed = pending(&first, session.token.expose()).await;
    assert_eq!(listed, std::slice::from_ref(&notice));
    first
        .send::<Value>(
            reqwest::Method::DELETE,
            &format!("/api/v1/account/recovery-notices/{}", notice.id),
            Some(session.token.expose()),
            None,
            None,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(pending(&second, session.token.expose()).await.is_empty());

    let mut later = first
        .open_stream(Some(session.token.expose()))
        .await
        .unwrap();
    first
        .send::<Value>(
            reqwest::Method::DELETE,
            "/api/v1/auth/session",
            Some(session.token.expose()),
            None,
            None,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        later.until_closed().await,
        (
            vec![ServerMessage::Ended {
                reason: EndReason::SessionEnded,
                recovery: None,
            }],
            Some(CloseCode::Ended)
        )
    );
}

#[tokio::test]
async fn a_stop_waits_until_the_connections_have_closed() {
    let mut server = TestServer::start_with(|config| {
        config.limits.stream.close_timeout = milliseconds(800);
    })
    .await;
    let owner = server.owner().await;
    let mut answering = server.subscribed(&owner, Level::Full).await;
    let _silent = server.subscribed(&owner, Level::Full).await;

    let reading = tokio::spawn(async move { answering.until_closed().await });
    let started = Instant::now();
    server.stop().await;
    assert!(
        started.elapsed() >= Duration::from_millis(600),
        "the stop returned after {:?}, before the close handshake of a connection ended",
        started.elapsed()
    );
    assert_eq!(server.state.streams.connections(), 0);
    assert_eq!(reading.await.unwrap(), (vec![], Some(CloseCode::Stopping)));
}

#[tokio::test]
async fn a_stop_aborts_the_connections_that_do_not_close_in_time() {
    let mut server = TestServer::start_with(|config| {
        config.limits.stream.close_timeout = time::Duration::seconds(30);
        config.limits.stream.stop_timeout = milliseconds(400);
    })
    .await;
    let owner = server.owner().await;
    let _silent = server.subscribed(&owner, Level::Full).await;

    let started = Instant::now();
    server.stop().await;
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(300), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert_eq!(server.state.streams.connections(), 0);
}

#[tokio::test]
async fn a_new_epoch_makes_every_older_cursor_unknown() {
    let server = TestServer::start_with(|config| {
        config.limits.stream.heartbeat = milliseconds(100);
        config.limits.stream.client_frames.attempts = 10_000;
    })
    .await;
    let owner = server.owner().await;
    let mut live = server.stream(&owner).await;
    let before = live.subscribe(Level::Full, None).await;

    let command = server.host_command(&["new-event-epoch"]).await;
    assert!(command.success, "{}", command.stderr);
    assert!(command.stdout.contains("epoch"), "{}", command.stdout);

    assert_eq!(
        live.next().await,
        Received::Message(ServerMessage::Resync {
            reason: ResyncReason::UnknownCursor
        })
    );
    let after = server.snapshot(&owner).await.cursor;
    assert_ne!(after.epoch, before.epoch);
    assert_eq!(after.position, before.position);

    let mut stream = server.stream(&owner).await;
    assert_eq!(stream.epoch, after.epoch);
    stream
        .send(&ClientMessage::Subscribe {
            level: Level::Full,
            cursor: Some(before),
        })
        .await;
    assert_eq!(
        stream.next().await,
        Received::Message(ServerMessage::Resync {
            reason: ResyncReason::UnknownCursor
        })
    );
    assert_eq!(stream.subscribe(Level::Full, Some(after)).await, after);

    let log: AuditPage = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    let newest = serde_json::to_value(&log.entries[0].event).unwrap();
    assert_eq!(newest["action"], "event_epoch_renewed");
    assert!(log.entries[0].actor_id.is_none());
}
