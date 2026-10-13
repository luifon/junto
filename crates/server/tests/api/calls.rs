//! Joining a call and the admission check, against the fake media server.

use junto_protocol::WorkspacePermission;
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::call::{CallPlace, LeaveReason};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::server::Health;
use junto_protocol::snapshot::Snapshot;
use junto_server::calls::admission::Outcome;
use junto_server::media::ParticipantKind;
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::Duration;

use crate::calls_support::{Scene, claims, names};
use crate::structure::set_override;
use crate::support::{DeviceKey, TestServer, drive_until, member};

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

#[tokio::test]
async fn a_member_joins_a_voice_channel_with_a_short_token_for_one_room() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let mut to_ann = server.full_stream(ann).await;
    let mut to_ben = server.full_stream(ben).await;

    let join = server.join_call(ann, scene.voice.id).await;
    assert_eq!(join.media_url, "wss://media.example.test");
    assert_eq!(
        join.token_expires_at,
        server.clock.now() + Duration::seconds(60)
    );
    assert!(join.call.participants.is_empty());
    assert_eq!(join.call.started_at, None);
    let place = CallPlace::Channel {
        workspace_id: scene.workspace_id,
        channel_id: scene.voice.id,
    };
    assert_eq!(join.call.place, place);

    let token = claims(&join);
    let room = token["video"]["room"].as_str().unwrap().to_owned();
    assert_eq!(token["sub"], join.identity);
    assert_eq!(token["exp"], server.clock.now().unix_timestamp() + 60);
    assert_eq!(token["video"]["roomJoin"], true);
    assert_eq!(
        token["video"]["canPublishSources"],
        serde_json::json!(["camera", "microphone", "screen_share", "screen_share_audio"])
    );
    for grant in ["roomAdmin", "roomCreate", "roomList", "hidden", "recorder"] {
        assert_eq!(token["video"].get(grant), None, "{grant}");
    }
    assert!(join.identity.starts_with(&format!("{}:", ann.account_id)));
    for revealing in [
        scene.voice.id.to_string(),
        scene.workspace_id.to_string(),
        ann.account_id.to_string(),
        "lounge".to_owned(),
    ] {
        assert!(!room.contains(&revealing), "{room} contains {revealing}");
    }
    assert!(room.starts_with("jr-") && room.len() == 48, "{room}");

    assert_eq!(
        server.call_events(ann, &mut to_ann).await,
        [Event::OwnCallChanged {
            call: Some(junto_protocol::call::OwnCall {
                hides_occupants: Some(false),
                call_id: join.call.id,
                place,
                device_id: ann.device_id,
                identity: join.identity.clone(),
            }),
            left: None,
        }]
    );
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());
    assert!(!server.channel_call(ben, scene.voice.id).await.in_use);

    let seat = server.connect(join);
    assert_eq!(
        server.check().await,
        Outcome::Checked {
            removed: 0,
            updated: 0
        }
    );
    let seen = server.call_events(ben, &mut to_ben).await;
    assert_eq!(names(&seen), ["call_started", "call_participant_joined"]);
    let Event::CallParticipantJoined { participant, .. } = &seen[1] else {
        panic!("{seen:?}");
    };
    assert_eq!(participant.account_id, ann.account_id);
    assert_eq!(participant.identity, seat.identity);
    assert!(!participant.muted && participant.microphone_block.is_none());
    assert_eq!(
        names(&server.call_events(ann, &mut to_ann).await),
        ["call_started", "call_participant_joined"]
    );

    let shown = server.channel_call(ben, scene.voice.id).await;
    assert!(shown.in_use);
    assert_eq!(
        shown.call.unwrap().participants[0].account_id,
        ann.account_id
    );
    let snapshot: Snapshot = server.snapshot(ben).await;
    assert_eq!(snapshot.own_call, None);
    assert_eq!(snapshot.workspaces[0].calls.len(), 1);
    assert_eq!(
        server.snapshot(ann).await.own_call.unwrap().identity,
        seat.identity
    );
    assert_eq!(
        server.own_call(ann).await.call.unwrap().own.device_id,
        ann.device_id
    );
    assert_eq!(server.own_call(ben).await.call, None);

    assert_eq!(
        server.check().await,
        Outcome::Checked {
            removed: 0,
            updated: 0
        }
    );
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());
}

