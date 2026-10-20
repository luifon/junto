use std::sync::Arc;

use junto_protocol::discovery::{
    Notification, NotificationKind, NotificationPage, NotificationSettings,
    ScopedNotificationSettings,
};
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, ConversationId, MessageId};
use junto_protocol::message::{Message, MessagePlace, ThreadState, UnreadThreads};
use junto_protocol::structure::ChannelKind;
use uuid::Uuid;

use super::message_tests::{changed, general, message, opened, state};
use super::tests::{Scene, channel, scene, time};
use super::*;
use crate::api::SettingsScope;

fn thread_of(scene: &Scene, root: &Message) -> PlaceId {
    PlaceId::Thread {
        channel_id: scene.general,
        root_id: root.id,
    }
}

fn reply(scene: &Scene, root: &Message, position: u64, text: &str) -> Message {
    let mut reply = message(scene.text_place(), position, AccountId::generate(), text);
    reply.thread_root = Some(root.id);
    reply
}

fn unread(root_id: MessageId, last_read: u64, latest: u64, unread: u64) -> ThreadState {
    ThreadState {
        root_id,
        last_read,
        latest,
        unread,
        mentions: 0,
        following: true,
        follow_setting: None,
    }
}

fn notice(message_id: MessageId) -> Notification {
    Notification {
        id: message_id.as_uuid(),
        message_id,
        kind: NotificationKind::Mention,
        created_at: time(),
        read_at: None,
    }
}

fn positions(model: &ServerModel, place: PlaceId) -> Vec<u64> {
    model
        .timeline(place)
        .unwrap()
        .messages
        .iter()
        .map(|message| message.position)
        .collect()
}

/// A model with the text channel open at the positions 1 to 3, and the thread under its
/// second message open with the replies at the positions 4 and 5.
fn with_thread(scene: &Scene) -> (ServerModel, Vec<Message>, PlaceId, Vec<Message>) {
    let (mut model, loaded) = opened(scene, 1..=3);
    let thread = thread_of(scene, &loaded[1]);
    model.open_timeline(thread);
    let replies = vec![
        reply(scene, &loaded[1], 4, "first"),
        reply(scene, &loaded[1], 5, "second"),
    ];
    let timeline = model.timeline_mut(thread).unwrap();
    timeline.replace(replies.clone(), false, false);
    timeline.root = Some(Arc::new(loaded[1].clone()));
    (model, loaded, thread, replies)
}

#[test]
fn a_snapshot_gives_the_unread_threads_the_notices_and_the_settings() {
    let mut scene = scene();
    let root_id = MessageId::generate();
    scene.snapshot.threads = vec![unread(root_id, 2, 9, 3)];
    scene.snapshot.threads_more = true;
    scene.snapshot.threads_next = Some("next".to_owned());
    scene.snapshot.notifications = vec![notice(MessageId::generate())];
    scene.snapshot.notification_settings = vec![ScopedNotificationSettings {
        scope_id: Uuid::nil(),
        settings: NotificationSettings {
            do_not_disturb: true,
            ..NotificationSettings::default()
        },
    }];
    let model = scene.model();
    assert_eq!(model.thread_state(root_id).unwrap().unread, 3);
    assert_eq!(model.threads_continuation(), Some("next"));
    assert_eq!(model.unread_notices().count(), 1);
    // One notice is not a full list of the snapshot: nothing follows it.
    assert!(!model.notices_more);
    assert!(model.is_quiet(|_| None));
    assert!(
        model
            .notification_settings(SettingsScope::Channel(scene.general))
            .is_none()
    );
    // The messages that the two lists name are not read yet.
    assert_eq!(model.missing_named().len(), 2);

    scene.snapshot.notifications = (0..50).map(|_| notice(MessageId::generate())).collect();
    assert!(scene.model().notices_more);
}

