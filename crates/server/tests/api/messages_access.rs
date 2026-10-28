//! Refusals for private conversations, hidden calls and accounts that lost access.
use crate::messages_support::*;
use crate::support::{Scratch, TestServer};
use junto_protocol::{
    error::ErrorCode,
    events::{Event, Level},
    message::*,
};
use reqwest::StatusCode;
use serde_json::json;

#[tokio::test]
async fn every_direct_route_hides_the_conversation_from_nonparticipants_including_administrators() {
    let directory = Scratch::new();
    let server = TestServer::start_with(|config| {
        config.attachments_directory = Some(directory.0.clone());
        config.limits.recordings.reserve_bytes = 0;
    })
    .await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let reader = server.join(&owner, "Reader").await;
    let outsider = server.join(&owner, "Outsider").await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(
            &owner,
            &administrator,
            &[junto_protocol::ServerPermission::Administrator],
        )
        .await;
    let conversation: Conversation = server
        .post(
            &author,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let base = format!("/api/v1/conversations/{}", conversation.id);
    let attachment = upload(
        &server,
        &author,
        json!({"conversation_id":conversation.id}),
        b"private file",
    )
    .await;
    let sent: Message = server
        .post(
            &author,
            &format!("{base}/messages"),
            &json!({"document":text("secret")["document"],"attachments":[attachment.id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    for denied in [&owner, &administrator, &outsider] {
        for route in [
            format!("{base}/messages"),
            format!("{base}/messages/read"),
            message(sent.id),
            format!("/api/v1/attachments/{}/file", attachment.id),
        ] {
            server.get(denied, &route).await.error(ErrorCode::NotFound);
        }
        server
            .post(denied, &format!("{base}/messages"), &text("hijack"))
            .await
            .error(ErrorCode::NotFound);
        server
            .put(
                denied,
                &format!("{base}/messages/read"),
                &json!({"position":sent.position}),
            )
            .await
            .error(ErrorCode::NotFound);
        server
            .put(
                denied,
                &format!("{}/reaction", message(sent.id)),
                &json!({"emoji":"👍"}),
            )
            .await
            .error(ErrorCode::NotFound);
        server
            .delete(denied, &format!("{}/reaction/👍", message(sent.id)))
            .await
            .error(ErrorCode::NotFound);
        server
            .delete(denied, &message(sent.id))
            .await
            .error(ErrorCode::NotFound);
        server.post(denied,"/api/v1/attachments",&json!({"conversation_id":conversation.id,"name":"file","bytes":1,"sha256":"00".repeat(32)})).await.error(ErrorCode::NotFound);
        let listed: Vec<Conversation> = server
            .get(denied, "/api/v1/conversations")
            .await
            .ok(StatusCode::OK);
        assert!(listed.is_empty());
    }
}

#[tokio::test]
async fn direct_writes_and_events_stop_with_contact_and_history_returns_with_contact() {
    let directory = Scratch::new();
    let server = TestServer::start_with(|config| {
        config.attachments_directory = Some(directory.0.clone());
        config.limits.recordings.reserve_bytes = 0;
    })
    .await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let reader = server.join(&owner, "Reader").await;
    let cursor = server.snapshot(&reader).await.cursor;
    let mut reader_live = server.subscribed(&reader, Level::Background).await;
    let mut outsider_live = server.subscribed(&owner, Level::Full).await;
    server.state.streams.hold();
    let conversation: Conversation = server
        .post(
            &author,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let base = format!("/api/v1/conversations/{}", conversation.id);
    let attachment = upload(
        &server,
        &author,
        json!({"conversation_id":conversation.id}),
        b"private file",
    )
    .await;
    let pending: Upload = server.post(&author, "/api/v1/attachments", &json!({"conversation_id":conversation.id,"name":"pending","bytes":1,"sha256":"00".repeat(32)})).await.ok(StatusCode::CREATED);
    let sent: Message = server
        .post(
            &author,
            &format!("{base}/messages"),
            &json!({"document":text("private")["document"],"attachments":[attachment.id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put(
            &reader,
            &format!("{base}/messages/read"),
            &json!({"position":sent.position}),
        )
        .await
        .ok::<MessageState>(StatusCode::OK);
    let workspace = server.default_workspace(&owner).await;
    sqlx::query("delete from workspace_members where workspace_id=$1 and account_id=$2")
        .bind(workspace.id)
        .bind(reader.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    server.state.streams.release();
    for denied in [&author, &reader] {
        server
            .get(
                denied,
                &format!("/api/v1/attachments/{}/file", attachment.id),
            )
            .await
            .error(ErrorCode::NotFound);
        server.post(denied, "/api/v1/attachments", &json!({"conversation_id":conversation.id,"name":"file","bytes":1,"sha256":"00".repeat(32)})).await.error(ErrorCode::NotFound);
        server
            .delete(denied, &format!("{}/reaction/👍", message(sent.id)))
            .await
            .error(ErrorCode::NotFound);
        server
            .post(denied, &format!("{base}/messages"), &text("lost contact"))
            .await
            .error(ErrorCode::NotFound);
        server
            .put(denied, &message(sent.id), &text("edit"))
            .await
            .error(ErrorCode::NotFound);
        server
            .delete(denied, &message(sent.id))
            .await
            .error(ErrorCode::NotFound);
        server
            .put(
                denied,
                &format!("{}/reaction", message(sent.id)),
                &json!({"emoji":"👍"}),
            )
            .await
            .error(ErrorCode::NotFound);
        server
            .put(
                denied,
                &format!("{base}/messages/read"),
                &json!({"position":sent.position}),
            )
            .await
            .error(ErrorCode::NotFound);
        server
            .get(denied, &format!("{base}/messages"))
            .await
            .error(ErrorCode::NotFound);
    }
    server
        .get(
            &author,
            &format!("/api/v1/attachments/{}/upload", pending.id),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .put_bytes(
            &author,
            &format!("/api/v1/attachments/{}/chunks/0", pending.id),
            None,
            vec![0],
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .post(
            &author,
            &format!("/api/v1/attachments/{}/complete", pending.id),
            &json!({}),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(
            &author,
            &format!("/api/v1/attachments/{}/upload", pending.id),
        )
        .await
        .error(ErrorCode::NotFound);
    let private = |event: &Event| {
        matches!(
            event,
            Event::MessageChanged { .. }
                | Event::ConversationOpened { .. }
                | Event::MessageRead { .. }
        )
    };
    assert!(
        server
            .traffic(&reader, &mut reader_live)
            .await
            .iter()
            .all(|(_, event)| !private(event))
    );
    assert!(
        server
            .traffic(&owner, &mut outsider_live)
            .await
            .iter()
            .all(|(_, event)| !private(event))
    );
    for account in [&reader, &owner] {
        let mut replay = server.stream(account).await;
        replay.subscribe(Level::Full, Some(cursor)).await;
        assert!(
            server
                .traffic(account, &mut replay)
                .await
                .iter()
                .all(|(_, event)| !private(event))
        );
    }
    sqlx::query(
        "insert into workspace_members(workspace_id,account_id,joined_at) values($1,$2,now())",
    )
    .bind(workspace.id)
    .bind(reader.account_id)
    .execute(&server.db)
    .await
    .unwrap();
    let page: MessagePage = server
        .get(&reader, &format!("{base}/messages"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(page.messages[0].id, sent.id);
    let listed: Vec<Conversation> = server
        .get(&reader, "/api/v1/conversations")
        .await
        .ok(StatusCode::OK);
    assert_eq!(listed, [conversation]);
}

#[tokio::test]
async fn hidden_call_lines_are_absent_from_pages_state_snapshot_and_replay_forever() {
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            json!({"hides_occupants":true}),
        )
        .await;
    let base = format!("/api/v1/channels/{}", scene.voice.id);
    let before: MessageState = server
        .get(&scene.ben, &format!("{base}/messages/read"))
        .await
        .ok(StatusCode::OK);
    let cursor = server.snapshot(&scene.ben).await.cursor;
    server.enter_call(&scene.ann, scene.voice.id).await;
    let inside: MessagePage = server
        .get(&scene.ann, &format!("{base}/messages"))
        .await
        .ok(StatusCode::OK);
    let started = inside
        .messages
        .iter()
        .find(|message| {
            message
                .system
                .as_ref()
                .is_some_and(|line| line.action == "call_started")
        })
        .unwrap();
    for ended in [false, true] {
        if ended {
            server.leave_call(&scene.ann).await;
        }
        for outsider in [&scene.ben, &scene.owner] {
            let page: MessagePage = server
                .get(outsider, &format!("{base}/messages"))
                .await
                .ok(StatusCode::OK);
            assert!(page.messages.iter().all(|message| message.id != started.id));
            let state: MessageState = server
                .get(outsider, &format!("{base}/messages/read"))
                .await
                .ok(StatusCode::OK);
            assert_eq!(state.latest, before.latest);
            assert_eq!(state.unread, 0);
            let snapshot = server.snapshot(outsider).await;
            assert_eq!(
                snapshot
                    .messages
                    .iter()
                    .find(|state| state.place == before.place)
                    .unwrap()
                    .latest,
                before.latest
            );
            server
                .put(
                    outsider,
                    &format!("{base}/messages/read"),
                    &json!({"position":before.latest+1}),
                )
                .await
                .error(ErrorCode::InvalidRequest);
            let mut replay = server.stream(outsider).await;
            replay.subscribe(Level::Full, Some(cursor)).await;
            assert!(server.traffic(outsider,&mut replay).await.iter().all(|(_,event)|!matches!(event,Event::MessageChanged{message_id,..} if *message_id==started.id)));
        }
        server
            .get(&scene.ann, &message(started.id))
            .await
            .ok::<Message>(StatusCode::OK);
    }
}

#[tokio::test]
async fn removed_and_banned_accounts_cannot_use_their_messages_or_uploads() {
    let directory = Scratch::new();
    let server = TestServer::start_with(|config| {
        config.attachments_directory = Some(directory.0.clone());
        config.limits.recordings.reserve_bytes = 0;
    })
    .await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    for removal in ["workspace", "removed", "banned"] {
        let author = server.join(&owner, "Author").await;
        let attachment = upload(
            &server,
            &author,
            json!({"channel_id":channel.id}),
            b"private",
        )
        .await;
        let pending: Upload = server
            .post(
                &author,
                "/api/v1/attachments",
                &json!({
                    "channel_id":channel.id,"name":"pending","bytes":7,
                    "sha256":junto_server::messages::uploads::hash(b"pending"),
                }),
            )
            .await
            .ok(StatusCode::CREATED);
        let sent: Message = server
            .post(
                &author,
                &format!("/api/v1/channels/{}/messages", channel.id),
                &json!({"document":text("own")["document"],"attachments":[attachment.id]}),
            )
            .await
            .ok(StatusCode::CREATED);
        let refused = if removal == "workspace" {
            ErrorCode::NotFound
        } else {
            ErrorCode::Unauthenticated
        };
        if removal == "workspace" {
            sqlx::query("delete from workspace_members where workspace_id=$1 and account_id=$2")
                .bind(channel.workspace_id)
                .bind(author.account_id)
                .execute(&server.db)
                .await
                .unwrap();
            let snapshot = server.snapshot(&author).await;
            assert!(
                !snapshot
                    .workspaces
                    .iter()
                    .any(|workspace| workspace.workspace.id == channel.workspace_id)
            );
        } else if removal == "banned" {
            server
                .put_empty(
                    &owner,
                    &format!("/api/v1/accounts/{}/ban", author.account_id),
                )
                .await
                .empty(StatusCode::NO_CONTENT);
        } else {
            server
                .post(
                    &owner,
                    &format!("/api/v1/accounts/{}/removal", author.account_id),
                    &json!({}),
                )
                .await
                .empty(StatusCode::NO_CONTENT);
        }
        for route in [
            message(sent.id),
            format!("/api/v1/attachments/{}/upload", pending.id),
            format!("/api/v1/attachments/{}/file", attachment.id),
        ] {
            server.get(&author, &route).await.error(refused);
        }
        server
            .put(&author, &message(sent.id), &text("edit"))
            .await
            .error(refused);
        server
            .delete(&author, &message(sent.id))
            .await
            .error(refused);
        server
            .put_bytes(
                &author,
                &format!("/api/v1/attachments/{}/chunks/0", pending.id),
                None,
                b"pending".to_vec(),
            )
            .await
            .error(refused);
        server
            .post(
                &author,
                &format!("/api/v1/attachments/{}/complete", pending.id),
                &json!({}),
            )
            .await
            .error(refused);
        server
            .delete(
                &author,
                &format!("/api/v1/attachments/{}/upload", pending.id),
            )
            .await
            .error(refused);
    }
}

#[tokio::test]
async fn a_hidden_channel_refuses_upload_start_reaction_removal_and_cross_place_replies() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let hidden = channel(&server, &owner, "text").await;
    let public = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &hidden, "hidden").await;
    let everyone = server.everyone_role(&owner, hidden.workspace_id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/rules/roles/{}", hidden.id, everyone.id),
            &json!({"deny":["view_channel"]}),
        )
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
    server
        .post(
            &member,
            "/api/v1/attachments",
            &json!({"channel_id":hidden.id,"name":"file","bytes":1,"sha256":"00".repeat(32)}),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .delete(&member, &format!("{}/reaction/👍", message(root.id)))
        .await
        .error(ErrorCode::NotFound);
    server
        .post(
            &member,
            &format!("/api/v1/channels/{}/messages", public.id),
            &json!({"document":text("reply")["document"],"reply_to":root.id}),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn conversation_open_is_idempotent_lists_participants_and_refuses_self() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let other = server.join(&owner, "Other").await;
    server
        .post(
            &owner,
            "/api/v1/conversations",
            &json!({"account_id":owner.account_id}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    let mut stream = server.subscribed(&other, Level::Background).await;
    let first: Conversation = server
        .post(
            &owner,
            "/api/v1/conversations",
            &json!({"account_id":other.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    assert!(
        matches!(stream.event().await.1,Event::ConversationOpened{conversation} if conversation==first)
    );
    server
        .post(
            &owner,
            "/api/v1/conversations",
            &json!({"account_id":other.account_id}),
        )
        .await
        .ok::<Conversation>(StatusCode::OK);
    assert!(server.traffic(&other, &mut stream).await.is_empty());
    let listed: Vec<Conversation> = server
        .get(&other, "/api/v1/conversations")
        .await
        .ok(StatusCode::OK);
    assert_eq!(listed, [first]);
}

#[tokio::test]
async fn public_lines_stay_public_when_the_active_call_changes_visibility() {
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    server.enter_call(&scene.ann, scene.voice.id).await;
    let page: MessagePage = server
        .get(
            &scene.ann,
            &format!("/api/v1/channels/{}/messages", scene.voice.id),
        )
        .await
        .ok(StatusCode::OK);
    let started = page
        .messages
        .iter()
        .find(|message| {
            message
                .system
                .as_ref()
                .is_some_and(|line| line.action == "call_started")
        })
        .unwrap();
    server
        .get(&scene.owner, &message(started.id))
        .await
        .ok::<Message>(StatusCode::OK);
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            json!({"hides_occupants":true}),
        )
        .await;
    server
        .get(&scene.owner, &message(started.id))
        .await
        .ok::<Message>(StatusCode::OK);
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            json!({"hides_occupants":false}),
        )
        .await;
    server.leave_call(&scene.ann).await;
    server
        .get(&scene.owner, &message(started.id))
        .await
        .ok::<Message>(StatusCode::OK);
    server
        .get(&scene.ann, &message(started.id))
        .await
        .ok::<Message>(StatusCode::OK);
}

#[tokio::test]
async fn hidden_lines_do_not_consume_public_positions_or_expose_reply_targets() {
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            json!({"hides_occupants":true}),
        )
        .await;
    let before = send(server, &scene.ann, &scene.voice, "before").await;
    server.enter_call(&scene.ann, scene.voice.id).await;
    let page: MessagePage = server
        .get(
            &scene.ann,
            &format!("/api/v1/channels/{}/messages", scene.voice.id),
        )
        .await
        .ok(StatusCode::OK);
    let hidden = page
        .messages
        .iter()
        .find(|line| {
            line.system
                .as_ref()
                .is_some_and(|line| line.action == "call_started")
        })
        .unwrap();
    let reply: Message = server
        .post(
            &scene.ann,
            &format!("/api/v1/channels/{}/messages", scene.voice.id),
            &json!({"document":text("reply")["document"],"reply_to":hidden.id}),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(
        reply.position,
        before.position + 1,
        "hidden lines must not consume public positions"
    );
    let outside: Message = server
        .get(&scene.owner, &message(reply.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        outside.reply_to, None,
        "an invisible target must not expose its identifier"
    );
    assert_eq!(reply.reply_to, Some(hidden.id));
}

#[tokio::test]
async fn a_late_joiner_cannot_read_hidden_lines_written_before_joining() {
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            json!({"hides_occupants":true}),
        )
        .await;
    server.enter_call(&scene.ann, scene.voice.id).await;
    let page: MessagePage = server
        .get(
            &scene.ann,
            &format!("/api/v1/channels/{}/messages", scene.voice.id),
        )
        .await
        .ok(StatusCode::OK);
    let hidden = page
        .messages
        .iter()
        .find(|line| {
            line.system
                .as_ref()
                .is_some_and(|line| line.action == "call_started")
        })
        .unwrap();
    server.enter_call(&scene.ben, scene.voice.id).await;
    server
        .get(&scene.ben, &message(hidden.id))
        .await
        .error(ErrorCode::NotFound);
    server
        .get(&scene.ann, &message(hidden.id))
        .await
        .ok::<Message>(StatusCode::OK);
}

/// A channel that does not exist and one that the caller may not see answer with the same
/// statements, so that the time of the answer does not tell them apart (open item CD5, ADR
/// 0304).
#[tokio::test]
async fn an_absent_and_a_hidden_channel_run_the_same_statements() {
    use crate::messages_queries::{measure_requests, since, statements};
    let server = crate::support::TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let ann = server.join(&owner, "Ann").await;
    let hidden = crate::messages_support::channel(&server, &owner, "text").await;
    let everyone = server.everyone_role(&owner, hidden.workspace_id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/rules/roles/{}", hidden.id, everyone.id),
            &serde_json::json!({ "deny": ["view_channel"] }),
        )
        .await
        .ok::<serde_json::Value>(reqwest::StatusCode::OK);
    let absent = junto_protocol::ids::ChannelId::generate();
    let (mut held, role) = measure_requests(&server).await;
    let observer = &mut **held.first_mut().unwrap();
    let mut cost = Vec::new();
    for channel_id in [hidden.id, absent, hidden.id, absent] {
        let before = statements(observer, role).await;
        server
            .get(&ann, &format!("/api/v1/channels/{channel_id}/messages"))
            .await
            .error(junto_protocol::error::ErrorCode::NotFound);
        cost.push(since(statements(observer, role).await, before));
    }
    // The first request of each prepares its statements; the second ones are compared.
    assert_eq!(cost[2], cost[3]);
    server.require_no_statement_eviction(evictions).await;
}
