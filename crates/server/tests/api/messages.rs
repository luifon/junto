pub use crate::messages_support::text;
use crate::messages_support::{channel, message as msg, send};
use crate::support::{Scratch, TestServer};
use junto_protocol::{
    WorkspacePermission,
    error::ErrorCode,
    events::{Event, Level},
    ids::MessageId,
    message::*,
    structure::{Channel, Workspace},
};
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::Duration;

#[tokio::test]
async fn channel_message_edits_require_the_author_and_do_not_store_earlier_text() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    for kind in ["text", "voice"] {
        let place = channel(&server, &owner, kind).await;
        let sent = send(&server, &author, &place, "original").await;
        assert_eq!(sent.author_name.as_deref(), Some("Writer"));
        assert!(sent.edited_at.is_none());
        server
            .put(&reader, &msg(sent.id), &text("other"))
            .await
            .error(ErrorCode::Forbidden);
        server
            .put(&owner, &msg(sent.id), &text("admin"))
            .await
            .error(ErrorCode::Forbidden);
        server.clock.advance(Duration::seconds(1));
        let edited: Message = server
            .put(&author, &msg(sent.id), &text("corrected"))
            .await
            .ok(StatusCode::OK);
        assert!(edited.edited_at.is_some());
        let stored:bool=sqlx::query_scalar("select exists(select 1 from events where payload::text like '%original%' or payload::text like '%corrected%')").fetch_one(&server.db).await.unwrap();
        assert!(!stored);
    }
}

#[tokio::test]
async fn deleting_channel_messages_preserves_replies_and_refuses_other_authors() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    for kind in ["text", "voice"] {
        let place = channel(&server, &owner, kind).await;
        let sent = send(&server, &author, &place, "original").await;
        let reply: Message = server
            .post(
                &reader,
                &format!("/api/v1/channels/{}/messages", place.id),
                &json!({"document":text("reply")["document"],"reply_to":sent.id}),
            )
            .await
            .ok(StatusCode::CREATED);
        assert_eq!(reply.reply_to, Some(sent.id));
        server
            .delete(&reader, &msg(sent.id))
            .await
            .error(ErrorCode::Forbidden);
        server
            .delete(&author, &msg(sent.id))
            .await
            .empty(StatusCode::NO_CONTENT);
        let marker: Message = server.get(&author, &msg(sent.id)).await.ok(StatusCode::OK);
        assert!(marker.deleted);
        assert!(marker.document.is_none());
        let got: Message = server.get(&reader, &msg(reply.id)).await.ok(StatusCode::OK);
        assert_eq!(got.reply_to, Some(sent.id));
    }
}

#[tokio::test]
async fn channel_reactions_count_accounts_and_refuse_invalid_emoji() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    for kind in ["text", "voice"] {
        let place = channel(&server, &owner, kind).await;
        let sent = send(&server, &author, &place, "original").await;
        let reaction = format!("{}/reaction", msg(sent.id));
        for _ in 0..2 {
            server
                .put(&reader, &reaction, &json!({"emoji":"👍🏽"}))
                .await
                .empty(StatusCode::NO_CONTENT);
        }
        server
            .put(&author, &reaction, &json!({"emoji":"👍🏽"}))
            .await
            .empty(StatusCode::NO_CONTENT);
        let got: Message = server.get(&reader, &msg(sent.id)).await.ok(StatusCode::OK);
        assert_eq!(got.reactions[0].count, 2);
        assert!(got.reactions[0].own);
        for invalid in ["abc", "😀😀", "🦄x", ":custom:"] {
            server
                .put(&reader, &reaction, &json!({"emoji":invalid}))
                .await
                .error(ErrorCode::InvalidRequest);
        }
        server
            .delete(&reader, &format!("{reaction}/👍🏽"))
            .await
            .empty(StatusCode::NO_CONTENT);
    }
}

