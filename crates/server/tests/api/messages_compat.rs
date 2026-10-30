use crate::messages_support::{channel, send};
use crate::support::TestServer;
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{AccountId, MessageId, WorkspaceRoleId};
use junto_protocol::message::{Mark, Message};
use junto_protocol::structure::{Channel, Workspace};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum OldInline {
    Text {
        text: String,
        #[serde(default)]
        marks: Vec<Mark>,
    },
    Code {
        text: String,
    },
    Link {
        text: String,
        url: String,
    },
    Account {
        account_id: AccountId,
    },
    Role {
        role_id: WorkspaceRoleId,
    },
    Everyone,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OldCheckItem {
    checked: bool,
    content: Vec<OldInline>,
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum OldBlock {
    Paragraph {
        content: Vec<OldInline>,
    },
    Quote {
        content: Vec<OldInline>,
    },
    Code {
        text: String,
        language: Option<String>,
    },
    List {
        ordered: bool,
        items: Vec<Vec<OldInline>>,
    },
    Checklist {
        items: Vec<OldCheckItem>,
    },
}
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OldDocument {
    blocks: Vec<OldBlock>,
}
#[derive(Debug, Deserialize, Serialize)]
struct OldMessage {
    id: MessageId,
    document: Option<OldDocument>,
}
#[derive(Debug, Deserialize, Serialize)]
struct OldPage {
    messages: Vec<OldMessage>,
    has_more: bool,
}

#[tokio::test]
async fn expanded_legacy_labels_keep_every_page_inside_the_transport_bound() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    server
        .patch(&owner, "/api/v1/account", &json!({"name":"🪨".repeat(64)}))
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
    let channel = channel(&server, &owner, "text").await;
    let path = format!("/api/v1/channels/{}/messages?limit=200", channel.id);
    let initial: OldPage = server.get(&owner, &path).await.ok(StatusCode::OK);
    let mut expected: Vec<_> = initial.messages.iter().map(|message| message.id).collect();
    let document = json!({"blocks":[{"kind":"heading", "level":1,
        "content":vec![json!({"kind":"account", "account_id":owner.account_id}); 200]}]});
    for _ in 0..90 {
        let message: Message = server
            .post(
                &owner,
                &format!("/api/v1/channels/{}/messages", channel.id),
                &json!({"document":document}),
            )
            .await
            .ok(StatusCode::CREATED);
        expected.push(message.id);
    }
    let mut cursor = None;
    let mut seen = Vec::new();
    let mut pages = 0;
    for _ in 0..5 {
        let mut target = format!("{}{path}", server.base);
        if let Some(cursor) = cursor {
            target.push_str(&format!("&before_message={cursor}"));
        }
        let response = reqwest::Client::new()
            .get(target)
            .bearer_auth(&owner.token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.bytes().await.unwrap();
        assert!(bytes.len() < 4 * 1024 * 1024);
        let page: OldPage = serde_json::from_slice(&bytes).unwrap();
        cursor = page.messages.first().map(|message| message.id);
        seen.extend(page.messages.iter().map(|message| message.id));
        pages += 1;
        if !page.has_more {
            break;
        }
    }
    assert!(pages > 1);
    seen.sort();
    expected.sort();
    assert_eq!(seen, expected);
}

#[tokio::test]
async fn legacy_labels_respect_current_profile_and_channel_visibility() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let public = channel(&server, &owner, "text").await;
    let private: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"restricted"}))
        .await
        .ok(StatusCode::CREATED);
    let subject = server
        .join_into(
            &owner,
            "Old public name",
            &[public.workspace_id, private.id],
        )
        .await;
    let hidden: Channel = server
        .post(
            &owner,
            &format!("/api/v1/workspaces/{}/channels", private.id),
            &json!({"kind":"text","name":"private-title"}),
        )
        .await
        .ok(StatusCode::CREATED);
    let message: Message = server.post(&owner, &format!("/api/v1/channels/{}/messages", public.id), &json!({"document":{"blocks":[
        {"kind":"heading", "level":1, "content":[{"kind":"account", "account_id":subject.account_id}, {"kind":"channel", "channel_id":hidden.id}]}
    ]}})).await.ok(StatusCode::CREATED);
    server
        .post(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/removal",
                public.workspace_id, subject.account_id
            ),
            &json!({}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .patch(
            &subject,
            "/api/v1/account",
            &json!({"name":"New private name"}),
        )
        .await
        .ok::<serde_json::Value>(StatusCode::OK);
    server
        .get(
            &reader,
            &format!("/api/v1/accounts/{}/profile", subject.account_id),
        )
        .await
        .error(ErrorCode::NotFound);
    let response = reqwest::Client::new()
        .get(format!("{}/api/v1/messages/{}", server.base, message.id))
        .bearer_auth(&reader.token)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let decoded: OldMessage = response.json().await.unwrap();
    let text = serde_json::to_string(&decoded).unwrap();
    assert!(
        text.contains("Unknown person") && text.contains("channel unavailable"),
        "{text}"
    );
    assert!(!text.contains("New private name") && !text.contains("private-title"));
}

