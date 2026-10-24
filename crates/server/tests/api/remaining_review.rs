use crate::calls_support::Scene;
use crate::support::{DeviceKey, TestServer};
use junto_protocol::{
    ServerPermission,
    account::Avatar,
    auth::SessionCreated,
    call::OwnCallState,
    error::ErrorCode,
    invite::{Invite, Redemption},
    structure::Workspace,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

#[tokio::test]
async fn review2_workspace_removal_revokes_a_single_workspace_invite() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let creator = server.join(&owner, "Creator").await;
    server
        .grant_server(&owner, &creator, &[ServerPermission::CreateInvites])
        .await;
    let workspace = server.default_workspace(&owner).await;
    let invite = server.invite(&creator, json!({"max_uses": 5})).await;
    let other = server.invite(&owner, json!({"max_uses": 2})).await;
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, creator.account_id
            ),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .join_reply(&DeviceKey::generate(), invite.code.expose(), "Returned")
        .await
        .error(ErrorCode::InviteRevoked);
    server
        .post(
            &creator,
            "/api/v1/invites/redemption",
            &json!({"invite_code":invite.code.expose()}),
        )
        .await
        .error(ErrorCode::InviteRevoked);
    let listed: Vec<Invite> = server
        .get(&owner, "/api/v1/invites")
        .await
        .ok(StatusCode::OK);
    assert!(
        listed
            .iter()
            .find(|item| item.id == invite.invite.id)
            .unwrap()
            .revocation
            .is_some()
    );
    let audit: i64 = sqlx::query_scalar("select count(*) from audit_entries where action='invite_revoked' and event->>'invite_id'=$1").bind(invite.invite.id.to_string()).fetch_one(&server.db).await.unwrap();
    let events: i64 = sqlx::query_scalar(
        "select count(*) from events where kind='invite_revoked' and payload->>'invite_id'=$1",
    )
    .bind(invite.invite.id.to_string())
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!((audit, events), (1, 1));
    server
        .post(
            &creator,
            "/api/v1/invites/redemption",
            &json!({"invite_code":other.code.expose()}),
        )
        .await
        .ok::<Redemption>(StatusCode::OK);
    let retained = server.invite(&creator, json!({"max_uses":2})).await;
    server
        .post(
            &owner,
            &format!("/api/v1/accounts/{}/removal", creator.account_id),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .join_reply(&creator.key, retained.code.expose(), "New account")
        .await
        .ok::<SessionCreated>(StatusCode::CREATED);
}

#[tokio::test]
async fn review2_workspace_removal_narrows_a_multi_workspace_invite() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let creator = server.join(&owner, "Creator").await;
    let reader = server.join(&owner, "Reader").await;
    server
        .grant_server(&owner, &creator, &[ServerPermission::CreateInvites])
        .await;
    let first = server.default_workspace(&owner).await;
    let second: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Second"}))
        .await
        .ok(StatusCode::CREATED);
    server
        .put(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                second.id, creator.account_id
            ),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let invite = server
        .invite(
            &creator,
            json!({"max_uses":5,"workspace_ids":[first.id, second.id]}),
        )
        .await;
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                first.id, reader.account_id
            ),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let before = server.snapshot(&owner).await.cursor;
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                first.id, creator.account_id
            ),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let changed = crate::discovery_support::events(
        &server,
        &owner,
        before,
        junto_protocol::events::Level::Full,
    )
    .await;
    let changed: Vec<Value> = changed
        .into_iter()
        .map(|e| serde_json::to_value(e).unwrap())
        .collect();
    assert!(changed.iter().any(|e| e["type"] == "invite_changed"
        && e["invite"]["id"] == json!(invite.invite.id)
        && e["invite"]["workspace_ids"] == json!([second.id])));
    let audit:i64=sqlx::query_scalar("select count(*) from audit_entries where action='invite_workspaces_changed' and event->>'invite_id'=$1 and event->>'removed_workspace_id'=$2").bind(invite.invite.id.to_string()).bind(first.id.to_string()).fetch_one(&server.db).await.unwrap();
    assert_eq!(audit, 1);
    let joined: SessionCreated = server
        .join_reply(&DeviceKey::generate(), invite.code.expose(), "New reader")
        .await
        .ok(StatusCode::CREATED);
    let workspaces: Vec<junto_protocol::ids::WorkspaceId> =
        sqlx::query_scalar("select workspace_id from workspace_members where account_id=$1")
            .bind(joined.account_id)
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert_eq!(workspaces, [second.id]);
    let redeemed: Redemption = server
        .post(
            &reader,
            "/api/v1/invites/redemption",
            &json!({"invite_code":invite.code.expose()}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        serde_json::to_value(redeemed).unwrap()["workspace_ids"],
        json!([second.id])
    );
    let listed: Vec<Invite> = server
        .get(&owner, "/api/v1/invites")
        .await
        .ok(StatusCode::OK);
    let narrowed = listed
        .iter()
        .find(|item| item.id == invite.invite.id)
        .unwrap();
    assert_eq!(narrowed.workspace_ids, [second.id]);
    assert!(narrowed.revocation.is_none());
}

