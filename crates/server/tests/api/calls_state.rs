//! Blocks and own state, meeting mode, screen share, speaking marks, direct-message calls and
//! the events of a call (ADR 0502, ADR 0503, ADR 0505, ADR 0507).

use std::time::{Duration as Wait, Instant};

use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::call::{
    BlockReason, CallInvitation, CallJoin, CallParticipant, CallPlace, InvitationOutcome,
    LeaveReason, MAX_SHARES, MeetingEnd, MicrophoneBlock, SHOWN_SHARES_ATTRIBUTE, ScreenShare,
    ShareAudio, ShareKind, SoundBlock,
};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::{AccountId, ChannelId};
use junto_protocol::{WorkspacePermission, WorkspacePermission as W};
use junto_server::calls::admission::Outcome;
use junto_server::calls::speaking;
use junto_server::media::{ParticipantKind, Publishing, TrackKind, TrackSource};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use time::Duration;

use crate::calls_support::{Scene, Seat, claims, names};
use crate::structure::set_override;
use crate::support::{Member, Reply, TestServer, drive_until};

const CLEAN: Outcome = Outcome::Checked {
    removed: 0,
    updated: 0,
};

fn updated(count: usize) -> Outcome {
    Outcome::Checked {
        removed: 0,
        updated: count,
    }
}

fn block_path(channel_id: ChannelId, account_id: AccountId, kind: &str) -> String {
    format!("/api/v1/channels/{channel_id}/call/participants/{account_id}/{kind}")
}

async fn participant(
    server: &TestServer,
    viewer: &Member,
    channel_id: ChannelId,
    account_id: AccountId,
) -> CallParticipant {
    server
        .channel_call(viewer, channel_id)
        .await
        .call
        .unwrap()
        .participants
        .into_iter()
        .find(|participant| participant.account_id == account_id)
        .unwrap()
}

async fn report_state(server: &TestServer, member: &Member, muted: bool, deafened: bool) -> Reply {
    let body = json!({ "muted": muted, "deafened": deafened });
    server.put(member, "/api/v1/call/state", &body).await
}

fn speaks(server: &TestServer, seat: &Seat) -> bool {
    server.fake().publish(
        &seat.room,
        &seat.identity,
        TrackKind::Audio,
        TrackSource::Microphone,
    )
}

#[tokio::test]
async fn a_microphone_block_is_enforced_and_is_separate_from_the_own_setting() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let ann_seat = server.enter_call(ann, voice).await;
    let ben_seat = server.enter_call(ben, voice).await;
    assert!(speaks(server, &ben_seat));
    let mut to_ann = server.full_stream(ann).await;
    let block = block_path(voice, ben.account_id, "microphone-block");

    server
        .put_empty(ann, &block)
        .await
        .error(ErrorCode::Forbidden);
    server.delete(ann, &block).await.error(ErrorCode::Forbidden);
    let absent = block_path(voice, owner.account_id, "microphone-block");
    server
        .put_empty(owner, &absent)
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(owner, &block)
        .await
        .error(ErrorCode::NotFound);

    report_state(server, ben, false, false)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(server.call_events(ann, &mut to_ann).await.is_empty());
    server
        .put_empty(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    let moderator_block = Some(MicrophoneBlock {
        by: owner.account_id,
        reason: BlockReason::Moderator,
    });
    let blocked = server.call_events(ann, &mut to_ann).await;
    assert!(matches!(
        blocked.as_slice(),
        [Event::CallParticipantUpdated { participant, .. }]
            if participant.microphone_block == moderator_block && !participant.muted
    ));

    assert!(server.fake().participants(&ben_seat.room)[1].publishes(TrackSource::Microphone));
    assert_eq!(server.check().await, updated(1));
    assert_eq!(
        server.fake().updated(),
        [(
            ben_seat.room.clone(),
            ben_seat.identity.clone(),
            Publishing::pictures_only()
        )]
    );
    assert!(!server.fake().participants(&ben_seat.room)[1].publishes(TrackSource::Microphone));
    assert!(!speaks(server, &ben_seat));
    assert!(!server.fake().publish(
        &ben_seat.room,
        &ben_seat.identity,
        TrackKind::Audio,
        TrackSource::ScreenShareAudio
    ));
    assert!(server.fake().publish(
        &ben_seat.room,
        &ben_seat.identity,
        TrackKind::Video,
        TrackSource::Camera
    ));
    assert!(speaks(server, &ann_seat));
    assert_eq!(server.check().await, CLEAN);

    report_state(server, ben, true, false)
        .await
        .empty(StatusCode::NO_CONTENT);
    let shown = participant(server, ann, voice, ben.account_id).await;
    assert!(shown.muted);
    assert_eq!(shown.microphone_block, moderator_block);

    let token_again = server.fake().connect(&ben_seat.join.token).unwrap();
    assert_eq!(token_again.1, ben_seat.identity);
    assert!(speaks(server, &ben_seat));
    assert_eq!(server.check().await, updated(1));
    assert!(!speaks(server, &ben_seat));

    server.leave_call(ben).await;
    server.check().await;
    let rejoined = server.join_call(ben, voice).await;
    assert_eq!(
        claims(&rejoined)["video"]["canPublishSources"],
        json!(["camera", "screen_share"])
    );
    let ben_seat = server.connect(rejoined);
    assert_eq!(server.check().await, CLEAN);
    assert!(!speaks(server, &ben_seat));
    let shown = participant(server, ann, voice, ben.account_id).await;
    assert_eq!(shown.microphone_block, moderator_block);
    assert!(!shown.muted);

    server.fake().force_track(
        &ben_seat.room,
        &ben_seat.identity,
        TrackKind::Audio,
        TrackSource::Camera,
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
            .identities(&ben_seat.room)
            .contains(&ben_seat.identity)
    );
    let ben_seat = server.connect(server.join_call(ben, voice).await);
    server.check().await;

    report_state(server, ben, true, false)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.call_events(ann, &mut to_ann).await;
    server
        .delete(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    let unblocked = server.call_events(ann, &mut to_ann).await;
    assert!(matches!(
        unblocked.as_slice(),
        [Event::CallParticipantUpdated { participant, .. }]
            if participant.microphone_block.is_none() && participant.muted
    ));
    assert_eq!(server.check().await, updated(1));
    assert!(speaks(server, &ben_seat));
    server
        .delete(owner, &block)
        .await
        .error(ErrorCode::NotFound);

    let log: AuditPage = server
        .get(
            owner,
            &format!("/api/v1/workspaces/{}/audit-log", scene.workspace_id),
        )
        .await
        .ok(StatusCode::OK);
    let account_id = ben.account_id;
    assert_eq!(
        [&log.entries[0].event, &log.entries[1].event],
        [
            &AuditEvent::CallMicrophoneUnblocked { account_id },
            &AuditEvent::CallMicrophoneBlocked { account_id }
        ]
    );
    assert_eq!(log.entries[0].channel_id, Some(voice));
}

#[tokio::test]
async fn a_sound_block_is_a_flag_for_the_client_and_is_written_to_the_audit_log() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server
        .grant_workspace(owner, ann, scene.workspace_id, &[W::DeafenMembers])
        .await;
    server.enter_call(ann, voice).await;
    server.enter_call(ben, voice).await;
    let block = block_path(voice, ben.account_id, "sound-block");
    let microphone = block_path(voice, ben.account_id, "microphone-block");

    server
        .put_empty(ben, &block)
        .await
        .error(ErrorCode::Forbidden);
    server
        .put_empty(ann, &microphone)
        .await
        .error(ErrorCode::Forbidden);
    report_state(server, ben, false, true)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(ann, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    let shown = participant(server, owner, voice, ben.account_id).await;
    assert!(shown.deafened);
    assert_eq!(shown.sound_block, Some(SoundBlock { by: ann.account_id }));
    assert_eq!(shown.microphone_block, None);
    assert_eq!(server.check().await, CLEAN);
    assert!(server.fake().updated().is_empty());

    report_state(server, ben, false, false)
        .await
        .empty(StatusCode::NO_CONTENT);
    let shown = participant(server, owner, voice, ben.account_id).await;
    assert!(!shown.deafened);
    assert!(shown.sound_block.is_some());
    server
        .delete(ann, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    let shown = participant(server, owner, voice, ben.account_id).await;
    assert_eq!((shown.deafened, shown.sound_block), (false, None));
    let log: AuditPage = server
        .get(
            owner,
            &format!("/api/v1/workspaces/{}/audit-log", scene.workspace_id),
        )
        .await
        .ok(StatusCode::OK);
    let account_id = ben.account_id;
    assert_eq!(
        [&log.entries[0].event, &log.entries[1].event],
        [
            &AuditEvent::CallSoundUnblocked { account_id },
            &AuditEvent::CallSoundBlocked { account_id }
        ]
    );
    assert_eq!(log.entries[1].actor_id, Some(ann.account_id));

    for body in [json!({}), json!({ "muted": "yes", "deafened": false })] {
        server
            .put(ben, "/api/v1/call/state", &body)
            .await
            .error(ErrorCode::InvalidRequest);
    }
    report_state(server, owner, true, true)
        .await
        .error(ErrorCode::NotInCall);
    for _ in 0..40 {
        let reply = report_state(server, ben, true, true).await;
        if reply.status != StatusCode::NO_CONTENT {
            reply.error(ErrorCode::RateLimited);
            return;
        }
    }
    panic!("the reports of a device are not limited");
}

async fn share(server: &TestServer, member: &Member, seat: &Seat, audio: &str) {
    server
        .put(
            member,
            "/api/v1/call/screen-share",
            &json!({ "audio": audio }),
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

#[tokio::test]
async fn a_screen_share_is_shown_with_its_sound_choice_while_the_media_server_lists_it() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let seat = server.enter_call(ann, voice).await;
    let mut to_ben = server.full_stream(ben).await;
    let path = "/api/v1/call/screen-share";

    for body in [
        json!({}),
        json!({ "audio": "microphone" }),
        json!({ "audio": null }),
    ] {
        server
            .put(ann, path, &body)
            .await
            .error(ErrorCode::InvalidRequest);
    }
    server
        .put(ben, path, &json!({ "audio": "none" }))
        .await
        .error(ErrorCode::NotInCall);
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());

    share(server, ann, &seat, "application").await;
    let started_at = server.clock.now();
    let shown = Some(ScreenShare {
        slot: 0,
        kind: None,
        audio: ShareAudio::Application,
        started_at,
    });
    assert!(matches!(
        server.call_events(ben, &mut to_ben).await.as_slice(),
        [Event::CallParticipantUpdated { participant, .. }] if participant.screen_share == shown
    ));
    server.clock.advance(Duration::minutes(5));
    assert_eq!(server.check().await, CLEAN);
    server
        .put(ann, path, &json!({ "audio": "system" }))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put(ann, path, &json!({ "audio": "system" }))
        .await
        .empty(StatusCode::NO_CONTENT);
    let changed = Some(ScreenShare {
        slot: 0,
        kind: None,
        audio: ShareAudio::System,
        started_at,
    });
    assert!(matches!(
        server.call_events(ben, &mut to_ben).await.as_slice(),
        [Event::CallParticipantUpdated { participant, .. }] if participant.screen_share == changed
    ));
    assert_eq!(
        participant(server, ben, voice, ann.account_id)
            .await
            .screen_share,
        changed
    );

    server
        .fake()
        .unpublish(&seat.room, &seat.identity, TrackSource::ScreenShare);
    assert_eq!(server.check().await, CLEAN);
    server.clock.advance(Duration::seconds(9));
    assert_eq!(server.check().await, CLEAN);
    assert!(
        participant(server, ben, voice, ann.account_id)
            .await
            .screen_share
            .is_some()
    );
    server.clock.advance(Duration::seconds(1));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(
        participant(server, ben, voice, ann.account_id)
            .await
            .screen_share,
        None
    );
    assert!(matches!(
        server.call_events(ben, &mut to_ben).await.as_slice(),
        [Event::CallParticipantUpdated { participant, .. }] if participant.screen_share.is_none()
    ));

    server
        .put(ann, path, &json!({ "audio": "none" }))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        participant(server, ben, voice, ann.account_id)
            .await
            .screen_share
            .unwrap()
            .audio,
        ShareAudio::None
    );
    server.delete(ann, path).await.empty(StatusCode::NO_CONTENT);
    server.delete(ann, path).await.empty(StatusCode::NO_CONTENT);
    assert_eq!(
        participant(server, ben, voice, ann.account_id)
            .await
            .screen_share,
        None
    );
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["call_participant_updated", "call_participant_updated"]
    );
}

