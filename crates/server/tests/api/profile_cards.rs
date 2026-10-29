use crate::support::TestServer;
use reqwest::StatusCode;
use serde_json::Value;

#[tokio::test]
async fn a_visible_account_has_a_profile_card() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let person = server.join(&owner, "Member").await;
    let card: Value = server
        .get(
            &owner,
            &format!("/api/v1/accounts/{}/card", person.account_id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(card["profile"]["id"], person.account_id.to_string());
    assert_eq!(card["profile"]["name"], "Member");
}

use junto_protocol::{
    ServerPermission as S, WorkspacePermission as W, account::AccountCard, error::ErrorCode,
};
use serde_json::json;

#[tokio::test]
async fn cards_show_public_role_names_but_only_visible_workspaces() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let viewer = server.join(&owner, "Viewer").await;
    let target = server.join(&owner, "Target").await;
    let shared = server.default_workspace(&owner).await;
    let private: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Private"}))
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}",
                private.id, target.account_id
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let server_role = server
        .grant_server(&owner, &target, &[S::CreateInvites])
        .await;
    let shared_role = server
        .grant_workspace(&owner, &target, shared.id, &[W::SendMessages])
        .await;
    let private_role = server
        .grant_workspace(&owner, &target, private.id, &[W::ManageChannel])
        .await;
    let route = format!("/api/v1/accounts/{}/card", target.account_id);
    let card: AccountCard = server.get(&viewer, &route).await.ok(StatusCode::OK);
    assert_eq!(
        card.server_roles
            .iter()
            .map(|role| role.id)
            .collect::<Vec<_>>(),
        [server_role.as_uuid()]
    );
    assert_eq!(card.workspaces.len(), 1);
    assert_eq!(card.workspaces[0].workspace_id, shared.id);
    assert_eq!(
        card.workspaces[0]
            .roles
            .iter()
            .map(|role| role.id)
            .collect::<Vec<_>>(),
        [shared_role.as_uuid()]
    );
    let card: AccountCard = server.get(&owner, &route).await.ok(StatusCode::OK);
    assert!(card.workspaces.iter().any(|workspace| {
        workspace.workspace_id == private.id
            && workspace
                .roles
                .iter()
                .any(|role| role.id == private_role.as_uuid())
    }));
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                shared.id, viewer.account_id
            ),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server.get(&viewer, &route).await.error(ErrorCode::NotFound);
}

#[tokio::test]
async fn unavailable_cards_answer_like_missing_accounts() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let viewer = server.join(&owner, "Viewer").await;
    let separate: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Separate"}))
        .await
        .ok(StatusCode::CREATED);
    let target = server.join_into(&owner, "Target", &[separate.id]).await;
    let route = format!("/api/v1/accounts/{}/card", target.account_id);
    server.get(&viewer, &route).await.error(ErrorCode::NotFound);
    server
        .get(
            &viewer,
            &format!(
                "/api/v1/accounts/{}/card",
                junto_protocol::ids::AccountId::generate()
            ),
        )
        .await
        .error(ErrorCode::NotFound);
    server
        .put_empty(
            &owner,
            &format!("/api/v1/accounts/{}/ban", target.account_id),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server.get(&owner, &route).await.error(ErrorCode::NotFound);
}
