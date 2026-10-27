use crate::messages_queries::{measure_requests, since, statements};
use crate::support::TestServer;
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::CategoryId;
use serde_json::json;

#[tokio::test]
async fn absent_and_hidden_categories_run_the_same_statements() {
    let server = TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let hidden = server.category(&owner, workspace.id, "Plans", 0).await;
    let absent = CategoryId::generate();
    let (mut held, role) = measure_requests(&server).await;
    let observer = &mut **held.first_mut().unwrap();
    let mut cost = Vec::new();
    for id in [hidden.id, absent, hidden.id, absent] {
        let before = statements(observer, role).await;
        server
            .patch(
                &reader,
                &format!("/api/v1/workspaces/{}/categories/{id}", workspace.id),
                &json!({"name":"Changed"}),
            )
            .await
            .error(ErrorCode::NotFound);
        cost.push(since(statements(observer, role).await, before));
    }
    assert_eq!(cost[2], cost[3]);
    server.require_no_statement_eviction(evictions).await;
}

#[tokio::test]
async fn an_absent_and_a_hidden_channel_run_the_same_statements() {
    use crate::messages_queries::{measure_requests, since, statements};
    let server = crate::support::TestServer::start_measured().await;
    let evictions = server.statement_evictions().await;
    let owner = server.owner().await;
    let ann = server.join(&owner, "Ann").await;
    let hidden = crate::messages_support::channel(&server, &owner, "text").await;
    let everyone = server.everyone_role(&owner, hidden.workspace_id).await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", hidden.id, everyone.id),
            &serde_json::json!({ "deny": ["view_channel"] }),
        )
        .await
        .ok::<serde_json::Value>(reqwest::StatusCode::OK);
    let absent = junto_protocol::ids::ChannelId::generate();
    let (mut held, role) = measure_requests(&server).await;
    let observer = &mut **held.first_mut().unwrap();
    let mut cost = Vec::new();
    for channel_id in [hidden.id, absent, hidden.id, absent] {
        let before = statements(observer, role).await;
        server
            .get(&ann, &format!("/api/v1/channels/{channel_id}/messages"))
            .await
            .error(junto_protocol::error::ErrorCode::NotFound);
        cost.push(since(statements(observer, role).await, before));
    }
    // The first request of each prepares its statements; the second ones are compared.
    assert_eq!(cost[2], cost[3]);
    server.require_no_statement_eviction(evictions).await;
}
