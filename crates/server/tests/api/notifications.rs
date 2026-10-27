use crate::discovery_support::*;
#[tokio::test]
async fn rule_23_notification_defaults_resolve_channel_workspace_server() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    settings(&server, &reader, uuid::Uuid::nil(), json!({"level":"all"})).await;
    let first = send(&server, &owner, &channel, "server all").await;
    assert_eq!(notices(&server, &reader).await[0].message_id, first.id);
    server
        .put(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/notification-default",
                channel.workspace_id
            ),
            &json!({"level":"nothing"}),
        )
        .await
        .ok::<NotificationDefault>(StatusCode::OK);
    send(&server, &owner, &channel, "workspace nothing").await;
    assert!(notices(&server, &reader).await.is_empty());
    settings(
        &server,
        &reader,
        channel.workspace_id,
        json!({"level":"all"}),
    )
    .await;
    let second = send(&server, &owner, &channel, "personal workspace all").await;
    assert!(
        notices(&server, &reader)
            .await
            .iter()
            .any(|n| n.message_id == second.id)
    );
    settings(&server, &reader, channel.id, json!({"level":"nothing"})).await;
    send(&server, &owner, &channel, "channel nothing").await;
    assert!(notices(&server, &reader).await.is_empty());
}

#[tokio::test]
async fn rule_24_levels_select_notices_and_unread_signs() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let route = format!("/api/v1/channels/{}/messages/read", channel.id);
    send(&server, &owner, &channel, "ordinary").await;
    assert!(notices(&server, &reader).await.is_empty());
    let state: MessageState = server.get(&reader, &route).await.ok(StatusCode::OK);
    assert_eq!(state.unread, 1);
    assert!(!state.unread_sign);
    mention(&server, &owner, &channel, &reader).await;
    let state: MessageState = server.get(&reader, &route).await.ok(StatusCode::OK);
    assert!(state.unread_sign);
    assert_eq!(state.notification_mentions, 1);
    settings(&server, &reader, channel.id, json!({"level":"nothing"})).await;
    let state: MessageState = server.get(&reader, &route).await.ok(StatusCode::OK);
    assert!(!state.unread_sign);
    assert_eq!(state.notification_mentions, 0);
    assert!(notices(&server, &reader).await.is_empty());
    settings(&server, &reader, channel.id, json!({"level":"all"})).await;
    assert!(
        server
            .get(&reader, &route)
            .await
            .ok::<MessageState>(StatusCode::OK)
            .unread_sign
    );
}

#[tokio::test]
async fn rule_25_26_direct_mentions_survive_independent_role_and_everyone_switches() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let role: junto_protocol::roles::WorkspaceRole = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", channel.workspace_id),
            &json!({"name":"team"}),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{}",
                channel.workspace_id, reader.account_id, role.id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let cursor = server.snapshot(&reader).await.cursor;
    let everyone: Message = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"everyone"}]}]}}),
        )
        .await
        .ok(StatusCode::CREATED);
    let role_message:Message=server.post(&owner,&format!("/api/v1/channels/{}/messages",channel.id),&json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"role","role_id":role.id}]}]}})).await.ok(StatusCode::CREATED);
    let explicit = mention(&server, &owner, &channel, &reader).await;
    assert_eq!(notices(&server, &reader).await.len(), 3);
    settings(
        &server,
        &reader,
        channel.id,
        json!({"role_mentions":false,"everyone_mentions":true}),
    )
    .await;
    let listed = notices(&server, &reader).await;
    assert!(listed.iter().any(|n| n.message_id == everyone.id));
    assert!(!listed.iter().any(|n| n.message_id == role_message.id));
    settings(
        &server,
        &reader,
        channel.id,
        json!({"role_mentions":true,"everyone_mentions":false}),
    )
    .await;
    let listed = notices(&server, &reader).await;
    assert!(!listed.iter().any(|n| n.message_id == everyone.id));
    assert!(listed.iter().any(|n| n.message_id == role_message.id));
    settings(
        &server,
        &reader,
        channel.id,
        json!({"role_mentions":false,"everyone_mentions":false}),
    )
    .await;
    let listed = notices(&server, &reader).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].message_id, explicit.id);
    let events = events(&server, &reader, cursor, Level::Background).await;
    assert!(events.iter().any(
        |event| matches!(event,Event::NotificationChanged{message_id,..} if *message_id==explicit.id)
    ));
    assert!(!events.iter().any(|event|matches!(event,Event::NotificationChanged{message_id,..}|Event::MessageMentioned{message_id,..} if *message_id==role_message.id || *message_id==everyone.id)));
}

