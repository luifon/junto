//! The system lines of a call and of a channel that the text chat shows (ADR 0408): the end of
//! a share, the lock of the room, the answer to a request to enter, the end of meeting mode
//! with its reason, the end of a call with its length, a pin, a rename, and the join of a
//! person who returns after a removal or a move.

use junto_protocol::ids::{AccountId, ChannelId};
use junto_protocol::message::{Message, MessagePage};
use junto_server::media::{TrackKind, TrackSource};
use reqwest::StatusCode;
use serde_json::json;
use time::Duration;

use crate::calls_support::Scene;
use crate::support::{Member, TestServer};

/// The action, the actor and the subject of each line of a channel after a position.
async fn lines_after(
    server: &TestServer,
    member: &Member,
    channel_id: ChannelId,
    position: u64,
) -> Vec<(String, Option<AccountId>, Option<AccountId>)> {
    let page: MessagePage = server
        .get(
            member,
            &format!("/api/v1/channels/{channel_id}/messages?after={position}"),
        )
        .await
        .ok(StatusCode::OK);
    page.messages
        .iter()
        .filter_map(|message| message.system.as_ref())
        .map(|line| (line.action.clone(), line.actor_id, line.subject_id))
        .collect()
}

/// The position of the latest message of a channel.
async fn latest(server: &TestServer, member: &Member, channel_id: ChannelId) -> u64 {
    let page: MessagePage = server
        .get(member, &format!("/api/v1/channels/{channel_id}/messages"))
        .await
        .ok(StatusCode::OK);
    page.messages.last().map_or(0, |message| message.position)
}

fn line(
    action: &str,
    actor: Option<AccountId>,
    subject: Option<AccountId>,
) -> (String, Option<AccountId>, Option<AccountId>) {
    (action.to_owned(), actor, subject)
}

