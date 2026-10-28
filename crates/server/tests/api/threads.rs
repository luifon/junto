use crate::discovery_support::*;

#[tokio::test]
async fn rule_01_thread_replies_are_paged_separately_from_channel_history() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    let first = reply(&server, &owner, root.id, "first").await;
    let second = reply(&server, &owner, root.id, "second").await;
    let history: MessagePage = server
        .get(&owner, &format!("/api/v1/channels/{}/messages", channel.id))
        .await
        .ok(StatusCode::OK);
    assert!(history.messages.iter().any(|m| m.id == root.id));
    assert!(
        !history
            .messages
            .iter()
            .any(|m| m.id == first.id || m.id == second.id)
    );
    let page: ThreadPage = server
        .get(&owner, &format!("{}/thread?limit=1", message(root.id)))
        .await
        .ok(StatusCode::OK);
    assert_eq!(page.replies.messages[0].id, second.id);
    assert!(page.replies.has_more);
    let page: ThreadPage = server
        .get(
            &owner,
            &format!(
                "{}/thread?before_message={}&limit=1",
                message(root.id),
                second.id
            ),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(page.replies.messages[0].id, first.id);
    assert!(!page.replies.has_more);
    let page: ThreadPage = server
        .get(
            &owner,
            &format!("{}/thread?after={}", message(root.id), first.position),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(page.replies.messages.len(), 1);
    assert_eq!(page.replies.messages[0].id, second.id);
}

#[tokio::test]
async fn rule_02_root_summary_and_event_follow_reply_deletion() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    let first = reply(&server, &owner, root.id, "first").await;
    let second = reply(&server, &owner, root.id, "second").await;
    let summary = thread(&server, &owner, root.id).await.root;
    assert_eq!(summary.reply_count, 2);
    assert_eq!(summary.last_reply, Some(second.id));
    let cursor = server.snapshot(&owner).await.cursor;
    server
        .delete(&owner, &message(second.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let summary = thread(&server, &owner, root.id).await.root;
    assert_eq!(summary.reply_count, 1);
    assert_eq!(summary.last_reply, Some(first.id));
    assert!(
        events(&server, &owner, cursor, Level::Full)
            .await
            .iter()
            .any(
                |event| matches!(event,Event::MessageChanged{message_id,..} if *message_id==root.id)
            )
    );
}

#[tokio::test]
async fn rule_03_deleted_root_keeps_permanent_replies() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &author, &channel, "root").await;
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({"permanent_messages":true}),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    let child = reply(&server, &author, root.id, "permanent reply").await;
    server
        .delete(&author, &message(root.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let page = thread(&server, &author, root.id).await;
    assert!(page.root.deleted);
    assert_eq!(page.root.document, None);
    assert_eq!(page.replies.messages[0].id, child.id);
    assert!(page.replies.messages[0].permanent);
    server
        .put(&owner, &message(child.id), &text("change"))
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn rule_04_nested_foreign_system_and_also_send_are_refused() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    let child = reply(&server, &owner, root.id, "reply").await;
    server
        .post(
            &owner,
            &format!("{}/thread", message(child.id)),
            &text("nested"),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    let other = send(&server, &owner, &channel, "other root").await;
    let mut body = text("wrong thread");
    body["reply_to"] = json!(other.id);
    server
        .post(&owner, &format!("{}/thread", message(root.id)), &body)
        .await
        .error(ErrorCode::InvalidRequest);
    let mut body = text("also send");
    body["also_send_to_channel"] = json!(true);
    server
        .post(&owner, &format!("{}/thread", message(root.id)), &body)
        .await
        .error(ErrorCode::InvalidRequest);
    let page: MessagePage = server
        .get(&owner, &format!("/api/v1/channels/{}/messages", channel.id))
        .await
        .ok(StatusCode::OK);
    let system = page.messages.iter().find(|m| m.system.is_some()).unwrap();
    server
        .post(
            &owner,
            &format!("{}/thread", message(system.id)),
            &text("system reply"),
        )
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn rule_05_thread_content_obeys_authorship_and_reactions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    let child = reply(&server, &author, root.id, "reply").await;
    server
        .put(&owner, &message(child.id), &text("other edit"))
        .await
        .error(ErrorCode::Forbidden);
    server
        .put(&author, &message(child.id), &text("edited"))
        .await
        .ok::<Message>(StatusCode::OK);
    server
        .put(
            &owner,
            &format!("{}/reaction", message(child.id)),
            &json!({"emoji":"👍"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let page = thread(&server, &owner, root.id).await;
    assert!(page.replies.messages[0].edited_at.is_some());
    assert_eq!(page.replies.messages[0].reactions[0].count, 1);
}

#[tokio::test]
async fn rule_06_07_fork_copies_text_and_voice_with_category_permissions_and_marker() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let category = server
        .category(&owner, channel.workspace_id, "category", 0)
        .await;
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({"category_id":category.id}),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    let everyone = server.everyone_role(&owner, channel.workspace_id).await;
    server
        .put(
            &owner,
            &format!(
                "/api/v1/channels/{}/rules/roles/{}",
                channel.id, everyone.id
            ),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    for kind in ["text", "voice"] {
        let root = send(&server, &owner, &channel, "root").await;
        reply(&server, &owner, root.id, "reply").await;
        let copied = fork(&server, &owner, root.id, kind).await;
        assert_eq!(copied.category_id, Some(category.id));
        assert_eq!(serde_json::to_value(copied.kind).unwrap(), kind);
        let history: MessagePage = server
            .get(&owner, &format!("/api/v1/channels/{}/messages", copied.id))
            .await
            .ok(StatusCode::OK);
        assert_eq!(
            history
                .messages
                .iter()
                .filter(|m| m.document.is_some())
                .count(),
            2
        );
        server
            .get(&reader, &format!("/api/v1/channels/{}/messages", copied.id))
            .await
            .error(ErrorCode::NotFound);
        assert!(
            thread(&server, &owner, root.id)
                .await
                .forks
                .contains(&copied.id)
        );
    }
}

#[tokio::test]
async fn rule_08_fork_requires_both_channel_and_workspace_management() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    server
        .post(
            &reader,
            &format!("{}/fork", message(root.id)),
            &json!({"name":"fork","kind":"text"}),
        )
        .await
        .error(ErrorCode::Forbidden);
    let everyone = server.everyone_role(&owner, channel.workspace_id).await;
    server
        .put(
            &owner,
            &format!(
                "/api/v1/channels/{}/rules/roles/{}",
                channel.id, everyone.id
            ),
            &json!({"allow":["manage_channel"],"deny":[]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .post(
            &reader,
            &format!("{}/fork", message(root.id)),
            &json!({"name":"fork","kind":"text"}),
        )
        .await
        .error(ErrorCode::Forbidden);
    let role = server
        .grant_workspace(
            &owner,
            &reader,
            channel.workspace_id,
            &[junto_protocol::WorkspacePermission::ManageChannel],
        )
        .await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/rules/roles/{}", channel.id, role),
            &json!({"allow":[],"deny":["manage_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let before = server.snapshot(&reader).await.cursor;
    server
        .post(
            &reader,
            &format!("{}/fork", message(root.id)),
            &json!({"name":"denied","kind":"text"}),
        )
        .await
        .error(ErrorCode::Forbidden);
    assert!(
        !events(&server, &reader, before, Level::Full)
            .await
            .iter()
            .any(|e| matches!(e, Event::ChannelCreated { .. }))
    );
    assert!(thread(&server, &owner, root.id).await.forks.is_empty());
}

#[tokio::test]
async fn rule_09_11_fork_copies_are_independent_without_reactions_or_notices() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = mention(&server, &owner, &channel, &reader).await;
    server
        .put(
            &reader,
            &format!("{}/reaction", message(root.id)),
            &json!({"emoji":"👍"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let before = notices(&server, &reader).await.len();
    let copied = fork(&server, &owner, root.id, "text").await;
    let state: MessageState = server
        .get(
            &reader,
            &format!("/api/v1/channels/{}/messages/read", copied.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(state.unread, 0);
    assert!(!state.unread_sign);
    let page: MessagePage = server
        .get(&owner, &format!("/api/v1/channels/{}/messages", copied.id))
        .await
        .ok(StatusCode::OK);
    let copy = page.messages.iter().find(|m| m.document.is_some()).unwrap();
    assert_ne!(copy.id, root.id);
    assert_eq!(copy.created_at, root.created_at);
    assert_eq!(copy.author_id, root.author_id);
    assert!(copy.reactions.is_empty());
    assert_eq!(notices(&server, &reader).await.len(), before);
    server
        .put(&owner, &message(copy.id), &text("copy change"))
        .await
        .ok::<Message>(StatusCode::OK);
    let original: Message = server
        .get(&owner, &message(root.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(original.document, root.document);
    server
        .delete(&owner, &message(root.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(
        server
            .get(&owner, &message(copy.id))
            .await
            .ok::<Message>(StatusCode::OK)
            .document
            .is_some()
    );
}

#[tokio::test]
async fn rule_10_fork_files_survive_either_copy_being_deleted() {
    let files = Files::open().await;
    let upload = files.upload(b"immutable bytes").await;
    let root = files.attach(&[upload.id]).await;
    let copied = fork(&files.server, &files.owner, root.id, "text").await;
    let page: MessagePage = files
        .server
        .get(
            &files.owner,
            &format!("/api/v1/channels/{}/messages", copied.id),
        )
        .await
        .ok(StatusCode::OK);
    let copy = page
        .messages
        .iter()
        .find(|m| !m.attachments.is_empty())
        .unwrap();
    let attachment = copy.attachments[0].id;
    assert_ne!(attachment, upload.id);
    files
        .server
        .delete(&files.owner, &message(root.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    files.server.clock.advance(time::Duration::minutes(2));
    junto_server::maintenance::run_once(&files.server.state)
        .await
        .unwrap();
    assert_eq!(
        files
            .server
            .get(
                &files.owner,
                &format!("/api/v1/attachments/{attachment}/file")
            )
            .await
            .bytes(),
        b"immutable bytes"
    );
    files
        .server
        .get(
            &files.owner,
            &format!("/api/v1/attachments/{}/file", upload.id),
        )
        .await
        .error(ErrorCode::NotFound);
    files
        .server
        .delete(&files.owner, &message(copy.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    junto_server::maintenance::run_once(&files.server.state)
        .await
        .unwrap();
    assert!(!files.directory.0.join(upload.id.to_string()).exists());
}

#[tokio::test]
async fn rule_12_fork_refuses_invalid_names_and_kinds_without_changes() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    for body in [
        json!({"name":"","kind":"text"}),
        json!({"name":"fork","kind":"invalid"}),
    ] {
        server
            .post(&owner, &format!("{}/fork", message(root.id)), &body)
            .await
            .error(ErrorCode::InvalidRequest);
    }
    assert!(thread(&server, &owner, root.id).await.forks.is_empty());
}

#[tokio::test]
async fn a_channel_with_a_pinned_reply_and_a_fork_is_deleted() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    let child = reply(&server, &owner, root.id, "reply").await;
    for id in [root.id, child.id] {
        server
            .put_empty(&owner, &format!("{}/pin", message(id)))
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let copied = fork(&server, &owner, root.id, "text").await;

    server
        .delete(&owner, &format!("/api/v1/channels/{}", channel.id))
        .await
        .empty(StatusCode::NO_CONTENT);

    server
        .get(&owner, &message(root.id))
        .await
        .error(ErrorCode::NotFound);
    let history: MessagePage = server
        .get(&owner, &format!("/api/v1/channels/{}/messages", copied.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        history
            .messages
            .iter()
            .filter(|message| message.document.is_some())
            .count(),
        2
    );
}

#[tokio::test]
async fn a_fork_gets_a_position_of_its_own() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;

    let copied = fork(&server, &owner, root.id, "text").await;

    assert_ne!(copied.position, channel.position);
}

#[tokio::test]
async fn the_copies_of_a_fork_are_not_unread() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    reply(&server, &owner, root.id, "reply").await;

    let copied = fork(&server, &owner, root.id, "text").await;

    let state: MessageState = server
        .get(
            &reader,
            &format!("/api/v1/channels/{}/messages/read", copied.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(state.unread, 0);
    assert!(!state.unread_sign);
}

#[tokio::test]
async fn fix_a_thread_has_one_fork_even_with_concurrent_requests() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    let before = server.count("channels").await;
    let route = format!("{}/fork", message(root.id));
    let body = json!({"name":"fork","kind":"text"});
    let (one, two) = tokio::join!(
        server.post(&owner, &route, &body),
        server.post(&owner, &route, &body)
    );
    let mut statuses = [one.status, two.status];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::CREATED, StatusCode::CONFLICT]);
    assert_eq!(thread(&server, &owner, root.id).await.forks.len(), 1);
    assert_eq!(server.count("channels").await, before + 1);
    server
        .post(&owner, &route, &body)
        .await
        .error(ErrorCode::Conflict);
}

#[tokio::test]
async fn fix_fork_copies_more_than_one_thousand_replies_and_reply_references() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    sqlx::query("insert into messages(id,place_id,position,author_id,created_at,document,reply_to,thread_root) select
             gen_random_uuid(),place_id,position+n,author_id,created_at,document,id,id from messages cross join
             generate_series(1,1001) n where id=$1")
        .bind(root.id)
        .execute(&mut server.test_data().await)
        .await
        .unwrap();
    sqlx::query("update messages set reply_to=(select id from messages where thread_root=$1 order by position limit 1) where id=(select id from messages where thread_root=$1 order by position desc limit 1)").bind(root.id).execute(&server.db).await.unwrap();
    let copied = fork(&server, &owner, root.id, "text").await;
    let page: MessagePage = server
        .get(
            &owner,
            &format!("/api/v1/channels/{}/messages?after=0&limit=200", copied.id),
        )
        .await
        .ok(StatusCode::OK);
    let copy = page
        .messages
        .iter()
        .find(|m| m.reply_to.is_none() && m.document.is_some())
        .unwrap();
    assert!(
        page.messages
            .iter()
            .filter(|m| m.id != copy.id)
            .all(|m| m.reply_to == Some(copy.id))
    );
    let count: i64 = sqlx::query_scalar(
        "select count(*) from messages where place_id=$1 and author_id is not null",
    )
    .bind(copied.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(count, 1002);
    let latest: MessagePage = server
        .get(
            &owner,
            &format!("/api/v1/channels/{}/messages?limit=2", copied.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        latest
            .messages
            .iter()
            .find(|m| m.document.is_some())
            .unwrap()
            .reply_to,
        Some(page.messages.iter().find(|m| m.position == 2).unwrap().id)
    );
}

#[tokio::test]
async fn fix_the_first_edit_of_a_fork_does_not_repeat_unchanged_mentions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = mention(&server, &owner, &channel, &reader).await;
    let copied = fork(&server, &owner, root.id, "text").await;
    let page: MessagePage = server
        .get(&owner, &format!("/api/v1/channels/{}/messages", copied.id))
        .await
        .ok(StatusCode::OK);
    let copy = page.messages.iter().find(|m| m.document.is_some()).unwrap();
    let mut body = json!({"document":copy.document});
    body["document"]["blocks"][0]["content"][0]["text"] = json!("changed quartz");
    let cursor = server.snapshot(&reader).await.cursor;
    server
        .put(&owner, &message(copy.id), &body)
        .await
        .ok::<Message>(StatusCode::OK);
    assert_eq!(notices(&server, &reader).await.len(), 1);
    assert!(
        !events(&server, &reader, cursor, Level::Background)
            .await
            .iter()
            .any(|e| matches!(
                e,
                Event::NotificationChanged { .. } | Event::MessageMentioned { .. }
            ))
    );
}

#[tokio::test]
async fn fix_thread_mentions_and_read_positions_are_independent_of_the_channel() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let stranger = server.join(&owner, "Stranger").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    let read_route = format!("/api/v1/channels/{}/messages/read", channel.id);
    server
        .put(&reader, &read_route, &json!({"position":root.position}))
        .await
        .ok::<MessageState>(StatusCode::OK);
    let body = json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"text","text":"reply"},{"kind":"account","account_id":reader.account_id}]}]}});
    let child: Message = server
        .post(&owner, &format!("{}/thread", message(root.id)), &body)
        .await
        .ok(StatusCode::CREATED);
    let state: MessageState = server.get(&reader, &read_route).await.ok(StatusCode::OK);
    assert_eq!(state.unread, 0);
    assert_eq!(state.mentions, 1);
    let route = format!("{}/thread/read", message(root.id));
    let state: Value = server.get(&reader, &route).await.ok(StatusCode::OK);
    assert_eq!(state["unread"], 1);
    server
        .get(&stranger, &route)
        .await
        .error(ErrorCode::NotFound);
    server
        .put(&stranger, &route, &json!({"position":child.position}))
        .await
        .error(ErrorCode::NotFound);
    let cursor = server.snapshot(&reader).await.cursor;
    server
        .put(&reader, &route, &json!({"position":child.position}))
        .await
        .ok::<Value>(StatusCode::OK);
    let state: MessageState = server.get(&reader, &read_route).await.ok(StatusCode::OK);
    assert_eq!(state.mentions, 0);
    assert!(notices(&server, &reader).await[0].read_at.is_some());
    let state: ThreadState = server.get(&reader, &route).await.ok(StatusCode::OK);
    assert_eq!(state.last_read, child.position);
    assert!(
        !server
            .snapshot(&reader)
            .await
            .threads
            .iter()
            .any(|t| t.root_id == root.id)
    );
    assert!(events(&server,&reader,cursor,Level::Background).await.iter().any(|e|matches!(e,Event::ThreadRead{root_id,position} if *root_id==root.id && *position==child.position)));
    assert!(
        !events(&server, &owner, cursor, Level::Full)
            .await
            .iter()
            .any(|e| matches!(e, Event::ThreadRead { .. }))
    );
}

#[tokio::test]
async fn fix_root_summary_is_stored_and_updated_without_counting_all_replies() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    reply(&server, &owner, root.id, "child").await;
    assert_eq!(thread(&server, &owner, root.id).await.root.reply_count, 1);
    let stored: bool = sqlx::query_scalar("select exists(select 1 from information_schema.columns where table_name='messages' and column_name='reply_count')")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(
        stored,
        "root count must be updated with replies instead of rescanned on reads"
    );
}

#[tokio::test]
async fn fix_a_fork_rechecks_source_access_after_an_overlapping_override() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let channel = channel(&server, &owner, "text").await;
    let role = server
        .grant_workspace(
            &owner,
            &manager,
            channel.workspace_id,
            &[junto_protocol::WorkspacePermission::ManageChannel],
        )
        .await;
    let root = send(&server, &owner, &channel, "root").await;
    let route = format!("{}/fork", message(root.id));
    let body = json!({"name":"fork","kind":"text"});
    server.gate().arm(0);
    let fork = server.post(&manager, &route, &body);
    tokio::pin!(fork);
    tokio::select! {biased; ()=server.gate().arrived()=>{} result=&mut fork=>panic!("fork escaped the gate: {:?}",result.status)}
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/rules/roles/{}", channel.id, role),
            &json!({"allow":[],"deny":["manage_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server.gate().release();
    fork.await.error(ErrorCode::Forbidden);
    assert!(thread(&server, &owner, root.id).await.forks.is_empty());
}

#[tokio::test]
async fn fix_fork_copies_more_than_one_thousand_attachments_in_batches() {
    let files = Files::open().await;
    let upload = files.upload(b"shared content").await;
    let root = files.attach(&[upload.id]).await;
    sqlx::query("with copies as (insert into
             attachments(id,storage_id,place_id,owner_id,message_id,name,bytes,received,sha256,complete,media_type,expires_at,touched_at,attached,staging_cleaned,position)
             select
             gen_random_uuid(),storage_id,place_id,owner_id,message_id,name,bytes,received,sha256,complete,media_type,expires_at,touched_at,attached,staging_cleaned,n
             from attachments cross join generate_series(1,1000) n where id=$1 returning id) insert into
             attachment_chunks(attachment_id,offset_bytes,bytes,sha256,stored) select
             copies.id,c.offset_bytes,c.bytes,c.sha256,c.stored from copies cross join attachment_chunks c where
             c.attachment_id=$1")
        .bind(upload.id)
        .execute(&mut files.server.test_data().await)
        .await
        .unwrap();
    let copied = fork(&files.server, &files.owner, root.id, "text").await;
    let page: MessagePage = files
        .server
        .get(
            &files.owner,
            &format!("/api/v1/channels/{}/messages", copied.id),
        )
        .await
        .ok(StatusCode::OK);
    let copy = page.messages.iter().find(|m| m.document.is_some()).unwrap();
    assert_eq!(copy.attachments.len(), 1001);
    let file = copy.attachments.last().unwrap();
    assert_eq!(
        files
            .server
            .get(
                &files.owner,
                &format!("/api/v1/attachments/{}/file", file.id)
            )
            .await
            .bytes(),
        b"shared content"
    );
}

#[tokio::test]
async fn fix_direct_participants_cannot_open_threads_or_pins() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let conversation: Conversation = server
        .post(
            &owner,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let sent: Message = server
        .post(
            &owner,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
            &text("direct"),
        )
        .await
        .ok(StatusCode::CREATED);
    for participant in [&owner, &reader] {
        server
            .get(participant, &format!("{}/thread", message(sent.id)))
            .await
            .error(ErrorCode::InvalidRequest);
        server
            .post(
                participant,
                &format!("{}/thread", message(sent.id)),
                &text("reply"),
            )
            .await
            .error(ErrorCode::InvalidRequest);
        server
            .post(
                participant,
                &format!("{}/fork", message(sent.id)),
                &json!({"name":"fork","kind":"text"}),
            )
            .await
            .error(ErrorCode::InvalidRequest);
        server
            .put_empty(participant, &format!("{}/pin", message(sent.id)))
            .await
            .error(ErrorCode::NotFound);
    }
}

#[tokio::test]
async fn round2_hidden_voice_forks_into_text_and_voice() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let source = channel(&server, &owner, "voice").await;
    server
        .call_settings(&owner, source.id, json!({"hides_occupants":true}))
        .await;
    for kind in ["text", "voice"] {
        let root = send(&server, &owner, &source, "root").await;
        let copy = fork(&server, &owner, root.id, kind).await;
        assert_eq!(copy.hides_occupants, kind == "voice");
    }
}

#[tokio::test]
async fn round2_deleted_fork_can_be_created_again() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let source = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &source, "root").await;
    let copy = fork(&server, &owner, root.id, "text").await;
    server
        .delete(&owner, &format!("/api/v1/channels/{}", copy.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let again = fork(&server, &owner, root.id, "text").await;
    assert_ne!(copy.id, again.id);
    assert_eq!(thread(&server, &owner, root.id).await.forks, vec![again.id]);
}

#[tokio::test]
async fn round2_group_mentions_do_not_follow_but_can_be_read() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let source = channel(&server, &owner, "text").await;
    for node in [
        json!({"kind":"everyone"}),
        json!({"kind":"role","role_id":server.everyone_role(&owner, source.workspace_id).await.id}),
    ] {
        let root = send(&server, &owner, &source, "root").await;
        let child: Message = server
            .post(
                &owner,
                &format!("{}/thread", message(root.id)),
                &json!({"document":{"blocks":[{"kind":"paragraph","content":[node.clone()]}]}}),
            )
            .await
            .ok(StatusCode::CREATED);
        let rows: i64 = sqlx::query_scalar(
            "select count(*) from thread_reads where account_id=$1 and root_id=$2",
        )
        .bind(reader.account_id)
        .bind(root.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
        assert_eq!(rows, 0, "group mentions must not create follower/read rows");
        let snapshot = server.snapshot(&reader).await;
        let state = snapshot
            .threads
            .iter()
            .find(|s| s.root_id == root.id)
            .unwrap();
        assert_eq!(state.unread, 0);
        assert_eq!(state.mentions, 1);
        server
            .put(
                &reader,
                &format!("{}/thread/read", message(root.id)),
                &json!({"position":child.position}),
            )
            .await
            .ok::<Value>(StatusCode::OK);
        assert!(
            !server
                .snapshot(&reader)
                .await
                .threads
                .iter()
                .any(|s| s.root_id == root.id)
        );
        reply(&server, &owner, root.id, "unmentioned").await;
        assert!(
            !server
                .snapshot(&reader)
                .await
                .threads
                .iter()
                .any(|s| s.root_id == root.id)
        );
        assert_eq!(
            notices(&server, &reader)
                .await
                .iter()
                .filter(|n| n.message_id != child.id)
                .count(),
            if node["kind"] == "role" { 1 } else { 0 }
        );
    }
}

#[tokio::test]
async fn round2_unread_threads_are_bounded_and_paged() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let source = channel(&server, &owner, "text").await;
    let root = send(&server, &reader, &source, "root").await;
    sqlx::query("insert into messages(id,place_id,position,author_id,created_at,document) select gen_random_uuid(),place_id,position+n,author_id,created_at,document from messages cross join generate_series(1,204) n where id=$1").bind(root.id).execute(&mut server.test_data().await).await.unwrap();
    sqlx::query("insert into messages(id,place_id,position,author_id,created_at,document,thread_root) select gen_random_uuid(),place_id,position+1000,$2,created_at,document,id from messages where place_id=$1 and author_id=$3").bind(source.id).bind(owner.account_id).bind(reader.account_id).execute(&server.db).await.unwrap();
    sqlx::query("with roots as (insert into messages(id,place_id,position,author_id,created_at,document)
        select gen_random_uuid(),place_id,position+2000+n,$2,created_at,document from messages
        cross join generate_series(1,5) n where id=$1 returning *),
        replies as (insert into messages(id,place_id,position,author_id,created_at,document,thread_root)
        select gen_random_uuid(),place_id,position+1000,author_id,created_at,document,id from roots returning *)
        insert into message_mention_accounts(place_id,account_id,position,message_id,thread_root)
        select place_id,$3,position,id,thread_root from replies")
        .bind(root.id).bind(owner.account_id).bind(reader.account_id).execute(&mut server.test_data().await).await.unwrap();
    let snapshot: Value = server
        .get(&reader, "/api/v1/snapshot")
        .await
        .ok(StatusCode::OK);
    assert_eq!(snapshot["threads"].as_array().unwrap().len(), 200);
    assert_eq!(snapshot["threads_more"], true);
    let page: Value = server
        .get(&reader, "/api/v1/threads/unread")
        .await
        .ok(StatusCode::OK);
    assert_eq!(page["threads"], snapshot["threads"]);
    assert_eq!(page["more"], true);
    assert_eq!(page["next"], snapshot["threads_next"]);
    let next = page["next"].as_str().unwrap();
    let rest: Value = server
        .get(&reader, &format!("/api/v1/threads/unread?before={next}"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(rest["threads"].as_array().unwrap().len(), 10);
    assert!(rest["next"].is_null());
    let entries: Vec<_> = page["threads"]
        .as_array()
        .unwrap()
        .iter()
        .chain(rest["threads"].as_array().unwrap())
        .collect();
    assert_eq!(
        entries
            .iter()
            .filter(|e| e["unread"] == 0 && e["mentions"] == 1)
            .count(),
        5
    );
    let ordered: Vec<MessageId> = sqlx::query_scalar("select root_id from (select root_id,latest_id as newest from thread_reads where account_id=$1 and following and position<latest union all select thread_root,message_id from message_mention_accounts where account_id=$1 and thread_head) entries order by newest desc,root_id desc")
        .bind(reader.account_id).fetch_all(&server.db).await.unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|e| e["root_id"].as_str().unwrap().parse::<MessageId>().unwrap())
            .collect::<Vec<_>>(),
        ordered
    );
    let mut ids = std::collections::HashSet::new();
    for entry in page["threads"]
        .as_array()
        .unwrap()
        .iter()
        .chain(rest["threads"].as_array().unwrap())
    {
        assert!(ids.insert(entry["root_id"].clone().to_string()));
    }
    server
        .get(&reader, "/api/v1/threads/unread?before=bad")
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .get(&reader, "/api/v1/threads/unread?limit=201")
        .await
        .error(ErrorCode::InvalidRequest);
    let mut connection = server.db.acquire().await.unwrap();
    sqlx::query("set enable_seqscan=off")
        .execute(&mut *connection)
        .await
        .unwrap();
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "explain(format json) {}",
        junto_server::messages::threads::UNREAD_QUERY
    )))
    .bind(reader.account_id)
    .bind(vec![source.id.as_uuid()])
    .bind(uuid::Uuid::max())
    .bind(uuid::Uuid::max())
    .bind(201_i64)
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(
        plan.to_string().contains("thread_reads_unread"),
        "unread followers need indexed reads: {plan}"
    );
    assert!(
        plan.to_string().contains("message_mention_inbox"),
        "unread non-followers need indexed reads: {plan}"
    );
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "explain(format json) {}",
        junto_server::messages::threads::STATES_QUERY
    )))
    .bind(reader.account_id)
    .bind(&ordered)
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(
        plan.to_string().contains("message_mention_thread_root"),
        "mention-only states need a root index: {plan}"
    );
}

#[tokio::test]
async fn round2_unread_thread_snapshot_hides_revoked_channels() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let source = channel(&server, &owner, "text").await;
    let root = send(&server, &reader, &source, "root").await;
    reply(&server, &owner, root.id, "reply").await;
    let group_root = send(&server, &owner, &source, "group root").await;
    let group =
        json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"everyone"}]}]}});
    server
        .post(
            &owner,
            &format!("{}/thread", message(group_root.id)),
            &group,
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    let snapshot = server.snapshot(&reader).await;
    assert_eq!(snapshot.threads.len(), 2);
    let group_state = snapshot
        .threads
        .iter()
        .find(|s| s.root_id == group_root.id)
        .unwrap();
    assert_eq!((group_state.unread, group_state.mentions), (0, 1));
    let everyone = server.everyone_role(&owner, source.workspace_id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/rules/roles/{}", source.id, everyone.id),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let snapshot = server.snapshot(&reader).await;
    assert!(snapshot.threads.is_empty());
    assert!(!snapshot.threads_more);
    assert!(snapshot.threads_next.is_none());
    let page: Value = server
        .get(&reader, "/api/v1/threads/unread")
        .await
        .ok(StatusCode::OK);
    assert!(page["threads"].as_array().unwrap().is_empty());
    assert_eq!(page["more"], false);
    assert!(page["next"].is_null());
}

#[tokio::test]
async fn round2_fork_marker_requires_destination_access() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let source = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &source, "root").await;
    let copy = fork(&server, &owner, root.id, "text").await;
    assert_eq!(thread(&server, &reader, root.id).await.forks, vec![copy.id]);
    let everyone = server.everyone_role(&owner, source.workspace_id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/rules/roles/{}", copy.id, everyone.id),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert!(thread(&server, &reader, root.id).await.forks.is_empty());
}