#[tokio::test]
async fn review2_absent_inside_is_not_stated() {
    let scene = Scene::start().await;
    scene.server.enter_call(&scene.ann, scene.voice.id).await;
    let own = scene.server.own_call(&scene.ann).await.call.unwrap();
    let mut value = serde_json::to_value(own).unwrap();
    assert_eq!(value["inside"], true);
    value.as_object_mut().unwrap().remove("inside");
    let decoded: OwnCallState = serde_json::from_value(value).unwrap();
    assert!(serde_json::to_value(decoded).unwrap()["inside"].is_null());
}

#[tokio::test]
async fn review2_audit_targets_ignore_non_uuid_identifiers() {
    let server = TestServer::start().await;
    let id = uuid::Uuid::new_v4();
    let rows: Vec<uuid::Uuid> = sqlx::query_scalar("select target_id from audit_targets($1,null,null,null)")
        .bind(json!({"room_id":"room-name", "segment_ids":["invalid",id,4,null],"account_id":id,"reason":id.to_string()}))
        .fetch_all(&server.db).await.unwrap();
    assert_eq!(rows, [id]);
}

#[tokio::test]
async fn review2_logo_revalidation_and_image_rules() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let route = "/api/v1/server/logo";
    for image in [
        junto_server::image::samples::animated_png(8, 8),
        junto_server::image::samples::webp_extended(8, 8, true),
    ] {
        server
            .put_bytes(&owner, route, None, image)
            .await
            .error(ErrorCode::ImageUnsupported);
    }
    let logo: Avatar = server
        .put_bytes(&owner, route, None, junto_server::image::samples::png(8, 8))
        .await
        .ok(StatusCode::OK);
    let client = reqwest::Client::new();
    let url = format!("{}{route}", server.base);
    let response = client.get(&url).send().await.unwrap();
    assert_eq!(response.headers()["cache-control"], "public, no-cache");
    let tag = format!("\"{}\"", logo.hash);
    for condition in [
        tag.clone(),
        format!("W/{tag}"),
        format!("\"other\", {tag}"),
        "*".to_owned(),
    ] {
        let response = client
            .get(&url)
            .header("if-none-match", condition)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()["etag"], tag);
        assert_eq!(response.headers()["cache-control"], "public, no-cache");
        assert!(response.bytes().await.unwrap().is_empty());
    }
    assert_eq!(
        client
            .get(url)
            .header("if-none-match", "\"different\"")
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn review2_messages_do_not_wake_an_unrelated_background_connection() {
    use futures_util::FutureExt;
    use junto_protocol::events::Level;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let channel = crate::messages_support::channel(&server, &owner, "text").await;
    let conversation: junto_protocol::message::Conversation = server
        .post(
            &owner,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let app = junto_server::state::AppState::new(
        server.database_with(|_| {}).await,
        server.state.config.clone(),
        server.state.clock.clone(),
        server.state.server_id,
        None,
        None,
    );
    let registration = app.streams.open(third.account_id, third.device_id).unwrap();
    registration.link().set_level(Some(Level::Background));
    let recipient = app
        .streams
        .open(reader.account_id, reader.device_id)
        .unwrap();
    recipient.link().set_level(Some(Level::Background));
    let mut recipient_wakes = 0;
    let mut wakes = 0;
    for route in [
        format!("/api/v1/channels/{}/messages", channel.id),
        format!("/api/v1/conversations/{}/messages", conversation.id),
    ] {
        for n in 0..3 {
            let before = server.snapshot(&third).await.cursor;
            server
                .post(
                    &owner,
                    &route,
                    &crate::messages_support::text(&format!("message {n}")),
                )
                .await
                .ok::<junto_protocol::message::Message>(StatusCode::CREATED);
            junto_server::events::watcher::advance(
                &app,
                Some(junto_server::events::log::change_of(before.position)),
            )
            .await
            .unwrap();
            wakes += usize::from(registration.link().news().now_or_never().is_some());
            recipient_wakes += usize::from(recipient.link().news().now_or_never().is_some());
        }
    }
    assert_eq!(wakes, 0);
    assert_eq!(recipient_wakes, 3);
}

#[tokio::test]
async fn review2_actual_audit_queries_bound_time_ranges_and_combined_filters() {
    use crate::plan_support::{analyze, index_names, visited_rows};
    use junto_protocol::audit::AuditFilterQuery;
    use sqlx::Execute;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let other = server.join(&owner, "Other").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "text").await;
    sqlx::query("insert into audit_entries(id,occurred_at,actor_id,workspace_id,channel_id,action,event) select ('00000000-0000-7000-8000-'||lpad(to_hex(n),12,'0'))::uuid,$1 - case when n>5000 then interval '60 days' else interval '1 day' end,case when n%2=0 then $2 else $3 end,$4,$5,case when n%2=0 then 'channel_created' else 'channel_updated' end,jsonb_build_object('action',case when n%2=0 then 'channel_created' else 'channel_updated' end,'name','example') from generate_series(1,10000) n")
        .bind(server.clock.now()).bind(owner.account_id).bind(other.account_id).bind(workspace.id).bind(channel.id).execute(&server.db).await.unwrap();
    analyze(&server.db, &["audit_entries", "audit_entry_targets"]).await;
    let queries = [
        AuditFilterQuery {
            since: Some(server.clock.now() - time::Duration::days(30)),
            ..Default::default()
        },
        AuditFilterQuery {
            action: Some("channel_updated".into()),
            actor_id: Some(owner.account_id),
            ..Default::default()
        },
    ];
    let mut costs = Vec::new();
    for query in queries {
        let mut statement = junto_server::routes::audit_log::statement(
            &query,
            Some(workspace.id),
            Some((owner.account_id, vec![channel.id])),
            50,
        );
        let mut built = statement.build();
        let arguments = built.take_arguments().unwrap().unwrap();
        let sql = format!("explain (analyze,format json) {}", built.sql().as_str());
        let plan: Value = sqlx::query_scalar_with(sqlx::AssertSqlSafe(sql), arguments)
            .fetch_one(&server.db)
            .await
            .unwrap();
        costs.push((
            format!("{query:?}"),
            visited_rows(&plan[0]["Plan"]),
            index_names(&plan),
            plan,
        ));
    }
    assert!(
        costs
            .iter()
            .all(|(_, rows, indexes, _)| *rows < 1000 && !indexes.is_empty()),
        "{costs:?}"
    );
}

#[tokio::test]
async fn review2_logo_reads_are_limited_per_address() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let route = "/api/v1/server/logo";
    server
        .put_bytes(&owner, route, None, junto_server::image::samples::png(1, 1))
        .await
        .ok::<Avatar>(StatusCode::OK);
    let client = reqwest::Client::new();
    let url = format!("{}{route}", server.base);
    for _ in 0..120 {
        assert_eq!(
            client.get(&url).send().await.unwrap().status(),
            StatusCode::OK
        );
    }
    let refused = client.get(&url).send().await.unwrap();
    assert_eq!(refused.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.headers()["retry-after"], "60");
    server.clock.advance(time::Duration::minutes(1));
    assert_eq!(
        client.get(url).send().await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn review2_read_boundaries_cover_missed_calls_mentions_and_older_marks() {
    use crate::discovery_support::{notices, settings};
    use crate::messages_support::{channel, send, text};
    use junto_protocol::{
        discovery::NotificationsRead,
        message::{Conversation, Message},
    };
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    settings(&server, &reader, uuid::Uuid::nil(), json!({"level":"all"})).await;
    let channel = channel(&server, &owner, "text").await;
    let sent = send(&server, &owner, &channel, "plain").await;
    let conversation: Conversation = server
        .post(
            &owner,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let line: Message = server
        .post(
            &owner,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
            &text("call line"),
        )
        .await
        .ok(StatusCode::CREATED);
    sqlx::query("delete from notifications where message_id=$1")
        .bind(line.id)
        .execute(&server.db)
        .await
        .unwrap();
    settings(&server, &reader, channel.id, json!({"mute":true})).await;
    let marked: NotificationsRead = server
        .put(&reader, "/api/v1/notifications/read", &json!({}))
        .await
        .ok(StatusCode::OK);
    let unread: bool = sqlx::query_scalar(
        "select read_at is null from notifications where account_id=$1 and message_id=$2",
    )
    .bind(reader.account_id)
    .bind(sent.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(unread);
    settings(&server, &reader, channel.id, json!({"mute":false})).await;
    server.clock.advance(time::Duration::seconds(1));
    server.put(&reader,"/api/v1/notifications/read",&json!({"until":(marked.until-time::Duration::seconds(1)).format(&time::format_description::well_known::Rfc3339).unwrap()})).await.ok::<NotificationsRead>(StatusCode::OK);
    let boundary: (time::OffsetDateTime, time::OffsetDateTime) = sqlx::query_as(
        "select read_until,read_at from notification_read_boundaries where account_id=$1",
    )
    .bind(reader.account_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(boundary, (marked.until, marked.read_at));
    let mut tx = server.db.begin().await.unwrap();
    junto_server::messages::notifications::missed_call(
        &mut tx,
        marked.until,
        line.id,
        conversation.id.as_uuid(),
        reader.account_id,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let missed = notices(&server, &reader)
        .await
        .into_iter()
        .find(|n| n.message_id == line.id)
        .unwrap();
    assert_eq!(missed.read_at, Some(marked.read_at));
    server.clock.advance(time::Duration::seconds(-1));
    server.put(&owner,&format!("/api/v1/messages/{}",sent.id),&json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"account","account_id":reader.account_id}]}]}})).await.ok::<Message>(StatusCode::OK);
    let mentioned = notices(&server, &reader)
        .await
        .into_iter()
        .find(|n| n.message_id == sent.id)
        .unwrap();
    assert_eq!(
        mentioned.kind,
        junto_protocol::discovery::NotificationKind::Mention
    );
    assert_eq!(mentioned.read_at, Some(marked.read_at));
}

