//! The system lines of a direct-message call in the conversation of its two accounts, the
//! notice and the unread sign of a call that was not answered (ADR 0518).

use junto_protocol::call::CallJoin;
use junto_protocol::events::Level;
use time::Duration;

use crate::calls_review2::ring;
use crate::calls_support::Scene;
use crate::discovery_support::*;
use crate::plan_support::{analyze, explained, index_names, visited_rows};

const STARTED: &str = "direct_call_started";
const NOT_ANSWERED: &str = "direct_call_not_answered";
const DECLINED: &str = "direct_call_declined";
const ENDED: &str = "direct_call_ended";

async fn conversation(
    server: &TestServer,
    member: &Member,
    other: &Member,
) -> Option<Conversation> {
    let listed: Vec<Conversation> = server
        .get(member, "/api/v1/conversations")
        .await
        .ok(StatusCode::OK);
    let pair = (
        member.account_id.min(other.account_id),
        member.account_id.max(other.account_id),
    );
    listed
        .into_iter()
        .find(|conversation| (conversation.low_account_id, conversation.high_account_id) == pair)
}

async fn history(server: &TestServer, member: &Member, other: &Member) -> Vec<Message> {
    let Some(conversation) = conversation(server, member, other).await else {
        return Vec::new();
    };
    server
        .get(
            member,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
        )
        .await
        .ok::<MessagePage>(StatusCode::OK)
        .messages
}

/// The actions of the lines of the conversation, in their order.
async fn lines(server: &TestServer, member: &Member, other: &Member) -> Vec<String> {
    history(server, member, other)
        .await
        .into_iter()
        .filter_map(|message| message.system.map(|system| system.action))
        .collect()
}

async fn state(server: &TestServer, member: &Member, other: &Member) -> MessageState {
    let conversation = conversation(server, member, other)
        .await
        .expect("a conversation");
    server
        .get(
            member,
            &format!("/api/v1/conversations/{}/messages/read", conversation.id),
        )
        .await
        .ok(StatusCode::OK)
}

async fn mute(server: &TestServer, member: &Member, other: &Member) {
    server
        .put(
            member,
            &format!("/api/v1/direct-messages/{}/mute", other.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
}

/// The ring ends without an answer: its 60 seconds pass and the check sees it.
async fn pass_the_ring(server: &TestServer) {
    let lifetime = server.state.config.limits.calls.invitation_lifetime;
    server.clock.advance(lifetime + Duration::seconds(1));
    server.check().await;
}

#[tokio::test]
async fn a_ring_writes_the_started_line_and_opens_the_conversation() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    assert!(conversation(server, ann, ben).await.is_none());
    let before = server.snapshot(ben).await;

    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    for (reader, other) in [(ann, ben), (ben, ann)] {
        let messages = history(server, reader, other).await;
        assert_eq!(messages.len(), 1);
        let line = messages[0].system.as_ref().expect("a system line");
        assert_eq!(line.action, STARTED);
        assert_eq!(line.actor_id, Some(ann.account_id));
        assert_eq!(line.subject_id, Some(ben.account_id));
        assert_eq!(line.duration_seconds, None);
        assert_eq!(messages[0].author_id, None);
        assert_eq!(messages[0].position, 1);
    }
    let told = events(server, ben, before.cursor, Level::Background).await;
    let opened = conversation(server, ben, ann)
        .await
        .expect("a conversation");
    assert!(
        told.contains(&Event::ConversationOpened {
            conversation: opened.clone()
        }),
        "{told:?}"
    );
    assert!(
        told.iter().any(|event| matches!(event,
            Event::MessageChanged { place: MessagePlace::Direct { conversation_id, .. }, position: 1, deleted: false, .. }
                if *conversation_id == opened.id)),
        "{told:?}"
    );
    // A second ring of the same call does not exist, and a new token writes no line.
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    assert_eq!(lines(server, ann, ben).await, [STARTED]);
    // The line of the start is no unread message and no notice.
    assert_eq!(state(server, ben, ann).await.unread, 0);
    assert!(notices(server, ben).await.is_empty());
}

#[tokio::test]
async fn a_call_that_is_not_answered_is_a_line_a_notice_and_an_unread_sign_for_the_callee() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    let before = server.snapshot(ben).await;
    pass_the_ring(server).await;

    assert_eq!(lines(server, ben, ann).await, [STARTED, NOT_ANSWERED]);
    let missed = history(server, ben, ann).await.pop().expect("a line");
    let line = missed.system.as_ref().expect("a system line");
    assert_eq!(line.actor_id, Some(ann.account_id));
    assert_eq!(line.subject_id, Some(ben.account_id));

    let callee = state(server, ben, ann).await;
    assert_eq!((callee.unread, callee.unread_sign), (1, true));
    let caller = state(server, ann, ben).await;
    assert_eq!((caller.unread, caller.unread_sign), (0, false));

    let listed = notices(server, ben).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, NotificationKind::MissedCall);
    assert_eq!(listed[0].message_id, missed.id);
    assert_eq!(listed[0].read_at, None);
    assert!(notices(server, ann).await.is_empty());
    let snapshot = server.snapshot(ben).await;
    assert_eq!(snapshot.notifications.len(), 1);
    assert_eq!(snapshot.notifications[0].kind, NotificationKind::MissedCall);

    // A connection at the background level receives the line and the notice.
    let told = events(server, ben, before.cursor, Level::Background).await;
    assert!(
        told.contains(&Event::NotificationChanged {
            message_id: missed.id
        }),
        "{told:?}"
    );
    assert!(
        told.iter().any(
            |event| matches!(event, Event::MessageChanged { message_id, .. } if *message_id == missed.id)
        ),
        "{told:?}"
    );

    // Reading the conversation reads the notice and ends the sign.
    let conversation = conversation(server, ben, ann)
        .await
        .expect("a conversation");
    server
        .put(
            ben,
            &format!("/api/v1/conversations/{}/messages/read", conversation.id),
            &json!({ "position": missed.position }),
        )
        .await
        .ok::<MessageState>(StatusCode::OK);
    let read = state(server, ben, ann).await;
    assert_eq!((read.unread, read.unread_sign), (0, false));
    assert!(notices(server, ben).await[0].read_at.is_some());
}

