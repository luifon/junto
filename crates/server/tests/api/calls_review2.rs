//! Tests for the defects that the second review of the call unit found.

use std::time::{Duration as Wait, Instant};

use junto_protocol::ServerPermission;
use junto_protocol::call::{
    CallInvitation, CallJoin, CallMove, EntryRequest, InvitationOutcome, MeetingEnd,
};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::{AccountId, ChannelId};
use junto_protocol::structure::Workspace;
use junto_server::calls::admission::{self, Outcome};
use junto_server::config::RateLimit;
use junto_server::media::{ParticipantKind, TrackKind, TrackSource};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use time::Duration;

use crate::calls_support::{Scene, names};
use crate::devices::link_second_device;
use crate::stream_support::Stream;
use crate::support::{DeviceKey, Member, Reply, TestServer, member};

fn removed(count: usize) -> Outcome {
    Outcome::Checked {
        removed: count,
        updated: 0,
    }
}

fn sorted(events: &[Event]) -> Vec<&'static str> {
    let mut names = names(events);
    names.sort_unstable();
    names
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

fn direct(account_id: AccountId) -> String {
    format!("/api/v1/direct-calls/{account_id}/join")
}

pub async fn ring(server: &TestServer, caller: &Member, callee: &Member) -> Reply {
    server
        .post(caller, &direct(callee.account_id), &Value::Null)
        .await
}

pub async fn invite(
    server: &TestServer,
    inviter: &Member,
    channel_id: ChannelId,
    invited: &Member,
) -> CallInvitation {
    server
        .post(
            inviter,
            &format!("/api/v1/channels/{channel_id}/call/invitations"),
            &json!({ "account_id": invited.account_id }),
        )
        .await
        .ok(StatusCode::CREATED)
}

pub async fn move_to(
    server: &TestServer,
    mover: &Member,
    from: ChannelId,
    account_id: AccountId,
    to: ChannelId,
) -> CallMove {
    let path = format!("/api/v1/channels/{from}/call/participants/{account_id}/move");
    server
        .post(mover, &path, &json!({ "to_channel_id": to }))
        .await
        .ok(StatusCode::CREATED)
}

