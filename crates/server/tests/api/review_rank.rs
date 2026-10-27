//! A server person rule changes the role order (ADR 0309).

use crate::support::TestServer;
use junto_protocol::error::ErrorCode;
use junto_protocol::{ServerPermission, WorkspacePermission};
use reqwest::StatusCode;
use serde_json::{Value, json};

#[tokio::test]
async fn a_server_person_deny_of_administrator_removes_the_rank_of_the_administrator_role() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let eli = server.join(&owner, "Eli").await;
    let workspace = server.default_workspace(&owner).await.id;
    let top = server.grant_workspace(&owner, &owner, workspace, &[]).await;
    server
        .grant_workspace(&owner, &eli, workspace, &[WorkspacePermission::ManageRoles])
        .await;
    server
        .grant_server(&owner, &eli, &[ServerPermission::Administrator])
        .await;
    let route = format!("/api/v1/workspaces/{workspace}/roles/{top}");
    server
        .patch(&eli, &route, &json!({"name": "renamed"}))
        .await
        .ok::<Value>(StatusCode::OK);

    server
        .put(
            &owner,
            &format!("/api/v1/accounts/{}/server-permissions", eli.account_id),
            &json!({"allow": [], "deny": ["administrator"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .patch(&eli, &route, &json!({"name": "again"}))
        .await
        .error(ErrorCode::Forbidden);
}