#[tokio::test]
async fn a_caller_who_leaves_before_the_answer_leaves_a_missed_call() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    server.enter_direct(ann, ben).await;
    server.clock.advance(Duration::seconds(5));
    server.leave_call(ann).await;

    assert_eq!(lines(server, ben, ann).await, [STARTED, NOT_ANSWERED]);
    assert_eq!(notices(server, ben).await.len(), 1);
    assert_eq!(state(server, ben, ann).await.unread, 1);
    // The 60 seconds of the ring pass later and add nothing.
    pass_the_ring(server).await;
    assert_eq!(lines(server, ben, ann).await, [STARTED, NOT_ANSWERED]);
}

/// An account that waits alone rings again with a new place in the same call. The call
/// started once, and each ring that is not answered has its line.
#[tokio::test]
async fn a_second_ring_of_one_call_writes_no_second_started_line() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let limits = server.state.config.limits.calls.clone();
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    pass_the_ring(server).await;
    assert_eq!(lines(server, ben, ann).await, [STARTED, NOT_ANSWERED]);
    assert_eq!(
        server.count("calls").await,
        1,
        "the caller still has its place"
    );

    // The place of the caller never connected and its time ends. The next request of the
    // caller is a new place in the same call, and it rings again (ADR 0515).
    server
        .clock
        .advance(limits.token_lifetime + limits.token_tolerance);
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    assert_eq!(server.count("calls").await, 1);
    assert_eq!(server.count("call_invitations").await, 1);
    assert_eq!(lines(server, ben, ann).await, [STARTED, NOT_ANSWERED]);

    pass_the_ring(server).await;
    assert_eq!(
        lines(server, ben, ann).await,
        [STARTED, NOT_ANSWERED, NOT_ANSWERED]
    );
    assert_eq!(notices(server, ben).await.len(), 2);
    assert_eq!(state(server, ben, ann).await.unread, 2);
}

#[tokio::test]
async fn a_declined_call_is_a_line_without_a_notice() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    server.enter_direct(ann, ben).await;
    server
        .post(
            ben,
            &format!("/api/v1/direct-calls/{}/refusal", ann.account_id),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);

    assert_eq!(lines(server, ann, ben).await, [STARTED, DECLINED]);
    let declined = history(server, ann, ben).await.pop().expect("a line");
    let line = declined.system.as_ref().expect("a system line");
    assert_eq!(line.actor_id, Some(ben.account_id));
    assert_eq!(line.subject_id, Some(ann.account_id));
    for member in [ann, ben] {
        assert!(notices(server, member).await.is_empty());
    }
    assert_eq!(state(server, ben, ann).await.unread, 0);
    assert_eq!(state(server, ann, ben).await.unread, 0);
    // The caller leaves the call that nobody answered: no further line.
    server.leave_call(ann).await;
    pass_the_ring(server).await;
    assert_eq!(lines(server, ann, ben).await, [STARTED, DECLINED]);
}