#[tokio::test]
async fn rule_27_28_mute_expiry_restores_level_for_each_duration() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    for hours in [1, 8, 24] {
        let until = server.clock.now() + time::Duration::hours(hours);
        settings(&server,&reader,channel.id,json!({"level":"all","mute":true,"muted_until":until.format(&time::format_description::well_known::Rfc3339).unwrap()})).await;
        let muted = mention(&server, &owner, &channel, &reader).await;
        assert!(
            !notices(&server, &reader)
                .await
                .iter()
                .any(|n| n.message_id == muted.id)
        );
        let state: MessageState = server
            .get(
                &reader,
                &format!("/api/v1/channels/{}/messages/read", channel.id),
            )
            .await
            .ok(StatusCode::OK);
        assert!(state.muted);
        assert!(!state.unread_sign);
        server.clock.advance(time::Duration::hours(hours));
        let active = send(&server, &owner, &channel, "active").await;
        assert!(
            notices(&server, &reader)
                .await
                .iter()
                .any(|n| n.message_id == active.id)
        );
    }
    settings(
        &server,
        &reader,
        channel.id,
        json!({"level":"all","mute":true}),
    )
    .await;
    let muted = send(&server, &owner, &channel, "indefinite").await;
    assert!(notices(&server, &reader).await.is_empty());
    settings(
        &server,
        &reader,
        channel.id,
        json!({"level":"all","mute":false}),
    )
    .await;
    assert!(
        !notices(&server, &reader)
            .await
            .iter()
            .any(|n| n.message_id == muted.id)
    );
    let active = send(&server, &owner, &channel, "unmuted").await;
    assert!(
        notices(&server, &reader)
            .await
            .iter()
            .any(|n| n.message_id == active.id)
    );
}

