//! Proofs of the audit of the whole server: calls and recordings. Each test fails while its
//! defect exists.

use junto_protocol::call::{CallJoin, EntryRequest, LeaveReason};
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::AccountId;
use junto_protocol::message::MessagePage;
use junto_server::media::{TrackKind, TrackSource};
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::Duration;

use crate::calls_review2::{move_to, ring};
use crate::calls_support::{Scene, names};
use crate::recordings_support::Studio;

/// ADR 0514: a voice channel that hides who is inside shows the people outside only that
/// it is in use, and a person who needs to know enters and is then visible to the people
/// inside. A member row is enough to be inside for every read, and a member that never
/// connects to the media server is shown to nobody. An account that asks for a place and
/// does not connect reads who is inside from the answer and from the route of the call,
/// and keeps that state with one request in each lifetime of a join token.
#[tokio::test]
async fn audit_a_seat_that_never_connects_does_not_read_a_hidden_voice_channel() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server
        .call_settings(owner, voice, json!({ "hides_occupants": true }))
        .await;
    server.enter_call(ann, voice).await;
    let mut to_ann = server.full_stream(ann).await;

    let join = server.join_call(ben, voice).await;
    server.check().await;
    let in_the_answer: Vec<AccountId> = join
        .call
        .participants
        .iter()
        .map(|participant| participant.account_id)
        .collect();
    let at_once = server.occupants(ben, voice).await;
    for _ in 0..3 {
        server.clock.advance(Duration::seconds(100));
        server.join_call(ben, voice).await;
        server.check().await;
    }
    let after_five_minutes = server.occupants(ben, voice).await;

    let told = names(&server.call_events(ann, &mut to_ann).await).join(", ");
    let shown = server.occupants(ann, voice).await.contains(&ben.account_id);
    let read_nothing =
        in_the_answer.is_empty() && at_once.is_empty() && after_five_minutes.is_empty();
    assert!(
        shown || !told.is_empty() || read_nothing,
        "the person inside does not see the account and got no event, and the account read \
         who is inside: in the answer of the join {in_the_answer:?}, from the route at once \
         {at_once:?}, from the route after five minutes {after_five_minutes:?}"
    );
}

/// ADR 0604: while its call runs in a channel that hides who is inside, a recording is
/// returned only to the people inside. An account with a member row that never connected
/// reads the recording and its segments, which name the people whose sound it holds.
#[tokio::test]
async fn audit_a_seat_that_never_connects_does_not_read_the_recording_of_a_hidden_call() {
    let studio = Studio::hidden().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let voice = scene.voice.id;
    let started = studio.record().await;
    let carl = server.join(owner, "Carl").await;
    server
        .recording_reply(&carl, started.id)
        .await
        .error(ErrorCode::NotFound);
    let mut to_ann = server.full_stream(ann).await;

    server.join_call(&carl, voice).await;
    server.check().await;
    let told = names(&server.call_events(ann, &mut to_ann).await).join(", ");
    let shown = server
        .occupants(ann, voice)
        .await
        .contains(&carl.account_id);
    let read = server.recording_reply(&carl, started.id).await;
    assert!(
        shown || !told.is_empty() || read.status == StatusCode::NOT_FOUND,
        "the person inside does not see the account and got no event, and the account read \
         the recording of the running call: {}",
        read.text()
    );
}

