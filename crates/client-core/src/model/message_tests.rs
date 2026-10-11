use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, AttachmentId, ConversationId, MessageId};
use junto_protocol::message::{
    Attachment, AttachmentLimits, Block, Conversation, Document, Inline, Message, MessagePage,
    MessagePlace, MessageState,
};

use super::tests::{Scene, scene, time};
use super::*;

pub(crate) fn document(text: &str) -> Document {
    Document {
        blocks: vec![Block::Paragraph {
            content: vec![Inline::Text {
                text: text.to_owned(),
                marks: Vec::new(),
            }],
        }],
    }
}

pub(crate) fn message(
    place: MessagePlace,
    position: u64,
    author: AccountId,
    text: &str,
) -> Message {
    Message {
        id: MessageId::generate(),
        place,
        position,
        author_id: Some(author),
        author_name: Some("Ana".to_owned()),
        created_at: time(),
        edited_at: None,
        document: Some(document(text)),
        system: None,
        reply_to: None,
        permanent: false,
        deleted: false,
        attachments: Vec::new(),
        reactions: Vec::new(),
        thread_root: None,
        reply_count: 0,
        last_reply: None,
        pinned: false,
        files_removed_by_retention: false,
    }
}

impl Scene {
    pub(crate) fn text_place(&self) -> MessagePlace {
        MessagePlace::Channel {
            workspace_id: self.workspace_id,
            channel_id: self.general,
        }
    }
}

pub(crate) fn general(scene: &Scene) -> PlaceId {
    PlaceId::Channel(scene.general)
}

/// A model with the timeline of the text channel open and loaded with `positions`.
pub(crate) fn opened(
    scene: &Scene,
    positions: std::ops::RangeInclusive<u64>,
) -> (ServerModel, Vec<Message>) {
    let mut model = scene.model();
    let place = general(scene);
    model.open_timeline(place);
    let other = AccountId::generate();
    let messages: Vec<Message> = positions
        .map(|position| message(scene.text_place(), position, other, "text"))
        .collect();
    model
        .timeline_mut(place)
        .unwrap()
        .replace(messages.clone(), false, false);
    (model, messages)
}