/// The event `call_access_ended` as the stream carried it last: it names nothing.
fn access_ended_frame(stream: &Stream) -> Value {
    let frame = stream
        .raw
        .iter()
        .rev()
        .find(|frame| frame.contains("call_access_ended"))
        .expect("a frame with call_access_ended");
    serde_json::from_str::<Value>(frame).unwrap()["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["event"].clone())
        .find(|event| event["type"] == "call_access_ended")
        .unwrap()
}

/// Finding B1: two accounts have a direct call only while they share a workspace or one of
/// them is an administrator. The rule holds for the join, for every event of the call when
/// it is delivered, and for the snapshot. Seeing a profile is not enough.
#[tokio::test]
async fn a_direct_call_needs_a_shared_workspace_or_an_administrator_at_every_step() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let second: Workspace = server
        .post(owner, "/api/v1/workspaces", &json!({ "name": "Second" }))
        .await
        .ok(StatusCode::CREATED);
    let carl = server.join_into(owner, "Carl", &[second.id]).await;
    let profile = format!("/api/v1/accounts/{}/profile", carl.account_id);

    server.get(ann, &profile).await.error(ErrorCode::NotFound);
    ring(server, ann, &carl).await.error(ErrorCode::NotFound);
    server
        .grant_server(owner, ann, &[ServerPermission::BanMembers])
        .await;
    server.get(ann, &profile).await.ok::<Value>(StatusCode::OK);
    ring(server, ann, &carl).await.error(ErrorCode::NotFound);
    ring(server, &carl, ann).await.error(ErrorCode::NotFound);
    assert_eq!(server.count("calls").await, 0);

    let administrator = |role| format!("/api/v1/accounts/{}/server-roles/{role}", carl.account_id);
    let role = server
        .grant_server(owner, &carl, &[ServerPermission::Administrator])
        .await;
    let mut to_carl = server.subscribed(&carl, Level::Background).await;
    let mut to_ann = server.full_stream(ann).await;
    server.state.streams.hold();
    ring(server, ann, &carl)
        .await
        .ok::<CallJoin>(StatusCode::OK);
    server
        .delete(owner, &administrator(role))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.state.streams.release();
    let told = server.call_events(&carl, &mut to_carl).await;
    assert!(told.is_empty(), "{told:?}");
    let raw = to_carl.raw.join("\n");
    assert!(!raw.contains(&ann.account_id.to_string()), "{raw}");
    assert!(server.snapshot(&carl).await.call_invitations.is_empty());

    server.call_events(ann, &mut to_ann).await;
    server.check().await;
    assert_eq!(server.count("calls").await, 0);
    assert_eq!(
        names(&server.call_events(&carl, &mut to_carl).await),
        ["call_access_ended"]
    );
    assert_eq!(
        access_ended_frame(&to_carl),
        json!({ "type": "call_access_ended" })
    );
    assert_eq!(
        sorted(&server.call_events(ann, &mut to_ann).await),
        ["call_access_ended", "own_call_changed"]
    );

    let role = server
        .grant_server(owner, &carl, &[ServerPermission::Administrator])
        .await;
    server.enter_direct(ann, &carl).await;
    let mut to_carl = server.full_stream(&carl).await;
    assert_eq!(server.snapshot(&carl).await.direct_calls.len(), 1);
    server.fake().set_unreachable(true);
    assert_eq!(server.check().await, Outcome::Unreachable);
    server
        .delete(owner, &administrator(role))
        .await
        .empty(StatusCode::NO_CONTENT);
    for account in [&carl, ann] {
        let snapshot = server.snapshot(account).await;
        assert!(snapshot.direct_calls.is_empty());
        assert!(snapshot.call_invitations.is_empty());
        assert!(snapshot.own_call.is_none());
    }
    server.call_events(&carl, &mut to_carl).await;
    let frames = to_carl.raw.len();
    server.state.streams.hold();
    server.leave_call(ann).await;
    server.state.streams.release();
    let told = server.call_events(&carl, &mut to_carl).await;
    assert!(told.is_empty(), "{told:?}");
    let raw = to_carl.raw[frames..].join("\n");
    assert!(!raw.contains(&ann.account_id.to_string()), "{raw}");
}

/// Finding B2: an account that loses a channel learns nothing more about what it waited for
/// there. It gets one event that names nothing, and no outcome.
#[tokio::test]
async fn an_account_that_lost_a_channel_gets_one_event_without_names_and_no_outcome() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let role = server
        .grant_workspace(owner, ann, scene.workspace_id, &[])
        .await;
    let role_of = |account: &Member| {
        format!(
            "/api/v1/workspaces/{}/members/{}/roles/{role}",
            scene.workspace_id, account.account_id
        )
    };
    server
        .put_empty(owner, &role_of(ben))
        .await
        .empty(StatusCode::NO_CONTENT);
    let board = server.voice(owner, scene.workspace_id, "board").await.id;
    server
        .make_private(owner, scene.workspace_id, board, role)
        .await;

    server.enter_call(ann, board).await;
    let mut to_ann = server.full_stream(ann).await;
    let invitation = invite(server, ann, board, ben).await;
    server.call_events(ann, &mut to_ann).await;
    server
        .delete(owner, &role_of(ann))
        .await
        .empty(StatusCode::NO_CONTENT);
    let acceptance = format!("/api/v1/call/invitations/{}/acceptance", invitation.id);
    let join: CallJoin = server
        .post(ben, &acceptance, &Value::Null)
        .await
        .ok(StatusCode::OK);
    let told = server.call_events(ann, &mut to_ann).await;
    assert!(told.is_empty(), "{told:?}");
    server.connect(join);
    server.check().await;
    assert_eq!(
        names(&server.call_events(ann, &mut to_ann).await),
        ["own_call_changed"]
    );

    server
        .put_empty(owner, &role_of(ann))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(ben, &format!("/api/v1/channels/{board}/call/lock"))
        .await
        .empty(StatusCode::NO_CONTENT);
    let _request: EntryRequest = server
        .post(
            ann,
            &format!("/api/v1/channels/{board}/call/entry-requests"),
            &Value::Null,
        )
        .await
        .ok(StatusCode::CREATED);
    invite(server, ben, board, ann).await;
    let mut to_ben = server.full_stream(ben).await;
    server.call_events(ann, &mut to_ann).await;
    server
        .delete(owner, &role_of(ann))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.check().await;
    assert_eq!(
        names(&server.call_events(ann, &mut to_ann).await),
        ["call_access_ended"]
    );
    assert_eq!(
        access_ended_frame(&to_ann),
        json!({ "type": "call_access_ended" })
    );
    assert_eq!(server.count("call_invitations").await, 0);
    assert_eq!(server.count("call_entry_requests").await, 0);
    assert_eq!(
        sorted(&server.call_events(ben, &mut to_ben).await),
        ["call_entry_resolved", "call_invitation_resolved"]
    );

    server.clock.advance(Duration::seconds(61));
    server.check().await;
    let told = server.call_events(ann, &mut to_ann).await;
    assert!(told.is_empty(), "{told:?}");
}