/// ADR 0514: the rule of a hidden voice channel holds for moderators, administrators and
/// the server owner outside. A person with `move_members` moves the own account towards the
/// hidden channel, reads the people inside from the answer of the join of that move, and
/// reports the move as failed. The people inside get no event.
#[tokio::test]
async fn audit_a_move_of_the_own_account_does_not_show_who_is_inside_a_hidden_voice_channel() {
    let scene = Scene::start().await;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let lounge = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    server
        .call_settings(owner, garden, json!({ "hides_occupants": true }))
        .await;
    server.enter_call(ann, garden).await;
    server.enter_call(owner, lounge).await;
    assert!(server.occupants(owner, garden).await.is_empty());
    let mut to_ann = server.full_stream(ann).await;

    let pending = move_to(server, owner, lounge, owner.account_id, garden).await;
    let join: CallJoin = server
        .post(
            owner,
            &format!("/api/v1/call/moves/{}/join", pending.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    server
        .post(
            owner,
            &format!("/api/v1/call/moves/{}/failure", pending.id),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(server.call_events(ann, &mut to_ann).await.is_empty());
    let read: Vec<AccountId> = join
        .call
        .participants
        .iter()
        .map(|participant| participant.account_id)
        .collect();
    assert!(
        read.is_empty(),
        "an account that never was inside read who is inside: {read:?}"
    );
}

/// ADR 0604 hides a recording of a running call in a hidden voice channel from the people
/// outside, because it names a person inside. The audit entry of its start names the same
/// person, and the workspace audit log returns it to a reader outside.
#[tokio::test]
async fn audit_the_audit_log_does_not_name_the_person_who_records_in_a_hidden_voice_channel() {
    let studio = Studio::hidden().await;
    let scene = &studio.scene;
    let (server, owner, ann) = (&scene.server, &scene.owner, &scene.ann);
    let voice = scene.voice.id;
    let started = studio.record().await;
    assert!(server.occupants(owner, voice).await.is_empty());
    server
        .recording_reply(owner, started.id)
        .await
        .error(ErrorCode::NotFound);

    for filter in [
        "action=recording_started".to_owned(),
        format!("actor_id={}", ann.account_id),
        format!("target_id={}", started.id),
    ] {
        let page: junto_protocol::audit::AuditPage = server
            .get(
                owner,
                &format!(
                    "/api/v1/workspaces/{}/audit-log?{filter}",
                    scene.workspace_id
                ),
            )
            .await
            .ok(StatusCode::OK);
        assert!(
            !page
                .entries
                .iter()
                .any(|entry| entry.channel_id == Some(voice)
                    && entry.actor_id == Some(ann.account_id))
        );
    }
    let entries = server.audit_actions(owner, scene.workspace_id).await;
    let named: Vec<String> = entries
        .iter()
        .filter(|entry| entry.channel_id == Some(voice) && entry.actor_id == Some(ann.account_id))
        .map(|entry| format!("{:?}", entry.event))
        .collect();
    assert!(
        named.is_empty(),
        "a reader outside the hidden voice channel reads entries of a person inside: {named:?}"
    );
}

/// ADR 0515: a caller cannot tell a muted conversation from a call that is not answered.
/// The caller of a direct call gets `call_invited` when the other account is rung, and no
/// event when the other account muted the conversation.
#[tokio::test]
async fn audit_the_caller_of_a_direct_call_cannot_tell_a_muted_conversation() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    server
        .put(
            ben,
            &format!("/api/v1/direct-messages/{}/mute", ann.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let mut to_ann = server.full_stream(ann).await;

    ring(server, ann, owner)
        .await
        .ok::<CallJoin>(StatusCode::OK);
    let rung = names(&server.call_events(ann, &mut to_ann).await).join(", ");
    server.leave_call(ann).await;
    server.call_events(ann, &mut to_ann).await;

    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    let muted = names(&server.call_events(ann, &mut to_ann).await).join(", ");
    assert_eq!(
        muted, rung,
        "the caller receives other events for an account that muted the conversation"
    );
}

/// ADR 0512 and ADR 0516 give the person who runs meeting mode the removal of a microphone
/// block and the lowering of a hand. The check names the presenter of the meeting and does
/// not ask whether that account is in the call. A presenter whom a moderator removed from
/// the call removes the block that the moderator set, until meeting mode ends 60 seconds
/// later.
#[tokio::test]
async fn audit_a_presenter_who_was_removed_from_the_call_does_not_remove_a_block() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let presenter = server.enter_call(ann, voice).await;
    server.enter_call(ben, voice).await;
    server
        .put(
            ann,
            "/api/v1/call/screen-share",
            &json!({ "audio": "application" }),
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
        .put_empty(ann, &format!("/api/v1/channels/{voice}/call/meeting"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.check().await;

    let participant =
        |account_id: AccountId| format!("/api/v1/channels/{voice}/call/participants/{account_id}");
    let block = format!("{}/microphone-block", participant(ben.account_id));
    server
        .put_empty(owner, &block)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(owner, &participant(ann.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.check().await;
    assert_eq!(server.occupants(owner, voice).await, [ben.account_id]);
    assert_eq!(server.count("call_microphone_blocks").await, 1);

    let removed = server.delete(ann, &block).await;
    assert_eq!(
        server.count("call_microphone_blocks").await,
        1,
        "an account that is not in the call removed the block of a moderator; the answer \
         was {}",
        removed.status
    );
}

/// The accounts that the events of `stream` name as participants until now.
async fn named(
    server: &crate::support::TestServer,
    member: &crate::support::Member,
    stream: &mut crate::stream_support::Stream,
    account_id: AccountId,
) -> Vec<&'static str> {
    server
        .traffic(member, stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .filter(|event| {
            serde_json::to_string(event)
                .unwrap()
                .contains(&account_id.to_string())
        })
        .map(|event| event.name())
        .collect()
}

/// ADR 0517: an account is inside a call from the moment the admission check has recorded
/// that the media server lists its seat. From that moment it reads the whole call, the
/// people inside are told of it in the same transaction, and before that moment it receives
/// no event of the call.
#[tokio::test]
async fn an_account_reads_a_hidden_call_from_the_check_that_sees_its_seat() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server
        .call_settings(owner, voice, json!({ "hides_occupants": true }))
        .await;
    server.enter_call(ann, voice).await;
    let mut to_ann = server.full_stream(ann).await;
    let mut to_ben = server.full_stream(ben).await;

    let join = server.join_call(ben, voice).await;
    assert!(join.call.hides_occupants);
    assert!(join.call.participants.is_empty() && join.entry_requests.is_empty());
    server
        .put(
            ann,
            "/api/v1/call/state",
            &json!({ "muted": true, "deafened": false }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let before = server.own_call(ben).await.call.unwrap();
    assert!(before.call.participants.is_empty());
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["own_call_changed"],
        "a seat that the check has not seen receives the events of the call"
    );

    let seat = server.connect(join);
    server.check().await;
    assert_eq!(
        names(&server.call_events(ann, &mut to_ann).await),
        ["call_participant_updated", "call_participant_joined"]
    );
    assert_eq!(
        names(&server.call_events(ben, &mut to_ben).await),
        ["call_participant_joined"],
        "the account is told that it is inside by the event of its own entry"
    );
    let after = server.own_call(ben).await.call.unwrap();
    let inside: Vec<AccountId> = after
        .call
        .participants
        .iter()
        .map(|participant| participant.account_id)
        .collect();
    assert_eq!(inside, [ann.account_id, ben.account_id]);
    assert!(after.call.hides_occupants);
    assert_eq!(
        server.occupants(ben, voice).await,
        [ann.account_id, ben.account_id]
    );
    let _ = seat;
}

/// ADR 0517: only a person inside acts on a call. An account with a seat that never
/// connected does not lock the call, does not answer a request to enter, does not invite,
/// and receives no request to enter, in a channel that hides who is inside and in one that
/// does not.
#[tokio::test]
async fn a_seat_that_never_connects_does_not_act_as_a_person_inside() {
    for hides in [true, false] {
        let scene = Scene::start().await;
        let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
        let voice = scene.voice.id;
        server
            .call_settings(owner, voice, json!({ "hides_occupants": hides }))
            .await;
        server.enter_call(ann, voice).await;
        server.join_call(ben, voice).await;
        let carl = server.join(owner, "Carl").await;
        let mut to_ben = server.full_stream(ben).await;

        let lock = format!("/api/v1/channels/{voice}/call/lock");
        server
            .put_empty(ben, &lock)
            .await
            .error(ErrorCode::NotInCall);
        server
            .post(
                ben,
                &format!("/api/v1/channels/{voice}/call/invitations"),
                &json!({ "account_id": carl.account_id }),
            )
            .await
            .error(ErrorCode::NotInCall);
        server
            .put_empty(ben, &format!("/api/v1/channels/{voice}/call/meeting"))
            .await
            .error(ErrorCode::NotInCall);

        server
            .put_empty(ann, &lock)
            .await
            .empty(StatusCode::NO_CONTENT);
        let request: EntryRequest = server
            .post(
                &carl,
                &format!("/api/v1/channels/{voice}/call/entry-requests"),
                &Value::Null,
            )
            .await
            .ok(StatusCode::CREATED);
        server
            .post(
                ben,
                &format!("/api/v1/call/entry-requests/{}/answer", request.id),
                &json!({ "accept": true }),
            )
            .await
            .error(ErrorCode::NotFound);
        assert!(
            server
                .own_call(ben)
                .await
                .call
                .unwrap()
                .entry_requests
                .is_empty()
        );
        let told = names(&server.call_events(ben, &mut to_ben).await);
        assert!(
            !told.contains(&"call_entry_requested"),
            "hides {hides}: a seat that never connected got {told:?}"
        );
        assert_eq!(
            told.contains(&"call_lock_changed"),
            !hides,
            "hides {hides}: {told:?}"
        );
    }
}

/// ADR 0517: a repeated request for a seat that has not connected does not move the end of
/// that seat. The seat ends when its first time has passed.
#[tokio::test]
async fn a_repeated_request_does_not_keep_a_seat_that_never_connected() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let limits = &server.state.config.limits.calls;
    let usable = limits.token_lifetime + limits.token_tolerance;
    server.enter_call(ann, voice).await;

    let first = server.join_call(ben, voice).await;
    server.clock.advance(usable - Duration::seconds(10));
    let again = server.join_call(ben, voice).await;
    assert_eq!(again.identity, first.identity);
    server.clock.advance(Duration::seconds(20));
    server.check().await;
    assert_eq!(
        server.count("call_members").await,
        1,
        "the second request moved the end of a seat that never connected"
    );

    let mut to_ben = server.full_stream(ben).await;
    let third = server.join_call(ben, voice).await;
    server.clock.advance(usable + Duration::seconds(1));
    let fourth = server.join_call(ben, voice).await;
    assert_ne!(
        fourth.identity, third.identity,
        "a request after the end of a seat is a new entry"
    );
    let own: Vec<Event> = server.call_events(ben, &mut to_ben).await;
    assert!(matches!(
        own.as_slice(),
        [
            Event::OwnCallChanged {
                call: Some(_),
                left: None
            },
            Event::OwnCallChanged {
                call: Some(_),
                left: Some(LeaveReason::NeverConnected)
            }
        ]
    ));
}

/// The actions of the system lines of a channel that `reader` reads, oldest first.
async fn system_lines(
    server: &crate::support::TestServer,
    reader: &crate::support::Member,
    channel_id: junto_protocol::ids::ChannelId,
) -> Vec<String> {
    let page: MessagePage = server
        .get(reader, &format!("/api/v1/channels/{channel_id}/messages"))
        .await
        .ok(StatusCode::OK);
    page.messages
        .into_iter()
        .filter_map(|line| line.system.map(|line| line.action))
        .collect()
}

/// The number of audit entries about recordings in a channel that `reader` reads.
async fn recording_entries(scene: &Scene, reader: &crate::support::Member) -> usize {
    scene
        .server
        .audit_actions(reader, scene.workspace_id)
        .await
        .into_iter()
        .filter(|entry| entry.channel_id == Some(scene.voice.id))
        .map(|entry| format!("{:?}", entry.event))
        .filter(|event| event.starts_with("Recording"))
        .count()
}

/// ADR 0408 and ADR 0517: the private system lines of a call that hides who is inside are
/// for the accounts inside it. An account with a seat that never connected reads none.
#[tokio::test]
async fn a_seat_that_never_connects_reads_no_private_line_of_a_hidden_call() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server
        .call_settings(owner, voice, json!({ "hides_occupants": true }))
        .await;
    let seat = server.enter_call(ann, voice).await;
    server.join_call(ben, voice).await;
    server
        .put(
            ann,
            "/api/v1/call/screen-share",
            &json!({ "audio": "none" }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server.check().await;
    server.clock.advance(Duration::seconds(200));
    server.check().await;
    assert_eq!(server.count("call_members").await, 1);
    server.leave_call(ann).await;

    let of_the_call = ["call_started", "screen_share_started", "call_ended"];
    assert_eq!(system_lines(server, ann, voice).await[2..], of_the_call);
    assert_eq!(
        system_lines(server, ben, voice).await,
        ["channel_created", "channel_call_settings_updated"],
        "an account that never was inside reads the lines of the hidden call"
    );
    let _ = seat;
}

/// ADR 0511 and ADR 0517: the account of a completed move into a call that hides who is
/// inside is inside from the check that saw it there: it is told by the event of its own
/// entry and reads the call then.
#[tokio::test]
async fn a_completed_move_into_a_hidden_call_reads_it_after_the_check() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let garden = server.voice(owner, scene.workspace_id, "garden").await.id;
    server
        .call_settings(owner, garden, json!({ "hides_occupants": true }))
        .await;
    server.enter_call(ann, garden).await;
    server.enter_call(ben, lounge).await;
    server.enter_call(owner, lounge).await;
    let mut to_ben = server.full_stream(ben).await;

    let pending = move_to(server, owner, lounge, ben.account_id, garden).await;
    let join: CallJoin = server
        .post(
            ben,
            &format!("/api/v1/call/moves/{}/join", pending.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    assert!(join.call.hides_occupants && join.call.participants.is_empty());
    let (_room, identity) = server.fake().connect(&join.token).unwrap();
    assert_eq!(identity, join.identity);
    server.check().await;

    let told = names(&server.call_events(ben, &mut to_ben).await);
    assert!(
        told.contains(&"call_participant_joined"),
        "the moved account got {told:?}"
    );
    assert_eq!(
        server.occupants(ben, garden).await,
        [ann.account_id, ben.account_id]
    );
    assert!(server.occupants(owner, garden).await.is_empty());
}

/// ADR 0516 and ADR 0517: the person who ran meeting mode lowers no hand after leaving the
/// call, in the time in which the meeting still names its presenter.
#[tokio::test]
async fn a_presenter_who_left_the_call_does_not_lower_a_hand() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let presenter = server.enter_call(ann, voice).await;
    server.enter_call(ben, voice).await;
    server
        .put(
            ann,
            "/api/v1/call/screen-share",
            &json!({ "audio": "application" }),
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
        .put_empty(ann, &format!("/api/v1/channels/{voice}/call/meeting"))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(ben, "/api/v1/call/hand")
        .await
        .empty(StatusCode::NO_CONTENT);
    let hand = format!(
        "/api/v1/channels/{voice}/call/participants/{}/hand",
        ben.account_id
    );
    let raised = || async {
        sqlx::query_scalar::<_, bool>(
            "select hand_raised_at is not null from call_members where account_id = $1",
        )
        .bind(ben.account_id)
        .fetch_one(&server.db)
        .await
        .unwrap()
    };

    server.leave_call(ann).await;
    server.check().await;
    server.delete(ann, &hand).await.error(ErrorCode::Forbidden);
    assert!(
        raised().await,
        "a presenter outside the call lowered a hand"
    );
}

/// The operator's decision for ADR 0514 and ADR 0604: a call keeps the setting that it
/// started with. A person outside who turns the setting off during a call reads nothing of
/// that call: no participant, no event, no recording. The next call shows who is inside.
#[tokio::test]
async fn turning_the_setting_off_during_a_call_shows_nothing_of_that_call() {
    let studio = Studio::hidden().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    let started = studio.record().await;
    let before = server.snapshot(owner).await.cursor;
    let mut to_owner = server.full_stream(owner).await;

    server
        .call_settings(owner, voice, json!({ "hides_occupants": false }))
        .await;
    server
        .put(
            ann,
            "/api/v1/call/state",
            &json!({ "muted": true, "deafened": false }),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(server.occupants(owner, voice).await.is_empty());
    assert!(
        server
            .snapshot(owner)
            .await
            .workspaces
            .iter()
            .flat_map(|workspace| &workspace.calls)
            .all(|call| call.call.is_none())
    );
    server
        .recording_reply(owner, started.id)
        .await
        .error(ErrorCode::NotFound);
    assert!(
        server
            .recordings(owner, scene.workspace_id)
            .await
            .recordings
            .is_empty()
    );
    let told = server.traffic(owner, &mut to_owner).await;
    let told: Vec<&str> = told.iter().map(|(_, event)| event.name()).collect();
    assert_eq!(
        told,
        ["message_changed", "channel_updated"],
        "the stream of a person outside"
    );
    let mut replay = server.stream(owner).await;
    replay.subscribe(Level::Full, Some(before)).await;
    for person in [ann, ben] {
        assert!(
            !serde_json::to_string(&server.traffic(owner, &mut replay).await)
                .unwrap()
                .contains(&person.account_id.to_string())
        );
    }
    assert_eq!(
        server.occupants(ann, voice).await,
        [ann.account_id, ben.account_id]
    );

    server.leave_call(ann).await;
    server.leave_call(ben).await;
    server.check().await;
    server.enter_call(ann, voice).await;
    assert_eq!(server.occupants(owner, voice).await, [ann.account_id]);
    assert!(
        !server
            .channel_call(owner, voice)
            .await
            .call
            .unwrap()
            .hides_occupants
    );
}

/// The same decision in the other direction: a call that began while the channel showed
/// who is inside keeps showing it after the setting is turned on. The stream and the
/// snapshot of a person outside agree (ADR 0207), and the next call hides who is inside.
#[tokio::test]
async fn turning_the_setting_on_during_a_call_applies_to_the_next_call() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server.enter_call(ann, voice).await;
    let mut to_owner = server.full_stream(owner).await;

    server
        .call_settings(owner, voice, json!({ "hides_occupants": true }))
        .await;
    server.enter_call(ben, voice).await;
    assert_eq!(
        named(server, owner, &mut to_owner, ben.account_id).await,
        ["call_participant_joined"]
    );
    assert_eq!(
        server.occupants(owner, voice).await,
        [ann.account_id, ben.account_id]
    );
    let in_snapshot: Vec<AccountId> = server
        .snapshot(owner)
        .await
        .workspaces
        .iter()
        .flat_map(|workspace| &workspace.calls)
        .filter_map(|call| call.call.as_ref())
        .flat_map(|call| &call.participants)
        .map(|participant| participant.account_id)
        .collect();
    assert_eq!(in_snapshot, [ann.account_id, ben.account_id]);

    server.leave_call(ann).await;
    server.leave_call(ben).await;
    server.check().await;
    server.enter_call(ann, voice).await;
    assert!(server.occupants(owner, voice).await.is_empty());
    assert!(server.channel_call(owner, voice).await.in_use);
    assert!(
        named(server, owner, &mut to_owner, ann.account_id)
            .await
            .iter()
            .all(|name| *name == "call_participant_left"),
        "the events of the next call reach a person outside"
    );
}

/// ADR 0604: the audit entries of a recording follow the recording. A reader inside the
/// call reads them while the call runs, and every reader of the channel when it has ended.
#[tokio::test]
async fn the_audit_entries_of_a_recording_are_read_inside_and_after_the_call() {
    let studio = Studio::hidden().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    studio.record().await;
    assert_eq!(recording_entries(scene, owner).await, 0);

    server.enter_call(owner, voice).await;
    assert_eq!(recording_entries(scene, owner).await, 1);
    for member in [owner, ann, ben] {
        server.leave_call(member).await;
    }
    server.check().await;
    assert_eq!(
        recording_entries(scene, owner).await,
        2,
        "the start and the stop at the end of the call"
    );
}

/// ADR 0515: an account that muted the conversation learns nothing of a ring: no event, no
/// invitation in the snapshot, and no result from the route that declines. The ring counts
/// for the limit of rings as every ring does.
#[tokio::test]
async fn a_ring_for_a_muted_conversation_is_silent_and_counted() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    server
        .put(
            ben,
            &format!("/api/v1/direct-messages/{}/mute", ann.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let mut to_ben = server.full_stream(ben).await;
    let most = server.state.config.limits.calls.rings.attempts;

    for callee in [owner, ben] {
        for _ in 0..most {
            ring(server, ann, callee)
                .await
                .ok::<CallJoin>(StatusCode::OK);
            server.leave_call(ann).await;
        }
        ring(server, ann, callee)
            .await
            .error(ErrorCode::RateLimited);
    }
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());

    server.clock.advance(Duration::hours(1));
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    assert_eq!(server.count("call_invitations").await, 1);
    assert!(server.snapshot(ben).await.call_invitations.is_empty());
    let sent = server.snapshot(ann).await.sent_call_invitations;
    assert_eq!(sent.len(), 1);
    for answer in ["acceptance", "refusal"] {
        server
            .post(
                ben,
                &format!("/api/v1/call/invitations/{}/{answer}", sent[0].id),
                &Value::Null,
            )
            .await
            .error(ErrorCode::NotFound);
    }
    server
        .post(
            ben,
            &format!("/api/v1/direct-calls/{}/refusal", ann.account_id),
            &Value::Null,
        )
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(server.count("call_invitations").await, 1);
    assert!(server.call_events(ben, &mut to_ben).await.is_empty());
}

/// A move can name a destination in another workspace. ADR 0511 asks for "move members" in the source and in
/// the destination and for the access of the moved person to the destination; each of the
/// three holds across workspaces.
#[tokio::test]
async fn a_move_into_another_workspace_needs_both_permissions_and_the_access_of_the_moved_person() {
    use junto_protocol::WorkspacePermission::MoveMembers;
    use junto_protocol::structure::Workspace;

    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let lounge = scene.voice.id;
    let second: Workspace = server
        .post(owner, "/api/v1/workspaces", &json!({ "name": "Second" }))
        .await
        .ok(StatusCode::CREATED);
    let far = server.voice(owner, second.id, "far").await.id;
    server.enter_call(ann, lounge).await;
    server
        .grant_workspace(owner, ben, scene.workspace_id, &[MoveMembers])
        .await;
    let attempt = || async {
        let path = format!(
            "/api/v1/channels/{lounge}/call/participants/{}/move",
            ann.account_id
        );
        server
            .post(ben, &path, &json!({ "to_channel_id": far }))
            .await
    };
    let member =
        |account_id: AccountId| format!("/api/v1/workspaces/{}/members/{account_id}", second.id);

    attempt().await.error(ErrorCode::NotFound);
    server
        .put_empty(owner, &member(ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    attempt().await.error(ErrorCode::Forbidden);
    server
        .grant_workspace(owner, ben, second.id, &[MoveMembers])
        .await;
    attempt().await.error(ErrorCode::Conflict);
    assert_eq!(server.count("call_moves").await, 0);
    server
        .put_empty(owner, &member(ann.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(attempt().await.status, StatusCode::CREATED);
}

/// ADR 0604: the audit entries of a recording are for the people inside while the call
/// runs and hides who is inside, also after the recording was removed in that time. The
/// rule read the call from the row of the recording, so the removal of the recording
/// showed its entries, with the person who recorded, to a reader outside.
#[tokio::test]
async fn the_audit_entries_of_a_recording_that_was_removed_stay_inside_while_the_call_runs() {
    let studio = Studio::hidden().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    server
        .grant_workspace(
            owner,
            ann,
            scene.workspace_id,
            &[junto_protocol::WorkspacePermission::ManageRecordings],
        )
        .await;
    let started = studio.record().await;
    studio.stop(started.id).await;
    server.process().await;
    server
        .delete(ann, &format!("/api/v1/recordings/{}", started.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        recording_entries(scene, owner).await,
        0,
        "a reader outside reads the entries of a recording that was removed during its call"
    );

    server.leave_call(ann).await;
    server.leave_call(ben).await;
    server.check().await;
    assert_eq!(recording_entries(scene, owner).await, 3);
}

/// ADR 0514 and ADR 0517: an account that is not inside a call that hides who is inside
/// learns nothing of its meeting mode. The join token states what the account may publish,
/// and meeting mode blocks every microphone but the presenter's, so the token of a seat
/// told whether meeting mode was on. A seat of such a call gets the same token in both
/// cases, and the admission check gives the account what it may publish when it has seen
/// the account in the room.
#[tokio::test]
async fn the_join_token_of_a_seat_tells_nothing_of_meeting_mode_in_a_hidden_call() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server
        .call_settings(owner, voice, json!({ "hides_occupants": true }))
        .await;
    let presenter = server.enter_call(ann, voice).await;
    let before = server.join_call(ben, voice).await;

    server
        .put(
            ann,
            "/api/v1/call/screen-share",
            &json!({ "audio": "application" }),
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
        .put_empty(ann, &format!("/api/v1/channels/{voice}/call/meeting"))
        .await
        .empty(StatusCode::NO_CONTENT);
    let carl = server.join(owner, "Carl").await;
    let during = server.join_call(&carl, voice).await;
    assert_eq!(
        crate::calls_support::claims(&during)["video"]["canPublishSources"],
        crate::calls_support::claims(&before)["video"]["canPublishSources"],
        "the token of a seat differs with meeting mode"
    );

    // Without meeting mode, the check that sees the account gives it its microphone.
    server
        .delete(ann, &format!("/api/v1/channels/{voice}/call/meeting"))
        .await
        .empty(StatusCode::NO_CONTENT);
    let seat = server.connect(server.join_call(ben, voice).await);
    server.check().await;
    server.check().await;
    let listed = server.fake().participants(&seat.room);
    let own = listed
        .iter()
        .find(|participant| participant.identity == seat.identity)
        .unwrap();
    assert_eq!(
        own.publishing,
        Some(junto_server::media::Publishing::open())
    );
}

/// The part of a migration between two of its lines.
fn part_of<'a>(migration: &'a str, from: &str, to: &str) -> &'a str {
    let start = migration.find(from).expect("the start of the part");
    let end = start + migration[start..].find(to).expect("the end of the part") + to.len();
    &migration[start..end]
}

/// ADR 0517: before migration 0011 an account with a seat that never connected became a
/// reader of the private lines of a call that hides who is inside. For a call that runs at
/// the upgrade, the migration takes that away from an account whose seat the admission
/// check has not seen and of whom no stored event states an entry. An account that was
/// inside before keeps its lines.
#[tokio::test]
async fn the_migration_takes_the_private_lines_from_a_seat_that_never_connected() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let voice = scene.voice.id;
    server
        .call_settings(owner, voice, json!({ "hides_occupants": true }))
        .await;
    server.enter_call(ann, voice).await;
    let carl = server.join(owner, "Carl").await;
    server.enter_call(&carl, voice).await;
    server.leave_call(&carl).await;
    server.check().await;
    server.join_call(&carl, voice).await;
    server.join_call(ben, voice).await;
    let dora = server.join(owner, "Dora").await;
    server.join_call(&dora, voice).await;
    let call: uuid::Uuid = sqlx::query_scalar("select id from calls where channel_id = $1")
        .bind(voice)
        .fetch_one(&server.db)
        .await
        .unwrap();
    // The state that the earlier rule left: the seats of Ben and of Dora are readers of
    // every line, in the lines and in the stored events of the lines.
    let mut earlier = server.db.begin().await.unwrap();
    sqlx::raw_sql("alter table events disable trigger events_no_change")
        .execute(&mut *earlier)
        .await
        .unwrap();
    for seat in [ben, &dora] {
        sqlx::query("insert into call_message_members values ($1, $2)")
            .bind(call)
            .bind(seat.account_id)
            .execute(&mut *earlier)
            .await
            .unwrap();
        sqlx::query("update messages set readers = array_append(readers, $2) where call_id = $1")
            .bind(call)
            .bind(seat.account_id)
            .execute(&mut *earlier)
            .await
            .unwrap();
        sqlx::query(
            "update events
             set audience = jsonb_set(audience, '{readers}', audience -> 'readers' || $2)
             where audience ->> 'to' = 'system_message' and audience ->> 'call_id' = $1
               and jsonb_typeof(audience -> 'readers') = 'array'",
        )
        .bind(call.to_string())
        .bind(json!([seat.account_id]))
        .execute(&mut *earlier)
        .await
        .unwrap();
    }
    sqlx::raw_sql("alter table events enable trigger events_no_change")
        .execute(&mut *earlier)
        .await
        .unwrap();
    earlier.commit().await.unwrap();
    for seat in [ben, &dora] {
        assert!(
            system_lines(server, seat, voice)
                .await
                .contains(&"call_started".to_owned())
        );
    }
    let named_in_events = |account_id: AccountId| async move {
        sqlx::query_scalar::<_, i64>(
            "select count(*) from events
             where audience ->> 'to' = 'system_message' and audience -> 'readers' ? $1",
        )
        .bind(account_id.to_string())
        .fetch_one(&server.db)
        .await
        .unwrap()
    };
    let of_ann = named_in_events(ann.account_id).await;
    assert!(of_ann > 0 && named_in_events(ben.account_id).await == of_ann);

    let migration = include_str!("../../migrations/0011_calls_inside.sql");
    let reconcile = part_of(
        migration,
        "create temporary table seats_never_seen",
        "alter table events enable trigger events_no_change;",
    );
    let mut upgrade = server.db.begin().await.unwrap();
    sqlx::raw_sql(sqlx::AssertSqlSafe(reconcile.to_owned()))
        .execute(&mut *upgrade)
        .await
        .unwrap();
    upgrade.commit().await.unwrap();

    for seat in [ben, &dora] {
        let lines = system_lines(server, seat, voice).await;
        assert!(
            !lines.contains(&"call_started".to_owned()),
            "a seat that never connected still reads {lines:?}"
        );
        assert_eq!(named_in_events(seat.account_id).await, 0);
    }
    assert_eq!(named_in_events(ann.account_id).await, of_ann);
    assert!(
        system_lines(server, ann, voice)
            .await
            .contains(&"call_started".to_owned())
    );
    let members: Vec<AccountId> = sqlx::query_scalar(
        "select account_id from call_message_members where call_id = $1 order by account_id",
    )
    .bind(call)
    .fetch_all(&server.db)
    .await
    .unwrap();
    // Carl was seen in the room before and has a seat that was not seen: a stored event
    // states the entry, so Carl stays.
    let mut expected = vec![ann.account_id, carl.account_id];
    expected.sort();
    assert_eq!(members, expected);
}