#[tokio::test]
async fn a_share_a_lock_an_answer_to_a_request_and_the_end_of_a_call_have_their_lines() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let cara = server.join(owner, "Cara").await;
    let start = latest(server, ann, voice).await;
    server.enter_call(ann, voice).await;
    server.enter_call(ben, voice).await;
    server
        .put(
            ben,
            "/api/v1/call/screen-share",
            &json!({ "audio": "none" }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(ben, "/api/v1/call/screen-share")
        .await
        .empty(StatusCode::NO_CONTENT);
    let lock = format!("/api/v1/channels/{voice}/call/lock");
    server
        .put_empty(ann, &lock)
        .await
        .empty(StatusCode::NO_CONTENT);
    let ask = format!("/api/v1/channels/{voice}/call/entry-requests");
    for accept in [false, true] {
        let request: junto_protocol::call::EntryRequest = server
            .post(&cara, &ask, &serde_json::Value::Null)
            .await
            .ok(StatusCode::CREATED);
        server
            .post(
                ann,
                &format!("/api/v1/call/entry-requests/{}/answer", request.id),
                &json!({ "accept": accept }),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    server
        .delete(ann, &lock)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.clock.advance(Duration::seconds(125));
    server.leave_call(ben).await;
    server.leave_call(ann).await;
    let (ann_id, ben_id, cara_id) = (
        Some(ann.account_id),
        Some(ben.account_id),
        Some(cara.account_id),
    );
    assert_eq!(
        lines_after(server, ann, voice, start).await,
        [
            line("call_started", ann_id, None),
            line("person_joined", ben_id, None),
            line("screen_share_started", ben_id, None),
            line("screen_share_ended", ben_id, None),
            line("call_locked", ann_id, None),
            line("entry_requested", cara_id, None),
            line("entry_denied", ann_id, cara_id),
            line("entry_requested", cara_id, None),
            line("entry_accepted", ann_id, cara_id),
            line("call_unlocked", ann_id, None),
            line("person_left", ben_id, None),
            line("call_ended", None, None),
        ]
    );
    // The end of the call states its length, from the first person in the room.
    let page: MessagePage = server
        .get(
            ann,
            &format!("/api/v1/channels/{voice}/messages?after={start}"),
        )
        .await
        .ok(StatusCode::OK);
    let ended = page
        .messages
        .iter()
        .filter_map(|message| message.system.as_ref())
        .find(|line| line.action == "call_ended")
        .unwrap();
    assert!(
        ended.duration_seconds.is_some_and(|seconds| seconds >= 125),
        "{ended:?}"
    );
}

#[tokio::test]
async fn the_end_of_meeting_mode_states_who_ended_it_or_why() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let start = latest(server, ann, voice).await;
    let presenter = server.enter_call(ann, voice).await;
    server.enter_call(ben, voice).await;
    let meeting = format!("/api/v1/channels/{voice}/call/meeting");
    let share = |audio: &'static str| json!({ "audio": audio });
    let start_meeting = async || {
        server
            .put(ann, "/api/v1/call/screen-share", &share("none"))
            .await
            .empty(StatusCode::NO_CONTENT);
        assert!(server.fake().publish(
            &presenter.room,
            &presenter.identity,
            TrackKind::Video,
            TrackSource::ScreenShare
        ));
        server.check().await;
        server
            .put_empty(ann, &meeting)
            .await
            .empty(StatusCode::NO_CONTENT);
    };
    // The presenter turns it off; then the share of the presenter ends it.
    start_meeting().await;
    server
        .delete(ann, &meeting)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(ann, "/api/v1/call/screen-share")
        .await
        .empty(StatusCode::NO_CONTENT);
    start_meeting().await;
    server
        .delete(ann, "/api/v1/call/screen-share")
        .await
        .empty(StatusCode::NO_CONTENT);
    let actions: Vec<(String, Option<AccountId>)> = lines_after(server, ann, voice, start)
        .await
        .into_iter()
        .filter(|(action, _, _)| action.starts_with("meeting"))
        .map(|(action, actor, _)| (action, actor))
        .collect();
    let ann_id = Some(ann.account_id);
    assert_eq!(
        actions,
        [
            ("meeting_started".to_owned(), ann_id),
            ("meeting_turned_off".to_owned(), ann_id),
            ("meeting_started".to_owned(), ann_id),
            ("meeting_ended_share_stopped".to_owned(), ann_id),
        ]
    );
}

/// A person whom a moderator removed, and who enters the same running call again, has a line
/// of the join (open item RP2: the latest note of the person said the person was inside).
#[tokio::test]
async fn a_person_who_returns_after_a_removal_has_a_line_of_the_join() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let start = latest(server, ann, voice).await;
    server.enter_call(ann, voice).await;
    server.enter_call(ben, voice).await;
    server
        .delete(
            owner,
            &format!(
                "/api/v1/channels/{voice}/call/participants/{}",
                ben.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server.enter_call(ben, voice).await;
    let (owner_id, ben_id) = (Some(owner.account_id), Some(ben.account_id));
    let lines = lines_after(server, ann, voice, start).await;
    assert_eq!(
        lines[1..],
        [
            line("person_joined", ben_id, None),
            line("call_member_disconnected", owner_id, ben_id),
            line("person_joined", ben_id, None),
        ]
    );
}

#[tokio::test]
async fn a_pin_and_a_rename_have_lines_and_renames_in_a_row_are_one_line() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace_id = server.default_workspace(&owner).await.id;
    let channel = server.channel(&owner, workspace_id, "notes").await;
    let start = latest(&server, &owner, channel.id).await;
    let message: Message = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &json!({ "document": { "blocks": [{ "kind": "paragraph", "content": [
                { "kind": "text", "text": "keep this", "marks": [] }
            ] }] }, "reply_to": null, "attachments": [] }),
        )
        .await
        .ok(StatusCode::CREATED);
    for _ in 0..2 {
        server
            .put_empty(&owner, &format!("/api/v1/messages/{}/pin", message.id))
            .await
            .empty(StatusCode::NO_CONTENT);
        server
            .delete(&owner, &format!("/api/v1/messages/{}/pin", message.id))
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let path = format!("/api/v1/channels/{}", channel.id);
    for name in ["plans", "roadmap"] {
        server
            .patch(&owner, &path, &json!({ "name": name }))
            .await
            .ok::<junto_protocol::structure::Channel>(StatusCode::OK);
    }
    // A move in the list makes no line.
    server
        .patch(&owner, &path, &json!({ "position": 3 }))
        .await
        .ok::<junto_protocol::structure::Channel>(StatusCode::OK);
    let owner_id = Some(owner.account_id);
    assert_eq!(
        lines_after(&server, &owner, channel.id, start).await,
        [
            line("message_pinned", owner_id, None),
            line("channel_renamed", owner_id, None),
        ]
    );
    let page: MessagePage = server
        .get(&owner, &format!("/api/v1/channels/{}/messages", channel.id))
        .await
        .ok(StatusCode::OK);
    let renamed = page
        .messages
        .iter()
        .filter_map(|message| message.system.as_ref())
        .find(|line| line.action == "channel_renamed")
        .unwrap();
    assert_eq!(
        (renamed.previous_name.as_deref(), renamed.name.as_deref()),
        (Some("notes"), Some("roadmap"))
    );
    // A later rename, after the joining time, has a line of its own.
    server.clock.advance(Duration::minutes(6));
    server
        .patch(&owner, &path, &json!({ "name": "archive" }))
        .await
        .ok::<junto_protocol::structure::Channel>(StatusCode::OK);
    assert_eq!(
        lines_after(&server, &owner, channel.id, start).await.len(),
        3
    );
}

/// A device that runs several shares ends each with its slot: each start and each end has
/// its line (ADR 0510).
#[tokio::test]
async fn each_share_that_ends_by_its_slot_has_its_line() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let start = latest(server, ann, voice).await;
    server.enter_call(ann, voice).await;
    server.enter_call(ben, voice).await;
    for slot in [0, 1] {
        let report = json!({
            "audio": "none",
            "slot": slot,
            "kind": "screen",
            "share_id": uuid::Uuid::now_v7(),
        });
        server
            .put(ben, "/api/v1/call/screen-share", &report)
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    for slot in [0, 1] {
        server
            .delete(ben, &format!("/api/v1/call/screen-share?slot={slot}"))
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let ben_id = Some(ben.account_id);
    assert_eq!(
        lines_after(server, ann, voice, start).await,
        [
            line("call_started", Some(ann.account_id), None),
            line("person_joined", ben_id, None),
            line("screen_share_started", ben_id, None),
            line("screen_share_started", ben_id, None),
            line("screen_share_ended", ben_id, None),
            line("screen_share_ended", ben_id, None),
        ]
    );
}