#[tokio::test]
async fn review2_the_kept_log_covers_the_original_number_of_messages() {
    use junto_protocol::events::{Cursor, Level};
    use junto_server::events::{
        delivery::{self, Batch, Start, Subscription},
        log,
    };
    for muted in [false, true] {
        let server = TestServer::start().await;
        let owner = server.owner().await;
        let channel = crate::messages_support::channel(&server, &owner, "text").await;
        let reader = server.join(&owner, "Reader").await;
        crate::discovery_support::settings(
            &server,
            &reader,
            channel.id,
            json!({"level":"all","mute":muted}),
        )
        .await;
        let message = crate::messages_support::send(&server, &owner, &channel, "template").await;
        let initial = server.snapshot(&owner).await.cursor;
        // Populate the shipped retention bound with changes of a message and its notice, as
        // a message writes them. A message whose notice has no recipient, in the muted case,
        // records no notice.
        let primary:Value=sqlx::query_scalar("select to_jsonb(e)-'id'-'position'-'ordinal' from events e where kind='message_changed' and payload->>'message_id'=$1 order by position desc limit 1").bind(message.id.to_string()).fetch_one(&server.db).await.unwrap();
        let notice:Option<Value>=sqlx::query_scalar("select to_jsonb(e)-'id'-'position'-'ordinal' from events e where kind='notification_changed' and payload->>'message_id'=$1 order by position desc limit 1").bind(message.id.to_string()).fetch_optional(&server.db).await.unwrap();
        let kept = server.state.config.limits.stream.events_kept;
        let insert = "with pair as materialized (select gen_random_uuid() as first, gen_random_uuid() as second) insert into events select (jsonb_populate_record(null::events,case when slot=0 then $1 else $2 end || jsonb_build_object('id',case when slot=0 then first else second end))).* from pair cross join generate_series(0,$3) slot order by slot";
        let pairs = |count: u32| {
            let (primary, notice, server) = (&primary, &notice, &server);
            async move {
                for _ in 0..count {
                    sqlx::query(insert)
                        .bind(primary)
                        .bind(notice)
                        .bind(i32::from(notice.is_some()))
                        .execute(&server.db)
                        .await
                        .unwrap();
                }
            }
        };
        pairs(kept).await;
        log::trim(&server.state).await.unwrap();
        let messages: i64 =
            sqlx::query_scalar("select count(*) from events where kind='message_changed'")
                .fetch_one(&server.db)
                .await
                .unwrap();
        assert_eq!(messages, kept as i64);
        let rows: i64 = sqlx::query_scalar("select count(*) from events")
            .fetch_one(&server.db)
            .await
            .unwrap();
        assert_eq!(rows, (1 + i64::from(notice.is_some())) * kept as i64);
        assert_eq!(notice.is_none(), muted);
        let positions: Vec<i64> =
            sqlx::query_scalar("select distinct position from events order by position")
                .fetch_all(&server.db)
                .await
                .unwrap();
        assert_eq!(positions.len(), kept as usize);
        let head = server.snapshot(&owner).await.cursor;
        let who = crate::stream_review::opened(&owner);
        let most = server.state.config.limits.stream.replay_most_events as usize;
        let middle = positions[positions.len() - most - 1];
        let at_change = |change: i64| Cursor {
            epoch: head.epoch,
            position: log::stream_position(change, log::END_OF_CHANGE).unwrap(),
        };
        let cursor = at_change(middle);
        assert!(matches!(
            delivery::subscribe(&server.state, &who, Level::Full, Some(cursor))
                .await
                .unwrap(),
            Start::Subscribed(_)
        ));
        let batch = delivery::next_batch(
            &server.state,
            &who,
            Subscription {
                epoch: head.epoch,
                position: middle,
                level: Level::Full,
            },
            100,
        )
        .await
        .unwrap();
        let Batch::Events { deliveries, .. } = batch else {
            panic!("{batch:?}")
        };
        assert_eq!(
            deliveries.len(),
            server.state.config.limits.stream.batch.min(100) as usize
        );
        assert!(deliveries.iter().all(|delivery| delivery.events == 1));
        assert!(matches!(
            delivery::subscribe(&server.state, &who, Level::Background, Some(initial))
                .await
                .unwrap(),
            Start::Subscribed(_)
        ));
        let oldest = positions[0];
        pairs(1).await;
        log::trim(&server.state).await.unwrap();
        let old: i64 = sqlx::query_scalar("select count(*) from events where position = $1")
            .bind(oldest)
            .fetch_one(&server.db)
            .await
            .unwrap();
        assert_eq!(old, 0);
        let old_cursor = at_change(oldest - 1);
        assert!(matches!(
            delivery::subscribe(&server.state, &who, Level::Background, Some(old_cursor))
                .await
                .unwrap(),
            Start::Resync(_)
        ));
        let boundary = at_change(oldest);
        assert!(matches!(
            delivery::subscribe(&server.state, &who, Level::Background, Some(boundary))
                .await
                .unwrap(),
            Start::Subscribed(_)
        ));
    }
}

