//! Room privacy: hidden occupants, requests to enter, invitations and moves (ADR 0504).

use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::call::{
    CallInvitation, CallJoin, CallMove, EntryOutcome, EntryRequest, InvitationOutcome, LeaveReason,
    MoveOutcome,
};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::{AccountId, ChannelId};
use junto_protocol::snapshot::Snapshot;
use junto_server::calls::admission::Outcome;
use junto_server::calls::speaking;
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::Duration;

use crate::calls_support::{Scene, claims, names};
use crate::support::{Member, Reply, TestServer, drive_until};

const CLEAN: Outcome = Outcome::Checked {
    removed: 0,
    updated: 0,
};

fn lock_path(channel_id: ChannelId) -> String {
    format!("/api/v1/channels/{channel_id}/call/lock")
}

async fn ask(server: &TestServer, member: &Member, channel_id: ChannelId) -> Reply {
    let path = format!("/api/v1/channels/{channel_id}/call/entry-requests");
    server.post(member, &path, &Value::Null).await
}

async fn answer(
    server: &TestServer,
    member: &Member,
    request: &EntryRequest,
    accept: bool,
) -> Reply {
    let path = format!("/api/v1/call/entry-requests/{}/answer", request.id);
    server
        .post(member, &path, &json!({ "accept": accept }))
        .await
}

async fn invite(
    server: &TestServer,
    member: &Member,
    channel_id: ChannelId,
    account_id: AccountId,
) -> Reply {
    let path = format!("/api/v1/channels/{channel_id}/call/invitations");
    server
        .post(member, &path, &json!({ "account_id": account_id }))
        .await
}

#[tokio::test]
async fn a_hidden_voice_channel_shows_the_people_outside_only_that_it_is_in_use() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server
        .call_settings(owner, voice, json!({ "hides_occupants": true }))
        .await;
    let mut to_ben = server.full_stream(ben).await;
    let mut to_owner = server.full_stream(owner).await;

    let seat = server.enter_call(ann, voice).await;
    server
        .put(
            ann,
            "/api/v1/call/state",
            &json!({ "muted": true, "deafened": true }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put(ann, "/api/v1/call/speaking", &json!({ "speaking": true }))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(speaking::publish(&server.state).await.unwrap(), 0);
    server
        .put_empty(ann, &lock_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);

    for outsider in [ben, owner] {
        let shown = server.channel_call(outsider, voice).await;
        assert!(shown.in_use);
        assert_eq!(shown.call, None);
        let snapshot: Snapshot = server.snapshot(outsider).await;
        assert_eq!(snapshot.workspaces[0].calls, [shown]);
    }
    for stream in [(&ben, &mut to_ben), (&owner, &mut to_owner)] {
        let seen = server.call_events(stream.0, stream.1).await;
        assert_eq!(names(&seen), ["call_started"]);
        let text = serde_json::to_string(&seen).unwrap();
        assert!(!text.contains(&ann.account_id.to_string()), "{text}");
    }
    let raw = to_ben.raw.join("\n");
    assert!(!raw.contains(&ann.account_id.to_string()), "{raw}");
    assert!(!raw.contains(&seat.identity), "{raw}");

    let block = |account: AccountId, kind: &str| {
        format!("/api/v1/channels/{voice}/call/participants/{account}/{kind}")
    };
    for account in [ann.account_id, ben.account_id, AccountId::generate()] {
        for kind in ["microphone-block", "sound-block"] {
            let inside = server.put_empty(owner, &block(account, kind)).await;
            inside.error(ErrorCode::NotInCall);
            let removed = server.delete(owner, &block(account, kind)).await;
            removed.error(ErrorCode::NotInCall);
        }
        let path = format!("/api/v1/channels/{voice}/call/participants/{account}");
        server
            .delete(owner, &path)
            .await
            .error(ErrorCode::NotInCall);
        let other = server.voice(owner, scene.workspace_id, "elsewhere").await;
        let moved = server
            .post(
                owner,
                &format!("{path}/move"),
                &json!({ "to_channel_id": other.id }),
            )
            .await;
        moved.error(ErrorCode::NotInCall);
    }
    assert_eq!(server.count("call_microphone_blocks").await, 0);

    server
        .delete(ann, &lock_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());
    let mut ann_stream = server.full_stream(ann).await;
    // The answer of the join shows nothing of the call before the check has seen the seat
    // (ADR 0517).
    let ben_join = server.join_call(ben, voice).await;
    assert!(ben_join.call.hides_occupants);
    assert!(ben_join.call.participants.is_empty());
    assert_eq!(ben_join.call.lock, None);
    server.connect(ben_join);
    // The token of a seat of a hidden call allows pictures only; the check that sees the
    // account gives it its microphone (ADR 0517).
    let seen = Outcome::Checked {
        removed: 0,
        updated: 1,
    };
    assert_eq!(server.check().await, seen);
    assert_eq!(server.check().await, CLEAN);
    let inside = server.channel_call(ben, voice).await.call.unwrap();
    assert_eq!(inside.participants.len(), 2);
    assert_eq!(inside.participants[0].account_id, ann.account_id);
    assert!(inside.participants[0].muted);
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["own_call_changed", "call_participant_joined"]
    );
    assert_eq!(
        names(&server.call_events(ann, &mut ann_stream).await),
        ["call_participant_joined"]
    );
    assert!(server.call_events(owner, &mut to_owner).await.is_empty());
    assert_eq!(server.channel_call(owner, voice).await.call, None);

    server.leave_call(ben).await;
    server.check().await;
    assert_eq!(server.channel_call(ben, voice).await.call, None);
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["own_call_changed"]
    );

    // The running call keeps the setting that it started with; the next call shows who is
    // inside (ADR 0517).
    server
        .call_settings(owner, voice, json!({ "hides_occupants": false }))
        .await;
    assert_eq!(server.channel_call(ben, voice).await.call, None);
    server
        .put(
            ann,
            "/api/v1/call/state",
            &json!({ "muted": false, "deafened": false }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server.leave_call(ann).await;
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["call_ended"]
    );
    server.enter_call(ann, voice).await;
    let open = server.channel_call(ben, voice).await.call.unwrap();
    assert_eq!(open.participants[0].account_id, ann.account_id);
    assert!(!open.hides_occupants);
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["call_started", "call_participant_joined"]
    );
}