#[tokio::test]
async fn round2_mention_inbox_tracks_edits_deletes_and_reads_without_following() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    let group =
        json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"everyone"}]}]}});
    let edited = reply(&server, &owner, root.id, "no mention").await;
    assert!(server.snapshot(&reader).await.threads.is_empty());
    server
        .put(&owner, &message(edited.id), &group)
        .await
        .ok::<Message>(StatusCode::OK);
    let state = server.snapshot(&reader).await.threads.remove(0);
    assert_eq!((state.unread, state.mentions), (0, 1));
    let following: bool = sqlx::query_scalar(
        "select exists(select 1 from thread_reads where account_id=$1 and root_id=$2)",
    )
    .bind(reader.account_id)
    .bind(root.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(!following);
    server
        .put(&owner, &message(edited.id), &text("removed mention"))
        .await
        .ok::<Message>(StatusCode::OK);
    assert!(server.snapshot(&reader).await.threads.is_empty());
    let first: Message = server
        .post(&owner, &format!("{}/thread", message(root.id)), &group)
        .await
        .ok(StatusCode::CREATED);
    let second: Message = server
        .post(&owner, &format!("{}/thread", message(root.id)), &group)
        .await
        .ok(StatusCode::CREATED);
    server
        .put(
            &reader,
            &format!("{}/thread/read", message(root.id)),
            &json!({"position":first.position}),
        )
        .await
        .ok::<ThreadState>(StatusCode::OK);
    let snapshot = server.snapshot(&reader).await;
    assert_eq!(snapshot.threads.len(), 1);
    assert_eq!(snapshot.threads[0].mentions, 1);
    assert_eq!(snapshot.threads[0].unread, 0);
    server
        .delete(&owner, &message(second.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(server.snapshot(&reader).await.threads.is_empty());
    let changed = json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"everyone"},{"kind":"text","text":"edited"}]}]}});
    server
        .put(&owner, &message(first.id), &changed)
        .await
        .ok::<Message>(StatusCode::OK);
    assert!(server.snapshot(&reader).await.threads.is_empty());
    let named = json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"account","account_id":reader.account_id}]}]}});
    let direct: Message = server
        .post(&owner, &format!("{}/thread", message(root.id)), &named)
        .await
        .ok(StatusCode::CREATED);
    let latest: i64 =
        sqlx::query_scalar("select latest from thread_reads where account_id=$1 and root_id=$2")
            .bind(reader.account_id)
            .bind(root.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(
        latest as u64, direct.position,
        "promoted followers must cache the current reply"
    );
    let state = server.snapshot(&reader).await.threads.remove(0);
    assert_eq!(state.latest, direct.position);
    assert_eq!(state.unread, 1);
    assert_eq!(state.mentions, 1);
    reply(&server, &owner, root.id, "following now").await;
    assert_eq!(server.snapshot(&reader).await.threads[0].unread, 2);
}