#[tokio::test]
async fn an_answered_call_ends_with_a_line_that_states_its_duration() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    server.enter_direct(ann, ben).await;
    server.clock.advance(Duration::seconds(7));
    server.enter_direct(ben, ann).await;
    assert_eq!(lines(server, ann, ben).await, [STARTED]);
    server.clock.advance(Duration::seconds(125));
    server.leave_call(ben).await;
    assert_eq!(lines(server, ann, ben).await, [STARTED]);
    server.clock.advance(Duration::seconds(3));
    server.leave_call(ann).await;

    assert_eq!(lines(server, ann, ben).await, [STARTED, ENDED]);
    let ended = history(server, ben, ann).await.pop().expect("a line");
    let line = ended.system.as_ref().expect("a system line");
    // From the check that saw both accounts in the room to the moment at which one left.
    assert_eq!(line.duration_seconds, Some(125));
    assert_eq!(line.actor_id, Some(ann.account_id));
    assert_eq!(line.subject_id, Some(ben.account_id));
    for member in [ann, ben] {
        assert!(notices(server, member).await.is_empty());
    }
    assert_eq!(state(server, ben, ann).await.unread, 0);

    // A call that nobody answered has no line of its end.
    server.clock.advance(Duration::hours(1));
    server.enter_direct(ben, ann).await;
    pass_the_ring(server).await;
    server.leave_call(ben).await;
    assert_eq!(
        lines(server, ann, ben).await,
        [STARTED, ENDED, STARTED, NOT_ANSWERED]
    );
}

/// What a caller reads of one call that is not answered: the events of the stream, the
/// lines with their distance in time, the own notices, the own unread state, and the
/// invitations of the snapshot during the ring and after it. `hangs_up` is the time after
/// which the caller leaves; without it the ring passes its 60 seconds.
async fn what_the_caller_reads(
    server: &TestServer,
    caller: &Member,
    callee: &Member,
    hangs_up: Option<Duration>,
) -> (
    Vec<String>,
    Vec<(String, Duration)>,
    usize,
    (u64, bool),
    [usize; 2],
) {
    let mut stream = server.full_stream(caller).await;
    server.enter_direct(caller, callee).await;
    let during = server.snapshot(caller).await.sent_call_invitations.len();
    match hangs_up {
        Some(after) => {
            server.clock.advance(after);
            server.leave_call(caller).await;
        }
        None => pass_the_ring(server).await,
    }
    let told = server
        .traffic(caller, &mut stream)
        .await
        .into_iter()
        .map(|(_, event)| match event {
            Event::MessageChanged {
                position, deleted, ..
            } => format!("message_changed {position} {deleted}"),
            Event::CallInvitationResolved { outcome, .. } => {
                format!("call_invitation_resolved {outcome:?}")
            }
            other => other.name().to_owned(),
        })
        .collect();
    let read = state(server, caller, callee).await;
    let after = server.snapshot(caller).await.sent_call_invitations.len();
    let history = history(server, caller, callee).await;
    let first = history[0].created_at;
    let written = history
        .into_iter()
        .filter_map(|message| {
            let at = message.created_at - first;
            message.system.map(|system| (system.action, at))
        })
        .collect();
    let seen = (
        told,
        written,
        notices(server, caller).await.len(),
        (read.unread, read.unread_sign),
        [during, after],
    );
    server.leave_call(caller).await;
    seen
}

#[tokio::test]
async fn the_caller_cannot_tell_a_muted_conversation_from_no_answer_by_the_lines() {
    for hangs_up in [None, Some(Duration::seconds(5))] {
        let scene = Scene::start().await;
        let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
        let quiet = server.join(owner, "Quiet").await;
        mute(server, ben, ann).await;
        settings(
            server,
            &quiet,
            uuid::Uuid::nil(),
            json!({ "do_not_disturb": true }),
        )
        .await;

        let unanswered = what_the_caller_reads(server, ann, owner, hangs_up).await;
        let actions: Vec<&str> = unanswered.1.iter().map(|(action, _)| &**action).collect();
        assert_eq!(actions, [STARTED, NOT_ANSWERED]);
        assert_eq!(unanswered.4, [1, 0], "the invitation waits during the ring");
        assert!(
            unanswered.0.contains(&"message_changed 2 false".to_owned()),
            "{:?}",
            unanswered.0
        );
        let muted = what_the_caller_reads(server, ann, ben, hangs_up).await;
        assert_eq!(muted, unanswered, "a muted conversation reads differently");
        let in_quiet_time = what_the_caller_reads(server, ann, &quiet, hangs_up).await;
        assert_eq!(in_quiet_time, unanswered, "a quiet time reads differently");
    }
}

