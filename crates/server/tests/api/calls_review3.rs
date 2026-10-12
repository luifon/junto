//! Tests for the defects that the third review of the call unit found.

use junto_protocol::ServerPermission;
use junto_protocol::call::CallJoin;
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::AccountId;
use junto_protocol::structure::Workspace;
use junto_server::calls::admission::Outcome;
use junto_server::media::{ParticipantKind, TrackSource};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::calls_review2::{invite, move_to, ring};
use crate::calls_support::{Scene, names};
use crate::support::Member;

/// Defect 1: in the check of one room, removals that take the whole time limit of the room
/// do not stop the changes of permissions, and changes of permissions that take it do not
/// stop the removals.
#[tokio::test]
async fn slow_removals_and_slow_changes_of_permissions_do_not_stop_each_other() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    let ann_seat = server.enter_call(ann, lounge).await;
    let ben_seat = server.enter_call(ben, garden).await;
    for (channel_id, account) in [(lounge, ann), (garden, ben)] {
        let block = format!(
            "/api/v1/channels/{channel_id}/call/participants/{}/microphone-block",
            account.account_id
        );
        server
            .put_empty(owner, &block)
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let limits = &server.state.config.limits.calls;
    let slow = Some(limits.room_check_time_limit.unsigned_abs() * 4);
    let (fake, standard) = (server.fake(), ParticipantKind::Standard);

    fake.enter(&ann_seat.room, "slow", standard, None);
    fake.stall_actions_on("slow", slow);
    fake.enter(&ben_seat.room, "intruder", standard, None);
    fake.stall_actions_on(&ben_seat.identity, slow);
    let found = server.check().await;

    let blocked: Vec<(String, String)> = fake
        .updated()
        .into_iter()
        .filter(|(_, _, publishing)| !publishing.allows(TrackSource::Microphone))
        .map(|(room, identity, _)| (room, identity))
        .collect();
    assert_eq!(
        blocked,
        [(ann_seat.room.clone(), ann_seat.identity.clone())]
    );
    assert_eq!(
        fake.removed(),
        [(ben_seat.room.clone(), "intruder".to_owned())]
    );
    assert_eq!(
        found,
        Outcome::Checked {
            removed: 1,
            updated: 1
        }
    );
    for seat in [&ann_seat, &ben_seat] {
        assert!(server.state.calls.is_failing(&seat.room));
    }

    fake.stall_actions_on("slow", None);
    fake.stall_actions_on(&ben_seat.identity, None);
    assert_eq!(
        server.check().await,
        Outcome::Checked {
            removed: 1,
            updated: 1
        }
    );
    for seat in [&ann_seat, &ben_seat] {
        assert!(!server.state.calls.is_failing(&seat.room));
        assert_eq!(
            fake.identities(&seat.room),
            std::slice::from_ref(&seat.identity)
        );
    }
}

/// Defect 2: an account that may no longer be at a voice channel learns nothing about an
/// invitation there. It gets the answer for an absent invitation while the invitation waits
/// and after it was answered, and the invitation stays as it is.
#[tokio::test]
async fn an_account_that_lost_the_channel_cannot_tell_whether_its_invitation_waits() {
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
    let invitation = invite(server, ann, board, ben).await;
    let withdrawal = format!("/api/v1/call/invitations/{}", invitation.id);
    server
        .delete(owner, &role_of(ann))
        .await
        .empty(StatusCode::NO_CONTENT);
    let waiting = server.delete(ann, &withdrawal).await;
    waiting.error(ErrorCode::NotFound);
    assert_eq!(server.count("call_invitations").await, 1);

    let acceptance = format!("/api/v1/call/invitations/{}/acceptance", invitation.id);
    let join: CallJoin = server
        .post(ben, &acceptance, &Value::Null)
        .await
        .ok(StatusCode::OK);
    assert_eq!(server.count("call_invitations").await, 0);
    let answered = server.delete(ann, &withdrawal).await;
    assert_eq!(answered.observable(), waiting.observable());

    server.connect(join);
    server.check().await;
    let carl = server.join(owner, "Carl").await;
    server
        .put_empty(owner, &role_of(&carl))
        .await
        .empty(StatusCode::NO_CONTENT);
    let invitation = invite(server, ben, board, &carl).await;
    server
        .delete(owner, &role_of(&carl))
        .await
        .empty(StatusCode::NO_CONTENT);
    let refusal = format!("/api/v1/call/invitations/{}/refusal", invitation.id);
    server
        .post(&carl, &refusal, &Value::Null)
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(server.count("call_invitations").await, 1);
}