#[test]
fn a_reply_goes_into_its_thread_and_never_into_the_timeline_of_its_channel() {
    let scene = scene();
    let place = general(&scene);
    let (mut model, loaded, thread, _) = with_thread(&scene);

    // The event of a new reply names the channel. A holder wants the message: the
    // channel timeline reaches its end, and so does the thread.
    let third = reply(&scene, &loaded[1], 6, "third");
    assert_eq!(
        model.apply(&changed(&third, false)).load,
        [Part::MessageState(place), Part::Message(place, third.id)]
    );
    assert!(model.put_message(third.clone()));
    assert_eq!(positions(&model, thread), [4, 5, 6]);
    assert_eq!(positions(&model, place), [1, 2, 3]);

    // A reply of a thread that no reader has open goes nowhere.
    let elsewhere = reply(&scene, &loaded[0], 7, "other thread");
    assert!(!model.put_message(elsewhere));
    assert_eq!(positions(&model, place), [1, 2, 3]);

    // Without a timeline of the channel the open thread still asks for the message.
    model.close_timeline(place);
    let fourth = reply(&scene, &loaded[1], 8, "fourth");
    assert!(
        model
            .apply(&changed(&fourth, false))
            .load
            .contains(&Part::Message(place, fourth.id))
    );
    assert!(model.put_message(fourth));
    assert_eq!(positions(&model, thread), [4, 5, 6, 8]);
}

#[test]
fn the_root_of_an_open_thread_follows_its_message_and_ends_the_thread_when_it_is_gone() {
    let scene = scene();
    let place = general(&scene);
    let (mut model, loaded, thread, _) = with_thread(&scene);

    // The notice of the root after a reply: the root is read also when the channel
    // timeline would not take it.
    let timeline = model.timeline_mut(place).unwrap();
    timeline.more_before = true;
    timeline.messages.drain(..2);
    assert!(!model.timeline(place).unwrap().wants(loaded[1].id, 2));
    let mut root = loaded[1].clone();
    assert!(
        model
            .apply(&changed(&root, false))
            .load
            .contains(&Part::Message(place, root.id))
    );
    root.reply_count = 3;
    assert!(model.put_message(root.clone()));
    assert_eq!(
        model
            .timeline(thread)
            .unwrap()
            .root
            .as_ref()
            .unwrap()
            .reply_count,
        3
    );
    // The same root again changes nothing.
    assert!(!model.put_message(root.clone()));

    // A reply that the server no longer has leaves the thread.
    let first = model.timeline(thread).unwrap().messages[0].id;
    assert!(model.remove_message(place, first));
    assert_eq!(positions(&model, thread), [5]);

    // The root is gone: the thread holds nothing and says why.
    assert!(model.remove_message(place, root.id));
    let timeline = model.timeline(thread).unwrap();
    assert!(timeline.messages.is_empty() && timeline.root.is_none());
    assert_eq!(timeline.failed, Some(ReadFailure::NotFound));
}

#[test]
fn a_deleted_channel_takes_its_threads_its_pins_and_the_entries_that_lie_in_it() {
    let scene = scene();
    let (mut model, loaded, thread, _) = with_thread(&scene);
    model.open_pins(scene.general);
    model.threads = vec![unread(loaded[1].id, 4, 5, 1)];
    assert!(model.put_message(loaded[1].clone()));
    assert!(matches!(model.named(loaded[1].id), Some(Named::Read(_))));
    let waits = PendingMessage::new(
        1,
        thread,
        super::message_tests::document("x"),
        None,
        Vec::new(),
        time(),
    );
    model.outbox.push(Arc::new(waits));

    model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
    });
    assert!(model.timelines.is_empty());
    assert!(model.outbox.is_empty());
    assert!(model.pins.is_empty());
    assert!(model.threads.is_empty());
    assert!(model.named.is_empty());
}

