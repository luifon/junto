use crate::discovery_support::*;

#[tokio::test]
async fn rule_51_direct_retention_commits_each_bounded_batch() {
    let server = TestServer::start_with(|config| {
        config.limits.maintenance_batch = 1;
    })
    .await;
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
    let route = format!("/api/v1/conversations/{}/messages", conversation.id);
    for _ in 0..3 {
        server
            .post(&owner, &route, &text("quartz expired"))
            .await
            .ok::<Message>(StatusCode::CREATED);
    }
    server.clock.advance(time::Duration::days(2));
    server
        .put(
            &owner,
            "/api/v1/direct-message-retention",
            &json!({"days":1}),
        )
        .await
        .ok::<Retention>(StatusCode::OK);
    // Skip the empty channel-file transaction and the first direct-message batch.
    server.gate().arm(2);
    let run = junto_server::messages::retention::run(&server.state);
    tokio::pin!(run);
    tokio::select! {
        biased;
        () = server.gate().arrived() => {}
        result = &mut run => panic!("retention finished without bounded batches: {result:?}"),
    }
    assert_eq!(search(&server, &reader, "quartz").await.results.len(), 2);
    assert_eq!(notices(&server, &reader).await.len(), 2);
    server.gate().release();
    run.await.unwrap();
    assert!(search(&server, &reader, "quartz").await.results.is_empty());
    assert!(notices(&server, &reader).await.is_empty());
}

#[tokio::test]
async fn rule_47_48_workspace_retention_defaults_never_and_skips_permanent_files() {
    let files = Files::open().await;
    let server = &files.server;
    let owner = &files.owner;
    let first = files.upload(b"ordinary").await;
    let ordinary = files.attach(&[first.id]).await;
    server
        .patch(
            owner,
            &format!("/api/v1/channels/{}", files.channel.id),
            &json!({"permanent_messages":true}),
        )
        .await
        .ok::<Channel>(StatusCode::OK);
    let second = files.upload(b"permanent").await;
    let permanent = files.attach(&[second.id]).await;
    let route = format!(
        "/api/v1/workspaces/{}/file-retention",
        files.channel.workspace_id
    );
    assert_eq!(
        server
            .get(owner, &route)
            .await
            .ok::<Retention>(StatusCode::OK)
            .days,
        None
    );
    server.clock.advance(time::Duration::days(2));
    junto_server::maintenance::run_once(&server.state)
        .await
        .unwrap();
    assert_eq!(
        server
            .get(owner, &format!("/api/v1/attachments/{}/file", first.id))
            .await
            .bytes(),
        b"ordinary"
    );
    server
        .put(owner, &route, &json!({"days":1}))
        .await
        .ok::<Retention>(StatusCode::OK);
    junto_server::messages::retention::run(&server.state)
        .await
        .unwrap();
    server
        .get(owner, &format!("/api/v1/attachments/{}/file", first.id))
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(
        server
            .get(owner, &format!("/api/v1/attachments/{}/file", second.id))
            .await
            .bytes(),
        b"permanent"
    );
    assert!(
        !server
            .get(owner, &message(ordinary.id))
            .await
            .ok::<Message>(StatusCode::OK)
            .deleted
    );
    assert!(
        server
            .get(owner, &message(permanent.id))
            .await
            .ok::<Message>(StatusCode::OK)
            .permanent
    );
}