#[tokio::test]
async fn rule_29_dnd_schedule_synchronizes_without_suppressing_unread() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let cursor = server.snapshot(&reader).await.cursor;
    settings(&server,&reader,uuid::Uuid::nil(),json!({"do_not_disturb":true,"daily_quiet":{"start_minute":1320,"end_minute":480,"time_zone":"America/Sao_Paulo"}})).await;
    mention(&server, &owner, &channel, &reader).await;
    let snapshot = server.snapshot(&reader).await;
    assert!(snapshot.notification_settings[0].settings.do_not_disturb);
    assert_eq!(
        snapshot.notification_settings[0]
            .settings
            .daily_quiet
            .as_ref()
            .unwrap()
            .start_minute,
        1320
    );
    assert!(
        snapshot
            .messages
            .iter()
            .find(
                |s| matches!(s.place,MessagePlace::Channel{channel_id,..} if channel_id==channel.id)
            )
            .unwrap()
            .unread_sign
    );
    assert!(
        events(&server, &reader, cursor, Level::Background)
            .await
            .iter()
            .any(|event| matches!(event, Event::NotificationSettingsChanged {}))
    );
    server
        .put(
            &reader,
            &format!("/api/v1/notification-settings/{}", uuid::Uuid::nil()),
            &json!({"daily_quiet":{"start_minute":1440,"end_minute":1,"time_zone":"UTC"}}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn rule_30_31_direct_notices_obey_contact_and_conversation_mute() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let reader = server.join(&owner, "Reader").await;
    let conversation: Conversation = server
        .post(
            &author,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let route = format!("/api/v1/conversations/{}/messages", conversation.id);
    let sent: Message = server
        .post(&author, &route, &text("direct"))
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(notices(&server, &reader).await[0].message_id, sent.id);
    assert!(notices(&server, &author).await.is_empty());
    assert!(notices(&server, &owner).await.is_empty());
    server
        .put(
            &reader,
            &format!("/api/v1/direct-messages/{}/mute", author.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(&author, &route, &text("muted"))
        .await
        .ok::<Message>(StatusCode::CREATED);
    assert!(notices(&server, &reader).await.is_empty());
    server
        .delete(
            &reader,
            &format!("/api/v1/direct-messages/{}/mute", author.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let active: Message = server
        .post(&author, &route, &text("active"))
        .await
        .ok(StatusCode::CREATED);
    assert!(
        notices(&server, &reader)
            .await
            .iter()
            .any(|n| n.message_id == active.id)
    );
    let workspace = server.default_workspace(&owner).await;
    sqlx::query("delete from workspace_members where workspace_id=$1 and account_id=$2")
        .bind(workspace.id)
        .bind(reader.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    assert!(notices(&server, &reader).await.is_empty());
    assert!(search(&server, &reader, "direct").await.results.is_empty());
}

#[tokio::test]
async fn rule_32_thread_replies_notify_previous_writers_only() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let reader = server.join(&owner, "Reader").await;
    let outsider = server.join(&owner, "Outsider").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &author, &channel, "root").await;
    let first = reply(&server, &reader, root.id, "reply").await;
    assert!(
        notices(&server, &author)
            .await
            .iter()
            .any(|n| n.message_id == first.id && n.kind == NotificationKind::Thread)
    );
    assert!(notices(&server, &reader).await.is_empty());
    assert!(notices(&server, &outsider).await.is_empty());
    let second = reply(&server, &owner, root.id, "third author").await;
    for writer in [&author, &reader] {
        assert!(
            notices(&server, writer)
                .await
                .iter()
                .any(|n| n.message_id == second.id)
        );
    }
    assert!(
        !notices(&server, &owner)
            .await
            .iter()
            .any(|n| n.message_id == second.id)
    );
    let role = server
        .grant_workspace(&owner, &reader, channel.workspace_id, &[])
        .await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", channel.id, role),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let cursor = server.snapshot(&reader).await.cursor;
    let last = reply(&server, &owner, root.id, "after revocation").await;
    assert!(notices(&server, &reader).await.is_empty());
    assert!(
        !events(&server, &reader, cursor, Level::Full)
            .await
            .iter()
            .any(|e| matches!(e,Event::NotificationChanged{message_id,..} if *message_id==last.id))
    );
    let stored: bool = sqlx::query_scalar(
        "select exists(select 1 from notifications where account_id=$1 and message_id=$2)",
    )
    .bind(reader.account_id)
    .bind(last.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(!stored);
}

#[tokio::test]
async fn rule_33_38_edits_and_reactions_do_not_repeat_or_preserve_deleted_content() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &owner, &channel, "quartz").await;
    let mentioned = mention(&server, &owner, &channel, &reader).await;
    server
        .put(
            &owner,
            &message(root.id),
            &json!({"document":mentioned.document}),
        )
        .await
        .ok::<Message>(StatusCode::OK);
    assert_eq!(notices(&server, &reader).await.len(), 2);
    let cursor = server.snapshot(&reader).await.cursor;
    server
        .put(
            &owner,
            &message(root.id),
            &json!({"document":mentioned.document}),
        )
        .await
        .ok::<Message>(StatusCode::OK);
    server
        .put(
            &owner,
            &format!("{}/reaction", message(root.id)),
            &json!({"emoji":"👍"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(
        !events(&server, &reader, cursor, Level::Background)
            .await
            .iter()
            .any(|event| matches!(
                event,
                Event::NotificationChanged { .. } | Event::MessageMentioned { .. }
            ))
    );
    server
        .delete(&owner, &message(root.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(
        !notices(&server, &reader)
            .await
            .iter()
            .any(|n| n.message_id == root.id)
    );
}

#[tokio::test]
async fn rule_34_35_36_notices_read_state_snapshot_and_stream_levels() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    settings(&server, &reader, channel.id, json!({"level":"all"})).await;
    let cursor = server.snapshot(&reader).await.cursor;
    let ordinary = send(&server, &owner, &channel, "ordinary").await;
    let mentioned = mention(&server, &owner, &channel, &reader).await;
    let full = events(&server, &reader, cursor, Level::Full).await;
    let background = events(&server, &reader, cursor, Level::Background).await;
    assert!(full.iter().any(
        |e| matches!(e,Event::NotificationChanged{message_id,..} if *message_id==ordinary.id)
    ));
    assert!(!background.iter().any(
        |e| matches!(e,Event::NotificationChanged{message_id,..} if *message_id==ordinary.id)
    ));
    assert!(background.iter().any(
        |e| matches!(e,Event::NotificationChanged{message_id,..} if *message_id==mentioned.id)
    ));
    let notice = notices(&server, &reader)
        .await
        .into_iter()
        .find(|n| n.message_id == mentioned.id)
        .unwrap();
    server
        .put_empty(&owner, &format!("/api/v1/notifications/{}/read", notice.id))
        .await
        .error(ErrorCode::NotFound);
    let cursor = server.snapshot(&reader).await.cursor;
    let read: Notification = server
        .put_empty(
            &reader,
            &format!("/api/v1/notifications/{}/read", notice.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(read.read_at.is_some());
    assert!(
        server
            .snapshot(&reader)
            .await
            .notifications
            .iter()
            .find(|n| n.id == notice.id)
            .unwrap()
            .read_at
            .is_some()
    );
    assert!(
        events(&server, &reader, cursor, Level::Background)
            .await
            .iter()
            .any(
                |e| matches!(e,Event::NotificationChanged{message_id,..} if *message_id==mentioned.id)
            )
    );
}

#[tokio::test]
async fn added_mention_upgrades_an_existing_general_notice_once() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    settings(&server, &reader, channel.id, json!({"level":"all"})).await;
    let root = send(&server, &owner, &channel, "ordinary").await;
    assert_eq!(
        notices(&server, &reader).await[0].kind,
        NotificationKind::Message
    );
    let cursor = server.snapshot(&reader).await.cursor;
    let body = json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"account","account_id":reader.account_id}]}]}});
    server
        .put(&owner, &message(root.id), &body)
        .await
        .ok::<Message>(StatusCode::OK);
    assert_eq!(
        notices(&server, &reader).await[0].kind,
        NotificationKind::Mention
    );
    assert!(
        events(&server, &reader, cursor, Level::Background)
            .await
            .iter()
            .any(|e| matches!(e,Event::NotificationChanged{message_id,..} if *message_id==root.id))
    );
}

#[tokio::test]
async fn notice_pages_put_a_new_mention_of_an_old_message_first() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let old = send(&server, &owner, &channel, "old").await;
    let recent = mention(&server, &owner, &channel, &reader).await;
    server.clock.advance(time::Duration::seconds(1));
    server
        .put(
            &owner,
            &message(old.id),
            &json!({"document":recent.document}),
        )
        .await
        .ok::<Message>(StatusCode::OK);
    let first: NotificationPage = server
        .get(&reader, "/api/v1/notifications?limit=1")
        .await
        .ok(StatusCode::OK);
    assert_eq!(first.notifications[0].message_id, old.id);
    assert!(first.has_more);
    let next: NotificationPage = server
        .get(
            &reader,
            &format!(
                "/api/v1/notifications?limit=1&before={}",
                first.notifications[0].id
            ),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(next.notifications[0].message_id, recent.id);
    assert!(!next.has_more);
}

#[tokio::test]
async fn fix_notices_are_not_stored_for_accounts_without_channel_access() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    settings(&server, &reader, channel.id, json!({"level":"all"})).await;
    let everyone = server.everyone_role(&owner, channel.workspace_id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", channel.id, everyone.id),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let hidden = mention(&server, &owner, &channel, &reader).await;
    let stored: i64 = sqlx::query_scalar(
        "select count(*) from notifications where message_id=$1 and account_id=$2",
    )
    .bind(hidden.id)
    .bind(reader.account_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(stored, 0);
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", channel.id, everyone.id),
            &json!({"allow":[],"deny":[]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert!(notices(&server, &reader).await.is_empty());
}

#[tokio::test]
async fn fix_reading_a_place_marks_its_notices_read() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let sent = mention(&server, &owner, &channel, &reader).await;
    assert!(notices(&server, &reader).await[0].read_at.is_none());
    server
        .put(
            &reader,
            &format!("/api/v1/channels/{}/messages/read", channel.id),
            &json!({"position":sent.position}),
        )
        .await
        .ok::<MessageState>(StatusCode::OK);
    assert!(notices(&server, &reader).await[0].read_at.is_some());
}

#[tokio::test]
async fn fix_unknown_or_other_account_notice_cursor_is_invalid() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let sent = mention(&server, &owner, &channel, &reader).await;
    for id in [sent.id.as_uuid(), uuid::Uuid::new_v4()] {
        server
            .get(&owner, &format!("/api/v1/notifications?before={id}"))
            .await
            .error(ErrorCode::InvalidRequest);
    }
}

#[tokio::test]
async fn fix_thread_notices_do_not_reach_background_without_a_mention() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &reader, &channel, "root").await;
    let cursor = server.snapshot(&reader).await.cursor;
    let child = reply(&server, &owner, root.id, "reply").await;
    assert!(
        notices(&server, &reader)
            .await
            .iter()
            .any(|n| n.message_id == child.id)
    );
    assert!(!events(&server, &reader, cursor, Level::Background).await.iter().any(|event| matches!(event,Event::NotificationChanged{message_id,..} if *message_id==child.id)));
    assert!(events(&server, &reader, cursor, Level::Full).await.iter().any(|event| matches!(event,Event::NotificationChanged{message_id,..} if *message_id==child.id)));
}

#[tokio::test]
async fn fix_dnd_suppresses_notifications_but_preserves_unread() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    settings(
        &server,
        &reader,
        uuid::Uuid::nil(),
        json!({"do_not_disturb":true}),
    )
    .await;
    let cursor = server.snapshot(&reader).await.cursor;
    let sent = mention(&server, &owner, &channel, &reader).await;
    let seen = events(&server, &reader, cursor, Level::Background).await;
    assert!(!seen.iter().any(|e| matches!(
        e,
        Event::NotificationChanged { .. } | Event::MessageMentioned { .. }
    )));
    let state: MessageState = server
        .get(
            &reader,
            &format!("/api/v1/channels/{}/messages/read", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(state.unread_sign);
    assert_eq!(state.mentions, 1);
    assert!(
        notices(&server, &reader)
            .await
            .iter()
            .any(|n| n.message_id == sent.id)
    );
    settings(
        &server,
        &reader,
        uuid::Uuid::nil(),
        json!({"do_not_disturb":false}),
    )
    .await;
    assert!(!events(&server,&reader,cursor,Level::Background).await.iter().any(|e|matches!(e,Event::NotificationChanged{message_id,..}|Event::MessageMentioned{message_id,..} if *message_id==sent.id)));

    settings(
        &server,
        &reader,
        uuid::Uuid::nil(),
        json!({"do_not_disturb":true}),
    )
    .await;
    let cursor = server.snapshot(&reader).await.cursor;
    server
        .put_empty(&reader, &format!("/api/v1/notifications/{}/read", sent.id))
        .await
        .ok::<Notification>(StatusCode::OK);
    assert!(
        events(&server, &reader, cursor, Level::Background)
            .await
            .iter()
            .any(|e| matches!(e,Event::NotificationChanged{message_id,..} if *message_id==sent.id))
    );
}

#[tokio::test]
async fn fix_notice_maintenance_keeps_only_the_newest_500_and_90_days() {
    let server = TestServer::start_with(|c| c.limits.maintenance_batch = 17).await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let old = mention(&server, &owner, &channel, &reader).await;
    sqlx::query(
        "update notifications set created_at=created_at-interval '91 days' where message_id=$1",
    )
    .bind(old.id)
    .execute(&server.db)
    .await
    .unwrap();
    let recent = mention(&server, &owner, &channel, &reader).await;
    sqlx::query("with copied as (insert into messages(id,place_id,position,author_id,created_at,document) select
             gen_random_uuid(),place_id,position+n,author_id,created_at,document from messages cross join
             generate_series(1,535) n where id=$1 returning id,created_at) insert into
             notifications(id,account_id,message_id,kind,created_at) select id,$2,id, 'thread' ,created_at from copied")
        .bind(recent.id)
        .bind(reader.account_id)
        .execute(&mut server.test_data().await)
        .await
        .unwrap();
    // Three empty retention transactions, the age trim, then one cap batch.
    server.gate().arm(5);
    let run = junto_server::messages::retention::run(&server.state);
    tokio::pin!(run);
    tokio::select! {
        biased;
        () = server.gate().arrived() => {}
        result = &mut run => panic!("notice trim finished without bounded batches: {result:?}"),
    }
    let remaining: i64 =
        sqlx::query_scalar("select count(*) from notifications where account_id=$1")
            .bind(reader.account_id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert_eq!(remaining, 519);
    server.gate().release();
    run.await.unwrap();
    let count: i64 = sqlx::query_scalar("select count(*) from notifications where account_id=$1")
        .bind(reader.account_id)
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert_eq!(count, 500);
    assert!(
        !notices(&server, &reader)
            .await
            .iter()
            .any(|n| n.message_id == old.id)
    );
    let old_exists: bool =
        sqlx::query_scalar("select exists(select 1 from notifications where message_id=$1)")
            .bind(old.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(!old_exists);
}

#[tokio::test]
async fn fix_scope_switches_mutes_and_past_expiries_are_refused() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    for scope in [uuid::Uuid::nil(), channel.workspace_id.as_uuid()] {
        for body in [
            json!({"mute":true}),
            json!({"role_mentions":false}),
            json!({"everyone_mentions":false}),
        ] {
            server
                .put(
                    &owner,
                    &format!("/api/v1/notification-settings/{scope}"),
                    &body,
                )
                .await
                .error(ErrorCode::InvalidRequest);
        }
    }
    let past = (server.clock.now() - time::Duration::seconds(1))
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap();
    server
        .put(
            &owner,
            &format!("/api/v1/notification-settings/{}", channel.id),
            &json!({"mute":true,"muted_until":past}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn fix_scheduled_dnd_suppresses_direct_notices_and_call_invites_until_it_ends() {
    let scene = crate::calls_support::Scene::start().await;
    let server = &scene.server;
    let (sender, reader) = (&scene.ann, &scene.ben);
    let minute =
        (u16::from(server.clock.now().hour()) * 60 + u16::from(server.clock.now().minute()) + 120)
            % 1440;
    settings(server,reader,uuid::Uuid::nil(),json!({"daily_quiet":{"start_minute":(minute+1439)%1440,"end_minute":(minute+1)%1440,"time_zone":"Etc/GMT-2"}})).await;
    let conversation: Conversation = server
        .post(
            sender,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let cursor = server.snapshot(reader).await.cursor;
    server
        .post(
            sender,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
            &text("quiet direct"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    server.enter_direct(sender, reader).await;
    assert!(server.snapshot(reader).await.call_invitations.is_empty());
    assert!(
        !events(server, reader, cursor, Level::Background)
            .await
            .iter()
            .any(|e| matches!(
                e,
                Event::NotificationChanged { .. } | Event::CallInvited { .. }
            ))
    );
    server
        .delete(sender, "/api/v1/call")
        .await
        .empty(StatusCode::NO_CONTENT);
    server.clock.advance(time::Duration::minutes(2));
    let cursor = server.snapshot(reader).await.cursor;
    let next: Message = server
        .post(
            sender,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
            &text("awake direct"),
        )
        .await
        .ok(StatusCode::CREATED);
    assert!(
        events(server, reader, cursor, Level::Background)
            .await
            .iter()
            .any(|e| matches!(e,Event::NotificationChanged{message_id,..} if *message_id==next.id))
    );
    server.enter_direct(sender, reader).await;
    assert_eq!(server.snapshot(reader).await.call_invitations.len(), 1);
}

#[tokio::test]
async fn fix_ordinary_sends_do_not_evaluate_policy_for_unsubscribed_members() {
    use crate::messages_queries::measure_requests;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    for n in 0..12 {
        server.join(&owner, &format!("Reader {n}")).await;
    }
    let (_held, _role) = measure_requests(&server).await;
    let mut connection = server.db.acquire().await.unwrap();
    sqlx::query("set track_functions='all'")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("alter function notification_policy(uuid,uuid,timestamptz) set enable_hashjoin=on")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("select pg_stat_reset_single_function_counters('notification_policy(uuid,uuid,timestamptz)'::regprocedure)")
        .execute(&mut *connection)
        .await
        .unwrap();
    drop(connection);
    send(&server, &owner, &channel, "ordinary").await;
    let mut connection = server.db.acquire().await.unwrap();
    sqlx::query("select pg_stat_force_next_flush()")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("select pg_stat_clear_snapshot()")
        .execute(&mut *connection)
        .await
        .unwrap();
    let count: Option<i64> = sqlx::query_scalar("select calls from pg_stat_user_functions where funcid='notification_policy(uuid,uuid,timestamptz)'::regprocedure")
        .fetch_optional(&mut *connection)
        .await
        .unwrap();
    assert_eq!(count.unwrap_or(0), 0);
}

#[tokio::test]
async fn round2_equal_quiet_endpoints_are_refused() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    server
        .put(
            &owner,
            &format!("/api/v1/notification-settings/{}", uuid::Uuid::nil()),
            &json!({"daily_quiet":{"start_minute":600,"end_minute":600,"time_zone":"UTC"}}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn round2_added_mention_upgrades_a_thread_notice() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let root = send(&server, &reader, &channel, "root").await;
    let child = reply(&server, &owner, root.id, "reply").await;
    server
        .put_empty(&reader, &format!("/api/v1/notifications/{}/read", child.id))
        .await
        .ok::<Value>(StatusCode::OK);
    let cursor = server.snapshot(&reader).await.cursor;
    server.put(&owner,&message(child.id),&json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"account","account_id":reader.account_id}]}]}})).await.ok::<Message>(StatusCode::OK);
    let notice = notices(&server, &reader).await.remove(0);
    assert_eq!(notice.kind, NotificationKind::Mention);
    assert!(notice.read_at.is_none());
    assert!(
        events(&server, &reader, cursor, Level::Background)
            .await
            .iter()
            .any(
                |e| matches!(e,Event::NotificationChanged{message_id,..} if *message_id==child.id)
            )
    );
}

#[tokio::test]
async fn round2_dnd_checks_replayed_notices_and_channel_invites() {
    use crate::calls_support::Scene;
    let scene = Scene::start().await;
    let server = &scene.server;
    let cursor = server.snapshot(&scene.ben).await.cursor;
    let message = mention(server, &scene.ann, &scene.voice, &scene.ben).await;
    assert!(
        events(server, &scene.ben, cursor, Level::Background)
            .await
            .iter()
            .any(
                |e| matches!(e,Event::NotificationChanged{message_id,..} if *message_id==message.id)
            )
    );
    settings(
        server,
        &scene.ben,
        uuid::Uuid::nil(),
        json!({"do_not_disturb":true}),
    )
    .await;
    server.enter_call(&scene.ann, scene.voice.id).await;
    server
        .post(
            &scene.ann,
            &format!("/api/v1/channels/{}/call/invitations", scene.voice.id),
            &json!({"account_id":scene.ben.account_id}),
        )
        .await
        .ok::<Value>(StatusCode::CREATED);
    for level in [Level::Full, Level::Background] {
        assert!(
            !events(server, &scene.ben, cursor, level)
                .await
                .iter()
                .any(|e| matches!(
                    e,
                    Event::NotificationChanged { .. }
                        | Event::MessageMentioned { .. }
                        | Event::CallInvited { .. }
                ))
        );
    }
}

#[tokio::test]
async fn round2_notice_trim_visits_only_changed_recipients() {
    use crate::messages_queries::{measure_requests, since, statements};
    let server = TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    mention(&server, &owner, &channel, &reader).await;
    junto_server::messages::retention::run(&server.state)
        .await
        .unwrap();
    let (mut held, role) = measure_requests(&server).await;
    let observer = &mut **held.first_mut().unwrap();
    let before = statements(observer, role).await;
    junto_server::messages::retention::run(&server.state)
        .await
        .unwrap();
    let changed = since(statements(observer, role).await, before);
    assert!(!changed.is_empty());
    assert!(
        !changed
            .keys()
            .any(|q| q.contains("offset") && q.contains("notifications")),
        "quiet accounts must not have their notice cap scanned: {changed:?}"
    );
    mention(&server, &owner, &channel, &reader).await;
    let before = statements(observer, role).await;
    junto_server::messages::retention::run(&server.state)
        .await
        .unwrap();
    let changed = since(statements(observer, role).await, before);
    assert!(
        changed
            .keys()
            .any(|q| q.contains("offset") && q.contains("notifications"))
    );
    server.require_no_statement_eviction(evictions).await;
}

#[tokio::test]
async fn round2_account_subscribers_are_limited_to_memberships() {
    use crate::messages_queries::measure_requests;
    let server = TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    for n in 0..8 {
        let other = server.join(&owner, &format!("Reader {n}")).await;
        settings(&server, &other, uuid::Uuid::nil(), json!({"level":"all"})).await;
    }
    let workspace: Value = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"separate"}))
        .await
        .ok(StatusCode::CREATED);
    let channel: Channel = server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/channels",
                workspace["id"].as_str().unwrap()
            ),
            &json!({"name":"alone","kind":"text"}),
        )
        .await
        .ok(StatusCode::CREATED);
    let (mut held, role) = measure_requests(&server).await;
    send(&server, &owner, &channel, "ordinary").await;
    let count:i64=sqlx::query_scalar("select coalesce(sum(rows),0)::bigint from pg_stat_statements where userid=$1::oid and query like 'with channels_selected as materialized%'").bind(role).fetch_one(&mut **held.first_mut().unwrap()).await.unwrap();
    assert_eq!(
        count, 0,
        "standing must not load subscribers of unrelated workspaces"
    );
    server.require_no_statement_eviction(evictions).await;
}

