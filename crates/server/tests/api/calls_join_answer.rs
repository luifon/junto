//! What the answer to a join states about the caller, and the leave that names the identity
//! of its place (ADR 0503, ADR 0510, ADR 0517).

use junto_protocol::call::{BlockReason, CallJoin, MicrophoneBlock, SoundBlock};
use junto_protocol::ids::{AccountId, ChannelId};
use junto_server::media::{TrackKind, TrackSource};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::calls_review2::{invite, move_to};
use crate::calls_support::Scene;
use crate::devices::link_second_device;
use crate::support::{Member, TestServer};

fn block_path(channel_id: ChannelId, account_id: AccountId, kind: &str) -> String {
    format!("/api/v1/channels/{channel_id}/call/participants/{account_id}/{kind}")
}

fn blocks(join: &CallJoin) -> (Option<MicrophoneBlock>, Option<SoundBlock>) {
    (join.microphone_block.clone(), join.sound_block.clone())
}

async fn leave_as(server: &TestServer, member: &Member, identity: &str) {
    server
        .delete(member, &format!("/api/v1/call?identity={identity}"))
        .await
        .empty(StatusCode::NO_CONTENT);
}

/// The identity of the place of an account, as the server holds it.
async fn held(server: &TestServer, member: &Member) -> Option<String> {
    server
        .own_call(member)
        .await
        .call
        .map(|call| call.own.identity)
}