#[tokio::test]
async fn rule_49_51_direct_retention_removes_due_text_files_and_notices_in_batches() {
    let files = Files::open().await;
    let server = &files.server;
    let owner = &files.owner;
    let reader = server.join(owner, "Reader").await;
    let conversation: Conversation = server
        .post(
            owner,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let route = format!("/api/v1/conversations/{}/messages", conversation.id);
    assert_eq!(
        server
            .get(owner, "/api/v1/direct-message-retention")
            .await
            .ok::<Retention>(StatusCode::OK)
            .days,
        None
    );
    let attachment = upload(
        server,
        owner,
        json!({"conversation_id":conversation.id}),
        b"expired",
    )
    .await;
    let mut body = text("quartz old");
    body["attachments"] = json!([attachment.id]);
    let old: Message = server
        .post(owner, &route, &body)
        .await
        .ok(StatusCode::CREATED);
    server.clock.advance(time::Duration::days(2));
    let mut body = text("quartz new");
    body["reply_to"] = json!(old.id);
    let new: Message = server
        .post(owner, &route, &body)
        .await
        .ok(StatusCode::CREATED);
    server
        .put(
            owner,
            "/api/v1/direct-message-retention",
            &json!({"days":1}),
        )
        .await
        .ok::<Retention>(StatusCode::OK);
    junto_server::messages::retention::run(&server.state)
        .await
        .unwrap();
    junto_server::messages::retention::run(&server.state)
        .await
        .unwrap();
    let marker: Message = server
        .get(&reader, &message(old.id))
        .await
        .ok(StatusCode::OK);
    assert!(marker.deleted);
    assert!(marker.document.is_none());
    assert_eq!(
        server
            .get(&reader, &message(new.id))
            .await
            .ok::<Message>(StatusCode::OK)
            .reply_to,
        Some(old.id)
    );
    server
        .get(
            &reader,
            &format!("/api/v1/attachments/{}/file", attachment.id),
        )
        .await
        .error(ErrorCode::NotFound);
    assert_eq!(notices(server, &reader).await.len(), 1);
    assert_eq!(search(server, &reader, "quartz").await.results.len(), 1);
}

#[tokio::test]
async fn rule_50_retention_permissions_bounds_persistence_audit_and_events() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    for route in [
        format!("/api/v1/workspaces/{}/file-retention", workspace.id),
        format!("/api/v1/workspaces/{}/recording-retention", workspace.id),
        "/api/v1/direct-message-retention".into(),
    ] {
        server
            .get(&reader, &route)
            .await
            .error(ErrorCode::Forbidden);
        server
            .put(&reader, &route, &json!({"days":1}))
            .await
            .error(ErrorCode::Forbidden);
        for days in [0, 36501] {
            server
                .put(&owner, &route, &json!({"days":days}))
                .await
                .error(ErrorCode::InvalidRequest);
        }
        let cursor = server.snapshot(&owner).await.cursor;
        server
            .put(&owner, &route, &json!({"days":30}))
            .await
            .ok::<Retention>(StatusCode::OK);
        assert_eq!(
            server
                .get(&owner, &route)
                .await
                .ok::<Retention>(StatusCode::OK)
                .days,
            Some(30)
        );
        assert!(
            events(&server, &owner, cursor, Level::Full)
                .await
                .iter()
                .any(|event| matches!(event, Event::RetentionChanged { .. }))
        );
    }
    let log: Value = server
        .get(&owner, "/api/v1/audit-log")
        .await
        .ok(StatusCode::OK);
    assert!(log.to_string().contains("retention_changed"));
    let workspace_log: Value = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/audit-log", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(
        workspace_log
            .to_string()
            .contains("recording_retention_changed")
    );
}

