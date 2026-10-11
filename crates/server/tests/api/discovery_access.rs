use crate::discovery_support::*;
async fn denied_channel_routes(
    server: &TestServer,
    reader: &Member,
    channel: &Channel,
    root: MessageId,
    code: ErrorCode,
) {
    for route in [
        format!("{}/thread", message(root)),
        format!("{}/thread/read", message(root)),
        format!("/api/v1/channels/{}/pins", channel.id),
        format!("/api/v1/notification-settings/{}", channel.id),
    ] {
        server.get(reader, &route).await.error(code);
    }
    server
        .put(
            reader,
            &format!("{}/thread/read", message(root)),
            &json!({"position":0}),
        )
        .await
        .error(code);
    server
        .post(reader, &format!("{}/thread", message(root)), &text("reply"))
        .await
        .error(code);
    server
        .post(
            reader,
            &format!("{}/fork", message(root)),
            &json!({"name":"fork","kind":"text"}),
        )
        .await
        .error(code);
    server
        .put_empty(reader, &format!("{}/pin", message(root)))
        .await
        .error(code);
    server
        .delete(reader, &format!("{}/pin", message(root)))
        .await
        .error(code);
    server
        .put(
            reader,
            &format!("/api/v1/notification-settings/{}", channel.id),
            &json!({"level":"all"}),
        )
        .await
        .error(code);
    server
        .get(
            reader,
            &format!("/api/v1/search?q=quartz&channel_id={}", channel.id),
        )
        .await
        .error(code);
}

#[tokio::test]
async fn rule_37_53_56_revocation_hides_every_route_notice_search_and_replay() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let cursor = server.snapshot(&reader).await.cursor;
    let root = mention(&server, &owner, &channel, &reader).await;
    reply(&server, &reader, root.id, "quartz reply").await;
    server
        .put_empty(&owner, &format!("{}/pin", message(root.id)))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(notices(&server, &reader).await.len(), 1);
    let everyone = server.everyone_role(&owner, channel.workspace_id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", channel.id, everyone.id),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    denied_channel_routes(&server, &reader, &channel, root.id, ErrorCode::NotFound).await;
    assert!(notices(&server, &reader).await.is_empty());
    assert!(search(&server, &reader, "quartz").await.results.is_empty());
    assert!(
        !events(&server, &reader, cursor, Level::Full)
            .await
            .iter()
            .any(|event| matches!(
                event,
                Event::NotificationChanged { .. }
                    | Event::MessageMentioned { .. }
                    | Event::MessageChanged { .. }
                    | Event::ChannelCreated { .. }
            ))
    );
    sqlx::query("delete from workspace_members where workspace_id=$1 and account_id=$2")
        .bind(channel.workspace_id)
        .bind(reader.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    denied_channel_routes(&server, &reader, &channel, root.id, ErrorCode::NotFound).await;
    assert!(server.snapshot(&reader).await.notifications.is_empty());
    let outside_workspace: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"outside"}))
        .await
        .ok(StatusCode::CREATED);
    let outsider = server
        .join_into(&owner, "Outsider", &[outside_workspace.id])
        .await;
    denied_channel_routes(&server, &outsider, &channel, root.id, ErrorCode::NotFound).await;
}

#[tokio::test]
async fn rule_55_nonparticipants_including_admin_cannot_use_direct_thread_pin_or_notices() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let reader = server.join(&owner, "Reader").await;
    let outsider = server.join(&owner, "Outsider").await;
    let conversation: Conversation = server
        .post(
            &author,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let root: Message = server
        .post(
            &author,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
            &text("quartz private"),
        )
        .await
        .ok(StatusCode::CREATED);
    for denied in [&owner, &outsider] {
        server
            .get(denied, &format!("{}/thread", message(root.id)))
            .await
            .error(ErrorCode::NotFound);
        server
            .post(
                denied,
                &format!("{}/thread", message(root.id)),
                &text("reply"),
            )
            .await
            .error(ErrorCode::NotFound);
        server
            .post(
                denied,
                &format!("{}/fork", message(root.id)),
                &json!({"name":"fork","kind":"text"}),
            )
            .await
            .error(ErrorCode::NotFound);
        server
            .put_empty(denied, &format!("{}/pin", message(root.id)))
            .await
            .error(ErrorCode::NotFound);
        server
            .delete(denied, &format!("{}/pin", message(root.id)))
            .await
            .error(ErrorCode::NotFound);
        assert!(search(&server, denied, "quartz").await.results.is_empty());
        assert!(notices(&server, denied).await.is_empty());
    }
}

