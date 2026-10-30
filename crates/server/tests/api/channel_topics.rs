use junto_protocol::error::ErrorCode;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::messages_support::channel;
use crate::support::TestServer;

#[tokio::test]
async fn a_channel_topic_is_stored_readable_and_changed_only_by_its_managers() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let path = format!("/api/v1/channels/{}", channel.id);
    let topic = "Plans\nand links";
    let created: Value = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", channel.workspace_id),
            &json!({"name":"voice-topic", "kind":"voice", "topic":"Call agenda"}),
        )
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(created["topic"], "Call agenda");
    let changed: Value = server
        .patch(&owner, &path, &json!({"topic":"  Plans\r\nand links  "}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(changed["topic"], topic);
    let read: Value = server.get(&reader, &path).await.ok(StatusCode::OK);
    assert_eq!(read["topic"], topic);
    server
        .patch(&reader, &path, &json!({"topic":"Changed"}))
        .await
        .error(ErrorCode::Forbidden);
    let renamed: Value = server
        .patch(&owner, &path, &json!({"name":"planning"}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(renamed["topic"], topic);
    let cleared: Value = server
        .patch(&owner, &path, &json!({"topic":""}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(cleared["topic"], "");
    crate::archive::archive(&server, &owner, channel.id)
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .patch(&owner, &path, &json!({"topic":"Archived change"}))
        .await
        .error(ErrorCode::ChannelArchived);
}

#[tokio::test]
async fn topics_are_bounded_unicode_text_and_structure_exports_keep_them() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let path = format!("/api/v1/channels/{}", channel.id);
    for topic in ["x".repeat(1025), "bad\0topic".to_owned()] {
        server
            .patch(&owner, &path, &json!({"topic":topic}))
            .await
            .error(ErrorCode::InvalidRequest);
    }
    let topic = "é".repeat(1024);
    let changed: Value = server
        .patch(&owner, &path, &json!({"topic":topic}))
        .await
        .ok(StatusCode::OK);
    assert_eq!(changed["topic"], topic);
    let exported: Value = server
        .get(&owner, "/api/v1/structure")
        .await
        .ok(StatusCode::OK);
    let stored = exported["structure"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|workspace| workspace["channels"].as_array().unwrap())
        .find(|entry| entry["name"] == channel.name)
        .unwrap();
    assert_eq!(stored["topic"], topic);
    let revision = exported["revision"].as_str().unwrap();
    let changed = json!([{"change":"update_channel", "id":stored["id"], "topic":"Changed through structure"}]);
    server
        .change_structure(&owner, revision, changed)
        .await
        .ok::<Value>(StatusCode::OK);
    let read: Value = server.get(&owner, &path).await.ok(StatusCode::OK);
    assert_eq!(read["topic"], "Changed through structure");
}