#[tokio::test]
async fn fix_retained_attachment_removal_is_visible_on_the_surviving_message() {
    let files = Files::open().await;
    let attachment = files.upload(b"gone").await;
    let sent = files.attach(&[attachment.id]).await;
    files
        .server
        .put(
            &files.owner,
            &format!(
                "/api/v1/workspaces/{}/file-retention",
                files.channel.workspace_id
            ),
            &json!({"days":1}),
        )
        .await
        .ok::<Retention>(StatusCode::OK);
    files.server.clock.advance(time::Duration::days(2));
    junto_server::messages::retention::run(&files.server.state)
        .await
        .unwrap();
    let shown: Value = files
        .server
        .get(&files.owner, &message(sent.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(shown["deleted"], false);
    assert_eq!(shown["files_removed_by_retention"], true);
}

#[tokio::test]
async fn fix_retention_and_notification_deletion_have_leading_indexes() {
    let server = TestServer::start().await;
    let indexes: Vec<String> =
        sqlx::query_scalar("select indexdef from pg_indexes where schemaname=current_schema()")
            .fetch_all(&server.db)
            .await
            .unwrap();
    for clause in [
        "notifications USING btree (message_id)",
        "notifications USING btree (account_id, created_at DESC, id DESC)",
        "attachments USING btree (place_id, retention_at, id)",
        "messages USING btree (place_id, created_at, id)",
    ] {
        assert!(
            indexes.iter().any(|index| index.contains(clause)),
            "missing indexed access path: {clause}"
        );
    }
    let mut connection = server.db.acquire().await.unwrap();
    sqlx::query("set enable_seqscan=off")
        .execute(&mut *connection)
        .await
        .unwrap();
    fn uses_age_index(plan: &Value, index: &str, age: &str) -> bool {
        (plan["Index Name"].as_str() == Some(index)
            && plan["Index Cond"]
                .as_str()
                .is_some_and(|condition| condition.contains(age)))
            || plan["Plans"].as_array().is_some_and(|children| {
                children
                    .iter()
                    .any(|child| uses_age_index(child, index, age))
            })
    }
    for (query, index, age) in [
        (
            junto_server::messages::retention::FILE_BATCH_QUERY,
            "attachments_retention",
            "retention_at",
        ),
        (
            junto_server::messages::retention::DIRECT_BATCH_QUERY,
            "messages_retention",
            "created_at",
        ),
        (
            junto_server::messages::retention::RECORDING_BATCH_QUERY,
            "recordings_retention",
            "requested_at",
        ),
    ] {
        let plan: Value =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("explain(format json) {query}")))
                .bind(server.clock.now())
                .bind(17_i64)
                .bind(uuid::Uuid::nil())
                .bind(1_i32)
                .fetch_one(&mut *connection)
                .await
                .unwrap();
        assert!(
            uses_age_index(&plan[0]["Plan"], index, age),
            "server retention statement must bound age through {index}: {plan}"
        );
    }
    let plan: Value = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "explain(format json) {}",
        junto_server::messages::retention::NOTICE_CAP_QUERY
    )))
    .bind(uuid::Uuid::nil())
    .bind(17_i64)
    .fetch_one(&mut *connection)
    .await
    .unwrap();
    assert!(
        plan.to_string().contains("notifications_account"),
        "notice trim must use ordered account index: {plan}"
    );
}

#[tokio::test]
async fn fix_workspace_recording_retention_uses_recording_removal() {
    use crate::recordings_support::{FakeWorker, Studio};
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let server = &studio.scene.server;
    let owner = &studio.scene.owner;
    let done = studio.finished_after(time::Duration::seconds(3)).await;
    let route = format!(
        "/api/v1/workspaces/{}/recording-retention",
        studio.scene.workspace_id
    );
    assert_eq!(
        server
            .get(owner, &route)
            .await
            .ok::<Retention>(StatusCode::OK)
            .days,
        None
    );
    server
        .put(owner, &route, &json!({"days":1}))
        .await
        .ok::<Retention>(StatusCode::OK);
    server.clock.advance(time::Duration::days(2));
    junto_server::messages::retention::run(&server.state)
        .await
        .unwrap();
    server
        .get(owner, &format!("/api/v1/recordings/{}", done.recording.id))
        .await
        .error(ErrorCode::NotFound);
    let queued: i64 = sqlx::query_scalar("select count(*) from file_removals")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(queued > 0);
}

#[tokio::test]
async fn fix_direct_retention_events_reach_only_administrators() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(
            &owner,
            &administrator,
            &[junto_protocol::ServerPermission::Administrator],
        )
        .await;
    let cursor = server.snapshot(&reader).await.cursor;
    server
        .put(
            &owner,
            "/api/v1/direct-message-retention",
            &json!({"days":30}),
        )
        .await
        .ok::<Retention>(StatusCode::OK);
    assert!(
        !events(&server, &reader, cursor, Level::Full)
            .await
            .iter()
            .any(|e| matches!(e, Event::RetentionChanged { workspace_id: None }))
    );
    assert!(
        events(&server, &administrator, cursor, Level::Full)
            .await
            .iter()
            .any(|e| matches!(e, Event::RetentionChanged { workspace_id: None }))
    );
}

