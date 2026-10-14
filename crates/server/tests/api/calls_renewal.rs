//! A request for a token that names the seat that it renews (ADR 0520).

use junto_protocol::call::{CallJoin, CallMove, LeaveReason};
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::ChannelId;
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::Duration;

use crate::calls_support::{Scene, Seat};
use crate::devices::link_second_device;
use crate::support::{Member, Reply, TestServer, drive_until};

fn renewal_path(channel_id: ChannelId, identity: &str) -> String {
    format!("/api/v1/channels/{channel_id}/call/join?renews={identity}")
}

async fn renew(server: &TestServer, member: &Member, channel_id: ChannelId, seat: &str) -> Reply {
    server
        .post(member, &renewal_path(channel_id, seat), &Value::Null)
        .await
}

/// Asserts that `reply` refuses the renewal because its seat ended, and returns the reason
/// that the answer states.
#[track_caller]
fn seat_ended(reply: &Reply) -> Option<LeaveReason> {
    reply.error(ErrorCode::SeatEnded);
    let body: Value = serde_json::from_slice(reply.bytes()).unwrap();
    serde_json::from_value(body["error"]["left"].clone()).unwrap()
}

fn removal_path(scene: &Scene, member: &Member) -> String {
    format!(
        "/api/v1/channels/{}/call/participants/{}",
        scene.voice.id, member.account_id
    )
}

async fn seats_of(server: &TestServer, member: &Member) -> Vec<String> {
    sqlx::query_scalar("select identity from call_members where account_id = $1")
        .bind(member.account_id)
        .fetch_all(&server.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_renewal_that_waits_behind_a_removal_gives_no_seat() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    server.enter_call(owner, scene.voice.id).await;
    let seat = server.enter_call(ann, scene.voice.id).await;

    let removal = removal_path(&scene, ann);
    server.gate().arm(0);
    let mut removing = Box::pin(server.delete(owner, &removal));
    drive_until(&mut removing, server.gate().arrived()).await;
    let mut renewing = Box::pin(renew(server, ann, scene.voice.id, &seat.identity));
    drive_until(&mut renewing, server.until_blocked()).await;
    server.gate().release();
    removing.await.empty(StatusCode::NO_CONTENT);
    let refused = renewing.await;

    assert_eq!(seat_ended(&refused), Some(LeaveReason::Disconnected));
    assert!(seats_of(server, ann).await.is_empty());
    assert_eq!(server.own_call(ann).await.call, None);
    assert_eq!(
        server.occupants(owner, scene.voice.id).await,
        [owner.account_id]
    );
}

#[tokio::test]
async fn a_removal_that_waits_behind_a_renewal_ends_the_renewed_seat() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    server.enter_call(owner, scene.voice.id).await;
    let seat = server.enter_call(ann, scene.voice.id).await;
    let removal = removal_path(&scene, ann);

    server.gate().arm(0);
    let mut renewing = Box::pin(renew(server, ann, scene.voice.id, &seat.identity));
    drive_until(&mut renewing, server.gate().arrived()).await;
    let mut removing = Box::pin(server.delete(owner, &removal));
    drive_until(&mut removing, server.until_blocked()).await;
    server.gate().release();
    let renewed: CallJoin = renewing.await.ok(StatusCode::OK);
    removing.await.empty(StatusCode::NO_CONTENT);

    assert_eq!(renewed.identity, seat.identity);
    assert!(seats_of(server, ann).await.is_empty());
    assert_eq!(
        seat_ended(&renew(server, ann, scene.voice.id, &seat.identity).await),
        Some(LeaveReason::Disconnected)
    );
}

#[tokio::test]
async fn a_renewal_does_not_take_the_seat_of_another_device() {
    let scene = Scene::start().await;
    let (server, ann) = (&scene.server, &scene.ann);
    let desk = server.enter_call(ann, scene.voice.id).await;
    let laptop = link_second_device(server, ann).await;
    let moved = server.enter_call(&laptop, scene.voice.id).await;

    let refused = renew(server, ann, scene.voice.id, &desk.identity).await;

    assert_eq!(seat_ended(&refused), Some(LeaveReason::Switched));
    assert_eq!(
        seats_of(server, ann).await,
        std::slice::from_ref(&moved.identity)
    );
    let own = server.own_call(&laptop).await.call.unwrap().own;
    assert_eq!(
        (own.device_id, own.identity),
        (laptop.device_id, moved.identity)
    );
}

#[tokio::test]
async fn a_renewal_that_names_the_seat_of_another_device_does_not_take_it() {
    let scene = Scene::start().await;
    let (server, ann) = (&scene.server, &scene.ann);
    let laptop = link_second_device(server, ann).await;
    let other = server.enter_call(&laptop, scene.voice.id).await;

    let refused = renew(server, ann, scene.voice.id, &other.identity).await;

    assert_eq!(seat_ended(&refused), Some(LeaveReason::Switched));
    assert_eq!(seats_of(server, ann).await, [other.identity]);
}

#[tokio::test]
async fn a_renewal_of_a_seat_that_the_check_ended_states_the_lost_connection() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    server.enter_call(owner, scene.voice.id).await;
    let seat = server.enter_call(ann, scene.voice.id).await;
    server.fake().leave(&seat.room, &seat.identity);
    server.check().await;
    server.clock.advance(Duration::seconds(20));
    server.check().await;
    assert!(seats_of(server, ann).await.is_empty());

    let refused = renew(server, ann, scene.voice.id, &seat.identity).await;

    assert_eq!(seat_ended(&refused), Some(LeaveReason::ConnectionLost));
    assert!(seats_of(server, ann).await.is_empty());
    let fresh = server.join_call(ann, scene.voice.id).await;
    assert_ne!(fresh.identity, seat.identity);
}