#[tokio::test]
async fn the_answer_to_a_join_states_the_blocks_of_the_caller() {
    let scene = Scene::start().await;
    let (server, owner, ben) = (&scene.server, &scene.owner, &scene.ben);
    let voice = scene.voice.id;
    server.enter_call(owner, voice).await;
    let first = server.enter_call(ben, voice).await;
    assert_eq!(blocks(&first.join), (None, None));

    for kind in ["microphone-block", "sound-block"] {
        server
            .put_empty(owner, &block_path(voice, ben.account_id, kind))
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let by_moderator = (
        Some(MicrophoneBlock {
            by: owner.account_id,
            reason: BlockReason::Moderator,
        }),
        Some(SoundBlock {
            by: owner.account_id,
        }),
    );
    // A new token for the same place.
    let renewed = server.join_call(ben, voice).await;
    assert_eq!(renewed.identity, first.identity);
    assert_eq!(blocks(&renewed), by_moderator);

    // The blocks belong to the call and the account. They hold for the next place, before
    // the call lists the person again.
    server.leave_call(ben).await;
    server.check().await;
    let again = server.join_call(ben, voice).await;
    assert_ne!(again.identity, first.identity);
    assert_eq!(blocks(&again), by_moderator);
    assert!(
        again
            .call
            .participants
            .iter()
            .all(|participant| participant.account_id != ben.account_id)
    );

    server
        .delete(owner, &block_path(voice, ben.account_id, "sound-block"))
        .await
        .empty(StatusCode::NO_CONTENT);
    let one = server.join_call(ben, voice).await;
    assert_eq!(blocks(&one), (by_moderator.0.clone(), None));
    server
        .delete(
            owner,
            &block_path(voice, ben.account_id, "microphone-block"),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(blocks(&server.join_call(ben, voice).await), (None, None));
}

#[tokio::test]
async fn the_answer_to_a_join_states_the_block_of_meeting_mode() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    let presenter = server.enter_call(ann, lounge).await;
    server
        .put(
            ann,
            "/api/v1/call/screen-share",
            &json!({ "audio": "none" }),
        )
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
        .put_empty(ann, &format!("/api/v1/channels/{lounge}/call/meeting"))
        .await
        .empty(StatusCode::NO_CONTENT);
    let by_meeting = Some(MicrophoneBlock {
        by: ann.account_id,
        reason: BlockReason::Meeting,
    });

    // A person who enters gets the block with the place.
    let entered = server.join_call(ben, lounge).await;
    assert_eq!(blocks(&entered), (by_meeting.clone(), None));
    // The presenter has none.
    assert_eq!(blocks(&server.join_call(ann, lounge).await), (None, None));

    // A person on the way in by a move is not a member of the destination yet, and gets
    // the block at the completion. The answer to the join of the move states it.
    server.enter_call(owner, garden).await;
    let pending: junto_protocol::call::CallMove = server
        .post(
            owner,
            &format!(
                "/api/v1/channels/{garden}/call/participants/{}/move",
                owner.account_id
            ),
            &json!({ "to_channel_id": lounge }),
        )
        .await
        .ok(StatusCode::CREATED);
    let moved: CallJoin = server
        .post(
            owner,
            &format!("/api/v1/call/moves/{}/join", pending.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(blocks(&moved), (by_meeting, None));
    assert!(moved.entry_requests.is_empty());
    // The seat of a move exists in the destination before the move completes: the answer
    // names the position at which it was read (ADR 0519).
    assert!(
        moved.cursor.is_some(),
        "the join of a move names no position"
    );
}

/// A block names the person who set it, and the block of a meeting tells that meeting mode
/// is on. An account that is not inside a call that hides who is inside learns neither
/// (ADR 0517): the answer states no block until the admission check has seen the account.
#[tokio::test]
async fn the_answer_states_no_block_before_the_account_is_inside_a_hidden_call() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    server
        .call_settings(owner, lounge, json!({ "hides_occupants": true }))
        .await;
    let presenter = server.enter_call(ann, lounge).await;
    server
        .put(
            ann,
            "/api/v1/call/screen-share",
            &json!({ "audio": "none" }),
        )
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
        .put_empty(ann, &format!("/api/v1/channels/{lounge}/call/meeting"))
        .await
        .empty(StatusCode::NO_CONTENT);

    // A seat that the check has not seen has the block of the meeting, and the answer does
    // not state it.
    let seat = server.join_call(ben, lounge).await;
    assert_eq!(server.count("call_microphone_blocks").await, 1);
    assert_eq!(blocks(&seat), (None, None));

    // An accepted invitation gives a seat in the same way.
    let carl = server.join(owner, "Carl").await;
    let invitation = invite(server, ann, lounge, &carl).await;
    let accepted: CallJoin = server
        .post(
            &carl,
            &format!("/api/v1/call/invitations/{}/acceptance", invitation.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(server.count("call_microphone_blocks").await, 2);
    assert_eq!(blocks(&accepted), (None, None));

    // A person on the way in by a move is not inside either.
    server.enter_call(owner, garden).await;
    let pending = move_to(server, owner, garden, owner.account_id, lounge).await;
    let moved: CallJoin = server
        .post(
            owner,
            &format!("/api/v1/call/moves/{}/join", pending.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(blocks(&moved), (None, None));
    server.fake().connect(&moved.token).unwrap();
    server.check().await;

    // A moderator inside blocks the sound of the seat. The answer would name the moderator.
    server
        .put_empty(owner, &block_path(lounge, ben.account_id, "sound-block"))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(blocks(&server.join_call(ben, lounge).await), (None, None));

    // The check sees the account. From then on the answer and the call state the blocks.
    server.connect(seat);
    server.check().await;
    let held = (
        Some(MicrophoneBlock {
            by: ann.account_id,
            reason: BlockReason::Meeting,
        }),
        Some(SoundBlock {
            by: owner.account_id,
        }),
    );
    assert_eq!(blocks(&server.join_call(ben, lounge).await), held);
    let read = server.own_call(ben).await.call.unwrap().call;
    let own = read
        .participants
        .into_iter()
        .find(|participant| participant.account_id == ben.account_id)
        .unwrap();
    assert_eq!((own.microphone_block, own.sound_block), held);
}

#[tokio::test]
async fn a_leave_that_names_an_identity_ends_only_the_place_with_that_identity() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server.enter_call(ben, voice).await;
    let first = server.enter_call(ann, voice).await;
    let other = link_second_device(server, ann).await;
    // The second device takes the place of the account in the same call.
    let second = server.enter_call(&other, voice).await;
    assert_ne!(second.identity, first.identity);
    assert_eq!(held(server, ann).await, Some(second.identity.clone()));
    let mut to_ben = server.full_stream(ben).await;

    // The first device gives up the place that it was given. The place of the second
    // device stays, and nobody is told anything.
    leave_as(server, ann, &first.identity).await;
    assert_eq!(held(server, ann).await, Some(second.identity.clone()));
    assert_eq!(server.count("call_members").await, 2);
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());
    assert_eq!(server.occupants(ben, voice).await.len(), 2);

    // The identity of the place ends the place, from whichever device of the account.
    leave_as(server, ann, &second.identity).await;
    assert_eq!(held(server, ann).await, None);
    assert_eq!(server.occupants(ben, voice).await, [ben.account_id]);
    // No place: the same answer.
    leave_as(server, ann, &second.identity).await;
    leave_as(server, ann, "").await;

    // Without an identity the call of the account ends from whichever device.
    let third = server.enter_call(&other, voice).await;
    assert_eq!(held(server, ann).await, Some(third.identity));
    server.leave_call(ann).await;
    assert_eq!(held(server, ann).await, None);
}