#[tokio::test]
async fn a_person_who_is_not_allowed_is_removed_at_every_check() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let seat = server.enter_call(ann, scene.voice.id).await;
    let ben_seat = server.enter_call(ben, scene.voice.id).await;
    let mut to_ann = server.full_stream(ann).await;
    let mut to_ben = server.full_stream(ben).await;

    server.leave_call(ben).await;
    assert_eq!(
        server.call_events(ann, &mut to_ann).await,
        [Event::CallParticipantLeft {
            call_id: seat.join.call.id,
            place: seat.join.call.place,
            account_id: ben.account_id,
            reason: LeaveReason::Left,
        }]
    );
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["call_participant_left", "own_call_changed"]
    );
    assert_eq!(server.fake().identities(&seat.room).len(), 2);
    assert_eq!(
        server.check().await,
        Outcome::Checked {
            removed: 1,
            updated: 0
        }
    );
    assert_eq!(
        server.fake().identities(&seat.room),
        std::slice::from_ref(&seat.identity)
    );

    for _ in 0..2 {
        server.clock.advance(Duration::seconds(55));
        let back = server.fake().connect(&ben_seat.join.token);
        assert_eq!(back.unwrap().1, ben_seat.identity);
        assert_eq!(
            server.occupants(ann, scene.voice.id).await,
            [ann.account_id]
        );
        assert_eq!(
            server.check().await,
            Outcome::Checked {
                removed: 1,
                updated: 0
            }
        );
        assert_eq!(
            server.fake().identities(&seat.room),
            std::slice::from_ref(&seat.identity)
        );
    }
    server.clock.advance(Duration::seconds(11));
    assert_eq!(
        server.fake().connect(&ben_seat.join.token),
        Err(junto_server::media::fake::Refusal::Expired)
    );
    server.clock.advance(Duration::seconds(600));
    server.fake().enter(
        &seat.room,
        &ben_seat.identity,
        ParticipantKind::Standard,
        None,
    );
    server
        .fake()
        .enter(&seat.room, "stranger", ParticipantKind::Other, None);
    server
        .fake()
        .enter(&seat.room, "EG_recording", ParticipantKind::Recorder, None);
    server.fake().connect(&seat.join.token).unwrap_err();
    server
        .fake()
        .enter(&seat.room, &seat.identity, ParticipantKind::Standard, None);
    assert_eq!(
        server.check().await,
        Outcome::Checked {
            removed: 2,
            updated: 1
        }
    );
    let mut left = server.fake().identities(&seat.room);
    left.sort();
    let mut expected = [seat.identity.clone(), "EG_recording".to_owned()];
    expected.sort();
    assert_eq!(left, expected);
    assert!(server.call_events(ann, &mut to_ann).await.is_empty());
    assert_eq!(
        server.occupants(ben, scene.voice.id).await,
        [ann.account_id]
    );

    let again = server.join_call(ben, scene.voice.id).await;
    assert_ne!(again.identity, ben_seat.identity);
    server.connect(again);
    assert_eq!(
        server.check().await,
        Outcome::Checked {
            removed: 0,
            updated: 0
        }
    );
    assert_eq!(
        server.occupants(ann, scene.voice.id).await,
        [ann.account_id, ben.account_id]
    );
}