#[tokio::test]
async fn a_person_outside_asks_to_enter_and_the_first_answer_of_a_person_inside_applies() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let carl = server.join(owner, "Carl").await;

    ask(server, ben, voice).await.error(ErrorCode::Conflict);
    server.enter_call(ann, voice).await;
    server.enter_call(owner, voice).await;
    ask(server, ben, voice).await.error(ErrorCode::Conflict);
    server
        .put_empty(ann, &lock_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    ask(server, ann, voice).await.error(ErrorCode::Conflict);

    let mut to_ann = server.full_stream(ann).await;
    let mut to_ben = server.full_stream(ben).await;
    let mut to_carl = server.full_stream(&carl).await;
    let request: EntryRequest = ask(server, ben, voice).await.ok(StatusCode::CREATED);
    assert_eq!(request.account_id, ben.account_id);
    assert_eq!(
        request.expires_at,
        server.clock.now() + Duration::seconds(60)
    );
    ask(server, ben, voice).await.error(ErrorCode::Conflict);
    assert_eq!(
        server.call_events(ann, &mut to_ann).await,
        [Event::CallEntryRequested {
            request: request.clone()
        }]
    );
    assert!(server.call_events(&carl, &mut to_carl).await.is_empty());
    // The account that asked is told of its own request, for its other devices.
    assert_eq!(
        server.call_events(ben, &mut to_ben).await,
        [Event::CallEntryRequested {
            request: request.clone()
        }]
    );
    assert_eq!(
        server.own_call(ann).await.call.unwrap().entry_requests,
        std::slice::from_ref(&request)
    );

    answer(server, &carl, &request, true)
        .await
        .error(ErrorCode::NotFound);
    answer(server, ben, &request, true)
        .await
        .error(ErrorCode::NotFound);
    server
        .call_join_reply(ben, voice)
        .await
        .error(ErrorCode::CallLocked);

    server.gate().arm(0);
    let mut first = Box::pin(answer(server, ann, &request, true));
    drive_until(&mut first, server.gate().arrived()).await;
    let mut second = Box::pin(answer(server, owner, &request, false));
    drive_until(&mut second, server.until_blocked()).await;
    server.gate().release();
    first.await.empty(StatusCode::NO_CONTENT);
    second.await.error(ErrorCode::Conflict);
    answer(server, owner, &request, false)
        .await
        .error(ErrorCode::Conflict);

    let resolved = |by| Event::CallEntryResolved {
        request_id: request.id,
        call_id: request.call_id,
        place: request.place,
        account_id: ben.account_id,
        outcome: EntryOutcome::Accepted,
        by,
    };
    assert_eq!(
        server.call_events(ben, &mut to_ben).await,
        [Event::CallEntryAnswered {
            request_id: request.id,
            outcome: EntryOutcome::Accepted,
        }]
    );
    assert_eq!(
        server.call_events(ann, &mut to_ann).await,
        [resolved(Some(ann.account_id))]
    );
    assert!(server.call_events(&carl, &mut to_carl).await.is_empty());
    server
        .call_join_reply(&carl, voice)
        .await
        .error(ErrorCode::CallLocked);

    server.clock.advance(Duration::seconds(59));
    let join = server.join_call(ben, voice).await;
    server.connect(join);
    assert_eq!(server.check().await, CLEAN);
    assert!(
        server
            .channel_call(ben, voice)
            .await
            .call
            .unwrap()
            .lock
            .is_some()
    );
    server.leave_call(ben).await;
    server.check().await;
    server
        .call_join_reply(ben, voice)
        .await
        .error(ErrorCode::CallLocked);

    let late: EntryRequest = ask(server, ben, voice).await.ok(StatusCode::CREATED);
    answer(server, owner, &late, true)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.clock.advance(Duration::seconds(60));
    server
        .call_join_reply(ben, voice)
        .await
        .error(ErrorCode::CallLocked);
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(server.count("call_entry_requests").await, 0);

    let denied: EntryRequest = ask(server, &carl, voice).await.ok(StatusCode::CREATED);
    server.call_events(&carl, &mut to_carl).await;
    answer(server, ann, &denied, false)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(matches!(
        server.call_events(&carl, &mut to_carl).await.as_slice(),
        [Event::CallEntryAnswered {
            outcome: EntryOutcome::Denied,
            ..
        }]
    ));
    server
        .call_join_reply(&carl, voice)
        .await
        .error(ErrorCode::CallLocked);

    let expiring: EntryRequest = ask(server, &carl, voice).await.ok(StatusCode::CREATED);
    server.clock.advance(Duration::seconds(59));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(server.count("call_entry_requests").await, 1);
    server.clock.advance(Duration::seconds(1));
    answer(server, ann, &expiring, true)
        .await
        .error(ErrorCode::Conflict);
    assert_eq!(server.check().await, CLEAN);
    assert!(matches!(
        server.call_events(&carl, &mut to_carl).await.as_slice(),
        [
            Event::CallEntryRequested { .. },
            Event::CallEntryAnswered {
                outcome: EntryOutcome::Expired,
                ..
            }
        ]
    ));
    answer(server, ann, &expiring, true)
        .await
        .error(ErrorCode::NotFound);

    let withdrawn: EntryRequest = ask(server, &carl, voice).await.ok(StatusCode::CREATED);
    let path = format!("/api/v1/call/entry-requests/{}", withdrawn.id);
    server.delete(ben, &path).await.error(ErrorCode::NotFound);
    server
        .delete(&carl, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.delete(&carl, &path).await.error(ErrorCode::NotFound);
    server.call_events(ann, &mut to_ann).await;

    server
        .delete(ann, &lock_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    ask(server, &carl, voice).await.error(ErrorCode::Conflict);
    server
        .call_settings(owner, voice, json!({ "user_limit": 2 }))
        .await;
    server
        .call_join_reply(&carl, voice)
        .await
        .error(ErrorCode::CallFull);
    let full: EntryRequest = ask(server, &carl, voice).await.ok(StatusCode::CREATED);
    answer(server, owner, &full, true)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.join_call(&carl, voice).await;
    assert_eq!(server.count("call_members").await, 3);
}

#[tokio::test]
async fn an_invitation_passes_the_lock_and_the_limit_and_no_permission() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let carl = server.join(owner, "Carl").await;
    let role = server
        .grant_workspace(owner, ann, scene.workspace_id, &[])
        .await;
    for holder in [ben, &carl] {
        let path = format!(
            "/api/v1/workspaces/{}/members/{}/roles/{role}",
            scene.workspace_id, holder.account_id
        );
        server
            .put_empty(owner, &path)
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let dana = server.join(owner, "Dana").await;
    server
        .make_private(owner, scene.workspace_id, voice, role)
        .await;
    server
        .call_settings(owner, voice, json!({ "user_limit": 1 }))
        .await;

    invite(server, ann, voice, ben.account_id)
        .await
        .error(ErrorCode::NotInCall);
    server.enter_call(ann, voice).await;
    server
        .put_empty(ann, &lock_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    invite(server, ben, voice, carl.account_id)
        .await
        .error(ErrorCode::NotInCall);
    invite(server, ann, voice, ann.account_id)
        .await
        .error(ErrorCode::InvalidRequest);
    invite(server, ann, voice, dana.account_id)
        .await
        .error(ErrorCode::Conflict);
    invite(server, ann, voice, AccountId::generate())
        .await
        .error(ErrorCode::NotFound);
    invite(server, &dana, voice, ben.account_id)
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(server.count("call_invitations").await, 0);

    let mut to_ben = server.subscribed(ben, Level::Background).await;
    let mut to_ann = server.full_stream(ann).await;
    let invitation: CallInvitation = invite(server, ann, voice, ben.account_id)
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(invitation.invited_by, ann.account_id);
    assert_eq!(
        invitation.expires_at,
        server.clock.now() + Duration::seconds(60)
    );
    assert_eq!(
        server.call_events(ben, &mut to_ben).await,
        [Event::CallInvited {
            invitation: invitation.clone()
        }]
    );
    assert_eq!(
        server.snapshot(ben).await.call_invitations,
        std::slice::from_ref(&invitation)
    );
    assert!(server.snapshot(&carl).await.call_invitations.is_empty());

    let accept = |id| format!("/api/v1/call/invitations/{id}/acceptance");
    let refuse = |id| format!("/api/v1/call/invitations/{id}/refusal");
    let cancel = |id| format!("/api/v1/call/invitations/{id}");
    server
        .post(&carl, &accept(invitation.id), &Value::Null)
        .await
        .error(ErrorCode::NotFound);
    server
        .call_join_reply(ben, voice)
        .await
        .error(ErrorCode::CallLocked);
    let join: CallJoin = server
        .post(ben, &accept(invitation.id), &Value::Null)
        .await
        .ok(StatusCode::OK);
    server.connect(join);
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(server.occupants(ann, voice).await.len(), 2);
    let resolved = server.call_events(ann, &mut to_ann).await;
    assert!(resolved.iter().any(|event| matches!(
        event,
        Event::CallInvitationResolved { outcome: InvitationOutcome::Accepted, invitation_id }
            if *invitation_id == invitation.id
    )));
    server
        .post(ben, &accept(invitation.id), &Value::Null)
        .await
        .error(ErrorCode::NotFound);
    invite(server, ann, voice, ben.account_id)
        .await
        .error(ErrorCode::Conflict);

    let mut to_carl = server.subscribed(&carl, Level::Background).await;
    let declined: CallInvitation = invite(server, ben, voice, carl.account_id)
        .await
        .ok(StatusCode::CREATED);
    server
        .post(&carl, &refuse(declined.id), &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    let told = server.call_events(&carl, &mut to_carl).await;
    assert_eq!(names(&told), ["call_invited", "call_invitation_resolved"]);
    assert!(matches!(
        &told[1],
        Event::CallInvitationResolved {
            outcome: InvitationOutcome::Declined,
            ..
        }
    ));

    let withdrawn: CallInvitation = invite(server, ben, voice, carl.account_id)
        .await
        .ok(StatusCode::CREATED);
    server
        .delete(ann, &cancel(withdrawn.id))
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(ben, &cancel(withdrawn.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(&carl, &accept(withdrawn.id), &Value::Null)
        .await
        .error(ErrorCode::NotFound);

    let expiring: CallInvitation = invite(server, ann, voice, carl.account_id)
        .await
        .ok(StatusCode::CREATED);
    server.clock.advance(Duration::seconds(60));
    server
        .post(&carl, &accept(expiring.id), &Value::Null)
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(server.count("call_invitations").await, 0);
    let told = server.call_events(&carl, &mut to_carl).await;
    assert!(matches!(
        told.last(),
        Some(Event::CallInvitationResolved {
            outcome: InvitationOutcome::Expired,
            ..
        })
    ));

    let unused: CallInvitation = invite(server, ann, voice, carl.account_id)
        .await
        .ok(StatusCode::CREATED);
    let path = format!(
        "/api/v1/workspaces/{}/members/{}/roles/{role}",
        scene.workspace_id, carl.account_id
    );
    server
        .delete(owner, &path)
        .await
        .empty(StatusCode::NO_CONTENT);
    let lost = server.post(&carl, &accept(unused.id), &Value::Null).await;
    lost.error(ErrorCode::NotFound);
    assert!(server.snapshot(&carl).await.call_invitations.is_empty());
    assert_eq!(server.count("call_members").await, 2);
}

async fn move_to(
    server: &TestServer,
    mover: &Member,
    from: ChannelId,
    account_id: AccountId,
    to: ChannelId,
) -> Reply {
    let path = format!("/api/v1/channels/{from}/call/participants/{account_id}/move");
    server
        .post(mover, &path, &json!({ "to_channel_id": to }))
        .await
}

#[tokio::test]
async fn a_move_joins_the_destination_first_and_leaves_the_source_when_the_join_is_seen() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    let text = server.channel(owner, scene.workspace_id, "notes").await.id;
    let source = server.enter_call(ann, lounge).await;
    server.enter_call(ben, lounge).await;
    server
        .put(
            ann,
            "/api/v1/call/state",
            &json!({ "muted": true, "deafened": false }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);

    move_to(server, ben, lounge, ann.account_id, garden)
        .await
        .error(ErrorCode::Forbidden);
    move_to(server, owner, lounge, owner.account_id, garden)
        .await
        .error(ErrorCode::NotFound);
    move_to(server, owner, lounge, ann.account_id, lounge)
        .await
        .error(ErrorCode::InvalidRequest);
    move_to(server, owner, lounge, ann.account_id, text)
        .await
        .error(ErrorCode::Conflict);
    move_to(server, owner, garden, ann.account_id, lounge)
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(server.count("call_moves").await, 0);
    assert_eq!(server.count("calls").await, 1);

    server
        .call_settings(owner, garden, json!({ "user_limit": 1 }))
        .await;
    let dana = server.join(owner, "Dana").await;
    server.enter_call(&dana, garden).await;
    server
        .put_empty(&dana, &lock_path(garden))
        .await
        .empty(StatusCode::NO_CONTENT);

    let mut to_ann = server.full_stream(ann).await;
    let mut to_owner = server.full_stream(owner).await;
    let mut to_ben = server.full_stream(ben).await;
    let pending: CallMove = move_to(server, owner, lounge, ann.account_id, garden)
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(
        pending.expires_at,
        server.clock.now() + Duration::seconds(30)
    );
    move_to(server, owner, lounge, ann.account_id, garden)
        .await
        .error(ErrorCode::Conflict);
    assert_eq!(
        server.call_events(ann, &mut to_ann).await,
        [Event::CallMoveRequested {
            call_move: pending.clone()
        }]
    );
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(
        server.own_call(ann).await.call.unwrap().own.call_id,
        source.join.call.id
    );
    assert_eq!(server.occupants(ben, lounge).await.len(), 2);
    assert_eq!(server.occupants(ben, garden).await, [dana.account_id]);

    let join_path = format!("/api/v1/call/moves/{}/join", pending.id);
    server
        .post(ben, &join_path, &Value::Null)
        .await
        .error(ErrorCode::NotFound);
    let join: CallJoin = server
        .post(ann, &join_path, &Value::Null)
        .await
        .ok(StatusCode::OK);
    assert_ne!(join.identity, source.identity);
    assert_ne!(
        claims(&join)["video"]["room"],
        claims(&source.join)["video"]["room"]
    );
    let destination = server.connect(join);
    assert!(
        server
            .fake()
            .identities(&source.room)
            .contains(&source.identity)
    );
    assert!(
        server
            .fake()
            .identities(&destination.room)
            .contains(&destination.identity)
    );

    assert_eq!(
        server.check().await,
        Outcome::Checked {
            removed: 1,
            updated: 0
        }
    );
    assert!(
        !server
            .fake()
            .identities(&source.room)
            .contains(&source.identity)
    );
    assert_eq!(server.occupants(ben, lounge).await, [ben.account_id]);
    let moved = server.channel_call(ben, garden).await.call.unwrap();
    assert_eq!(moved.participants.len(), 2);
    assert!(moved.lock.is_some());
    let arrived = moved
        .participants
        .iter()
        .find(|participant| participant.account_id == ann.account_id)
        .unwrap();
    assert!(arrived.muted);
    assert_eq!(arrived.identity, destination.identity);
    assert_eq!(
        server.own_call(ann).await.call.unwrap().own.identity,
        destination.identity
    );

    let told = server.call_events(ann, &mut to_ann).await;
    assert_eq!(
        names(&told),
        [
            "call_participant_left",
            "call_participant_joined",
            "own_call_changed",
            "call_move_resolved"
        ]
    );
    assert!(matches!(
        &told[2],
        Event::OwnCallChanged { call: Some(own), left: Some(LeaveReason::Moved) }
            if own.call_id == moved.id
    ));
    let completed = Event::CallMoveResolved {
        move_id: pending.id,
        outcome: MoveOutcome::Completed,
    };
    assert_eq!(told[3], completed);
    assert!(
        server
            .call_events(owner, &mut to_owner)
            .await
            .contains(&completed)
    );
    assert!(matches!(
        server.call_events(ben, &mut to_ben).await.as_slice(),
        [
            Event::CallParticipantLeft {
                reason: LeaveReason::Moved,
                ..
            },
            Event::CallParticipantJoined { .. }
        ]
    ));
    let log: AuditPage = server
        .get(
            owner,
            &format!("/api/v1/workspaces/{}/audit-log", scene.workspace_id),
        )
        .await
        .ok(StatusCode::OK);
    let account_id = ann.account_id;
    assert_eq!(
        log.entries[0].event,
        AuditEvent::CallMemberMovedIn { account_id }
    );
    assert_eq!(log.entries[0].channel_id, Some(garden));
    assert_eq!(
        log.entries[1].event,
        AuditEvent::CallMemberMovedOut { account_id }
    );
    assert_eq!(log.entries[1].channel_id, Some(lounge));
    for entry in &log.entries[..2] {
        assert_eq!(entry.actor_id, Some(owner.account_id));
    }
    server
        .post(ann, &join_path, &Value::Null)
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn a_move_that_does_not_complete_changes_nothing_and_both_sides_are_told() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    let source = server.enter_call(ann, lounge).await;
    let mut to_ann = server.full_stream(ann).await;
    let mut to_owner = server.full_stream(owner).await;
    let mut to_ben = server.full_stream(ben).await;

    let pending: CallMove = move_to(server, owner, lounge, ann.account_id, garden)
        .await
        .ok(StatusCode::CREATED);
    let join_path = format!("/api/v1/call/moves/{}/join", pending.id);
    let join: CallJoin = server
        .post(ann, &join_path, &Value::Null)
        .await
        .ok(StatusCode::OK);
    server.clock.advance(Duration::seconds(29));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(server.count("call_moves").await, 1);
    server.clock.advance(Duration::seconds(1));
    server
        .post(ann, &join_path, &Value::Null)
        .await
        .error(ErrorCode::NotFound);
    let late = server.connect(join);
    assert_eq!(
        server.check().await,
        Outcome::Checked {
            removed: 1,
            updated: 0
        }
    );
    assert!(server.fake().identities(&late.room).is_empty());
    assert_eq!(server.count("call_moves").await, 0);
    assert_eq!(server.count("calls").await, 1);
    let failed = Event::CallMoveResolved {
        move_id: pending.id,
        outcome: MoveOutcome::TimedOut,
    };
    assert_eq!(
        server.call_events(ann, &mut to_ann).await,
        [
            Event::CallMoveRequested {
                call_move: pending.clone()
            },
            failed.clone()
        ]
    );
    assert_eq!(
        server.call_events(owner, &mut to_owner).await,
        [
            Event::CallMoveRequested {
                call_move: pending.clone()
            },
            failed
        ]
    );
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());
    assert_eq!(
        server.own_call(ann).await.call.unwrap().own.identity,
        source.identity
    );
    assert_eq!(server.occupants(ben, lounge).await, [ann.account_id]);
    assert!(!server.channel_call(ben, garden).await.in_use);

    server.fake().enter(
        &late.room,
        &late.identity,
        junto_server::media::ParticipantKind::Standard,
        None,
    );
    assert_eq!(
        server.check().await,
        Outcome::Checked {
            removed: 1,
            updated: 0
        }
    );
    assert!(server.fake().identities(&late.room).is_empty());
    assert!(
        server
            .fake()
            .identities(&source.room)
            .contains(&source.identity)
    );

    let refused: CallMove = move_to(server, owner, lounge, ann.account_id, garden)
        .await
        .ok(StatusCode::CREATED);
    let failure = format!("/api/v1/call/moves/{}/failure", refused.id);
    server
        .post(ben, &failure, &Value::Null)
        .await
        .error(ErrorCode::NotFound);
    server
        .post(ann, &failure, &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(matches!(
        server.call_events(owner, &mut to_owner).await.as_slice(),
        [
            Event::CallMoveRequested { .. },
            Event::CallMoveResolved {
                outcome: MoveOutcome::Refused,
                ..
            }
        ]
    ));
    assert_eq!(server.count("calls").await, 1);
    assert_eq!(server.occupants(ben, lounge).await, [ann.account_id]);

    let abandoned: CallMove = move_to(server, owner, lounge, ann.account_id, garden)
        .await
        .ok(StatusCode::CREATED);
    server.call_events(owner, &mut to_owner).await;
    server.leave_call(ann).await;
    assert!(matches!(
        server.call_events(owner, &mut to_owner).await.as_slice(),
        [
            Event::CallMoveResolved { outcome: MoveOutcome::Abandoned, move_id, .. },
            Event::CallParticipantLeft { .. },
            Event::CallEnded { .. }
        ] if *move_id == abandoned.id
    ));
    assert_eq!(server.count("calls").await, 0);
    assert_eq!(server.count("call_moves").await, 0);
}

#[tokio::test]
async fn a_move_and_a_join_that_meet_at_a_limited_destination_keep_the_record_consistent() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    server
        .call_settings(owner, garden, json!({ "user_limit": 1 }))
        .await;
    server.enter_call(ann, lounge).await;

    server.gate().arm(0);
    let mut moving = Box::pin(move_to(server, owner, lounge, ann.account_id, garden));
    drive_until(&mut moving, server.gate().arrived()).await;
    let mut joining = Box::pin(server.call_join_reply(ben, garden));
    drive_until(&mut joining, server.until_blocked()).await;
    server.gate().release();
    let pending: CallMove = moving.await.ok(StatusCode::CREATED);
    joining.await.error(ErrorCode::CallFull);
    assert_eq!(server.count("calls").await, 2);
    assert_eq!(server.own_call(ben).await.call, None);

    let join_path = format!("/api/v1/call/moves/{}/join", pending.id);
    let join: CallJoin = server
        .post(ann, &join_path, &Value::Null)
        .await
        .ok(StatusCode::OK);
    server.connect(join);
    server.check().await;
    assert_eq!(server.occupants(ben, garden).await, [ann.account_id]);
    assert!(!server.channel_call(ben, lounge).await.in_use);
    server
        .call_join_reply(ben, garden)
        .await
        .error(ErrorCode::CallFull);

    let orchard = server.voice(owner, scene.workspace_id, "orchard").await.id;
    server
        .call_settings(owner, orchard, json!({ "user_limit": 1 }))
        .await;
    server.gate().arm(0);
    let mut joining = Box::pin(server.call_join_reply(ben, orchard));
    drive_until(&mut joining, server.gate().arrived()).await;
    let mut moving = Box::pin(move_to(server, owner, garden, ann.account_id, orchard));
    drive_until(&mut moving, server.until_blocked()).await;
    server.gate().release();
    let ben_join: CallJoin = joining.await.ok(StatusCode::OK);
    let pending: CallMove = moving.await.ok(StatusCode::CREATED);
    server.connect(ben_join);
    let join_path = format!("/api/v1/call/moves/{}/join", pending.id);
    let join: CallJoin = server
        .post(ann, &join_path, &Value::Null)
        .await
        .ok(StatusCode::OK);
    server.connect(join);
    server.check().await;
    assert_eq!(server.occupants(ben, orchard).await.len(), 2);
    assert_eq!(server.count("call_members").await, 2);
    assert_eq!(server.count("call_moves").await, 0);
    assert_eq!(server.count("calls").await, 1);
}
