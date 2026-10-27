//! Threads, pins and notices in the stream client (ADR 0722), against routes that the test
//! plays.

use junto_protocol::discovery::{
    Notification, NotificationKind, NotificationSettings, ScopedNotificationSettings,
};
use junto_protocol::ids::{ChannelId, MessageId};
use junto_protocol::message::{Message, ThreadState};

use super::messages::{Room, positions, room, room_at};
use super::*;
use crate::model::message_tests::{document, message};
use crate::model::{Loadable, Named, PlaceId};

const UNREAD: &str = "/api/v1/threads/unread";

fn thread_place(room: &Room, root: &Message) -> PlaceId {
    PlaceId::Thread {
        channel_id: room.rig.scene.general,
        root_id: root.id,
    }
}

fn root_of(room: &Room) -> Message {
    room.chat(|chat| chat.messages[1].clone())
}

/// A reply of the played server under `root`, at a position of the channel.
fn reply(room: &Room, root: &Message, position: u64, text: &str) -> Message {
    let mut reply = room.held(position, text);
    reply.thread_root = Some(root.id);
    room.chat(|chat| chat.replies.push(reply.clone()));
    reply
}

fn unread(root: &Message, last_read: u64, latest: u64, unread: u64) -> ThreadState {
    ThreadState {
        root_id: root.id,
        last_read,
        latest,
        unread,
        mentions: 0,
        following: true,
        follow_setting: None,
    }
}

async fn open_thread(room: &Room, root: &Message) -> PlaceId {
    let thread = thread_place(room, root);
    soon("the thread", room.rig.live.open_messages(thread))
        .await
        .unwrap();
    thread
}

#[tokio::test(start_paused = true)]
async fn a_thread_opens_with_its_root_and_a_reply_reaches_it_and_not_its_channel() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let root = root_of(&room);
    reply(&room, &root, 4, "first");
    room.chat(|chat| chat.forks.push(ChannelId::generate()));
    let thread = open_thread(&room, &root).await;
    let model = rig.model();
    let timeline = model.timeline(thread).unwrap();
    assert_eq!(positions(&model, thread), [4]);
    assert_eq!(timeline.root.as_ref().unwrap().id, root.id);
    assert_eq!(timeline.root.as_ref().unwrap().reply_count, 1);
    assert_eq!(timeline.forks.len(), 1);

    // The server tells a new reply with the place of the channel, and then its root.
    let second = reply(&room, &root, 5, "second");
    room.changed(41, &second, false).await;
    room.changed(42, &root, false).await;
    let model = rig
        .model_when("the reply", |model| positions(model, thread) == [4, 5])
        .await;
    assert_eq!(positions(&model, place), [1, 2, 3]);

    // A reply that the server deleted leaves the thread.
    room.chat(|chat| chat.replies.retain(|held| held.id != second.id));
    room.changed(43, &second, true).await;
    rig.model_when("the deletion", |model| positions(model, thread) == [4])
        .await;
    assert_eq!(positions(&rig.model(), place), [1, 2, 3]);
}

