//! Formatting, mention changes, reaction events and read positions.
use crate::messages_support::*;
use crate::support::TestServer;
use junto_protocol::{
    WorkspacePermission,
    error::ErrorCode,
    events::{Event, Level},
    message::*,
    structure::Workspace,
};
use reqwest::StatusCode;
use serde_json::{Value, json};

fn mentions(nodes: Vec<Value>) -> Value {
    json!({"document":{"blocks":[{"kind":"paragraph","content":nodes}]}})
}

#[tokio::test]
async fn new_mentions_must_belong_to_the_place_and_everyone_needs_permission_on_edit() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let workspace: Workspace = server
        .post(&owner, "/api/v1/workspaces", &json!({"name":"other"}))
        .await
        .ok(StatusCode::CREATED);
    let outsider = server.join_into(&owner, "Outsider", &[workspace.id]).await;
    let role = server.everyone_role(&owner, workspace.id).await;
    for node in [
        json!({"kind":"account","account_id":outsider.account_id}),
        json!({"kind":"role","role_id":role.id}),
    ] {
        server
            .post(
                &author,
                &format!("/api/v1/channels/{}/messages", channel.id),
                &mentions(vec![node]),
            )
            .await
            .error(ErrorCode::InvalidRequest);
    }
    let conversation: Conversation = server
        .post(
            &author,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    server
        .post(
            &author,
            &format!("/api/v1/conversations/{}/messages", conversation.id),
            &mentions(vec![
                json!({"kind":"account","account_id":owner.account_id}),
            ]),
        )
        .await
        .error(ErrorCode::InvalidRequest);
    let sent = send(&server, &author, &channel, "text").await;
    server
        .put(
            &author,
            &message(sent.id),
            &mentions(vec![json!({"kind":"everyone"})]),
        )
        .await
        .error(ErrorCode::Forbidden);
}

#[tokio::test]
async fn role_and_everyone_mentions_reach_readers_once_and_never_the_author_or_hidden_accounts() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let hidden = server.join(&owner, "Hidden").await;
    let channel = channel(&server, &owner, "text").await;
    let role = server
        .grant_workspace(
            &owner,
            &reader,
            channel.workspace_id,
            &[WorkspacePermission::ManageMessages],
        )
        .await;
    let own_role = server
        .grant_workspace(&owner, &hidden, channel.workspace_id, &[])
        .await;
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", channel.id, own_role),
            &json!({"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    let mut reader_stream = server.subscribed(&reader, Level::Background).await;
    let mut author_stream = server.subscribed(&owner, Level::Background).await;
    let mut hidden_stream = server.subscribed(&hidden, Level::Background).await;
    for node in [
        json!({"kind":"role","role_id":role}),
        json!({"kind":"everyone"}),
    ] {
        let sent: Message = server
            .post(
                &owner,
                &format!("/api/v1/channels/{}/messages", channel.id),
                &mentions(vec![node.clone()]),
            )
            .await
            .ok(StatusCode::CREATED);
        assert!(
            matches!(reader_stream.event().await.1,Event::MessageMentioned{message_id,..} if message_id==sent.id)
        );
        assert!(
            matches!(reader_stream.event().await.1,Event::NotificationChanged{message_id} if message_id==sent.id)
        );
        server
            .put(
                &owner,
                &message(sent.id),
                &mentions(vec![node, json!({"kind":"text","text":" edited"})]),
            )
            .await
            .ok::<Message>(StatusCode::OK);
        assert!(server.traffic(&reader, &mut reader_stream).await.is_empty());
    }
    let own_events = server.traffic(&owner, &mut author_stream).await;
    assert_eq!(own_events.len(), 2);
    assert!(
        own_events
            .iter()
            .all(|(_, event)| matches!(event, Event::MessageRead { .. }))
    );
    assert!(server.traffic(&hidden, &mut hidden_stream).await.is_empty());
    let state: MessageState = server
        .get(
            &reader,
            &format!("/api/v1/channels/{}/messages/read", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(state.mentions, 2);
}

#[tokio::test]
async fn an_edit_notifies_only_the_newly_mentioned_account() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let first = server.join(&owner, "First").await;
    let second = server.join(&owner, "Second").await;
    let channel = channel(&server, &owner, "text").await;
    let mut first_stream = server.subscribed(&first, Level::Background).await;
    let mut second_stream = server.subscribed(&second, Level::Background).await;
    let first_node = json!({"kind":"account","account_id":first.account_id});
    let second_node = json!({"kind":"account","account_id":second.account_id});
    let sent: Message = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &mentions(vec![first_node.clone()]),
        )
        .await
        .ok(StatusCode::CREATED);
    assert!(matches!(
        first_stream.event().await.1,
        Event::MessageMentioned { .. }
    ));
    assert!(
        matches!(first_stream.event().await.1,Event::NotificationChanged{message_id} if message_id==sent.id)
    );
    server
        .put(
            &owner,
            &message(sent.id),
            &mentions(vec![first_node, second_node]),
        )
        .await
        .ok::<Message>(StatusCode::OK);
    assert!(
        matches!(second_stream.event().await.1,Event::MessageMentioned{message_id,..} if message_id==sent.id)
    );
    assert!(
        matches!(second_stream.event().await.1,Event::NotificationChanged{message_id} if message_id==sent.id)
    );
    assert!(server.traffic(&first, &mut first_stream).await.is_empty());
}

#[tokio::test]
async fn reaction_changes_have_one_notice_and_repeated_removal_has_none() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let sent = send(&server, &owner, &channel, "react").await;
    let mut stream = server.subscribed(&reader, Level::Full).await;
    let route = format!("{}/reaction", message(sent.id));
    for add in [true, false] {
        if add {
            server
                .put(&reader, &route, &json!({"emoji":"👍"}))
                .await
                .empty(StatusCode::NO_CONTENT);
        } else {
            server
                .delete(&reader, &format!("{route}/👍"))
                .await
                .empty(StatusCode::NO_CONTENT);
        }
        let events = server.traffic(&reader, &mut stream).await;
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0].1,Event::MessageChanged{message_id,deleted:false,..} if message_id==sent.id)
        );
    }
    server
        .delete(&reader, &format!("{route}/👍"))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(server.traffic(&reader, &mut stream).await.is_empty());
}

