//! Person rules and the end of a membership (ADR 0304).

use crate::support::{Member, TestServer};
use junto_protocol::access_token::TokenAction;
use junto_protocol::error::ErrorCode;
use junto_protocol::structure::{Channel, Workspace};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

async fn put_rule(server: &TestServer, owner: &Member, path: &str, body: Value) {
    server
        .put(owner, path, &body)
        .await
        .ok::<Value>(StatusCode::OK);
}

#[tokio::test]
async fn a_person_rule_stays_when_the_person_leaves_and_applies_again_on_return() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Dana").await;
    let side: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name": "Side"}))
        .await
        .ok(StatusCode::CREATED);
    let channel = server.channel(&owner, side.id, "plans").await;
    let membership = format!(
        "/api/v1/workspaces/{}/members/{}",
        side.id, reader.account_id
    );
    server
        .put_empty(&owner, &membership)
        .await
        .empty(StatusCode::NO_CONTENT);
    let rule = format!(
        "/api/v1/channels/{}/rules/people/{}",
        channel.id, reader.account_id
    );
    let channel_path = format!("/api/v1/channels/{}/messages", channel.id);
    server
        .get(&reader, &channel_path)
        .await
        .ok::<Value>(StatusCode::OK);
    put_rule(&server, &owner, &rule, json!({"deny": ["view_channel"]})).await;
    server
        .get(&reader, &channel_path)
        .await
        .error(ErrorCode::NotFound);

    server
        .post(
            &reader,
            &format!("/api/v1/workspaces/{}/departure", side.id),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let listed: Value = server
        .get(&owner, &format!("/api/v1/channels/{}/rules", channel.id))
        .await
        .ok(StatusCode::OK);
    assert!(
        listed["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|held| held["subject"]["account_id"] == reader.account_id.to_string()),
        "{listed}"
    );
    server
        .put(&owner, &rule, &json!({"deny": ["attach_files"]}))
        .await
        .error(ErrorCode::NotFound);

    server
        .put_empty(&owner, &membership)
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .get(&reader, &channel_path)
        .await
        .error(ErrorCode::NotFound);

    server
        .post(
            &reader,
            &format!("/api/v1/workspaces/{}/departure", side.id),
            &Value::Null,
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    put_rule(&server, &owner, &rule, json!({})).await;
    let listed: Value = server
        .get(&owner, &format!("/api/v1/channels/{}/rules", channel.id))
        .await
        .ok(StatusCode::OK);
    assert!(
        listed["rules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|held| held["subject"]["kind"] != "person"),
        "{listed}"
    );
}

#[tokio::test]
async fn an_access_token_does_not_read_the_rules_of_people() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Dana").await;
    let workspace = server.default_workspace(&owner).await.id;
    let category = server.category(&owner, workspace, "Plans", 0).await;
    let channel: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{workspace}/channels"),
            &json!({"name": "plans", "kind": "text", "category_id": category.id}),
        )
        .await
        .ok(StatusCode::CREATED);
    put_rule(
        &server,
        &owner,
        &format!(
            "/api/v1/channels/{}/rules/people/{}",
            channel.id, reader.account_id
        ),
        json!({"deny": ["attach_files"]}),
    )
    .await;
    let token = server
        .access_token(
            &owner,
            &[TokenAction::ReadStructure, TokenAction::ManageStructure],
        )
        .await;
    let rules = format!("/api/v1/channels/{}/rules", channel.id);
    let read: Value = server
        .with_token(Method::GET, &rules, &token, None)
        .await
        .ok(StatusCode::OK);
    assert!(read["rules"].as_array().unwrap().is_empty(), "{read}");
    let category_rules = format!(
        "/api/v1/workspaces/{workspace}/categories/{}/rules/people/{}",
        category.id, reader.account_id
    );
    put_rule(
        &server,
        &owner,
        &category_rules,
        json!({"deny": ["add_reactions"]}),
    )
    .await;
    let read: Value = server
        .with_token(
            Method::GET,
            &format!(
                "/api/v1/workspaces/{workspace}/categories/{}/rules",
                category.id
            ),
            &token,
            None,
        )
        .await
        .ok(StatusCode::OK);
    assert!(read["rules"].as_array().unwrap().is_empty(), "{read}");
    let synced: Value = server
        .with_token(Method::POST, &format!("{rules}/sync"), &token, None)
        .await
        .ok(StatusCode::OK);
    assert_eq!(synced["synced"], true);
    assert!(synced["rules"].as_array().unwrap().is_empty(), "{synced}");
    let owner_read: Value = server.get(&owner, &rules).await.ok(StatusCode::OK);
    assert_eq!(owner_read["rules"].as_array().unwrap().len(), 1);
}