#[tokio::test(start_paused = true)]
async fn a_reply_is_sent_into_its_thread_and_the_read_position_goes_to_the_thread_route() {
    let room = room().await;
    let rig = &room.rig;
    let root = root_of(&room);
    reply(&room, &root, 4, "first");
    room.chat(|chat| chat.threads = vec![unread(&root, 0, 4, 1)]);
    let thread = open_thread(&room, &root).await;

    rig.live
        .send_message(thread, document("an answer"), None)
        .unwrap();
    let model = rig
        .model_when("the sent reply", |model| {
            positions(model, thread) == [4, 5] && model.outbox.is_empty()
        })
        .await;
    assert_eq!(positions(&model, room.place), [1, 2, 3]);
    let sent = model.timeline(thread).unwrap().messages[1].clone();
    assert_eq!(sent.thread_root, Some(root.id));
    assert_eq!(
        room.sent_to(&format!("/api/v1/messages/{}/thread", root.id)),
        2
    );

    // The read position of a thread is sent to the route of the thread, and the summary
    // of the channel is read after it: a mention of a thread counts there.
    let summaries = rig.http.count("/messages/read");
    rig.live.mark_read(thread, 5);
    soon("the read position", async {
        while room.chat(|chat| chat.thread_marked.is_empty()) {
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert_eq!(room.chat(|chat| chat.thread_marked.clone()), [(root.id, 5)]);
    assert!(room.chat(|chat| chat.marked.is_empty()));
    soon("the summary", async {
        while rig.http.count("/messages/read") == summaries {
            tokio::task::yield_now().await;
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn the_unread_threads_are_read_after_the_notice_of_a_root_and_two_reads_keep_a_distance() {
    // A server of revision 1: its events state no thread, and the list is read.
    let room = room_at(1).await;
    let rig = &room.rig;
    let root = root_of(&room);
    assert_eq!(room.sent_to(UNREAD), 0);

    // A new message of the channel changes no thread.
    let fourth = room.held(4, "four");
    room.chat(|chat| chat.messages.push(fourth.clone()));
    room.changed(41, &fourth, false).await;
    rig.model_when("the message", |model| {
        positions(model, room.place) == [1, 2, 3, 4]
    })
    .await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(room.sent_to(UNREAD), 0);

    // A reply and the notice of its root: the list is read once.
    let answer = reply(&room, &root, 5, "reply");
    room.chat(|chat| chat.threads = vec![unread(&root, 0, 5, 1)]);
    room.changed(42, &answer, false).await;
    room.changed(43, &root, false).await;
    // The list names the root, which is read for its place and its text.
    let model = rig
        .model_when("the unread thread with its root", |model| {
            matches!(model.named(root.id), Some(Named::Read(_)))
        })
        .await;
    assert_eq!(model.thread_state(root.id).unwrap().unread, 1);
    assert_eq!(room.sent_to(UNREAD), 1);

    // Further notices at once: the next read waits for its distance, and is one read.
    let policy = StreamPolicy::default();
    for id in 44..50 {
        room.changed(id, &root, false).await;
    }
    rig.model_when("the events", |model| model.cursor.position == 49)
        .await;
    tokio::time::sleep(policy.list_interval / 2).await;
    assert_eq!(room.sent_to(UNREAD), 1);
    tokio::time::sleep(policy.list_interval).await;
    assert_eq!(room.sent_to(UNREAD), 2);
    tokio::time::sleep(policy.list_interval * 4).await;
    assert_eq!(room.sent_to(UNREAD), 2);
}

#[tokio::test(start_paused = true)]
async fn a_thread_opens_at_its_read_position_and_leaves_the_list_when_it_is_read() {
    let mut rig = rig();
    let other = AccountId::generate();
    let place = PlaceId::Channel(rig.scene.general);
    let root = message(rig.scene.text_place(), 1, other, "root");
    let state = ThreadState {
        root_id: root.id,
        last_read: 40,
        latest: 300,
        unread: 100,
        mentions: 0,
        following: true,
        follow_setting: None,
    };
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.chat.own = Some(rig.scene.account_id);
        routes.chat.messages = vec![root.clone()];
        routes.chat.replies = (2..=300)
            .map(|position| {
                let mut reply = message(rig.scene.text_place(), position, other, "reply");
                reply.thread_root = Some(root.id);
                reply
            })
            .collect();
        routes.chat.threads = vec![state.clone()];
        routes.snapshot.as_mut().unwrap().threads = vec![state];
    }
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    // The root of the unread thread is read: a reader needs its place and its text.
    rig.model_when("the root", |model| {
        matches!(model.named(root.id), Some(Named::Read(_)))
    })
    .await;

    let thread = PlaceId::Thread {
        channel_id: rig.scene.general,
        root_id: root.id,
    };
    soon("the thread", rig.live.open_messages(thread))
        .await
        .unwrap();
    let model = rig.model();
    let timeline = model.timeline(thread).unwrap();
    // The page before the read position and the page after it.
    assert_eq!(timeline.messages.first().unwrap().position, 11);
    assert_eq!(timeline.messages.last().unwrap().position, 140);
    assert_eq!(timeline.unread_after, Some(40));
    assert!(timeline.more_before && timeline.more_after);
    assert!(model.timeline(place).is_none());

    // Another device of the account reads the thread to its end.
    peer.event(
        41,
        Event::ThreadRead {
            root_id: root.id,
            position: 300,
        },
    )
    .await;
    let model = rig
        .model_when("the read thread", |model| model.threads.is_empty())
        .await;
    assert!(model.named(root.id).is_none());
}

#[tokio::test(start_paused = true)]
async fn the_pin_list_is_read_once_and_follows_the_messages_that_change() {
    let room = room().await;
    let rig = &room.rig;
    let general = rig.scene.general;
    room.chat(|chat| chat.messages[0].pinned = true);
    soon("the pins", rig.live.open_pins(general)).await.unwrap();
    let pins = |model: &ServerModel| -> Vec<u64> {
        model
            .pin_list(general)
            .and_then(|list| list.messages.loaded())
            .map(|pins| pins.iter().map(|pin| pin.position).collect())
            .unwrap_or_default()
    };
    assert_eq!(pins(&rig.model()), [1]);

    // A person pins the third message: the server tells a change of that message.
    let third = room.chat(|chat| {
        chat.messages[2].pinned = true;
        chat.messages[2].clone()
    });
    room.changed(41, &third, false).await;
    rig.model_when("the new pin", |model| pins(model) == [1, 3])
        .await;
    // The first pin is taken away.
    let first = room.chat(|chat| {
        chat.messages[0].pinned = false;
        chat.messages[0].clone()
    });
    room.changed(42, &first, false).await;
    let model = rig
        .model_when("the removed pin", |model| pins(model) == [3])
        .await;
    assert!(model.timeline(room.place).unwrap().messages[2].pinned);
    assert_eq!(rig.http.count("/pins"), 1);

    // After a reload the list is read again by the client itself.
    rig.set_snapshot(90, |_| {});
    room.peer
        .send(&ServerMessage::Resync {
            reason: ResyncReason::TooFarBehind,
        })
        .await;
    rig.model_when("the list after the reload", |model| {
        model.cursor.position == 90 && pins(model) == [3]
    })
    .await;
    assert_eq!(rig.http.count("/pins"), 2);

    // Without a reader the list leaves the state.
    rig.live.release_pins(general);
    rig.model_when("no list", |model| model.pin_list(general).is_none())
        .await;
}

#[tokio::test(start_paused = true)]
async fn a_pin_list_that_is_not_read_tells_its_reader_and_the_state() {
    let room = room().await;
    let rig = &room.rig;
    let general = rig.scene.general;
    rig.http.fail("/pins", 403, "forbidden");
    let opened = soon("the refusal", rig.live.open_pins(general)).await;
    assert!(matches!(opened, Err(LoadError::Request(_))));
    let model = rig.model();
    let list = model.pin_list(general).unwrap();
    assert_eq!(list.messages, Loadable::NotLoaded);
    assert!(list.failed.is_some());

    // A channel that the state does not hold has no list.
    let unseen = soon("the answer", rig.live.open_pins(ChannelId::generate())).await;
    assert!(matches!(unseen, Err(LoadError::NotInState)));
}

#[tokio::test(start_paused = true)]
async fn a_notice_event_reads_the_notices_and_the_messages_that_they_name() {
    let room = room().await;
    let rig = &room.rig;
    // A mention in a channel message that no timeline holds.
    let mut far = room.held(9, "@you");
    far.id = MessageId::generate();
    let notice = Notification {
        id: far.id.as_uuid(),
        message_id: far.id,
        kind: NotificationKind::Mention,
        created_at: crate::model::tests::time(),
        read_at: None,
    };
    room.chat(|chat| {
        chat.replies.push(far.clone());
        chat.notices = vec![notice.clone()];
    });
    room.peer
        .event(
            41,
            Event::NotificationChanged {
                message_id: far.id,
                notice: None,
            },
        )
        .await;
    let model = rig
        .model_when("the notice with its message", |model| {
            matches!(model.named(far.id), Some(Named::Read(_)))
        })
        .await;
    assert_eq!(model.notices, std::slice::from_ref(&notice));
    assert_eq!(rig.http.count("/notifications"), 1);
    // The event also asks for the unread threads: the notice can be one of a thread.
    assert_eq!(room.sent_to(UNREAD), 1);

    // The message is deleted: its notice names a message that is gone.
    room.chat(|chat| chat.replies.clear());
    room.changed(42, &far, true).await;
    rig.model_when("the gone message", |model| {
        model.named(far.id) == Some(&Named::Gone)
    })
    .await;

    // The notice is read on another device: the list is read again and names nothing.
    room.chat(|chat| chat.notices.clear());
    room.peer
        .event(
            43,
            Event::NotificationChanged {
                message_id: far.id,
                notice: None,
            },
        )
        .await;
    let model = rig
        .model_when("no notice", |model| model.notices.is_empty())
        .await;
    assert!(model.named.is_empty());
}

#[tokio::test(start_paused = true)]
async fn notices_at_once_read_the_notices_with_a_distance_and_the_last_one_is_held() {
    let room = room().await;
    let rig = &room.rig;
    let policy = StreamPolicy::default();
    let notice = |message: &Message| Notification {
        id: message.id.as_uuid(),
        message_id: message.id,
        kind: NotificationKind::Mention,
        created_at: crate::model::tests::time(),
        read_at: None,
    };
    let first = room.held(9, "@you");
    room.chat(|chat| chat.notices = vec![notice(&first)]);
    room.peer
        .event(
            41,
            Event::NotificationChanged {
                message_id: first.id,
                notice: None,
            },
        )
        .await;
    rig.model_when("the first notice", |model| model.notices.len() == 1)
        .await;
    assert_eq!(rig.http.count("/notifications"), 1);

    // Ten more notices at once: the next read waits for its distance, and is one read
    // that holds the last notice.
    let mut held = vec![notice(&first)];
    for number in 0..10 {
        let mut mention = room.held(10 + number, "@you");
        mention.id = MessageId::generate();
        held.insert(0, notice(&mention));
        let shown = held.clone();
        room.chat(|chat| chat.notices = shown);
        room.peer
            .event(
                42 + number,
                Event::NotificationChanged {
                    message_id: mention.id,
                    notice: None,
                },
            )
            .await;
    }
    rig.model_when("the events", |model| model.cursor.position == 51)
        .await;
    tokio::time::sleep(policy.list_interval / 2).await;
    assert_eq!(rig.http.count("/notifications"), 1);
    let model = rig
        .model_when("every notice", |model| model.notices.len() == 11)
        .await;
    assert_eq!(model.notices, held);
    assert_eq!(rig.http.count("/notifications"), 2);
    tokio::time::sleep(policy.list_interval * 4).await;
    assert_eq!(rig.http.count("/notifications"), 2);
}

/// A notice of a message of the room.
fn notice_of(message: &Message) -> Notification {
    Notification {
        id: message.id.as_uuid(),
        message_id: message.id,
        kind: NotificationKind::Mention,
        created_at: crate::model::tests::time(),
        read_at: None,
    }
}

#[tokio::test(start_paused = true)]
async fn a_limited_read_of_the_notices_waits_for_the_time_that_the_server_names() {
    let room = room().await;
    let rig = &room.rig;
    let held = room.held(9, "@you");
    room.chat(|chat| chat.notices = vec![notice_of(&held)]);
    rig.http.fail("/notifications", 429, "rate_limited");
    room.peer
        .event(
            41,
            Event::NotificationChanged {
                message_id: held.id,
                notice: None,
            },
        )
        .await;
    rig.model_when("the event", |model| model.cursor.position == 41)
        .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(rig.http.count("/notifications"), 1);

    // A second notice during the named time asks for the list again: the read waits for
    // the end of that time too. The first wait of a failed read is 2 seconds, and the
    // distance of two reads of the list is 2 seconds; the server named 30.
    let mut later = room.held(10, "@you again");
    later.id = MessageId::generate();
    room.chat(|chat| chat.notices = vec![notice_of(&later), notice_of(&held)]);
    tokio::time::sleep(Duration::from_secs(4)).await;
    room.peer
        .event(
            42,
            Event::NotificationChanged {
                message_id: later.id,
                notice: None,
            },
        )
        .await;
    rig.model_when("the second event", |model| model.cursor.position == 42)
        .await;
    tokio::time::sleep(super::LIMITED_WAIT - Duration::from_secs(7)).await;
    assert_eq!(rig.http.count("/notifications"), 1);

    // At the end of the named time one read takes both notices.
    let model = rig
        .model_when("both notices", |model| model.notices.len() == 2)
        .await;
    assert_eq!(model.notices, [notice_of(&later), notice_of(&held)]);
    tokio::time::sleep(StreamPolicy::default().list_interval * 4).await;
    assert_eq!(rig.http.count("/notifications"), 2);
}

#[tokio::test(start_paused = true)]
async fn a_named_wait_longer_than_the_longest_retry_is_bounded() {
    let room = room().await;
    let rig = &room.rig;
    let held = room.held(9, "@you");
    room.chat(|chat| chat.notices = vec![notice_of(&held)]);
    // A proxy names an hour.
    let started = tokio::time::Instant::now();
    rig.http.fail_naming(
        "/notifications",
        429,
        "rate_limited",
        Some(Duration::from_secs(3600)),
    );
    room.peer
        .event(
            41,
            Event::NotificationChanged {
                message_id: held.id,
                notice: None,
            },
        )
        .await;
    rig.model_when("the event", |model| model.cursor.position == 41)
        .await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(rig.http.count("/notifications"), 1);
    // The read is tried again after the longest doubled wait, 64 seconds.
    let model = rig
        .model_when("the notice", |model| model.notices.len() == 1)
        .await;
    assert_eq!(model.notices, [notice_of(&held)]);
    assert_eq!(rig.http.count("/notifications"), 2);
    assert!(
        started.elapsed() < Duration::from_secs(70),
        "{:?}",
        started.elapsed()
    );
}

#[tokio::test(start_paused = true)]
async fn a_refused_cursor_of_older_notices_reads_the_first_page_again() {
    let room = room().await;
    let rig = &room.rig;
    let notice = |message: &Message| Notification {
        id: message.id.as_uuid(),
        message_id: message.id,
        kind: NotificationKind::Mention,
        created_at: crate::model::tests::time(),
        read_at: None,
    };
    let held = room.held(9, "@you");
    let first = notice(&held);
    room.chat(|chat| {
        chat.notices = vec![first.clone()];
        chat.notices_more = true;
    });
    room.peer
        .event(
            41,
            Event::NotificationChanged {
                message_id: held.id,
                notice: None,
            },
        )
        .await;
    rig.model_when("the first page", |model| {
        model.notices.len() == 1 && model.notices_more
    })
    .await;
    assert_eq!(rig.http.count("/notifications"), 1);

    // The server no longer takes the held notice as a cursor, as a notice of a deleted
    // place: the page after it is refused, and the first page is read again.
    room.chat(|chat| {
        chat.refused_cursors = vec![first.id];
        chat.notices_more = false;
    });
    rig.live.read_more_notices();
    rig.model_when("the first page again", |model| !model.notices_more)
        .await;
    assert_eq!(rig.http.count("/notifications"), 3);
}

#[tokio::test(start_paused = true)]
async fn a_changed_notification_setting_reloads_the_state() {
    let mut room = room().await;
    let general = room.rig.scene.general;
    assert_eq!(room.rig.http.count("/snapshot"), 1);
    room.rig.set_snapshot(90, |snapshot| {
        snapshot.notification_settings = vec![ScopedNotificationSettings {
            scope_id: general.as_uuid(),
            settings: NotificationSettings {
                mute: true,
                ..NotificationSettings::default()
            },
        }];
    });
    room.peer
        .event(41, Event::NotificationSettingsChanged {})
        .await;
    let model = room
        .rig
        .model_when("the settings", |model| {
            !model.notification_settings.is_empty()
        })
        .await;
    assert_eq!(model.cursor.position, 90);
    assert_eq!(room.rig.http.count("/snapshot"), 2);
    room.peer.serve().await;
}

#[tokio::test(start_paused = true)]
async fn the_summary_of_a_channel_is_read_when_its_mute_ends() {
    let mut rig = rig();
    let general = rig.scene.general;
    let until = time::OffsetDateTime::now_utc() + time::Duration::minutes(10);
    rig.set_snapshot(40, |snapshot| {
        snapshot.notification_settings = vec![ScopedNotificationSettings {
            scope_id: general.as_uuid(),
            settings: NotificationSettings {
                mute: true,
                muted_until: Some(until),
                ..NotificationSettings::default()
            },
        }];
    });
    rig.want(ConnectionLevel::Selected, "jst_one");
    let _served = rig.served().await;
    tokio::time::sleep(Duration::from_secs(9 * 60)).await;
    assert_eq!(rig.http.count("/messages/read"), 0);
    tokio::time::sleep(Duration::from_secs(2 * 60)).await;
    assert_eq!(rig.http.count("/messages/read"), 1);
    // The end is told once.
    tokio::time::sleep(Duration::from_secs(60 * 60)).await;
    assert_eq!(rig.http.count("/messages/read"), 1);
}

#[tokio::test(start_paused = true)]
async fn the_page_around_a_position_replaces_what_is_loaded_and_is_the_anchor_of_a_reload() {
    let mut room = room().await;
    let place = room.place;
    let text_place = room.rig.scene.text_place();
    let other = room.other;
    room.chat(|chat| {
        chat.messages = (1..=300)
            .map(|position| message(text_place, position, other, "x"))
            .collect();
    });
    room.rig.live.read_messages_around(place, 150);
    let model = room
        .rig
        .model_when("the page around the position", |model| {
            positions(model, place).first() == Some(&121)
        })
        .await;
    let timeline = model.timeline(place).unwrap();
    assert_eq!(timeline.messages.last().unwrap().position, 250);
    assert!(timeline.more_before && timeline.more_after);

    // A reload reads from the position that the reader went to.
    room.rig.set_snapshot(90, |_| {});
    room.peer
        .send(&ServerMessage::Resync {
            reason: ResyncReason::TooFarBehind,
        })
        .await;
    let model = room
        .rig
        .model_when("the page after the reload", |model| {
            model.cursor.position == 90
                && model
                    .timeline(place)
                    .is_some_and(|timeline| !timeline.stale)
        })
        .await;
    assert_eq!(positions(&model, place).first(), Some(&150));
    room.peer.serve().await;
}

#[tokio::test(start_paused = true)]
async fn a_reload_reads_an_open_thread_again_and_closes_the_thread_of_a_lost_channel() {
    let mut room = room().await;
    let root = root_of(&room);
    reply(&room, &root, 4, "first");
    let thread = open_thread(&room, &root).await;
    room.rig.live.release_messages(room.place);

    reply(&room, &root, 5, "second");
    room.rig.set_snapshot(90, |_| {});
    room.peer
        .send(&ServerMessage::Resync {
            reason: ResyncReason::TooFarBehind,
        })
        .await;
    let model = room
        .rig
        .model_when("the thread after the reload", |model| {
            model.cursor.position == 90 && positions(model, thread) == [4, 5]
        })
        .await;
    assert!(!model.timeline(thread).unwrap().stale);
    room.peer.serve().await;

    // The account loses the channel: the snapshot does not hold it, and the thread is
    // not in the state any more.
    let general = room.rig.scene.general;
    room.rig.set_snapshot(95, |snapshot| {
        snapshot.workspaces[0]
            .channels
            .retain(|channel| channel.id != general);
    });
    room.peer
        .event(
            91,
            Event::PermissionsChanged {
                workspace_id: Some(room.rig.scene.workspace_id),
            },
        )
        .await;
    let model = room
        .rig
        .model_when("the state without the channel", |model| {
            model.cursor.position == 95
        })
        .await;
    assert!(model.timeline(thread).is_none());
}

#[tokio::test(start_paused = true)]
async fn the_forks_of_an_open_thread_are_read_after_a_new_channel() {
    let room = room().await;
    let rig = &room.rig;
    let root = root_of(&room);
    let thread = open_thread(&room, &root).await;
    assert!(rig.model().timeline(thread).unwrap().forks.is_empty());

    let fork = channel(rig.scene.workspace_id, ChannelKind::Text, "fork", 9);
    room.chat(|chat| chat.forks.push(fork.id));
    room.peer
        .event(
            41,
            Event::ChannelCreated {
                channel: fork.clone(),
            },
        )
        .await;
    rig.model_when("the fork", |model| {
        model
            .timeline(thread)
            .is_some_and(|timeline| timeline.forks == [fork.id])
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn a_page_of_a_thread_and_a_pin_list_take_only_the_messages_of_their_place() {
    let room = room().await;
    let rig = &room.rig;
    let root = root_of(&room);
    reply(&room, &root, 4, "first");
    // The server answers with a reply of the thread that names another channel, and
    // with a pinned message of another channel.
    let mut foreign = room.held(5, "from another channel");
    foreign.thread_root = Some(root.id);
    foreign.pinned = true;
    foreign.place = junto_protocol::message::MessagePlace::Channel {
        workspace_id: rig.scene.workspace_id,
        channel_id: rig.scene.daily,
    };
    room.chat(|chat| {
        chat.replies.push(foreign.clone());
        chat.messages[0].pinned = true;
    });
    let thread = open_thread(&room, &root).await;
    assert_eq!(positions(&rig.model(), thread), [4]);

    soon("the pins", rig.live.open_pins(rig.scene.general))
        .await
        .unwrap();
    let model = rig.model();
    let pins = model
        .pin_list(rig.scene.general)
        .unwrap()
        .messages
        .loaded()
        .unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].position, 1);
}

#[tokio::test(start_paused = true)]
async fn a_named_message_that_the_server_answers_with_another_message_is_not_asked_for_again() {
    let room = room().await;
    let rig = &room.rig;
    let wanted = MessageId::generate();
    let notice = Notification {
        id: wanted.as_uuid(),
        message_id: wanted,
        kind: NotificationKind::Mention,
        created_at: crate::model::tests::time(),
        read_at: None,
    };
    room.chat(|chat| {
        chat.notices = vec![notice];
        chat.answered_wrongly = vec![wanted];
    });
    room.peer
        .event(
            41,
            Event::NotificationChanged {
                message_id: wanted,
                notice: None,
            },
        )
        .await;
    rig.model_when("the read that failed", |model| {
        model.named(wanted) == Some(&Named::Failed)
    })
    .await;
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert_eq!(room.sent_to(&format!("/api/v1/messages/{wanted}")), 1);
}

#[tokio::test(start_paused = true)]
async fn the_answer_to_a_read_position_of_a_thread_does_not_remove_what_came_after_it() {
    let mut rig = rig();
    let other = AccountId::generate();
    let root = message(rig.scene.text_place(), 1, other, "root");
    let state = |latest: u64, last_read: u64, unread: u64| ThreadState {
        root_id: root.id,
        last_read,
        latest,
        unread,
        mentions: 0,
        following: true,
        follow_setting: None,
    };
    let text_place = rig.scene.text_place();
    let reply_at = |position: u64| {
        let mut reply = message(text_place, position, other, "reply");
        reply.thread_root = Some(root.id);
        reply
    };
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.chat.own = Some(rig.scene.account_id);
        routes.chat.messages = vec![root.clone()];
        routes.chat.replies = (2..=5).map(reply_at).collect();
        routes.chat.threads = vec![state(5, 4, 1)];
        routes.snapshot.as_mut().unwrap().threads = vec![state(5, 4, 1)];
    }
    rig.want(ConnectionLevel::Selected, "jst_one");
    // A server of revision 1: its events state no thread, and the list is read.
    let (peer, _, _) = rig.served_at(1).await;
    let thread = PlaceId::Thread {
        channel_id: rig.scene.general,
        root_id: root.id,
    };
    soon("the thread", rig.live.open_messages(thread))
        .await
        .unwrap();

    // The person reads the thread to its end. The server makes its answer, and the
    // answer is late.
    let gate = rig.http.hold("PUT /thread/read");
    rig.live.mark_read(thread, 5);
    soon("the request", async {
        while rig.http.count("/thread/read") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    // A new reply arrives, and the list of the unread threads says so.
    let sixth = reply_at(6);
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.chat.replies.push(sixth.clone());
        routes.chat.threads = vec![state(6, 4, 2)];
        routes.chat.thread_mark_answer = Some(state(5, 5, 0));
    }
    let changed = |held: &Message| Event::MessageChanged {
        place: held.place,
        message_id: held.id,
        position: held.position,
        deleted: false,
        thread_root: None,
        thread: None,
    };
    peer.event(41, changed(&sixth)).await;
    peer.event(42, changed(&root)).await;
    rig.model_when("the new reply in the list", |model| {
        model
            .thread_state(root.id)
            .is_some_and(|state| state.latest == 6)
    })
    .await;
    gate.add_permits(64);
    tokio::time::sleep(Duration::from_secs(1)).await;
    // The late answer knows nothing of the sixth reply: the thread stays unread, and
    // the list is read for the counts of now.
    let model = rig
        .model_when("the counts of now", |model| {
            model
                .thread_state(root.id)
                .is_some_and(|state| state.latest == 6 && state.unread > 0)
        })
        .await;
    assert!(model.thread_state(root.id).is_some());
}

#[tokio::test(start_paused = true)]
async fn a_jump_ends_the_page_reads_that_were_on_their_way() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let text_place = rig.scene.text_place();
    let other = room.other;
    room.chat(|chat| {
        chat.messages = (1..=300)
            .map(|position| message(text_place, position, other, "x"))
            .collect();
    });
    rig.live.read_latest_messages(place);
    rig.model_when("the latest page", |model| {
        positions(model, place).first() == Some(&251)
    })
    .await;
    // A page of older messages is on its way when the person goes to a message.
    let gate = rig.http.hold("/messages");
    rig.live.read_older_messages(place);
    rig.model_when("the read of older messages", |model| {
        model.timeline(place).unwrap().reading.older
    })
    .await;
    rig.live.read_messages_around(place, 100);
    gate.add_permits(64);
    let model = rig
        .model_when("the page around the position", |model| {
            positions(model, place).first() == Some(&71)
        })
        .await;
    let reading = model.timeline(place).unwrap().reading;
    assert!(!reading.older && !reading.newer && !reading.open);
    // The next page of older messages is read.
    rig.live.read_older_messages(place);
    rig.model_when("the older page", |model| {
        positions(model, place).first() == Some(&21)
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn a_reply_whose_read_fails_marks_its_thread_as_not_current() {
    let room = room().await;
    let rig = &room.rig;
    let root = root_of(&room);
    let first = reply(&room, &root, 4, "first");
    let thread = open_thread(&room, &root).await;
    rig.live.release_messages(room.place);
    rig.model_when("only the thread is open", |model| {
        model.timeline(room.place).is_none()
    })
    .await;
    // The reply changes, and its read gets no answer, try after try.
    for _ in 0..8 {
        rig.http.fail("/api/v1/messages/", 503, "unavailable");
    }
    let end: &'static str = Box::leak(format!("/{}", first.id).into_boxed_str());
    for _ in 0..8 {
        rig.http.fail(end, 503, "unavailable");
    }
    room.changed(41, &first, false).await;
    rig.model_when("the thread is marked", |model| {
        model
            .timeline(thread)
            .is_some_and(|timeline| timeline.stale && timeline.failed.is_some())
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn the_end_of_a_mute_is_asked_for_until_the_server_says_so() {
    let mut rig = rig();
    let general = rig.scene.general;
    let other = AccountId::generate();
    // The clock of this machine is ahead: the mute has ended here, and the summary of
    // the snapshot says that the server still holds it.
    let until = time::OffsetDateTime::now_utc() - time::Duration::minutes(1);
    let place = rig.scene.text_place();
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.chat.own = Some(rig.scene.account_id);
        routes.chat.messages = vec![message(place, 1, other, "one")];
        routes.chat.muted = true;
    }
    rig.set_snapshot(40, |snapshot| {
        snapshot.notification_settings = vec![ScopedNotificationSettings {
            scope_id: general.as_uuid(),
            settings: NotificationSettings {
                mute: true,
                muted_until: Some(until),
                ..NotificationSettings::default()
            },
        }];
        snapshot.messages = vec![junto_protocol::message::MessageState {
            place,
            last_read: 0,
            latest: 1,
            unread: 1,
            mentions: 0,
            unread_sign: false,
            notification_mentions: 0,
            muted: true,
        }];
    });
    rig.want(ConnectionLevel::Selected, "jst_one");
    let _served = rig.served().await;
    // The summary is read, and read again while it still says "muted".
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(rig.http.count("/messages/read"), 1);
    tokio::time::sleep(Duration::from_secs(60)).await;
    let asked = rig.http.count("/messages/read");
    assert!((2..=4).contains(&asked), "{asked} reads");
    // The server ends the mute: the sign is shown, and the reads end.
    rig.http.routes.lock().unwrap().chat.muted = false;
    let model = rig
        .model_when("the sign", |model| {
            model
                .message_state(PlaceId::Channel(general))
                .is_some_and(|state| state.unread_sign && !state.muted)
        })
        .await;
    assert!(model.message_state(PlaceId::Channel(general)).is_some());
    let asked = rig.http.count("/messages/read");
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert_eq!(rig.http.count("/messages/read"), asked);
}

#[tokio::test(start_paused = true)]
async fn a_place_that_the_server_no_longer_gives_holds_no_message() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    assert_eq!(positions(&rig.model(), place), [1, 2, 3]);
    rig.http.fail("/messages", 404, "not_found");
    rig.live.read_latest_messages(place);
    let model = rig
        .model_when("the place without its messages", |model| {
            model
                .timeline(place)
                .is_some_and(|timeline| timeline.failed.is_some())
        })
        .await;
    let timeline = model.timeline(place).unwrap();
    assert!(timeline.messages.is_empty() && !timeline.loaded);
    assert_eq!(timeline.failed, Some(crate::model::ReadFailure::NotFound));
    // The server gives the place again: a reader who asks again gets its messages.
    rig.live.read_latest_messages(place);
    let model = rig
        .model_when("the messages again", |model| {
            model
                .timeline(place)
                .is_some_and(|timeline| timeline.loaded)
        })
        .await;
    assert_eq!(positions(&model, place), [1, 2, 3]);
    assert_eq!(model.timeline(place).unwrap().failed, None);
}

// The list asks for a page once per number of its rows (ADR 0740), so a page read that
// failed for a while is read again by the client core.
#[tokio::test(start_paused = true)]
async fn a_page_of_older_messages_that_failed_for_a_while_is_read_again() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let text_place = rig.scene.text_place();
    let other = room.other;
    room.chat(|chat| {
        chat.messages = (1..=300)
            .map(|position| message(text_place, position, other, "x"))
            .collect();
    });
    rig.live.read_latest_messages(place);
    rig.model_when("the latest page", |model| {
        positions(model, place).first() == Some(&251)
    })
    .await;
    rig.http.fail("/messages", 503, "unavailable");
    rig.http.fail("/messages", 503, "unavailable");
    rig.live.read_older_messages(place);
    let model = rig
        .model_when("the older page", |model| {
            positions(model, place).first() == Some(&201)
        })
        .await;
    assert!(!model.timeline(place).unwrap().reading.older);
}

#[tokio::test(start_paused = true)]
async fn a_page_that_starts_at_a_message_that_is_gone_does_not_empty_its_place() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let text_place = rig.scene.text_place();
    let other = room.other;
    room.chat(|chat| {
        chat.messages = (1..=300)
            .map(|position| message(text_place, position, other, "x"))
            .collect();
    });
    rig.live.read_latest_messages(place);
    rig.model_when("the latest page", |model| {
        positions(model, place).first() == Some(&251)
    })
    .await;
    // The first loaded message was deleted, and its event was not followed yet. The
    // server does not find the message that the page of older messages starts at.
    rig.http.fail("/messages", 404, "not_found");
    rig.live.read_older_messages(place);
    let model = rig
        .model_when("the page around the first message", |model| {
            positions(model, place).first() == Some(&222)
        })
        .await;
    let timeline = model.timeline(place).unwrap();
    assert!(timeline.loaded && timeline.failed.is_none());
}

#[tokio::test(start_paused = true)]
async fn a_late_answer_to_a_read_position_of_a_thread_does_not_move_the_position_back() {
    let mut rig = rig();
    let other = AccountId::generate();
    let root = message(rig.scene.text_place(), 1, other, "root");
    let state = |last_read: u64, unread: u64| ThreadState {
        root_id: root.id,
        last_read,
        latest: 5,
        unread,
        mentions: 0,
        following: true,
        follow_setting: None,
    };
    let text_place = rig.scene.text_place();
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.chat.own = Some(rig.scene.account_id);
        routes.chat.messages = vec![root.clone()];
        routes.chat.replies = (2..=5)
            .map(|position| {
                let mut reply = message(text_place, position, other, "reply");
                reply.thread_root = Some(root.id);
                reply
            })
            .collect();
        routes.chat.threads = vec![state(2, 3)];
        routes.snapshot.as_mut().unwrap().threads = vec![state(2, 3)];
    }
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let thread = PlaceId::Thread {
        channel_id: rig.scene.general,
        root_id: root.id,
    };
    soon("the thread", rig.live.open_messages(thread))
        .await
        .unwrap();

    // This device reads up to the third position. The answer of the server is late.
    let gate = rig.http.hold("PUT /thread/read");
    rig.live.mark_read(thread, 3);
    soon("the request", async {
        while rig.http.count("/thread/read") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    // Another device of the account reads up to the fourth position.
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.chat.threads = vec![state(4, 1)];
        routes.chat.thread_mark_answer = Some(state(3, 2));
    }
    peer.event(
        41,
        Event::ThreadRead {
            root_id: root.id,
            position: 4,
        },
    )
    .await;
    rig.model_when("the position of the other device", |model| {
        model.thread_state(root.id) == Some(&state(4, 1))
    })
    .await;
    gate.add_permits(64);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(rig.model().thread_state(root.id), Some(&state(4, 1)));
}

#[tokio::test(start_paused = true)]
async fn a_changed_message_that_the_server_answers_with_another_message_marks_its_place() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let second = room.chat(|chat| {
        let second = chat.messages[1].clone();
        chat.answered_wrongly = vec![second.id];
        second
    });
    room.changed(41, &second, false).await;
    // The change of the message is not in the state: the loaded messages are not
    // current, and the state says so.
    let model = rig
        .model_when("the place is marked", |model| {
            model
                .timeline(place)
                .is_some_and(|timeline| timeline.stale && timeline.failed.is_some())
        })
        .await;
    assert_eq!(positions(&model, place), [1, 2, 3]);
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert_eq!(room.sent_to(&format!("/api/v1/messages/{}", second.id)), 1);
}

#[tokio::test(start_paused = true)]
async fn a_page_with_only_messages_of_another_place_ends_the_reads_in_its_direction() {
    let room = room().await;
    let (rig, place) = (&room.rig, room.place);
    let text_place = rig.scene.text_place();
    let other = room.other;
    let foreign_place = junto_protocol::message::MessagePlace::Channel {
        workspace_id: rig.scene.workspace_id,
        channel_id: rig.scene.daily,
    };
    // Every message before the latest page names another channel.
    room.chat(|chat| {
        chat.messages = (1..=300)
            .map(|position| {
                let mut held = message(text_place, position, other, "x");
                if position <= 250 {
                    held.place = foreign_place;
                }
                held
            })
            .collect();
    });
    rig.live.read_latest_messages(place);
    rig.model_when("the latest page", |model| {
        positions(model, place).first() == Some(&251) && model.timeline(place).unwrap().more_before
    })
    .await;
    rig.live.read_older_messages(place);
    // No message of the answer is taken. The state does not promise older messages,
    // and says that the read failed.
    let model = rig
        .model_when("the refused page", |model| {
            model
                .timeline(place)
                .is_some_and(|timeline| timeline.failed.is_some() && !timeline.reading.older)
        })
        .await;
    let timeline = model.timeline(place).unwrap();
    assert!(!timeline.more_before);
    assert_eq!(positions(&model, place).first(), Some(&251));
    let asked = rig.http.count("/messages");
    rig.live.read_older_messages(place);
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(rig.http.count("/messages"), asked);
}

#[tokio::test(start_paused = true)]
async fn a_mute_with_a_later_end_is_not_asked_for_before_that_end() {
    let mut rig = rig();
    let general = rig.scene.general;
    let other = AccountId::generate();
    let until = time::OffsetDateTime::now_utc() + time::Duration::days(30);
    let place = rig.scene.text_place();
    let first = message(place, 1, other, "one");
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.chat.own = Some(rig.scene.account_id);
        routes.chat.messages = vec![first.clone()];
        routes.chat.muted = true;
    }
    rig.set_snapshot(40, |snapshot| {
        snapshot.notification_settings = vec![ScopedNotificationSettings {
            scope_id: general.as_uuid(),
            settings: NotificationSettings {
                mute: true,
                muted_until: Some(until),
                ..NotificationSettings::default()
            },
        }];
        snapshot.messages = vec![junto_protocol::message::MessageState {
            place,
            last_read: 0,
            latest: 1,
            unread: 1,
            mentions: 0,
            unread_sign: false,
            notification_mentions: 0,
            muted: true,
        }];
    });
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    assert_eq!(rig.http.count("/messages/read"), 0);
    // A new message asks for the summary once. The summary says "muted", which is
    // true until the end of the mute.
    let second = message(place, 2, other, "two");
    rig.http
        .routes
        .lock()
        .unwrap()
        .chat
        .messages
        .push(second.clone());
    peer.event(
        41,
        Event::MessageChanged {
            place: second.place,
            message_id: second.id,
            position: second.position,
            deleted: false,
            thread_root: None,
            thread: None,
        },
    )
    .await;
    soon("the summary", async {
        while rig.http.count("/messages/read") == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert_eq!(rig.http.count("/messages/read"), 1);
}

/// A burst of replies in a followed thread, each with its notice. A server of event
/// revision 2 states the notice and the thread state of each for the account, and the
/// client keeps both lists from the events: it reads neither list again (ADR 0409,
/// ADR 0412). Before, each reply read both lists, and a busy channel spent the reads that
/// the server allows an account in a minute.
#[tokio::test(start_paused = true)]
async fn a_burst_of_replies_keeps_the_notices_and_the_unread_threads_without_reading_them() {
    const BURST: u64 = 30;
    let room = room().await;
    // The server sends a frame at each heartbeat: the minute without a read below is no
    // silence of two heartbeats, after which the client would end the connection.
    room.peer.beat(Duration::from_secs(25));
    let rig = &room.rig;
    let root = root_of(&room);
    let lists = || (rig.http.count("/notifications"), room.sent_to(UNREAD));
    let before = lists();
    for number in 1..=BURST {
        let reply = reply(&room, &root, 3 + number, "a reply");
        let id = 40 + number * 2;
        room.peer
            .event(
                id,
                Event::MessageChanged {
                    place: reply.place,
                    message_id: reply.id,
                    position: reply.position,
                    deleted: false,
                    thread_root: Some(root.id),
                    thread: Some(unread(&root, 0, reply.position, number)),
                },
            )
            .await;
        room.peer
            .event(
                id + 1,
                Event::NotificationChanged {
                    message_id: reply.id,
                    notice: Some(Notification {
                        id: reply.id.as_uuid(),
                        message_id: reply.id,
                        kind: NotificationKind::Thread,
                        created_at: crate::model::tests::time()
                            + ::time::Duration::seconds(i64::try_from(number).unwrap()),
                        read_at: None,
                    }),
                },
            )
            .await;
    }
    let model = rig
        .model_when("every reply is noticed and counted", |model| {
            model.notices.len() == usize::try_from(BURST).unwrap()
                && model
                    .thread_state(root.id)
                    .is_some_and(|state| state.unread == BURST)
        })
        .await;
    assert!(
        model
            .notices
            .windows(2)
            .all(|pair| pair[0].created_at > pair[1].created_at)
    );
    assert_eq!(model.threads.len(), 1);
    tokio::time::sleep(std::time::Duration::from_secs(60)).await;
    assert_eq!(lists(), before);

    // A notice event that does not state the notice, as an older server sends it, reads
    // the list once.
    let last = reply(&room, &root, 3 + BURST + 1, "the last reply");
    room.peer
        .event(
            200,
            Event::NotificationChanged {
                message_id: last.id,
                notice: None,
            },
        )
        .await;
    soon("the list is read", async {
        while rig.http.count("/notifications") == before.0 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    assert_eq!(rig.http.count("/notifications"), before.0 + 1);
}

#[tokio::test(start_paused = true)]
async fn background_notices_refresh_the_thread_without_channel_events() {
    let mut rig = rig();
    let place = rig.scene.text_place();
    let root = message(place, 1, rig.scene.account_id, "root");
    let answer = message(place, 2, AccountId::generate(), "reply");
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.chat.own = Some(rig.scene.account_id);
        routes.chat.messages = vec![root.clone()];
        let mut reply = answer.clone();
        reply.thread_root = Some(root.id);
        routes.chat.replies = vec![reply];
        routes.chat.threads = vec![unread(&root, 0, 2, 1)];
    }
    rig.set_snapshot(40, |snapshot| {
        snapshot.threads = vec![unread(&root, 0, 2, 1)];
    });
    rig.want(ConnectionLevel::Background, "jst_one");
    let (peer, level, _) = rig.served_at(2).await;
    assert_eq!(level, Level::Background);
    rig.model_when("the starting thread", |model| model.threads.len() == 1)
        .await;
    rig.http.routes.lock().unwrap().chat.threads = vec![unread(&root, 0, 3, 2)];
    peer.event(
        41,
        Event::NotificationChanged {
            message_id: answer.id,
            notice: Some(Notification {
                id: answer.id.as_uuid(),
                message_id: answer.id,
                kind: NotificationKind::Thread,
                created_at: crate::model::tests::time(),
                read_at: None,
            }),
        },
    )
    .await;
    rig.model_when("the background thread count", |model| {
        model
            .thread_state(root.id)
            .is_some_and(|state| state.unread == 2)
    })
    .await;
}