#[test]
fn the_unread_threads_are_asked_for_after_an_event_of_a_message_that_is_not_new() {
    let mut scene = scene();
    scene.snapshot.messages = vec![state(scene.text_place(), 3, 3, 0)];
    let mut model = scene.model();
    let other = AccountId::generate();
    let asks = |model: &mut ServerModel, held: &Message| {
        model
            .apply(&changed(held, false))
            .load
            .contains(&Part::UnreadThreads)
    };

    // A new message of the channel is no change of a thread.
    let fourth = message(scene.text_place(), 4, other, "four");
    assert!(!asks(&mut model, &fourth));
    // A reply is new too. The notice of its root follows, with a lower position.
    let root = message(scene.text_place(), 2, other, "root");
    let answer = reply(&scene, &root, 5, "reply");
    assert!(!asks(&mut model, &answer));
    assert!(asks(&mut model, &root));
    // The newest message again: an edit can add a mention in a thread (ADR 0409).
    assert!(asks(&mut model, &answer));
    // The highest position stays known when a summary that is older arrives.
    model.set_message_state(state(scene.text_place(), 3, 4, 1));
    let fifth_again = message(scene.text_place(), 5, other, "edited");
    assert!(asks(&mut model, &fifth_again));

    // A direct conversation has no thread.
    let direct = MessagePlace::Direct {
        conversation_id: ConversationId::generate(),
        low_account_id: scene.account_id,
        high_account_id: other,
    };
    let first = message(direct, 1, other, "hello");
    assert!(!asks(&mut model, &first));
    assert!(!asks(&mut model, &first));
}

#[test]
fn a_thread_that_is_read_leaves_the_list_and_the_summaries_with_mentions_are_read() {
    let mut scene = scene();
    let (read, partly) = (MessageId::generate(), MessageId::generate());
    scene.snapshot.threads = vec![unread(read, 2, 9, 3), unread(partly, 1, 8, 2)];
    let mut summary = state(scene.text_place(), 3, 9, 0);
    summary.mentions = 1;
    scene.snapshot.messages = vec![summary];
    let mut model = scene.model();
    let place = general(&scene);
    let thread_read = |root_id, position| Event::ThreadRead { root_id, position };

    // The thread is read to its latest reply: it leaves the list. The event names no
    // channel, so the summary of the channel that shows a mention is read.
    assert_eq!(
        model.apply(&thread_read(read, 9)).load,
        [Part::MessageState(place)]
    );
    assert!(model.thread_state(read).is_none());

    // Something stays unread: the counts are not known, and the list is read.
    assert_eq!(
        model.apply(&thread_read(partly, 5)).load,
        [Part::MessageState(place), Part::UnreadThreads]
    );
    assert_eq!(model.thread_state(partly).unwrap().last_read, 5);
    // A lower position changes nothing.
    model.apply(&thread_read(partly, 3));
    assert_eq!(model.thread_state(partly).unwrap().last_read, 5);

    // A thread that the list does not hold: the list is read.
    assert!(
        model
            .apply(&thread_read(MessageId::generate(), 4))
            .load
            .contains(&Part::UnreadThreads)
    );
}

#[test]
fn pages_of_unread_threads_replace_and_continue_the_list() {
    let scene = scene();
    let mut model = scene.model();
    let roots: Vec<MessageId> = (0..4).map(|_| MessageId::generate()).collect();
    let page = |roots: &[MessageId], more: bool| UnreadThreads {
        threads: roots.iter().map(|root| unread(*root, 1, 5, 2)).collect(),
        more,
        next: more.then(|| "token".to_owned()),
    };
    model.set_threads(page(&roots[..2], true));
    assert_eq!(model.threads_continuation(), Some("token"));
    // The next page adds what the list does not hold.
    model.add_threads(page(&roots[1..], false));
    assert_eq!(
        model
            .threads
            .iter()
            .map(|held| held.root_id)
            .collect::<Vec<_>>(),
        roots
    );
    assert_eq!(model.threads_continuation(), None);

    // The answer to a read position of a thread: counts change, and a thread without
    // anything unread leaves. A thread that the list does not hold is not added.
    assert!(model.set_thread_state(unread(roots[0], 3, 5, 1)));
    assert_eq!(model.thread_state(roots[0]).unwrap().unread, 1);
    assert!(model.set_thread_state(unread(roots[0], 5, 5, 0)));
    assert!(model.thread_state(roots[0]).is_none());
    assert!(!model.set_thread_state(unread(MessageId::generate(), 1, 5, 2)));

    // A new first page replaces every page.
    model.set_threads(page(&roots[3..], false));
    assert_eq!(model.threads.len(), 1);
}

