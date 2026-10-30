use crate::messages_support::{channel, message, send, text, upload};
use crate::support::{Scratch, TestServer, origin};
use junto_protocol::{error::ErrorCode, ids::MessageId, message::Message};
use reqwest::StatusCode;
use serde_json::{Value, json};

fn forwarded(id: MessageId, words: &str) -> Value {
    json!({"document":{"blocks":[
        {"kind":"forward", "server":origin(), "message_id":id},
        {"kind":"paragraph", "content":[{"kind":"text", "text":words}]}
    ]}})
}

#[tokio::test]
async fn the_server_copies_the_readable_source_instead_of_the_submitted_body() {
    let server = TestServer::start().await;
    let author = server.owner().await;
    let copier = server.join(&author, "Copier").await;
    let channel = channel(&server, &author, "text").await;
    let source = send(&server, &author, &channel, "authentic source text").await;
    let copy: Message = server
        .post(
            &copier,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &forwarded(source.id, "forged source text"),
        )
        .await
        .ok(StatusCode::CREATED);
    let value = serde_json::to_value(&copy.document).unwrap();
    assert!(value.to_string().contains("authentic source text"));
    assert!(!value.to_string().contains("forged source text"));
    assert_eq!(value["blocks"][0]["verified"], true);
    server
        .put(&author, &message(source.id), &text("later source edit"))
        .await
        .ok::<Message>(StatusCode::OK);
    let retained: Message = server
        .get(&copier, &message(copy.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(retained.document, copy.document);
}

#[tokio::test]
async fn an_invented_local_origin_cannot_be_forwarded() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let copier = server.join(&owner, "Copier").await;
    let channel = channel(&server, &owner, "text").await;
    server
        .post(
            &copier,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &forwarded(MessageId::generate(), "invented body"),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn editing_a_copy_cannot_retain_authenticated_source_attribution() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let copier = server.join(&owner, "Copier").await;
    let channel = channel(&server, &owner, "text").await;
    let source = send(&server, &owner, &channel, "authentic text").await;
    let copy: Message = server
        .post(
            &copier,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &forwarded(source.id, "authentic text"),
        )
        .await
        .ok(StatusCode::CREATED);
    let mut replacement = serde_json::to_value(&copy.document).unwrap();
    replacement["blocks"][1]["content"][0]["text"] = json!("edited copy text");
    let edited: Message = server
        .put(&copier, &message(copy.id), &json!({"document":replacement}))
        .await
        .ok(StatusCode::OK);
    let value = serde_json::to_value(&edited.document).unwrap();
    assert!(value.to_string().contains("edited copy text"));
    assert_eq!(value["blocks"][0]["verified"], false);
    let retained: Message = server
        .get(&owner, &message(source.id))
        .await
        .ok(StatusCode::OK);
    assert!(
        serde_json::to_string(&retained.document)
            .unwrap()
            .contains("authentic text")
    );
}

#[tokio::test]
async fn a_forged_file_cannot_carry_a_source_messages_attribution() {
    let directory = Scratch::new();
    let server =
        TestServer::start_with(|config| config.attachments_directory = Some(directory.0.clone()))
            .await;
    let owner = server.owner().await;
    let copier = server.join(&owner, "Copier").await;
    let channel = channel(&server, &owner, "text").await;
    let source_file = upload(
        &server,
        &owner,
        json!({"channel_id":channel.id}),
        b"authentic attachment",
    )
    .await;
    let mut source_body = text("a real file");
    source_body["attachments"] = json!([source_file.id]);
    let path = format!("/api/v1/channels/{}/messages", channel.id);
    let source: Message = server
        .post(&owner, &path, &source_body)
        .await
        .ok(StatusCode::CREATED);
    let forged_file = upload(
        &server,
        &copier,
        json!({"channel_id":channel.id}),
        b"forged attachment",
    )
    .await;
    let mut body = forwarded(source.id, "a real file");
    body["attachments"] = json!([forged_file.id]);
    server
        .post(&copier, &path, &body)
        .await
        .error(ErrorCode::InvalidRequest);
    let real_copy = upload(
        &server,
        &copier,
        json!({"channel_id":channel.id}),
        b"authentic attachment",
    )
    .await;
    body["attachments"] = json!([real_copy.id]);
    let copied: Message = server
        .post(&copier, &path, &body)
        .await
        .ok(StatusCode::CREATED);
    assert_eq!(copied.attachments[0].sha256, source.attachments[0].sha256);
}

#[tokio::test]
async fn an_unreadable_local_source_cannot_be_forwarded() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let copier = server.join(&owner, "Copier").await;
    let target = channel(&server, &owner, "text").await;
    let workspace: junto_protocol::structure::Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"Private"}))
        .await
        .ok(StatusCode::CREATED);
    let private: junto_protocol::structure::Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({"name":"private", "kind":"text"}),
        )
        .await
        .ok(StatusCode::CREATED);
    let source = send(&server, &owner, &private, "secret source").await;
    server
        .post(
            &copier,
            &format!("/api/v1/channels/{}/messages", target.id),
            &forwarded(source.id, "forged secret"),
        )
        .await
        .error(ErrorCode::NotFound);
}

#[tokio::test]
async fn remote_origins_are_unverified_even_when_the_client_claims_verification() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let mut body = forwarded(MessageId::generate(), "a cross-server copy");
    body["document"]["blocks"][0]["server"] = json!("https://remote.example.test");
    body["document"]["blocks"][0]["verified"] = json!(true);
    let copy: Message = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &body,
        )
        .await
        .ok(StatusCode::CREATED);
    let value = serde_json::to_value(&copy.document).unwrap();
    assert_eq!(value["blocks"][0]["verified"], false);
    assert!(value.to_string().contains("a cross-server copy"));
    let legacy: Value = reqwest::Client::new()
        .get(format!("{}{}", server.base, message(copy.id)))
        .header("Junto-Event-Revision", 9)
        .bearer_auth(&owner.token)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(legacy["document"]["blocks"][0]["kind"], "quote");
    assert!(legacy.to_string().contains("Not verified"));
}