#[tokio::test]
async fn review2_invite_names_and_avatar_metadata_have_the_containing_audience() {
    use junto_protocol::{access_token::TokenAction, call::CallJoin, structure::WorkspaceMember};
    let server = TestServer::start_for_calls().await;
    let owner = server.owner().await;
    let administrator = server.administrator(&owner, "Administrator").await;
    let private: Workspace = server
        .post(
            &administrator,
            "/api/v1/workspaces",
            &json!({"name":"Separate"}),
        )
        .await
        .ok(StatusCode::CREATED);
    let reader = server.join_into(&owner, "Reader", &[private.id]).await;
    server
        .grant_server(&owner, &reader, &[ServerPermission::CreateInvites])
        .await;
    let invite = server
        .invite(&owner, json!({"workspace_ids":[private.id]}))
        .await;
    server
        .get(&reader, &format!("/api/v1/accounts/{}", owner.account_id))
        .await
        .error(ErrorCode::NotFound);
    let listed: Vec<Invite> = server
        .get(&reader, "/api/v1/invites")
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        listed
            .iter()
            .find(|i| i.id == invite.invite.id)
            .unwrap()
            .created_by_name
            .as_deref(),
        Some("Owner")
    );
    let avatar: Avatar = server
        .put_bytes(
            &owner,
            "/api/v1/account/avatar",
            None,
            junto_server::image::samples::png(8, 8),
        )
        .await
        .ok(StatusCode::OK);
    let workspace = server.default_workspace(&owner).await;
    let token = server
        .access_token(&owner, &[TokenAction::ReadStructure])
        .await;
    let members: Vec<WorkspaceMember> = server
        .with_token(
            reqwest::Method::GET,
            &format!("/api/v1/workspaces/{}/members", workspace.id),
            &token,
            None,
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(
        members
            .iter()
            .find(|m| m.account_id == owner.account_id)
            .unwrap()
            .avatar,
        Some(avatar.clone())
    );
    server
        .with_token(
            reqwest::Method::GET,
            &format!("/api/v1/accounts/{}/avatar", owner.account_id),
            &token,
            None,
        )
        .await
        .error(ErrorCode::Forbidden);
    let join: CallJoin = crate::calls_review2::ring(&server, &owner, &reader)
        .await
        .ok(StatusCode::OK);
    server.connect(join);
    server.check().await;
    let invitation = server
        .snapshot(&reader)
        .await
        .call_invitations
        .pop()
        .unwrap();
    let join: CallJoin = server
        .post(
            &reader,
            &format!("/api/v1/call/invitations/{}/acceptance", invitation.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    server.connect(join);
    server.check().await;
    let call = server.own_call(&reader).await.call.unwrap();
    assert_eq!(
        call.call
            .participants
            .iter()
            .find(|p| p.account_id == owner.account_id)
            .unwrap()
            .avatar,
        Some(avatar)
    );
    // The image itself follows the rule of the profile: the reader may write to the owner,
    // who administers the server, and so sees the owner's profile (ADR 0119).
    assert_eq!(
        server
            .get(
                &reader,
                &format!("/api/v1/accounts/{}/avatar", owner.account_id),
            )
            .await
            .status,
        StatusCode::OK
    );
}

#[tokio::test]
async fn review2_audit_reads_have_an_account_limit() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    for _ in 0..120 {
        server
            .get(&owner, "/api/v1/audit-log")
            .await
            .ok::<junto_protocol::audit::AuditPage>(StatusCode::OK);
    }
    server
        .get(&owner, "/api/v1/audit-log")
        .await
        .error(ErrorCode::RateLimited);
    server.clock.advance(time::Duration::minutes(1));
    server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok::<junto_protocol::audit::AuditPage>(StatusCode::OK);
}

#[tokio::test]
async fn review2_self_redemption_uses_the_ordinary_membership_rule() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let invite = server.invite(&owner, json!({})).await;
    server
        .post(
            &owner,
            "/api/v1/invites/redemption",
            &json!({"invite_code":invite.code.expose()}),
        )
        .await
        .error(ErrorCode::Conflict);
}

