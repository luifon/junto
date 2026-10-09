use junto_protocol::audit::{AuditEntry, AuditEvent, AuditPage};
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{ChannelId, WorkspaceId};
use junto_protocol::structure::{Channel, ChannelOverride, Workspace};
use junto_protocol::{ServerPermission, WorkspacePermission};
use reqwest::StatusCode;
use serde_json::json;

use crate::structure::set_override;
use crate::support::{Member, TestServer};

use WorkspacePermission as W;

async fn workspace_log(
    server: &TestServer,
    caller: &Member,
    workspace_id: WorkspaceId,
) -> Vec<AuditEntry> {
    let page: AuditPage = server
        .get(
            caller,
            &format!("/api/v1/workspaces/{workspace_id}/audit-log?limit=200"),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(page.next_before, None);
    page.entries
}

fn about(entries: &[AuditEntry], channel_id: ChannelId) -> Vec<&'static str> {
    entries
        .iter()
        .filter(|entry| entry.channel_id == Some(channel_id))
        .map(|entry| entry.event.action())
        .collect()
}

async fn create_channel(
    server: &TestServer,
    caller: &Member,
    workspace_id: WorkspaceId,
    name: &str,
) -> Channel {
    server
        .post(
            caller,
            &format!("/api/v1/workspaces/{workspace_id}/channels"),
            &json!({ "name": name, "kind": "text" }),
        )
        .await
        .ok(StatusCode::CREATED)
}