async fn rooms_with_one_person_each(server: &TestServer, count: usize) -> Vec<String> {
    let owner = server.owner().await;
    let workspace_id = server.default_workspace(&owner).await.id;
    let mut rooms = Vec::new();
    for index in 0..count {
        let person = server.join(&owner, &format!("Person {index}")).await;
        let name = format!("room-{index}");
        let voice = server.voice(&owner, workspace_id, &name).await.id;
        let seat = server.connect(server.join_call(&person, voice).await);
        rooms.push(seat.room);
    }
    rooms.sort();
    rooms
}

/// Finding B3: one check looks into a bounded number of rooms and takes the rooms in turn,
/// the ones with a call and the ones without. A room that keeps failing does not take the
/// turn of another room.
#[tokio::test]
async fn the_full_check_takes_the_rooms_in_turn_and_in_parts() {
    let server = TestServer::start_for_calls_with(|config| {
        config.limits.calls.rooms_per_check = 2;
        config.limits.calls.most_ended_rooms_per_check = 1;
    })
    .await;
    let rooms = rooms_with_one_person_each(&server, 5).await;
    for _ in 0..3 {
        server.check().await;
    }
    assert_eq!(
        server
            .count("call_members where connected_at is not null")
            .await,
        5
    );

    for room in &rooms {
        server
            .fake()
            .enter(room, "intruder", ParticipantKind::Standard, None);
    }
    let mut found = Vec::new();
    for _ in 0..3 {
        found.push(server.check().await);
    }
    assert_eq!(found, [removed(2), removed(2), removed(1)]);

    let namespace = server.state.calls.namespace().to_owned();
    let ended: Vec<String> = (1..=3)
        .map(|number| format!("jr-{namespace}-{number:032x}"))
        .collect();
    for (number, room) in ended.iter().enumerate() {
        let ghost = format!("ghost-{number}");
        server
            .fake()
            .enter(room, &ghost, ParticipantKind::Standard, None);
    }
    server.fake().fail_removal_of("ghost-0", true);
    for _ in 0..3 {
        server.check().await;
    }
    assert_eq!(server.fake().identities(&ended[0]), ["ghost-0"]);
    assert!(server.fake().identities(&ended[1]).is_empty());
    assert!(server.fake().identities(&ended[2]).is_empty());
}

/// Finding B3: one check asks the media server about a bounded number of rooms at one time.
#[tokio::test]
async fn one_check_looks_into_a_bounded_number_of_rooms_at_one_time() {
    let server = TestServer::start_for_calls_with(|config| {
        config.limits.calls.concurrent_room_checks = 3;
    })
    .await;
    rooms_with_one_person_each(&server, 6).await;
    server.check().await;
    server.fake().set_delay(Some(Wait::from_millis(300)));
    server.check().await;
    let most = server.fake().most_concurrent();
    assert!((2..=3).contains(&most), "{most} requests at one time");
}