#[tokio::test]
async fn round2_delivery_authorizes_only_received_notices() {
    use crate::messages_queries::{measure_requests, since, statements};
    use junto_server::events::delivery::{Subscription, next_batch};
    let server = TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let outsider = server.join(&owner, "Other").await;
    let channel = channel(&server, &owner, "text").await;
    let start = server.snapshot(&outsider).await.cursor;
    mention(&server, &owner, &channel, &reader).await;
    let (mut held, _role) = measure_requests(&server).await;
    let observer = &mut **held.first_mut().unwrap();
    let role: i64 =
        sqlx::query_scalar("select oid::bigint from pg_roles where rolname=current_user")
            .fetch_one(&mut *observer)
            .await
            .unwrap();
    let subscription = Subscription {
        level: Level::Background,
        position: junto_server::events::log::change_of(start.position),
        epoch: start.epoch,
    };
    next_batch(
        &server.state,
        &crate::stream_review::opened(&outsider),
        subscription,
        500,
    )
    .await
    .unwrap();
    let before = statements(observer, role).await;
    next_batch(
        &server.state,
        &crate::stream_review::opened(&outsider),
        subscription,
        500,
    )
    .await
    .unwrap();
    let changed = since(statements(observer, role).await, before);
    assert!(
        !changed
            .keys()
            .any(|q| q.starts_with("with channels_selected")
                || q.starts_with("select n.* from notifications")),
        "unrelated connections must not authorize notice contents: {changed:?}"
    );
    server.require_no_statement_eviction(evictions).await;
}