#[tokio::test]
async fn a_call_is_joined_only_by_an_account_that_sees_the_voice_channel() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let role = server
        .grant_workspace(owner, ann, scene.workspace_id, &[])
        .await;
    let private = server.voice(owner, scene.workspace_id, "board").await;
    server
        .make_private(owner, scene.workspace_id, private.id, role)
        .await;
    let text = server.channel(owner, scene.workspace_id, "notes").await;

    let hidden = server.call_join_reply(ben, private.id).await;
    hidden.error(ErrorCode::NotFound);
    let absent = junto_protocol::ids::ChannelId::generate();
    assert_eq!(
        hidden.observable(),
        server.call_join_reply(ben, absent).await.observable()
    );
    server
        .get(ben, &format!("/api/v1/channels/{}/call", private.id))
        .await
        .error(ErrorCode::NotFound);
    server
        .call_join_reply(ann, text.id)
        .await
        .error(ErrorCode::Conflict);
    assert_eq!(server.count("calls").await, 0);

    let seat = server.enter_call(ann, private.id).await;
    server.enter_call(owner, private.id).await;
    assert_eq!(server.occupants(owner, private.id).await.len(), 2);
    server
        .call_join_reply(ben, private.id)
        .await
        .error(ErrorCode::NotFound);

    let mut to_ann = server.full_stream(ann).await;
    server
        .delete(
            owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{role}",
                scene.workspace_id, ann.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.fake().identities(&seat.room).len(), 2);
    assert_eq!(server.check().await, removed(1));
    assert!(
        !server
            .fake()
            .identities(&seat.room)
            .contains(&seat.identity)
    );
    let told = server.call_events(ann, &mut to_ann).await;
    assert_eq!(
        told,
        [Event::OwnCallChanged {
            call: None,
            left: Some(LeaveReason::AccessEnded),
        }]
    );
    assert_eq!(
        server.occupants(owner, private.id).await,
        [owner.account_id]
    );
    assert_eq!(server.own_call(ann).await.call, None);

    server.fake().connect(&seat.join.token).unwrap();
    assert_eq!(server.check().await, removed(1));
    assert_eq!(
        server.occupants(owner, private.id).await,
        [owner.account_id]
    );
}

#[tokio::test]
async fn without_a_reachable_media_server_nobody_joins_and_the_check_resumes_on_return() {
    let without = TestServer::start().await;
    let owner = without.owner().await;
    let workspace_id = without.default_workspace(&owner).await.id;
    let voice = without.voice(&owner, workspace_id, "lounge").await;
    let refused = without.call_join_reply(&owner, voice.id).await;
    refused.error(ErrorCode::MediaUnavailable);
    assert_eq!(refused.retry_after.as_deref(), Some("2"));
    let health: Health = without
        .send::<Value>(reqwest::Method::GET, "/api/v1/health", None, None, None)
        .await
        .ok(StatusCode::OK);
    assert_eq!(health.media_server, None);
    assert_eq!(without.check().await, Outcome::NoMediaServer);

    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let health = || async {
        server
            .send::<Value>(reqwest::Method::GET, "/api/v1/health", None, None, None)
            .await
            .ok::<Health>(StatusCode::OK)
    };
    assert_eq!(health().await.media_server, Some(true));
    let ann_seat = server.enter_call(ann, scene.voice.id).await;
    let ben_seat = server.enter_call(ben, scene.voice.id).await;

    server.fake().set_unreachable(true);
    assert_eq!(health().await.media_server, Some(true));
    assert_eq!(server.check().await, Outcome::Unreachable);
    let health_now = health().await;
    assert_eq!(
        (health_now.database, health_now.media_server),
        (true, Some(false))
    );
    server
        .call_join_reply(owner, scene.voice.id)
        .await
        .error(ErrorCode::MediaUnavailable);
    server
        .post(
            owner,
            &format!("/api/v1/direct-calls/{}/join", ann.account_id),
            &Value::Null,
        )
        .await
        .error(ErrorCode::MediaUnavailable);
    assert_eq!(server.count("call_members").await, 2);

    server
        .put_empty(owner, &format!("/api/v1/accounts/{}/ban", ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.clock.advance(Duration::minutes(10));
    assert_eq!(server.check().await, Outcome::Unreachable);
    assert_eq!(server.count("call_members").await, 1);
    assert_eq!(server.fake().identities(&ann_seat.room).len(), 2);
    assert!(server.fake().removed().is_empty());

    server.fake().set_unreachable(false);
    assert_eq!(server.check().await, removed(1));
    assert_eq!(
        server.fake().identities(&ann_seat.room),
        std::slice::from_ref(&ann_seat.identity)
    );
    assert_eq!(
        server.fake().removed(),
        [(ben_seat.room.clone(), ben_seat.identity.clone())]
    );
    assert_eq!(health().await.media_server, Some(true));
    assert_eq!(
        server.occupants(owner, scene.voice.id).await,
        [ann.account_id]
    );
    server.join_call(owner, scene.voice.id).await;
}

#[tokio::test]
async fn a_place_that_is_not_taken_while_the_token_is_usable_is_given_up() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let mut to_ann = server.full_stream(ann).await;
    let mut to_ben = server.full_stream(ben).await;
    let join = server.join_call(ann, scene.voice.id).await;
    server.call_events(ann, &mut to_ann).await;

    server.clock.advance(Duration::seconds(119));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(server.count("call_members").await, 1);
    server.clock.advance(Duration::seconds(1));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(server.count("call_members").await, 0);
    assert_eq!(server.count("calls").await, 0);
    assert_eq!(
        server.call_events(ann, &mut to_ann).await,
        [Event::OwnCallChanged {
            call: None,
            left: Some(LeaveReason::NeverConnected),
        }]
    );
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());

    server.clock.advance(Duration::seconds(1));
    assert!(server.fake().connect(&join.token).is_err());
    let again = server.join_call(ann, scene.voice.id).await;
    assert_ne!(
        claims(&again)["video"]["room"],
        claims(&join)["video"]["room"]
    );
}