#[tokio::test]
async fn review2_time_filtered_audit_paginates_across_a_backward_clock() {
    use junto_protocol::{audit::AuditPage, structure::Channel};
    use time::format_description::well_known::Rfc3339;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "time").await;
    let since = server.clock.now() - time::Duration::minutes(1);
    let mut expected = Vec::new();
    for (delta, name) in [(10, "later"), (-5, "earlier"), (0, "same time")] {
        server.clock.advance(time::Duration::seconds(delta));
        server
            .patch(
                &owner,
                &format!("/api/v1/channels/{}", channel.id),
                &json!({"name":name}),
            )
            .await
            .ok::<Channel>(StatusCode::OK);
    }
    let rows:Vec<(time::OffsetDateTime,junto_protocol::ids::AuditEntryId)>=sqlx::query_as("select occurred_at,id from audit_entries where channel_id=$1 and action='channel_updated' order by occurred_at desc,id desc").bind(channel.id).fetch_all(&server.db).await.unwrap();
    expected.extend(rows.into_iter().map(|(_, id)| id));
    let root = format!(
        "/api/v1/workspaces/{}/audit-log?action=channel_updated&since={}&limit=1",
        workspace.id,
        since.format(&Rfc3339).unwrap()
    );
    let mut actual = Vec::new();
    let mut route = root.clone();
    loop {
        let page: AuditPage = server.get(&owner, &route).await.ok(StatusCode::OK);
        actual.extend(page.entries.into_iter().map(|e| e.id));
        match page.next_before {
            Some(id) => route = format!("{root}&before={id}"),
            None => break,
        }
        assert!(actual.len() <= 3);
    }
    assert_eq!(actual, expected);
    let absent: AuditPage = server
        .get(&owner, &format!("{root}&before={}", uuid::Uuid::nil()))
        .await
        .ok(StatusCode::OK);
    assert!(absent.entries.is_empty());
}