#[tokio::test]
async fn history_has_stable_both_direction_pages_and_monotonic_reads() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Writer").await;
    let place = channel(&server, &owner, "text").await;
    let mut sent = Vec::new();
    for index in 0..5 {
        sent.push(send(&server, &author, &place, &format!("message {index}")).await);
    }
    let own: MessageState = server
        .get(
            &author,
            &format!("/api/v1/channels/{}/messages/read", place.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(own.unread, 0);
    let history = format!("/api/v1/channels/{}/messages", place.id);
    let page: MessagePage = server
        .get(&owner, &format!("{history}?limit=2"))
        .await
        .ok(StatusCode::OK);
    assert!(page.has_more);
    assert_eq!(
        page.messages.iter().map(|sent| sent.id).collect::<Vec<_>>(),
        vec![sent[3].id, sent[4].id]
    );
    let page: MessagePage = server
        .get(
            &owner,
            &format!("{history}?after={}&limit=2", sent[0].position),
        )
        .await
        .ok(StatusCode::OK);
    assert!(page.has_more);
    assert_eq!(page.messages[0].id, sent[1].id);
    let page: MessagePage = server
        .get(
            &owner,
            &format!("{history}?before={}&limit=2", sent[3].position),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(page.messages[0].id, sent[1].id);
    for q in [
        "limit=0",
        "limit=201",
        "after=1&before=2",
        "after=18446744073709551615",
    ] {
        server
            .get(&owner, &format!("{history}?{q}"))
            .await
            .error(ErrorCode::InvalidRequest);
    }
    let read = format!("{history}/read");
    let state: MessageState = server
        .put(&owner, &read, &json!({"position":sent[1].position}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(state.unread, 3);
    let state: MessageState = server
        .put(&owner, &read, &json!({"position":0}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(state.last_read, sent[1].position);
    server
        .put(&owner, &read, &json!({"position":sent[4].position+1}))
        .await
        .error(ErrorCode::InvalidRequest);
    let snap = server.snapshot(&owner).await;
    assert_eq!(
        snap.messages
            .iter()
            .find(|state| state.place == sent[0].place)
            .unwrap(),
        &state
    );
}
#[tokio::test]
async fn formatting_mentions_and_reply_scope_are_validated() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::ERROR)
        .try_init();
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Writer").await;
    let place = channel(&server, &owner, "text").await;
    let other = channel(&server, &owner, "text").await;
    let parent = send(&server, &author, &other, "parent").await;
    let route = format!("/api/v1/channels/{}/messages", place.id);
    let blocks = json!([
     {"kind":"paragraph","content":[{"kind":"text","text":"bold","marks":["bold","italic","strikethrough"]},{"kind":"code","text":"inline"},{"kind":"link","text":"link","url":"https://example.test/a"},{"kind":"account","account_id":owner.account_id}]},
     {"kind":"quote","content":[{"kind":"text","text":"quoted"}]},
     {"kind":"code","text":"let x = 1;","language":"rust"},
     {"kind":"list","ordered":true,"items":[[{"kind":"text","text":"first"}]]},
     {"kind":"checklist","items":[{"checked":false,"content":[{"kind":"text","text":"task"}]}]}
    ]);
    let sent: Message = server
        .post(&author, &route, &json!({"document":{"blocks":blocks}}))
        .await
        .ok(StatusCode::CREATED);
    let state = server
        .snapshot(&owner)
        .await
        .messages
        .into_iter()
        .find(|state| state.place == sent.place)
        .unwrap();
    assert_eq!(state.mentions, 1);
    server
        .post(
            &author,
            &route,
            &json!({"document":text("reply")["document"],"reply_to":parent.id}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    for bad in [
        json!({"kind":"link","text":"bad","url":"javascript:alert(1)"}),
        json!({"kind":"link","text":"bad","url":"https://user:secret@example.test"}),
        json!({"kind":"text","text":"bad\u{0}"}),
        json!({"kind":"text","text":"bad","marks":["bold","bold"]}),
        json!({"kind":"html","text":"<b>bad</b>"}),
    ] {
        server
            .post(
                &author,
                &route,
                &json!({"document":{"blocks":[{"kind":"paragraph","content":[bad]}]}}),
            )
            .await
            .error(ErrorCode::InvalidRequest);
    }
    server
        .post(&author, &route, &text(""))
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .post(&author, &route, &text(&"x".repeat(17000)))
        .await
        .error(ErrorCode::InvalidRequest);
    let everyone =
        json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"everyone"}]}]}});
    server
        .post(&author, &route, &everyone)
        .await
        .error(ErrorCode::Forbidden);
    server
        .post(&owner, &route, &everyone)
        .await
        .ok::<Message>(StatusCode::CREATED);
    let workspace = server.default_workspace(&owner).await;
    server
        .grant_workspace(
            &owner,
            &author,
            workspace.id,
            &[WorkspacePermission::ManageMessages],
        )
        .await;
    let sent = send(&server, &owner, &place, "moderated").await;
    server
        .delete(&author, &msg(sent.id))
        .await
        .empty(StatusCode::NO_CONTENT);
}
#[tokio::test]
async fn direct_conversations_are_private_and_follow_contact_changes() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "A").await;
    let reader = server.join(&owner, "B").await;
    let conversation: Conversation = server
        .post(
            &author,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let reverse: Conversation = server
        .post(
            &reader,
            "/api/v1/conversations",
            &json!({"account_id":author.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(conversation, reverse);
    let route = format!("/api/v1/conversations/{}/messages", conversation.id);
    let mut stream = server.subscribed(&reader, Level::Background).await;
    let sent: Message = server
        .post(&author, &route, &text("private"))
        .await
        .ok(StatusCode::CREATED);
    assert!(
        matches!(stream.event().await.1,Event::MessageChanged{message_id,..} if message_id==sent.id)
    );
    assert!(
        matches!(stream.event().await.1,Event::NotificationChanged{message_id,..} if message_id==sent.id)
    );
    for who in [&owner, &reader] {
        server.put(who, &msg(sent.id), &text("hijack")).await.error(
            if who.account_id == owner.account_id {
                ErrorCode::NotFound
            } else {
                ErrorCode::Forbidden
            },
        );
    }
    server
        .delete(&reader, &msg(sent.id))
        .await
        .error(ErrorCode::Forbidden);
    let absent = server
        .get(&owner, &msg(MessageId::generate()))
        .await
        .observable();
    assert_eq!(server.get(&owner, &msg(sent.id)).await.observable(), absent);
    let cursor = server.snapshot(&reader).await.cursor;
    server.state.streams.hold();
    server
        .post(&author, &route, &text("queued private"))
        .await
        .ok::<Message>(StatusCode::CREATED);
    let workspace = server.default_workspace(&owner).await;
    sqlx::query("delete from workspace_members where workspace_id=$1 and account_id=$2")
        .bind(workspace.id)
        .bind(reader.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    server.state.streams.release();
    assert!(
        server
            .traffic(&reader, &mut stream)
            .await
            .iter()
            .all(|(_, event)| !matches!(event, Event::MessageChanged { .. }))
    );
    let mut replay = server.stream(&reader).await;
    replay.subscribe(Level::Background, Some(cursor)).await;
    assert!(
        server
            .traffic(&reader, &mut replay)
            .await
            .iter()
            .all(|(_, event)| !matches!(event, Event::MessageChanged { .. }))
    );
    for who in [&author, &reader] {
        server
            .post(who, &route, &text("denied"))
            .await
            .error(ErrorCode::NotFound);
        server
            .put(who, &msg(sent.id), &text("denied"))
            .await
            .error(ErrorCode::NotFound);
        server
            .put(
                who,
                &format!("{}/reaction", msg(sent.id)),
                &json!({"emoji":"👍"}),
            )
            .await
            .error(ErrorCode::NotFound);
        server.get(who, &route).await.error(ErrorCode::NotFound);
        server
            .get(who, &msg(sent.id))
            .await
            .error(ErrorCode::NotFound);
        assert!(
            server
                .snapshot(who)
                .await
                .messages
                .iter()
                .all(|state| state.place != sent.place)
        );
    }
    let outside: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"outside"}))
        .await
        .ok(StatusCode::CREATED);
    let outsider = server.join_into(&owner, "C", &[outside.id]).await;
    server
        .post(
            &author,
            "/api/v1/conversations",
            &json!({"account_id":outsider.account_id}),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .post(
            &owner,
            "/api/v1/conversations",
            &json!({"account_id":outsider.account_id}),
        )
        .await
        .ok::<Conversation>(StatusCode::OK);
}
#[tokio::test]
async fn uploads_resume_check_hash_authorize_download_and_remove_files() {
    let dir = Scratch::new();
    let server = TestServer::start_with(|config| {
        config.attachments_directory = Some(dir.0.clone());
        config.limits.recordings.reserve_bytes = 0;
    })
    .await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let reader = server.join(&owner, "Other").await;
    let place = channel(&server, &owner, "text").await;
    let mut bytes = vec![42; 1024 * 1024];
    bytes.extend(b"last bytes");
    let hash = junto_server::messages::uploads::hash(&bytes);
    let pending: Upload = server
        .post(
            &author,
            "/api/v1/attachments",
            &json!({"channel_id":place.id,"name":"file.bin","bytes":bytes.len(),"sha256":hash}),
        )
        .await
        .ok(StatusCode::CREATED);
    let path = format!("/api/v1/attachments/{}", pending.id);
    server
        .get(&reader, &format!("{path}/upload"))
        .await
        .error(ErrorCode::NotFound);
    server
        .put_bytes(
            &author,
            &format!("{path}/chunks/0"),
            None,
            bytes[..1024 * 1024].to_vec(),
        )
        .await
        .ok::<Upload>(StatusCode::OK);
    let resumed: Upload = server
        .get(&author, &format!("{path}/upload"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(resumed.received, 1024 * 1024);
    server
        .post(&author, &format!("{path}/complete"), &json!({}))
        .await
        .error(ErrorCode::Conflict);
    server
        .put_bytes(
            &author,
            &format!("{path}/chunks/0"),
            None,
            bytes[..1024 * 1024].to_vec(),
        )
        .await
        .ok::<Upload>(StatusCode::OK);
    server
        .put_bytes(
            &author,
            &format!("{path}/chunks/1048576"),
            None,
            bytes[1024 * 1024..].to_vec(),
        )
        .await
        .ok::<Upload>(StatusCode::OK);
    let done: Upload = server
        .post(&author, &format!("{path}/complete"), &json!({}))
        .await
        .ok(StatusCode::OK);
    assert!(done.complete);
    server
        .get(&author, &format!("{path}/file"))
        .await
        .error(ErrorCode::NotFound);
    let sent: Message = server
        .post(
            &author,
            &format!("/api/v1/channels/{}/messages", place.id),
            &json!({"document":{"blocks":[]},"attachments":[pending.id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(sent.attachments[0].sha256, hash);
    let file = server.get(&reader, &format!("{path}/file")).await;
    assert_eq!(file.status, StatusCode::OK);
    assert_eq!(file.bytes(), bytes);
    assert_eq!(file.header("x-content-type-options"), Some("nosniff"));
    assert_eq!(file.header("content-disposition"), Some("attachment"));
    assert_eq!(file.header("cache-control"), Some("no-store"));
    assert_eq!(file.header("etag"), Some(format!("\"{hash}\"").as_str()));
    assert_eq!(
        file.header("content-type"),
        Some("application/octet-stream")
    );
    server
        .delete(&author, &msg(sent.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&reader, &format!("{path}/file"))
        .await
        .error(ErrorCode::NotFound);
    server.clock.advance(Duration::minutes(2));
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    assert_eq!(server.count("attachments").await, 0);
    assert!(std::fs::read_dir(&dir.0).unwrap().next().is_none());
}
#[tokio::test]
async fn system_lines_are_immutable_and_recording_posts_use_recording_permissions() {
    let studio = crate::recordings_support::Studio::open().await;
    let scene = &studio.scene;
    let server = &scene.server;
    let owner = &scene.owner;
    let author = &scene.ann;
    let rec = studio.record().await;
    server.clock.advance(Duration::seconds(i64::from(
        junto_server::recorder::samples::SECONDS,
    )));
    studio.stop(rec.id).await;
    server.process().await;
    let route = format!("/api/v1/channels/{}/messages", scene.voice.id);
    let page: MessagePage = server.get(author, &route).await.ok(StatusCode::OK);
    let actions: Vec<_> = page
        .messages
        .iter()
        .filter_map(|message| message.system.as_ref().map(|line| line.action.as_str()))
        .filter(|action| {
            matches!(
                *action,
                "call_started" | "recording_started" | "recording_stopped" | "recording_ready"
            )
        })
        .collect();
    assert_eq!(
        actions,
        [
            "call_started",
            "recording_started",
            "recording_stopped",
            "recording_ready"
        ]
    );
    let post = page
        .messages
        .iter()
        .find(|sent| {
            sent.system
                .as_ref()
                .is_some_and(|line| line.action == "recording_ready")
        })
        .unwrap();
    assert_eq!(post.system.as_ref().unwrap().recording_id, Some(rec.id));
    server
        .put(owner, &msg(post.id), &text("change"))
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(author, &msg(post.id))
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(owner, &msg(post.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(owner, &format!("/api/v1/recordings/{}", rec.id))
        .await
        .error(ErrorCode::NotFound);
    server
        .get(owner, &msg(post.id))
        .await
        .error(ErrorCode::NotFound);
    let line = page
        .messages
        .iter()
        .find(|sent| {
            sent.system
                .as_ref()
                .is_some_and(|line| line.action == "call_started")
        })
        .unwrap();
    server
        .delete(owner, &msg(line.id))
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn hidden_channels_refuse_every_message_operation_and_queued_events() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let place = channel(&server, &owner, "text").await;
    let mut stream = server.subscribed(&author, Level::Full).await;
    server.state.streams.hold();
    let sent = send(&server, &owner, &place, "hidden text").await;
    let everyone = server.everyone_role(&owner, workspace.id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", place.id, everyone.id),
            &json!({"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server.state.streams.release();
    let got = server.traffic(&author, &mut stream).await;
    assert!(
        got.iter()
            .all(|(_, event)| !matches!(event, Event::MessageChanged { .. }))
    );
    assert!(!stream.raw.join("").contains(&sent.id.to_string()));
    let route = format!("/api/v1/channels/{}/messages", place.id);
    for reply in [
        server.get(&author, &route).await,
        server.post(&author, &route, &text("no")).await,
        server.get(&author, &msg(sent.id)).await,
        server.put(&author, &msg(sent.id), &text("no")).await,
        server.delete(&author, &msg(sent.id)).await,
        server
            .put(
                &author,
                &format!("{}/reaction", msg(sent.id)),
                &json!({"emoji":"👍"}),
            )
            .await,
        server
            .put(
                &author,
                &format!("{route}/read"),
                &json!({"position":sent.position}),
            )
            .await,
    ] {
        reply.error(ErrorCode::NotFound);
    }
    assert_eq!(
        server.get(&author, &msg(sent.id)).await.observable(),
        server
            .get(&author, &msg(MessageId::generate()))
            .await
            .observable()
    );
}

#[tokio::test]
async fn mentions_use_current_text_roles_and_permissions_at_delivery_and_replay() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Mentioned").await;
    let reader = server.join(&owner, "Other").await;
    let place = channel(&server, &owner, "text").await;
    let mut to_a = server.subscribed(&author, Level::Background).await;
    let mut to_b = server.subscribed(&reader, Level::Background).await;
    let cursor = server.snapshot(&author).await.cursor;
    let route = format!("/api/v1/channels/{}/messages", place.id);
    let mention = json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"account","account_id":author.account_id}]}]}});
    let sent: Message = server
        .post(&owner, &route, &mention)
        .await
        .ok(StatusCode::CREATED);
    assert!(
        matches!(to_a.event().await.1,Event::MessageMentioned{message_id,..} if message_id==sent.id)
    );
    assert!(server.traffic(&reader, &mut to_b).await.is_empty());
    server
        .put(&owner, &msg(sent.id), &text("mention removed"))
        .await
        .ok::<Message>(StatusCode::OK);
    let mut replay = server.stream(&author).await;
    replay.subscribe(Level::Background, Some(cursor)).await;
    assert!(
        server
            .traffic(&author, &mut replay)
            .await
            .iter()
            .all(|(_, event)| !matches!(event, Event::MessageMentioned { .. }))
    );
    let workspace = server.default_workspace(&owner).await;
    let role = server
        .grant_workspace(
            &owner,
            &author,
            workspace.id,
            &[WorkspacePermission::ManageMessages],
        )
        .await;
    let role_mention = json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"role","role_id":role}]}]}});
    server.traffic(&author, &mut to_a).await;
    let positive: Message = server
        .post(&owner, &route, &role_mention)
        .await
        .ok(StatusCode::CREATED);
    assert!(
        matches!(server.traffic(&author,&mut to_a).await.as_slice(),[(_,Event::MessageMentioned{message_id,..}),(_,Event::NotificationChanged{message_id:notice})] if *message_id==positive.id && *notice==positive.id)
    );
    server.state.streams.hold();
    let sent: Message = server
        .post(&owner, &route, &role_mention)
        .await
        .ok(StatusCode::CREATED);
    server
        .delete(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{}",
                workspace.id, author.account_id, role
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server.state.streams.release();
    assert!(server.traffic(&author, &mut to_a).await.iter().all(
        |(_, event)| !matches!(event,Event::MessageMentioned{message_id,..} if *message_id==sent.id)
    ));
}