#[tokio::test]
async fn reactions_allow_twenty_distinct_emoji_and_refuse_the_next() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let sent = send(&server, &owner, &channel, "react").await;
    let emojis = [
        "😀", "😃", "😄", "😁", "😆", "😅", "😂", "🤣", "😊", "😇", "🙂", "🙃", "😉", "😌", "😍",
        "🥰", "😘", "😗", "😙", "😚",
    ];
    for emoji in emojis {
        server
            .put(
                &owner,
                &format!("{}/reaction", message(sent.id)),
                &json!({"emoji":emoji}),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
    }
    server
        .put(
            &owner,
            &format!("{}/reaction", message(sent.id)),
            &json!({"emoji":"👍"}),
        )
        .await
        .error(ErrorCode::LimitReached);
    let read: Message = server
        .get(&owner, &message(sent.id))
        .await
        .ok(StatusCode::OK);
    assert_eq!(read.reactions.len(), 20);
}

#[tokio::test]
async fn edits_and_deletions_publish_once_and_an_unchanged_edit_publishes_nothing() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let sent = send(&server, &owner, &channel, "first").await;
    let mut stream = server.subscribed(&reader, Level::Full).await;
    server
        .put(&owner, &message(sent.id), &text("first"))
        .await
        .ok::<Message>(StatusCode::OK);
    assert!(server.traffic(&reader, &mut stream).await.is_empty());
    server
        .put(&owner, &message(sent.id), &text("second"))
        .await
        .ok::<Message>(StatusCode::OK);
    let events = server.traffic(&reader, &mut stream).await;
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].1,
        Event::MessageChanged { deleted: false, .. }
    ));
    server
        .delete(&owner, &message(sent.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    let events = server.traffic(&reader, &mut stream).await;
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0].1,
        Event::MessageChanged { deleted: true, .. }
    ));
}

#[tokio::test]
async fn read_positions_reach_the_accounts_other_connections_and_own_messages_are_not_unread() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let other_device = crate::devices::link_second_device(&server, &reader).await;
    let mut stream = server.subscribed(&other_device, Level::Background).await;
    let own = send(&server, &reader, &channel, "own").await;
    assert!(
        matches!(stream.event().await.1,Event::MessageRead{position,..} if position==own.position)
    );
    let state: MessageState = server
        .get(
            &reader,
            &format!("/api/v1/channels/{}/messages/read", channel.id),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(state.unread, 0);
    assert_eq!(state.last_read, own.position);
    let other = send(&server, &owner, &channel, "next unread").await;
    server
        .put(
            &reader,
            &format!("/api/v1/channels/{}/messages/read", channel.id),
            &json!({"position":other.position}),
        )
        .await
        .ok::<MessageState>(StatusCode::OK);
    assert!(
        matches!(stream.event().await.1,Event::MessageRead{position,..} if position==other.position)
    );
    server
        .put(
            &reader,
            &format!("/api/v1/channels/{}/messages/read", channel.id),
            &json!({"position":other.position}),
        )
        .await
        .ok::<MessageState>(StatusCode::OK);
    assert!(server.traffic(&other_device, &mut stream).await.is_empty());
}