#[tokio::test]
async fn round2_partly_failed_recordings_expire() {
    use crate::recordings_support::{FakeWorker, Studio};
    let worker = FakeWorker::new();
    let studio = Studio::on(worker.server().await).await;
    let server = &studio.scene.server;
    let owner = &studio.scene.owner;
    let done = studio.finished_after(time::Duration::seconds(3)).await;
    sqlx::query("update recordings set state='partly_failed' where id=$1")
        .bind(done.recording.id)
        .execute(&server.db)
        .await
        .unwrap();
    let route = format!(
        "/api/v1/workspaces/{}/recording-retention",
        studio.scene.workspace_id
    );
    assert_eq!(
        server
            .get(owner, &route)
            .await
            .ok::<Retention>(StatusCode::OK)
            .days,
        None
    );
    server
        .put(owner, &route, &json!({"days":1}))
        .await
        .ok::<Retention>(StatusCode::OK);
    server.clock.advance(time::Duration::days(2));
    junto_server::messages::retention::run(&server.state)
        .await
        .unwrap();
    server
        .get(owner, &format!("/api/v1/recordings/{}", done.recording.id))
        .await
        .error(ErrorCode::NotFound);
    let queued: i64 = sqlx::query_scalar("select count(*) from file_removals")
        .fetch_one(&server.db)
        .await
        .unwrap();
    assert!(queued > 0);
}

#[tokio::test]
async fn round2_file_retention_commits_bounded_batches() {
    let files = Files::open().await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(files.upload(b"file").await.id);
    }
    files.attach(&ids).await;
    files
        .server
        .put(
            &files.owner,
            &format!(
                "/api/v1/workspaces/{}/file-retention",
                files.channel.workspace_id
            ),
            &json!({"days":1}),
        )
        .await
        .ok::<Retention>(StatusCode::OK);
    files.server.clock.advance(time::Duration::days(2));
    let strict = files
        .server
        .strict(|c| c.limits.maintenance_batch = 1)
        .await;
    strict.gate().arm(1);
    let run = junto_server::messages::retention::run(&strict.state);
    tokio::pin!(run);
    tokio::select! {biased; ()=strict.gate().arrived()=>{} result=&mut run=>panic!("no second file batch: {result:?}")}
    let removed: i64 = sqlx::query_scalar(
        "select count(*) from attachments where removed_at is not null and id=any($1)",
    )
    .bind(&ids)
    .fetch_one(&strict.db)
    .await
    .unwrap();
    assert_eq!(removed, 1);
    strict.gate().release();
    run.await.unwrap();
    for id in ids {
        files
            .server
            .get(&files.owner, &format!("/api/v1/attachments/{id}/file"))
            .await
            .error(ErrorCode::NotFound);
    }
}

#[tokio::test]
async fn round2_recording_retention_commits_bounded_batches() {
    use crate::recordings_support::Studio;
    let studio = Studio::open().await;
    let server = &studio.scene.server;
    for _ in 0..3 {
        studio.finished_after(time::Duration::seconds(3)).await;
    }
    server
        .put(
            &studio.scene.owner,
            &format!(
                "/api/v1/workspaces/{}/recording-retention",
                studio.scene.workspace_id
            ),
            &json!({"days":1}),
        )
        .await
        .ok::<Retention>(StatusCode::OK);
    server.clock.advance(time::Duration::days(2));
    let strict = server.strict(|c| c.limits.maintenance_batch = 1).await;
    strict.gate().arm(3);
    let run = junto_server::messages::retention::run(&strict.state);
    tokio::pin!(run);
    tokio::select! {biased; ()=strict.gate().arrived()=>{} result=&mut run=>panic!("no second recording batch: {result:?}")}
    assert_eq!(server.count("recordings").await, 2);
    strict.gate().release();
    run.await.unwrap();
    assert_eq!(server.count("recordings").await, 0);
}