/// Finding B4: the time of absence counts only between two looks at the room itself. A
/// check that reached the media server and could not read the record saw nothing, and a
/// removal that fails does not stop the clock of a room that was seen.
#[tokio::test]
async fn only_absence_that_the_server_saw_of_a_room_counts() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    let members = "call_members where account_id = $1";
    let in_a_call = async |server: &TestServer, account: &Member| -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "select count(*) from {members}"
        )))
        .bind(account.account_id.as_uuid())
        .fetch_one(&server.db)
        .await
        .unwrap()
    };

    let seat = server.enter_call(ben, garden).await;
    let strict = server
        .strict(|config| config.limits.database.lock = Duration::milliseconds(300))
        .await;
    strict.check().await;
    server.fake().leave(&seat.room, &seat.identity);
    strict.check().await;
    server.fake().set_unreachable(true);
    strict.clock.advance(Duration::minutes(10));
    assert_eq!(strict.check().await, Outcome::Unreachable);
    server.fake().set_unreachable(false);
    let mut lock = strict.db.begin().await.unwrap();
    sqlx::query("lock table calls in access exclusive mode")
        .execute(&mut *lock)
        .await
        .unwrap();
    assert!(admission::check(&strict.state).await.is_err());
    lock.rollback().await.unwrap();
    strict.check().await;
    assert_eq!(in_a_call(&strict, ben).await, 1);
    strict.clock.advance(Duration::seconds(21));
    strict.check().await;
    assert_eq!(in_a_call(&strict, ben).await, 0);

    let seat = server.enter_call(ann, scene.voice.id).await;
    server
        .fake()
        .enter(&seat.room, "stuck", ParticipantKind::Standard, None);
    server.fake().fail_removal_of("stuck", true);
    server.fake().leave(&seat.room, &seat.identity);
    server.check().await;
    for _ in 0..3 {
        server.clock.advance(Duration::seconds(8));
        server.check().await;
    }
    assert_eq!(in_a_call(server, ann).await, 0);
}

/// The invitations among `events` that ring `account`.
fn rings_heard(events: &[Event], account: &Member) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(event, Event::CallInvited { invitation }
                if invitation.account_id == account.account_id)
        })
        .count()
}