#[tokio::test]
async fn a_person_whom_the_media_server_no_longer_lists_has_left_after_twenty_seconds() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let seat = server.enter_call(ann, scene.voice.id).await;
    server.enter_call(ben, scene.voice.id).await;
    let mut to_ben = server.full_stream(ben).await;

    server.fake().leave(&seat.room, &seat.identity);
    assert_eq!(server.check().await, CLEAN);
    server.clock.advance(Duration::seconds(19));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(
        server.occupants(ben, scene.voice.id).await,
        [ann.account_id, ben.account_id]
    );
    server.fake().connect(&seat.join.token).unwrap();
    assert_eq!(server.check().await, CLEAN);
    server.clock.advance(Duration::seconds(30));
    assert_eq!(server.check().await, CLEAN);
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());

    server.fake().leave(&seat.room, &seat.identity);
    assert_eq!(server.check().await, CLEAN);
    server.clock.advance(Duration::seconds(20));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(
        server.call_events(ben, &mut to_ben).await,
        [Event::CallParticipantLeft {
            call_id: seat.join.call.id,
            place: seat.join.call.place,
            account_id: ann.account_id,
            reason: LeaveReason::ConnectionLost,
        }]
    );
    server
        .fake()
        .enter(&seat.room, &seat.identity, ParticipantKind::Standard, None);
    assert_eq!(server.check().await, removed(1));
}

