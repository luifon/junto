//! Tests for the defects that the review of the call unit found: each states a rule that an
//! earlier version broke.

use std::time::{Duration as Wait, Instant};

use junto_protocol::call::{CallInvitation, CallJoin, CallMove, LeaveReason};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::AccountId;
use junto_protocol::{WorkspacePermission, WorkspacePermission as W};
use junto_server::calls::admission::Outcome;
use junto_server::calls::speaking;
use junto_server::config::RateLimit;
use junto_server::media::ParticipantKind;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use time::Duration;

use crate::calls_support::{Scene, names};
use crate::support::{Member, TestServer, member};

const CLEAN: Outcome = Outcome::Checked {
    removed: 0,
    updated: 0,
};

fn removed(count: usize) -> Outcome {
    Outcome::Checked {
        removed: count,
        updated: 0,
    }
}

async fn health(server: &TestServer) -> Value {
    server
        .send::<Value>(Method::GET, "/api/v1/health", None, None, None)
        .await
        .ok(StatusCode::OK)
}

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

async fn move_to(
    server: &TestServer,
    mover: &Member,
    from: junto_protocol::ids::ChannelId,
    account_id: AccountId,
    to: junto_protocol::ids::ChannelId,
) -> CallMove {
    let path = format!("/api/v1/channels/{from}/call/participants/{account_id}/move");
    server
        .post(mover, &path, &json!({ "to_channel_id": to }))
        .await
        .ok(StatusCode::CREATED)
}

/// Finding 1: the end of a session ends its place in a call in the same transaction, and a
/// later session of the same device gets another identity.
#[tokio::test]
async fn a_session_that_ended_leaves_no_place_and_its_seat_is_not_used_again() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    let seat = server.enter_call(ann, voice).await;
    server.enter_call(owner, voice).await;
    move_to(server, owner, voice, ann.account_id, garden).await;
    assert_eq!(server.count("call_moves").await, 1);
    let mut to_owner = server.full_stream(owner).await;

    server
        .delete(ann, "/api/v1/auth/session")
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.count("call_members").await, 1);
    assert_eq!(server.count("call_moves").await, 0);
    assert_eq!(server.occupants(owner, voice).await, [owner.account_id]);
    let told = server.call_events(owner, &mut to_owner).await;
    assert_eq!(
        names(&told),
        ["call_move_resolved", "call_participant_left"]
    );

    let session = server.login(&ann.key).await;
    let again = Member {
        key: crate::support::DeviceKey::generate(),
        account_id: session.account_id,
        device_id: session.device_id,
        token: session.token.expose().to_owned(),
    };
    assert_eq!(again.device_id, ann.device_id);
    let join = server.join_call(&again, voice).await;
    assert_ne!(join.identity, seat.identity);
    assert!(
        server
            .fake()
            .identities(&seat.room)
            .contains(&seat.identity)
    );
    let new_seat = server.connect(join);
    assert_eq!(server.check().await, removed(1));
    assert!(
        !server
            .fake()
            .identities(&seat.room)
            .contains(&seat.identity)
    );
    server.fake().connect(&seat.join.token).unwrap();
    assert_eq!(server.check().await, removed(1));
    assert!(
        server
            .fake()
            .identities(&seat.room)
            .contains(&new_seat.identity)
    );

    let ben_seat = server.enter_call(ben, voice).await;
    sqlx::query("update sessions set expires_at = $2 where device_id = $1")
        .bind(ben.device_id.as_uuid())
        .bind(server.clock.now())
        .execute(&server.db)
        .await
        .unwrap();
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    let members: Vec<uuid::Uuid> = sqlx::query_scalar("select account_id from call_members")
        .fetch_all(&server.db)
        .await
        .unwrap();
    assert!(!members.contains(&ben.account_id.as_uuid()), "{members:?}");
    let session = server.login(&ben.key).await;
    let ben_again = member(crate::support::DeviceKey::generate(), session);
    let rejoined = server.join_call(&ben_again, voice).await;
    assert_ne!(rejoined.identity, ben_seat.identity);

    let carl = server.join(owner, "Carl").await;
    server.enter_call(&carl, voice).await;
    let before = server.count("call_members").await;
    server
        .put_empty(owner, &format!("/api/v1/accounts/{}/ban", carl.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.count("call_members").await, before - 1);

    let dora = server.join(owner, "Dora").await;
    let dora_seat = server.enter_call(&dora, voice).await;
    let session = server.login(&dora.key).await;
    let second = member(crate::support::DeviceKey::generate(), session);
    assert_eq!(second.device_id, dora.device_id);
    let state = json!({ "muted": true, "deafened": false });
    server
        .put(&second, "/api/v1/call/state", &state)
        .await
        .error(ErrorCode::NotInCall);
    let taken = server.join_call(&second, voice).await;
    assert_ne!(taken.identity, dora_seat.identity);
    server
        .put(&dora, "/api/v1/call/state", &state)
        .await
        .error(ErrorCode::NotInCall);
}