#[tokio::test]
async fn round2_snapshot_and_delivery_queries_are_batched() {
    use crate::messages_queries::{measure_requests, since, statements};
    use junto_server::events::delivery::{Subscription, next_batch};
    let server = TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    settings(&server, &reader, channel.id, json!({"level":"all"})).await;
    let start = server.snapshot(&reader).await.cursor;
    let root = send(&server, &reader, &channel, "root").await;
    reply(&server, &owner, root.id, "reply").await;
    let (mut held, role) = measure_requests(&server).await;
    let observer = &mut **held.first_mut().unwrap();
    server.snapshot(&reader).await;
    let before = statements(observer, role).await;
    server.snapshot(&reader).await;
    let small_snapshot = since(statements(observer, role).await, before);
    let stream_role: i64 =
        sqlx::query_scalar("select oid::bigint from pg_roles where rolname=current_user")
            .fetch_one(&mut *observer)
            .await
            .unwrap();
    let subscription = Subscription {
        level: Level::Full,
        position: junto_server::events::log::change_of(start.position),
        epoch: start.epoch,
    };
    next_batch(
        &server.state,
        &crate::stream_review::opened(&reader),
        subscription,
        500,
    )
    .await
    .unwrap();
    let before = statements(observer, stream_role).await;
    next_batch(
        &server.state,
        &crate::stream_review::opened(&reader),
        subscription,
        500,
    )
    .await
    .unwrap();
    let relevant = |q: &String| {
        q.starts_with("with channels_selected")
            || q.starts_with("select n.* from notifications")
            || q.contains("from notification_receivers")
    };
    let small_delivery: std::collections::BTreeMap<_, _> =
        since(statements(observer, stream_role).await, before)
            .into_iter()
            .filter(|(q, _)| relevant(q))
            .collect();
    assert!(!small_delivery.is_empty());
    for counts in [&small_snapshot, &small_delivery] {
        assert_eq!(
            counts
                .iter()
                .filter(|(q, _)| q.starts_with("with channels_selected"))
                .map(|(_, n)| n)
                .sum::<i64>(),
            1
        );
    }
    for _ in 0..6 {
        let root = send(&server, &reader, &channel, "root").await;
        reply(&server, &owner, root.id, "reply").await;
    }
    let before = statements(observer, role).await;
    server.snapshot(&reader).await;
    assert_eq!(
        since(statements(observer, role).await, before),
        small_snapshot
    );
    let before = statements(observer, stream_role).await;
    next_batch(
        &server.state,
        &crate::stream_review::opened(&reader),
        subscription,
        500,
    )
    .await
    .unwrap();
    assert_eq!(
        since(statements(observer, stream_role).await, before)
            .into_iter()
            .filter(|(q, _)| relevant(q))
            .collect::<std::collections::BTreeMap<_, _>>(),
        small_delivery
    );
    for n in 0..5 {
        let workspace: serde_json::Value = server
            .post(
                &owner,
                "/api/v1/workspaces",
                &json!({"name":format!("workspace {n}")}),
            )
            .await
            .ok(StatusCode::CREATED);
        let workspace: uuid::Uuid = workspace["id"].as_str().unwrap().parse().unwrap();
        sqlx::query(
            "insert into workspace_members(workspace_id,account_id,joined_at) values($1,$2,$3)",
        )
        .bind(workspace)
        .bind(reader.account_id)
        .bind(server.state.now())
        .execute(&mut *observer)
        .await
        .unwrap();
    }
    let relevant_snapshot =
        |q: &String| relevant(q) || q.starts_with("with followed") || q.starts_with("select p.id");
    server.snapshot(&reader).await;
    let before = statements(observer, role).await;
    server.snapshot(&reader).await;
    assert_eq!(
        since(statements(observer, role).await, before)
            .into_iter()
            .filter(|(q, _)| relevant_snapshot(q))
            .collect::<std::collections::BTreeMap<_, _>>(),
        small_snapshot
            .into_iter()
            .filter(|(q, _)| relevant_snapshot(q))
            .collect()
    );
    let before = statements(observer, stream_role).await;
    next_batch(
        &server.state,
        &crate::stream_review::opened(&reader),
        subscription,
        500,
    )
    .await
    .unwrap();
    assert_eq!(
        since(statements(observer, stream_role).await, before)
            .into_iter()
            .filter(|(q, _)| relevant(q))
            .collect::<std::collections::BTreeMap<_, _>>(),
        small_delivery
    );
    server.require_no_statement_eviction(evictions).await;
}