/// Each way in which an account loses its place takes effect through the admission check.
#[tokio::test]
async fn removal_ban_session_end_and_a_deleted_channel_end_the_place_in_the_room() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    server
        .grant_workspace(
            owner,
            ann,
            scene.workspace_id,
            &[WorkspacePermission::DisconnectMembers],
        )
        .await;
    let anchor = server.enter_call(owner, scene.voice.id).await;
    let room = anchor.room.clone();

    let disconnected = server.join(owner, "Disconnected").await;
    let seat = server.enter_call(&disconnected, scene.voice.id).await;
    let kick = format!(
        "/api/v1/channels/{}/call/participants/{}",
        scene.voice.id, disconnected.account_id
    );
    server
        .delete(&scene.ben, &kick)
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(ann, &kick)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.delete(ann, &kick).await.error(ErrorCode::NotFound);
    assert_eq!(server.check().await, removed(1));
    assert!(!server.fake().identities(&room).contains(&seat.identity));
    let log: AuditPage = server
        .get(
            owner,
            &format!("/api/v1/workspaces/{}/audit-log", scene.workspace_id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        log.entries[0].event,
        AuditEvent::CallMemberDisconnected {
            account_id: disconnected.account_id
        }
    );
    assert_eq!(log.entries[0].actor_id, Some(ann.account_id));
    assert_eq!(log.entries[0].channel_id, Some(scene.voice.id));

    let banned = server.join(owner, "Banned").await;
    let removed_account = server.join(owner, "Removed").await;
    let logged_out = server.join(owner, "LoggedOut").await;
    let expired = server.join(owner, "Expired").await;
    let other_device = server.join(owner, "OtherDevice").await;
    let mut seats = Vec::new();
    for person in [
        &banned,
        &removed_account,
        &logged_out,
        &expired,
        &other_device,
    ] {
        seats.push(server.enter_call(person, scene.voice.id).await);
    }
    assert_eq!(server.occupants(owner, scene.voice.id).await.len(), 6);

    server
        .put_empty(
            owner,
            &format!("/api/v1/accounts/{}/ban", banned.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.check().await, removed(1));
    server
        .post(
            owner,
            &format!("/api/v1/accounts/{}/removal", removed_account.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.check().await, removed(1));
    server
        .delete(&logged_out, "/api/v1/auth/session")
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.check().await, removed(1));
    assert_eq!(server.occupants(owner, scene.voice.id).await.len(), 3);

    sqlx::query("update sessions set expires_at = $2 where device_id = $1")
        .bind(expired.device_id.as_uuid())
        .bind(server.clock.now())
        .execute(&server.db)
        .await
        .unwrap();
    assert_eq!(server.check().await, removed(1));

    let second_key = DeviceKey::generate();
    let code = crate::devices::request_link(server, &second_key, "second").await;
    server
        .post(
            &other_device,
            "/api/v1/account/device-links/approve",
            &json!({ "code": code.code, "public_key": second_key.public() }),
        )
        .await
        .ok::<Value>(StatusCode::CREATED);
    let session = server.login(&second_key).await;
    let second = member(second_key, session);
    server
        .delete(
            &second,
            &format!("/api/v1/account/devices/{}", other_device.device_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.check().await, removed(1));
    assert_eq!(
        server.occupants(owner, scene.voice.id).await,
        [owner.account_id]
    );
    assert_eq!(
        server.fake().identities(&room),
        std::slice::from_ref(&anchor.identity)
    );

    server
        .delete(owner, &format!("/api/v1/channels/{}", scene.voice.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.check().await, removed(1));
    assert!(server.fake().identities(&room).is_empty());
    assert_eq!(server.count("calls").await, 0);
    assert_eq!(server.own_call(owner).await.call, None);
    for seat in seats {
        server
            .fake()
            .enter(&seat.room, &seat.identity, ParticipantKind::Standard, None);
    }
    assert_eq!(server.check().await, removed(5));
    assert!(server.fake().room_names().is_empty());
}

#[tokio::test]
async fn an_account_is_in_one_call_from_one_device() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let other = server.voice(owner, scene.workspace_id, "garden").await;
    let first = server.enter_call(ann, scene.voice.id).await;
    server.enter_call(ben, scene.voice.id).await;
    let mut to_ben = server.full_stream(ben).await;

    let again = server.join_call(ann, scene.voice.id).await;
    assert_eq!(again.identity, first.identity);
    assert_eq!(server.check().await, CLEAN);
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());

    let second_key = DeviceKey::generate();
    let code = crate::devices::request_link(server, &second_key, "phone").await;
    server
        .post(
            ann,
            "/api/v1/account/device-links/approve",
            &json!({ "code": code.code, "public_key": second_key.public() }),
        )
        .await
        .ok::<Value>(StatusCode::CREATED);
    let session = server.login(&second_key).await;
    let phone = member(second_key, session);
    let mut to_desk = server.full_stream(ann).await;

    server
        .put(
            ann,
            "/api/v1/call/state",
            &json!({ "muted": true, "deafened": false }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put(
            &phone,
            "/api/v1/call/state",
            &json!({ "muted": false, "deafened": false }),
        )
        .await
        .error(ErrorCode::NotInCall);

    let moved = server.join_call(&phone, scene.voice.id).await;
    assert_ne!(moved.identity, first.identity);
    assert_eq!(moved.call.id, first.join.call.id);
    let told = server.call_events(ann, &mut to_desk).await;
    assert_eq!(
        names(&told),
        [
            "call_participant_updated",
            "call_participant_left",
            "own_call_changed"
        ]
    );
    assert!(matches!(
        &told[2],
        Event::OwnCallChanged { call: Some(own), left: Some(LeaveReason::Switched) }
            if own.device_id == phone.device_id && own.identity == moved.identity
    ));
    assert_eq!(server.count("call_members").await, 2);
    server
        .put(
            ann,
            "/api/v1/call/state",
            &json!({ "muted": true, "deafened": false }),
        )
        .await
        .error(ErrorCode::NotInCall);

    assert_eq!(server.check().await, removed(1));
    assert!(
        !server
            .fake()
            .identities(&first.room)
            .contains(&first.identity)
    );
    server.fake().connect(&first.join.token).unwrap();
    let phone_seat = server.connect(moved);
    assert_eq!(server.check().await, removed(1));
    assert_eq!(server.fake().identities(&first.room).len(), 2);
    assert!(
        server
            .fake()
            .identities(&first.room)
            .contains(&phone_seat.identity)
    );
    let seen = server.call_events(ben, &mut to_ben).await;
    assert_eq!(
        names(&seen),
        [
            "call_participant_updated",
            "call_participant_left",
            "call_participant_joined"
        ]
    );
    let Event::CallParticipantJoined { participant, .. } = &seen[2] else {
        panic!("{seen:?}");
    };
    assert!(!participant.muted);

    let elsewhere = server.join_call(&phone, other.id).await;
    assert_ne!(elsewhere.call.id, first.join.call.id);
    assert_eq!(server.count("call_members").await, 2);
    assert_eq!(
        server.own_call(ann).await.call.unwrap().own.call_id,
        elsewhere.call.id
    );
    assert_eq!(server.check().await, removed(1));
    assert_eq!(
        server.occupants(ben, scene.voice.id).await,
        [ben.account_id]
    );

    let direct = sqlx::query(
        "insert into call_members
             (call_id, account_id, device_id, session_id, identity, admitted_at, usable_until)
         select $1, account_id, device_id, session_id, account_id::text || ':' || repeat('0', 32),
                admitted_at, usable_until
         from call_members where account_id = $2",
    )
    .bind(first.join.call.id.as_uuid())
    .bind(ann.account_id.as_uuid())
    .execute(&server.db)
    .await;
    let refused = direct.unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("call_members_one_call_per_account"),
        "{refused}"
    );
    let stolen = sqlx::query("update call_members set identity = $1 where account_id = $2")
        .bind(format!("{}:{}", ben.account_id, "0".repeat(32)))
        .bind(ann.account_id.as_uuid())
        .execute(&server.db)
        .await;
    assert!(stolen.is_err());
}

#[tokio::test]
async fn a_user_limit_and_a_lock_stop_the_entry_and_the_lock_ends_with_the_call() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let settings = format!("/api/v1/channels/{voice}/call/settings");
    server
        .patch(ann, &settings, &json!({ "user_limit": 2 }))
        .await
        .error(ErrorCode::Forbidden);
    for invalid in [json!({ "user_limit": 0 }), json!({ "user_limit": 100 })] {
        server
            .patch(owner, &settings, &invalid)
            .await
            .error(ErrorCode::InvalidRequest);
    }
    let text = server.channel(owner, scene.workspace_id, "notes").await;
    server
        .patch(
            owner,
            &format!("/api/v1/channels/{}/call/settings", text.id),
            &json!({ "user_limit": 2 }),
        )
        .await
        .error(ErrorCode::Conflict);
    let on_text = sqlx::query("update channels set user_limit = 2 where id = $1")
        .bind(text.id.as_uuid())
        .execute(&server.db)
        .await;
    assert!(on_text.is_err());
    server
        .call_settings(owner, voice, json!({ "user_limit": 2 }))
        .await;

    server.enter_call(ann, voice).await;
    server.join_call(ben, voice).await;
    server
        .call_join_reply(owner, voice)
        .await
        .error(ErrorCode::CallFull);
    server.join_call(ben, voice).await;
    server
        .call_settings(owner, voice, json!({ "user_limit": null }))
        .await;
    let owner_seat = server.enter_call(owner, voice).await;
    server.leave_call(owner).await;
    server.leave_call(ben).await;
    assert_eq!(server.check().await, removed(1));

    let lock = format!("/api/v1/channels/{voice}/call/lock");
    let mut to_owner = server.full_stream(owner).await;
    server
        .put_empty(ben, &lock)
        .await
        .error(ErrorCode::NotInCall);
    let everyone = server.everyone_role(owner, scene.workspace_id).await;
    set_override(
        server,
        owner,
        voice,
        everyone.id,
        &[],
        &[WorkspacePermission::LockRoom],
    )
    .await
    .ok::<Value>(StatusCode::OK);
    server
        .put_empty(ann, &lock)
        .await
        .error(ErrorCode::Forbidden);
    set_override(server, owner, voice, everyone.id, &[], &[])
        .await
        .ok::<Value>(StatusCode::OK);
    server.call_events(owner, &mut to_owner).await;
    server
        .put_empty(ann, &lock)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(ann, &lock)
        .await
        .empty(StatusCode::NO_CONTENT);
    let locked = server.call_events(owner, &mut to_owner).await;
    assert!(matches!(
        locked.as_slice(),
        [Event::CallLockChanged { lock: Some(lock), .. }] if lock.by == ann.account_id
    ));
    assert_eq!(
        server
            .channel_call(owner, voice)
            .await
            .call
            .unwrap()
            .lock
            .unwrap()
            .by,
        ann.account_id
    );
    server
        .call_join_reply(ben, voice)
        .await
        .error(ErrorCode::CallLocked);
    server
        .call_join_reply(owner, voice)
        .await
        .error(ErrorCode::CallLocked);
    server.fake().connect(&owner_seat.join.token).unwrap();
    assert_eq!(server.check().await, removed(1));
    server.join_call(ann, voice).await;

    server
        .delete(ann, &lock)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(matches!(
        server.call_events(owner, &mut to_owner).await.as_slice(),
        [Event::CallLockChanged { lock: None, .. }]
    ));
    server.join_call(ben, voice).await;
    server.leave_call(ben).await;
    server
        .put_empty(ann, &lock)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.leave_call(ann).await;
    assert_eq!(
        names(&server.call_events(owner, &mut to_owner).await),
        ["call_lock_changed", "call_participant_left", "call_ended"]
    );
    assert_eq!(server.count("calls").await, 0);
    let fresh = server.join_call(ben, voice).await;
    assert_eq!(fresh.call.lock, None);
    let on_direct = sqlx::query(
        "insert into calls (id, room_name, direct_low, direct_high, created_at, locked_by, locked_at)
         values ($1, $2, $3, $4, now(), $3, now())",
    )
    .bind(uuid::Uuid::now_v7())
    .bind(format!("jr-{}-{}", "0".repeat(12), "0".repeat(32)))
    .bind(ann.account_id.as_uuid().min(ben.account_id.as_uuid()))
    .bind(ann.account_id.as_uuid().max(ben.account_id.as_uuid()))
    .execute(&server.db)
    .await;
    assert!(on_direct.is_err());
}

#[tokio::test]
async fn two_people_do_not_both_take_the_last_place() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    server
        .call_settings(owner, scene.voice.id, json!({ "user_limit": 2 }))
        .await;
    server.enter_call(owner, scene.voice.id).await;

    server.gate().arm(0);
    let mut first = Box::pin(server.call_join_reply(ann, scene.voice.id));
    drive_until(&mut first, server.gate().arrived()).await;
    let mut second = Box::pin(server.call_join_reply(ben, scene.voice.id));
    drive_until(&mut second, server.until_blocked()).await;
    server.gate().release();
    first.await.ok::<Value>(StatusCode::OK);
    second.await.error(ErrorCode::CallFull);
    assert_eq!(server.count("call_members").await, 2);
    assert_eq!(server.own_call(ben).await.call, None);
}