#[tokio::test]
async fn same_place_positions_commit_in_order_and_retry_has_one_message_and_event() {
    use crate::support::drive_until;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let place = channel(&server, &owner, "text").await;
    let route = format!("/api/v1/channels/{}/messages", place.id);
    server.gate().arm(0);
    let one_body = text("one");
    let one = server.post(&owner, &route, &one_body);
    tokio::pin!(one);
    drive_until(&mut one, server.gate().arrived()).await;
    let two_body = text("two");
    let two = server.post(&owner, &route, &two_body);
    tokio::pin!(two);
    drive_until(&mut two, server.until_blocked()).await;
    server.gate().release();
    let first: Message = one.await.ok(StatusCode::CREATED);
    let second: Message = two.await.ok(StatusCode::CREATED);
    assert!(second.position > first.position);
    server.gate().lose_next(1);
    let third = send(&server, &owner, &place, "retry").await;
    let count: i64 = sqlx::query_scalar("select count(*) from messages where id=$1")
        .bind(third.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    let count: i64 = sqlx::query_scalar(
        "select count(*) from events where kind='message_changed' and payload->>'message_id'=$1",
    )
    .bind(third.id.to_string())
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(count, 1);
    let before = server.count("messages").await;
    server.gate().lose_next(8);
    server
        .post(&owner, &route, &text("rollback"))
        .await
        .error(ErrorCode::Busy);
    assert_eq!(server.count("messages").await, before);
}

#[tokio::test]
async fn uploads_enforce_limits_ownership_integrity_expiry_and_preview_checks() {
    let dir = Scratch::new();
    let server = TestServer::start_with(|config| {
        config.attachments_directory = Some(dir.0.clone());
        config.limits.recordings.reserve_bytes = 0;
        config.limits.attachment_bytes = 4096;
        config.limits.attachments_per_message = 1;
    })
    .await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let reader = server.join(&owner, "Other").await;
    let place = channel(&server, &owner, "text").await;
    let other = channel(&server, &owner, "text").await;
    let png = junto_server::image::samples::png(64, 48);
    let hash = junto_server::messages::uploads::hash(&png);
    let body = json!({"channel_id":place.id,"name":"photo.png","bytes":png.len(),"sha256":hash});
    for (field, value) in [
        ("name", json!("../escape")),
        ("name", json!("bad\nname")),
        ("bytes", json!(0)),
        ("bytes", json!(4097)),
        ("sha256", json!("bad")),
    ] {
        let mut bad = body.clone();
        bad[field] = value;
        server
            .post(&author, "/api/v1/attachments", &bad)
            .await
            .error(ErrorCode::InvalidRequest);
    }
    let pending: Upload = server
        .post(&author, "/api/v1/attachments", &body)
        .await
        .ok(StatusCode::CREATED);
    let path = format!("/api/v1/attachments/{}", pending.id);
    server
        .put_bytes(&reader, &format!("{path}/chunks/0"), None, png.clone())
        .await
        .error(ErrorCode::NotFound);
    server
        .put_bytes(&author, &format!("{path}/chunks/1"), None, png.clone())
        .await
        .error(ErrorCode::Conflict);
    server
        .put_bytes(
            &author,
            &format!("{path}/chunks/0"),
            Some("text/html"),
            png.clone(),
        )
        .await
        .ok::<Upload>(StatusCode::OK);
    server
        .put_bytes(
            &author,
            &format!("{path}/chunks/0"),
            None,
            vec![1; png.len()],
        )
        .await
        .error(ErrorCode::Conflict);
    server
        .post(&author, &format!("{path}/complete"), &json!({}))
        .await
        .ok::<Upload>(StatusCode::OK);
    let second =
        crate::messages_support::upload(&server, &author, json!({"channel_id":place.id}), &png)
            .await;
    server
        .post(
            &author,
            &format!("/api/v1/channels/{}/messages", place.id),
            &json!({"document":{"blocks":[]},"attachments":[pending.id,second.id]}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    let message = json!({"document":{"blocks":[]},"attachments":[pending.id]});
    server
        .post(
            &reader,
            &format!("/api/v1/channels/{}/messages", place.id),
            &message,
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .post(
            &author,
            &format!("/api/v1/channels/{}/messages", other.id),
            &message,
        )
        .await
        .error(ErrorCode::InvalidRequest);
    let sent: Message = server
        .post(
            &author,
            &format!("/api/v1/channels/{}/messages", place.id),
            &message,
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(
        (sent.attachments[0].width, sent.attachments[0].height),
        (Some(64), Some(48))
    );
    assert_eq!(sent.attachments[0].media_type, "image/png");
    server
        .post(
            &author,
            &format!("/api/v1/channels/{}/messages", place.id),
            &message,
        )
        .await
        .error(ErrorCode::NotFound);
    let mut wrong = body.clone();
    wrong["sha256"] = json!("00".repeat(32));
    let bad: Upload = server
        .post(&author, "/api/v1/attachments", &wrong)
        .await
        .ok(StatusCode::CREATED);
    let badpath = format!("/api/v1/attachments/{}", bad.id);
    let low_chunk = server
        .strict(|config| config.limits.recordings.reserve_bytes = u64::MAX)
        .await;
    low_chunk
        .put_bytes(&author, &format!("{badpath}/chunks/0"), None, png.clone())
        .await
        .error(ErrorCode::StorageLow);
    server
        .put_bytes(&author, &format!("{badpath}/chunks/0"), None, png)
        .await
        .ok::<Upload>(StatusCode::OK);
    server
        .post(&author, &format!("{badpath}/complete"), &json!({}))
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .delete(&reader, &format!("{badpath}/upload"))
        .await
        .error(ErrorCode::NotFound);
    server.clock.advance(Duration::hours(24));
    server
        .get(&author, &format!("{badpath}/upload"))
        .await
        .error(ErrorCode::NotFound);
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    assert!(!dir.0.join(bad.id.to_string()).exists());
    let low = server
        .strict(|config| config.limits.recordings.reserve_bytes = u64::MAX)
        .await;
    low.post(&author, "/api/v1/attachments", &body)
        .await
        .error(ErrorCode::StorageLow);
    let workspace = server.default_workspace(&owner).await;
    let everyone = server.everyone_role(&owner, workspace.id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", place.id, everyone.id),
            &json!({"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .get(&author, &format!("{path}/file"))
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn hidden_call_notes_stay_private_after_the_call_ends() {
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            json!({"hides_occupants":true}),
        )
        .await;
    let before: MessageState = server
        .get(
            &scene.ben,
            &format!("/api/v1/channels/{}/messages/read", scene.voice.id),
        )
        .await
        .ok(StatusCode::OK);
    let mut outsider = server.subscribed(&scene.ben, Level::Full).await;
    server.enter_call(&scene.ann, scene.voice.id).await;
    let route = format!("/api/v1/channels/{}/messages", scene.voice.id);
    let inside: MessagePage = server.get(&scene.ann, &route).await.ok(StatusCode::OK);
    let started = inside
        .messages
        .iter()
        .find(|sent| {
            sent.system
                .as_ref()
                .is_some_and(|line| line.action == "call_started")
        })
        .unwrap();
    let outside: MessagePage = server.get(&scene.ben, &route).await.ok(StatusCode::OK);
    assert!(
        outside
            .messages
            .iter()
            .all(|message| message.id != started.id)
    );
    let state: MessageState = server
        .get(&scene.ben, &format!("{route}/read"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(state.latest, before.latest);
    assert_eq!(state.unread, 0);
    assert!(
        server
            .snapshot(&scene.ben)
            .await
            .messages
            .iter()
            .find(|state| state.place == started.place)
            .unwrap()
            .latest
            == before.latest
    );
    server
        .get(&scene.ben, &msg(started.id))
        .await
        .error(ErrorCode::NotFound);
    server
        .get(&scene.owner, &msg(started.id))
        .await
        .error(ErrorCode::NotFound);
    let traffic = server.traffic(&scene.ben, &mut outsider).await;
    assert!(traffic.iter().all(
        |(_, event)| !matches!(event,Event::MessageChanged{message_id,..} if *message_id==started.id)
    ));
    server.leave_call(&scene.ann).await;
    server
        .get(&scene.ben, &msg(started.id))
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn permanent_messages_keep_their_flag_count_and_deletion_audit() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Writer").await;
    let place = channel(&server, &owner, "text").await;
    let route = format!("/api/v1/channels/{}", place.id);
    server
        .patch(&owner, &route, &json!({"permanent_messages":true}))
        .await
        .ok::<Channel>(StatusCode::OK);
    let sent = send(&server, &author, &place, "permanent").await;
    assert!(sent.permanent);
    server
        .patch(&owner, &route, &json!({"permanent_messages":false}))
        .await
        .ok::<Channel>(StatusCode::OK);
    for who in [&author, &owner] {
        server
            .put(who, &msg(sent.id), &text("changed"))
            .await
            .error(ErrorCode::Forbidden);
    }
    server
        .delete(&author, &msg(sent.id))
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&owner, &msg(sent.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let count: i64 = sqlx::query_scalar("select permanent_message_count from channels where id=$1")
        .bind(place.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let audit: i64 = sqlx::query_scalar(
        "select count(*) from audit_entries where action='permanent_message_deleted'",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(audit, 1);
    let page: MessagePage = server
        .get(&author, &format!("{route}/messages"))
        .await
        .ok(StatusCode::OK);
    assert!(page.messages.iter().any(|sent| {
        sent.system
            .as_ref()
            .is_some_and(|line| line.action == "permanent_message_deleted")
    }));
}

#[tokio::test]
async fn administrators_change_attachment_limits_and_every_device_is_told() {
    let dir = Scratch::new();
    let server = TestServer::start_with(|config| {
        config.attachments_directory = Some(dir.0.clone());
        config.limits.recordings.reserve_bytes = 0;
    })
    .await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Member").await;
    let place = channel(&server, &owner, "text").await;
    let mut stream = server.subscribed(&author, Level::Background).await;
    let limits = json!({"bytes_per_file":12,"files_per_message":1});
    server
        .put(&author, "/api/v1/attachment-limits", &limits)
        .await
        .error(ErrorCode::Forbidden);
    server
        .put(&owner, "/api/v1/attachment-limits", &limits)
        .await
        .ok::<AttachmentLimits>(StatusCode::OK);
    assert!(
        matches!(stream.event().await.1,Event::AttachmentLimitsChanged{limits} if limits.bytes_per_file==12 && limits.files_per_message==1)
    );
    server
        .post(
            &author,
            "/api/v1/attachments",
            &json!({"channel_id":place.id,"name":"large","bytes":13,"sha256":"00".repeat(32)}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    for invalid in [
        json!({"bytes_per_file":0,"files_per_message":1}),
        json!({"bytes_per_file":12,"files_per_message":101}),
    ] {
        server
            .put(&owner, "/api/v1/attachment-limits", &invalid)
            .await
            .error(ErrorCode::InvalidRequest);
    }
    let reopened = server.strict(|_| {}).await;
    let got: AttachmentLimits = reopened
        .get(&author, "/api/v1/attachment-limits")
        .await
        .ok(StatusCode::OK);
    assert_eq!(got.bytes_per_file, 12);
}

#[tokio::test]
async fn database_refuses_cross_place_replies_and_invalid_positions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let one = channel(&server, &owner, "text").await;
    let two = channel(&server, &owner, "text").await;
    let first = send(&server, &owner, &one, "one").await;
    let second = send(&server, &owner, &two, "two").await;
    assert!(
        sqlx::query("update messages set reply_to=$2 where id=$1")
            .bind(first.id)
            .bind(second.id)
            .execute(&server.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("update messages set position=0 where id=$1")
            .bind(first.id)
            .execute(&server.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("update message_places set head=-1 where id=$1")
            .bind(one.id)
            .execute(&server.db)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn message_write_rate_is_bounded_and_recovers_with_the_clock() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let place = channel(&server, &owner, "text").await;
    let sent = send(&server, &owner, &place, "text").await;
    for _ in 0..119 {
        server
            .put(&owner, &msg(sent.id), &text("text"))
            .await
            .ok::<Message>(StatusCode::OK);
    }
    server
        .put(&owner, &msg(sent.id), &text("changed"))
        .await
        .error(ErrorCode::RateLimited);
    server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", place.id),
            &text("limited"),
        )
        .await
        .error(ErrorCode::RateLimited);
    server
        .delete(&owner, &msg(sent.id))
        .await
        .error(ErrorCode::RateLimited);
    server
        .put(
            &owner,
            &format!("{}/reaction", msg(sent.id)),
            &json!({"emoji":"👍"}),
        )
        .await
        .error(ErrorCode::RateLimited);
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/messages/read", place.id),
            &json!({"position":sent.position}),
        )
        .await
        .error(ErrorCode::RateLimited);
    let other = server.join(&owner, "Other").await;
    server
        .post(
            &owner,
            "/api/v1/conversations",
            &json!({"account_id":other.account_id}),
        )
        .await
        .error(ErrorCode::RateLimited);
    server.clock.advance(Duration::minutes(1));
    server
        .put(&owner, &msg(sent.id), &text("changed"))
        .await
        .ok::<Message>(StatusCode::OK);
}