#[tokio::test]
async fn rule_13_a_fork_records_and_tells_each_override_that_it_copies() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let everyone = server.everyone_role(&owner, channel.workspace_id).await;
    server
        .put(
            &owner,
            &format!(
                "/api/v1/channels/{}/rules/roles/{}",
                channel.id, everyone.id
            ),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let root = send(&server, &owner, &channel, "root").await;
    let mut stream = server.full_stream(&owner).await;
    let copied = fork(&server, &owner, root.id, "text").await;
    let set = junto_protocol::audit::AuditEvent::ChannelOverrideSet {
        role_id: everyone.id,
        allow: vec![],
        deny: vec![junto_protocol::WorkspacePermission::ViewChannel],
    };
    let log: junto_protocol::audit::AuditPage = server
        .get(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/audit-log?target_id={}",
                channel.workspace_id, copied.id
            ),
        )
        .await
        .ok(StatusCode::OK);
    let recorded: Vec<_> = log
        .entries
        .iter()
        .rev()
        .map(|entry| (entry.event.clone(), entry.actor_id, entry.channel_id))
        .collect();
    let by = Some(owner.account_id);
    assert_eq!(
        recorded,
        [
            (
                junto_protocol::audit::AuditEvent::ChannelCreated {
                    name: "fork".to_owned(),
                    kind: junto_protocol::structure::ChannelKind::Text,
                },
                by,
                Some(copied.id)
            ),
            (set, by, Some(copied.id)),
        ]
    );
    let history: MessagePage = server
        .get(&owner, &format!("/api/v1/channels/{}/messages", copied.id))
        .await
        .ok(StatusCode::OK);
    let lines: Vec<_> = history
        .messages
        .iter()
        .filter_map(|message| message.system.as_ref())
        .map(|line| line.action.as_str())
        .collect();
    assert_eq!(lines, ["channel_created", "channel_override_set"]);
    let told = server.traffic(&owner, &mut stream).await;
    assert!(told.iter().any(|(_, event)| matches!(
        event,
        Event::PermissionRuleSet {
            place: junto_protocol::structure::RulePlace::Channel { channel_id },
            subject: junto_protocol::structure::RuleSubject::Role { role_id },
            allow,
            deny,
            ..
        } if *channel_id == copied.id
                && *role_id == everyone.id
                && allow.is_empty()
                && *deny == [junto_protocol::WorkspacePermission::ViewChannel]
    )));
}

