use super::*;
use junto_protocol::typing::Typing;

async fn next_typing(notices: &mut Notices) -> Option<Typing> {
    soon("a transient typing notice", async {
        loop {
            match notices.next().await {
                Some(Ok(Notice::Typing(activity))) => return activity,
                Some(Ok(_)) => {}
                other => panic!("no activity notice: {other:?}"),
            }
        }
    })
    .await
}

#[tokio::test(start_paused = true)]
async fn typing_is_owner_and_watch_scoped_without_moving_the_cursor_and_resets_offline() {
    let mut room = messages::room().await;
    let watch = soon("the typing watch", room.peer.from_client.recv())
        .await
        .unwrap();
    assert!(
        matches!(serde_json::from_str(&watch).unwrap(), ClientMessage::TypingWatch { places } if places == [room.place.typing_place()])
    );
    assert!(!room.rig.live.typing_watched_for_test(room.place));
    room.peer
        .send(&ServerMessage::TypingWatching {
            places: vec![room.place.typing_place()],
        })
        .await;
    let mut notices = room.rig.live.notices();
    room.rig
        .live
        .typing(AccountId::generate(), room.place, 1000);
    room.rig
        .live
        .typing(room.rig.scene.account_id, room.place, 2000);
    let sent = soon("owner activity", room.peer.from_client.recv())
        .await
        .unwrap();
    assert!(
        matches!(serde_json::from_str(&sent).unwrap(), ClientMessage::Typing { place, lifetime_ms: 2000 } if place == room.place.typing_place())
    );
    let cursor = room.rig.model().cursor;
    let activity = Typing {
        place: room.place.typing_place(),
        account_id: room.other,
        name: "Ada".to_owned(),
        active: true,
        expires_in_ms: 8000,
    };
    room.peer
        .send(&ServerMessage::Typing {
            activity: activity.clone(),
        })
        .await;
    assert_eq!(next_typing(&mut notices).await, Some(activity.clone()));
    assert!(room.rig.live.typing_watched_for_test(room.place));
    assert_eq!(room.rig.model().cursor, cursor);
    let mut hidden = activity.clone();
    hidden.place =
        crate::model::PlaceId::Channel(junto_protocol::ids::ChannelId::generate()).typing_place();
    room.peer
        .send(&ServerMessage::Typing { activity: hidden })
        .await;
    let stopped = Typing {
        active: false,
        expires_in_ms: 0,
        ..activity
    };
    room.peer
        .send(&ServerMessage::Typing {
            activity: stopped.clone(),
        })
        .await;
    assert_eq!(next_typing(&mut notices).await, Some(stopped));
    room.rig.off();
    assert_eq!(next_typing(&mut notices).await, None);
    assert!(!room.rig.live.typing_watched_for_test(room.place));
    assert!(room.peer.dropped().await);
}

#[tokio::test(start_paused = true)]
async fn an_older_server_receives_no_activity_frames() {
    let mut room = messages::room_at(7).await;
    room.rig
        .live
        .typing(room.rig.scene.account_id, room.place, 8000);
    room.rig.off();
    assert!(room.peer.dropped().await);
}

/// An activity asked for while the state is read again does not reach the server: the
/// subscription that follows the new state tells it, with the rest of its lifetime.
#[tokio::test(start_paused = true)]
async fn an_activity_asked_for_during_a_reload_is_told_when_the_subscription_is_live() {
    let mut room = messages::room().await;
    let snapshot = room.rig.http.hold("/snapshot");
    let read = room.rig.http.count("/snapshot");
    room.peer.send(&ServerMessage::AppearanceChanged {}).await;
    soon("the snapshot is read again", async {
        while room.rig.http.count("/snapshot") <= read {
            tokio::task::yield_now().await;
        }
    })
    .await;
    room.rig
        .live
        .typing(room.rig.scene.account_id, room.place, 8000);
    snapshot.add_permits(1);
    let (level, cursor) = room.peer.subscribe().await;
    room.peer
        .send(&ServerMessage::Subscribed { level, cursor })
        .await;
    let (place, lifetime_ms) = soon("the activity is told", async {
        loop {
            let text = room.peer.from_client.recv().await.unwrap();
            match serde_json::from_str(&text).unwrap() {
                ClientMessage::Typing { place, lifetime_ms } => return (place, lifetime_ms),
                ClientMessage::TypingWatch { .. } => {}
                other => panic!("unexpected frame {other:?}"),
            }
        }
    })
    .await;
    assert_eq!(place, room.place.typing_place());
    assert!(lifetime_ms > 0 && lifetime_ms <= 8000, "{lifetime_ms}");
}