#[tokio::test]
async fn old_v1_decoders_read_new_documents_and_surrounding_history() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let path = format!("/api/v1/channels/{}/messages", channel.id);
    let before = send(&server, &owner, &channel, "ordinary before").await;
    let formatted: Message = server.post(&owner, &path, &json!({"document":{"blocks":[
        {"kind":"heading", "level":1, "content":[{"kind":"text", "text":"Visible heading", "marks":["bold"]}]},
        {"kind":"small", "content":[{"kind":"text", "text":"Visible small"}]},
        {"kind":"paragraph", "content":[
            {"kind":"channel", "channel_id":channel.id},
            {"kind":"spoiler", "content":[{"kind":"text", "text":"hidden secret"}]}
        ]}
    ]}})).await.ok(StatusCode::CREATED);
    let forwarded: Message = server.post(&owner, &path, &json!({"document":{"blocks":[
        {"kind":"forward", "server":"https://source.example.test", "message_id":formatted.id},
        {"kind":"paragraph", "content":[{"kind":"text", "text":"visible copied body"}]}
    ]}})).await.ok(StatusCode::CREATED);
    let after = send(&server, &owner, &channel, "ordinary after").await;
    for revision in [None, Some(2), Some(3), Some(8)] {
        let mut request = reqwest::Client::new()
            .get(format!("{}{path}", server.base))
            .bearer_auth(&owner.token);
        if let Some(revision) = revision {
            request = request.header("Junto-Event-Revision", revision.to_string());
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.bytes().await.unwrap();
        let page = serde_json::from_slice::<OldPage>(&bytes);
        assert!(page.is_ok(), "revision {revision:?}: {page:?}");
        let page = page.unwrap();
        assert!(!page.has_more);
        for id in [before.id, formatted.id, forwarded.id, after.id] {
            assert!(page.messages.iter().any(|message| message.id == id));
        }
        let text = serde_json::to_string(&page).unwrap();
        for visible in [
            "ordinary before",
            "Visible heading",
            "Visible small",
            "visible copied body",
            "ordinary after",
            "Forwarded",
            "https://source.example.test",
            "#messages",
            "Spoiler",
        ] {
            assert!(text.contains(visible), "{visible} is missing: {text}");
        }
        assert!(!text.contains("hidden secret"));
        let copy = page
            .messages
            .iter()
            .find(|message| message.id == forwarded.id)
            .unwrap();
        assert!(matches!(
            &copy.document.as_ref().unwrap().blocks[0],
            OldBlock::Quote { .. }
        ));
    }
    let response = reqwest::Client::new().put(format!("{}/api/v1/messages/{}", server.base, formatted.id))
        .bearer_auth(&owner.token).json(&json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"text","text":"a lossy old edit"}]}]}})).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let retained: Message = server
        .get(&owner, &format!("/api/v1/messages/{}", formatted.id))
        .await
        .ok(StatusCode::OK);
    assert!(matches!(
        retained.document.unwrap().blocks[0],
        junto_protocol::message::Block::Heading { .. }
    ));
}