#[tokio::test]
async fn review2_filtered_audit_with_a_token_and_deleted_workspace() {
    use junto_protocol::{access_token::TokenAction, audit::AuditPage};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Disposable"}))
        .await
        .ok(StatusCode::CREATED);
    let channel = server.channel(&owner, workspace.id, "visible").await;
    let token = server
        .access_token(&owner, &[TokenAction::ReadAuditLog])
        .await;
    let route = format!(
        "/api/v1/workspaces/{}/audit-log?action=channel_created&target_id={}",
        workspace.id, channel.id
    );
    let before: AuditPage = server
        .with_token(reqwest::Method::GET, &route, &token, None)
        .await
        .ok(StatusCode::OK);
    assert_eq!(before.entries.len(), 1);
    server
        .delete(&owner, &format!("/api/v1/workspaces/{}", workspace.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let after: AuditPage = server
        .with_token(reqwest::Method::GET, &route, &token, None)
        .await
        .ok(StatusCode::OK);
    assert_eq!(after.entries, before.entries);
}

#[tokio::test]
async fn review2_workspace_invite_withdrawal_and_removal_roll_back_together() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let creator = server.join(&owner, "Creator").await;
    server
        .grant_server(&owner, &creator, &[ServerPermission::CreateInvites])
        .await;
    let workspace = server.default_workspace(&owner).await;
    let invite = server.invite(&creator, json!({"max_uses":3})).await;
    sqlx::raw_sql("create function reject_invite_audit() returns trigger language plpgsql as $$ begin if new.action='invite_revoked' then raise exception 'forced invite audit failure'; end if; return new; end $$; create trigger reject_invite_audit before insert on audit_entries for each row execute function reject_invite_audit();").execute(&server.db).await.unwrap();
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, creator.account_id
            ),
            &Value::Null,
        )
        .await
        .error(ErrorCode::Internal);
    let member: bool = sqlx::query_scalar(
        "select exists(select 1 from workspace_members where workspace_id=$1 and account_id=$2)",
    )
    .bind(workspace.id)
    .bind(creator.account_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(member);
    let revoked: bool =
        sqlx::query_scalar("select revocation is not null from invites where id=$1")
            .bind(invite.invite.id)
            .fetch_one(&server.db)
            .await
            .unwrap();
    assert!(!revoked);
    let changes: i64 = sqlx::query_scalar(
        "select count(*) from events where kind in ('invite_revoked','workspace_member_removed')",
    )
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(changes, 0);
}

