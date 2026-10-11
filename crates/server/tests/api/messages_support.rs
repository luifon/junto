//! Shared fixtures for message documents, places and complete attachment uploads.
use crate::support::{Member, TestServer};
use junto_protocol::{
    ids::MessageId,
    message::{Message, Upload},
    structure::Channel,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

pub fn text(words: &str) -> Value {
    json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"text","text":words}]}]}})
}
pub async fn channel(server: &TestServer, owner: &Member, kind: &str) -> Channel {
    let workspace = server.default_workspace(owner).await;
    server
        .post(
            owner,
            &format!("/api/v1/workspaces/{}/channels", workspace.id),
            &json!({"name":"messages","kind":kind}),
        )
        .await
        .ok(StatusCode::CREATED)
}
pub fn message(id: MessageId) -> String {
    format!("/api/v1/messages/{id}")
}
pub async fn send(server: &TestServer, author: &Member, channel: &Channel, words: &str) -> Message {
    server
        .post(
            author,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &text(words),
        )
        .await
        .ok(StatusCode::CREATED)
}
pub async fn upload(server: &TestServer, author: &Member, place: Value, bytes: &[u8]) -> Upload {
    let mut body = place;
    body["name"] = json!("file.bin");
    body["bytes"] = json!(bytes.len());
    body["sha256"] = json!(junto_server::messages::uploads::hash(bytes));
    let upload: Upload = server
        .post(author, "/api/v1/attachments", &body)
        .await
        .ok(StatusCode::CREATED);
    for (index, chunk) in bytes.chunks(1024 * 1024).enumerate() {
        server
            .put_bytes(
                author,
                &format!(
                    "/api/v1/attachments/{}/chunks/{}",
                    upload.id,
                    index * 1024 * 1024
                ),
                None,
                chunk.to_vec(),
            )
            .await
            .ok::<Upload>(StatusCode::OK);
    }
    server
        .post(
            author,
            &format!("/api/v1/attachments/{}/complete", upload.id),
            &json!({}),
        )
        .await
        .ok(StatusCode::OK)
}

pub struct Files {
    pub directory: crate::support::Scratch,
    pub server: TestServer,
    pub owner: Member,
    pub channel: Channel,
}
impl Files {
    pub async fn open() -> Self {
        let directory = crate::support::Scratch::new();
        let server = TestServer::start_with(|config| {
            config.attachments_directory = Some(directory.0.clone());
            config.limits.recordings.reserve_bytes = 0;
        })
        .await;
        let owner = server.owner().await;
        let channel = channel(&server, &owner, "text").await;
        Self {
            directory,
            server,
            owner,
            channel,
        }
    }
    pub async fn start(&self, bytes: &[u8]) -> Upload {
        self.server.post(&self.owner,"/api/v1/attachments",&json!({"channel_id":self.channel.id,"name":"file.bin","bytes":bytes.len(),"sha256":junto_server::messages::uploads::hash(bytes)})).await.ok(StatusCode::CREATED)
    }
    pub async fn upload(&self, bytes: &[u8]) -> Upload {
        upload(
            &self.server,
            &self.owner,
            json!({"channel_id":self.channel.id}),
            bytes,
        )
        .await
    }
    pub async fn attach(&self, ids: &[junto_protocol::ids::AttachmentId]) -> Message {
        self.server
            .post(
                &self.owner,
                &format!("/api/v1/channels/{}/messages", self.channel.id),
                &json!({"document":{"blocks":[]},"attachments":ids}),
            )
            .await
            .ok(StatusCode::CREATED)
    }
}