/// Reports a share of a slot, as a client of several shares does, and publishes its picture
/// track with the name that states the slot (ADR 0737).
async fn share_in(server: &TestServer, member: &Member, seat: &Seat, slot: u8, kind: &str) {
    server
        .put(
            member,
            "/api/v1/call/screen-share",
            &json!({ "audio": "system", "slot": slot, "kind": kind }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(server.fake().publish_named(
        &seat.room,
        &seat.identity,
        TrackKind::Video,
        TrackSource::ScreenShare,
        &share_track(slot, kind),
    ));
    server.check().await;
}

fn share_track(slot: u8, kind: &str) -> String {
    if slot == 0 {
        format!("{kind}/30")
    } else {
        format!("{kind}/30/{slot}")
    }
}

async fn shares_of(server: &TestServer, viewer: &Member, voice: ChannelId, of: &Member) -> Vec<u8> {
    participant(server, viewer, voice, of.account_id)
        .await
        .screen_shares
        .iter()
        .map(|share| share.slot)
        .collect()
}

#[tokio::test]
async fn a_person_runs_up_to_three_shares_and_each_lasts_while_its_named_track_is_listed() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let seat = server.enter_call(ann, voice).await;
    let mut to_ben = server.full_stream(ben).await;
    let path = "/api/v1/call/screen-share";

    share_in(server, ann, &seat, 0, "screen").await;
    share_in(server, ann, &seat, 1, "window").await;
    let started_at = server.clock.now();
    let shown = participant(server, ben, voice, ann.account_id).await;
    let share = |slot, kind| ScreenShare {
        slot,
        kind: Some(kind),
        audio: ShareAudio::System,
        started_at,
    };
    assert_eq!(
        shown.screen_shares,
        [share(0, ShareKind::Screen), share(1, ShareKind::Window)]
    );
    // A client of one share reads the share of the lowest slot.
    assert_eq!(shown.screen_share, Some(share(0, ShareKind::Screen)));
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["call_participant_updated", "call_participant_updated"]
    );
    let lines: i64 = sqlx::query_scalar(
        "select count(*) from call_notes where kind = 'screen_share_started' and account_id = $1",
    )
    .bind(ann.account_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(lines, 2);

    // A fourth share has no slot.
    for slot in [3, 255] {
        server
            .put(ann, path, &json!({ "audio": "none", "slot": slot }))
            .await
            .error(ErrorCode::InvalidRequest);
    }
    share_in(server, ann, &seat, 2, "screen").await;
    assert_eq!(shares_of(server, ben, voice, ann).await, [0, 1, 2]);

    // The track of slot 1 ends: only that share ends after the time of grace, while the
    // other tracks of the person stay listed.
    server.fake().unpublish_named(
        &seat.room,
        &seat.identity,
        TrackSource::ScreenShare,
        &share_track(1, "window"),
    );
    server.call_events(ben, &mut to_ben).await;
    assert_eq!(server.check().await, CLEAN);
    server.clock.advance(Duration::seconds(10));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(shares_of(server, ben, voice, ann).await, [0, 2]);
    assert!(matches!(
        server.call_events(ben, &mut to_ben).await.as_slice(),
        [Event::CallParticipantUpdated { participant, .. }]
            if participant.screen_shares.len() == 2
    ));

    // The end of one slot leaves the others; an end without a slot ends every share.
    server
        .delete(ann, &format!("{path}?slot=0"))
        .await
        .empty(StatusCode::NO_CONTENT);
    let shown = participant(server, ben, voice, ann.account_id).await;
    assert_eq!(shown.screen_shares.len(), 1);
    assert_eq!(shown.screen_share.map(|share| share.slot), Some(2));
    server
        .delete(ann, &format!("{path}?slot=0"))
        .await
        .empty(StatusCode::NO_CONTENT);
    share_in(server, ann, &seat, 0, "window").await;
    server.delete(ann, path).await.empty(StatusCode::NO_CONTENT);
    let shown = participant(server, ben, voice, ann.account_id).await;
    assert_eq!(
        (shown.screen_share, shown.screen_shares),
        (None, Vec::new())
    );
    assert_eq!(server.count("call_shares").await, 0);
}

/// Each answer that gives a place states the number of shares of one person, and a client
/// that shows several shares has its number in the attribute of its token (ADR 0510).
#[tokio::test]
async fn a_join_states_the_shares_of_one_person_and_the_token_states_what_the_client_shows() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let attribute = |join: &CallJoin| {
        claims(join)["attributes"][SHOWN_SHARES_ATTRIBUTE]
            .as_str()
            .map(str::to_owned)
    };
    // A client from before several shares states nothing.
    let old = server.join_call(ann, voice).await;
    assert_eq!(old.shares, Some(MAX_SHARES));
    assert_eq!(attribute(&old), None);
    let path = format!("/api/v1/channels/{voice}/call/join?shares=3");
    let new: CallJoin = server
        .post(ann, &path, &Value::Null)
        .await
        .ok(StatusCode::OK);
    assert_eq!(new.shares, Some(MAX_SHARES));
    assert_eq!(attribute(&new).as_deref(), Some("3"));
    // A renewal of the seat states it too.
    let renewal = format!(
        "/api/v1/channels/{voice}/call/join?renews={}&shares=2",
        new.identity
    );
    let renewed: CallJoin = server
        .post(ann, &renewal, &Value::Null)
        .await
        .ok(StatusCode::OK);
    assert_eq!(attribute(&renewed).as_deref(), Some("2"));
    let other: CallJoin = server
        .post(ben, &path, &Value::Null)
        .await
        .ok(StatusCode::OK);
    assert_eq!(attribute(&other).as_deref(), Some("3"));
}