#[tokio::test]
async fn a_lock_that_is_set_while_a_person_joins_either_stops_the_join_or_follows_it() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lock = format!("/api/v1/channels/{}/call/lock", scene.voice.id);
    server.enter_call(owner, scene.voice.id).await;

    server.gate().arm(0);
    let mut locking = Box::pin(server.put_empty(owner, &lock));
    drive_until(&mut locking, server.gate().arrived()).await;
    let mut joining = Box::pin(server.call_join_reply(ann, scene.voice.id));
    drive_until(&mut joining, server.until_blocked()).await;
    server.gate().release();
    locking.await.empty(StatusCode::NO_CONTENT);
    joining.await.error(ErrorCode::CallLocked);
    assert_eq!(server.count("call_members").await, 1);

    server
        .delete(owner, &lock)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.gate().arm(0);
    let mut joining = Box::pin(server.call_join_reply(ben, scene.voice.id));
    drive_until(&mut joining, server.gate().arrived()).await;
    let mut locking = Box::pin(server.put_empty(owner, &lock));
    drive_until(&mut locking, server.until_blocked()).await;
    server.gate().release();
    joining.await.ok::<Value>(StatusCode::OK);
    locking.await.empty(StatusCode::NO_CONTENT);
    assert_eq!(server.count("call_members").await, 2);
    assert!(
        server
            .channel_call(owner, scene.voice.id)
            .await
            .call
            .unwrap()
            .lock
            .is_some()
    );
    server
        .call_join_reply(ann, scene.voice.id)
        .await
        .error(ErrorCode::CallLocked);
}

