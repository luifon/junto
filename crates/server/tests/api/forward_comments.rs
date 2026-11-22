//! The comment of a forward is a field of its own beside the forwarded content (ADR 0408).

use crate::messages_support::{channel, message, send, text};
use crate::support::{TestServer, origin};
use junto_protocol::{discovery::SearchPage, error::ErrorCode, ids::MessageId, message::Message};
use reqwest::StatusCode;
use serde_json::{Value, json};

fn paragraph(words: &str) -> Value {
    json!({"blocks":[{"kind":"paragraph", "content":[{"kind":"text", "text":words}]}]})
}

fn forwarded(id: MessageId, comment: Option<&str>) -> Value {
    let mut body = json!({"document":{"blocks":[
        {"kind":"forward", "server":origin(), "message_id":id},
        {"kind":"paragraph", "content":[{"kind":"text", "text":"submitted copy"}]}
    ]}});
    if let Some(comment) = comment {
        body["comment"] = paragraph(comment);
    }
    body
}

fn words(document: &Option<junto_protocol::message::Document>) -> String {
    serde_json::to_string(document).unwrap()
}

#[tokio::test]
async fn a_forward_keeps_its_comment_apart_and_its_author_edits_the_comment_alone() {
    let server = TestServer::start().await;
    let author = server.owner().await;
    let copier = server.join(&author, "Copier").await;
    let channel = channel(&server, &author, "text").await;
    let source = send(&server, &author, &channel, "the source text").await;
    let path = format!("/api/v1/channels/{}/messages", channel.id);
    let copy: Message = server
        .post(&copier, &path, &forwarded(source.id, Some("see this plan")))
        .await
        .ok(StatusCode::CREATED);
    assert!(words(&copy.comment).contains("see this plan"));
    assert!(!words(&copy.document).contains("see this plan"));
    assert!(words(&copy.document).contains("the source text"));
    let document = serde_json::to_value(&copy.document).unwrap();
    assert_eq!(document["blocks"][0]["verified"], true);

    // A new comment with the stored content changes the comment and keeps the verified copy.
    let edited: Message = server
        .put(
            &copier,
            &message(copy.id),
            &json!({"document":copy.document, "comment":paragraph("see this plan today")}),
        )
        .await
        .ok(StatusCode::OK);
    assert!(words(&edited.comment).contains("see this plan today"));
    assert_eq!(edited.document, copy.document);
    assert!(edited.edited_at.is_some());

    // An edit without a comment keeps it; an empty comment removes it.
    let kept: Message = server
        .put(
            &copier,
            &message(copy.id),
            &json!({"document":copy.document}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(kept.comment, edited.comment);
    let removed: Message = server
        .put(
            &copier,
            &message(copy.id),
            &json!({"document":copy.document, "comment":{"blocks":[]}}),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(removed.comment, None);
    assert_eq!(removed.document, copy.document);

    // Another person may not change the comment.
    server
        .put(
            &author,
            &message(copy.id),
            &json!({"document":copy.document, "comment":paragraph("not mine")}),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn a_comment_belongs_to_a_forward_and_has_no_origin_of_its_own() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let path = format!("/api/v1/channels/{}/messages", channel.id);
    let mut plain = text("an ordinary message");
    plain["comment"] = paragraph("a comment");
    server
        .post(&owner, &path, &plain)
        .await
        .error(ErrorCode::InvalidRequest);
    let source = send(&server, &owner, &channel, "the source text").await;
    let mut nested = forwarded(source.id, None);
    nested["comment"] = json!({"blocks":[
        {"kind":"forward", "server":origin(), "message_id":source.id}
    ]});
    server
        .post(&owner, &path, &nested)
        .await
        .error(ErrorCode::InvalidRequest);
    let ordinary = send(&server, &owner, &channel, "words").await;
    server
        .put(
            &owner,
            &message(ordinary.id),
            &json!({"document":ordinary.document, "comment":paragraph("a comment")}),
        )
        .await
        .error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn the_comment_of_a_forward_is_searched_and_leaves_with_its_deleted_message() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let source = send(&server, &owner, &channel, "the source text").await;
    let path = format!("/api/v1/channels/{}/messages", channel.id);
    let copy: Message = server
        .post(
            &owner,
            &path,
            &forwarded(source.id, Some("cobalt reminder")),
        )
        .await
        .ok(StatusCode::CREATED);
    let found: SearchPage = server
        .get(&owner, "/api/v1/search?q=cobalt")
        .await
        .ok(StatusCode::OK);
    assert_eq!(found.results.len(), 1);
    assert_eq!(found.results[0].message_id, Some(copy.id));
    assert!(found.results[0].text.starts_with("cobalt reminder"));
    server
        .delete(&owner, &message(copy.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let after: SearchPage = server
        .get(&owner, "/api/v1/search?q=cobalt")
        .await
        .ok(StatusCode::OK);
    assert!(after.results.is_empty());
}
