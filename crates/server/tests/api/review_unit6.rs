//! Tests for the defects that the review of the text message unit found. Each test states
//! the behaviour that is expected after the fix. The numbers are those of the review.

use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::message::{Message, MessagePage, Upload};
use junto_protocol::structure::Channel;
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::calls_support::Scene;
use crate::messages::text;
use crate::support::{Member, Scratch, TestServer};

async fn text_channel(server: &TestServer, owner: &Member) -> Channel {
    let workspace = server.default_workspace(owner).await;
    server
        .post(
            owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({"name": "review", "kind": "text"}),
        )
        .await
        .ok(StatusCode::CREATED)
}

fn mention_of(member: &Member, words: &str) -> Value {
    json!({"document": {"blocks": [{"kind": "paragraph", "content": [
        {"kind": "account", "account_id": member.account_id},
        {"kind": "text", "text": words}
    ]}]}})
}

/// Defect 4: a reaction on a message that mentions an account records `message_mentioned`
/// again for that account.
#[tokio::test]
async fn a_reaction_does_not_repeat_the_mention_notice() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let mentioned = server.join(&owner, "Mentioned").await;
    let other = server.join(&owner, "Other").await;
    let channel = text_channel(&server, &owner).await;
    let mut stream = server.subscribed(&mentioned, Level::Background).await;
    let message: Message = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &mention_of(&mentioned, " look"),
        )
        .await
        .ok(StatusCode::CREATED);
    assert!(matches!(
        stream.event().await.1,
        Event::MessageMentioned { message_id, .. } if message_id == message.id
    ));
    server
        .put(
            &other,
            &format!("/api/v1/messages/{}/reaction", message.id),
            &json!({"emoji": "👍"}),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let later = server.traffic(&mentioned, &mut stream).await;
    assert!(
        later
            .iter()
            .all(|(_, event)| !matches!(event, Event::MessageMentioned { .. })),
        "{later:?}"
    );
}

