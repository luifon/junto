//! The grant of meeting mode in the state of the person who got it (ADR 0521).

use junto_protocol::call::CallJoin;
use junto_protocol::ids::{AccountId, ChannelId};
use junto_server::media::{TrackKind, TrackSource};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::calls_support::{Scene, Seat};
use crate::devices::link_second_device;
use crate::support::{Member, TestServer};

fn meeting_path(channel_id: ChannelId) -> String {
    format!("/api/v1/channels/{channel_id}/call/meeting")
}

fn microphone_block(channel_id: ChannelId, account_id: AccountId) -> String {
    format!("/api/v1/channels/{channel_id}/call/participants/{account_id}/microphone-block")
}

async fn share(server: &TestServer, member: &Member, seat: &Seat) {
    server
        .put(
            member,
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
}

/// Ann presents in meeting mode; Ben listens with the block of the meeting.
async fn meeting(scene: &Scene) {
    let (server, ann) = (&scene.server, &scene.ann);
    let presenter = server.enter_call(ann, scene.voice.id).await;
    server.enter_call(&scene.ben, scene.voice.id).await;
    share(server, ann, &presenter).await;
    server
        .put_empty(ann, &meeting_path(scene.voice.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.check().await;
}

async fn grant_of(server: &TestServer, member: &Member) -> Option<bool> {
    server.own_call(member).await.call.unwrap().meeting_grant
}

async fn report(server: &TestServer, member: &Member, muted: bool) {
    server
        .put(
            member,
            "/api/v1/call/state",
            &json!({ "muted": muted, "deafened": false }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
}

async fn renewal(server: &TestServer, member: &Member, channel_id: ChannelId) -> CallJoin {
    let identity = server.own_call(member).await.call.unwrap().own.identity;
    server
        .post(
            member,
            &format!("/api/v1/channels/{channel_id}/call/join?renews={identity}"),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK)
}

#[tokio::test]
async fn a_grant_stays_in_the_own_state_after_the_meeting_until_the_person_opens_the_microphone() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    meeting(&scene).await;
    assert_eq!(grant_of(server, ben).await, Some(false));
    assert_eq!(renewal(server, ben, voice).await.meeting_grant, Some(false));

    server
        .delete(ann, &microphone_block(voice, ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(grant_of(server, ben).await, Some(true));
    assert_eq!(renewal(server, ben, voice).await.meeting_grant, Some(true));

    server
        .delete(ann, &meeting_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(grant_of(server, ben).await, Some(true));
    report(server, ben, false).await;
    assert_eq!(grant_of(server, ben).await, Some(true));
    report(server, ben, true).await;
    assert_eq!(grant_of(server, ben).await, Some(true));
    report(server, ben, false).await;
    assert_eq!(grant_of(server, ben).await, Some(false));
    assert_eq!(renewal(server, ben, voice).await.meeting_grant, Some(false));
}

/// A grant belongs to a seat (ADR 0521). A person who moves the call to another device
/// during the meeting gets a new seat: it gets the block of the meeting, as every entry
/// does (ADR 0512), and no grant. The seat on the first device ends.
#[tokio::test]
async fn a_switch_of_device_during_the_meeting_gives_a_seat_with_the_block_and_no_grant() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    meeting(&scene).await;
    server
        .delete(ann, &microphone_block(voice, ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(grant_of(server, ben).await, Some(true));
    let desk = server.own_call(ben).await.call.unwrap().own.identity;

    let laptop = link_second_device(server, ben).await;
    let moved = server.enter_call(&laptop, voice).await;

    let own = server.own_call(&laptop).await.call.unwrap();
    assert_eq!(
        (own.own.device_id, &own.own.identity),
        (laptop.device_id, &moved.identity)
    );
    assert_ne!(moved.identity, desk);
    assert_eq!(own.meeting_grant, Some(false));
    assert_eq!(
        renewal(server, &laptop, voice).await.meeting_grant,
        Some(false)
    );
    let reason: Option<String> = sqlx::query_scalar(
        "select reason from call_microphone_blocks b join calls c on c.id = b.call_id
         where c.channel_id = $1 and b.account_id = $2",
    )
    .bind(voice)
    .bind(ben.account_id)
    .fetch_optional(&server.db)
    .await
    .unwrap();
    assert_eq!(reason.as_deref(), Some("meeting"));
    let seats: Vec<String> =
        sqlx::query_scalar("select identity from call_members where account_id = $1")
            .bind(ben.account_id)
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert_eq!(seats, [moved.identity]);
}

#[tokio::test]
async fn a_new_block_withdraws_a_grant() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let block = microphone_block(voice, ben.account_id);
    meeting(&scene).await;

    server
        .delete(ann, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(grant_of(server, ben).await, Some(true));
    server
        .put_empty(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(grant_of(server, ben).await, Some(false));

    server
        .delete(ann, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(grant_of(server, ben).await, Some(true));
    let meeting = meeting_path(voice);
    server
        .delete(ann, &meeting)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(ann, &meeting)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(grant_of(server, ben).await, Some(false));
}

#[tokio::test]
async fn the_end_of_a_block_without_meeting_mode_of_another_person_is_no_grant() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server.enter_call(ben, voice).await;
    let ben_block = microphone_block(voice, ben.account_id);
    server
        .put_empty(owner, &ben_block)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(owner, &ben_block)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(grant_of(server, ben).await, Some(false));

    let presenter = server.enter_call(ann, voice).await;
    share(server, ann, &presenter).await;
    server
        .put_empty(ann, &meeting_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    let ann_block = microphone_block(voice, ann.account_id);
    server
        .put_empty(owner, &ann_block)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(owner, &ann_block)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(grant_of(server, ann).await, Some(false));
}

#[tokio::test]
async fn a_grant_is_told_to_nobody_but_the_person() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    meeting(&scene).await;
    server
        .delete(ann, &microphone_block(voice, ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(grant_of(server, ben).await, Some(true));

    assert_eq!(grant_of(server, ann).await, Some(false));
    for viewer in [ann, &scene.owner] {
        let seen: Value = server
            .get(viewer, &format!("/api/v1/channels/{voice}/call"))
            .await
            .ok(StatusCode::OK);
        assert!(!seen.to_string().contains("grant"), "{seen}");
    }
}

#[tokio::test]
async fn a_person_who_is_not_inside_a_call_that_hides_who_is_inside_is_told_no_grant() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let voice = scene.voice.id;
    server
        .call_settings(owner, voice, json!({ "hides_occupants": true }))
        .await;
    let presenter = server.enter_call(ann, voice).await;
    share(server, ann, &presenter).await;
    server
        .put_empty(ann, &meeting_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    let carl = server.join(owner, "Carl").await;
    let join = server.join_call(&carl, voice).await;
    assert_eq!(join.meeting_grant, None);
    server
        .delete(ann, &microphone_block(voice, carl.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);

    assert_eq!(grant_of(server, &carl).await, None);
    assert_eq!(renewal(server, &carl, voice).await.meeting_grant, None);
    server.connect(join);
    server.check().await;
    assert_eq!(grant_of(server, &carl).await, Some(true));
}