#[test]
fn the_messages_that_the_lists_name_are_kept_while_a_list_names_them() {
    let scene = scene();
    let mut model = scene.model();
    let place = general(&scene);
    let other = AccountId::generate();
    let root = message(scene.text_place(), 2, other, "root");
    let noticed = message(scene.text_place(), 7, other, "@you");
    let unnamed = message(scene.text_place(), 8, other, "plain");

    model.set_threads(UnreadThreads {
        threads: vec![unread(root.id, 2, 9, 1)],
        more: false,
        next: None,
    });
    model.set_notices(NotificationPage {
        notifications: vec![notice(noticed.id)],
        has_more: false,
    });
    assert_eq!(model.missing_named(), [root.id, noticed.id]);
    assert!(model.put_message(root.clone()));
    assert!(model.put_message(noticed.clone()));
    // A message that no list names is not kept.
    assert!(!model.put_message(unnamed.clone()));
    assert!(model.named(unnamed.id).is_none());
    assert!(model.missing_named().is_empty());

    // An event that names one of them reads it again, without a timeline.
    assert!(
        model
            .apply(&changed(&root, false))
            .load
            .contains(&Part::Message(place, root.id))
    );
    assert!(
        !model
            .apply(&changed(&unnamed, false))
            .load
            .contains(&Part::Message(place, unnamed.id))
    );

    // The server has the message no longer; a read that failed for good is told apart.
    assert!(model.remove_message(place, noticed.id));
    assert_eq!(model.named(noticed.id), Some(&Named::Gone));
    assert!(model.settle_named(root.id, false));
    assert_eq!(model.named(root.id), Some(&Named::Failed));

    // A list that names the message no longer drops it.
    model.set_notices(NotificationPage {
        notifications: Vec::new(),
        has_more: false,
    });
    assert!(model.named(noticed.id).is_none());
    assert!(model.named(root.id).is_some());
}

#[test]
fn an_archived_channel_keeps_its_notices_and_a_deleted_one_does_not() {
    let scene = scene();
    let mut model = scene.model();
    let other = AccountId::generate();
    let root = message(scene.text_place(), 2, other, "root");
    let noticed = message(scene.text_place(), 7, other, "@you");
    model.set_threads(UnreadThreads {
        threads: vec![unread(root.id, 2, 9, 1)],
        more: false,
        next: None,
    });
    model.set_notices(NotificationPage {
        notifications: vec![notice(noticed.id)],
        has_more: false,
    });
    assert!(model.put_message(root.clone()));
    assert!(model.put_message(noticed.clone()));

    // No reader has the channel open: its unread thread leaves the list, and its notice
    // stays with its message (ADR 0410).
    let channel = model.channel(scene.general).unwrap().channel.clone();
    model.apply(&Event::ChannelArchived {
        channel: junto_protocol::structure::ArchivedChannel {
            channel,
            archived_at: time(),
            deletes_at: None,
        },
    });
    assert!(model.channel(scene.general).is_none());
    assert!(model.threads.is_empty() && model.named(root.id).is_none());
    assert_eq!(model.notices.len(), 1);
    assert!(matches!(model.named(noticed.id), Some(Named::Read(_))));

    // A deleted channel leaves no notice.
    model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
    });
    assert!(model.notices.is_empty() && model.named(noticed.id).is_none());
}

#[test]
fn a_deleted_workspace_takes_the_notices_of_its_unread_archived_channels() {
    let scene = scene();
    let mut model = scene.model();
    let other = AccountId::generate();
    let noticed = message(scene.text_place(), 7, other, "@you");
    model.set_notices(NotificationPage {
        notifications: vec![notice(noticed.id)],
        has_more: false,
    });
    assert!(model.put_message(noticed.clone()));
    // The channel is archived while no reader has it open and no archived list is read:
    // the state does not know the channel, and the notice stays.
    let channel = model.channel(scene.general).unwrap().channel.clone();
    model.apply(&Event::ChannelArchived {
        channel: junto_protocol::structure::ArchivedChannel {
            channel,
            archived_at: time(),
            deletes_at: None,
        },
    });
    assert!(model.admin.archived_channel(scene.general).is_none());
    assert_eq!(model.notices.len(), 1);

    // The workspace is deleted: its notices leave with it.
    model.apply(&Event::WorkspaceDeleted {
        workspace_id: scene.workspace_id,
    });
    assert!(model.notices.is_empty() && model.named(noticed.id).is_none());
}