/// Finding B5: an account does not ring another account again and again. A declined ring
/// is followed by a pause, the rings of one account to another are limited whatever session
/// sends them, and a muted conversation is not rung.
#[tokio::test]
async fn a_direct_call_does_not_ring_again_and_again() {
    let server = TestServer::start_for_calls_with(|config| {
        config.limits.calls.rings = RateLimit {
            attempts: 3,
            window: Duration::minutes(10),
        };
    })
    .await;
    let scene = Scene::on(server).await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let mut to_ben = server.subscribed(ben, Level::Background).await;
    let refusal = format!("/api/v1/direct-calls/{}/refusal", ann.account_id);

    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    server
        .post(ben, &refusal, &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.leave_call(ann).await;
    assert_eq!(
        rings_heard(&server.call_events(ben, &mut to_ben).await, ben),
        1
    );

    let limited = ring(server, ann, ben).await;
    limited.error(ErrorCode::RateLimited);
    assert!(limited.retry_after.is_some());
    let session = server.login(&ann.key).await;
    let again = member(DeviceKey::generate(), session);
    ring(server, &again, ben)
        .await
        .error(ErrorCode::RateLimited);
    assert_eq!(
        rings_heard(&server.call_events(ben, &mut to_ben).await, ben),
        0
    );
    assert_eq!(server.count("calls").await, 0);

    server.clock.advance(Duration::minutes(5));
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    server.leave_call(ann).await;
    ring(server, &again, ben)
        .await
        .ok::<CallJoin>(StatusCode::OK);
    server.leave_call(&again).await;
    ring(server, ann, ben).await.error(ErrorCode::RateLimited);
    assert_eq!(
        rings_heard(&server.call_events(ben, &mut to_ben).await, ben),
        2
    );
    ring(server, ben, ann).await.ok::<CallJoin>(StatusCode::OK);
    server.leave_call(ben).await;

    server.clock.advance(Duration::minutes(10));
    let mute = format!("/api/v1/direct-messages/{}/mute", ann.account_id);
    server
        .put(ben, &mute, &json!({}))
        .await
        .empty(StatusCode::NO_CONTENT);
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    // The ring for a muted conversation is stored for the caller and silent for the
    // account that muted (ADR 0515).
    let silent: Vec<bool> = sqlx::query_scalar("select silent from call_invitations")
        .fetch_all(&server.db)
        .await
        .unwrap();
    assert_eq!(silent, [true]);
    server.leave_call(ann).await;
    assert_eq!(
        rings_heard(&server.call_events(ben, &mut to_ben).await, ben),
        0
    );

    let until = server.clock.now() + Duration::hours(1);
    let until = until.format(&time::format_description::well_known::Rfc3339);
    server
        .put(ben, &mute, &json!({ "until": until.unwrap() }))
        .await
        .empty(StatusCode::NO_CONTENT);
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    server.leave_call(ann).await;
    assert_eq!(
        rings_heard(&server.call_events(ben, &mut to_ben).await, ben),
        0
    );
    server.clock.advance(Duration::hours(1));
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    server.leave_call(ann).await;
    assert_eq!(
        rings_heard(&server.call_events(ben, &mut to_ben).await, ben),
        1
    );

    server
        .put(ben, &mute, &json!({}))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(ben, &mute)
        .await
        .empty(StatusCode::NO_CONTENT);
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    assert_eq!(
        rings_heard(&server.call_events(ben, &mut to_ben).await, ben),
        1
    );

    for _ in 0..4 {
        ring(server, ben, ann).await.ok::<CallJoin>(StatusCode::OK);
        server.leave_call(ann).await;
        server.leave_call(ben).await;
        ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    }
}

/// Finding B6: a move is reported as failed only by the session that holds the place in the
/// source. Another session of the account cannot end the move.
#[tokio::test]
async fn only_the_session_in_the_call_reports_that_a_move_failed() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    server.enter_call(ann, scene.voice.id).await;
    let other = link_second_device(server, ann).await;

    let pending = move_to(server, owner, scene.voice.id, ann.account_id, garden).await;
    let failure = format!("/api/v1/call/moves/{}/failure", pending.id);
    server
        .post(&other, &failure, &Value::Null)
        .await
        .error(ErrorCode::NotInCall);
    assert_eq!(server.count("call_moves").await, 1);
    server
        .post(ann, &failure, &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.count("call_moves").await, 0);
}

