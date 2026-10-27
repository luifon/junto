use super::*;
use crate::discovery_support::mention;
use junto_protocol::events::Event;
use serde_json::json;

#[tokio::test]
async fn mention_replay_hides_old_messages_and_keeps_new_live_mentions() {
    let (server, owner, reader, channel, _) = denied().await;
    let cursor = server.snapshot(&reader).await.cursor;
    let old = mention(&server, &owner, &channel, &reader).await;
    server.clock.advance(Duration::seconds(1));
    let mut stream = server.stream(&reader).await;
    stream.subscribe(Level::Full, Some(cursor)).await;
    server.clock.advance(Duration::seconds(1));
    let live = mention(&server, &owner, &channel, &reader).await;
    let events = server.traffic(&reader, &mut stream).await;
    assert!(events.iter().any(
        |(_, event)| matches!(event,Event::MessageMentioned{message_id,..} if *message_id==live.id)
    ));
    assert!(!events.iter().any(
        |(_, event)| matches!(event,Event::MessageMentioned{message_id,..} if *message_id==old.id)
    ));
}

#[tokio::test]
async fn adding_a_mention_to_an_old_message_does_not_make_it_live() {
    let (server, owner, reader, channel, _) = denied().await;
    let old = send(&server, &owner, &channel, "Before connection").await;
    let mut stream = server.subscribed(&reader, Level::Full).await;
    server.clock.advance(Duration::seconds(1));
    server.put(&owner,&message(old.id),&json!({"document":{"blocks":[{"kind":"paragraph","content":[{"kind":"account","account_id":reader.account_id}]}]}})).await.ok::<Message>(StatusCode::OK);
    assert!(!server.traffic(&reader, &mut stream).await.iter().any(
        |(_, event)| matches!(event,Event::MessageMentioned{message_id,..} if *message_id==old.id)
    ));
}

#[tokio::test]
async fn a_mention_created_at_connection_start_needs_history_permission() {
    let (server, owner, reader, channel, _) = denied().await;
    let mut stream = server.subscribed(&reader, Level::Full).await;
    let same_time = mention(&server, &owner, &channel, &reader).await;
    assert!(!server.traffic(&reader,&mut stream).await.iter().any(|(_,event)| matches!(event,Event::MessageMentioned{message_id,..} if *message_id==same_time.id)));
}

#[tokio::test]
async fn granting_history_allows_replay_of_an_older_mention() {
    let (server, owner, reader, channel, _) = denied().await;
    let cursor = server.snapshot(&reader).await.cursor;
    let old = mention(&server, &owner, &channel, &reader).await;
    sqlx::query("delete from channel_person_overrides where channel_id=$1 and account_id=$2")
        .bind(channel.id)
        .bind(reader.account_id)
        .execute(&server.db)
        .await
        .unwrap();
    server.clock.advance(Duration::seconds(1));
    let events = crate::discovery_support::events(&server, &reader, cursor, Level::Full).await;
    assert!(events.iter().any(
        |event| matches!(event,Event::MessageMentioned{message_id,..} if *message_id==old.id)
    ));
}