/// Defect 3: an account mutes its conversation with every account that may call it
/// (ADR 0307), also when it may not see the profile of that account. An account that may do
/// neither gets the answer for an absent one.
#[tokio::test]
async fn a_conversation_with_an_account_that_may_call_is_muted_without_sight_of_its_profile() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let second: Workspace = server
        .post(owner, "/api/v1/workspaces", &json!({ "name": "Second" }))
        .await
        .ok(StatusCode::CREATED);
    let carl = server.join_into(owner, "Carl", &[second.id]).await;
    let dora = server.join_into(owner, "Dora", &[second.id]).await;
    server
        .grant_server(owner, &carl, &[ServerPermission::Administrator])
        .await;
    let mute = |account_id: AccountId| format!("/api/v1/direct-messages/{account_id}/mute");
    let profile = format!("/api/v1/accounts/{}/profile", carl.account_id);
    server.get(ann, &profile).await.error(ErrorCode::NotFound);

    ring(server, &carl, ann)
        .await
        .ok::<CallJoin>(StatusCode::OK);
    assert_eq!(server.count("call_invitations").await, 1);
    server.leave_call(&carl).await;
    server
        .put(ann, &mute(carl.account_id), &json!({}))
        .await
        .empty(StatusCode::NO_CONTENT);
    ring(server, &carl, ann)
        .await
        .ok::<CallJoin>(StatusCode::OK);
    let silent: Vec<bool> = sqlx::query_scalar("select silent from call_invitations")
        .fetch_all(&server.db)
        .await
        .unwrap();
    assert_eq!(silent, [true]);
    assert!(server.snapshot(ann).await.call_invitations.is_empty());
    server.leave_call(&carl).await;
    server
        .delete(ann, &mute(carl.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.count("direct_mutes").await, 0);

    let absent = mute(AccountId::generate());
    let unrelated = server.put(ann, &mute(dora.account_id), &json!({})).await;
    unrelated.error(ErrorCode::NotFound);
    let unknown = server.put(ann, &absent, &json!({})).await;
    assert_eq!(unrelated.observable(), unknown.observable());
    let unrelated = server.delete(ann, &mute(dora.account_id)).await;
    unrelated.error(ErrorCode::NotFound);
    let unknown = server.delete(ann, &absent).await;
    assert_eq!(unrelated.observable(), unknown.observable());
}

/// Defect 5: a person who moves themselves is told once how the move ended.
#[tokio::test]
async fn a_person_who_moves_themselves_is_told_the_outcome_once() {
    let scene = Scene::start().await;
    let (server, owner) = (&scene.server, &scene.owner);
    let lounge = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    server.enter_call(owner, lounge).await;
    let mut to_owner = server.full_stream(owner).await;

    let pending = move_to(server, owner, lounge, owner.account_id, garden).await;
    let failure = format!("/api/v1/call/moves/{}/failure", pending.id);
    server
        .post(owner, &failure, &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        names(&server.call_events(owner, &mut to_owner).await),
        ["call_move_requested", "call_move_resolved"]
    );

    let pending = move_to(server, owner, lounge, owner.account_id, garden).await;
    let join: CallJoin = server
        .post(
            owner,
            &format!("/api/v1/call/moves/{}/join", pending.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    server.connect(join);
    server.check().await;
    let told = names(&server.call_events(owner, &mut to_owner).await);
    let resolved = told.iter().filter(|name| **name == "call_move_resolved");
    assert_eq!(resolved.count(), 1, "{told:?}");
}