#[tokio::test]
async fn history_defaults_to_fifty_and_reports_the_end_and_empty_conversations() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let conversation: Conversation = server
        .post(
            &owner,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let route = format!("/api/v1/conversations/{}/messages", conversation.id);
    let empty: MessagePage = server.get(&reader, &route).await.ok(StatusCode::OK);
    assert!(empty.messages.is_empty());
    assert!(!empty.has_more);
    for number in 0..51 {
        server
            .post(&owner, &route, &text(&number.to_string()))
            .await
            .ok::<Message>(StatusCode::CREATED);
    }
    let page: MessagePage = server.get(&reader, &route).await.ok(StatusCode::OK);
    assert_eq!(page.messages.len(), 50);
    assert!(page.has_more);
    let end: MessagePage = server
        .get(
            &reader,
            &format!("{route}?before={}", page.messages[0].position),
        )
        .await
        .ok(StatusCode::OK);
    assert_eq!(end.messages.len(), 1);
    assert!(!end.has_more);
    let end: MessagePage = server
        .get(
            &reader,
            &format!("{route}?after={}", page.messages.last().unwrap().position),
        )
        .await
        .ok(StatusCode::OK);
    assert!(end.messages.is_empty());
    assert!(!end.has_more);
}

#[tokio::test]
async fn documents_refuse_unknown_fields_and_accept_http_and_mailto_links() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let channel = channel(&server, &owner, "text").await;
    let route = format!("/api/v1/channels/{}/messages", channel.id);
    let mut unknown = text("unknown");
    unknown["document"]["surprise"] = json!(true);
    server
        .post(&owner, &route, &unknown)
        .await
        .error(ErrorCode::InvalidRequest);
    for url in ["http://example.test/path", "mailto:person@example.test"] {
        server
            .post(
                &owner,
                &route,
                &mentions(vec![json!({"kind":"link","text":"link","url":url})]),
            )
            .await
            .ok::<Message>(StatusCode::CREATED);
    }
    server.post(&owner,&route,&mentions(vec![json!({"kind":"link","text":"link","url":format!("http://example.test/{}","x".repeat(2048))})])).await.error(ErrorCode::InvalidRequest);
}

#[tokio::test]
async fn editing_keeps_an_existing_deleted_role_mention() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let role = server
        .grant_workspace(
            &owner,
            &reader,
            channel.workspace_id,
            &[WorkspacePermission::ManageMessages],
        )
        .await;
    let node = json!({"kind":"role","role_id":role});
    let sent: Message = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &mentions(vec![node.clone()]),
        )
        .await
        .ok(StatusCode::CREATED);
    sqlx::query("delete from member_roles where role_id=$1")
        .bind(role)
        .execute(&server.db)
        .await
        .unwrap();
    sqlx::query("delete from workspace_roles where id=$1")
        .bind(role)
        .execute(&server.db)
        .await
        .unwrap();
    server
        .put(
            &owner,
            &message(sent.id),
            &mentions(vec![node, json!({"kind":"text","text":" corrected"})]),
        )
        .await
        .ok::<Message>(StatusCode::OK);
}

