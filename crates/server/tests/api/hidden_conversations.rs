use crate::{messages_support::text, support::TestServer};
use junto_protocol::{
    error::ErrorCode,
    message::{Conversation, Message, MessagePage},
};
use reqwest::StatusCode;
use serde_json::{Value, json};

#[tokio::test]
async fn hiding_is_personal_keeps_history_and_a_new_message_restores_the_conversation() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let mut writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let pair: Conversation = server
        .post(
            &writer,
            "/api/v1/conversations",
            &json!({"account_id":reader.account_id}),
        )
        .await
        .ok(StatusCode::OK);
    let group: Value = server
        .post(
            &writer,
            "/api/v1/group-conversations",
            &json!({"name":"Project","account_ids":[reader.account_id,third.account_id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    let group_id = serde_json::from_value(group["id"].clone()).unwrap();
    for id in [pair.id, group_id] {
        let messages = format!("/api/v1/conversations/{id}/messages");
        let visibility = format!("/api/v1/conversations/{id}/visibility");
        let earlier: Message = server
            .post(&reader, &messages, &text("Earlier plan"))
            .await
            .ok(StatusCode::CREATED);
        server
            .put(&writer, &visibility, &json!({"hidden":true}))
            .await
            .empty(StatusCode::NO_CONTENT);
        let snapshot: Value = server
            .get(&writer, "/api/v1/snapshot")
            .await
            .ok(StatusCode::OK);
        assert!(
            snapshot["hidden_conversations"]
                .as_array()
                .unwrap()
                .contains(&json!(id))
        );
        let other: Value = server
            .get(&reader, "/api/v1/snapshot")
            .await
            .ok(StatusCode::OK);
        assert!(
            !other["hidden_conversations"]
                .as_array()
                .unwrap()
                .contains(&json!(id))
        );
        let history: MessagePage = server.get(&writer, &messages).await.ok(StatusCode::OK);
        assert!(
            history
                .messages
                .iter()
                .any(|message| message.id == earlier.id)
        );
        server
            .put(
                &reader,
                &format!("/api/v1/messages/{}", earlier.id),
                &text("Edited plan"),
            )
            .await
            .ok::<Message>(StatusCode::OK);
        server
            .put(
                &reader,
                &format!("/api/v1/messages/{}/reaction", earlier.id),
                &json!({"emoji":"👍"}),
            )
            .await
            .empty(StatusCode::NO_CONTENT);
        server
            .put(
                &writer,
                &format!("{messages}/read"),
                &json!({"position":earlier.position}),
            )
            .await
            .ok::<junto_protocol::message::MessageState>(StatusCode::OK);
        let still_hidden: Value = server
            .get(&writer, "/api/v1/snapshot")
            .await
            .ok(StatusCode::OK);
        assert!(
            still_hidden["hidden_conversations"]
                .as_array()
                .unwrap()
                .contains(&json!(id))
        );
        let another_session = server.login(&writer.key).await;
        let same_account: Value = server
            .send(
                reqwest::Method::GET,
                "/api/v1/snapshot",
                Some(another_session.token.expose()),
                None::<&Value>,
                None,
            )
            .await
            .ok(StatusCode::OK);
        assert!(
            same_account["hidden_conversations"]
                .as_array()
                .unwrap()
                .contains(&json!(id))
        );
        writer.token = another_session.token.expose().to_owned();
        server
            .put(&owner, &visibility, &json!({"hidden":true}))
            .await
            .error(ErrorCode::NotFound);
        server
            .post(&reader, &messages, &text("New plan"))
            .await
            .ok::<Message>(StatusCode::CREATED);
        let snapshot: Value = server
            .get(&writer, "/api/v1/snapshot")
            .await
            .ok(StatusCode::OK);
        assert!(
            !snapshot["hidden_conversations"]
                .as_array()
                .unwrap()
                .contains(&json!(id))
        );
    }
}

#[tokio::test]
async fn group_visibility_events_are_personal_and_do_not_reveal_messages_after_leaving() {
    use junto_protocol::events::{Event, Level};
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let group: Value = server
        .post(
            &writer,
            "/api/v1/group-conversations",
            &json!({"name":"Project","account_ids":[reader.account_id,third.account_id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    let id: junto_protocol::ids::ConversationId =
        serde_json::from_value(group["id"].clone()).unwrap();
    let mut own = server.subscribed(&reader, Level::Background).await;
    let mut peer = server.subscribed(&writer, Level::Full).await;
    let visibility = format!("/api/v1/conversations/{id}/visibility");
    server
        .put(&reader, &visibility, &json!({"hidden":true}))
        .await
        .empty(StatusCode::NO_CONTENT);
    let mine = server.traffic(&reader, &mut own).await;
    assert!(mine.iter().any(|(_,event)| matches!(event,Event::ConversationVisibilityChanged { conversation_id,hidden:true } if *conversation_id==id)));
    let theirs = server.traffic(&writer, &mut peer).await;
    assert!(
        !theirs
            .iter()
            .any(|(_, event)| matches!(event, Event::ConversationVisibilityChanged { .. }))
    );
    let cursor = server.snapshot(&reader).await.cursor;
    server
        .delete(
            &reader,
            &format!("/api/v1/group-conversations/{id}/membership"),
        )
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(
            &writer,
            &format!("/api/v1/conversations/{id}/messages"),
            &text("After the departure"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    let mut replay = server.stream(&reader).await;
    replay.subscribe(Level::Full, Some(cursor)).await;
    let replayed = server.traffic(&reader, &mut replay).await;
    assert!(!replayed.iter().any(|(_, event)| matches!(
        event,
        Event::ConversationVisibilityChanged { .. } | Event::MessageChanged { .. }
    )));
}

#[tokio::test]
async fn overlapping_visibility_and_new_messages_converge_across_sessions() {
    let server = TestServer::start().await;
    let owner = server.owner().await;
    let writer = server.join(&owner, "Writer").await;
    let mut reader = server.join(&owner, "Reader").await;
    let third = server.join(&owner, "Third").await;
    let session = server.login(&reader.key).await;
    let other_token = session.token.expose().to_owned();
    let group: Value = server
        .post(
            &writer,
            "/api/v1/group-conversations",
            &json!({"name":"Project","account_ids":[reader.account_id,third.account_id]}),
        )
        .await
        .ok(StatusCode::CREATED);
    let id = &group["id"];
    let id = id.as_str().unwrap();
    let visibility = format!("/api/v1/conversations/{id}/visibility");
    let messages = format!("/api/v1/conversations/{id}/messages");
    let mut stream = server
        .subscribed(&reader, junto_protocol::events::Level::Full)
        .await;
    let hide_body = json!({"hidden":true});
    let reopen_body = json!({"hidden":false});
    let message_body = text("Concurrent decision");
    let (hide, reopen, sent) = tokio::join!(
        server.put(&reader, &visibility, &hide_body),
        server.send(
            reqwest::Method::PUT,
            &visibility,
            Some(other_token.as_str()),
            Some(&reopen_body),
            None
        ),
        server.post(&writer, &messages, &message_body)
    );
    hide.empty(StatusCode::NO_CONTENT);
    reopen.empty(StatusCode::NO_CONTENT);
    let message: Message = sent.ok(StatusCode::CREATED);
    let events = server.traffic(&reader, &mut stream).await;
    let last = events
        .iter()
        .filter_map(|(_, event)| match event {
            junto_protocol::events::Event::ConversationVisibilityChanged {
                conversation_id,
                hidden,
            } if conversation_id.to_string() == id => Some(*hidden),
            _ => None,
        })
        .next_back()
        .unwrap();
    let first = server.snapshot(&reader).await;
    assert_eq!(
        first
            .hidden_conversations
            .iter()
            .any(|value| value.to_string() == id),
        last
    );
    reader.token = other_token;
    let second = server.snapshot(&reader).await;
    assert_eq!(second.hidden_conversations, first.hidden_conversations);
    let page: MessagePage = server.get(&reader, &messages).await.ok(StatusCode::OK);
    assert!(page.messages.iter().any(|value| value.id == message.id));
    server
        .put(&reader, &visibility, &json!({"hidden":true}))
        .await
        .empty(StatusCode::NO_CONTENT);
    server
        .post(
            &writer,
            &messages,
            &text("Later message restores visibility"),
        )
        .await
        .ok::<Message>(StatusCode::CREATED);
    assert!(
        !server
            .snapshot(&reader)
            .await
            .hidden_conversations
            .iter()
            .any(|value| value.to_string() == id)
    );
}