/// The change of a reply names its thread and carries the state of the thread for the
/// account that receives it, and a notice event carries the notice of that account, so
/// that a client keeps its lists without reading them again (event revision 2, ADR 0409,
/// ADR 0412).
#[tokio::test]
async fn the_events_of_a_reply_state_the_thread_and_the_notice_of_the_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    // The owner writes in the thread and so follows it.
    reply(&server, &owner, root.id, "own").await;
    let cursor = server.snapshot(&owner).await.cursor;
    let answer = reply(&server, &author, root.id, "an answer").await;
    let told = events(&server, &owner, cursor, Level::Full).await;
    let (thread_root, thread) = told
        .iter()
        .find_map(|event| match event {
            Event::MessageChanged {
                message_id,
                thread_root,
                thread,
                ..
            } if *message_id == answer.id => Some((*thread_root, thread.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(thread_root, Some(root.id));
    let state = thread.unwrap();
    assert_eq!(
        (state.root_id, state.unread, state.following),
        (root.id, 1, true)
    );
    let notice = told
        .iter()
        .find_map(|event| match event {
            Event::NotificationChanged { message_id, notice } if *message_id == answer.id => {
                notice.clone()
            }
            _ => None,
        })
        .unwrap();
    assert_eq!(
        (notice.message_id, notice.kind),
        (
            answer.id,
            junto_protocol::discovery::NotificationKind::Thread
        )
    );
    // The root is no reply: its change names no thread.
    assert!(told.iter().all(|event| !matches!(
        event,
        Event::MessageChanged {
            message_id,
            thread_root: Some(_),
            ..
        } if *message_id == root.id
    )));
}

#[tokio::test]
async fn deleting_unread_replies_states_the_remaining_thread_count() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    reply(&server, &owner, root.id, "own").await;
    let first = reply(&server, &author, root.id, "first").await;
    let second = reply(&server, &author, root.id, "second").await;
    for (removed, remaining) in [(first, 1), (second, 0)] {
        let cursor = server.snapshot(&owner).await.cursor;
        server
            .delete(&author, &message(removed.id))
            .await
            .empty(StatusCode::NO_CONTENT);
        let told = events(&server, &owner, cursor, Level::Full).await;
        let state = told
            .iter()
            .find_map(|event| match event {
                Event::MessageChanged {
                    message_id,
                    deleted: true,
                    thread: Some(state),
                    ..
                } if *message_id == removed.id => Some(state),
                _ => None,
            })
            .expect("the deletion must carry its surviving root's state");
        assert_eq!(state.root_id, root.id);
        assert_eq!(state.unread, remaining);
    }
}