#[tokio::test]
async fn old_clients_read_and_change_settings_after_a_new_device_sets_privacy() {
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    #[allow(dead_code)]
    struct OldSettings {
        level: Option<NotificationLevel>,
        mute: bool,
        muted_until: Option<String>,
        role_mentions: Option<bool>,
        everyone_mentions: Option<bool>,
        do_not_disturb: bool,
        daily_quiet: Option<DailyQuiet>,
    }
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let path = format!("/api/v1/notification-settings/{}", uuid::Uuid::nil());
    let response = TestServer::reply(
        reqwest::Client::new()
            .put(format!("{}{path}", server.base))
            .bearer_auth(&owner.token)
            .header("Junto-Notification-Preferences", "1")
            .json(&json!({"hide_text":true,"hold_while_presenting":false})),
    )
    .await;
    response.ok::<NotificationSettings>(StatusCode::OK);
    let old = server.get(&owner, &path).await.ok::<Value>(StatusCode::OK);
    serde_json::from_value::<OldSettings>(old).expect("old client reads preferences");
    let snapshot = server
        .get(&owner, "/api/v1/snapshot")
        .await
        .ok::<Value>(StatusCode::OK);
    for scope in snapshot["notification_settings"].as_array().unwrap() {
        serde_json::from_value::<OldSettings>(scope["settings"].clone())
            .expect("old client reads snapshot");
    }
    let old = server
        .put(&owner, &path, &json!({"level":"all"}))
        .await
        .ok::<Value>(StatusCode::OK);
    serde_json::from_value::<OldSettings>(old).expect("old client reads mutation response");
    let current = TestServer::reply(
        reqwest::Client::new()
            .get(format!("{}{path}", server.base))
            .bearer_auth(&owner.token)
            .header("Junto-Notification-Preferences", "1"),
    )
    .await
    .ok::<NotificationSettings>(StatusCode::OK);
    assert!(current.hide_text);
    assert_eq!(current.hold_while_presenting, Some(false));
    assert_eq!(current.level, Some(NotificationLevel::All));
    server
        .put(&owner, &path, &json!({"unknown_preference":true}))
        .await
        .error(ErrorCode::InvalidRequest);
    let future = serde_json::from_value::<NotificationSettings>(json!({"future_preference":true}));
    assert!(future.is_ok(), "response settings allow future fields");
}