pub(crate) fn changed(message: &Message, deleted: bool) -> Event {
    Event::MessageChanged {
        place: message.place,
        message_id: message.id,
        position: message.position,
        deleted,
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

pub(crate) fn state(place: MessagePlace, last_read: u64, latest: u64, unread: u64) -> MessageState {
    MessageState {
        place,
        last_read,
        latest,
        unread,
        mentions: 0,
        unread_sign: unread > 0,
        notification_mentions: 0,
        muted: false,
    }
}

#[test]
fn a_snapshot_gives_the_unread_summaries_and_the_direct_conversations() {
    let mut scene = scene();
    let other = AccountId::generate();
    let conversation_id = ConversationId::generate();
    let direct = MessagePlace::Direct {
        conversation_id,
        low_account_id: scene.account_id.min(other),
        high_account_id: scene.account_id.max(other),
    };
    scene.snapshot.messages = vec![state(scene.text_place(), 3, 9, 6), state(direct, 0, 2, 2)];
    let model = scene.model();
    assert_eq!(model.message_state(general(&scene)).unwrap().unread, 6);
    assert_eq!(
        model
            .message_state(PlaceId::Direct(conversation_id))
            .unwrap()
            .latest,
        2
    );
    assert!(model.message_state(PlaceId::Channel(scene.daily)).is_none());
    assert_eq!(model.conversations.len(), 1);
    assert_eq!(model.conversation_with(other).unwrap().id, conversation_id);
    assert!(model.conversation_with(AccountId::generate()).is_none());
    assert_eq!(model.place(PlaceId::Direct(conversation_id)), Some(direct));
    assert_eq!(model.place(general(&scene)), Some(scene.text_place()));
    assert!(model.timelines.is_empty() && model.outbox.is_empty());
    assert_eq!(model.attachment_limits, None);
}

#[test]
fn a_changed_message_asks_for_the_summary_and_for_what_a_reader_has_loaded() {
    let scene = scene();
    let place = general(&scene);
    let other = AccountId::generate();
    let outside = message(scene.text_place(), 4, other, "new");

    // Without a reader the event asks for the unread summary only.
    let mut model = scene.model();
    assert_eq!(
        model.apply(&changed(&outside, false)).load,
        [Part::MessageState(place)]
    );
    // A place that the state does not hold asks for nothing.
    let mut unseen = outside.clone();
    unseen.place = MessagePlace::Channel {
        workspace_id: scene.workspace_id,
        channel_id: junto_protocol::ids::ChannelId::generate(),
    };
    assert_eq!(model.apply(&changed(&unseen, false)), Follow::default());

    // A timeline that reaches the end takes a new message, and one that it holds.
    let (mut model, loaded) = opened(&scene, 1..=3);
    assert_eq!(
        model.apply(&changed(&outside, false)).load,
        [Part::MessageState(place), Part::Message(place, outside.id)]
    );
    // An event of a message that is not new can be the notice of the root of a thread:
    // it also asks for the unread threads (ADR 0722).
    assert_eq!(
        model.apply(&changed(&loaded[1], true)).load,
        [
            Part::MessageState(place),
            Part::Message(place, loaded[1].id),
            Part::UnreadThreads
        ]
    );

    // A timeline with more messages after its end does not take what lies beyond it.
    model.timeline_mut(place).unwrap().more_after = true;
    let beyond = message(scene.text_place(), 5, other, "newer");
    assert_eq!(
        model.apply(&changed(&beyond, false)).load,
        [Part::MessageState(place)]
    );
    // A line that shares the position of a loaded message lies inside the range.
    let shared = message(scene.text_place(), 2, other, "private line");
    assert!(model.timeline(place).unwrap().wants(shared.id, 2));
    // Before the first loaded message while the history has more before it: not taken.
    model.timeline_mut(place).unwrap().more_before = true;
    let earlier = message(scene.text_place(), 0, other, "earlier");
    assert!(!model.timeline(place).unwrap().wants(earlier.id, 0));
}

#[test]
fn messages_keep_the_order_of_the_server_and_one_copy_of_each() {
    let scene = scene();
    let place = general(&scene);
    let (mut model, loaded) = opened(&scene, 1..=3);
    let other = AccountId::generate();

    // A message that arrives after a later one is placed before it.
    let fifth = message(scene.text_place(), 5, other, "five");
    let fourth = message(scene.text_place(), 4, other, "four");
    assert!(model.put_message(fifth.clone()));
    assert!(model.put_message(fourth));
    assert_eq!(positions(&model, place), [1, 2, 3, 4, 5]);

    // The same content again changes nothing; another content replaces the copy.
    assert!(!model.put_message(fifth.clone()));
    let mut edited = fifth.clone();
    edited.document = Some(document("edited"));
    edited.edited_at = Some(time());
    assert!(model.put_message(edited.clone()));
    assert_eq!(positions(&model, place), [1, 2, 3, 4, 5]);
    assert_eq!(
        model
            .timeline(place)
            .unwrap()
            .message(fifth.id)
            .unwrap()
            .document,
        edited.document
    );

    // Two lines that share a position are ordered by their identifiers.
    let line = message(scene.text_place(), 5, other, "line");
    assert!(model.put_message(line.clone()));
    let at_five: Vec<MessageId> = model
        .timeline(place)
        .unwrap()
        .messages
        .iter()
        .filter(|held| held.position == 5)
        .map(|held| held.id)
        .collect();
    let mut expected = vec![fifth.id, line.id];
    expected.sort();
    assert_eq!(at_five, expected);

    assert!(model.remove_message(place, loaded[0].id));
    assert!(!model.remove_message(place, loaded[0].id));
    assert_eq!(positions(&model, place), [2, 3, 4, 5, 5]);

    // A message of a place without a reader is not held.
    let mut elsewhere = message(scene.text_place(), 1, other, "voice chat");
    elsewhere.place = MessagePlace::Channel {
        workspace_id: scene.workspace_id,
        channel_id: scene.daily,
    };
    assert!(!model.put_message(elsewhere));
}

#[test]
fn pages_extend_the_range_at_both_ends_without_a_second_copy() {
    let scene = scene();
    let place = general(&scene);
    let (mut model, loaded) = opened(&scene, 10..=12);
    let other = AccountId::generate();
    let page = |positions: std::ops::RangeInclusive<u64>, has_more| MessagePage {
        messages: positions
            .map(|position| message(scene.text_place(), position, other, "page"))
            .collect(),
        has_more,
    };
    let timeline = model.timeline_mut(place).unwrap();
    timeline.add_older(page(7..=9, true));
    assert!(timeline.more_before && !timeline.more_after);
    let mut newer = page(13..=14, false);
    // The page repeats a message that an event brought already.
    newer.messages.insert(0, loaded[2].clone());
    timeline.add_newer(newer);
    assert!(!timeline.more_after);
    assert_eq!(positions(&model, place), [7, 8, 9, 10, 11, 12, 13, 14]);
    assert_eq!(
        model
            .timeline(place)
            .unwrap()
            .messages
            .iter()
            .filter(|held| held.id == loaded[2].id)
            .count(),
        1
    );
}

#[test]
fn a_page_that_adds_no_message_does_not_promise_more_messages() {
    let scene = scene();
    let place = general(&scene);
    let (mut model, loaded) = opened(&scene, 10..=12);
    let timeline = model.timeline_mut(place).unwrap();
    let nothing_new = |has_more| MessagePage {
        messages: vec![loaded[0].clone()],
        has_more,
    };
    timeline.add_older(nothing_new(true));
    assert!(!timeline.more_before);
    timeline.add_newer(MessagePage {
        messages: Vec::new(),
        has_more: true,
    });
    assert!(!timeline.more_after);
    assert_eq!(positions(&model, place), [10, 11, 12]);
}

#[test]
fn a_timeline_holds_a_bounded_number_of_messages() {
    let scene = scene();
    let place = general(&scene);
    let other = AccountId::generate();
    let mut model = scene.model();
    model.open_timeline(place);
    let many = |from: u64, count: u64| MessagePage {
        messages: (from..from + count)
            .map(|position| message(scene.text_place(), position, other, "m"))
            .collect(),
        has_more: false,
    };
    let most = MOST_LOADED as u64;
    let timeline = model.timeline_mut(place).unwrap();
    timeline.replace(many(1_000, most).messages, false, false);
    assert_eq!(timeline.messages.len(), MOST_LOADED);

    // A page before the first message drops messages at the end.
    timeline.add_older(many(900, 100));
    assert_eq!(timeline.messages.len(), MOST_LOADED);
    assert_eq!(timeline.messages[0].position, 900);
    assert_eq!(timeline.messages[MOST_LOADED - 1].position, 900 + most - 1);
    assert!(timeline.more_after);

    // A page after the last message drops messages at the start.
    timeline.add_newer(many(900 + most, 50));
    assert_eq!(timeline.messages.len(), MOST_LOADED);
    assert_eq!(timeline.messages[0].position, 950);
    assert!(timeline.more_before);
}

#[test]
fn a_read_position_moves_forward_and_asks_for_the_counts_when_they_are_not_known() {
    let mut scene = scene();
    scene.snapshot.messages = vec![state(scene.text_place(), 3, 9, 6)];
    let place = general(&scene);
    let mut model = scene.model();
    let read = |position| Event::MessageRead {
        place: scene.text_place(),
        position,
    };
    assert_eq!(model.apply(&read(5)).load, [Part::MessageState(place)]);
    assert_eq!(model.message_state(place).unwrap().last_read, 5);
    // A lower position changes nothing.
    model.apply(&read(4));
    assert_eq!(model.message_state(place).unwrap().last_read, 5);
    // At the latest position nothing is unread, and nothing is asked for.
    assert_eq!(model.apply(&read(9)), Follow::default());
    let held = model.message_state(place).unwrap();
    assert_eq!((held.last_read, held.unread, held.mentions), (9, 0, 0));

    // A place without a summary gets one.
    let voice = MessagePlace::Channel {
        workspace_id: scene.workspace_id,
        channel_id: scene.daily,
    };
    assert_eq!(
        model
            .apply(&Event::MessageRead {
                place: voice,
                position: 2
            })
            .load,
        Vec::new()
    );
    assert_eq!(
        model
            .message_state(PlaceId::Channel(scene.daily))
            .unwrap()
            .last_read,
        2
    );

    let mention = Event::MessageMentioned {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
        message_id: MessageId::generate(),
        position: 10,
    };
    // The mention can be one of a thread: the unread threads are asked for too.
    assert_eq!(
        model.apply(&mention).load,
        [Part::MessageState(place), Part::UnreadThreads]
    );
}

#[test]
fn conversations_limits_and_deleted_channels_change_the_state() {
    let scene = scene();
    let place = general(&scene);
    let (mut model, _) = opened(&scene, 1..=2);
    model.set_message_state(state(scene.text_place(), 0, 2, 2));
    let other = AccountId::generate();
    let conversation = Conversation {
        id: ConversationId::generate(),
        low_account_id: scene.account_id.min(other),
        high_account_id: scene.account_id.max(other),
    };
    let opened = Event::ConversationOpened {
        conversation: conversation.clone(),
    };
    model.apply(&opened);
    model.apply(&opened);
    assert_eq!(model.conversations, std::slice::from_ref(&conversation));

    // An event of a conversation that the state does not hold adds it.
    let third = AccountId::generate();
    let unknown = MessagePlace::Direct {
        conversation_id: ConversationId::generate(),
        low_account_id: scene.account_id.min(third),
        high_account_id: scene.account_id.max(third),
    };
    let from_unknown = message(unknown, 1, third, "hello");
    assert_eq!(
        model.apply(&changed(&from_unknown, false)).load,
        [Part::MessageState(PlaceId::of(&unknown))]
    );
    assert!(model.conversation_with(third).is_some());
    // A conversation of two other accounts is not one of this account.
    let foreign = MessagePlace::Direct {
        conversation_id: ConversationId::generate(),
        low_account_id: other.min(third),
        high_account_id: other.max(third),
    };
    let from_foreign = message(foreign, 1, third, "x");
    assert_eq!(
        model.apply(&changed(&from_foreign, false)),
        Follow::default()
    );
    assert_eq!(model.conversations.len(), 2);

    let limits = AttachmentLimits {
        bytes_per_file: 5_000_000,
        files_per_message: 4,
    };
    model.apply(&Event::AttachmentLimitsChanged {
        limits: limits.clone(),
    });
    assert_eq!(model.attachment_limits, Some(limits));

    model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.general,
    });
    assert!(model.timeline(place).is_none());
    assert!(model.message_state(place).is_none());
}

