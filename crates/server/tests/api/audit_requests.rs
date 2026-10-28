use junto_protocol::audit::AuditPage;
use junto_protocol::error::ErrorCode;
use junto_protocol::structure::Channel;
use reqwest::StatusCode;
use serde_json::json;
use time::format_description::well_known::Rfc3339;

use crate::support::TestServer;

#[tokio::test]
async fn filters_and_continuations_select_only_matching_entries() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "filtered").await;
    let since = server.clock.now();
    for name in ["one", "two", "three", "four"] {
        server.clock.advance(time::Duration::seconds(1));
        server
            .patch(
                &owner,
                &format!("/api/v1/channels/{}", channel.id),
                &json!({"name": name}),
            )
            .await
            .ok::<Channel>(StatusCode::OK);
    }
    let until = server.clock.now();
    let root = format!("/api/v1/workspaces/{}/audit-log", workspace.id);
    let filter = format!(
        "action=channel_updated&actor_id={}&target_id={}&since={}&until={}",
        owner.account_id,
        channel.id,
        since.format(&Rfc3339).unwrap(),
        until.format(&Rfc3339).unwrap()
    );
    let first: AuditPage = server
        .get(&owner, &format!("{root}?{filter}&limit=2"))
        .await
        .ok(StatusCode::OK);
    assert_eq!(first.entries.len(), 2);
    assert!(
        first
            .entries
            .iter()
            .all(|entry| entry.event.action() == "channel_updated" && entry.occurred_at < until)
    );
    let second: AuditPage = server
        .get(
            &owner,
            &format!(
                "{root}?{filter}&limit=2&before={}",
                first.next_before.unwrap()
            ),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(second.entries.len(), 1);
    assert!(second.next_before.is_none());
    assert!(second.entries[0].id < first.entries[1].id);
    let last: AuditPage = server
        .get(
            &owner,
            &format!(
                "{root}?action=channel_updated&since={}",
                until.format(&Rfc3339).unwrap()
            ),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(last.entries.len(), 1);
    assert_eq!(last.entries[0].occurred_at, until);
    for absent in [
        "action=no_such_action".to_owned(),
        format!("actor_id={}", uuid::Uuid::nil()),
        format!("target_id={}", uuid::Uuid::nil()),
    ] {
        for route in [&root, &"/api/v1/audit-log".to_owned()] {
            let page: AuditPage = server
                .get(&owner, &format!("{route}?{absent}"))
                .await
                .ok(StatusCode::OK);
            assert!(page.entries.is_empty(), "{absent}: {:?}", page.entries);
        }
    }
    for invalid in [
        "action=Bad-Action".to_owned(),
        "since=invalid".to_owned(),
        format!(
            "since={}&until={}",
            until.format(&Rfc3339).unwrap(),
            since.format(&Rfc3339).unwrap()
        ),
    ] {
        server
            .get(&owner, &format!("{root}?{invalid}"))
            .await
            .error(ErrorCode::InvalidRequest);
    }
}

#[tokio::test]
async fn filters_preserve_current_visibility() {
    use junto_protocol::{WorkspacePermission as W, structure::PermissionRule};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let viewer = server.join(&owner, "Viewer").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "private").await;
    let role = server
        .grant_workspace(&owner, &viewer, workspace.id, &[W::ViewAuditEntries])
        .await;
    let route = format!(
        "/api/v1/workspaces/{}/audit-log?action=channel_created&target_id={}",
        workspace.id, channel.id
    );
    let page: AuditPage = server.get(&viewer, &route).await.ok(StatusCode::OK);
    assert_eq!(page.entries.len(), 1);
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/rules/roles/{role}", channel.id),
            &json!({"allow":[], "deny":["view_channel"]}),
        )
        .await
        .ok::<PermissionRule>(StatusCode::OK);
    let page: AuditPage = server.get(&viewer, &route).await.ok(StatusCode::OK);
    assert!(page.entries.is_empty());
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                workspace.id, viewer.account_id
            ),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server.get(&viewer, &route).await.error(ErrorCode::NotFound);
    server
        .put(
            &owner,
            &format!("/api/v1/accounts/{}/ban", viewer.account_id),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&viewer, &route)
        .await
        .error(ErrorCode::Unauthenticated);
}