#[tokio::test]
async fn a_renewal_of_a_seat_whose_token_time_ended_unused_makes_no_new_seat() {
    let scene = Scene::start().await;
    let (server, ann) = (&scene.server, &scene.ann);
    let join = server.join_call(ann, scene.voice.id).await;
    server.clock.advance(Duration::seconds(600));

    let refused = renew(server, ann, scene.voice.id, &join.identity).await;

    assert_eq!(seat_ended(&refused), Some(LeaveReason::NeverConnected));
    assert_eq!(
        seats_of(server, ann).await,
        std::slice::from_ref(&join.identity)
    );
    let fresh = server.join_call(ann, scene.voice.id).await;
    assert_ne!(fresh.identity, join.identity);
}

#[tokio::test]
async fn a_renewal_of_the_held_seat_gives_a_token_for_it() {
    let scene = Scene::start().await;
    let (server, ann) = (&scene.server, &scene.ann);
    let seat = server.enter_call(ann, scene.voice.id).await;

    let renewed: CallJoin = renew(server, ann, scene.voice.id, &seat.identity)
        .await
        .ok(StatusCode::OK);

    assert_eq!(renewed.identity, seat.identity);
    assert_eq!(renewed.call.id, seat.join.call.id);
    assert_eq!(
        seats_of(server, ann).await,
        std::slice::from_ref(&seat.identity)
    );
    server.connect(renewed);
}

#[tokio::test]
async fn a_renewal_of_an_unknown_seat_states_no_reason() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let other = server.enter_call(ben, scene.voice.id).await;
    let own_form = format!("{}:{}", ann.account_id, "0".repeat(32));

    for named in [own_form.as_str(), other.identity.as_str(), "anything"] {
        let refused = renew(server, ann, scene.voice.id, named).await;
        assert_eq!(seat_ended(&refused), None, "{named}");
    }
    assert!(seats_of(server, ann).await.is_empty());
    assert_eq!(seats_of(server, ben).await, [other.identity]);
}

#[tokio::test]
async fn a_renewal_in_another_place_does_not_move_the_seat() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let garden = server.voice(owner, scene.workspace_id, "garden").await;
    let seat = server.enter_call(ann, scene.voice.id).await;

    let refused = renew(server, ann, garden.id, &seat.identity).await;

    assert_eq!(seat_ended(&refused), Some(LeaveReason::Switched));
    assert_eq!(seats_of(server, ann).await, [seat.identity]);
    assert_eq!(server.count("calls").await, 1);
}

#[tokio::test]
async fn a_request_without_the_seat_after_a_removal_gives_a_new_seat_as_before() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    server.enter_call(owner, scene.voice.id).await;
    let seat = server.enter_call(ann, scene.voice.id).await;
    server
        .delete(owner, &removal_path(&scene, ann))
        .await
        .empty(StatusCode::NO_CONTENT);

    let again = server.join_call(ann, scene.voice.id).await;

    assert_ne!(again.identity, seat.identity);
    assert_eq!(seats_of(server, ann).await, [again.identity]);
}

#[tokio::test]
async fn a_renewal_of_a_direct_call_names_its_seat_too() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let direct = |identity: &str| {
        format!(
            "/api/v1/direct-calls/{}/join?renews={identity}",
            ben.account_id
        )
    };
    let seat: Seat = server.enter_direct(ann, ben).await;

    let renewed: CallJoin = server
        .post(ann, &direct(&seat.identity), &Value::Null)
        .await
        .ok(StatusCode::OK);
    assert_eq!(renewed.identity, seat.identity);

    server.leave_call(ann).await;
    assert_eq!(server.count("calls").await, 0);
    let refused = server
        .post(ann, &direct(&seat.identity), &Value::Null)
        .await;
    assert_eq!(seat_ended(&refused), Some(LeaveReason::Left));
    assert!(seats_of(server, ann).await.is_empty());
    assert_eq!(server.count("calls").await, 0);
}

#[tokio::test]
async fn a_renewal_of_a_seat_that_a_move_ended_states_the_move() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let garden = server.voice(owner, scene.workspace_id, "garden").await;
    let source = server.enter_call(ann, scene.voice.id).await;
    let moving = format!(
        "/api/v1/channels/{}/call/participants/{}/move",
        scene.voice.id, ann.account_id
    );
    let pending: CallMove = server
        .post(owner, &moving, &json!({ "to_channel_id": garden.id }))
        .await
        .ok(StatusCode::CREATED);
    let join: CallJoin = server
        .post(
            ann,
            &format!("/api/v1/call/moves/{}/join", pending.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    let destination = server.connect(join);
    server.check().await;
    assert_eq!(
        seats_of(server, ann).await,
        std::slice::from_ref(&destination.identity)
    );

    let refused = renew(server, ann, scene.voice.id, &source.identity).await;

    assert_eq!(seat_ended(&refused), Some(LeaveReason::Moved));
    assert_eq!(seats_of(server, ann).await, [destination.identity]);
    assert_eq!(server.count("calls").await, 1);
}