fn waiting(scene: &Scene, id: u64, text: &str, state: SendState) -> PendingMessage {
    let mut pending =
        PendingMessage::new(id, general(scene), document(text), None, Vec::new(), time());
    pending.state = state;
    pending.after = 3;
    pending
}

#[test]
fn a_message_without_an_answer_is_confirmed_by_its_copy_and_by_nothing_else() {
    let scene = scene();
    let place = general(&scene);
    let (mut model, _) = opened(&scene, 1..=3);
    let own = scene.account_id;
    let other = AccountId::generate();
    model.outbox = vec![
        Arc::new(waiting(&scene, 1, "same", SendState::Unconfirmed)),
        Arc::new(waiting(&scene, 2, "same", SendState::Unconfirmed)),
        Arc::new(waiting(&scene, 3, "other", SendState::Unconfirmed)),
    ];
    // A message of another account, one before the attempt, and one with another text
    // confirm nothing.
    model.put_message(message(scene.text_place(), 4, other, "same"));
    let mut early = message(scene.text_place(), 3, own, "same");
    early.id = MessageId::generate();
    model.put_message(early);
    model.put_message(message(scene.text_place(), 5, own, "different"));
    assert!(!model.confirm_arrived(place));
    assert_eq!(model.outbox.len(), 3);

    // One copy confirms one of the two messages with the same text.
    model.put_message(message(scene.text_place(), 6, own, "same"));
    assert!(model.confirm_arrived(place));
    let left: Vec<u64> = model.outbox.iter().map(|pending| pending.id).collect();
    assert_eq!(left, [2, 3]);
    assert!(!model.confirm_arrived(place));
    model.put_message(message(scene.text_place(), 7, own, "same"));
    assert!(model.confirm_arrived(place));
    let left: Vec<u64> = model.outbox.iter().map(|pending| pending.id).collect();
    assert_eq!(left, [3]);

    // The answer to a send of this client confirms no other message.
    let answered = message(scene.text_place(), 8, own, "other");
    model.note_sent(answered.id);
    model.put_message(answered);
    assert!(!model.confirm_arrived(place));

    // A reply and a file belong to what is compared.
    let mut reply = waiting(&scene, 4, "reply", SendState::Unconfirmed);
    reply.reply_to = Some(MessageId::generate());
    let attachment_id = AttachmentId::generate();
    reply.files = vec![PendingFile {
        upload: 1,
        name: "a.txt".to_owned(),
        bytes: 1,
        attachment_id: Some(attachment_id),
    }];
    let mut copy = message(scene.text_place(), 9, own, "reply");
    assert!(!reply.is_confirmed_by(&copy, own));
    copy.reply_to = reply.reply_to;
    assert!(!reply.is_confirmed_by(&copy, own));
    copy.attachments = vec![Attachment {
        id: attachment_id,
        name: "a.txt".to_owned(),
        bytes: 1,
        sha256: String::new(),
        media_type: "application/octet-stream".to_owned(),
        width: None,
        height: None,
    }];
    assert!(reply.is_confirmed_by(&copy, own));
    // A message that failed is not confirmed: the server refused it.
    reply.state = SendState::Failed(SendFailure::NotSent);
    assert!(!reply.is_confirmed_by(&copy, own));
}