#[tokio::test]
async fn a_muted_callee_gets_the_line_without_notice_and_sign() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    mute(server, ben, ann).await;
    let before = server.snapshot(ben).await;
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    pass_the_ring(server).await;

    assert_eq!(lines(server, ben, ann).await, [STARTED, NOT_ANSWERED]);
    assert!(notices(server, ben).await.is_empty());
    assert!(server.snapshot(ben).await.notifications.is_empty());
    let read = state(server, ben, ann).await;
    assert!(read.muted);
    assert!(!read.unread_sign);
    let told = events(server, ben, before.cursor, Level::Full).await;
    assert!(
        told.iter().all(|event| !matches!(
            event,
            Event::NotificationChanged { .. }
                | Event::CallInvited { .. }
                | Event::CallInvitationResolved { .. }
        )),
        "{told:?}"
    );
    let stored: i64 = sqlx::query_scalar("select count(*) from notifications")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(stored, 0);
}

#[tokio::test]
async fn a_quiet_callee_gets_the_notice_without_an_event() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    settings(
        server,
        ben,
        uuid::Uuid::nil(),
        json!({ "do_not_disturb": true }),
    )
    .await;
    let before = server.snapshot(ben).await;
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    pass_the_ring(server).await;

    assert_eq!(lines(server, ben, ann).await, [STARTED, NOT_ANSWERED]);
    let told = events(server, ben, before.cursor, Level::Full).await;
    assert!(
        told.iter().all(|event| !matches!(
            event,
            Event::NotificationChanged { .. } | Event::CallInvited { .. }
        )),
        "{told:?}"
    );
    // The notice is stored and the unread state counts the call.
    let listed = notices(server, ben).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, NotificationKind::MissedCall);
    assert_eq!(state(server, ben, ann).await.unread, 1);

    // The quiet time ends. The event of the notice has no receiver, so a device that reads
    // the log from before the call gets no late alert (ADR 0409).
    settings(
        server,
        ben,
        uuid::Uuid::nil(),
        json!({ "do_not_disturb": false }),
    )
    .await;
    let replayed = events(server, ben, before.cursor, Level::Full).await;
    assert!(
        replayed
            .iter()
            .any(|event| matches!(event, Event::MessageChanged { .. })),
        "{replayed:?}"
    );
    assert!(
        replayed
            .iter()
            .all(|event| !matches!(event, Event::NotificationChanged { .. })),
        "{replayed:?}"
    );
}

#[tokio::test]
async fn only_the_two_accounts_read_the_lines_of_a_direct_call() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    let third = server.join(owner, "Third").await;
    let outside = [owner, &third];
    let mut cursors = Vec::new();
    for member in outside {
        cursors.push(server.snapshot(member).await.cursor);
    }
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    pass_the_ring(server).await;
    let conversation = conversation(server, ann, ben)
        .await
        .expect("a conversation");
    let missed = history(server, ben, ann).await.pop().expect("a line");

    for (member, cursor) in outside.into_iter().zip(cursors) {
        server
            .get(
                member,
                &format!("/api/v1/conversations/{}/messages", conversation.id),
            )
            .await
            .error(ErrorCode::NotFound);
        server
            .get(member, &message(missed.id))
            .await
            .error(ErrorCode::NotFound);
        server
            .put_empty(member, &format!("/api/v1/notifications/{}/read", missed.id))
            .await
            .error(ErrorCode::NotFound);
        assert!(notices(server, member).await.is_empty());
        let snapshot = server.snapshot(member).await;
        assert!(snapshot.notifications.is_empty());
        assert!(
            snapshot
                .messages
                .iter()
                .all(|state| !matches!(state.place, MessagePlace::Direct { .. }))
        );
        let told = events(server, member, cursor, Level::Full).await;
        assert!(
            told.iter().all(|event| !matches!(
                event,
                Event::MessageChanged { .. }
                    | Event::NotificationChanged { .. }
                    | Event::ConversationOpened { .. }
            )),
            "{told:?}"
        );
    }
}