/// A client of v0.1.0-test.1 or test.2 reports its one share without a slot and names its
/// picture track `screen/30`; a picture track of another name counts as slot 0 too.
#[tokio::test]
async fn a_client_of_one_share_reports_and_ends_it_as_before() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let seat = server.enter_call(ann, voice).await;
    let path = "/api/v1/call/screen-share";
    server
        .put(ann, path, &json!({ "audio": "application" }))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(server.fake().publish_named(
        &seat.room,
        &seat.identity,
        TrackKind::Video,
        TrackSource::ScreenShare,
        "screen/30",
    ));
    server.check().await;
    server.clock.advance(Duration::seconds(30));
    assert_eq!(server.check().await, CLEAN);
    let shown = participant(server, ben, voice, ann.account_id).await;
    assert_eq!(shown.shares().len(), 1);
    let share = shown.screen_share.clone().unwrap();
    assert_eq!(
        (share.slot, share.kind, share.audio),
        (0, None, ShareAudio::Application)
    );
    let json = serde_json::to_value(&shown).unwrap();
    assert_eq!(json["screen_share"]["audio"], "application");
    assert!(json["screen_share"].get("kind").is_none());

    server.delete(ann, path).await.empty(StatusCode::NO_CONTENT);
    assert_eq!(
        participant(server, ben, voice, ann.account_id)
            .await
            .screen_share,
        None
    );
}

#[tokio::test]
async fn meeting_mode_lasts_while_its_presenter_runs_a_share() {
    let (scene, presenter, listener) = meeting().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let path = "/api/v1/call/screen-share";
    share_in(server, ann, &presenter, 1, "window").await;
    let mut to_ben = server.full_stream(ben).await;

    // The presenter ends the share of slot 0, and runs the share of slot 1.
    server
        .delete(ann, &format!("{path}?slot=0"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.fake().unpublish(
        &presenter.room,
        &presenter.identity,
        TrackSource::ScreenShare,
    );
    assert!(server.fake().publish_named(
        &presenter.room,
        &presenter.identity,
        TrackKind::Video,
        TrackSource::ScreenShare,
        &share_track(1, "window"),
    ));
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["call_participant_updated"]
    );
    server.clock.advance(Duration::seconds(120));
    server.check().await;
    assert_eq!(meeting_of(&scene).await, Some(ann.account_id));
    assert!(!speaks(server, &listener));

    // The last share ends when the media server stops listing its track.
    server.fake().unpublish(
        &presenter.room,
        &presenter.identity,
        TrackSource::ScreenShare,
    );
    assert_eq!(server.check().await, CLEAN);
    server.clock.advance(Duration::seconds(10));
    server.check().await;
    assert!(matches!(
        server.call_events(ben, &mut to_ben).await.as_slice(),
        [
            Event::CallParticipantUpdated { .. },
            Event::CallMeetingEnded {
                reason: MeetingEnd::ShareStopped,
                ..
            }
        ]
    ));
    assert_eq!(meeting_of(&scene).await, None);
}

/// Migration 0035 moves the one share of each member of `call_members` into `call_shares`
/// as slot 0, and keeps what the admission check had seen of it.
#[tokio::test]
async fn migration_0035_keeps_the_share_of_each_member_as_slot_0() {
    use sqlx::ConnectOptions;

    let database = crate::support::empty_database().await;
    let mut connection = database.options.connect().await.unwrap();
    crate::migration_identifiers::migrations_up_to(34)
        .run(&mut connection)
        .await
        .unwrap();
    let mut accounts = [uuid::Uuid::now_v7(), uuid::Uuid::now_v7()];
    accounts.sort();
    for (account, name) in accounts.iter().zip(["Ana", "Ben"]) {
        sqlx::query(
            "insert into accounts (id, name, status, created_at) values ($1, $2, 'active', now())",
        )
        .bind(account)
        .bind(name)
        .execute(&mut connection)
        .await
        .unwrap();
    }
    let call = uuid::Uuid::now_v7();
    sqlx::query(
        "insert into calls (id, room_name, direct_low, direct_high, created_at)
         values ($1, $2, $3, $4, now())",
    )
    .bind(call)
    .bind(format!("jr-{}-{}", "a".repeat(12), "b".repeat(32)))
    .bind(accounts[0])
    .bind(accounts[1])
    .execute(&mut connection)
    .await
    .unwrap();
    let seen = time::OffsetDateTime::now_utc();
    for (index, account) in accounts.iter().enumerate() {
        let shares = index == 0;
        sqlx::query(
            "insert into call_members (call_id, account_id, device_id, session_id, identity,
                 admitted_at, usable_until, connected_at, share_audio, share_started_at,
                 share_seen_at, share_missing_since)
             values ($1, $2, $3, $4, $5, now(), now(), now(), $6, $7, $8, $9)",
        )
        .bind(call)
        .bind(account)
        .bind(uuid::Uuid::now_v7())
        .bind(uuid::Uuid::now_v7())
        .bind(format!("{account}:{}", "c".repeat(31) + &index.to_string()))
        .bind(shares.then_some("system"))
        .bind(shares.then_some(seen))
        .bind(shares.then_some(seen))
        .bind(shares.then_some(seen))
        .execute(&mut connection)
        .await
        .unwrap();
    }
    crate::migration_identifiers::migrations_up_to(i64::MAX)
        .run(&mut connection)
        .await
        .unwrap();
    let rows: Vec<(uuid::Uuid, i16, Option<String>, String, bool, bool)> = sqlx::query_as(
        "select account_id, slot, kind, audio, seen_at is not null, missing_since is not null
         from call_shares order by account_id",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(
        rows,
        [(accounts[0], 0, None, "system".to_owned(), true, true)]
    );
    let left: bool = sqlx::query_scalar(
        "select exists (select 1 from information_schema.columns
                        where table_name = 'call_members' and column_name like 'share_%')",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert!(!left, "the columns of the one share are gone");
}

fn meeting_path(channel_id: ChannelId) -> String {
    format!("/api/v1/channels/{channel_id}/call/meeting")
}