#[tokio::test]
async fn rule_57_removed_and_banned_accounts_lose_routes_and_streams() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    for banned in [false, true] {
        let reader = server.join(&owner, "Reader").await;
        let root = mention(&server, &owner, &channel, &reader).await;
        let mut stream = server.subscribed(&reader, Level::Background).await;
        if banned {
            server
                .put_empty(
                    &owner,
                    &format!("/api/v1/accounts/{}/ban", reader.account_id),
                )
                .await
                .empty(StatusCode::NO_CONTENT);
        } else {
            server
                .post(
                    &owner,
                    &format!("/api/v1/accounts/{}/removal", reader.account_id),
                    &json!({}),
                )
                .await
                .empty(StatusCode::NO_CONTENT);
        }
        denied_channel_routes(
            &server,
            &reader,
            &channel,
            root.id,
            ErrorCode::Unauthenticated,
        )
        .await;
        for route in [
            "/api/v1/search?q=quartz",
            "/api/v1/notifications",
            "/api/v1/threads/unread",
            "/api/v1/direct-message-retention",
        ] {
            server
                .get(&reader, route)
                .await
                .error(ErrorCode::Unauthenticated);
        }
        assert!(stream.until_closed().await.1.is_some());
    }
}

#[tokio::test]
async fn rule_58_hidden_call_lines_never_become_thread_pin_search_or_notice_content() {
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
    let private = page
        .messages
        .iter()
        .find(|m| {
            m.system
                .as_ref()
                .is_some_and(|s| s.action == "call_started")
        })
        .unwrap();
    // Give the private fixture searchable text and queued notices: removing its audience
    // check must expose content, rather than merely returning another empty search.
    sqlx::query("update messages set system=null,document=$2,author_id=$3 where id=$1")
        .bind(private.id)
        .bind(sqlx::types::Json(text("call_started")["document"].clone()))
        .bind(scene.ann.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    for outsider in [&scene.owner, &scene.ben] {
        settings(server, outsider, uuid::Uuid::nil(), json!({"level":"all"})).await;
        sqlx::query("insert into notifications(id,account_id,message_id,kind,created_at) values($1,$2,$1,'message',$3)")
            .bind(private.id)
            .bind(outsider.account_id)
            .bind(server.clock.now())
            .execute(&server.db)
            .await
            .unwrap();
    }
    assert_eq!(
        search(server, &scene.ann, "call_started")
            .await
            .results
            .len(),
        1
    );
    for ended in [false, true] {
        if ended {
            server
                .delete(&scene.ann, "/api/v1/call")
                .await
                .empty(StatusCode::NO_CONTENT);
        }
        for outsider in [&scene.owner, &scene.ben] {
            server
                .get(outsider, &format!("{}/thread", message(private.id)))
                .await
                .error(ErrorCode::NotFound);
            server
                .post(
                    outsider,
                    &format!("{}/thread", message(private.id)),
                    &text("reply"),
                )
                .await
                .error(ErrorCode::NotFound);
            server
                .post(
                    outsider,
                    &format!("{}/fork", message(private.id)),
                    &json!({"name":"fork","kind":"voice"}),
                )
                .await
                .error(ErrorCode::NotFound);
            server
                .put_empty(outsider, &format!("{}/pin", message(private.id)))
                .await
                .error(ErrorCode::NotFound);
            assert!(
                search(server, outsider, "call_started")
                    .await
                    .results
                    .is_empty()
            );
            assert!(notices(server, outsider).await.is_empty());
        }
    }
}

#[tokio::test]
async fn rule_59_deleted_channel_loses_routes_search_notices_and_keeps_audit() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = mention(&server, &owner, &channel, &reader).await;
    server
        .delete(&owner, &format!("/api/v1/channels/{}", channel.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    denied_channel_routes(&server, &owner, &channel, root.id, ErrorCode::NotFound).await;
    assert!(search(&server, &reader, "quartz").await.results.is_empty());
    assert!(notices(&server, &reader).await.is_empty());
    let audit: Value = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/audit-log", channel.workspace_id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(audit.to_string().contains(&channel.id.to_string()));
}

#[tokio::test]
async fn rule_60_concurrent_pins_preserve_limit_and_rollback_refused_events() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "root").await;
    sqlx::query("with copied as(insert into messages(id,place_id,position,author_id,created_at,document) select
             gen_random_uuid(),place_id,position+n,author_id,created_at,document from messages cross join
             generate_series(1,99) n where id=$1 returning id,place_id,author_id,created_at) insert into message_pins
             select id,place_id,author_id,created_at from copied")
        .bind(root.id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("update message_places set head=head+99 where id=$1")
        .bind(channel.id)
        .execute(&server.db)
        .await
        .unwrap();
    let second = send(&server, &owner, &channel, "second").await;
    let cursor = server.snapshot(&owner).await.cursor;
    let a = format!("{}/pin", message(root.id));
    let b = format!("{}/pin", message(second.id));
    server.gate().arm(0);
    let first = server.put_empty(&owner, &a);
    tokio::pin!(first);
    crate::support::drive_until(&mut first, server.gate().arrived()).await;
    let second = server.put_empty(&owner, &b).await;
    server.gate().release();
    let first = first.await;
    assert_eq!(
        [first.status, second.status]
            .iter()
            .filter(|status| **status == StatusCode::NO_CONTENT)
            .count(),
        1
    );
    if first.status == StatusCode::NO_CONTENT {
        second.error(ErrorCode::LimitReached);
    } else {
        first.error(ErrorCode::LimitReached);
    }
    assert_eq!(
        events(&server, &owner, cursor, Level::Full)
            .await
            .iter()
            .filter(|e| matches!(e, Event::MessageChanged { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn rule_61_new_write_and_read_routes_share_bounded_account_limits() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "quartz").await;
    while server
        .state
        .message_writes
        .attempt_of(owner.account_id, server.clock.now())
        .is_ok()
    {}
    server
        .post(
            &owner,
            &format!("{}/thread", message(root.id)),
            &text("reply"),
        )
        .await
        .error(ErrorCode::RateLimited);
    server
        .post(
            &owner,
            &format!("{}/fork", message(root.id)),
            &json!({"name":"fork","kind":"text"}),
        )
        .await
        .error(ErrorCode::RateLimited);
    server
        .put_empty(&owner, &format!("{}/pin", message(root.id)))
        .await
        .error(ErrorCode::RateLimited);
    server
        .put(
            &owner,
            &format!("/api/v1/notification-settings/{}", channel.id),
            &json!({"level":"all"}),
        )
        .await
        .error(ErrorCode::RateLimited);
    server
        .put(
            &owner,
            "/api/v1/direct-message-retention",
            &json!({"days":1}),
        )
        .await
        .error(ErrorCode::RateLimited);
    while server
        .state
        .message_history
        .attempt_of(owner.account_id, server.clock.now())
        .is_ok()
    {}
    for route in [
        format!("{}/thread", message(root.id)),
        format!("/api/v1/channels/{}/pins", channel.id),
        "/api/v1/search?q=quartz".into(),
        "/api/v1/notifications".into(),
        "/api/v1/threads/unread".into(),
    ] {
        server
            .get(&owner, &route)
            .await
            .error(ErrorCode::RateLimited);
    }
    server.clock.advance(time::Duration::minutes(1));
    assert_eq!(search(&server, &owner, "quartz").await.results.len(), 1);
}

#[tokio::test]
async fn rule_63_database_rejects_cross_place_threads_pins_and_invalid_retention() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let first = channel(&server, &owner, "text").await;
    let second = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &first, "root").await;
    let child = send(&server, &owner, &second, "child").await;
    assert!(
        sqlx::query("update messages set thread_root=$2 where id=$1")
            .bind(child.id)
            .bind(root.id)
            .execute(&server.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("insert into message_pins values($1,$2,$3,$4)")
            .bind(root.id)
            .bind(second.id)
            .bind(owner.account_id)
            .bind(server.clock.now())
            .execute(&server.db)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("update direct_message_retention set days=0")
            .execute(&server.db)
            .await
            .is_err()
    );
    server
        .get(&owner, &format!("/api/v1/channels/{}/pins", first.id))
        .await
        .ok::<Vec<Message>>(StatusCode::OK);
}

#[tokio::test]
async fn rule_64_individual_permanence_unmarking_is_absent() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({"permanent_messages":true}),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    let root = send(&server, &owner, &channel, "record").await;
    server
        .put(
            &owner,
            &format!("{}/permanent", message(root.id)),
            &json!({"permanent":false}),
        )
        .await
        .error(ErrorCode::NotFound);
    assert!(
        server
            .get(&owner, &message(root.id))
            .await
            .ok::<Message>(StatusCode::OK)
            .permanent
    );
    let document: Value = serde_json::from_str(&junto_server::openapi_document().unwrap()).unwrap();
    assert!(
        document["paths"]
            .as_object()
            .unwrap()
            .keys()
            .all(|route| !route.contains("/permanent"))
    );
}