#[tokio::test]
async fn a_missed_call_is_hidden_while_the_contact_rule_fails() {
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let shared: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Shared" }))
        .await
        .ok(StatusCode::CREATED);
    let cat = server.join_into(&owner, "Cat", &[shared.id]).await;
    let dan = server.join_into(&owner, "Dan", &[shared.id]).await;
    server.enter_direct(&cat, &dan).await;
    pass_the_ring(&server).await;
    server.leave_call(&cat).await;
    let found = conversation(&server, &dan, &cat)
        .await
        .expect("a conversation");
    let missed = history(&server, &dan, &cat).await.pop().expect("a line");
    assert_eq!(notices(&server, &dan).await.len(), 1);

    // The two accounts no longer share a workspace, and neither administers the server.
    server
        .delete(&owner, &format!("/api/v1/workspaces/{}", shared.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let before = server.snapshot(&dan).await;
    assert!(before.notifications.is_empty());
    assert!(notices(&server, &dan).await.is_empty());
    assert!(conversation(&server, &dan, &cat).await.is_none());
    server
        .get(
            &dan,
            &format!("/api/v1/conversations/{}/messages", found.id),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .get(&dan, &message(missed.id))
        .await
        .error(ErrorCode::NotFound);
    server
        .put_empty(&dan, &format!("/api/v1/notifications/{}/read", missed.id))
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn nobody_changes_a_line_of_a_direct_call() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    pass_the_ring(server).await;
    for line in history(server, ann, ben).await {
        for member in [ann, ben] {
            server
                .put(member, &message(line.id), &text("another text"))
                .await
                .error(ErrorCode::Forbidden);
            server
                .delete(member, &message(line.id))
                .await
                .error(ErrorCode::Forbidden);
        }
    }
    assert_eq!(lines(server, ann, ben).await, [STARTED, NOT_ANSWERED]);
}

#[tokio::test]
async fn a_line_of_a_direct_call_is_no_search_hit() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    pass_the_ring(server).await;
    for words in ["direct_call_not_answered", "direct", "call"] {
        for member in [ann, ben] {
            assert!(search(server, member, words).await.results.is_empty());
        }
    }
}

/// An account that takes a place and never connects to the media server has not answered:
/// the call has no duration, and the account has a missed call.
#[tokio::test]
async fn a_place_that_never_connects_is_no_answer() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let limits = server.state.config.limits.calls.clone();
    server.enter_direct(ann, ben).await;
    ring(server, ben, ann).await.ok::<CallJoin>(StatusCode::OK);
    server.check().await;
    server.clock.advance(Duration::seconds(30));
    server.leave_call(ann).await;
    assert_eq!(lines(server, ann, ben).await, [STARTED]);
    // The place of the other account ends, and the call with it.
    server
        .clock
        .advance(limits.token_lifetime + limits.token_tolerance);
    server.check().await;
    assert_eq!(server.count("calls").await, 0);

    assert_eq!(lines(server, ann, ben).await, [STARTED, NOT_ANSWERED]);
    let listed = notices(server, ben).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, NotificationKind::MissedCall);
    assert!(notices(server, ann).await.is_empty());
}

/// The account that was called was inside the call, and the caller never was: the call has
/// no duration, and it is no missed call for the account that was inside.
#[tokio::test]
async fn an_account_that_was_inside_alone_has_no_missed_call() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let limits = server.state.config.limits.calls.clone();
    // The device of the caller never reaches the media server.
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    server.enter_direct(ben, ann).await;
    server
        .clock
        .advance(limits.token_lifetime + limits.token_tolerance);
    server.check().await;
    server.leave_call(ben).await;
    assert_eq!(server.count("calls").await, 0);

    assert_eq!(lines(server, ben, ann).await, [STARTED]);
    for member in [ann, ben] {
        assert!(notices(server, member).await.is_empty());
    }
    assert_eq!(state(server, ben, ann).await.unread, 0);
}

/// The duration runs to the moment from which one account was alone for good: an account
/// that leaves and enters again continues the same time.
#[tokio::test]
async fn the_duration_ends_when_one_account_is_alone_for_good() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    server.enter_direct(ann, ben).await;
    server.enter_direct(ben, ann).await;
    server.clock.advance(Duration::seconds(10));
    server.leave_call(ben).await;
    server.clock.advance(Duration::seconds(10));
    server.enter_direct(ben, ann).await;
    server.clock.advance(Duration::seconds(30));
    server.leave_call(ben).await;
    server.clock.advance(Duration::seconds(40));
    server.leave_call(ann).await;

    assert_eq!(lines(server, ann, ben).await, [STARTED, ENDED]);
    let ended = history(server, ann, ben).await.pop().expect("a line");
    assert_eq!(ended.system.unwrap().duration_seconds, Some(50));
}

/// An account whose connection to the media server is lost is not inside for the answer:
/// the other account that connects in that time has not met it.
#[tokio::test]
async fn an_account_whose_connection_is_lost_is_not_met() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    let limits = server.state.config.limits.calls.clone();
    let seat = server.enter_direct(ann, ben).await;
    server.fake().leave(&seat.room, &seat.identity);
    server.check().await;
    server.enter_direct(ben, ann).await;
    server.clock.advance(limits.absence_grace);
    server.check().await;
    server.leave_call(ben).await;
    assert_eq!(server.count("calls").await, 0);
    let written = lines(server, ann, ben).await;
    assert!(!written.contains(&ENDED.to_owned()), "{written:?}");
}

