//! Audit entries of categories follow the visibility of the category (ADR 0304).

use crate::support::{Member, TestServer};
use junto_protocol::WorkspacePermission;
use junto_protocol::audit::{AuditEvent, AuditPage};
use junto_protocol::ids::{CategoryId, WorkspaceId};
use junto_protocol::structure::{Category, Channel};
use reqwest::StatusCode;
use serde_json::{Value, json};

async fn put_rule(server: &TestServer, owner: &Member, path: &str, body: Value) {
    server
        .put(owner, path, &body)
        .await
        .ok::<Value>(StatusCode::OK);
}

async fn category_actions(
    server: &TestServer,
    reader: &Member,
    workspace: WorkspaceId,
    category: CategoryId,
) -> Vec<&'static str> {
    let page: AuditPage = server
        .get(
            reader,
            &format!("/api/v1/workspaces/{workspace}/audit-log?limit=200"),
        )
        .await
        .ok(StatusCode::OK);
    page.entries
        .iter()
        .filter(|entry| match &entry.event {
            AuditEvent::CategoryCreated { category_id, .. }
            | AuditEvent::CategoryUpdated { category_id, .. }
            | AuditEvent::CategoryDeleted { category_id, .. }
            | AuditEvent::CategoryRoleRuleSet { category_id, .. }
            | AuditEvent::CategoryPersonRuleSet { category_id, .. } => *category_id == category,
            _ => false,
        })
        .map(|entry| entry.event.action())
        .collect()
}

#[tokio::test]
async fn audit_entries_of_a_hidden_category_are_hidden_like_those_of_a_hidden_channel() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let workspace = server.default_workspace(&owner).await.id;
    let everyone = server.everyone_role(&owner, workspace).await.id;
    let moderator = server.join(&owner, "Moderator").await;
    server
        .grant_workspace(
            &owner,
            &moderator,
            workspace,
            &[WorkspacePermission::ViewAuditEntries],
        )
        .await;

    let open = server.category(&owner, workspace, "Open", 0).await;
    server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{workspace}/channels"),
            &json!({"name": "open-chat", "kind": "text", "category_id": open.id}),
        )
        .await
        .ok::<Channel>(StatusCode::CREATED);
    let hidden = server.category(&owner, workspace, "Clients B", 1).await;
    let base = format!("/api/v1/workspaces/{workspace}/categories/{}", hidden.id);
    server
        .patch(&owner, &base, &json!({"name": "Clients C"}))
        .await
        .ok::<Category>(StatusCode::OK);
    put_rule(
        &server,
        &owner,
        &format!("{base}/rules/roles/{everyone}"),
        json!({"deny": ["view_channel"]}),
    )
    .await;
    put_rule(
        &server,
        &owner,
        &format!("{base}/rules/people/{}", owner.account_id),
        json!({"allow": ["view_channel"]}),
    )
    .await;

    assert_eq!(
        category_actions(&server, &moderator, workspace, hidden.id).await,
        Vec::<&str>::new()
    );
    assert_eq!(
        category_actions(&server, &moderator, workspace, open.id).await,
        ["category_created"]
    );
    assert_eq!(
        category_actions(&server, &owner, workspace, hidden.id)
            .await
            .len(),
        4
    );

    server
        .delete(&owner, &base)
        .await
        .empty(StatusCode::NO_CONTENT);
    assert_eq!(
        category_actions(&server, &moderator, workspace, hidden.id).await,
        Vec::<&str>::new()
    );
    assert_eq!(
        category_actions(&server, &owner, workspace, hidden.id)
            .await
            .len(),
        5
    );
}