#[tokio::test]
async fn review2_concurrent_marks_and_channel_sends_report_retry_cost() {
    use crate::{
        discovery_support::{notices, settings},
        messages_support::{channel, send},
    };
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let mut readers = Vec::new();
    for n in 0..4 {
        let reader = server.join(&owner, &format!("Reader {n}")).await;
        settings(&server, &reader, uuid::Uuid::nil(), json!({"level":"all"})).await;
        readers.push(reader);
    }
    sqlx::raw_sql("create sequence notice_attempts; create function count_notice_attempt() returns trigger language plpgsql as $$ begin perform nextval('notice_attempts'); return new; end $$; create trigger count_notice_attempt before insert on notifications for each row execute function count_notice_attempt();").execute(&server.db).await.unwrap();
    let started = std::time::Instant::now();
    let sending = async {
        for _ in 0..40 {
            send(&server, &owner, &channel, "concurrent").await;
        }
    };
    let marking = futures_util::future::join_all(readers.iter().map(|reader| async {
        for _ in 0..20 {
            server
                .put(reader, "/api/v1/notifications/read", &json!({}))
                .await
                .ok::<junto_protocol::discovery::NotificationsRead>(StatusCode::OK);
        }
    }));
    tokio::join!(sending, marking);
    let attempts: i64 = sqlx::query_scalar("select last_value from notice_attempts")
        .fetch_one(&server.db)
        .await
        .unwrap();
    eprintln!(
        "notice contention: 40 sends, 80 marks, 160 committed notice inserts, {attempts} attempted inserts, elapsed {:?}",
        started.elapsed()
    );
    assert!(attempts >= 160);
    for reader in readers {
        let rows = notices(&server, &reader).await;
        assert_eq!(rows.len(), 40);
        assert!(rows.iter().all(|n| n.read_at.is_some()));
    }
}