#[tokio::test]
async fn an_entry_about_a_private_channel_is_shown_only_to_viewers_who_see_the_channel() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let insider = server.join(&owner, "Insider").await;
    let outsider = server.join(&owner, "Outsider").await;
    let plain = server.join(&owner, "Plain").await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    let insider_role = server
        .grant_workspace(&owner, &insider, workspace.id, &[W::ViewAuditEntries])
        .await;
    server
        .grant_workspace(&owner, &outsider, workspace.id, &[W::ViewAuditEntries])
        .await;
    let everyone = server
        .get(
            &owner,
            &format!("/api/v1/workspaces/{}/roles", workspace.id),
        )
        .await
        .ok::<Vec<junto_protocol::roles::WorkspaceRole>>(StatusCode::OK)
        .into_iter()
        .find(|role| role.is_everyone)
        .unwrap();

    let open = create_channel(&server, &owner, workspace.id, "open").await;
    let private = create_channel(&server, &owner, workspace.id, "private").await;
    set_override(
        &server,
        &owner,
        private.id,
        everyone.id,
        &[],
        &[W::ViewChannel],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    set_override(
        &server,
        &owner,
        private.id,
        insider_role,
        &[W::ViewChannel],
        &[],
    )
    .await
    .ok::<ChannelOverride>(StatusCode::OK);
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", private.id),
            &json!({ "name": "secret plans" }),
        )
        .await
        .ok::<Channel>(StatusCode::OK);

    let private_actions = [
        "channel_updated",
        "channel_override_set",
        "channel_override_set",
        "channel_created",
    ];
    for viewer in [&insider, &administrator, &owner] {
        let entries = workspace_log(&server, viewer, workspace.id).await;
        assert_eq!(about(&entries, private.id), private_actions);
        assert_eq!(about(&entries, open.id), ["channel_created"]);
    }

    let entries = workspace_log(&server, &outsider, workspace.id).await;
    assert!(about(&entries, private.id).is_empty());
    assert_eq!(about(&entries, open.id), ["channel_created"]);
    assert!(
        !serde_json::to_string(&entries)
            .unwrap()
            .contains("secret plans")
    );
    assert!(
        !serde_json::to_string(&entries)
            .unwrap()
            .contains(&private.id.to_string())
    );
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry.event, AuditEvent::WorkspaceRoleAssigned { .. }))
    );

    server
        .get(
            &plain,
            &format!("/api/v1/workspaces/{}/audit-log", workspace.id),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn entries_about_a_deleted_channel_stay_visible_to_server_administrators_only() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let auditor = server.join(&owner, "Auditor").await;
    let administrator = server.join(&owner, "Administrator").await;
    server
        .grant_workspace(&owner, &auditor, workspace.id, W::ALL)
        .await;
    server
        .grant_server(&owner, &administrator, &[ServerPermission::Administrator])
        .await;
    let channel = create_channel(&server, &owner, workspace.id, "short lived").await;

    let before = workspace_log(&server, &auditor, workspace.id).await;
    assert_eq!(about(&before, channel.id), ["channel_created"]);

    server
        .delete(&auditor, &format!("/api/v1/channels/{}", channel.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .delete(&owner, &format!("/api/v1/channels/{}", channel.id))
        .await
        .error(ErrorCode::NotFound);
    server
        .patch(
            &owner,
            &format!("/api/v1/channels/{}", channel.id),
            &json!({ "name": "back" }),
        )
        .await
        .error(ErrorCode::NotFound);

    let after = workspace_log(&server, &auditor, workspace.id).await;
    assert!(about(&after, channel.id).is_empty());
    assert!(
        !serde_json::to_string(&after)
            .unwrap()
            .contains("short lived")
    );

    for viewer in [&administrator, &owner] {
        let entries = workspace_log(&server, viewer, workspace.id).await;
        assert_eq!(
            about(&entries, channel.id),
            ["channel_deleted", "channel_created"]
        );
        let deletion = entries
            .iter()
            .find(|entry| entry.event.action() == "channel_deleted")
            .unwrap();
        assert_eq!(deletion.actor_id, Some(auditor.account_id));
        assert_eq!(
            deletion.event,
            AuditEvent::ChannelDeleted {
                name: "short lived".to_owned()
            }
        );
    }
}

#[tokio::test]
async fn the_server_log_and_the_workspace_log_need_their_own_permissions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await;
    let side: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({ "name": "Side" }))
        .await
        .ok(StatusCode::CREATED);
    let server_auditor = server.join(&owner, "Server auditor").await;
    let workspace_auditor = server.join(&owner, "Workspace auditor").await;
    server
        .grant_server(
            &owner,
            &server_auditor,
            &[ServerPermission::ViewServerAuditLog],
        )
        .await;
    server
        .grant_workspace(
            &owner,
            &workspace_auditor,
            workspace.id,
            &[W::ViewAuditEntries],
        )
        .await;

    let server_log: AuditPage = server
        .get(&server_auditor, "/api/v1/audit-log?limit=200")
        .await
        .ok(StatusCode::OK);
    assert!(
        server_log
            .entries
            .iter()
            .all(|entry| entry.workspace_id.is_none())
    );
    let actions: Vec<&str> = server_log
        .entries
        .iter()
        .map(|entry| entry.event.action())
        .collect();
    for expected in [
        "owner_claimed",
        "workspace_created",
        "invite_created",
        "account_joined",
        "server_role_created",
        "server_role_assigned",
    ] {
        assert!(actions.contains(&expected), "{expected}");
    }
    assert!(!actions.contains(&"workspace_role_created"));
    server
        .get(
            &server_auditor,
            &format!("/api/v1/workspaces/{}/audit-log", workspace.id),
        )
        .await
        .error(ErrorCode::Forbidden);

    let entries = workspace_log(&server, &workspace_auditor, workspace.id).await;
    assert!(
        entries
            .iter()
            .all(|entry| entry.workspace_id == Some(workspace.id))
    );
    let actions: Vec<&str> = entries.iter().map(|entry| entry.event.action()).collect();
    assert_eq!(
        actions,
        ["workspace_role_assigned", "workspace_role_created"]
    );
    server
        .get(&workspace_auditor, "/api/v1/audit-log")
        .await
        .error(ErrorCode::Forbidden);
    server
        .get(
            &workspace_auditor,
            &format!("/api/v1/workspaces/{}/audit-log", side.id),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn the_audit_log_is_read_in_pages_from_the_newest_entry() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    for _ in 0..4 {
        server.invite(&owner, json!({})).await;
    }

    let first: AuditPage = server
        .get(&owner, "/api/v1/audit-log?limit=2")
        .await
        .ok(StatusCode::OK);
    assert_eq!(first.entries.len(), 2);
    let cursor = first.next_before.unwrap();
    assert_eq!(cursor, first.entries[1].id);

    let second: AuditPage = server
        .get(
            &owner,
            &format!("/api/v1/audit-log?limit=2&before={cursor}"),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(second.entries.len(), 2);
    let third: AuditPage = server
        .get(
            &owner,
            &format!(
                "/api/v1/audit-log?limit=2&before={}",
                second.next_before.unwrap()
            ),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(third.entries.len(), 1);
    assert_eq!(third.next_before, None);
    assert_eq!(third.entries[0].event.action(), "owner_claimed");

    let mut ids: Vec<_> = [&first, &second, &third]
        .iter()
        .flat_map(|page| page.entries.iter().map(|entry| entry.id))
        .collect();
    let read_order = ids.clone();
    ids.sort_by(|left, right| right.cmp(left));
    ids.dedup();
    assert_eq!(ids, read_order);

    for query in ["limit=0", "limit=201", "limit=abc", "before=not-an-id"] {
        server
            .get(&owner, &format!("/api/v1/audit-log?{query}"))
            .await
            .error(ErrorCode::InvalidRequest);
    }
}
