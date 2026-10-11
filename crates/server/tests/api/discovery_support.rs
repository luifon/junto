//! Shared API fixtures for threads, notices, search and policy refusals.
pub use crate::messages_support::*;
pub use crate::support::{Member, TestServer};
pub use junto_protocol::{
    discovery::*,
    error::ErrorCode,
    events::{Event, Level},
    ids::*,
    message::*,
    structure::Channel,
};
pub use reqwest::StatusCode;
pub use serde_json::{Value, json};
pub async fn reply(server: &TestServer, author: &Member, root: MessageId, words: &str) -> Message {
    server
        .post(author, &format!("{}/thread", message(root)), &text(words))
        .await
        .ok(StatusCode::CREATED)
}

pub async fn thread(server: &TestServer, reader: &Member, id: MessageId) -> ThreadPage {
    server
        .get(reader, &format!("{}/thread", message(id)))
        .await
        .ok(StatusCode::OK)
}

pub async fn fork(server: &TestServer, actor: &Member, id: MessageId, kind: &str) -> Channel {
    server
        .post(
            actor,
            &format!("{}/fork", message(id)),
            &json!({"name":"fork","kind":kind}),
        )
        .await
        .ok(StatusCode::CREATED)
}

pub async fn notices(server: &TestServer, reader: &Member) -> Vec<Notification> {
    server
        .get(reader, "/api/v1/notifications")
        .await
        .ok::<NotificationPage>(StatusCode::OK)
        .notifications
}

pub async fn settings(
    server: &TestServer,
    reader: &Member,
    id: impl std::fmt::Display,
    body: Value,
) {
    server
        .put(
            reader,
            &format!("/api/v1/notification-settings/{id}"),
            &body,
        )
        .await
        .ok::<NotificationSettings>(StatusCode::OK);
}

pub async fn search(server: &TestServer, reader: &Member, query: &str) -> SearchPage {
    server
        .get(reader, &format!("/api/v1/search?q={query}"))
        .await
        .ok(StatusCode::OK)
}

pub async fn mention(
    server: &TestServer,
    author: &Member,
    channel: &Channel,
    reader: &Member,
) -> Message {
    server.post(author,&format!("/api/v1/channels/{}/messages",channel.id),&json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"text","text":"quartz"},{"kind":"account","account_id":reader.account_id}]}]}})).await.ok(StatusCode::CREATED)
}

pub async fn events(
    server: &TestServer,
    reader: &Member,
    cursor: junto_protocol::events::Cursor,
    level: Level,
) -> Vec<Event> {
    let mut stream = server.stream(reader).await;
    stream.subscribe(level, Some(cursor)).await;
    server
        .traffic(reader, &mut stream)
        .await
        .into_iter()
        .map(|(_, event)| event)
        .collect()
}