#[tokio::test]
async fn a_ban_during_a_join_leaves_no_place_in_the_room() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    server.enter_call(owner, scene.voice.id).await;
    let ban = format!("/api/v1/accounts/{}/ban", ann.account_id);

    server.gate().arm(0);
    let mut joining = Box::pin(server.call_join_reply(ann, scene.voice.id));
    drive_until(&mut joining, server.gate().arrived()).await;
    let banning = server.put_empty(owner, &ban);
    let (banned, joined) = tokio::join!(banning, async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        server.gate().release();
        joining.await
    });
    banned.empty(StatusCode::NO_CONTENT);
    let token = match joined.status {
        StatusCode::OK => Some(joined.ok::<junto_protocol::call::CallJoin>(StatusCode::OK)),
        _ => {
            joined.error(ErrorCode::Unauthenticated);
            None
        }
    };
    if let Some(join) = token {
        server.fake().connect(&join.token).unwrap();
    }
    server.check().await;
    assert_eq!(server.count("call_members").await, 1);
    assert_eq!(
        server.occupants(owner, scene.voice.id).await,
        [owner.account_id]
    );
    assert_eq!(server.fake().room_names().len(), 1);
    assert_eq!(
        server
            .fake()
            .participants(&server.fake().room_names()[0])
            .len(),
        1
    );
}

