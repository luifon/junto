use crate::discovery_support::*;
#[tokio::test]
async fn rule_13_16_permanence_and_count_include_threads_and_forks() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    assert!(!channel.permanent_messages);
    let ordinary = send(&server, &owner, &channel, "ordinary").await;
    assert!(!ordinary.permanent);
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({"permanent_messages":true}),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    let root = send(&server, &owner, &channel, "permanent").await;
    let child = reply(&server, &owner, root.id, "permanent reply").await;
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({"permanent_messages":false}),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    let copy = fork(&server, &owner, root.id, "text").await;
    for id in [channel.id, copy.id] {
        let count: i64 =
            sqlx::query_scalar("select permanent_message_count from channels where id=$1")
                .bind(id)
                .fetch_one(&server.db)
                .await
                .unwrap();
        assert_eq!(count, 2);
    }
    assert!(thread(&server, &owner, root.id).await.root.permanent);
    assert!(child.permanent);
    server
        .delete(&owner, &message(child.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let count: i64 = sqlx::query_scalar("select permanent_message_count from channels where id=$1")
        .bind(channel.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn rule_14_15_permanent_edits_and_deletes_enforce_administrator_limits() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let admin = server.join(&owner, "Admin").await;
    let moderator = server.join(&owner, "Moderator").await;
    let channel = channel(&server, &owner, "text").await;
    server
        .grant_server(
            &owner,
            &admin,
            &[junto_protocol::ServerPermission::Administrator],
        )
        .await;
    server
        .grant_workspace(
            &owner,
            &moderator,
            channel.workspace_id,
            &[junto_protocol::WorkspacePermission::ManageMessages],
        )
        .await;
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({"permanent_messages":true}),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    let root = send(&server, &author, &channel, "permanent").await;
    for member in [&author, &admin, &moderator, &owner] {
        server
            .put(member, &message(root.id), &text("edit"))
            .await
            .error(ErrorCode::Forbidden);
    }
    for member in [&author, &moderator] {
        server
            .delete(member, &message(root.id))
            .await
            .error(ErrorCode::Forbidden);
    }
    server
        .delete(&admin, &message(root.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let page: MessagePage = server
        .get(
            &author,
            &format!("/api/v1/channels/{}/messages", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        page.messages
            .iter()
            .filter(|m| m
                .system
                .as_ref()
                .is_some_and(|s| s.action == "permanent_message_deleted"))
            .count(),
        1
    );
    let audit: Value = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/audit-log", channel.workspace_id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        audit
            .to_string()
            .matches("permanent_message_deleted")
            .count(),
        1
    );
}

#[tokio::test]
async fn rule_17_18_permanent_channels_need_owner_and_setting_needs_manager() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let member = server.join(&owner, "Member").await;
    let admin = server.join(&owner, "Admin").await;
    let channel = channel(&server, &owner, "text").await;
    server
        .grant_server(
            &owner,
            &admin,
            &[junto_protocol::ServerPermission::Administrator],
        )
        .await;
    let route = format!("/api/v1/channels/{}", channel.id);
    server
        .patch(&member, &route, &json!({"permanent_messages":true}))
        .await
        .error(ErrorCode::Forbidden);
    let cursor = server.snapshot(&member).await.cursor;
    server
        .patch(&owner, &route, &json!({"permanent_messages":true}))
        .await
        .ok::<Channel>(StatusCode::OK);
    assert!(events(&server,&member,cursor,Level::Full).await.iter().any(|event|matches!(event,Event::ChannelUpdated{channel:updated} if updated.id==channel.id && updated.permanent_messages)));
    send(&server, &member, &channel, "record").await;
    server
        .delete(&admin, &route)
        .await
        .error(ErrorCode::OwnerOnly);
    server
        .delete(&owner, &route)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&owner, &format!("{route}/pins"))
        .await
        .error(ErrorCode::NotFound);
    let marker: Option<time::OffsetDateTime> =
        sqlx::query_scalar("select deleted_at from channels where id=$1")
            .bind(channel.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(marker.is_some());
}

#[tokio::test]
async fn rule_19_21_pins_require_permission_and_track_current_content() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &author, &channel, "original").await;
    let route = format!("{}/pin", message(root.id));
    server
        .put_empty(&author, &route)
        .await
        .error(ErrorCode::Forbidden);
    server
        .delete(&author, &route)
        .await
        .error(ErrorCode::Forbidden);
    server
        .put_empty(&owner, &route)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put(&author, &message(root.id), &text("edited"))
        .await
        .ok::<Message>(StatusCode::OK);
    let pins: Vec<Message> = server
        .get(&author, &format!("/api/v1/channels/{}/pins", channel.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(pins.len(), 1);
    assert!(pins[0].pinned);
    assert!(!pins[0].permanent);
    assert!(serde_json::to_string(&pins[0]).unwrap().contains("edited"));
    server
        .delete(&author, &message(root.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(
        server
            .get(&owner, &format!("/api/v1/channels/{}/pins", channel.id))
            .await
            .ok::<Vec<Message>>(StatusCode::OK)
            .is_empty()
    );
    let page: MessagePage = server
        .get(&owner, &format!("/api/v1/channels/{}/messages", channel.id))
        .await
        .ok(StatusCode::OK);
    let system = page.messages.iter().find(|m| m.system.is_some()).unwrap();
    server
        .put_empty(&owner, &format!("{}/pin", message(system.id)))
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn rule_20_22_pin_limit_idempotence_and_change_events() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "pin").await;
    let route = format!("{}/pin", message(root.id));
    let cursor = server.snapshot(&owner).await.cursor;
    server
        .put_empty(&owner, &route)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(&owner, &route)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        events(&server, &owner, cursor, Level::Full)
            .await
            .iter()
            .filter(
                |event| matches!(event,Event::MessageChanged{message_id,..} if *message_id==root.id)
            )
            .count(),
        1
    );
    // The copies take the positions after the head: the line of the pin above holds one.
    sqlx::query("with copied as(insert into messages(id,place_id,position,author_id,created_at,document) select
             gen_random_uuid(),m.place_id,p.head+n,m.author_id,m.created_at,m.document from messages m join
             message_places p on p.id=m.place_id cross join generate_series(1,99) n where m.id=$1
             returning id,place_id,author_id,created_at) insert into message_pins
             select id,place_id,author_id,created_at from copied")
        .bind(root.id)
        .execute(&mut server.test_data().await)
        .await
        .unwrap();
    sqlx::query("update message_places set head=head+99 where id=$1")
        .bind(channel.id)
        .execute(&server.db)
        .await
        .unwrap();
    let extra = send(&server, &owner, &channel, "too many").await;
    server
        .put_empty(&owner, &route)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(&owner, &format!("{}/pin", message(extra.id)))
        .await
        .error(ErrorCode::LimitReached);
    server
        .delete(&owner, &route)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&owner, &route)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .put_empty(&owner, &format!("{}/pin", message(extra.id)))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        server
            .get(&owner, &format!("/api/v1/channels/{}/pins", channel.id))
            .await
            .ok::<Vec<Message>>(StatusCode::OK)
            .len(),
        100
    );
}

#[tokio::test]
async fn fix_permanent_count_survives_concurrent_thread_sends_and_deletes() {
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
    let root = send(&server, &owner, &channel, "root").await;
    let (first, second) = tokio::join!(
        reply(&server, &owner, root.id, "first"),
        reply(&server, &owner, root.id, "second")
    );
    let count: i64 = sqlx::query_scalar("select permanent_message_count from channels where id=$1")
        .bind(channel.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 3);
    assert_eq!(thread(&server, &owner, root.id).await.root.reply_count, 2);
    let route = message(first.id);
    let second_route = message(second.id);
    let (one, two) = tokio::join!(
        server.delete(&owner, &route),
        server.delete(&owner, &second_route)
    );
    one.empty(StatusCode::NO_CONTENT);
    two.empty(StatusCode::NO_CONTENT);
    let count: i64 = sqlx::query_scalar("select permanent_message_count from channels where id=$1")
        .bind(channel.id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 1);
    assert_eq!(thread(&server, &owner, root.id).await.root.reply_count, 0);
}