#[test]
fn a_message_on_its_way_is_not_drawn_beside_its_copy() {
    let scene = scene();
    let place = general(&scene);
    let (mut model, _) = opened(&scene, 1..=3);
    let own = scene.account_id;
    model.outbox = vec![
        Arc::new(waiting(&scene, 1, "same", SendState::Sending)),
        Arc::new(waiting(&scene, 2, "same", SendState::Sending)),
        Arc::new(waiting(
            &scene,
            3,
            "failed",
            SendState::Failed(SendFailure::NotSent),
        )),
    ];
    let shown = |model: &ServerModel| -> Vec<u64> {
        model
            .shown_waiting(place)
            .iter()
            .map(|pending| pending.id)
            .collect()
    };
    assert_eq!(shown(&model), [1, 2, 3]);
    // The event of the first message was followed before its answer arrived.
    model.put_message(message(scene.text_place(), 4, own, "same"));
    assert_eq!(shown(&model), [2, 3]);
    // The state still holds both: the answer decides.
    assert_eq!(model.outbox.len(), 3);
    assert!(
        model
            .waiting(PlaceId::Channel(scene.daily))
            .next()
            .is_none()
    );
}

#[test]
fn a_reload_keeps_the_open_timelines_and_the_waiting_messages_for_the_same_account() {
    let scene = scene();
    let place = general(&scene);
    let (mut before, _) = opened(&scene, 1..=3);
    before.open_timeline(PlaceId::Channel(scene.daily));
    before.outbox = vec![Arc::new(waiting(&scene, 1, "text", SendState::Sending))];
    before.attachment_limits = Some(AttachmentLimits {
        bytes_per_file: 1,
        files_per_message: 1,
    });
    before.note_sent(MessageId::generate());
    before.timeline_mut(place).unwrap().reading.older = true;

    let mut after = scene.model();
    after.carry_messages(&before, &[place]);
    let carried = after.timeline(place).unwrap();
    assert!(carried.stale && carried.loaded);
    assert_eq!(carried.reading, Reading::default());
    assert_eq!(carried.messages.len(), 3);
    // A timeline that no reader holds is not carried.
    assert!(after.timeline(PlaceId::Channel(scene.daily)).is_none());
    assert_eq!(after.outbox.len(), 1);
    assert_eq!(after.sent_ids.len(), 1);
    assert!(after.attachment_limits.is_some());

    // Another account starts without them.
    let mut other = scene.model();
    other.account.id = AccountId::generate();
    other.carry_messages(&before, &[place]);
    assert!(other.timelines.is_empty() && other.outbox.is_empty());
}

