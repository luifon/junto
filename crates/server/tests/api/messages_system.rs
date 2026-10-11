//! System line projection, recording posts and their mutation boundaries.
use crate::messages_support::*;
use crate::support::TestServer;
use junto_protocol::{audit::AuditEvent, error::ErrorCode, ids::MessageId, message::*};
use reqwest::StatusCode;
use time::Duration;

#[tokio::test]
async fn channel_audit_actions_project_only_kind_actor_and_safe_subject() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let subject = server.join(&owner, "Subject").await;
    let channel = channel(&server, &owner, "voice").await;
    let role = server.everyone_role(&owner, channel.workspace_id).await;
    let actions = [
        AuditEvent::ChannelCreated {
            name: "secret name".into(),
            kind: junto_protocol::structure::ChannelKind::Voice,
        },
        AuditEvent::ChannelUpdated {
            name: "secret name".into(),
            category_id: None,
            position: 0,
            permanent_messages: true,
        },
        AuditEvent::ChannelOverrideSet {
            role_id: role.id,
            allow: vec![],
            deny: vec![junto_protocol::WorkspacePermission::MentionEveryone],
        },
        AuditEvent::ChannelDeleted {
            name: "secret name".into(),
        },
        AuditEvent::ChannelCallSettingsUpdated {
            user_limit: Some(10),
            hides_occupants: false,
        },
        AuditEvent::CallMicrophoneBlocked {
            account_id: subject.account_id,
        },
        AuditEvent::CallMicrophoneUnblocked {
            account_id: subject.account_id,
        },
        AuditEvent::CallSoundBlocked {
            account_id: subject.account_id,
        },
        AuditEvent::CallSoundUnblocked {
            account_id: subject.account_id,
        },
        AuditEvent::CallMemberDisconnected {
            account_id: subject.account_id,
        },
        AuditEvent::CallMemberMovedOut {
            account_id: subject.account_id,
        },
        AuditEvent::CallMemberMovedIn {
            account_id: subject.account_id,
        },
        AuditEvent::PermanentMessageDeleted {
            message_id: MessageId::generate(),
        },
    ];
    let mut transaction = server.db.begin().await.unwrap();
    for action in &actions {
        junto_server::audit::record(
            &mut transaction,
            server.clock.now(),
            Some(owner.account_id),
            junto_server::audit::About::Channel(channel.workspace_id, channel.id),
            action,
        )
        .await
        .unwrap();
    }
    transaction.commit().await.unwrap();
    let page: MessagePage = server
        .get(
            &subject,
            &format!("/api/v1/channels/{}/messages", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    let lines: Vec<_> = page
        .messages
        .iter()
        .skip(1)
        .map(|message| message.system.as_ref().unwrap())
        .collect();
    assert_eq!(lines.len(), actions.len());
    for (line, action) in lines.iter().zip(actions) {
        assert_eq!(line.action, action.action());
        assert_eq!(line.actor_id, Some(owner.account_id));
        assert_eq!(
            line.subject_id,
            line.action
                .starts_with("call_")
                .then_some(subject.account_id)
        );
    }
    let serialized = serde_json::to_string(&lines).unwrap();
    assert!(!serialized.contains("secret name"));
    assert!(!serialized.contains("details"));
    assert!(!serialized.contains("mention_everyone"));
    let state: MessageState = server
        .get(
            &subject,
            &format!("/api/v1/channels/{}/messages/read", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(state.unread, 0);
}

#[tokio::test]
async fn a_recording_post_refuses_removal_while_processing_and_counts_its_delete() {
    let studio = crate::recordings_support::Studio::open().await;
    let server = &studio.scene.server;
    let owner = &studio.scene.owner;
    let recording = studio.record().await;
    server.clock.advance(Duration::seconds(i64::from(
        junto_server::recorder::samples::SECONDS,
    )));
    studio.stop(recording.id).await;
    server.process().await;
    let page: MessagePage = server
        .get(
            owner,
            &format!("/api/v1/channels/{}/messages", studio.scene.voice.id),
        )
        .await
        .ok(StatusCode::OK);
    let post = page
        .messages
        .iter()
        .find(|message| {
            message
                .system
                .as_ref()
                .is_some_and(|line| line.action == "recording_ready")
        })
        .unwrap();
    sqlx::query("update recordings set state='processing',processed_at=null where id=$1")
        .bind(recording.id)
        .execute(&server.db)
        .await
        .unwrap();
    server
        .delete(owner, &message(post.id))
        .await
        .error(ErrorCode::Conflict);
    sqlx::query("update recordings set state='ready',processed_at=now() where id=$1")
        .bind(recording.id)
        .execute(&server.db)
        .await
        .unwrap();
    while server
        .state
        .message_writes
        .attempt_of(owner.account_id, server.clock.now())
        .is_ok()
    {}
    server
        .delete(owner, &message(post.id))
        .await
        .error(ErrorCode::RateLimited);
    server.clock.advance(Duration::minutes(1));
    server
        .delete(owner, &message(post.id))
        .await
        .empty(StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn a_failed_recording_has_one_failure_line_before_its_post() {
    let studio = crate::recordings_support::Studio::open().await;
    let server = &studio.scene.server;
    let track = server
        .fake()
        .track_id(
            &studio.ben_seat.room,
            &studio.ben_seat.identity,
            junto_server::media::TrackSource::Microphone,
        )
        .unwrap();
    server
        .recorder()
        .set_track_of(&track, junto_server::recorder::samples::holed_track());
    let recording = studio.record().await;
    server.clock.advance(Duration::seconds(3));
    studio.stop(recording.id).await;
    server.process().await;
    let page: MessagePage = server
        .get(
            &studio.scene.ann,
            &format!("/api/v1/channels/{}/messages", studio.scene.voice.id),
        )
        .await
        .ok(StatusCode::OK);
    let notes: Vec<_> = page
        .messages
        .iter()
        .filter_map(|message| message.system.as_ref())
        .filter(|line| line.recording_id == Some(recording.id))
        .map(|line| line.action.as_str())
        .collect();
    assert_eq!(
        notes,
        [
            "recording_started",
            "recording_stopped",
            "recording_failed",
            "recording_ready"
        ]
    );
}

#[tokio::test]
async fn every_call_note_becomes_exactly_one_line_in_order() {
    use junto_server::calls::store::{self, By, Note};
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    let seat = server.enter_call(&scene.ann, scene.voice.id).await;
    let route = format!("/api/v1/channels/{}/messages", scene.voice.id);
    let before: MessagePage = server.get(&scene.ann, &route).await.ok(StatusCode::OK);
    let position = before.messages.last().unwrap().position;
    let notes = [
        (Note::CallStarted, "call_started"),
        (Note::ScreenShareStarted, "screen_share_started"),
        (Note::EntryRequested, "entry_requested"),
        (Note::JoinedByInvitation, "joined_by_invitation"),
        (Note::MeetingStarted, "meeting_started"),
        (Note::MeetingEnded, "meeting_ended"),
        (Note::CallEnded, "call_ended"),
    ];
    let mut transaction = server.db.begin().await.unwrap();
    let call = store::call(&mut transaction, By::Id(seat.join.call.id), false)
        .await
        .unwrap()
        .unwrap();
    for (note, _) in notes {
        store::note(
            &mut transaction,
            server.clock.now(),
            &call,
            note,
            Some(scene.ann.account_id),
        )
        .await
        .unwrap();
    }
    transaction.commit().await.unwrap();
    let page: MessagePage = server
        .get(&scene.ann, &format!("{route}?after={position}"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(page.messages.len(), notes.len());
    for (message, (_, action)) in page.messages.iter().zip(notes) {
        let line = message.system.as_ref().unwrap();
        assert_eq!(line.action, action);
        assert_eq!(line.actor_id, Some(scene.ann.account_id));
    }
}

#[tokio::test]
async fn hidden_recording_post_reactions_and_deletion_keep_the_participant_audience() {
    use junto_protocol::events::{Event, Level};
    use serde_json::json;
    let studio = crate::recordings_support::Studio::open().await;
    let scene = &studio.scene;
    let server = &scene.server;
    server
        .grant_workspace(
            &scene.owner,
            &scene.ann,
            scene.workspace_id,
            &[junto_protocol::WorkspacePermission::ManageRecordings],
        )
        .await;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            json!({"hides_occupants":true}),
        )
        .await;
    let recording = studio.record().await;
    server.clock.advance(Duration::seconds(i64::from(
        junto_server::recorder::samples::SECONDS,
    )));
    studio.stop(recording.id).await;
    server.process().await;
    let page: MessagePage = server
        .get(
            &scene.ann,
            &format!("/api/v1/channels/{}/messages", scene.voice.id),
        )
        .await
        .ok(StatusCode::OK);
    let post = page
        .messages
        .iter()
        .find(|message| {
            message
                .system
                .as_ref()
                .is_some_and(|line| line.action == "recording_ready")
        })
        .unwrap();
    let cursor = server.snapshot(&scene.owner).await.cursor;
    let mut outsider = server.subscribed(&scene.owner, Level::Full).await;
    let mut participant = server.subscribed(&scene.ann, Level::Full).await;
    server.state.streams.hold();
    server
        .put(
            &scene.ann,
            &format!("{}/reaction", message(post.id)),
            &json!({"emoji":"👍"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server.leave_call(&scene.ben).await;
    server.leave_call(&scene.ann).await;
    server
        .delete(&scene.ann, &message(post.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server.state.streams.release();
    let post_change = |event: &Event| matches!(event, Event::MessageChanged { message_id, .. } if *message_id==post.id);
    assert!(
        server
            .traffic(&scene.owner, &mut outsider)
            .await
            .iter()
            .all(|(_, event)| !post_change(event))
    );
    let events = server.traffic(&scene.ann, &mut participant).await;
    assert_eq!(
        events
            .iter()
            .filter(|(_, event)| post_change(event))
            .count(),
        2
    );
    for account in [&scene.owner, &scene.ann] {
        let mut replay = server.stream(account).await;
        replay.subscribe(Level::Full, Some(cursor)).await;
        let events = server.traffic(account, &mut replay).await;
        assert_eq!(
            events
                .iter()
                .filter(|(_, event)| post_change(event))
                .count(),
            if account.account_id == scene.ann.account_id {
                2
            } else {
                0
            }
        );
    }
}

#[tokio::test]
async fn a_later_hidden_call_cannot_hide_lines_of_an_ended_public_call() {
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    server.enter_call(&scene.ann, scene.voice.id).await;
    let (line, call): (MessageId, junto_protocol::ids::CallId) = sqlx::query_as(
        "select id,call_id from messages where place_id=$1 and system->>'action'='call_started'",
    )
    .bind(scene.voice.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    server.leave_call(&scene.ann).await;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            serde_json::json!({"hides_occupants":true}),
        )
        .await;
    server.enter_call(&scene.ben, scene.voice.id).await;
    let mut transaction = server.db.begin().await.unwrap();
    junto_server::messages::system(
        &mut transaction,
        server.clock.now(),
        scene.voice.id,
        MessageId::generate(),
        &SystemMessage {
            action: "recording_removed".into(),
            actor_id: Some(scene.ann.account_id),
            subject_id: None,
            recording_id: None,
        },
        Some(call),
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    server
        .get(&scene.owner, &message(line))
        .await
        .ok::<Message>(StatusCode::OK);
}

async fn call_line(scene: &crate::calls_support::Scene, action: &str) -> MessageId {
    let server = &scene.server;
    let call = sqlx::query_scalar("select id from calls where channel_id=$1")
        .bind(scene.voice.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    let id = MessageId::generate();
    let mut transaction = server.db.begin().await.unwrap();
    junto_server::messages::system(
        &mut transaction,
        server.clock.now(),
        scene.voice.id,
        id,
        &SystemMessage {
            action: action.into(),
            actor_id: Some(scene.ann.account_id),
            subject_id: None,
            recording_id: None,
        },
        Some(call),
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    id
}

#[tokio::test]
async fn each_line_keeps_its_audience_when_the_active_call_switches_in_both_directions() {
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    server.enter_call(&scene.ann, scene.voice.id).await;
    let public = call_line(&scene, "meeting_started").await;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            serde_json::json!({"hides_occupants":true}),
        )
        .await;
    let private = call_line(&scene, "meeting_ended").await;
    server.enter_call(&scene.ben, scene.voice.id).await;
    server
        .get(&scene.ben, &message(private))
        .await
        .error(ErrorCode::NotFound);
    let shared = call_line(&scene, "meeting_started").await;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            serde_json::json!({"hides_occupants":false}),
        )
        .await;
    let public_again = call_line(&scene, "meeting_ended").await;
    for id in [public, public_again] {
        server
            .get(&scene.owner, &message(id))
            .await
            .ok::<Message>(StatusCode::OK);
    }
    for id in [private, shared] {
        server
            .get(&scene.owner, &message(id))
            .await
            .error(ErrorCode::NotFound);
        server
            .get(&scene.ann, &message(id))
            .await
            .ok::<Message>(StatusCode::OK);
    }
    server
        .get(&scene.ben, &message(shared))
        .await
        .ok::<Message>(StatusCode::OK);
}

#[tokio::test]
async fn message_id_cursors_page_through_private_lines_sharing_a_public_position() {
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            serde_json::json!({"hides_occupants":true}),
        )
        .await;
    server.enter_call(&scene.ann, scene.voice.id).await;
    for action in ["meeting_started", "meeting_ended", "meeting_started"] {
        call_line(&scene, action).await;
    }
    let route = format!("/api/v1/channels/{}/messages", scene.voice.id);
    let full: MessagePage = server.get(&scene.ann, &route).await.ok(StatusCode::OK);
    assert!(
        full.messages
            .windows(2)
            .any(|lines| lines[0].position == lines[1].position)
    );
    for forward in [false, true] {
        let mut ids = vec![];
        let mut cursor = None;
        loop {
            let query = if let Some(id) = cursor {
                format!(
                    "{route}?limit=1&{}_message={id}",
                    if forward { "after" } else { "before" }
                )
            } else if forward {
                format!("{route}?limit=1&after=0")
            } else {
                format!("{route}?limit=1")
            };
            let page: MessagePage = server.get(&scene.ann, &query).await.ok(StatusCode::OK);
            for line in page.messages {
                cursor = Some(line.id);
                ids.push(line.id);
            }
            if !page.has_more {
                break;
            }
        }
        if !forward {
            ids.reverse();
        }
        assert_eq!(
            ids,
            full.messages.iter().map(|line| line.id).collect::<Vec<_>>()
        );
    }
    let private = full.messages.last().unwrap();
    server
        .get(
            &scene.owner,
            &format!("{route}?before_message={}", private.id),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn an_account_that_left_a_hidden_call_before_its_end_reads_the_call_ended_line() {
    use junto_protocol::{
        error::ErrorCode,
        message::{Message, MessagePage},
    };
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            serde_json::json!({"hides_occupants":true}),
        )
        .await;
    server.enter_call(&scene.ann, scene.voice.id).await;
    server.enter_call(&scene.ben, scene.voice.id).await;
    let call = sqlx::query_scalar("select id from calls where channel_id=$1")
        .bind(scene.voice.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    server.leave_call(&scene.ben).await;
    server.leave_call(&scene.ann).await;
    let page: MessagePage = server
        .get(
            &scene.ann,
            &format!("/api/v1/channels/{}/messages", scene.voice.id),
        )
        .await
        .ok(StatusCode::OK);
    let actions: Vec<_> = page
        .messages
        .iter()
        .filter_map(|line| line.system.as_ref().map(|line| line.action.clone()))
        .collect();
    let ended = page
        .messages
        .iter()
        .find(|line| {
            line.system
                .as_ref()
                .is_some_and(|line| line.action == "call_ended")
        })
        .unwrap_or_else(|| panic!("the last member does not read call_ended: {actions:?}"));
    server
        .get(&scene.owner, &message(ended.id))
        .await
        .error(ErrorCode::NotFound);
    server
        .get(&scene.ben, &message(ended.id))
        .await
        .ok::<Message>(StatusCode::OK);
    let later = MessageId::generate();
    let mut transaction = server.db.begin().await.unwrap();
    junto_server::messages::system(
        &mut transaction,
        server.clock.now(),
        scene.voice.id,
        later,
        &SystemMessage {
            action: "recording_ready".into(),
            actor_id: None,
            subject_id: None,
            recording_id: None,
        },
        Some(call),
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    for participant in [&scene.ann, &scene.ben] {
        server
            .get(participant, &message(later))
            .await
            .ok::<Message>(StatusCode::OK);
    }
    server
        .get(&scene.owner, &message(later))
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn only_participation_while_hidden_grants_later_call_lines() {
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    server.enter_call(&scene.ann, scene.voice.id).await;
    server.enter_call(&scene.ben, scene.voice.id).await;
    server.leave_call(&scene.ben).await;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            serde_json::json!({"hides_occupants":true}),
        )
        .await;
    // Ann was already inside when the setting changed and no new admission follows.
    server.leave_call(&scene.ann).await;
    let page: MessagePage = server
        .get(
            &scene.ann,
            &format!("/api/v1/channels/{}/messages", scene.voice.id),
        )
        .await
        .ok(StatusCode::OK);
    let ended = page
        .messages
        .iter()
        .find(|line| {
            line.system
                .as_ref()
                .is_some_and(|line| line.action == "call_ended")
        })
        .unwrap();
    for outside in [&scene.ben, &scene.owner] {
        server
            .get(outside, &message(ended.id))
            .await
            .error(ErrorCode::NotFound);
    }
}