#[tokio::test]
async fn filter_queries_use_indexes() {
    use crate::plan_support::{analyze, index_names, visited_rows};
    use junto_protocol::audit::AuditFilterQuery;
    use serde_json::Value;
    use sqlx::Execute;
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let other = server.join(&owner, "Other").await;
    let (first, last) = (1, 10_000);
    sqlx::query(
        "insert into audit_entries (id, occurred_at, actor_id, action, event)
    select gen_random_uuid(), '2020-01-01'::timestamptz, $1, 'owner_claimed',
           jsonb_build_object('action','owner_claimed','account_id',gen_random_uuid())
    from generate_series($2::int,$3::int)",
    )
    .bind(owner.account_id)
    .bind(first)
    .bind(last)
    .execute(&mut server.test_data().await)
    .await
    .unwrap();
    let id = junto_protocol::ids::AuditEntryId::generate();
    sqlx::query("insert into audit_entries (id, occurred_at, actor_id, action, event)
        values ($1, '2021-01-01', $2, 'owner_assigned', jsonb_build_object('action','owner_assigned','account_id',$2::uuid))")
        .bind(id).bind(other.account_id).execute(&server.db).await.unwrap();
    analyze(&server.db, &["audit_entries", "audit_entry_targets"]).await;
    for query in [
        AuditFilterQuery {
            action: Some("owner_assigned".into()),
            ..Default::default()
        },
        AuditFilterQuery {
            actor_id: Some(other.account_id),
            ..Default::default()
        },
        AuditFilterQuery {
            since: Some(time::OffsetDateTime::from_unix_timestamp(1609459200).unwrap()),
            until: Some(time::OffsetDateTime::from_unix_timestamp(1609545600).unwrap()),
            ..Default::default()
        },
        AuditFilterQuery {
            target_id: Some(other.account_id.as_uuid()),
            ..Default::default()
        },
    ] {
        let mut statement = junto_server::routes::audit_log::statement(&query, None, None, 200);
        let mut built = statement.build();
        let arguments = built.take_arguments().unwrap().unwrap();
        let sql = format!("explain (analyze,format json) {}", built.sql().as_str());
        let plan: Value = sqlx::query_scalar_with(sqlx::AssertSqlSafe(sql), arguments)
            .fetch_one(&server.db)
            .await
            .unwrap();
        assert!(!index_names(&plan).is_empty(), "{plan}");
        assert!(visited_rows(&plan[0]["Plan"]) < 100, "{plan}");
    }
    let absent: i64 = sqlx::query_scalar(
        "select count(*) from audit_entry_targets where entry_id=$1 and target_id=$2",
    )
    .bind(id)
    .bind(other.account_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert_eq!(absent, 1);
    assert!(
        sqlx::query("delete from audit_entry_targets where entry_id=$1")
            .bind(id)
            .execute(&server.db)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn migration_indexes_old_targets_and_never_indexes_reason_text() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reason_id = uuid::Uuid::new_v4();
    let id = junto_protocol::ids::AuditEntryId::generate();
    sqlx::raw_sql("drop trigger audit_entries_index_targets on audit_entries; drop table audit_entry_targets;
        drop function index_audit_targets(); drop function audit_targets(jsonb,uuid,uuid,uuid);
        drop index audit_entries_action; drop index audit_entries_actor; drop index audit_entries_time; drop index audit_entries_action_actor; drop index audit_entries_action_time; drop index audit_entries_actor_time; drop index audit_entries_action_actor_time;").execute(&server.db).await.unwrap();
    sqlx::query("insert into audit_entries(id,occurred_at,actor_id,action,event)
        values($1,now(),$2,'account_removed',jsonb_build_object('action','account_removed','account_id',$2::uuid,'reason',$3::text))")
        .bind(id).bind(owner.account_id).bind(reason_id.to_string()).execute(&server.db).await.unwrap();
    sqlx::raw_sql(include_str!("../../migrations/0018_audit_filters.sql"))
        .execute(&server.db)
        .await
        .unwrap();
    let targets: Vec<uuid::Uuid> =
        sqlx::query_scalar("select target_id from audit_entry_targets where entry_id=$1")
            .bind(id)
            .fetch_all(&server.db)
            .await
            .unwrap();
    assert_eq!(targets, [owner.account_id.as_uuid()]);
}