/// ADR 0510 permits moderation audit identities to outside audit readers;
/// ADR 0604 withholds recording identities until the hidden call ends.
#[tokio::test]
async fn review2_hidden_call_moderation_and_recording_have_distinct_audiences() {
    use junto_protocol::{WorkspacePermission as W, audit::AuditPage};
    let studio = crate::recordings_support::Studio::hidden().await;
    let scene = &studio.scene;
    let (server, owner, ann, ben) = (&scene.server, &scene.owner, &scene.ann, &scene.ben);
    server
        .grant_workspace(
            owner,
            ann,
            scene.workspace_id,
            &[W::DisconnectMembers, W::ViewAuditEntries],
        )
        .await;
    let recording = studio.record().await;
    server
        .delete(
            ann,
            &format!(
                "/api/v1/channels/{}/call/participants/{}",
                scene.voice.id, ben.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let root = format!("/api/v1/workspaces/{}/audit-log", scene.workspace_id);
    let moderation = format!(
        "{root}?action=call_member_disconnected&target_id={}",
        ben.account_id
    );
    let recordings = format!("{root}?action=recording_started&target_id={}", recording.id);
    let inside: AuditPage = server.get(ann, &moderation).await.ok(StatusCode::OK);
    assert_eq!(inside.entries.len(), 1);
    let outside: AuditPage = server.get(owner, &moderation).await.ok(StatusCode::OK);
    assert_eq!(outside.entries, inside.entries);
    let inside: AuditPage = server.get(ann, &recordings).await.ok(StatusCode::OK);
    assert_eq!(inside.entries.len(), 1);
    let outside: AuditPage = server.get(owner, &recordings).await.ok(StatusCode::OK);
    assert!(outside.entries.is_empty());
    server.leave_call(ann).await;
    let ended: AuditPage = server.get(owner, &recordings).await.ok(StatusCode::OK);
    assert_eq!(ended.entries, inside.entries);
}

#[tokio::test]
async fn review2_workspace_invite_withdrawal_reads_indexed_creator_pages() {
    use crate::plan_support::{analyze, explained, index_names, visited_rows};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    sqlx::query("insert into invites(id,code_hash,created_by,created_at,expires_at,max_uses) select gen_random_uuid(),decode(md5(n::text)||md5(n::text),'hex'),$1,$2,$2+interval '1 day',1 from generate_series(1,10000) n").bind(owner.account_id).bind(server.clock.now()).execute(&server.db).await.unwrap();
    analyze(&server.db, &["invites", "invite_workspaces", "accounts"]).await;
    let plan: Value = sqlx::query_scalar(explained(
        &junto_server::routes::invites::withdrawal_statement(),
    ))
    .bind(owner.account_id)
    .bind(uuid::Uuid::nil())
    .bind(server.clock.now())
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(!index_names(&plan).is_empty(), "{plan}");
    assert!(visited_rows(&plan[0]["Plan"]) < 1000, "{plan}");
}

#[tokio::test]
async fn review2_a_connection_opened_during_a_watcher_read_gets_its_notice() {
    use futures_util::FutureExt;
    use junto_protocol::events::Level;
    use junto_server::database::Attempt;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = crate::messages_support::channel(&server, &owner, "text").await;
    let message = crate::messages_support::send(&server, &owner, &channel, "notice").await;
    let before = server.snapshot(&reader).await.cursor;
    let app = junto_server::state::AppState::new(
        server.database_with(|_| {}).await,
        server.state.config.clone(),
        server.state.clock.clone(),
        server.state.server_id,
        None,
        None,
    );
    let mut held = Vec::new();
    for _ in 0..app.config.limits.stream.pool().connections {
        held.push(
            app.database
                .acquire_for_stream(&Attempt::default())
                .await
                .unwrap(),
        );
    }
    let pending = junto_server::events::watcher::advance(
        &app,
        Some(junto_server::events::log::change_of(before.position)),
    );
    tokio::pin!(pending);
    assert!(pending.as_mut().now_or_never().is_none());
    let registration = app
        .streams
        .open(reader.account_id, reader.device_id)
        .unwrap();
    registration.link().set_level(Some(Level::Background));
    let mut tx = server.db.begin().await.unwrap();
    junto_server::messages::notifications::publish(
        &mut tx,
        server.clock.now(),
        message.id,
        &[(reader.account_id, true)],
        false,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    drop(held);
    pending.await.unwrap();
    assert!(registration.link().news().now_or_never().is_some());
}