#[tokio::test]
async fn meeting_mode_blocks_every_other_microphone_and_its_end_removes_every_block() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let meeting = meeting_path(voice);
    let carl = server.join(owner, "Carl").await;
    let ann_seat = server.enter_call(ann, voice).await;
    let ben_seat = server.enter_call(ben, voice).await;
    let carl_seat = server.enter_call(&carl, voice).await;
    let carl_block = block_path(voice, carl.account_id, "microphone-block");
    server
        .put_empty(owner, &carl_block)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.check().await, updated(1));

    server
        .put_empty(owner, &meeting)
        .await
        .error(ErrorCode::NotInCall);
    server
        .put_empty(ann, &meeting)
        .await
        .error(ErrorCode::Conflict);
    server
        .put(
            ann,
            "/api/v1/call/screen-share",
            &json!({ "audio": "none" }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(ann, &meeting)
        .await
        .error(ErrorCode::Conflict);
    share(server, ann, &ann_seat, "none").await;
    let everyone = server.everyone_role(owner, scene.workspace_id).await;
    set_override(server, owner, voice, everyone.id, &[], &[W::RunMeetingMode])
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .put_empty(ann, &meeting)
        .await
        .error(ErrorCode::Forbidden);
    set_override(server, owner, voice, everyone.id, &[], &[])
        .await
        .ok::<Value>(StatusCode::OK);
    assert_eq!(
        server.channel_call(ben, voice).await.call.unwrap().meeting,
        None
    );

    let mut to_ben = server.full_stream(ben).await;
    server
        .put_empty(ann, &meeting)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(ann, &meeting)
        .await
        .empty(StatusCode::NO_CONTENT);
    let started = server.call_events(ben, &mut to_ben).await;
    assert!(matches!(
        started.as_slice(),
        [Event::CallMeetingStarted { meeting, .. }] if meeting.presenter == ann.account_id
    ));
    let running = server.channel_call(ben, voice).await.call.unwrap();
    assert_eq!(running.meeting.unwrap().presenter, ann.account_id);
    let block_of = |account: AccountId| {
        running
            .participants
            .iter()
            .find(|participant| participant.account_id == account)
            .unwrap()
            .microphone_block
            .clone()
    };
    assert_eq!(block_of(ann.account_id), None);
    assert_eq!(
        block_of(ben.account_id),
        Some(MicrophoneBlock {
            by: ann.account_id,
            reason: BlockReason::Meeting
        })
    );
    assert_eq!(
        block_of(carl.account_id),
        Some(MicrophoneBlock {
            by: owner.account_id,
            reason: BlockReason::Moderator
        })
    );
    assert_eq!(server.check().await, updated(1));
    assert!(!speaks(server, &ben_seat) && !speaks(server, &carl_seat));
    assert!(speaks(server, &ann_seat));

    share(server, ben, &ben_seat, "none").await;
    server
        .put_empty(ben, &meeting)
        .await
        .error(ErrorCode::Conflict);
    server
        .delete(ben, &meeting)
        .await
        .error(ErrorCode::Forbidden);

    let late = server.join_call(owner, voice).await;
    assert_eq!(
        claims(&late)["video"]["canPublishSources"],
        json!(["camera", "screen_share"])
    );
    let owner_seat = server.connect(late);
    assert_eq!(server.check().await, CLEAN);
    assert!(!speaks(server, &owner_seat));
    assert_eq!(
        participant(server, ben, voice, owner.account_id)
            .await
            .microphone_block
            .unwrap()
            .reason,
        BlockReason::Meeting
    );

    server
        .put_empty(ben, "/api/v1/call/hand")
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(
        participant(server, ann, voice, ben.account_id)
            .await
            .hand_raised
    );
    let ben_block = block_path(voice, ben.account_id, "microphone-block");
    server
        .delete(ben, &ben_block)
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(ann, &ben_block)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.check().await, updated(1));
    assert!(speaks(server, &ben_seat));
    server
        .delete(ben, "/api/v1/call/hand")
        .await
        .empty(StatusCode::NO_CONTENT);
    let allowed = participant(server, ann, voice, ben.account_id).await;
    assert_eq!(
        (allowed.hand_raised, allowed.microphone_block),
        (false, None)
    );
    server
        .put_empty(ann, &ben_block)
        .await
        .error(ErrorCode::Forbidden);

    server.call_events(ben, &mut to_ben).await;
    server
        .delete(ann, &meeting)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(ann, &meeting)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        server.call_events(ben, &mut to_ben).await,
        [Event::CallMeetingEnded {
            call_id: running.id,
            place: running.place,
            reason: MeetingEnd::TurnedOff,
        }]
    );
    let after = server.channel_call(ben, voice).await.call.unwrap();
    assert_eq!(after.meeting, None);
    assert!(
        after
            .participants
            .iter()
            .all(|participant| participant.microphone_block.is_none())
    );
    assert_eq!(server.count("call_microphone_blocks").await, 0);
    assert_eq!(server.check().await, updated(2));
    assert!(speaks(server, &carl_seat) && speaks(server, &owner_seat));
}