#[test]
fn notices_that_an_event_takes_whole_are_read_again_when_the_server_holds_more() {
    let scene = scene();
    let other = AccountId::generate();
    let noticed = message(scene.text_place(), 7, other, "@you");
    let held = |more: bool| {
        let mut model = scene.model();
        model.set_notices(NotificationPage {
            notifications: vec![notice(noticed.id)],
            has_more: more,
        });
        assert!(model.put_message(noticed.clone()));
        model
    };

    // The held notices lie in the deleted workspace, and the server holds older ones of
    // other places: no notice is left to continue from, so the first page is read again.
    let mut model = held(true);
    let follow = model.apply(&Event::WorkspaceDeleted {
        workspace_id: scene.workspace_id,
    });
    assert!(model.notices.is_empty() && model.notices_more);
    assert!(follow.load.contains(&Part::Notices), "{:?}", follow.load);

    // The same for a deleted channel.
    let mut model = held(true);
    let follow = model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
    });
    assert!(follow.load.contains(&Part::Notices), "{:?}", follow.load);

    // Without more notices on the server the empty list is the whole list.
    let mut model = held(false);
    let follow = model.apply(&Event::WorkspaceDeleted {
        workspace_id: scene.workspace_id,
    });
    assert!(!follow.load.contains(&Part::Notices), "{:?}", follow.load);

    // An answer that was read before the deletion holds the deleted notices again, and the
    // state does not place their messages any more: the first page is read again.
    let mut model = held(true);
    let deleted = Event::WorkspaceDeleted {
        workspace_id: scene.workspace_id,
    };
    model.apply(&deleted);
    model.set_notices(NotificationPage {
        notifications: vec![notice(noticed.id)],
        has_more: true,
    });
    assert_eq!(
        model.replay_reads(Part::Notices, [&deleted]),
        [Part::Notices]
    );
    let channel_deleted = Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
    };
    assert_eq!(
        model.replay_reads(Part::Notices, [&channel_deleted]),
        [Part::Notices]
    );
}

#[test]
fn notices_are_replaced_by_their_first_page_and_continued_from_their_last_one() {
    let scene = scene();
    let mut model = scene.model();
    let first: Vec<Notification> = (0..3).map(|_| notice(MessageId::generate())).collect();
    model.set_notices(NotificationPage {
        notifications: first.clone(),
        has_more: true,
    });
    assert!(model.notices_more);
    let next = notice(MessageId::generate());
    // A page that continues another list than the one that is held is not added.
    let stale = NotificationPage {
        notifications: vec![next.clone()],
        has_more: false,
    };
    assert!(!model.add_notices(first[0].id, stale.clone()));
    assert_eq!(model.notices.len(), 3);
    assert!(model.add_notices(first[2].id, stale));
    assert_eq!(model.notices.len(), 4);
    assert!(!model.notices_more);

    // A notice event asks for the notices and for the unread threads; a settings event
    // names nothing and reloads the state.
    assert_eq!(
        model
            .apply(&Event::NotificationChanged {
                message_id: next.message_id
            })
            .load,
        [Part::Notices, Part::UnreadThreads]
    );
    assert!(model.apply(&Event::NotificationSettingsChanged {}).reload);
    assert_eq!(
        model.apply(&Event::RetentionChanged { workspace_id: None }),
        Follow::default()
    );
}