/// Finding 2: a room whose requests fail does not stop the check of the other rooms, and one
/// action that fails does not stop the other actions.
#[tokio::test]
async fn a_room_that_fails_does_not_stop_the_enforcement_in_the_other_rooms() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    let ann_seat = server.enter_call(ann, lounge).await;
    let ben_seat = server.enter_call(ben, garden).await;
    let carl = server.join(owner, "Carl").await;
    let carl_seat = server.enter_call(&carl, garden).await;

    server.fake().fail_room(&ben_seat.room, true);
    server.leave_call(ann).await;
    server.leave_call(&carl).await;
    assert_eq!(server.check().await, removed(1));
    assert!(server.fake().identities(&ann_seat.room).is_empty());
    assert!(
        server
            .fake()
            .identities(&ben_seat.room)
            .contains(&carl_seat.identity)
    );
    assert_eq!(health(server).await["media_server"], true);
    server.join_call(ann, lounge).await;
    server
        .call_join_reply(owner, garden)
        .await
        .error(ErrorCode::MediaUnavailable);
    for _ in 0..3 {
        server.check().await;
    }
    assert_eq!(health(server).await["failing_call_rooms"], 1);

    server.fake().fail_room(&ben_seat.room, false);
    assert_eq!(server.check().await, removed(1));
    assert_eq!(
        server.fake().identities(&ben_seat.room),
        std::slice::from_ref(&ben_seat.identity)
    );
    assert_eq!(health(server).await["failing_call_rooms"], 0);
    server.join_call(owner, garden).await;

    for intruder in ["first", "second", "third"] {
        server
            .fake()
            .enter(&ben_seat.room, intruder, ParticipantKind::Standard, None);
    }
    let block = format!(
        "/api/v1/channels/{garden}/call/participants/{}/microphone-block",
        ben.account_id
    );
    server
        .put_empty(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.fake().fail_removal_of("first", true);
    assert_eq!(
        server.check().await,
        Outcome::Checked {
            removed: 2,
            updated: 1
        }
    );
    let left = server.fake().identities(&ben_seat.room);
    assert!(left.contains(&"first".to_owned()) && !left.contains(&"second".to_owned()));
    assert!(!left.contains(&"third".to_owned()));
    server.fake().fail_removal_of("first", false);
    assert_eq!(server.check().await, removed(1));
    assert_eq!(
        server.fake().identities(&ben_seat.room),
        std::slice::from_ref(&ben_seat.identity)
    );
}

