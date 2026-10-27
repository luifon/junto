//! Person rules and the end of a membership (ADR 0304).

use crate::support::{Member, TestServer};
use junto_protocol::error::ErrorCode;
use junto_protocol::structure::Workspace;
use reqwest::StatusCode;
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