#[test]
fn an_open_pin_list_follows_the_single_messages_of_its_channel() {
    let scene = scene();
    let mut model = scene.model();
    let place = general(&scene);
    let other = AccountId::generate();
    let mut first = message(scene.text_place(), 1, other, "one");
    first.pinned = true;
    let mut second = message(scene.text_place(), 2, other, "two");

    // No reader: a pinned message is kept nowhere, and its event asks for no message.
    assert!(!model.put_message(first.clone()));
    assert!(
        !model
            .apply(&changed(&first, false))
            .load
            .contains(&Part::Message(place, first.id))
    );

    model.open_pins(scene.general);
    // The list is not read yet: it is not built from single messages.
    assert!(!model.put_message(first.clone()));
    model.set_pins(scene.general, vec![first.clone()]);
    let ids = |model: &ServerModel| -> Vec<MessageId> {
        let list = model.pin_list(scene.general).unwrap();
        list.messages
            .loaded()
            .unwrap()
            .iter()
            .map(|pin| pin.id)
            .collect()
    };
    // With the list open every event of the channel asks for its message.
    assert!(
        model
            .apply(&changed(&second, false))
            .load
            .contains(&Part::Message(place, second.id))
    );
    // A message without the pin changes nothing; with the pin it is added at the end.
    assert!(!model.put_message(second.clone()));
    second.pinned = true;
    assert!(model.put_message(second.clone()));
    assert_eq!(ids(&model), [first.id, second.id]);
    // An edit of a pinned message replaces its copy.
    first.document = Some(super::message_tests::document("one, edited"));
    assert!(model.put_message(first.clone()));
    assert_eq!(ids(&model), [first.id, second.id]);
    // The pin is taken away, and a pinned message is deleted.
    first.pinned = false;
    assert!(model.put_message(first));
    assert!(model.remove_message(place, second.id));
    assert!(ids(&model).is_empty());

    // A pinned reply of a thread belongs to the list of its channel.
    let root = message(scene.text_place(), 3, other, "root");
    let mut pinned_reply = reply(&scene, &root, 4, "pinned reply");
    pinned_reply.pinned = true;
    assert!(model.put_message(pinned_reply.clone()));
    assert_eq!(ids(&model), [pinned_reply.id]);

    model.close_pins(scene.general);
    assert!(model.pin_list(scene.general).is_none());
}

#[test]
fn a_new_or_deleted_channel_asks_for_the_forks_of_the_open_threads() {
    let scene = scene();
    let (mut model, _, thread, _) = with_thread(&scene);
    let fork = channel(scene.workspace_id, ChannelKind::Text, "fork", 9);
    assert_eq!(
        model
            .apply(&Event::ChannelCreated {
                channel: fork.clone()
            })
            .load,
        [Part::ThreadHead(thread)]
    );
    // A change of a channel is no fork.
    assert_eq!(
        model.apply(&Event::ChannelUpdated {
            channel: fork.clone()
        }),
        Follow::default()
    );
    assert_eq!(
        model
            .apply(&Event::ChannelDeleted {
                workspace_id: scene.workspace_id,
                channel_id: fork.id
            })
            .load,
        [Part::ThreadHead(thread)]
    );
}

#[test]
fn the_end_of_a_mute_is_known_for_the_channels_that_the_state_holds() {
    let mut scene = scene();
    let now = time();
    let muted = |until| NotificationSettings {
        mute: true,
        muted_until: until,
        ..NotificationSettings::default()
    };
    let scoped = |scope_id, settings| ScopedNotificationSettings { scope_id, settings };
    scene.snapshot.notification_settings = vec![
        scoped(
            scene.general.as_uuid(),
            muted(Some(now + time::Duration::hours(1))),
        ),
        // Without an end, with an end that has passed, and of a channel that is not held.
        scoped(scene.daily.as_uuid(), muted(None)),
        scoped(
            scene.hidden.as_uuid(),
            muted(Some(now - time::Duration::hours(1))),
        ),
        scoped(Uuid::now_v7(), muted(Some(now + time::Duration::hours(1)))),
    ];
    let model = scene.model();
    assert_eq!(
        model.mute_ends(now),
        [(scene.general, now + time::Duration::hours(1))]
    );
    assert!(!model.is_quiet(|_| Some(0)));
}

#[test]
fn a_message_that_waits_for_a_thread_is_confirmed_by_a_reply_of_that_thread_only() {
    let scene = scene();
    let (mut model, loaded, thread, _) = with_thread(&scene);
    let text = super::message_tests::document("same text");
    let mut waits = PendingMessage::new(7, thread, text.clone(), None, Vec::new(), time());
    waits.state = SendState::Unconfirmed;
    waits.after = 5;
    model.outbox.push(Arc::new(waits));

    // The same text in the channel is another message.
    let mut in_channel = message(scene.text_place(), 6, scene.account_id, "");
    in_channel.document = Some(text.clone());
    model.put_message(in_channel);
    assert!(!model.confirm_arrived(thread));

    let mut in_thread = reply(&scene, &loaded[1], 7, "");
    in_thread.author_id = Some(scene.account_id);
    in_thread.document = Some(text);
    model.put_message(in_thread);
    assert!(model.confirm_arrived(thread));
    assert!(model.outbox.is_empty());
}

