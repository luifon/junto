use crate::support::TestServer;
use junto_protocol::WorkspacePermission as W;
use junto_protocol::error::ErrorCode;
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::structure::ChannelOverride;
use reqwest::StatusCode;
use serde_json::{Value, json};

#[tokio::test]
async fn one_permission_preserves_other_bits() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "text").await;
    let role: WorkspaceRole = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
            &json!({"name":"Higher", "permissions":[]}),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .grant_workspace(&owner, &manager, workspace.id, &[W::ManageChannel])
        .await;
    let route = format!("/api/v1/channels/{}/overrides/{}", channel.id, role.id);
    server
        .put(
            &owner,
            &route,
            &json!({"allow":["manage_messages"],"deny":["disconnect_members"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let changed: ChannelOverride = server
        .patch(
            &manager,
            &route,
            &json!({"permission":"mute_members","state":"deny"}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(changed.allow, [W::ManageMessages]);
    assert!(changed.deny.contains(&W::DisconnectMembers));
    assert!(changed.deny.contains(&W::MuteMembers));
    // Removing an allow is not a grant; removing a deny is.
    server
        .patch(
            &manager,
            &route,
            &json!({"permission":"manage_messages","state":"inherit"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .patch(
            &manager,
            &route,
            &json!({"permission":"mute_members","state":"inherit"}),
        )
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    let changed: ChannelOverride = server
        .patch(
            &owner,
            &route,
            &json!({"permission":"mute_members","state":"allow"}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(changed.allow, [W::MuteMembers]);
    assert_eq!(changed.deny, [W::DisconnectMembers]);
}

#[tokio::test]
async fn one_permission_obeys_visibility_and_the_grant_ceiling() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let manager = server.join(&owner, "Manager").await;
    let plain = server.join(&owner, "Plain").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "text").await;
    server
        .grant_workspace(&owner, &manager, workspace.id, &[W::ManageChannel])
        .await;
    let role = server.everyone_role(&owner, workspace.id).await;
    let route = format!("/api/v1/channels/{}/overrides/{}", channel.id, role.id);
    let body = json!({"permission":"mute_members","state":"allow"});
    server
        .patch(
            &plain,
            &route,
            &json!({"permission":"mute_members","state":"deny"}),
        )
        .await
        .error(ErrorCode::Forbidden);
    server
        .patch(&manager, &route, &body)
        .await
        .error(ErrorCode::ExceedsOwnPermissions);
    server
        .patch(
            &owner,
            &route,
            &json!({"permission":"unknown","state":"allow"}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .patch(
            &owner,
            &route,
            &json!({"permission":"mute_members","state":"unknown"}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    server
        .patch(
            &owner,
            &route,
            &json!({"permission":"view_channel","state":"deny"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .patch(&manager, &route, &body)
        .await
        .error(ErrorCode::NotFound);
    let route = format!(
        "/api/v1/channels/{}/overrides/{}",
        uuid::Uuid::new_v4(),
        role.id
    );
    server
        .patch(&manager, &route, &body)
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn concurrent_permission_changes_are_preserved() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "text").await;
    let role = server.everyone_role(&owner, workspace.id).await;
    let route = format!("/api/v1/channels/{}/overrides/{}", channel.id, role.id);
    let one = json!({"permission":"mute_members","state":"deny"});
    let two = json!({"permission":"disconnect_members","state":"deny"});
    server.gate().arm(0);
    let first = server.patch(&owner, &route, &one);
    tokio::pin!(first);
    crate::support::drive_until(&mut first, server.gate().arrived()).await;
    let second = server.patch(&owner, &route, &two);
    tokio::pin!(second);
    crate::support::drive_until(&mut second, server.until_blocked()).await;
    server.gate().release();
    let (first, second) = tokio::join!(first, second);
    first.ok::<Value>(StatusCode::OK);
    second.ok::<Value>(StatusCode::OK);
    let overrides: Vec<ChannelOverride> = server
        .get(
            &owner,
            &format!("/api/v1/channels/{}/overrides", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(overrides.len(), 1);
    assert_eq!(overrides[0].deny.len(), 2);
    assert!(overrides[0].deny.contains(&W::MuteMembers));
    assert!(overrides[0].deny.contains(&W::DisconnectMembers));
}

#[tokio::test]
async fn override_and_membership_changes_use_their_indexes() {
    use crate::plan_support::{analyze, explained, index_names, visited_rows};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "text").await;
    let role = server.everyone_role(&owner, workspace.id).await;
    sqlx::query("with people as (insert into accounts(id,name,status,created_at) select gen_random_uuid(),'filler','active',$2 from generate_series(1,10000) returning id) insert into workspace_members(workspace_id,account_id,joined_at) select $1,id,$2 from people")
        .bind(workspace.id).bind(server.clock.now()).execute(&mut server.test_data().await).await.unwrap();
    sqlx::query("with copies as (insert into channels(id,workspace_id,kind,name,position,created_at) select gen_random_uuid(),workspace_id,kind,name,position+n,created_at from channels cross join generate_series(1,10000) n where id=$1 returning id,workspace_id) insert into channel_overrides(channel_id,role_id,workspace_id,allow,deny) select id,$2,workspace_id,0,1 from copies")
        .bind(channel.id).bind(role.id).execute(&mut server.test_data().await).await.unwrap();
    analyze(&server.db, &["workspace_members", "channel_overrides"]).await;
    let membership: Value = sqlx::query_scalar(explained(
        "delete from workspace_members where workspace_id=$1 and account_id=$2",
    ))
    .bind(workspace.id)
    .bind(owner.account_id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(
        index_names(&membership)
            .iter()
            .any(|n| n == "workspace_members_pkey" || n == "workspace_members_account"),
        "{membership}"
    );
    assert!(visited_rows(&membership[0]["Plan"]) < 10, "{membership}");
    let overrides: Value = sqlx::query_scalar(explained(
        "select allow,deny from channel_overrides where channel_id=$1 and role_id=$2 for update",
    ))
    .bind(channel.id)
    .bind(role.id)
    .fetch_one(&server.db)
    .await
    .unwrap();
    assert!(
        index_names(&overrides)
            .iter()
            .any(|n| n == "channel_overrides_pkey"),
        "{overrides}"
    );
    assert!(visited_rows(&overrides[0]["Plan"]) < 10, "{overrides}");
}

/// A request that sets the whole override to what it holds writes no audit entry, no line
/// and no event.
#[tokio::test]
async fn an_identical_override_writes_no_history() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "text").await;
    let role = server.everyone_role(&owner, workspace.id).await;
    let route = format!("/api/v1/channels/{}/overrides/{}", channel.id, role.id);
    for body in [
        json!({"allow": [], "deny": []}),
        json!({"allow": ["mute_members"], "deny": ["lock_room"]}),
    ] {
        server
            .put(&owner, &route, &body)
            .await
            .ok::<Value>(StatusCode::OK);
        let before = (
            server.count("audit_entries").await,
            server.count("messages").await,
            server.snapshot(&owner).await.cursor,
        );
        server
            .put(&owner, &route, &body)
            .await
            .ok::<Value>(StatusCode::OK);
        assert_eq!(
            (
                server.count("audit_entries").await,
                server.count("messages").await,
                server.snapshot(&owner).await.cursor
            ),
            before
        );
    }
}

#[tokio::test]
async fn an_unchanged_permission_writes_no_history() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "text").await;
    let role = server.everyone_role(&owner, workspace.id).await;
    let route = format!("/api/v1/channels/{}/overrides/{}", channel.id, role.id);
    for state in ["inherit", "deny", "allow"] {
        let body = json!({"permission":"mute_members","state":state});
        if state != "inherit" {
            server
                .patch(&owner, &route, &body)
                .await
                .ok::<Value>(StatusCode::OK);
        }
        let before = (
            server.count("audit_entries").await,
            server.count("messages").await,
            server.snapshot(&owner).await.cursor,
        );
        server
            .patch(&owner, &route, &body)
            .await
            .ok::<Value>(StatusCode::OK);
        assert_eq!(
            (
                server.count("audit_entries").await,
                server.count("messages").await,
                server.snapshot(&owner).await.cursor
            ),
            before
        );
    }
}

#[tokio::test]
async fn patch_audits_notifies_and_removes_the_last_permission() {
    use junto_protocol::audit::{AuditEvent, AuditPage};
    use junto_protocol::events::{Event, Level};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let holder = server.join(&owner, "Holder").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "text").await;
    let role = server
        .grant_workspace(&owner, &holder, workspace.id, &[])
        .await;
    let route = format!("/api/v1/channels/{}/overrides/{role}", channel.id);
    let before = server.snapshot(&holder).await.cursor;
    server
        .patch(
            &owner,
            &route,
            &json!({"permission":"mute_members","state":"deny"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let log: AuditPage = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/audit-log", workspace.id),
        )
        .await
        .ok(StatusCode::OK);
    assert!(log.entries.iter().any(|e| matches!(&e.event, AuditEvent::ChannelOverrideSet{role_id,deny,..} if *role_id==role && deny==&[W::MuteMembers])));
    let events = crate::discovery_support::events(&server, &holder, before, Level::Full).await;
    let manager_events =
        crate::discovery_support::events(&server, &owner, before, Level::Full).await;
    assert!(
        manager_events
            .iter()
            .any(|e| matches!(e, Event::ChannelOverrideSet { .. }))
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::PermissionsChanged { .. }))
    );
    server
        .patch(
            &owner,
            &route,
            &json!({"permission":"mute_members","state":"inherit"}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    assert_eq!(server.count("channel_overrides").await, 0);
    let other: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Other"}))
        .await
        .ok(StatusCode::CREATED);
    let foreign = server.everyone_role(&owner, other.id).await;
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", channel.id, foreign.id),
            &json!({"permission":"mute_members","state":"inherit"}),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn patch_retries_after_a_concurrent_put() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let channel = server.channel(&owner, workspace.id, "text").await;
    let role = server.everyone_role(&owner, workspace.id).await;
    let route = format!("/api/v1/channels/{}/overrides/{}", channel.id, role.id);
    let put = json!({"allow":[],"deny":["disconnect_members"]});
    let patch = json!({"permission":"mute_members","state":"deny"});
    server.gate().arm(0);
    let first = server.put(&owner, &route, &put);
    tokio::pin!(first);
    crate::support::drive_until(&mut first, server.gate().arrived()).await;
    let second = server.patch(&owner, &route, &patch);
    tokio::pin!(second);
    crate::support::drive_until(&mut second, server.until_blocked()).await;
    server.gate().release();
    let (first, second) = tokio::join!(first, second);
    first.ok::<Value>(StatusCode::OK);
    let changed: ChannelOverride = second.ok(StatusCode::OK);
    assert_eq!(changed.deny.len(), 2);
    assert!(changed.deny.contains(&W::MuteMembers));
    assert!(changed.deny.contains(&W::DisconnectMembers));
}