#[tokio::test]
async fn two_devices_of_one_account_that_join_at_once_leave_one_place() {
    let scene = Scene::start().await;
    let (server, ann) = (&scene.server, &scene.ann);
    let second_key = DeviceKey::generate();
    let code = crate::devices::request_link(server, &second_key, "phone").await;
    server
        .post(
            ann,
            "/api/v1/account/device-links/approve",
            &json!({ "code": code.code, "public_key": second_key.public() }),
        )
        .await
        .ok::<Value>(StatusCode::CREATED);
    let session = server.login(&second_key).await;
    let phone = member(second_key, session);

    server.gate().arm(0);
    let mut desk_joins = Box::pin(server.call_join_reply(ann, scene.voice.id));
    drive_until(&mut desk_joins, server.gate().arrived()).await;
    let mut phone_joins = Box::pin(server.call_join_reply(&phone, scene.voice.id));
    drive_until(&mut phone_joins, server.until_blocked()).await;
    server.gate().release();
    let desk: junto_protocol::call::CallJoin = desk_joins.await.ok(StatusCode::OK);
    let on_phone: junto_protocol::call::CallJoin = phone_joins.await.ok(StatusCode::OK);
    assert_ne!(desk.identity, on_phone.identity);
    assert_eq!(server.count("call_members").await, 1);
    assert_eq!(server.count("calls").await, 1);
    let own = server.own_call(ann).await.call.unwrap().own;
    assert_eq!(
        (own.device_id, own.identity),
        (phone.device_id, on_phone.identity.clone())
    );

    server.fake().connect(&desk.token).unwrap();
    server.connect(on_phone);
    assert_eq!(server.check().await, removed(1));
    assert_eq!(
        server.occupants(ann, scene.voice.id).await,
        [ann.account_id]
    );
}

#[tokio::test]
async fn the_requests_of_one_session_that_change_calls_are_limited() {
    let server = TestServer::start_for_calls_with(|config| {
        config.limits.calls.actions = junto_server::config::RateLimit {
            attempts: 5,
            window: Duration::minutes(1),
        };
    })
    .await;
    let scene = Scene::on(server).await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    for _ in 0..5 {
        server.join_call(ann, scene.voice.id).await;
    }
    let limited = server.call_join_reply(ann, scene.voice.id).await;
    limited.error(ErrorCode::RateLimited);
    assert_eq!(limited.retry_after.as_deref(), Some("60"));
    server
        .put_empty(
            ann,
            &format!("/api/v1/channels/{}/call/lock", scene.voice.id),
        )
        .await
        .error(ErrorCode::RateLimited);
    server.join_call(ben, scene.voice.id).await;
    server.clock.advance(Duration::minutes(1));
    server.join_call(ann, scene.voice.id).await;
}