/// A call with a presenter who runs meeting mode and one listener.
async fn meeting() -> (Scene, Seat, Seat) {
    let scene = Scene::start().await;
    let voice = scene.voice.id;
    let presenter = scene.server.enter_call(&scene.ann, voice).await;
    let listener = scene.server.enter_call(&scene.ben, voice).await;
    share(&scene.server, &scene.ann, &presenter, "application").await;
    scene
        .server
        .put_empty(&scene.ann, &meeting_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(scene.server.check().await, updated(1));
    (scene, presenter, listener)
}

async fn meeting_of(scene: &Scene) -> Option<AccountId> {
    scene
        .server
        .channel_call(&scene.ben, scene.voice.id)
        .await
        .call
        .and_then(|call| call.meeting)
        .map(|meeting| meeting.presenter)
}

#[tokio::test]
async fn meeting_mode_ends_when_its_presenter_stops_sharing() {
    let (scene, presenter, listener) = meeting().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let mut to_ben = server.full_stream(ben).await;
    assert!(!speaks(server, &listener));

    server
        .delete(ann, "/api/v1/call/screen-share")
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["call_participant_updated", "call_meeting_ended"]
    );
    assert_eq!(meeting_of(&scene).await, None);
    assert_eq!(server.check().await, updated(1));
    assert!(speaks(server, &listener));

    server
        .put(
            ann,
            "/api/v1/call/screen-share",
            &json!({ "audio": "none" }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server.check().await;
    server
        .put_empty(ann, &meeting_path(scene.voice.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(server.check().await, updated(1));
    server.call_events(ben, &mut to_ben).await;
    server.fake().unpublish(
        &presenter.room,
        &presenter.identity,
        TrackSource::ScreenShare,
    );
    assert_eq!(server.check().await, CLEAN);
    server.clock.advance(Duration::seconds(9));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(meeting_of(&scene).await, Some(ann.account_id));
    server.clock.advance(Duration::seconds(1));
    server.check().await;
    let ended = server.call_events(ben, &mut to_ben).await;
    assert!(matches!(
        ended.as_slice(),
        [
            Event::CallParticipantUpdated { .. },
            Event::CallMeetingEnded {
                reason: MeetingEnd::ShareStopped,
                ..
            }
        ]
    ));
    assert_eq!(server.count("call_microphone_blocks").await, 0);
}

#[tokio::test]
async fn meeting_mode_ends_when_its_presenter_has_left_for_a_minute() {
    let (scene, presenter, listener) = meeting().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let mut to_ben = server.full_stream(ben).await;

    server.leave_call(ann).await;
    server.check().await;
    assert_eq!(meeting_of(&scene).await, Some(ann.account_id));
    assert!(!speaks(server, &listener));
    server.clock.advance(Duration::seconds(40));
    assert_eq!(server.check().await, CLEAN);

    let back = server.enter_call(ann, voice).await;
    assert!(speaks(server, &back));
    share(server, ann, &back, "application").await;
    server.clock.advance(Duration::seconds(300));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(meeting_of(&scene).await, Some(ann.account_id));
    assert!(!speaks(server, &listener));
    let _ = presenter;

    server.fake().leave(&back.room, &back.identity);
    server.check().await;
    server.clock.advance(Duration::seconds(20));
    server.check().await;
    assert_eq!(server.occupants(ben, voice).await, [ben.account_id]);
    assert_eq!(meeting_of(&scene).await, Some(ann.account_id));
    server.clock.advance(Duration::seconds(39));
    assert_eq!(server.check().await, CLEAN);
    assert_eq!(meeting_of(&scene).await, Some(ann.account_id));
    let again = server.enter_call(ann, voice).await;
    server.clock.advance(Duration::seconds(1));
    server.call_events(ben, &mut to_ben).await;
    server.check().await;
    assert_eq!(
        server.call_events(ben, &mut to_ben).await,
        [Event::CallMeetingEnded {
            call_id: again.join.call.id,
            place: again.join.call.place,
            reason: MeetingEnd::PresenterLeft,
        }]
    );
    assert_eq!(meeting_of(&scene).await, None);
    server.check().await;
    assert!(speaks(server, &listener));
}

#[tokio::test]
async fn a_block_that_is_set_while_meeting_mode_ends_stays_as_a_block_of_the_moderator() {
    let (scene, _, listener) = meeting().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let block = block_path(voice, ben.account_id, "microphone-block");
    let meeting = meeting_path(voice);

    server.gate().arm(0);
    let mut ending = Box::pin(server.delete(ann, &meeting));
    drive_until(&mut ending, server.gate().arrived()).await;
    let mut blocking = Box::pin(server.put_empty(owner, &block));
    drive_until(&mut blocking, server.until_blocked()).await;
    server.gate().release();
    ending.await.empty(StatusCode::NO_CONTENT);
    blocking.await.empty(StatusCode::NO_CONTENT);

    assert_eq!(meeting_of(&scene).await, None);
    assert_eq!(
        participant(server, ann, voice, ben.account_id)
            .await
            .microphone_block,
        Some(MicrophoneBlock {
            by: owner.account_id,
            reason: BlockReason::Moderator
        })
    );
    assert_eq!(server.check().await, CLEAN);
    assert!(!speaks(server, &listener));

    server
        .delete(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.check().await;
    server
        .put_empty(ann, &meeting_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.check().await;
    server.gate().arm(0);
    let mut blocking = Box::pin(server.put_empty(owner, &block));
    drive_until(&mut blocking, server.gate().arrived()).await;
    let mut ending = Box::pin(server.delete(ann, &meeting));
    drive_until(&mut ending, server.until_blocked()).await;
    server.gate().release();
    blocking.await.empty(StatusCode::NO_CONTENT);
    ending.await.empty(StatusCode::NO_CONTENT);
    assert_eq!(meeting_of(&scene).await, None);
    assert_eq!(
        participant(server, ann, voice, ben.account_id)
            .await
            .microphone_block,
        None
    );
    server.check().await;
    assert!(speaks(server, &listener));
}

async fn say(server: &TestServer, member: &Member, speaking: bool) -> Reply {
    let body = json!({ "speaking": speaking, "level": 0.8 });
    server.put(member, "/api/v1/call/speaking", &body).await
}

#[tokio::test]
async fn speaking_marks_reach_the_people_outside_at_a_bounded_rate() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server.enter_call(ann, voice).await;
    server.enter_call(ben, voice).await;
    let mut to_owner = server.full_stream(owner).await;
    let publish = || async { speaking::publish(&server.state).await.unwrap() };
    let spoken = |events: Vec<Event>| -> Vec<Vec<AccountId>> {
        events
            .into_iter()
            .map(|event| match event {
                Event::CallSpeaking { account_ids, .. } => account_ids,
                other => panic!("{other:?}"),
            })
            .collect()
    };

    say(server, owner, true).await.error(ErrorCode::NotInCall);
    assert_eq!(publish().await, 0);
    say(server, ann, true).await.empty(StatusCode::NO_CONTENT);
    assert_eq!(publish().await, 1);
    let raw_before = to_owner.raw.len();
    assert_eq!(
        spoken(server.call_events(owner, &mut to_owner).await),
        [vec![ann.account_id]]
    );
    assert!(!to_owner.raw[raw_before..].join("").contains("level"));

    say(server, ben, true).await.empty(StatusCode::NO_CONTENT);
    assert_eq!(publish().await, 0);
    server.clock.advance(Duration::milliseconds(999));
    assert_eq!(publish().await, 0);
    server.clock.advance(Duration::milliseconds(1));
    say(server, ann, true).await.empty(StatusCode::NO_CONTENT);
    assert_eq!(publish().await, 1);
    assert_eq!(publish().await, 0);
    assert_eq!(
        spoken(server.call_events(owner, &mut to_owner).await),
        [vec![ann.account_id, ben.account_id]]
    );

    server.clock.advance(Duration::seconds(1));
    say(server, ann, false).await.empty(StatusCode::NO_CONTENT);
    assert_eq!(publish().await, 1);
    server.clock.advance(Duration::seconds(4));
    assert_eq!(publish().await, 1);
    assert_eq!(
        spoken(server.call_events(owner, &mut to_owner).await),
        [vec![ben.account_id], vec![]]
    );

    report_state(server, ann, true, false)
        .await
        .empty(StatusCode::NO_CONTENT);
    let block = block_path(voice, ben.account_id, "microphone-block");
    server
        .put_empty(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.call_events(owner, &mut to_owner).await;
    server.clock.advance(Duration::seconds(2));
    say(server, ann, true).await.empty(StatusCode::NO_CONTENT);
    say(server, ben, true).await.empty(StatusCode::NO_CONTENT);
    publish().await;
    assert!(
        spoken(server.call_events(owner, &mut to_owner).await)
            .iter()
            .all(Vec::is_empty)
    );

    // The call that begins after the setting hides who is inside (ADR 0517): it has no
    // speaking mark.
    server
        .call_settings(owner, voice, json!({ "hides_occupants": true }))
        .await;
    server.leave_call(ann).await;
    server.leave_call(ben).await;
    server.check().await;
    let seat = server.enter_call(ann, voice).await;
    server.call_events(owner, &mut to_owner).await;
    report_state(server, ann, false, false)
        .await
        .empty(StatusCode::NO_CONTENT);
    server.clock.advance(Duration::seconds(10));
    publish().await;
    server.clock.advance(Duration::seconds(2));
    say(server, ann, true).await.empty(StatusCode::NO_CONTENT);
    assert_eq!(publish().await, 0);
    let mut to_ben = server.full_stream(ben).await;
    server.clock.advance(Duration::seconds(2));
    say(server, ann, false).await.empty(StatusCode::NO_CONTENT);
    assert_eq!(publish().await, 0);
    assert!(server.call_events(owner, &mut to_owner).await.is_empty());
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());
    let _ = seat;

    let mut limited = false;
    for _ in 0..25 {
        let reply = say(server, ann, true).await;
        if reply.status != StatusCode::NO_CONTENT {
            reply.error(ErrorCode::RateLimited);
            assert!(reply.retry_after.is_some());
            limited = true;
            break;
        }
    }
    assert!(limited);
}

async fn call(server: &TestServer, caller: &Member, callee: AccountId) -> Reply {
    let path = format!("/api/v1/direct-calls/{callee}/join");
    server.post(caller, &path, &Value::Null).await
}

#[tokio::test]
async fn a_direct_message_call_reaches_its_two_participants_and_nobody_else() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let mut to_ben = server.subscribed(ben, Level::Background).await;
    let mut to_owner = server.full_stream(owner).await;
    let mut to_ann = server.full_stream(ann).await;

    call(server, ann, ann.account_id)
        .await
        .error(ErrorCode::InvalidRequest);
    call(server, ann, AccountId::generate())
        .await
        .error(ErrorCode::NotFound);
    let join: CallJoin = call(server, ann, ben.account_id).await.ok(StatusCode::OK);
    let place = CallPlace::direct(ann.account_id, ben.account_id);
    assert_eq!(join.call.place, place);
    assert_eq!(
        claims(&join)["video"]["canPublishSources"],
        json!(["camera", "microphone", "screen_share", "screen_share_audio"])
    );
    let rung = server.call_events(ben, &mut to_ben).await;
    let [Event::CallInvited { invitation }] = rung.as_slice() else {
        panic!("{rung:?}");
    };
    assert_eq!(invitation.invited_by, ann.account_id);
    assert_eq!(invitation.place, place);
    let waiting: Vec<CallInvitation> = server.snapshot(ben).await.call_invitations;
    assert_eq!(waiting, std::slice::from_ref(invitation));
    call(server, ann, ben.account_id)
        .await
        .ok::<CallJoin>(StatusCode::OK);
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());

    let ann_seat = server.connect(join);
    server.check().await;
    assert_eq!(
        names(&server.call_events(ann, &mut to_ann).await),
        [
            "call_invited",
            "own_call_changed",
            "call_started",
            "call_participant_joined"
        ]
    );
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());
    assert_eq!(server.snapshot(ben).await.direct_calls.len(), 1);
    assert!(server.snapshot(owner).await.direct_calls.is_empty());

    let answered: CallJoin = call(server, ben, ann.account_id).await.ok(StatusCode::OK);
    assert_eq!(answered.call.id, ann_seat.join.call.id);
    assert_eq!(answered.call.participants.len(), 1);
    let ben_seat = server.connect(answered);
    assert_eq!(ben_seat.room, ann_seat.room);
    server.check().await;
    let to_callee = server.call_events(ben, &mut to_ben).await;
    assert_eq!(
        names(&to_callee),
        [
            "call_invitation_resolved",
            "own_call_changed",
            "call_participant_joined"
        ]
    );
    assert!(matches!(
        &to_callee[0],
        Event::CallInvitationResolved {
            outcome: InvitationOutcome::Accepted,
            ..
        }
    ));
    assert_eq!(
        names(&server.call_events(ann, &mut to_ann).await),
        ["call_invitation_resolved", "call_participant_joined"]
    );
    server
        .put(
            ann,
            "/api/v1/call/screen-share",
            &json!({ "audio": "system" }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    report_state(server, ben, true, false)
        .await
        .empty(StatusCode::NO_CONTENT);
    say(server, ann, true).await.empty(StatusCode::NO_CONTENT);
    assert_eq!(speaking::publish(&server.state).await.unwrap(), 0);
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["call_participant_updated", "call_participant_updated"]
    );
    assert_eq!(
        server
            .own_call(ben)
            .await
            .call
            .unwrap()
            .call
            .participants
            .len(),
        2
    );

    assert!(server.call_events(owner, &mut to_owner).await.is_empty());
    let raw = to_owner.raw.join("\n");
    assert!(!raw.contains(&ann_seat.join.call.id.to_string()), "{raw}");
    for path in [
        meeting_path(scene.voice.id),
        format!("/api/v1/channels/{}/call/lock", scene.voice.id),
    ] {
        server
            .put_empty(ann, &path)
            .await
            .error(ErrorCode::NotInCall);
    }
    let outsider = sqlx::query(
        "insert into call_members
             (call_id, account_id, device_id, session_id, identity, admitted_at, usable_until)
         values ($1, $2, $3, $3, $4, now(), now())",
    )
    .bind(ann_seat.join.call.id.as_uuid())
    .bind(owner.account_id.as_uuid())
    .bind(uuid::Uuid::now_v7())
    .bind(format!("{}:{}", owner.account_id, "0".repeat(32)))
    .execute(&server.db)
    .await;
    assert!(
        outsider
            .unwrap_err()
            .to_string()
            .contains("not a participant")
    );

    server
        .fake()
        .enter(&ann_seat.room, "intruder", ParticipantKind::Standard, None);
    let seats = server.fake().identities(&ann_seat.room).len();
    assert_eq!(seats, 3);
    assert_eq!(
        server.check().await,
        Outcome::Checked {
            removed: 1,
            updated: 0
        }
    );

    server.join_call(ann, scene.voice.id).await;
    assert_eq!(
        server.call_events(ben, &mut to_ben).await,
        [Event::CallParticipantLeft {
            call_id: ann_seat.join.call.id,
            place,
            account_id: ann.account_id,
            reason: LeaveReason::Switched,
        }]
    );
    server.leave_call(ben).await;
    server.check().await;
    assert_eq!(server.count("calls").await, 1);
    assert!(server.fake().identities(&ann_seat.room).is_empty());
    assert!(server.call_events(owner, &mut to_owner).await.is_empty());
}

#[tokio::test]
async fn a_direct_message_call_that_is_declined_or_given_up_stops_ringing() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let elsewhere: junto_protocol::structure::Workspace = server
        .post(owner, "/api/v1/workspaces", &json!({ "name": "Elsewhere" }))
        .await
        .ok(StatusCode::CREATED);
    let distant = server.join_into(owner, "Distant", &[elsewhere.id]).await;
    let hidden = call(server, ann, distant.account_id).await;
    hidden.error(ErrorCode::NotFound);
    assert_eq!(
        hidden.observable(),
        call(server, ann, AccountId::generate()).await.observable()
    );
    server
        .put_empty(
            owner,
            &format!("/api/v1/accounts/{}/ban", distant.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    call(server, owner, distant.account_id)
        .await
        .error(ErrorCode::NotFound);

    let mut to_ben = server.subscribed(ben, Level::Background).await;
    let mut to_ann = server.subscribed(ann, Level::Background).await;
    let refusal = |account: AccountId| format!("/api/v1/direct-calls/{account}/refusal");
    server
        .post(ben, &refusal(ann.account_id), &Value::Null)
        .await
        .error(ErrorCode::NotFound);
    let join: CallJoin = call(server, ann, ben.account_id).await.ok(StatusCode::OK);
    server
        .post(ann, &refusal(ben.account_id), &Value::Null)
        .await
        .error(ErrorCode::NotFound);
    server
        .post(ben, &refusal(ann.account_id), &Value::Null)
        .await
        .empty(StatusCode::NO_CONTENT);
    let declined = server.call_events(ann, &mut to_ann).await;
    assert!(matches!(
        declined.as_slice(),
        [
            Event::CallInvited { .. },
            Event::OwnCallChanged { .. },
            Event::CallInvitationResolved {
                outcome: InvitationOutcome::Declined,
                ..
            }
        ]
    ));
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["call_invited", "call_invitation_resolved"]
    );
    server.leave_call(ann).await;
    assert_eq!(server.count("calls").await, 0);

    call(server, ann, ben.account_id)
        .await
        .error(ErrorCode::RateLimited);
    server.clock.advance(Duration::minutes(5));
    call(server, ann, ben.account_id)
        .await
        .ok::<CallJoin>(StatusCode::OK);
    server.leave_call(ann).await;
    let given_up = server.call_events(ben, &mut to_ben).await;
    assert!(matches!(
        given_up.as_slice(),
        [
            Event::CallInvited { .. },
            Event::CallInvitationResolved {
                outcome: InvitationOutcome::Cancelled,
                ..
            }
        ]
    ));
    assert!(server.snapshot(ben).await.call_invitations.is_empty());

    let rung: CallJoin = call(server, ann, ben.account_id).await.ok(StatusCode::OK);
    server.connect(rung);
    server.check().await;
    server.clock.advance(Duration::seconds(60));
    server.check().await;
    let expired = server.call_events(ben, &mut to_ben).await;
    assert!(matches!(
        expired.as_slice(),
        [
            Event::CallInvited { .. },
            Event::CallInvitationResolved {
                outcome: InvitationOutcome::Expired,
                ..
            }
        ]
    ));
    assert_eq!(server.count("calls").await, 1);
    call(server, ben, ann.account_id)
        .await
        .ok::<CallJoin>(StatusCode::OK);
    assert_eq!(server.count("call_members").await, 2);
    let _ = join;
}

#[tokio::test]
async fn a_background_connection_of_an_account_in_a_call_receives_the_events_of_that_call() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let other = server.voice(owner, scene.workspace_id, "garden").await.id;
    let mut background = server.subscribed(ben, Level::Background).await;

    server.enter_call(ann, voice).await;
    server.enter_call(owner, other).await;
    assert!(server.call_events(ben, &mut background).await.is_empty());

    server.enter_call(ben, voice).await;
    report_state(server, ann, true, false)
        .await
        .empty(StatusCode::NO_CONTENT);
    report_state(server, owner, true, false)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .category(owner, scene.workspace_id, "Projects", 0)
        .await;
    let during = server.traffic(ben, &mut background).await;
    let during: Vec<&str> = during.iter().map(|(_, event)| event.name()).collect();
    assert_eq!(
        during,
        [
            "own_call_changed",
            // The line of the chat that Ben joined the running call.
            "message_changed",
            "call_participant_joined",
            "call_participant_updated"
        ]
    );

    server.leave_call(ben).await;
    server.check().await;
    report_state(server, ann, false, false)
        .await
        .empty(StatusCode::NO_CONTENT);
    let after = server.call_events(ben, &mut background).await;
    assert!(matches!(
        after.as_slice(),
        [Event::OwnCallChanged {
            call: None,
            left: Some(LeaveReason::Left)
        }] | [
            Event::CallParticipantLeft { .. },
            Event::OwnCallChanged { call: None, .. }
        ]
    ));
}

async fn notify(server: &TestServer, authorization: Option<&str>, body: &str) -> StatusCode {
    let mut request = reqwest::Client::new()
        .request(
            Method::POST,
            format!("{}/internal/media/notifications", server.base),
        )
        .header("content-type", "application/webhook+json")
        .body(body.to_owned());
    if let Some(authorization) = authorization {
        request = request.header("authorization", authorization);
    }
    request.send().await.unwrap().status()
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

#[tokio::test]
async fn a_signed_notification_of_the_media_server_makes_the_check_run_at_once() {
    let server = TestServer::start_with_tasks(|config| {
        config.limits.calls.check_interval = Duration::hours(1);
    })
    .await;
    let scene = Scene::on(server).await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    until("the first check has run", async || {
        server.state.calls.reachable() == Some(true)
    })
    .await;
    let join = server.join_call(ann, scene.voice.id).await;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    let elsewhere = server.join_call(owner, garden).await;
    server.settled().await;
    let seat = server.connect(join);
    let other_seat = server.connect(elsewhere);
    for room in [&seat.room, &other_seat.room] {
        server
            .fake()
            .enter(room, "intruder", ParticipantKind::Standard, None);
    }

    let body = format!(
        r#"{{"event":"participant_joined","room":{{"name":"{}"}}}}"#,
        seat.room
    );
    let body = body.as_str();
    let signed = server.fake().sign_notification(body.as_bytes());
    let tampered = r#"{"event":"participant_joined","room":{"name":"jr-other"}}"#;
    assert_eq!(notify(server, None, body).await, StatusCode::UNAUTHORIZED);
    assert_eq!(
        notify(server, Some("not-a-token"), body).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        notify(server, Some(&signed), tampered).await,
        StatusCode::UNAUTHORIZED
    );
    server.settled().await;
    assert_eq!(server.fake().identities(&seat.room).len(), 2);
    assert!(server.occupants(ann, scene.voice.id).await.is_empty());

    assert_eq!(
        notify(server, Some(&signed), body).await,
        StatusCode::NO_CONTENT
    );
    until("the intruder is removed", async || {
        server.fake().identities(&seat.room) == std::slice::from_ref(&seat.identity)
    })
    .await;
    until("the person is in the call", async || {
        server.occupants(ann, scene.voice.id).await == [ann.account_id]
    })
    .await;
    assert_eq!(server.fake().identities(&other_seat.room).len(), 2);

    let without_a_room = r#"{"event":"room_finished"}"#;
    let signed_without_a_room = server.fake().sign_notification(without_a_room.as_bytes());
    assert_eq!(
        notify(server, Some(&signed_without_a_room), without_a_room).await,
        StatusCode::NO_CONTENT
    );
    until(
        "a notification without a room starts a full check",
        async || server.fake().identities(&other_seat.room).len() == 1,
    )
    .await;

    let plain = TestServer::start().await;
    assert_eq!(
        notify(&plain, Some(&signed), body).await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn the_admission_check_runs_by_itself_at_its_interval() {
    let server = TestServer::start_with_tasks(|config| {
        config.limits.calls.check_interval = Duration::milliseconds(50);
        config.limits.calls.speaking_interval = Duration::milliseconds(50);
    })
    .await;
    let scene = Scene::on(server).await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    until("the first check has run", async || {
        server.state.calls.reachable() == Some(true)
    })
    .await;
    let mut to_owner = server.full_stream(owner).await;

    let seat = server.connect(server.join_call(ann, scene.voice.id).await);
    until("the person is in the call", async || {
        server.occupants(owner, scene.voice.id).await == [ann.account_id]
    })
    .await;
    server
        .fake()
        .enter(&seat.room, "intruder", ParticipantKind::Standard, None);
    until("the intruder is removed", async || {
        server.fake().identities(&seat.room) == std::slice::from_ref(&seat.identity)
    })
    .await;

    server.fake().set_unreachable(true);
    until("the outage is noticed", async || {
        server.state.calls.reachable() == Some(false)
    })
    .await;
    server
        .call_join_reply(ben, scene.voice.id)
        .await
        .error(ErrorCode::MediaUnavailable);
    server.leave_call(ann).await;
    tokio::time::sleep(Wait::from_millis(200)).await;
    assert_eq!(server.fake().identities(&seat.room).len(), 1);
    server.fake().set_unreachable(false);
    until(
        "the person who left is removed after the return",
        async || server.fake().identities(&seat.room).is_empty(),
    )
    .await;
    until("joins are accepted again", async || {
        server.state.calls.reachable() == Some(true)
    })
    .await;

    let ben_seat = server.connect(server.join_call(ben, scene.voice.id).await);
    until("the next person is in the call", async || {
        server.occupants(owner, scene.voice.id).await == [ben.account_id]
    })
    .await;
    server
        .put(ben, "/api/v1/call/speaking", &json!({ "speaking": true }))
        .await
        .empty(StatusCode::NO_CONTENT);
    let mut spoke = false;
    let started = Instant::now();
    while !spoke {
        assert!(
            started.elapsed() < Wait::from_secs(30),
            "no speaking mark arrived"
        );
        spoke = server
            .call_events(owner, &mut to_owner)
            .await
            .iter()
            .any(|event| matches!(event, Event::CallSpeaking { account_ids, .. } if account_ids == &[ben.account_id]));
    }
    let _ = (ben_seat, WorkspacePermission::ViewChannel);
}

#[tokio::test]
async fn a_person_whose_microphone_is_blocked_does_not_run_meeting_mode() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let seat = server.enter_call(ann, voice).await;
    let listener = server.enter_call(ben, voice).await;
    let block = block_path(voice, ann.account_id, "microphone-block");
    server
        .put_empty(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    share(server, ann, &seat, "none").await;

    server
        .put_empty(ann, &meeting_path(voice))
        .await
        .error(ErrorCode::Conflict);
    assert_eq!(meeting_of(&scene).await, None);
    assert!(!speaks(server, &seat) && speaks(server, &listener));
    assert_eq!(server.count("call_microphone_blocks").await, 1);

    server
        .delete(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(ann, &meeting_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(meeting_of(&scene).await, Some(ann.account_id));
}

#[tokio::test]
async fn a_direct_message_call_ends_for_both_when_one_account_is_banned() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let join: CallJoin = call(server, ann, ben.account_id).await.ok(StatusCode::OK);
    let ann_seat = server.connect(join);
    let answered: CallJoin = call(server, ben, ann.account_id).await.ok(StatusCode::OK);
    server.connect(answered);
    server.check().await;
    let mut to_ann = server.full_stream(ann).await;

    server
        .put_empty(owner, &format!("/api/v1/accounts/{}/ban", ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        server.check().await,
        Outcome::Checked {
            removed: 2,
            updated: 0
        }
    );
    assert!(server.fake().identities(&ann_seat.room).is_empty());
    assert_eq!(server.count("calls").await, 0);
    let told = server.call_events(ann, &mut to_ann).await;
    assert!(told.contains(&Event::OwnCallChanged {
        call: None,
        left: Some(LeaveReason::AccessEnded),
    }));
    call(server, ann, ben.account_id)
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn what_happens_in_the_call_of_a_voice_channel_is_kept_for_the_lines_of_its_chat() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let seat = server.enter_call(ann, voice).await;
    share(server, ann, &seat, "system").await;
    server
        .put(
            ann,
            "/api/v1/call/screen-share",
            &json!({ "audio": "none" }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(ann, &meeting_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(ann, &meeting_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    let lock = format!("/api/v1/channels/{voice}/call/lock");
    server
        .put_empty(ann, &lock)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(
            ben,
            &format!("/api/v1/channels/{voice}/call/entry-requests"),
            &Value::Null,
        )
        .await
        .ok::<Value>(StatusCode::CREATED);
    let invitation: CallInvitation = server
        .post(
            ann,
            &format!("/api/v1/channels/{voice}/call/invitations"),
            &json!({ "account_id": owner.account_id }),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .post(
            owner,
            &format!("/api/v1/call/invitations/{}/acceptance", invitation.id),
            &Value::Null,
        )
        .await
        .ok::<CallJoin>(StatusCode::OK);
    server.leave_call(owner).await;
    server.leave_call(ann).await;

    let direct: CallJoin = call(server, ann, ben.account_id).await.ok(StatusCode::OK);
    server.connect(direct);
    server.check().await;
    server.leave_call(ann).await;

    let notes: Vec<(String, Option<uuid::Uuid>, uuid::Uuid)> =
        sqlx::query_as("select kind, account_id, channel_id from call_notes order by id")
            .fetch_all(&server.db)
            .await
            .unwrap();
    let (ann_id, ben_id) = (
        Some(ann.account_id.as_uuid()),
        Some(ben.account_id.as_uuid()),
    );
    let expected = [
        ("call_started", ann_id),
        ("screen_share_started", ann_id),
        ("meeting_started", ann_id),
        ("meeting_ended", ann_id),
        ("entry_requested", ben_id),
        ("joined_by_invitation", Some(owner.account_id.as_uuid())),
        ("call_ended", None),
    ];
    let kept: Vec<(&str, Option<uuid::Uuid>)> = notes
        .iter()
        .map(|(kind, account_id, _)| (kind.as_str(), *account_id))
        .collect();
    assert_eq!(kept, expected);
    assert!(
        notes
            .iter()
            .all(|(_, _, channel_id)| *channel_id == voice.as_uuid())
    );
    let other_kind = sqlx::query("update call_notes set kind = 'left'")
        .execute(&server.db)
        .await;
    assert!(other_kind.is_err());
}

async fn raise_hand(server: &TestServer, member: &Member) {
    server
        .put_empty(member, "/api/v1/call/hand")
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn rule_70_73_the_presenter_lowers_the_hand_of_another_person() {
    let (scene, _, _) = meeting().await;
    let (server, ann, ben, voice) = (&scene.server, &scene.ann, &scene.ben, scene.voice.id);
    raise_hand(server, ben).await;
    assert!(
        participant(server, ann, voice, ben.account_id)
            .await
            .hand_raised
    );
    let hand = block_path(voice, ben.account_id, "hand");
    let audit_entries = server.count("audit_entries").await;
    let mut to_ben = server.full_stream(ben).await;

    server
        .delete(ann, &hand)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(
        !participant(server, ann, voice, ben.account_id)
            .await
            .hand_raised
    );
    let told = server.call_events(ben, &mut to_ben).await;
    assert_eq!(names(&told), ["call_participant_updated"]);
    assert!(matches!(
        &told[0],
        Event::CallParticipantUpdated { participant, .. }
            if participant.account_id == ben.account_id && !participant.hand_raised
    ));
    // The block of the meeting stays: lowering a hand does not let the person speak.
    assert_eq!(
        participant(server, ann, voice, ben.account_id)
            .await
            .microphone_block
            .unwrap()
            .reason,
        BlockReason::Meeting
    );

    // A hand that is not raised stays as it is, and nothing is told.
    server
        .delete(ann, &hand)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(names(&server.call_events(ben, &mut to_ben).await), [""; 0]);
    assert_eq!(server.count("audit_entries").await, audit_entries);

    // A listener does not lower the hand of the presenter or of another listener.
    raise_hand(server, ann).await;
    let of_presenter = block_path(voice, ann.account_id, "hand");
    server
        .delete(ben, &of_presenter)
        .await
        .error(ErrorCode::Forbidden);
    assert!(
        participant(server, ben, voice, ann.account_id)
            .await
            .hand_raised
    );

    // When meeting mode has ended, the person who ran it has no such right any more.
    server
        .delete(ann, &meeting_path(voice))
        .await
        .empty(StatusCode::NO_CONTENT);
    raise_hand(server, ben).await;
    server.delete(ann, &hand).await.error(ErrorCode::Forbidden);
    assert!(
        participant(server, ann, voice, ben.account_id)
            .await
            .hand_raised
    );
}

#[tokio::test]
async fn rule_71_72_a_moderator_lowers_a_hand_and_other_people_are_refused() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server.enter_call(ann, voice).await;
    server.enter_call(ben, voice).await;
    let hand = block_path(voice, ben.account_id, "hand");
    let raised = || async {
        participant(server, ann, voice, ben.account_id)
            .await
            .hand_raised
    };

    raise_hand(server, ben).await;
    server.delete(ann, &hand).await.error(ErrorCode::Forbidden);
    assert!(raised().await);

    server
        .grant_workspace(owner, ann, scene.workspace_id, &[W::MuteMembers])
        .await;
    server
        .delete(ann, &hand)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(!raised().await);

    // A moderator outside the call lowers a hand too, as for a microphone block.
    raise_hand(server, ben).await;
    server
        .delete(owner, &hand)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(!raised().await);

    // A person who is not in the call, a channel without a call, a text channel and a
    // channel that the caller does not see.
    let outside = block_path(voice, owner.account_id, "hand");
    server
        .delete(owner, &outside)
        .await
        .error(ErrorCode::NotFound);
    let empty = server.voice(owner, scene.workspace_id, "empty").await;
    server
        .delete(owner, &block_path(empty.id, ben.account_id, "hand"))
        .await
        .error(ErrorCode::NotFound);
    let text = server.channel(owner, scene.workspace_id, "text").await;
    server
        .delete(owner, &block_path(text.id, ben.account_id, "hand"))
        .await
        .error(ErrorCode::Conflict);
    let stranger = server.join(owner, "Stranger").await;
    let role = server.everyone_role(owner, scene.workspace_id).await;
    let hidden = server.voice(owner, scene.workspace_id, "hidden").await;
    server
        .make_private(owner, scene.workspace_id, hidden.id, role.id)
        .await;
    server
        .delete(&stranger, &block_path(hidden.id, ben.account_id, "hand"))
        .await
        .error(ErrorCode::NotFound);

    // In a call that hides who is inside, a moderator outside learns nothing. The setting
    // applies to the call that begins after it (ADR 0517).
    server
        .call_settings(owner, voice, json!({ "hides_occupants": true }))
        .await;
    server.leave_call(ann).await;
    server.leave_call(ben).await;
    server.check().await;
    server.enter_call(ann, voice).await;
    server.enter_call(ben, voice).await;
    raise_hand(server, ben).await;
    server
        .delete(owner, &hand)
        .await
        .error(ErrorCode::NotInCall);
    server
        .delete(owner, &outside)
        .await
        .error(ErrorCode::NotInCall);
    assert!(raised().await);
    server
        .delete(ann, &hand)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(!raised().await);
}

#[tokio::test]
async fn rule_64_a_join_token_carries_the_display_name_of_the_account() {
    let scene = Scene::start().await;
    let (server, ann) = (&scene.server, &scene.ann);
    let first = server.join_call(ann, scene.voice.id).await;
    assert_eq!(claims(&first)["name"], "Ann");
    assert_eq!(claims(&first)["sub"], first.identity);
    assert!(!first.identity.contains("Ann"));

    server
        .patch(ann, "/api/v1/account", &json!({ "name": "Ann Example" }))
        .await
        .ok::<Value>(StatusCode::OK);
    let second = server.join_call(ann, scene.voice.id).await;
    assert_eq!(claims(&second)["name"], "Ann Example");

    let direct = server.enter_direct(&scene.ben, ann).await;
    assert_eq!(claims(&direct.join)["name"], "Ben");
}