/// Finding B7: an invitation that another one replaces ends with an event; the person who
/// invites and the person who moves get the event of what they started; and a meeting that
/// ends because the last person left is told and noted before the call ends.
#[tokio::test]
async fn a_replaced_invitation_a_new_move_and_a_meeting_that_ends_with_its_call_are_told() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    let seat = server.enter_call(ann, lounge).await;
    server.enter_call(owner, lounge).await;
    let mut to_ann = server.full_stream(ann).await;
    let mut to_ben = server.subscribed(ben, Level::Background).await;
    let mut to_owner = server.full_stream(owner).await;

    let first = invite(server, ann, lounge, ben).await;
    let second = invite(server, owner, lounge, ben).await;
    assert_eq!(
        server.call_events(ben, &mut to_ben).await,
        [
            Event::CallInvited {
                invitation: first.clone()
            },
            Event::CallInvitationResolved {
                invitation_id: first.id,
                outcome: InvitationOutcome::Cancelled,
            },
            Event::CallInvited {
                invitation: second.clone()
            },
        ]
    );
    assert_eq!(
        server.call_events(ann, &mut to_ann).await,
        [
            Event::CallInvitationResolved {
                invitation_id: first.id,
                outcome: InvitationOutcome::Cancelled,
            },
            Event::CallInvited { invitation: first },
        ]
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
    );
    assert_eq!(
        server.call_events(owner, &mut to_owner).await,
        [Event::CallInvited { invitation: second }]
    );

    let pending = move_to(server, owner, lounge, ann.account_id, garden).await;
    assert_eq!(
        server.call_events(owner, &mut to_owner).await,
        [Event::CallMoveRequested {
            call_move: pending.clone()
        }]
    );
    let failure = format!("/api/v1/call/moves/{}/failure", pending.id);
    server
        .post(ann, &failure, &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.leave_call(owner).await;
    server.check().await;

    server
        .put(
            ann,
            "/api/v1/call/screen-share",
            &json!({ "audio": "none" }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(server.fake().publish(
        &seat.room,
        &seat.identity,
        TrackKind::Video,
        TrackSource::ScreenShare
    ));
    server.check().await;
    server
        .put_empty(ann, &format!("/api/v1/channels/{lounge}/call/meeting"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.call_events(owner, &mut to_owner).await;
    server.leave_call(ann).await;
    let told = server.call_events(owner, &mut to_owner).await;
    assert_eq!(
        names(&told),
        [
            "call_participant_left",
            "call_meeting_ended",
            "call_invitation_resolved",
            "call_ended"
        ]
    );
    assert!(matches!(
        told[1],
        Event::CallMeetingEnded {
            reason: MeetingEnd::PresenterLeft,
            ..
        }
    ));
    let notes: Vec<String> =
        sqlx::query_scalar("select kind from call_notes order by occurred_at, id")
            .fetch_all(&server.db)
            .await
            .unwrap();
    let last: Vec<&str> = notes.iter().rev().take(2).map(String::as_str).collect();
    assert_eq!(last, ["call_ended", "meeting_ended"]);
}

async fn notify(server: &TestServer, authorization: &str, body: &str) -> Reply {
    let request = reqwest::Client::new()
        .request(
            Method::POST,
            format!("{}/internal/media/notifications", server.base),
        )
        .header("content-type", "application/webhook+json")
        .header("authorization", authorization)
        .body(body.to_owned());
    TestServer::reply(request).await
}

/// Finding B8: a notification of the media server starts a check once, also when it is sent
/// again while its signature is valid, and the notifications that start a check are limited.
#[tokio::test]
async fn a_notification_of_the_media_server_is_processed_once_and_their_number_is_limited() {
    let server = TestServer::start_with_tasks(|config| {
        config.limits.calls.check_interval = Duration::hours(1);
        config.limits.calls.notifications = RateLimit {
            attempts: 4,
            window: Duration::minutes(1),
        };
    })
    .await;
    let scene = Scene::on(server).await;
    let (server, ann) = (&scene.server, &scene.ann);
    until("the first check has run", async || {
        server.state.calls.reachable() == Some(true)
    })
    .await;
    let seat = server.connect(server.join_call(ann, scene.voice.id).await);
    server.settled().await;
    let about = |number: u32| {
        let body = format!(
            r#"{{"event":"participant_joined","id":"EV_{number}","room":{{"name":"{}"}}}}"#,
            seat.room
        );
        let signed = server.fake().sign_notification(body.as_bytes());
        (signed, body)
    };
    let intrude = || {
        server
            .fake()
            .enter(&seat.room, "intruder", ParticipantKind::Standard, None);
    };

    let (signed, body) = about(1);
    intrude();
    notify(server, &signed, &body)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.settled().await;
    assert_eq!(server.fake().identities(&seat.room).len(), 1);

    intrude();
    let before = server.fake().requests();
    for spelling in [
        signed.clone(),
        format!("Bearer {signed}"),
        format!("Bearer   {signed}"),
    ] {
        notify(server, &spelling, &body)
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    server.settled().await;
    assert_eq!(server.fake().requests(), before);
    assert_eq!(server.fake().identities(&seat.room).len(), 2);

    for number in 2..=4 {
        let (signed, body) = about(number);
        notify(server, &signed, &body)
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    server.settled().await;
    assert_eq!(server.fake().identities(&seat.room).len(), 1);
    let (signed, body) = about(5);
    let limited = notify(server, &signed, &body).await;
    limited.error(ErrorCode::RateLimited);
    assert!(limited.retry_after.is_some());
}