/// Finding 4: the own call is read and changed only by an account that may still be at the
/// place of the call.
#[tokio::test]
async fn an_account_that_lost_sight_of_the_channel_reads_and_changes_nothing_of_its_call() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let role = server
        .grant_workspace(owner, ann, scene.workspace_id, &[])
        .await;
    let role_of = |account: AccountId| {
        format!(
            "/api/v1/workspaces/{}/members/{account}/roles/{role}",
            scene.workspace_id
        )
    };
    server
        .put_empty(owner, &role_of(ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let board = server.voice(owner, scene.workspace_id, "board").await.id;
    server
        .make_private(owner, scene.workspace_id, board, role)
        .await;
    server.enter_call(ann, board).await;
    server.enter_call(ben, board).await;
    assert_eq!(
        server
            .own_call(ann)
            .await
            .call
            .unwrap()
            .call
            .participants
            .len(),
        2
    );

    server.fake().set_unreachable(true);
    server
        .delete(owner, &role_of(ann.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.check().await, Outcome::Unreachable);
    assert_eq!(server.count("call_members").await, 2);

    let own = server.get(ann, "/api/v1/call").await;
    assert_eq!(own.status, StatusCode::OK);
    assert!(
        !own.text().contains(&ben.account_id.to_string()),
        "{}",
        own.text()
    );
    assert!(!own.text().contains(&board.to_string()), "{}", own.text());
    assert_eq!(server.own_call(ann).await.call, None);
    let snapshot = server.get(ann, "/api/v1/snapshot").await.text();
    assert!(!snapshot.contains(&board.to_string()), "{snapshot}");
    for (method, path, body) in [
        (
            Method::PUT,
            "/api/v1/call/state",
            json!({ "muted": true, "deafened": false }),
        ),
        (
            Method::PUT,
            "/api/v1/call/screen-share",
            json!({ "audio": "none" }),
        ),
        (Method::DELETE, "/api/v1/call/screen-share", Value::Null),
        (Method::PUT, "/api/v1/call/hand", Value::Null),
        (
            Method::PUT,
            "/api/v1/call/speaking",
            json!({ "speaking": true }),
        ),
    ] {
        let body = (!body.is_null()).then_some(&body);
        let reply = server
            .send(method.clone(), path, Some(&ann.token), body, None)
            .await;
        assert_eq!(reply.code(), ErrorCode::NotInCall, "{method} {path}");
    }
    let shown = server
        .channel_call(ben, board)
        .await
        .call
        .unwrap()
        .participants
        .into_iter()
        .find(|participant| participant.account_id == ann.account_id)
        .unwrap();
    assert!(!shown.muted && !shown.hand_raised && shown.screen_share.is_none());
    server.leave_call(ann).await;
    assert_eq!(server.count("call_members").await, 1);
}

/// Finding 5: an event for one account that names a channel reaches the account only while
/// it may see that channel, and the event that tells an end names nothing protected.
#[tokio::test]
async fn an_event_for_one_account_about_a_channel_needs_sight_of_the_channel_at_delivery() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let role = server
        .grant_workspace(owner, ann, scene.workspace_id, &[])
        .await;
    let role_of = |account: AccountId| {
        format!(
            "/api/v1/workspaces/{}/members/{account}/roles/{role}",
            scene.workspace_id
        )
    };
    server
        .put_empty(owner, &role_of(ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let board = server.voice(owner, scene.workspace_id, "board").await.id;
    server
        .make_private(owner, scene.workspace_id, board, role)
        .await;
    server.enter_call(ann, board).await;
    let mut to_ben = server.subscribed(ben, Level::Background).await;

    server.state.streams.hold();
    let invitation: CallInvitation = server
        .post(
            ann,
            &format!("/api/v1/channels/{board}/call/invitations"),
            &json!({ "account_id": ben.account_id }),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .delete(owner, &role_of(ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.state.streams.release();
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());

    let frames = to_ben.raw.len();
    server.clock.advance(Duration::seconds(60));
    server.check().await;
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["call_access_ended"]
    );
    let told = to_ben.raw[frames..].join("\n");
    let protected = [
        invitation.id.to_string(),
        board.to_string(),
        ann.account_id.to_string(),
    ];
    for protected in protected {
        assert!(!told.contains(&protected), "{told}");
    }

    let mut to_ben = server.full_stream(ben).await;
    server.enter_call(ben, lounge).await;
    server
        .put_empty(owner, &role_of(ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.call_events(ben, &mut to_ben).await;
    server.state.streams.hold();
    let pending = move_to(server, owner, lounge, ben.account_id, board).await;
    server
        .delete(owner, &role_of(ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.state.streams.release();
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());
    let frames = to_ben.raw.len();
    server.clock.advance(Duration::seconds(30));
    server.check().await;
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["call_access_ended"]
    );
    let told = to_ben.raw[frames..].join("\n");
    assert!(!told.contains(&pending.id.to_string()), "{told}");
    assert!(!told.contains(&board.to_string()), "{told}");

    server.leave_call(ben).await;
    server
        .put_empty(owner, &role_of(ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.call_events(ben, &mut to_ben).await;
    server.state.streams.hold();
    server.join_call(ben, board).await;
    server
        .delete(owner, &role_of(ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.state.streams.release();
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());
    server.check().await;
    assert_eq!(
        server.call_events(ben, &mut to_ben).await,
        [Event::OwnCallChanged {
            call: None,
            left: Some(LeaveReason::AccessEnded),
        }]
    );
}

/// Finding 6: the audit entries of a move do not tell the readers of one channel about the
/// other channel.
#[tokio::test]
async fn a_move_is_in_the_audit_log_of_each_channel_without_the_other_channel() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let role = server
        .grant_workspace(owner, ann, scene.workspace_id, &[])
        .await;
    let vault = server.voice(owner, scene.workspace_id, "vault").await.id;
    server
        .make_private(owner, scene.workspace_id, vault, role)
        .await;
    server
        .grant_workspace(owner, ben, scene.workspace_id, &[W::ViewAuditEntries])
        .await;
    server.enter_call(ann, lounge).await;
    let pending = move_to(server, owner, lounge, ann.account_id, vault).await;
    let join: CallJoin = server
        .post(
            ann,
            &format!("/api/v1/call/moves/{}/join", pending.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    server.connect(join);
    server.check().await;
    assert_eq!(
        server.own_call(ann).await.call.unwrap().call.id,
        pending_call(server, vault).await
    );

    let log = format!("/api/v1/workspaces/{}/audit-log", scene.workspace_id);
    let for_ben = server.get(ben, &log).await;
    assert_eq!(for_ben.status, StatusCode::OK);
    assert!(
        !for_ben.text().contains(&vault.to_string()),
        "{}",
        for_ben.text()
    );
    let entries: Value = serde_json::from_str(&for_ben.text()).unwrap();
    let moved_out = entries["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["event"]["action"] == "call_member_moved_out")
        .unwrap_or_else(|| panic!("{entries}"));
    assert_eq!(moved_out["channel_id"], lounge.to_string());
    assert_eq!(moved_out["event"]["account_id"], ann.account_id.to_string());
    assert_eq!(moved_out["actor_id"], owner.account_id.to_string());

    let entries: Value = serde_json::from_str(&server.get(owner, &log).await.text()).unwrap();
    let moved_in = entries["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["event"]["action"] == "call_member_moved_in")
        .unwrap_or_else(|| panic!("{entries}"));
    assert_eq!(moved_in["channel_id"], vault.to_string());
    assert!(
        !moved_in.to_string().contains(&lounge.to_string()),
        "{moved_in}"
    );
}

async fn pending_call(
    server: &TestServer,
    channel_id: junto_protocol::ids::ChannelId,
) -> junto_protocol::ids::CallId {
    let id: uuid::Uuid = sqlx::query_scalar("select id from calls where channel_id = $1")
        .bind(channel_id.as_uuid())
        .fetch_one(&server.db)
        .await
        .unwrap();
    junto_protocol::ids::CallId::from_uuid(id)
}

/// Finding 7: a request that changes nothing starts no check, and a request that changes one
/// call checks that room only.
#[tokio::test]
async fn a_request_that_changes_nothing_sends_no_request_to_the_media_server() {
    let server = TestServer::start_with_tasks(|config| {
        config.limits.calls.check_interval = Duration::hours(1);
    })
    .await;
    let scene = Scene::on(server).await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    until("the first check has run", async || {
        server.state.calls.reachable() == Some(true)
    })
    .await;
    let other = server.voice(owner, scene.workspace_id, "garden").await.id;
    let ben_seat = server.connect(server.join_call(ben, other).await);
    server.settled().await;
    server
        .fake()
        .enter(&ben_seat.room, "intruder", ParticipantKind::Standard, None);

    let before = server.fake().requests();
    for _ in 0..20 {
        server.leave_call(ann).await;
        server.delete(ann, "/api/v1/call/screen-share").await;
        server.delete(ann, "/api/v1/call/hand").await;
    }
    let block = format!(
        "/api/v1/channels/{other}/call/participants/{}/microphone-block",
        ann.account_id
    );
    server
        .delete(owner, &block)
        .await
        .error(ErrorCode::NotFound);
    server.settled().await;
    assert_eq!(server.fake().requests(), before);

    let seat = server.connect(server.join_call(ann, scene.voice.id).await);
    server.leave_call(ann).await;
    until("the person who left is removed from the room", async || {
        server.fake().identities(&seat.room).is_empty()
    })
    .await;
    server.settled().await;
    assert!(
        server
            .fake()
            .identities(&ben_seat.room)
            .contains(&"intruder".to_owned()),
        "a change of one call started a check of another room"
    );
    assert!(
        server.fake().requests() - before <= 6,
        "{}",
        server.fake().requests() - before
    );
}

/// Finding 7: every request that changes a call counts against a limit of its session.
#[tokio::test]
async fn every_request_that_changes_a_call_is_limited() {
    let three = RateLimit {
        attempts: 3,
        window: Duration::minutes(1),
    };
    let server = TestServer::start_for_calls_with(|config| {
        config.limits.calls.actions = three;
        config.limits.calls.state_reports = three;
        config.limits.calls.speaking_reports = three;
    })
    .await;
    let scene = Scene::on(server).await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let absent = uuid::Uuid::now_v7();
    let person = ben.account_id;
    let participant = format!("/api/v1/channels/{voice}/call/participants/{person}");
    let none = Value::Null;
    let routes: Vec<(Method, String, Value)> = vec![
        (Method::DELETE, "/api/v1/call".to_owned(), none.clone()),
        (
            Method::PUT,
            "/api/v1/call/state".to_owned(),
            json!({ "muted": true, "deafened": false }),
        ),
        (
            Method::PUT,
            "/api/v1/call/screen-share".to_owned(),
            json!({ "audio": "none" }),
        ),
        (
            Method::DELETE,
            "/api/v1/call/screen-share".to_owned(),
            none.clone(),
        ),
        (Method::PUT, "/api/v1/call/hand".to_owned(), none.clone()),
        (Method::DELETE, "/api/v1/call/hand".to_owned(), none.clone()),
        (
            Method::PUT,
            "/api/v1/call/speaking".to_owned(),
            json!({ "speaking": true }),
        ),
        (
            Method::PATCH,
            format!("/api/v1/channels/{voice}/call/settings"),
            json!({ "user_limit": 5 }),
        ),
        (
            Method::POST,
            format!("/api/v1/channels/{voice}/call/join"),
            none.clone(),
        ),
        (
            Method::PUT,
            format!("/api/v1/channels/{voice}/call/lock"),
            none.clone(),
        ),
        (
            Method::DELETE,
            format!("/api/v1/channels/{voice}/call/lock"),
            none.clone(),
        ),
        (
            Method::POST,
            format!("/api/v1/channels/{voice}/call/entry-requests"),
            none.clone(),
        ),
        (
            Method::POST,
            format!("/api/v1/call/entry-requests/{absent}/answer"),
            json!({ "accept": true }),
        ),
        (
            Method::DELETE,
            format!("/api/v1/call/entry-requests/{absent}"),
            none.clone(),
        ),
        (
            Method::POST,
            format!("/api/v1/channels/{voice}/call/invitations"),
            json!({ "account_id": person }),
        ),
        (
            Method::POST,
            format!("/api/v1/call/invitations/{absent}/acceptance"),
            none.clone(),
        ),
        (
            Method::POST,
            format!("/api/v1/call/invitations/{absent}/refusal"),
            none.clone(),
        ),
        (
            Method::DELETE,
            format!("/api/v1/call/invitations/{absent}"),
            none.clone(),
        ),
        (
            Method::POST,
            format!("{participant}/move"),
            json!({ "to_channel_id": absent }),
        ),
        (
            Method::POST,
            format!("/api/v1/call/moves/{absent}/join"),
            none.clone(),
        ),
        (
            Method::POST,
            format!("/api/v1/call/moves/{absent}/failure"),
            none.clone(),
        ),
        (
            Method::PUT,
            format!("/api/v1/channels/{voice}/call/meeting"),
            none.clone(),
        ),
        (
            Method::DELETE,
            format!("/api/v1/channels/{voice}/call/meeting"),
            none.clone(),
        ),
        (
            Method::PUT,
            format!("{participant}/microphone-block"),
            none.clone(),
        ),
        (
            Method::DELETE,
            format!("{participant}/microphone-block"),
            none.clone(),
        ),
        (
            Method::PUT,
            format!("{participant}/sound-block"),
            none.clone(),
        ),
        (
            Method::DELETE,
            format!("{participant}/sound-block"),
            none.clone(),
        ),
        (Method::DELETE, participant.clone(), none.clone()),
        (
            Method::POST,
            format!("/api/v1/direct-calls/{person}/join"),
            none.clone(),
        ),
        (
            Method::POST,
            format!("/api/v1/direct-calls/{person}/refusal"),
            none.clone(),
        ),
    ];
    let document: Value = serde_json::from_str(&junto_server::openapi_document().unwrap()).unwrap();
    let mut changing = 0;
    for (path, operations) in document["paths"].as_object().unwrap() {
        let of_calls = path.contains("/call") || path.contains("/direct-calls/");
        for method in operations.as_object().unwrap().keys() {
            if of_calls && method != "get" {
                changing += 1;
            }
        }
    }
    assert_eq!(
        routes.len(),
        changing,
        "a call route is missing in this test"
    );

    for (method, path, body) in &routes {
        let send = || async {
            let body = (!body.is_null()).then_some(body);
            server
                .send(method.clone(), path, Some(&ann.token), body, None)
                .await
        };
        for attempt in 1..=3 {
            let reply = send().await;
            assert_ne!(
                reply.status,
                StatusCode::TOO_MANY_REQUESTS,
                "{method} {path}, attempt {attempt}"
            );
        }
        let limited = send().await;
        assert_eq!(
            limited.status,
            StatusCode::TOO_MANY_REQUESTS,
            "{method} {path}"
        );
        limited.error(ErrorCode::RateLimited);
        server.clock.advance(Duration::minutes(1));
    }
}

/// Finding 8: only the time in which the server saw that a person was absent counts.
#[tokio::test]
async fn an_outage_of_the_media_server_does_not_count_as_absence() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let seat = server.enter_call(ann, voice).await;
    server.enter_call(ben, voice).await;

    server.fake().leave(&seat.room, &seat.identity);
    assert_eq!(server.check().await, CLEAN);
    server.clock.advance(Duration::seconds(10));
    server.fake().set_unreachable(true);
    assert_eq!(server.check().await, Outcome::Unreachable);
    server.clock.advance(Duration::seconds(60));
    assert_eq!(server.check().await, Outcome::Unreachable);
    server.fake().set_unreachable(false);
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(
        server.occupants(ben, voice).await,
        [ann.account_id, ben.account_id]
    );
    server.clock.advance(Duration::seconds(19));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(server.occupants(ben, voice).await.len(), 2);
    server.clock.advance(Duration::seconds(1));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(server.occupants(ben, voice).await, [ben.account_id]);

    let ben_seat_room = seat.room.clone();
    let ben_identity = server.fake().identities(&ben_seat_room)[0].clone();
    server.fake().leave(&ben_seat_room, &ben_identity);
    server.fake().enter(
        &ben_seat_room,
        "EG_recording",
        ParticipantKind::Recorder,
        None,
    );
    assert_eq!(server.check().await, CLEAN);
    server.fake().fail_room(&ben_seat_room, true);
    server.clock.advance(Duration::seconds(60));
    server.check().await;
    server.fake().fail_room(&ben_seat_room, false);
    server.check().await;
    assert_eq!(server.occupants(ben, voice).await, [ben.account_id]);

    let restarted = server.restart().await;
    restarted.clock.advance(Duration::seconds(60));
    restarted.check().await;
    assert_eq!(restarted.occupants(ben, voice).await, [ben.account_id]);
    restarted.clock.advance(Duration::seconds(20));
    restarted.check().await;
    assert!(restarted.occupants(ben, voice).await.is_empty());
}

/// Finding 9: the set of speakers that the server sends follows the blocks, the own mute and
/// the people in the call, also while the reports do not change.
#[tokio::test]
async fn speaking_marks_follow_a_block_while_the_person_keeps_reporting() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server.enter_call(ann, voice).await;
    let mut to_ben = server.full_stream(ben).await;
    async fn step(
        server: &TestServer,
        speaker: &Member,
        viewer: &Member,
        stream: &mut crate::stream_support::Stream,
    ) -> Vec<Vec<AccountId>> {
        server.clock.advance(Duration::seconds(1));
        server
            .put(
                speaker,
                "/api/v1/call/speaking",
                &json!({ "speaking": true }),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        speaking::publish(&server.state).await.unwrap();
        server
            .call_events(viewer, stream)
            .await
            .into_iter()
            .filter_map(|event| match event {
                Event::CallSpeaking { account_ids, .. } => Some(account_ids),
                _ => None,
            })
            .collect()
    }
    let nobody: Vec<AccountId> = Vec::new();
    assert_eq!(
        step(server, ann, ben, &mut to_ben).await,
        [vec![ann.account_id]]
    );
    assert!(step(server, ann, ben, &mut to_ben).await.is_empty());

    let block = format!(
        "/api/v1/channels/{voice}/call/participants/{}/microphone-block",
        ann.account_id
    );
    server
        .put_empty(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        step(server, ann, ben, &mut to_ben).await,
        std::slice::from_ref(&nobody)
    );
    assert!(step(server, ann, ben, &mut to_ben).await.is_empty());
    server
        .delete(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        step(server, ann, ben, &mut to_ben).await,
        [vec![ann.account_id]]
    );

    let state = |muted: bool| json!({ "muted": muted, "deafened": false });
    server
        .put(ann, "/api/v1/call/state", &state(true))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        step(server, ann, ben, &mut to_ben).await,
        std::slice::from_ref(&nobody)
    );
    server
        .put(ann, "/api/v1/call/state", &state(false))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        step(server, ann, ben, &mut to_ben).await,
        [vec![ann.account_id]]
    );

    server.enter_call(owner, voice).await;
    server.leave_call(ann).await;
    server.clock.advance(Duration::seconds(1));
    speaking::publish(&server.state).await.unwrap();
    let after: Vec<Event> = server
        .call_events(ben, &mut to_ben)
        .await
        .into_iter()
        .filter(|event| matches!(event, Event::CallSpeaking { .. }))
        .collect();
    assert!(
        matches!(after.as_slice(), [Event::CallSpeaking { account_ids, .. }] if account_ids.is_empty()),
        "{after:?}"
    );
    let _ = WorkspacePermission::ViewChannel;
}