#[test]
fn a_read_position_asks_for_the_notices_that_it_can_have_marked() {
    let mut scene = scene();
    scene.snapshot.messages = vec![state(scene.text_place(), 3, 9, 6)];
    let mut model = scene.model();
    let other = AccountId::generate();
    let in_channel = message(scene.text_place(), 7, other, "@you");
    let root = message(scene.text_place(), 2, other, "root");
    let in_thread = reply(&scene, &root, 8, "@you in a thread");
    let read = |position| Event::MessageRead {
        place: scene.text_place(),
        position,
    };
    let asks =
        |model: &mut ServerModel, event: &Event| model.apply(event).load.contains(&Part::Notices);

    // Without an unread notice no read position asks for the notices.
    assert!(!asks(&mut model, &read(4)));

    model.set_notices(NotificationPage {
        notifications: vec![notice(in_channel.id), notice(in_thread.id)],
        has_more: false,
    });
    // The messages of the notices are not read yet: their places are not known.
    assert!(asks(&mut model, &read(5)));
    model.put_message(in_channel.clone());
    model.put_message(in_thread.clone());

    // A position before the message of the notice marks nothing.
    assert!(!asks(&mut model, &read(6)));
    // The position of the channel reaches the message of the channel, not the reply.
    assert!(asks(&mut model, &read(7)));
    model.notices[0].read_at = Some(time());
    assert!(!asks(&mut model, &read(9)));
    // The read position of the thread reaches the reply.
    let thread_read = |root_id, position| Event::ThreadRead { root_id, position };
    assert!(!asks(&mut model, &thread_read(root.id, 7)));
    assert!(!asks(&mut model, &thread_read(MessageId::generate(), 8)));
    assert!(asks(&mut model, &thread_read(root.id, 8)));
}

#[test]
fn the_root_of_a_thread_is_taken_only_for_the_thread_that_was_opened() {
    let scene = scene();
    let (mut model, loaded, thread, replies) = with_thread(&scene);
    let root = loaded[1].clone();
    let fork = junto_protocol::ids::ChannelId::generate();
    let held = |model: &ServerModel| model.timeline(thread).unwrap().root.clone().unwrap();

    let mut changed = root.clone();
    changed.reply_count = 9;
    assert!(model.set_thread_head(thread, changed, vec![fork]));
    assert_eq!(held(&model).reply_count, 9);
    assert_eq!(model.timeline(thread).unwrap().forks, [fork]);

    // Another message of the channel, a reply, and the root under another channel.
    assert!(!model.set_thread_head(thread, loaded[0].clone(), Vec::new()));
    assert!(!model.set_thread_head(thread, replies[0].clone(), Vec::new()));
    let mut elsewhere = root.clone();
    elsewhere.place = MessagePlace::Channel {
        workspace_id: scene.workspace_id,
        channel_id: scene.daily,
    };
    assert!(!model.set_thread_head(thread, elsewhere, Vec::new()));
    assert_eq!(held(&model).id, root.id);
    assert_eq!(model.timeline(thread).unwrap().forks, [fork]);
}

#[test]
fn a_pin_list_takes_only_the_messages_of_its_channel() {
    let scene = scene();
    let mut model = scene.model();
    let other = AccountId::generate();
    let mut own = message(scene.text_place(), 1, other, "of the channel");
    own.pinned = true;
    let mut foreign = message(
        MessagePlace::Channel {
            workspace_id: scene.workspace_id,
            channel_id: scene.daily,
        },
        2,
        other,
        "of another channel",
    );
    foreign.pinned = true;
    model.open_pins(scene.general);
    model.set_pins(scene.general, vec![foreign, own.clone()]);
    let pins = model
        .pin_list(scene.general)
        .unwrap()
        .messages
        .loaded()
        .unwrap();
    assert_eq!(pins.len(), 1);
    assert_eq!(pins[0].id, own.id);
}

