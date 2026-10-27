use super::*;
use crate::support::Member;
use junto_protocol::events::Event;

/// An old root with an old reply in its thread, written by the owner before every
/// connection; a member with history; and a member who cannot see the channel.
struct OldThread {
    server: TestServer,
    owner: Member,
    reader: Member,
    historian: Member,
    outsider: Member,
    channel: Channel,
    root: Message,
    reply: Message,
}

async fn old_thread() -> OldThread {
    let (server, owner, reader, channel, _) = denied().await;
    let historian = server.join(&owner, "Historian").await;
    let outsider = server.join(&owner, "Outsider").await;
    sqlx::query("insert into channel_person_overrides(channel_id,account_id,workspace_id,allow,deny) values($1,$2,$3,0,$4)")
        .bind(channel.id).bind(outsider.account_id).bind(channel.workspace_id).bind(1_i64 << W::ViewChannel as u8).execute(&server.db).await.unwrap();
    let root = send(&server, &owner, &channel, "Old root").await;
    let reply: Message = server
        .post(
            &owner,
            &format!("{}/thread", message(root.id)),
            &text("Old reply"),
        )
        .await
        .ok(StatusCode::CREATED);
    OldThread {
        server,
        owner,
        reader,
        historian,
        outsider,
        channel,
        root,
        reply,
    }
}

impl OldThread {
    /// An edit of the root, then a deletion of the reply, each its own change. The
    /// deletion also changes the root, so its change holds two events of the channel.
    async fn change_old_messages(&self) {
        self.server.clock.advance(Duration::seconds(1));
        self.server
            .put(
                &self.owner,
                &message(self.root.id),
                &text("Edited old root"),
            )
            .await
            .ok::<Message>(StatusCode::OK);
        self.server
            .delete(&self.owner, &message(self.reply.id))
            .await
            .empty(StatusCode::NO_CONTENT);
    }

    fn withdrawal(&self) -> Event {
        Event::ChannelHistoryWithdrawn {
            workspace_id: self.channel.workspace_id,
            channel_id: self.channel.id,
        }
    }

    /// The reader received one withdrawal for each of the two changes, and no frame of its
    /// stream names an old message.
    fn assert_withdrawn_twice(&self, events: &[(u64, Event)], raw: &[String]) {
        let received: Vec<&Event> = events.iter().map(|(_, event)| event).collect();
        assert_eq!(received, [&self.withdrawal(), &self.withdrawal()]);
        for frame in raw {
            assert!(!frame.contains(&self.root.id.to_string()), "{frame}");
            assert!(!frame.contains(&self.reply.id.to_string()), "{frame}");
            assert!(
                frame.matches("channel_history_withdrawn").count() <= 1,
                "{frame}"
            );
        }
    }
}

#[tokio::test]
async fn old_message_changes_reach_a_live_viewer_without_history_only_as_a_channel_withdrawal() {
    let thread = old_thread().await;
    let server = &thread.server;
    let mut reader = server.subscribed(&thread.reader, Level::Full).await;
    let mut historian = server.subscribed(&thread.historian, Level::Full).await;
    let mut outsider = server.subscribed(&thread.outsider, Level::Full).await;
    thread.change_old_messages().await;

    let events = server.traffic(&thread.reader, &mut reader).await;
    thread.assert_withdrawn_twice(&events, &reader.raw);

    let changed = |events: &[(u64, Event)], id| {
        events
            .iter()
            .filter(|(_, event)| matches!(event, Event::MessageChanged { message_id, .. } if *message_id == id))
            .count()
    };
    let full = server.traffic(&thread.historian, &mut historian).await;
    assert!(changed(&full, thread.root.id) >= 1);
    assert_eq!(changed(&full, thread.reply.id), 1);
    assert!(!full.iter().any(|(_, event)| *event == thread.withdrawal()));

    let none = server.traffic(&thread.outsider, &mut outsider).await;
    assert!(
        !none.iter().any(|(_, event)| matches!(
            event,
            Event::MessageChanged { .. } | Event::ChannelHistoryWithdrawn { .. }
        )),
        "{none:?}"
    );
}

#[tokio::test]
async fn old_message_changes_are_replayed_after_a_reconnect_only_as_a_channel_withdrawal() {
    let thread = old_thread().await;
    let server = &thread.server;
    let first = server.subscribed(&thread.reader, Level::Full).await;
    let cursor = server.snapshot(&thread.reader).await.cursor;
    drop(first);
    thread.change_old_messages().await;
    server.clock.advance(Duration::seconds(1));

    let mut again = server.stream(&thread.reader).await;
    again.subscribe(Level::Full, Some(cursor)).await;
    let events = server.traffic(&thread.reader, &mut again).await;
    thread.assert_withdrawn_twice(&events, &again.raw);

    let full =
        crate::discovery_support::events(server, &thread.historian, cursor, Level::Full).await;
    assert!(full.iter().any(|event| matches!(event, Event::MessageChanged { message_id, deleted: true, .. } if *message_id == thread.reply.id)));
    let none =
        crate::discovery_support::events(server, &thread.outsider, cursor, Level::Full).await;
    assert!(
        !none.iter().any(|event| matches!(
            event,
            Event::MessageChanged { .. } | Event::ChannelHistoryWithdrawn { .. }
        )),
        "{none:?}"
    );
}

#[tokio::test]
async fn a_message_created_between_connections_is_neither_replayed_nor_a_withdrawal() {
    let thread = old_thread().await;
    let server = &thread.server;
    let first = server.subscribed(&thread.reader, Level::Full).await;
    let cursor = server.snapshot(&thread.reader).await.cursor;
    drop(first);
    server.clock.advance(Duration::seconds(1));
    let between = send(
        server,
        &thread.owner,
        &thread.channel,
        "Between connections",
    )
    .await;
    server.clock.advance(Duration::seconds(1));

    let mut again = server.stream(&thread.reader).await;
    again.subscribe(Level::Full, Some(cursor)).await;
    let events = server.traffic(&thread.reader, &mut again).await;
    assert!(
        !events.iter().any(|(_, event)| matches!(
            event,
            Event::MessageChanged { .. } | Event::ChannelHistoryWithdrawn { .. }
        )),
        "{events:?}"
    );
    assert!(
        !again
            .raw
            .iter()
            .any(|frame| frame.contains(&between.id.to_string()))
    );

    server.clock.advance(Duration::seconds(1));
    let live = send(
        server,
        &thread.owner,
        &thread.channel,
        "During the connection",
    )
    .await;
    let events = server.traffic(&thread.reader, &mut again).await;
    assert!(events.iter().any(
        |(_, event)| matches!(event, Event::MessageChanged { message_id, .. } if *message_id == live.id)
    ));
}