/// Defect 3: the system line of a channel override carries the role and the permission
/// sets, which the override route and its event give only to `manage_channel`.
#[tokio::test]
async fn a_system_line_does_not_give_the_overrides_to_an_account_without_manage_channel() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = text_channel(&server, &owner).await;
    let everyone = server.everyone_role(&owner, workspace.id).await;
    let overrides = format!("/api/v1/channels/{}/overrides", channel.id);
    server
        .put(
            &owner,
            &format!("{overrides}/{}", everyone.id),
            &json!({"deny": ["mention_everyone"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server
        .get(&reader, &overrides)
        .await
        .error(ErrorCode::Forbidden);
    let page: MessagePage = server
        .get(
            &reader,
            &format!("/api/v1/channels/{}/messages", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    let lines = serde_json::to_string(&page.messages).unwrap();
    assert!(!lines.contains("mention_everyone"), "{lines}");
}

/// Defect 2: after the call of a voice channel that hides its occupants has ended, its
/// system lines name the people who were inside to every account that sees the channel.
#[tokio::test]
async fn the_lines_of_a_hidden_call_do_not_name_its_people_after_the_call() {
    let scene = Scene::start().await;
    let server = &scene.server;
    server
        .call_settings(
            &scene.owner,
            scene.voice.id,
            json!({"hides_occupants": true}),
        )
        .await;
    server.enter_call(&scene.ann, scene.voice.id).await;
    server.leave_call(&scene.ann).await;
    let page: MessagePage = server
        .get(
            &scene.ben,
            &format!("/api/v1/channels/{}/messages", scene.voice.id),
        )
        .await
        .ok(StatusCode::OK);
    let lines = serde_json::to_string(&page.messages).unwrap();
    assert!(
        !lines.contains(&scene.ann.account_id.to_string()),
        "{lines}"
    );
}

/// Defect 5: an author cannot edit a message that mentions an account which has left the
/// workspace since, because the edit validates every mention again.
#[tokio::test]
async fn an_edit_keeps_the_mention_of_an_account_that_left_the_workspace() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let leaver = server.join(&owner, "Leaver").await;
    let workspace = server.default_workspace(&owner).await;
    let channel = text_channel(&server, &owner).await;
    let message: Message = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &mention_of(&leaver, " frist"),
        )
        .await
        .ok(StatusCode::CREATED);
    sqlx::query("delete from workspace_members where workspace_id = $1 and account_id = $2")
        .bind(workspace.id)
        .bind(leaver.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    server
        .put(
            &owner,
            &format!("/api/v1/messages/{}", message.id),
            &json!({"document": mention_of(&leaver, " first")["document"]}),
        )
        .await
        .ok::<Message>(StatusCode::OK);
}

/// Defect 7: the two spellings of one emoji, with and without the variation selector, are
/// stored as two reactions of one account.
#[tokio::test]
async fn the_two_spellings_of_one_emoji_are_one_reaction() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = text_channel(&server, &owner).await;
    let message: Message = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &text("react"),
        )
        .await
        .ok(StatusCode::CREATED);
    let reaction = format!("/api/v1/messages/{}/reaction", message.id);
    for emoji in ["\u{2764}", "\u{2764}\u{fe0f}"] {
        server
            .put(&owner, &reaction, &json!({"emoji": emoji}))
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    let read: Message = server
        .get(&owner, &format!("/api/v1/messages/{}", message.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(read.reactions.len(), 1, "{:?}", read.reactions);
    assert_eq!(read.reactions[0].count, 1);
}

/// Defect 1: the download of an attachment has no bound for a client that takes no bytes.
/// Each such download holds one of the four places that every upload and download of the
/// server shares. The test expects the bound of the recording downloads, `download_stall`.
#[tokio::test]
async fn stalled_downloads_of_an_attachment_give_their_places_back() {
    const MEBIBYTE: usize = 1024 * 1024;
    let directory = Scratch::new();
    let server = TestServer::start_tuned(
        |config| {
            config.attachments_directory = Some(directory.0.clone());
            config.limits.recordings.reserve_bytes = 0;
            config.limits.recordings.download_stall = time::Duration::seconds(2);
        },
        Some(16 * 1024),
    )
    .await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let writer = server.join(&owner, "Writer").await;
    let channel = text_channel(&server, &owner).await;
    let file = vec![7u8; 16 * MEBIBYTE];
    let start = |bytes: &[u8]| {
        json!({
            "channel_id": channel.id,
            "name": "file.bin",
            "bytes": bytes.len(),
            "sha256": junto_server::messages::uploads::hash(bytes),
        })
    };
    let upload: Upload = server
        .post(&owner, "/api/v1/attachments", &start(&file))
        .await
        .ok(StatusCode::CREATED);
    let path = format!("/api/v1/attachments/{}", upload.id);
    for (index, chunk) in file.chunks(MEBIBYTE).enumerate() {
        let offset = index * MEBIBYTE;
        server
            .put_bytes(
                &owner,
                &format!("{path}/chunks/{offset}"),
                None,
                chunk.to_vec(),
            )
            .await
            .ok::<Upload>(StatusCode::OK);
    }
    server
        .post(&owner, &format!("{path}/complete"), &json!({}))
        .await
        .ok::<Upload>(StatusCode::OK);
    server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &json!({"document": {"blocks": []}, "attachments": [upload.id]}),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);

    let client = reqwest::Client::new();
    let mut held = Vec::new();
    for _ in 0..2 {
        let response = client
            .get(format!("{}{path}/file", server.base))
            .bearer_auth(&reader.token)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        held.push(response);
    }
    let refusal = client
        .get(format!("{}{path}/file", server.base))
        .bearer_auth(&reader.token)
        .send()
        .await
        .unwrap();
    assert_eq!(refusal.status(), StatusCode::SERVICE_UNAVAILABLE);
    tokio::time::timeout(
        std::time::Duration::from_secs(20),
        server.state.attachment_downloads.wait_idle(),
    )
    .await
    .expect("stalled downloads released their slots");
    let resumed = client
        .get(format!("{}{path}/file", server.base))
        .bearer_auth(&reader.token)
        .header("Range", "bytes=0-15")
        .send()
        .await
        .unwrap();
    assert_eq!(resumed.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(resumed.bytes().await.unwrap().as_ref(), &file[..16]);

    let small = vec![1u8; 16];
    let second: Upload = server
        .post(&writer, "/api/v1/attachments", &start(&small))
        .await
        .ok(StatusCode::CREATED);
    server
        .put_bytes(
            &writer,
            &format!("/api/v1/attachments/{}/chunks/0", second.id),
            None,
            small,
        )
        .await
        .ok::<Upload>(StatusCode::OK);
    drop(held);
}

/// Defect 8: the system line of an account's own action counts as unread for that account.
#[tokio::test]
async fn the_line_of_an_own_action_is_not_unread_for_its_actor() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = text_channel(&server, &owner).await;
    let state = server
        .snapshot(&owner)
        .await
        .messages
        .into_iter()
        .find(|state| {
            matches!(
                state.place,
                junto_protocol::message::MessagePlace::Channel { channel_id, .. }
                    if channel_id == channel.id
            )
        })
        .unwrap();
    assert_eq!(state.unread, 0);
}