#[test]
fn a_read_place_keeps_no_sign_and_asks_for_the_counts_while_mentions_can_remain() {
    let mut scene = scene();
    let mut summary = state(scene.text_place(), 3, 9, 6);
    summary.unread_sign = true;
    scene.snapshot.messages = vec![summary.clone()];
    let place = general(&scene);
    let read = Event::MessageRead {
        place: scene.text_place(),
        position: 9,
    };
    // Nothing mentions the account: everything is read, and no sign stays.
    let mut model = scene.model();
    assert_eq!(model.apply(&read), Follow::default());
    let held = model.message_state(place).unwrap();
    assert!(!held.unread_sign);
    assert_eq!((held.unread, held.notification_mentions), (0, 0));

    // A mention can be one of a thread, which the read position of the channel does
    // not reach (ADR 0409): the counts are asked for.
    summary.mentions = 1;
    summary.notification_mentions = 1;
    scene.snapshot.messages = vec![summary];
    let mut model = scene.model();
    assert_eq!(model.apply(&read).load, [Part::MessageState(place)]);
    assert_eq!(model.message_state(place).unwrap().unread, 0);
}

#[test]
fn the_forks_of_a_thread_are_read_without_an_older_copy_of_its_root() {
    let scene = scene();
    let (mut model, loaded, thread, _) = with_thread(&scene);
    let mut newer = loaded[1].clone();
    newer.reply_count = 9;
    assert!(model.put_message(newer));
    let fork = junto_protocol::ids::ChannelId::generate();
    // The answer for the forks was made before the root changed.
    assert!(model.set_thread_forks(thread, loaded[1].clone(), vec![fork]));
    let timeline = model.timeline(thread).unwrap();
    assert_eq!(timeline.root.as_ref().unwrap().reply_count, 9);
    assert_eq!(timeline.forks, [fork]);
    // The answer for another thread changes nothing.
    assert!(!model.set_thread_forks(thread, loaded[0].clone(), Vec::new()));
    assert_eq!(model.timeline(thread).unwrap().forks, [fork]);
}

#[test]
fn a_notice_whose_message_was_not_read_can_be_marked_by_any_read_position() {
    let scene = scene();
    let mut model = scene.model();
    let unknown = MessageId::generate();
    model.set_notices(NotificationPage {
        notifications: vec![notice(unknown)],
        has_more: false,
    });
    assert!(model.settle_named(unknown, false));
    let read = Event::MessageRead {
        place: scene.text_place(),
        position: 4,
    };
    assert!(model.apply(&read).load.contains(&Part::Notices));
    // A message that the server does not have marks nothing.
    assert!(model.settle_named(unknown, true));
    assert!(!model.apply(&read).load.contains(&Part::Notices));
}

#[test]
fn a_reload_keeps_the_named_messages_for_display_until_each_is_read_again() {
    let scene = scene();
    let other = AccountId::generate();
    let noticed = message(scene.text_place(), 7, other, "@you");
    let dropped = message(scene.text_place(), 8, other, "@you again");
    let page = |messages: &[&Message]| NotificationPage {
        notifications: messages.iter().map(|held| notice(held.id)).collect(),
        has_more: false,
    };
    let mut before = scene.model();
    before.set_notices(page(&[&noticed, &dropped]));
    before.put_message(noticed.clone());
    before.put_message(dropped.clone());
    assert!(before.missing_named().is_empty());

    // The new state names one of the two messages. It shows the message of before and
    // reads it again; the other one is not taken.
    let mut after = scene.model();
    after.set_notices(page(&[&noticed]));
    after.carry_named(&before);
    assert!(matches!(after.named(noticed.id), Some(Named::Read(held)) if held.id == noticed.id));
    assert!(after.named(dropped.id).is_none());
    assert_eq!(after.missing_named(), [noticed.id]);
    assert!(after.needs_named(noticed.id));
    // The read of the message ends the carried mark, also when the message is the same.
    assert!(after.put_message(noticed.clone()));
    assert!(after.missing_named().is_empty());
    assert!(!after.needs_named(noticed.id));

    // Another account takes nothing of the state before.
    let mut other_account = scene.model();
    other_account.account.id = AccountId::generate();
    other_account.set_notices(page(&[&noticed]));
    other_account.carry_named(&before);
    assert!(other_account.named(noticed.id).is_none());
}