/// A line is not written for an account that is no longer active.
#[tokio::test]
async fn a_ring_that_ends_after_a_ban_writes_no_line() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    server.enter_direct(ann, ben).await;
    server
        .put_empty(owner, &format!("/api/v1/accounts/{}/ban", ben.account_id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.leave_call(ann).await;
    pass_the_ring(server).await;
    assert_eq!(server.count("calls").await, 0);
    let actions: Vec<String> =
        sqlx::query_scalar("select system->>'action' from messages order by position")
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert!(!actions.contains(&NOT_ANSWERED.to_owned()), "{actions:?}");
    assert_eq!(server.count("notifications").await, 0);
}

/// Two accounts that no longer reach each other get no line for what ends then (ADR 0307):
/// nothing waits unread in the conversation for the day on which they share a workspace
/// again.
#[tokio::test]
async fn a_ring_that_ends_after_the_contact_rule_ended_writes_no_line() {
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let shared: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Shared" }))
        .await
        .ok(StatusCode::CREATED);
    let cat = server.join_into(&owner, "Cat", &[shared.id]).await;
    let dan = server.join_into(&owner, "Dan", &[shared.id]).await;
    server.enter_direct(&cat, &dan).await;
    server
        .delete(&owner, &format!("/api/v1/workspaces/{}", shared.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    // The caller leaves before a check has looked at the room.
    server.leave_call(&cat).await;
    assert_eq!(server.count("calls").await, 0);

    let again: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Again" }))
        .await
        .ok(StatusCode::CREATED);
    for member in [&cat, &dan] {
        server
            .put_empty(
                &owner,
                &format!(
                    "/api/v1/workspaces/{}/members/{}",
                    again.id, member.account_id
                ),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    assert_eq!(lines(&server, &dan, &cat).await, [STARTED]);
    assert!(notices(&server, &dan).await.is_empty());
    assert_eq!(state(&server, &dan, &cat).await.unread, 0);
}

/// The retention of direct messages removes the lines of calls with their notices.
#[tokio::test]
async fn the_retention_of_direct_messages_removes_the_lines_of_a_call() {
    let scene = Scene::start().await;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    pass_the_ring(server).await;
    server.leave_call(ann).await;
    assert_eq!(notices(server, ben).await.len(), 1);

    server
        .put(
            owner,
            "/api/v1/direct-message-retention",
            &json!({ "days": 1 }),
        )
        .await
        .ok::<Retention>(StatusCode::OK);
    server.clock.advance(Duration::days(2));
    junto_server::messages::retention::run(&server.state)
        .await
        .unwrap();
    assert!(lines(server, ben, ann).await.is_empty());
    assert!(notices(server, ben).await.is_empty());
    let read = state(server, ben, ann).await;
    assert_eq!((read.unread, read.unread_sign), (0, false));
    assert_eq!(server.count("notifications").await, 0);
}

/// The unread count of a conversation with many missed calls reads them through an index:
/// the callee reads at most the 100 that the count states, and the caller reads none of
/// the lines that count for the callee.
#[tokio::test]
async fn the_unread_count_reads_missed_calls_by_an_index() {
    let scene = Scene::start().await;
    let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
    ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
    pass_the_ring(server).await;
    let conversation = conversation(server, ben, ann)
        .await
        .expect("a conversation");
    let originals: Vec<uuid::Uuid> =
        sqlx::query_scalar("select id from messages where place_id=$1 and notifies is not null")
            .bind(conversation.id)
            .fetch_all(&server.db)
            .await
            .unwrap();
    let (first, last) = (1, 20_000);
    sqlx::query(
        "insert into messages (id, place_id, position, created_at, system, notifies)
     select gen_random_uuid(), m.place_id, m.position + n, m.created_at, m.system, m.notifies
     from messages m cross join generate_series($2::int, $3::int) n
     where m.id = any($1)",
    )
    .bind(&originals)
    .bind(first)
    .bind(last)
    .execute(&mut server.test_data().await)
    .await
    .unwrap();
    analyze(&server.db, &["messages"]).await;

    assert_eq!(
        state(server, ben, ann).await.unread,
        100,
        "100 means 100 or more"
    );
    assert_eq!(state(server, ann, ben).await.unread, 0);
    for reader in [ben, ann] {
        let plan: Value = sqlx::query_scalar(explained(&junto_server::messages::state_query()))
            .bind(vec![conversation.id.as_uuid()])
            .bind(reader.account_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
        assert!(
            index_names(&plan)
                .iter()
                .any(|name| name == "messages_notifies"),
            "{plan}"
        );
        assert!(visited_rows(&plan[0]["Plan"]) < 1000, "{plan}");
    }
}

/// The log is trimmed to a number of changes, so the trimmed boundary that a caller can probe
/// with cursors is the same whether its ring was silent or not (ADR 0207, ADR 0515).
#[tokio::test]
async fn the_trimmed_boundary_does_not_show_a_muted_ring() {
    let mut boundaries = Vec::new();
    for muted in [false, true] {
        let scene = Scene::on(
            TestServer::start_for_calls_with(|config| config.limits.stream.events_kept = 16).await,
        )
        .await;
        let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
        if muted {
            mute(server, ben, ann).await;
        }
        let before =
            junto_server::events::log::change_of(server.snapshot(ann).await.cursor.position);
        let join = ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
        server.connect(join);
        server.check().await;
        server.leave_call(ann).await;
        // Each rename moves the kept part of the log by one, so the boundary passes over the
        // ring and its end one step at a time.
        let mut seen = Vec::new();
        for rename in 0..20 {
            server
                .patch(
                    ann,
                    &format!("/api/v1/account/devices/{}", ann.device_id),
                    &json!({ "name": format!("device {rename}") }),
                )
                .await
                .ok::<serde_json::Value>(StatusCode::OK);
            junto_server::events::log::trim(&server.state)
                .await
                .unwrap();
            let trimmed: i64 = sqlx::query_scalar("select trimmed_through from event_stream")
                .fetch_one(&server.db)
                .await
                .unwrap();
            seen.push(trimmed.max(before) - before);
        }
        boundaries.push(seen);
    }
    assert_eq!(boundaries[0], boundaries[1]);
}

#[tokio::test]
async fn muted_and_unanswered_calls_consume_the_same_positions() {
    let mut counts = Vec::new();
    for cancelled in [false, true] {
        for mode in 0..3 {
            let silent = mode != 0;
            let scene = Scene::start().await;
            let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
            if mode == 1 {
                mute(server, ben, ann).await;
            } else if mode == 2 {
                settings(
                    server,
                    ben,
                    uuid::Uuid::nil(),
                    json!({"do_not_disturb":true}),
                )
                .await;
            }
            let before = server.snapshot(ann).await.cursor;
            let join = ring(server, ann, ben).await.ok::<CallJoin>(StatusCode::OK);
            let rung = server.snapshot(ann).await.cursor.position;
            server.connect(join);
            server.check().await;
            if cancelled {
                server.leave_call(ann).await;
            } else {
                pass_the_ring(server).await;
            }
            let expired = server.snapshot(ann).await.cursor.position;
            if !cancelled {
                server.leave_call(ann).await;
            }
            let ended = server.snapshot(ann).await.cursor.position;
            let mut sender_stream = server.stream(ann).await;
            sender_stream.subscribe(Level::Full, Some(before)).await;
            let positions: Vec<_> = server
                .traffic(ann, &mut sender_stream)
                .await
                .into_iter()
                .map(|(position, event)| {
                    (
                        position - before.position,
                        serde_json::to_value(event).unwrap()["type"].clone(),
                    )
                })
                .collect();
            let read = state(server, ann, ben).await;
            counts.push((
                rung - before.position,
                expired - rung,
                ended - expired,
                positions,
                read.unread,
                history(server, ann, ben).await.len(),
                notices(server, ann).await.len(),
            ));
            let received = events(server, ben, before, Level::Full).await;
            assert_eq!(
                received
                    .iter()
                    .any(|event| matches!(event, Event::CallInvited { .. })),
                !silent
            );
            assert_eq!(
                received
                    .iter()
                    .any(|event| matches!(event, Event::CallInvitationResolved { .. })),
                !silent
            );
            assert_eq!(notices(server, ben).await.is_empty(), mode == 1);
        }
    }
    for modes in counts.chunks(3) {
        assert_eq!(modes[0], modes[1]);
        assert_eq!(modes[0], modes[2]);
    }
}

#[tokio::test]
async fn a_sender_cannot_measure_a_mute_from_text_positions_or_counts() {
    let mut observations = Vec::new();
    for muted in [false, true] {
        let scene = Scene::start().await;
        let (server, ann, ben) = (&scene.server, &scene.ann, &scene.ben);
        let conversation: Conversation = server
            .post(
                ann,
                "/api/v1/conversations",
                &json!({"account_id":ben.account_id}),
            )
            .await
            .ok(StatusCode::OK);
        if muted {
            mute(server, ben, ann).await;
        }
        let before = server.snapshot(ann).await.cursor;
        let sent: Message = server
            .post(
                ann,
                &format!("/api/v1/conversations/{}/messages", conversation.id),
                &text("Hello"),
            )
            .await
            .ok(StatusCode::CREATED);
        let after = server.snapshot(ann).await.cursor;
        let mut stream = server.stream(ann).await;
        stream.subscribe(Level::Full, Some(before)).await;
        let positions: Vec<_> = server
            .traffic(ann, &mut stream)
            .await
            .into_iter()
            .map(|(position, event)| {
                (
                    position - before.position,
                    serde_json::to_value(event).unwrap()["type"].clone(),
                )
            })
            .collect();
        let read = state(server, ann, ben).await;
        observations.push((
            after.position - before.position,
            positions,
            sent.position,
            read.unread,
            history(server, ann, ben).await.len(),
            notices(server, ann).await.len(),
        ));
        assert_eq!(notices(server, ben).await.is_empty(), muted);
    }
    assert_eq!(observations[0], observations[1]);
}

#[tokio::test]
async fn review2_sender_observations_match_for_all_text_variants() {
    for in_channel in [false, true] {
        let mut modes = Vec::new();
        for mode in 0..3 {
            let directory = crate::support::Scratch::new();
            let fixture = crate::support::TestServer::start_with(|config| {
                config.attachments_directory = Some(directory.0.clone());
            })
            .await;
            fixture.state.attachment_space.use_disk(std::sync::Arc::new(
                junto_server::storage::FakeDisk::new(crate::support::FREE_DISK),
            ));
            let author = fixture.owner().await;
            let recipient = fixture.join(&author, "Recipient").await;
            let (server, ann, ben) = (&fixture, &author, &recipient);
            let (route, upload_place) = if in_channel {
                let channel = crate::messages_support::channel(server, ann, "text").await;
                settings(
                    server,
                    ben,
                    channel.id,
                    json!({"level":"all","mute":mode==1}),
                )
                .await;
                (
                    format!("/api/v1/channels/{}/messages", channel.id),
                    json!({"channel_id":channel.id}),
                )
            } else {
                let conversation: Conversation = server
                    .post(
                        ann,
                        "/api/v1/conversations",
                        &json!({"account_id":ben.account_id}),
                    )
                    .await
                    .ok(StatusCode::OK);
                if mode == 1 {
                    mute(server, ben, ann).await;
                }
                (
                    format!("/api/v1/conversations/{}/messages", conversation.id),
                    json!({"conversation_id":conversation.id}),
                )
            };
            if mode == 2 {
                settings(
                    server,
                    ben,
                    uuid::Uuid::nil(),
                    json!({"do_not_disturb":true}),
                )
                .await;
            }
            let root: Message = server
                .post(ann, &route, &text("root"))
                .await
                .ok(StatusCode::CREATED);
            if in_channel {
                server
                    .put(
                        ben,
                        &format!("/api/v1/messages/{}/thread/follow", root.id),
                        &json!({"following":true}),
                    )
                    .await
                    .ok::<Value>(StatusCode::OK);
            }
            let uploaded =
                crate::messages_support::upload(server, ann, upload_place, b"attachment").await;
            let mut observations = Vec::new();
            for variant in 0..4 {
                if variant == 2 && !in_channel {
                    continue;
                }
                let before = server.snapshot(ann).await.cursor;
                let sent: Message = match variant {
                    0 => server
                        .post(ann, &route, &text("plain"))
                        .await
                        .ok(StatusCode::CREATED),
                    1 => server
                        .put(
                            ann,
                            &format!("/api/v1/messages/{}", root.id),
                            &text("edited"),
                        )
                        .await
                        .ok(StatusCode::OK),
                    2 => server
                        .post(
                            ann,
                            &format!("/api/v1/messages/{}/thread", root.id),
                            &text("thread"),
                        )
                        .await
                        .ok(StatusCode::CREATED),
                    _ => server
                        .post(
                            ann,
                            &route,
                            &json!({"document":{"blocks":[]},"attachments":[uploaded.id]}),
                        )
                        .await
                        .ok(StatusCode::CREATED),
                };
                let after = server.snapshot(ann).await.cursor;
                let mut stream = server.stream(ann).await;
                stream.subscribe(Level::Full, Some(before)).await;
                let positions: Vec<_> = server
                    .traffic(ann, &mut stream)
                    .await
                    .into_iter()
                    .map(|(position, event)| (position - before.position, event.name()))
                    .collect();
                let read: MessageState = server
                    .get(ann, &format!("{route}/read"))
                    .await
                    .ok(StatusCode::OK);
                let history: MessagePage = server.get(ann, &route).await.ok(StatusCode::OK);
                observations.push((
                    after.position - before.position,
                    positions,
                    sent.position,
                    read.unread,
                    read.mentions,
                    history.messages.len(),
                    notices(server, ann).await.len(),
                ));
            }
            modes.push(observations);
        }
        assert_eq!(modes[0], modes[1], "channel={in_channel}: mute");
        assert_eq!(modes[0], modes[2], "channel={in_channel}: quiet");
    }
}