#[test]
fn events_during_a_read_name_the_reads_that_follow_its_answer() {
    let scene = scene();
    let place = general(&scene);
    let (mut model, loaded) = opened(&scene, 1..=3);
    let other = AccountId::generate();
    let new = message(scene.text_place(), 4, other, "new");
    let voice = MessagePlace::Channel {
        workspace_id: scene.workspace_id,
        channel_id: scene.daily,
    };
    let elsewhere = message(voice, 1, other, "elsewhere");
    let kept = [
        changed(&loaded[0], false),
        changed(&new, false),
        changed(&loaded[0], false),
        changed(&elsewhere, false),
        Event::MessageRead {
            place: scene.text_place(),
            position: 1,
        },
    ];
    // After a page: each message that an event named, once.
    assert_eq!(
        model.replay_reads(Part::Messages(place, Reach::Older), &kept),
        [
            Part::Message(place, loaded[0].id),
            Part::Message(place, new.id)
        ]
    );
    // After the unread summary: the summary again.
    model.set_message_state(state(scene.text_place(), 0, 4, 4));
    assert_eq!(
        model.replay_reads(Part::MessageState(place), &kept),
        [Part::MessageState(place)]
    );
    // After one message: that message again, when an event named it.
    assert_eq!(
        model.replay_reads(Part::Message(place, new.id), &kept),
        [Part::Message(place, new.id)]
    );
    assert!(
        model
            .replay_reads(Part::Message(place, MessageId::generate()), &kept)
            .is_empty()
    );
    assert!(Part::AttachmentLimits.is_read_again());
    assert!(!Part::OwnCall.is_read_again());
}
