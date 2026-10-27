use super::*;
use junto_protocol::events::Event;

#[tokio::test]
async fn an_edit_does_not_make_an_old_message_visible_to_a_live_connection() {
    let (server, owner, reader, channel, old) = denied().await;
    let mut stream = server.subscribed(&reader, Level::Full).await;
    server.clock.advance(Duration::seconds(1));
    server
        .put(&reader, &message(old.id), &text("Changed old message"))
        .await
        .ok::<Message>(StatusCode::OK);
    let fresh = send(&server, &owner, &channel, "Live message").await;
    let events = server.traffic(&reader, &mut stream).await;
    assert!(events.iter().any(
        |(_, event)| matches!(event,Event::MessageChanged{message_id,..} if *message_id==fresh.id)
    ));
    assert!(!events.iter().any(
        |(_, event)| matches!(event,Event::MessageChanged{message_id,..} if *message_id==old.id)
    ));
}

#[tokio::test]
async fn reconnect_does_not_replay_a_message_from_the_earlier_connection() {
    let (server, owner, reader, channel, _) = denied().await;
    let mut first = server.subscribed(&reader, Level::Full).await;
    let cursor = server.snapshot(&reader).await.cursor;
    server.clock.advance(Duration::seconds(1));
    let fresh = send(&server, &owner, &channel, "Live message").await;
    assert!(server.traffic(&reader, &mut first).await.iter().any(
        |(_, event)| matches!(event,Event::MessageChanged{message_id,..} if *message_id==fresh.id)
    ));
    server.clock.advance(Duration::seconds(1));
    let events = crate::discovery_support::events(&server, &reader, cursor, Level::Full).await;
    assert!(!events.iter().any(
        |event| matches!(event,Event::MessageChanged{message_id,..} if *message_id==fresh.id)
    ));
}

#[tokio::test]
async fn deleting_a_live_reply_still_tells_the_connection_after_the_row_is_erased() {
    let (server, owner, reader, channel, _) = denied().await;
    let mut stream = server.subscribed(&reader, Level::Full).await;
    server.clock.advance(Duration::seconds(1));
    let root = send(&server, &owner, &channel, "Live root").await;
    let reply: Message = server
        .post(
            &owner,
            &format!("{}/thread", message(root.id)),
            &text("Live reply"),
        )
        .await
        .ok(StatusCode::CREATED);
    server.traffic(&reader, &mut stream).await;
    server.clock.advance(Duration::seconds(1));
    server
        .delete(&owner, &message(reply.id))
        .await
        .empty(StatusCode::NO_CONTENT);
    assert!(server.traffic(&reader,&mut stream).await.iter().any(|(_,event)| matches!(event,Event::MessageChanged{message_id,deleted:true,..} if *message_id==reply.id)));
}

#[tokio::test]
async fn a_message_at_the_connection_start_is_not_delivered_without_history_permission() {
    let (server, owner, reader, channel, _) = denied().await;
    let mut stream = server.subscribed(&reader, Level::Full).await;
    let same_time = send(&server, &owner, &channel, "At connection start").await;
    assert!(!server.traffic(&reader,&mut stream).await.iter().any(|(_,event)| matches!(event,Event::MessageChanged{message_id,..} if *message_id==same_time.id)));
}

#[tokio::test]
async fn old_recording_system_lines_are_not_replayed_without_history_permission() {
    let studio = crate::recordings_support::Studio::open().await;
    let scene = &studio.scene;
    let cursor = scene.server.snapshot(&scene.ben).await.cursor;
    studio.finished_after(Duration::seconds(1)).await;
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(scene.voice.id).bind(scene.ben.account_id).bind(scene.workspace_id).bind(1_i64 << W::ReadMessageHistory as u8).execute(&scene.server.db).await.unwrap();
    scene.server.clock.advance(Duration::seconds(1));
    let events =
        crate::discovery_support::events(&scene.server, &scene.ben, cursor, Level::Full).await;
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::MessageChanged { .. }))
    );
}

#[tokio::test]
async fn a_fresh_edit_receipt_names_the_change_that_the_live_connection_receives() {
    let (server, _, reader, channel, _) = denied().await;
    let mut stream = server.subscribed(&reader, Level::Full).await;
    server.clock.advance(Duration::seconds(1));
    let fresh = send(&server, &reader, &channel, "Live message").await;
    server.traffic(&reader, &mut stream).await;
    let edited = server
        .put(&reader, &message(fresh.id), &text("Edited live message"))
        .await;
    assert_eq!(edited.status, StatusCode::OK);
    let _: junto_protocol::events::Cursor = serde_json::from_str(
        edited
            .header(junto_protocol::events::EVENT_CURSOR_HEADER)
            .unwrap(),
    )
    .unwrap();
    assert!(server.traffic(&reader, &mut stream).await.iter().any(
        |(_, event)| matches!(event,Event::MessageChanged{message_id,..} if *message_id==fresh.id)
    ));
}
