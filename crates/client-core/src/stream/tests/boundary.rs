//! A channel whose history the account may not read (ADR 0304): the server refuses its
//! pages, its timeline holds the messages that the connections brought, a reconnect keeps
//! them, and a withdrawal of the older messages or a permission change keeps only the
//! messages of the current connection.

use junto_protocol::WorkspacePermission;
use junto_protocol::message::Message;

use super::messages::positions;
use super::*;
use crate::model::PlaceId;
use crate::model::message_tests::message;

struct LiveRoom {
    rig: Rig,
    peer: Peer,
    place: PlaceId,
    other: AccountId,
}

impl LiveRoom {
    /// A served connection whose page of the text channel the server refuses, with the
    /// timeline of the channel open.
    async fn open() -> Self {
        let mut rig = rig();
        let place = PlaceId::Channel(rig.scene.general);
        rig.http.fail("/messages", 403, "forbidden");
        rig.want(ConnectionLevel::Selected, "jst_one");
        let (peer, _, _) = rig.served().await;
        soon("the opening", rig.live.open_messages(place))
            .await
            .unwrap();
        let model = rig
            .model_when("the timeline", |model| {
                model
                    .timeline(place)
                    .is_some_and(|timeline| timeline.loaded && timeline.failed.is_none())
            })
            .await;
        assert!(positions(&model, place).is_empty());
        Self {
            rig,
            peer,
            place,
            other: AccountId::generate(),
        }
    }

    /// The server holds a new message at `position`, and the connection is told of it.
    async fn arrives(&self, id: u64, position: u64) -> Message {
        let held = message(self.rig.scene.text_place(), position, self.other, "live");
        self.rig
            .http
            .routes
            .lock()
            .unwrap()
            .chat
            .messages
            .push(held.clone());
        self.peer
            .event(
                id,
                Event::MessageChanged {
                    place: held.place,
                    message_id: held.id,
                    position,
                    deleted: false,
                },
            )
            .await;
        let place = self.place;
        self.rig
            .model_when("the new message", |model| {
                positions(model, place).contains(&position)
            })
            .await;
        held
    }

    /// The connection is lost, and the next one continues from the cursor.
    async fn reconnect(&mut self) {
        self.peer.close(4003).await;
        self.rig
            .when("interrupted", |view| view.status != StreamStatus::Live)
            .await;
        let (peer, _, _) = self.rig.served().await;
        self.peer = peer;
    }

    fn withdrawal(&self) -> Event {
        Event::ChannelHistoryWithdrawn {
            workspace_id: self.rig.scene.workspace_id,
            channel_id: self.rig.scene.general,
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_withdrawal_drops_the_messages_of_earlier_connections_and_a_reconnect_keeps_them() {
    let mut room = LiveRoom::open().await;
    let place = room.place;
    let older = room.arrives(41, 4).await;
    room.reconnect().await;
    assert_eq!(positions(&room.rig.model(), place), [4]);
    room.arrives(42, 5).await;

    // The older message is deleted. The server names no message: the client keeps what
    // the current connection brought.
    room.rig
        .http
        .routes
        .lock()
        .unwrap()
        .chat
        .messages
        .retain(|held| held.id != older.id);
    room.peer.event(43, room.withdrawal()).await;
    let model = room
        .rig
        .model_when("the withdrawal", |model| model.cursor.position == 43)
        .await;
    assert_eq!(positions(&model, place), [5]);
    let timeline = model.timeline(place).unwrap();
    assert!(timeline.loaded && !timeline.stale && timeline.failed.is_none());
    assert_eq!(room.rig.http.count("/snapshot"), 1);

    // A new message of the connection still arrives.
    room.arrives(44, 6).await;
    assert_eq!(positions(&room.rig.model(), place), [5, 6]);
}

#[tokio::test(start_paused = true)]
async fn a_permission_change_without_history_keeps_only_the_messages_of_the_current_connection() {
    let mut room = LiveRoom::open().await;
    let place = room.place;
    room.arrives(41, 4).await;
    room.reconnect().await;
    room.arrives(42, 5).await;

    room.rig.set_snapshot(43, |_| {});
    room.rig.http.fail("/messages", 403, "forbidden");
    room.peer
        .event(
            43,
            Event::PermissionsChanged {
                workspace_id: Some(room.rig.scene.workspace_id),
            },
        )
        .await;
    room.peer.serve().await;
    let model = room
        .rig
        .model_when("the timeline after the reload", |model| {
            model.cursor.position == 43
                && model
                    .timeline(place)
                    .is_some_and(|timeline| timeline.loaded && !timeline.stale)
        })
        .await;
    assert_eq!(positions(&model, place), [5]);
}

#[tokio::test(start_paused = true)]
async fn losing_the_history_drops_the_messages_that_its_pages_gave() {
    let mut rig = rig();
    let place = PlaceId::Channel(rig.scene.general);
    let other = AccountId::generate();
    let history = WorkspacePermission::ReadMessageHistory;
    rig.set_snapshot(40, |snapshot| {
        snapshot.workspaces[0].permissions.push(history);
    });
    rig.http.routes.lock().unwrap().chat.messages = (1..=3)
        .map(|position| message(rig.scene.text_place(), position, other, "old"))
        .collect();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (mut peer, _, _) = rig.served().await;
    soon("the first page", rig.live.open_messages(place))
        .await
        .unwrap();
    assert_eq!(positions(&rig.model(), place), [1, 2, 3]);

    rig.set_snapshot(41, |snapshot| {
        snapshot.workspaces[0]
            .permissions
            .retain(|held| *held != history);
    });
    rig.http.fail("/messages", 403, "forbidden");
    peer.event(
        41,
        Event::PermissionsChanged {
            workspace_id: Some(rig.scene.workspace_id),
        },
    )
    .await;
    peer.serve().await;
    let model = rig
        .model_when("the timeline after the reload", |model| {
            model.cursor.position == 41
                && model
                    .timeline(place)
                    .is_some_and(|timeline| timeline.loaded && !timeline.stale)
        })
        .await;
    assert!(positions(&model, place).is_empty());

    // The messages of the connection arrive from now on.
    let fresh = message(rig.scene.text_place(), 4, other, "fresh");
    rig.http
        .routes
        .lock()
        .unwrap()
        .chat
        .messages
        .push(fresh.clone());
    peer.event(
        42,
        Event::MessageChanged {
            place: fresh.place,
            message_id: fresh.id,
            position: 4,
            deleted: false,
        },
    )
    .await;
    rig.model_when("the fresh message", |model| positions(model, place) == [4])
        .await;
}