#[tokio::test]
async fn an_unchanged_role_mention_does_not_notify_someone_who_joined_the_role_later() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let first = server.join(&owner, "First").await;
    let later = server.join(&owner, "Later").await;
    let channel = channel(&server, &owner, "text").await;
    let role = server
        .grant_workspace(
            &owner,
            &first,
            channel.workspace_id,
            &[WorkspacePermission::ManageMessages],
        )
        .await;
    let node = json!({"kind":"role","role_id":role});
    let sent: Message = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &mentions(vec![node.clone()]),
        )
        .await
        .ok(StatusCode::CREATED);
    server
        .put_empty(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{}",
                channel.workspace_id, later.account_id, role
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let mut stream = server.subscribed(&later, Level::Background).await;
    server
        .put(
            &owner,
            &message(sent.id),
            &mentions(vec![node, json!({"kind":"text","text":" corrected"})]),
        )
        .await
        .ok::<Message>(StatusCode::OK);
    assert!(server.traffic(&later, &mut stream).await.is_empty());
    let route = format!("/api/v1/channels/{}/messages/read", channel.id);
    let state: MessageState = server.get(&later, &route).await.ok(StatusCode::OK);
    assert_eq!(state.mentions, 0);
    server
        .delete(
            &owner,
            &format!(
                "/api/v1/workspaces/{}/members/{}/roles/{}",
                channel.workspace_id, first.account_id, role
            ),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    let state: MessageState = server.get(&first, &route).await.ok(StatusCode::OK);
    assert_eq!(state.mentions, 1);
}

#[tokio::test]
async fn builtin_everyone_role_requires_everyone_permission_on_send_and_edit() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let author = server.join(&owner, "Author").await;
    let channel = channel(&server, &owner, "text").await;
    let role = server.everyone_role(&owner, channel.workspace_id).await;
    let body = mentions(vec![json!({"kind":"role","role_id":role.id})]);
    server
        .post(
            &author,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &body,
        )
        .await
        .error(ErrorCode::Forbidden);
    let sent = send(&server, &author, &channel, "text").await;
    server
        .put(&author, &message(sent.id), &body)
        .await
        .error(ErrorCode::Forbidden);
    server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &body,
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
}

#[tokio::test]
async fn losing_channel_access_hides_unread_mentions_and_replayed_notices() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let reader = server.join(&owner, "Reader").await;
    let channel = channel(&server, &owner, "text").await;
    let role = server
        .grant_workspace(
            &owner,
            &reader,
            channel.workspace_id,
            &[WorkspacePermission::ManageMessages],
        )
        .await;
    let cursor = server.snapshot(&reader).await.cursor;
    let sent: Message = server
        .post(
            &owner,
            &format!("/api/v1/channels/{}/messages", channel.id),
            &mentions(vec![json!({"kind":"role","role_id":role})]),
        )
        .await
        .ok(StatusCode::CREATED);
    let route = format!("/api/v1/channels/{}/messages/read", channel.id);
    let state: MessageState = server.get(&reader, &route).await.ok(StatusCode::OK);
    assert_eq!(state.mentions, 1);
    server
        .put(
            &owner,
            &format!("/api/v1/channels/{}/overrides/{}", channel.id, role),
            &json!({"allow":[],"deny":["view_channel"]}),
        )
        .await
        .ok::<Value>(StatusCode::OK);
    server.get(&reader, &route).await.error(ErrorCode::NotFound);
    let snapshot = server.snapshot(&reader).await;
    assert!(
        !snapshot
            .messages
            .iter()
            .any(|state| state.place == sent.place)
    );
    let mut replay = server.stream(&reader).await;
    replay.subscribe(Level::Background, Some(cursor)).await;
    assert!(server.traffic(&reader, &mut replay).await.iter().all(
        |(_, event)| !matches!(event,Event::MessageMentioned{message_id,..} if *message_id==sent.id)
    ));
}

#[tokio::test]
async fn every_authored_message_route_advances_read_with_the_send_transaction() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let peer = server.join(&owner, "Peer").await;
    let text_channel = channel(&server, &owner, "text").await;
    let voice_channel = channel(&server, &owner, "voice").await;
    let conversation: Conversation = server
        .post(
            &owner,
            "/api/v1/conversations",
            &json!({"account_id":peer.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    for route in [
        format!("/api/v1/channels/{}/messages", text_channel.id),
        format!("/api/v1/channels/{}/messages", voice_channel.id),
        format!("/api/v1/conversations/{}/messages", conversation.id),
    ] {
        let root: Message = server
            .post(&peer, &route, &text("earlier unread"))
            .await
            .ok(StatusCode::CREATED);
        let read_route = format!("{route}/read");
        for reply in [None, Some(root.id)] {
            let mut body = text("own message");
            body["reply_to"] = json!(reply);
            let sent: Message = server
                .post(&owner, &route, &body)
                .await
                .ok(StatusCode::CREATED);
            let state: MessageState = server.get(&owner, &read_route).await.ok(StatusCode::OK);
            assert_eq!(state.last_read, sent.position);
            assert_eq!(state.unread, 0);
            body["attachments"] = json!([junto_protocol::ids::AttachmentId::generate()]);
            server
                .post(&owner, &route, &body)
                .await
                .error(ErrorCode::NotFound);
            let after: MessageState = server.get(&owner, &read_route).await.ok(StatusCode::OK);
            assert_eq!(after.last_read, state.last_read);
            assert_eq!(after.latest, state.latest);
        }
    }
}
