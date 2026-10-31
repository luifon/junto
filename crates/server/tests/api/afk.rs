use junto_protocol::error::ErrorCode;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::calls_support::Scene;

#[tokio::test]
async fn a_workspace_has_an_afk_channel_and_only_a_manager_changes_its_settings() {
    let scene = Scene::start().await;
    let server = &scene.server;
    let path = format!("/api/v1/workspaces/{}/afk", scene.workspace_id);
    let settings: Value = server.get(&scene.ann, &path).await.ok(StatusCode::OK);
    assert_eq!(settings["enabled"], true);
    assert_eq!(settings["idle_minutes"], 15);
    assert!(settings["channel_id"].is_string());
    let body = json!({"channel_id": scene.voice.id, "enabled": true, "idle_minutes": 5});
    server
        .put(&scene.ann, &path, &body)
        .await
        .error(ErrorCode::Forbidden);
    let updated: Value = server
        .put(&scene.owner, &path, &body)
        .await
        .ok(StatusCode::OK);
    assert_eq!(updated, body);
    sqlx::query("insert into workspace_person_overrides(workspace_id,account_id,allow,deny) values($1,$2,$3,0)")
        .bind(scene.workspace_id).bind(scene.ann.account_id)
        .bind(1_i64 << junto_protocol::WorkspacePermission::ManageChannel as u8)
        .execute(&server.db).await.unwrap();
    assert_eq!(
        server
            .put(&scene.ann, &path, &body)
            .await
            .ok::<Value>(StatusCode::OK),
        body
    );
    let invalid = json!({"channel_id": scene.voice.id, "enabled": true, "idle_minutes": 7});
    server
        .put(&scene.owner, &path, &invalid)
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn an_idle_move_rechecks_the_seat_and_destination_and_refuses_every_publishing_source() {
    use crate::calls_support::claims;
    use junto_protocol::call::{CallJoin, CallMove, MoveOutcome};
    use junto_protocol::events::Event;
    use junto_protocol::ids::ChannelId;
    let scene = Scene::start().await;
    let server = &scene.server;
    let path = format!("/api/v1/workspaces/{}/afk", scene.workspace_id);
    let settings: Value = server.get(&scene.ann, &path).await.ok(StatusCode::OK);
    let afk: ChannelId = serde_json::from_value(settings["channel_id"].clone()).unwrap();
    let source = server.enter_call(&scene.ann, scene.voice.id).await;
    let mut stream = server.full_stream(&scene.ann).await;
    let mut idle = json!({"call_id":source.join.call.id, "identity":source.identity,
        "system_idle_seconds":900, "microphone_silent_seconds":900});
    idle["system_idle_seconds"] = json!(899);
    server
        .post(&scene.ann, "/api/v1/call/idle", &idle)
        .await
        .error(ErrorCode::Conflict);
    idle["system_idle_seconds"] = json!(900);
    idle["microphone_silent_seconds"] = json!(899);
    server
        .post(&scene.ann, "/api/v1/call/idle", &idle)
        .await
        .error(ErrorCode::Conflict);
    idle["microphone_silent_seconds"] = json!(900);
    idle["identity"] = json!("an-earlier-seat");
    server
        .post(&scene.ann, "/api/v1/call/idle", &idle)
        .await
        .error(ErrorCode::NotInCall);
    idle["identity"] = json!(source.identity);
    let moved: CallMove = server
        .post(&scene.ann, "/api/v1/call/idle", &idle)
        .await
        .ok(StatusCode::OK);
    assert_eq!(moved.to.channel_id(), Some(afk));
    let join: CallJoin = server
        .post(
            &scene.ann,
            &format!("/api/v1/call/moves/{}/join", moved.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    assert!(join.listen_only);
    assert_eq!(claims(&join)["video"]["canPublish"], false);
    let destination = server.connect(join);
    for (kind, source) in [
        (
            junto_server::media::TrackKind::Audio,
            junto_server::media::TrackSource::Microphone,
        ),
        (
            junto_server::media::TrackKind::Video,
            junto_server::media::TrackSource::Camera,
        ),
        (
            junto_server::media::TrackKind::Video,
            junto_server::media::TrackSource::ScreenShare,
        ),
        (
            junto_server::media::TrackKind::Audio,
            junto_server::media::TrackSource::ScreenShareAudio,
        ),
    ] {
        assert!(
            !server
                .fake()
                .publish(&destination.room, &destination.identity, kind, source)
        );
    }
    server.check().await;
    assert_eq!(
        server
            .own_call(&scene.ann)
            .await
            .call
            .unwrap()
            .call
            .place
            .channel_id(),
        Some(afk)
    );
    let events = server.call_events(&scene.ann, &mut stream).await;
    assert!(events.iter().any(|e| matches!(e, Event::CallMoveResolved { move_id, outcome: MoveOutcome::Completed } if *move_id == moved.id)));
    server
        .put(
            &scene.ann,
            "/api/v1/call/state",
            &json!({"muted":false,"deafened":false}),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .put(
            &scene.ann,
            "/api/v1/call/screen-share",
            &json!({"audio":"none"}),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .put(
            &scene.ann,
            "/api/v1/call/speaking",
            &json!({"speaking":true}),
        )
        .await
        .error(ErrorCode::Forbidden);
    let idle = json!({"call_id":destination.join.call.id,"identity":destination.identity,"system_idle_seconds":900,"microphone_silent_seconds":900});
    server
        .post(&scene.ann, "/api/v1/call/idle", &idle)
        .await
        .error(ErrorCode::Conflict);
    let audit: junto_protocol::audit::AuditPage = server
        .get(
            &scene.owner,
            &format!(
                "/api/v1/workspaces/{}/audit-log?limit=200",
                scene.workspace_id
            ),
        )
        .await
        .ok(StatusCode::OK);
    assert!(audit.entries.iter().any(|entry| matches!(
        entry.event,
        junto_protocol::audit::AuditEvent::CallMemberMovedOut { .. }
    )));
}

#[tokio::test]
async fn disabling_afk_cancels_a_pending_move_and_old_clients_stay_in_their_call() {
    use junto_protocol::call::{CallJoin, CallMove};
    let scene = Scene::start().await;
    let server = &scene.server;
    let path = format!("/api/v1/workspaces/{}/afk", scene.workspace_id);
    let mut settings: Value = server.get(&scene.owner, &path).await.ok(StatusCode::OK);
    let source = server.enter_call(&scene.ann, scene.voice.id).await;
    let idle = json!({"call_id":source.join.call.id,"identity":source.identity,"system_idle_seconds":900,"microphone_silent_seconds":900});
    let moved: CallMove = server
        .post(&scene.ann, "/api/v1/call/idle", &idle)
        .await
        .ok(StatusCode::OK);
    let join: CallJoin = server
        .post(
            &scene.ann,
            &format!("/api/v1/call/moves/{}/join", moved.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    settings["enabled"] = json!(false);
    server
        .put(&scene.owner, &path, &settings)
        .await
        .ok::<Value>(StatusCode::OK);
    server.connect(join);
    server.check().await;
    assert_eq!(
        server.own_call(&scene.ann).await.call.unwrap().call.id,
        source.join.call.id
    );
    server
        .post(&scene.ann, "/api/v1/call/idle", &idle)
        .await
        .error(ErrorCode::Conflict);
    settings["enabled"] = json!(true);
    server
        .put(&scene.owner, &path, &settings)
        .await
        .ok::<Value>(StatusCode::OK);
    server.clock.advance(time::Duration::hours(2));
    server.check().await;
    assert_eq!(
        server.own_call(&scene.ann).await.call.unwrap().call.id,
        source.join.call.id
    );
}

#[tokio::test]
async fn meeting_direct_group_calls_and_inaccessible_destinations_are_exempt() {
    let scene = Scene::start().await;
    let server = &scene.server;
    let path = format!("/api/v1/workspaces/{}/afk", scene.workspace_id);
    let settings: Value = server.get(&scene.owner, &path).await.ok(StatusCode::OK);
    let afk: junto_protocol::ids::ChannelId =
        serde_json::from_value(settings["channel_id"].clone()).unwrap();
    let source = server.enter_call(&scene.ann, scene.voice.id).await;
    let idle = json!({"call_id":source.join.call.id,"identity":source.identity,"system_idle_seconds":3600,"microphone_silent_seconds":3600});
    sqlx::query("update calls set meeting_presenter=$2, meeting_started_at=$3 where id=$1")
        .bind(source.join.call.id)
        .bind(scene.ann.account_id)
        .bind(server.clock.now())
        .execute(&server.db)
        .await
        .unwrap();
    server
        .post(&scene.ann, "/api/v1/call/idle", &idle)
        .await
        .error(ErrorCode::Conflict);
    sqlx::query("update calls set meeting_presenter=null, meeting_started_at=null where id=$1")
        .bind(source.join.call.id)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("insert into channel_person_overrides(channel_id,workspace_id,account_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(afk).bind(scene.workspace_id).bind(scene.ann.account_id)
        .bind(1_i64 << junto_protocol::WorkspacePermission::Connect as u8)
        .execute(&server.db).await.unwrap();
    let refused = server.post(&scene.ann, "/api/v1/call/idle", &idle).await;
    assert!(matches!(
        refused.code(),
        ErrorCode::Forbidden | ErrorCode::Conflict | ErrorCode::NotFound
    ));
    server.leave_call(&scene.ann).await;
    let direct = server.enter_direct(&scene.ann, &scene.ben).await;
    let idle = json!({"call_id":direct.join.call.id,"identity":direct.identity,"system_idle_seconds":3600,"microphone_silent_seconds":3600});
    server
        .post(&scene.ann, "/api/v1/call/idle", &idle)
        .await
        .error(ErrorCode::Conflict);
    server.leave_call(&scene.ann).await;
    let group: Value = server
        .post(&scene.ann, "/api/v1/group-conversations", &json!({"name":"Planning", "account_ids":[scene.ben.account_id, scene.owner.account_id]}))
        .await.ok(StatusCode::CREATED);
    let join: junto_protocol::call::CallJoin = server
        .post(
            &scene.ann,
            &format!(
                "/api/v1/group-conversations/{}/call/join",
                group["id"].as_str().unwrap()
            ),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK);
    server.connect(join.clone());
    server.check().await;
    let idle = json!({"call_id":join.call.id,"identity":join.identity,"system_idle_seconds":3600,"microphone_silent_seconds":3600});
    server
        .post(&scene.ann, "/api/v1/call/idle", &idle)
        .await
        .error(ErrorCode::Conflict);
}

#[tokio::test]
async fn a_new_workspace_gets_a_renameable_afk_channel_and_rejects_foreign_channels() {
    let scene = Scene::start().await;
    let server = &scene.server;
    let workspace: junto_protocol::structure::Workspace = server
        .post(
            &scene.owner,
            "/api/v1/workspaces",
            &json!({"name":"Second workspace"}),
        )
        .await
        .ok(StatusCode::CREATED);
    let path = format!("/api/v1/workspaces/{}/afk", workspace.id);
    let mut afk: Value = server.get(&scene.owner, &path).await.ok(StatusCode::OK);
    let id = afk["channel_id"].as_str().unwrap();
    server
        .patch(
            &scene.owner,
            &format!("/api/v1/channels/{id}"),
            &json!({"name":"Away"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .delete(&scene.owner, &format!("/api/v1/channels/{id}"))
        .await
        .error(ErrorCode::Conflict);
    server
        .put(
            &scene.owner,
            &format!("/api/v1/channels/{id}/archive"),
            &Value::Null,
        )
        .await
        .error(ErrorCode::Conflict);
    afk["channel_id"] = json!(scene.voice.id);
    server
        .put(&scene.owner, &path, &afk)
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn migration_0056_adds_an_afk_channel_to_each_existing_workspace() {
    use sqlx::ConnectOptions as _;
    let database = crate::support::empty_database().await;
    let mut connection = database.options.connect().await.unwrap();
    crate::migration_identifiers::migrations_up_to(55)
        .run(&mut connection)
        .await
        .unwrap();
    for name in ["First workspace", "Second workspace"] {
        sqlx::query("insert into workspaces (id,name,created_at) values($1,$2,now())")
            .bind(uuid::Uuid::now_v7())
            .bind(name)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    crate::migration_identifiers::migrations_up_to(56)
        .run(&mut connection)
        .await
        .unwrap();
    let rows: Vec<(String, String, bool, i32)> = sqlx::query_as("select w.name,c.name,a.enabled,a.idle_minutes from workspaces w join workspace_afk a on a.workspace_id=w.id join channels c on c.id=a.channel_id and c.workspace_id=w.id and c.kind='voice' order by w.name")
        .fetch_all(&mut connection).await.unwrap();
    assert_eq!(
        rows,
        vec![
            ("First workspace".into(), "AFK".into(), true, 15),
            ("Second workspace".into(), "AFK".into(), true, 15)
        ]
    );
}

#[tokio::test]
async fn afk_settings_do_not_disclose_a_hidden_destination() {
    let scene = Scene::start().await;
    let server = &scene.server;
    let path = format!("/api/v1/workspaces/{}/afk", scene.workspace_id);
    let settings: Value = server.get(&scene.owner, &path).await.ok(StatusCode::OK);
    let afk: junto_protocol::ids::ChannelId =
        serde_json::from_value(settings["channel_id"].clone()).unwrap();
    sqlx::query("insert into channel_person_overrides(channel_id,workspace_id,account_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(afk).bind(scene.workspace_id).bind(scene.ann.account_id)
        .bind(1_i64 << junto_protocol::WorkspacePermission::ViewChannel as u8)
        .execute(&server.db).await.unwrap();
    server
        .get(&scene.ann, &path)
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn raising_the_idle_timeout_cancels_a_move_that_has_not_completed() {
    use junto_protocol::call::{CallJoin, CallMove};
    let scene = Scene::start().await;
    let server = &scene.server;
    let path = format!("/api/v1/workspaces/{}/afk", scene.workspace_id);
    let mut settings: Value = server.get(&scene.owner, &path).await.ok(StatusCode::OK);
    settings["idle_minutes"] = json!(5);
    server
        .put(&scene.owner, &path, &settings)
        .await
        .ok::<Value>(StatusCode::OK);
    let source = server.enter_call(&scene.ann, scene.voice.id).await;
    let idle = json!({"call_id":source.join.call.id,"identity":source.identity,"system_idle_seconds":300,"microphone_silent_seconds":300});
    let moved: CallMove = server
        .post(&scene.ann, "/api/v1/call/idle", &idle)
        .await
        .ok(StatusCode::OK);
    let join: CallJoin = server
        .post(
            &scene.ann,
            &format!("/api/v1/call/moves/{}/join", moved.id),
            &Value::Null,
        )
        .await
        .ok(StatusCode::OK);
    settings["idle_minutes"] = json!(60);
    server
        .put(&scene.owner, &path, &settings)
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .post(
            &scene.ann,
            &format!("/api/v1/call/moves/{}/join", moved.id),
            &Value::Null,
        )
        .await
        .error(ErrorCode::Conflict);
    server.connect(join);
    server.check().await;
    assert_eq!(
        server.own_call(&scene.ann).await.call.unwrap().call.id,
        source.join.call.id
    );
}

#[tokio::test]
async fn migration_0056_requires_a_snapshot_when_a_pre_upgrade_client_reconnects() {
    use crate::stream_support::Frame;
    use crate::support::TestServer;
    use junto_protocol::events::{ClientMessage, Level, ResyncReason, ServerMessage};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    // Restore the populated schema at 0055, before the migration creates any channels.
    sqlx::raw_sql(
        "with old as (delete from workspace_afk returning channel_id)
        delete from channels where id in (select channel_id from old)",
    )
    .execute(&server.db)
    .await
    .unwrap();
    let before = server.snapshot(&owner).await;
    assert!(before.workspaces.iter().all(|workspace| {
        workspace
            .channels
            .iter()
            .all(|channel| channel.name != "AFK")
    }));
    sqlx::raw_sql(
        "drop table workspace_afk;
        alter table call_moves drop constraint call_move_afk_duration;
        alter table call_moves drop column automatic_afk, drop column afk_idle_seconds;
        delete from _sqlx_migrations where version=56;",
    )
    .execute(&server.db)
    .await
    .unwrap();
    let upgraded = server.restart().await;
    let mut stream = upgraded.stream(&owner).await;
    stream
        .send(&ClientMessage::Subscribe {
            presence: false,
            appearance: false,
            level: Level::Full,
            cursor: Some(before.cursor),
        })
        .await;
    assert_eq!(
        stream.next().await,
        Frame::Message(ServerMessage::Resync {
            reason: ResyncReason::UnknownCursor
        })
    );
    let after = upgraded.snapshot(&owner).await;
    assert_ne!(before.cursor.epoch, after.cursor.epoch);
    assert!(after.workspaces.iter().all(|workspace| {
        workspace
            .channels
            .iter()
            .any(|channel| channel.name == "AFK")
    }));
    assert_eq!(
        stream.subscribe(Level::Full, Some(after.cursor)).await,
        after.cursor
    );
}
