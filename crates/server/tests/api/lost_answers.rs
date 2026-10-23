//! A change whose COMMIT PostgreSQL ran and whose answer did not reach the server (ADR 0009).

use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::MessageId;
use junto_protocol::message::{Message, MessagePage};
use reqwest::StatusCode;

use crate::messages_support::{channel, send, text};
use crate::postgres_proxy::{Cut, PostgresProxy, Stop};
use crate::support::TestServer;

/// The server cannot tell a COMMIT whose answer was lost from a COMMIT that failed. It
/// answers `unavailable`, which does not claim that nothing changed; the change is stored
/// and every reader sees it, its event included, and the next request of the process works.
#[tokio::test]
async fn a_change_whose_commit_answer_is_lost_is_unavailable_and_stays_made() {
    let direct = TestServer::start().await;
    let owner = direct.owner().await;
    let reader = direct.join(&owner, "Reader").await;
    let room = channel(&direct, &owner, "text").await;
    let proxy = PostgresProxy::start(direct.database_options()).await;
    let server = direct.through(&proxy).await;
    let mut stream = direct.subscribed(&reader, Level::Full).await;
    let path = format!("/api/v1/channels/{}/messages", room.id);

    let struck = proxy.arm(Stop::at("commit", Cut::AfterAnswer).after("insert into messages"));
    let reply = server.post(&owner, &path, &text("kept")).await;
    struck.wait().await;
    reply.error(ErrorCode::Unavailable);
    assert!(reply.retry_after.is_none());

    let page: MessagePage = direct.get(&reader, &path).await.ok(StatusCode::OK);
    let stored: Vec<MessageId> = page
        .messages
        .iter()
        .filter(|message| message.document.is_some())
        .map(|message| message.id)
        .collect();
    assert_eq!(stored.len(), 1, "{page:?}");
    loop {
        match stream.event().await {
            (_, Event::MessageChanged { message_id, .. }) if message_id == stored[0] => break,
            _ => {}
        }
    }

    let next: Message = send(&server, &owner, &room, "after").await;
    let page: MessagePage = direct.get(&reader, &path).await.ok(StatusCode::OK);
    assert_eq!(
        page.messages.last().map(|message| message.id),
        Some(next.id)
    );
}