#[tokio::test]
async fn round2_retention_visits_one_place_per_batch() {
    let files = Files::open().await;
    let first = files.upload(b"one").await;
    files.attach(&[first.id]).await;
    let other = channel(&files.server, &files.owner, "text").await;
    let second = upload(
        &files.server,
        &files.owner,
        json!({"channel_id":other.id}),
        b"two",
    )
    .await;
    files
        .server
        .post(
            &files.owner,
            &format!("/api/v1/channels/{}/messages", other.id),
            &json!({"document":{"blocks":[]},"attachments":[second.id]}),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    files
        .server
        .put(
            &files.owner,
            &format!("/api/v1/workspaces/{}/file-retention", other.workspace_id),
            &json!({"days":1}),
        )
        .await
        .ok::<Retention>(StatusCode::OK);
    files.server.clock.advance(time::Duration::days(2));
    let strict = files
        .server
        .strict(|c| c.limits.maintenance_batch = 2)
        .await;
    strict.gate().arm(1);
    let run = junto_server::messages::retention::run(&strict.state);
    tokio::pin!(run);
    tokio::select! {biased; ()=strict.gate().arrived()=>{} result=&mut run=>panic!("no per-place continuation: {result:?}")}
    let places: Vec<ChannelId> =
        sqlx::query_scalar("select place_id from attachments where removed_at is not null")
            .fetch_all(&strict.db)
            .await
            .unwrap();
    assert_eq!(places, vec![files.channel.id]);
    strict.gate().release();
    run.await.unwrap();
    let removed: Vec<ChannelId> = sqlx::query_scalar(
        "select place_id from attachments where removed_at is not null order by place_id",
    )
    .fetch_all(&strict.db)
    .await
    .unwrap();
    assert_eq!(removed, vec![files.channel.id, other.id]);
}

#[tokio::test]
async fn round3_retention_continuation_does_not_revisit_earlier_places() {
    let files = Files::open().await;
    let item = files.upload(b"backlog").await;
    files.attach(&[item.id]).await;
    let server = &files.server;
    sqlx::query("insert into attachments(id,storage_id,place_id,owner_id,message_id,name,bytes,received,sha256,complete,expires_at,touched_at,attached,staging_cleaned,retention_at) select gen_random_uuid(),storage_id,place_id,owner_id,message_id,name,bytes,received,sha256,complete,expires_at,touched_at,attached,staging_cleaned,retention_at from attachments cross join generate_series(1,5) where id=$1")
        .bind(item.id).execute(&server.db).await.unwrap();
    server
        .put(
            &files.owner,
            &format!(
                "/api/v1/workspaces/{}/file-retention",
                files.channel.workspace_id
            ),
            &json!({"days":1}),
        )
        .await
        .ok::<Retention>(StatusCode::OK);
    server.clock.advance(time::Duration::days(2));
    let strict = server.strict(|c| c.limits.maintenance_batch = 2).await;
    sqlx::raw_sql("create table round3_probes(id uuid);
        create function round3_probe(id uuid) returns boolean language plpgsql volatile as $$
        begin insert into round3_probes values(id); return true; end $$;
        alter table channels rename to round3_channels;
        create view channels with(security_barrier=true) as select * from round3_channels where round3_probe(id)")
        .execute(&strict.db).await.unwrap();
    junto_server::messages::retention::run(&strict.state)
        .await
        .unwrap();
    let repeated: Vec<(uuid::Uuid, i64)> = sqlx::query_as(
        "select id,count(*) from round3_probes where id<$1 group by id having count(*)>1",
    )
    .bind(files.channel.id)
    .fetch_all(&strict.db)
    .await
    .unwrap();
    let probed: i64 = sqlx::query_scalar("select count(*) from round3_probes where id<$1")
        .bind(files.channel.id)
        .fetch_one(&strict.db)
        .await
        .unwrap();
    assert!(probed > 0, "the fixture must include earlier empty places");
    assert!(
        repeated.is_empty(),
        "earlier places were revisited: {repeated:?}"
    );
    let removed: i64 = sqlx::query_scalar(
        "select count(*) from attachments where place_id=$1 and removed_at is not null",
    )
    .bind(files.channel.id)
    .fetch_one(&strict.db)
    .await
    .unwrap();
    assert_eq!(removed, 6);
}
