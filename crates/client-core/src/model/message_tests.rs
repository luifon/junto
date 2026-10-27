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
        reply_authors: Vec::new(),
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
fn a_change_of_permissions_withdraws_what_its_scope_holds() {
    let mut scene = scene();
    let direct = MessagePlace::Direct {
        conversation_id: ConversationId::generate(),
        low_account_id: scene.account_id,
        high_account_id: AccountId::generate(),
    };
    scene.snapshot.messages = vec![state(scene.text_place(), 1, 3, 2), state(direct, 1, 2, 1)];
    let place = general(&scene);
    let (mut model, loaded) = opened(&scene, 1..=3);
    model.open_pins(scene.general);
    model.set_pins(scene.general, vec![loaded[0].clone()]);
    Arc::make_mut(&mut model.workspaces[0]).members = Loadable::Loaded(Vec::new());
    let admin = Arc::make_mut(&mut model.admin);
    admin.accounts = Collection::Loaded(Vec::new());
    admin.overrides = vec![(scene.general, Collection::Loaded(Vec::new()))];
    admin.archived = vec![(scene.workspace_id, Collection::Loaded(Vec::new()))];

    // A change in the workspace: what it holds is withdrawn until the next state.
    let follow = model.apply(&Event::PermissionsChanged {
        workspace_id: Some(scene.workspace_id),
    });
    assert!(follow.reload && follow.withdrawn);
    let timeline = model.timeline(place).unwrap();
    assert!(timeline.messages.is_empty() && !timeline.loaded && timeline.stale);
    assert!(model.message_state(place).is_none());
    assert!(model.message_state(PlaceId::of(&direct)).is_some());
    assert_eq!(
        model.pin_list(scene.general).unwrap().messages,
        Loadable::Loading
    );
    assert_eq!(model.workspaces[0].members, Loadable::Loading);
    assert_eq!(*model.admin.overrides(scene.general), Collection::Loading);
    assert_eq!(
        *model.admin.archived(scene.workspace_id),
        Collection::Loading
    );
    // The structure stays until the snapshot replaces it, and so do the collections of
    // the server.
    assert!(model.channel(scene.general).is_some());
    assert_eq!(model.admin.accounts, Collection::Loaded(Vec::new()));
    // A page of messages is not taken by the withdrawn timeline: only the page that the
    // next state reads fills it.
    assert!(!model.put_message(message(scene.text_place(), 4, AccountId::generate(), "x")));

    // A change on the server withdraws the collections of the server as well.
    let follow = model.apply(&Event::PermissionsChanged { workspace_id: None });
    assert!(follow.withdrawn);
    assert_eq!(model.admin.accounts, Collection::Loading);
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
    // A read position cannot pass the latest message: a position above the latest one of
    // the summary names a message that the summary does not hold yet.
    assert_eq!(model.apply(&read(11)), Follow::default());
    let held = model.message_state(place).unwrap();
    assert_eq!((held.last_read, held.latest, held.unread), (11, 11, 0));

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
            .map(|held| (held.last_read, held.latest)),
        Some((2, 2))
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
        frames: None,
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

/// A message whose document is one block of code with `bytes` bytes.
pub(crate) fn heavy(place: MessagePlace, position: u64, bytes: usize) -> Message {
    let mut heavy = message(place, position, AccountId::generate(), "");
    heavy.document = Some(Document {
        blocks: vec![Block::Code {
            text: "x".repeat(bytes),
            language: None,
        }],
    });
    heavy
}

fn weight_of(messages: &[std::sync::Arc<Message>]) -> usize {
    messages.iter().map(|held| weight(held)).sum()
}

#[test]
fn a_message_inside_the_bounds_of_the_server_fits_and_one_above_them_does_not() {
    use junto_protocol::message::Reaction;

    let scene = scene();
    let place = scene.text_place();
    let plain = message(place, 1, AccountId::generate(), "text");
    assert!(fits(&plain));

    // The most that a server of ADR 0408 stores in one message: a document of 16 KiB with
    // 100 blocks and 1,000 inline nodes, 100 files with the longest name, 20 reactions.
    let mut most = plain.clone();
    let node = |text: &str| Inline::Text {
        text: text.to_owned(),
        marks: Vec::new(),
    };
    most.document = Some(Document {
        blocks: (0..MOST_BLOCKS)
            .map(|_| Block::Paragraph {
                content: (0..MOST_INLINE_NODES / MOST_BLOCKS)
                    .map(|_| node("sixteen bytes..."))
                    .collect(),
            })
            .collect(),
    });
    most.attachments = (0..MOST_FILES)
        .map(|_| Attachment {
            id: AttachmentId::generate(),
            name: "é".repeat(200),
            bytes: 1,
            sha256: "a".repeat(64),
            media_type: "application/octet-stream".to_owned(),
            width: None,
            height: None,
            frames: None,
        })
        .collect();
    most.reactions = (0..MOST_REACTIONS)
        .map(|_| Reaction {
            emoji: "\u{1f44d}".to_owned(),
            count: 1,
            own: false,
        })
        .collect();
    assert!(fits(&most));
    assert!(weight(&most) < MOST_MESSAGE_WEIGHT / 2);

    // A list of empty items is the document with the most structure in 16 KiB.
    let mut items = plain.clone();
    items.document = Some(Document {
        blocks: vec![Block::List {
            ordered: false,
            items: vec![Vec::new(); 5_400],
        }],
    });
    assert!(fits(&items));
    assert!(weight(&items) < MOST_MESSAGE_WEIGHT / 2);

    let mut more_items = items.clone();
    more_items.document = Some(Document {
        blocks: vec![Block::List {
            ordered: true,
            items: vec![Vec::new(); MOST_LIST_ITEMS + 1],
        }],
    });
    assert!(!fits(&more_items));

    // One part more than a server stores, in each kind.
    let mut blocks = most.clone();
    if let Some(document) = &mut blocks.document {
        document.blocks.push(Block::Paragraph {
            content: Vec::new(),
        });
    }
    assert!(!fits(&blocks));
    let mut nodes = most.clone();
    if let Some(Block::Paragraph { content }) = nodes
        .document
        .as_mut()
        .and_then(|document| document.blocks.first_mut())
    {
        content.push(node(""));
    }
    assert!(!fits(&nodes));
    let mut files = most.clone();
    files.attachments.push(most.attachments[0].clone());
    assert!(!fits(&files));
    let mut reactions = most.clone();
    reactions.reactions.push(most.reactions[0].clone());
    assert!(!fits(&reactions));

    // A text that no server of ADR 0408 stores, in a structure that it would store.
    assert!(fits(&heavy(place, 1, MOST_MESSAGE_WEIGHT - 4096)));
    assert!(!fits(&heavy(place, 1, MOST_MESSAGE_WEIGHT)));
    let mut name = plain.clone();
    name.author_name = Some("n".repeat(MOST_MESSAGE_WEIGHT));
    assert!(!fits(&name));
    let mut list = plain;
    list.document = Some(Document {
        blocks: vec![Block::Checklist {
            items: vec![
                junto_protocol::message::CheckItem {
                    checked: false,
                    content: Vec::new(),
                };
                MOST_MESSAGE_WEIGHT / 16
            ],
        }],
    });
    assert!(!fits(&list));
}

#[test]
fn a_timeline_holds_a_bounded_weight_of_messages() {
    let scene = scene();
    let place = general(&scene);
    let mut model = scene.model();
    model.open_timeline(place);
    // Each message is close to the most that one message weighs.
    let each = MOST_MESSAGE_WEIGHT - 4096;
    let many = |from: u64, count: u64| -> Vec<Message> {
        (from..from + count)
            .map(|position| heavy(scene.text_place(), position, each))
            .collect()
    };
    let fit = (MOST_LOADED_WEIGHT / weight(&many(1, 1)[0])) as u64;

    // The page that opens the place holds more than the timeline may weigh: the start of
    // the page gives way, and the history has more there.
    let timeline = model.timeline_mut(place).unwrap();
    timeline.replace(many(1_000, fit + 10), false, false);
    assert_eq!(timeline.messages.len() as u64, fit);
    assert!(weight_of(&timeline.messages) <= MOST_LOADED_WEIGHT);
    assert_eq!(timeline.messages[0].position, 1_010);
    assert!(timeline.more_before && !timeline.more_after);

    // A page before the first message drops messages at the end.
    timeline.add_older(MessagePage {
        messages: many(990, 5),
        has_more: true,
    });
    assert!(weight_of(&timeline.messages) <= MOST_LOADED_WEIGHT);
    assert_eq!(timeline.messages[0].position, 990);
    assert_eq!(timeline.messages.len() as u64, fit);
    assert!(timeline.more_after);

    // A page after the last message drops messages at the start.
    let last = timeline.messages.last().unwrap().position;
    timeline.add_newer(MessagePage {
        messages: many(last + 1, 3),
        has_more: false,
    });
    assert!(weight_of(&timeline.messages) <= MOST_LOADED_WEIGHT);
    assert_eq!(timeline.messages.last().unwrap().position, last + 3);
    assert_eq!(timeline.messages[0].position, 993);

    // Single messages that events name: a new one at the end, and a held one whose new
    // copy weighs more than the one before.
    let timeline = model.timeline_mut(place).unwrap();
    timeline.replace(
        (1..=100)
            .map(|position| message(scene.text_place(), position, AccountId::generate(), "m"))
            .collect(),
        false,
        false,
    );
    assert_eq!(timeline.messages.len(), 100);
    for position in 101..=100 + fit + 5 {
        assert!(model.put_message(heavy(scene.text_place(), position, each)));
        let timeline = model.timeline(place).unwrap();
        assert!(weight_of(&timeline.messages) <= MOST_LOADED_WEIGHT);
        assert_eq!(timeline.messages.last().unwrap().position, position);
    }
    let timeline = model.timeline(place).unwrap();
    assert_eq!(timeline.messages.len() as u64, fit);
    assert!(timeline.more_before);
    let small = message(scene.text_place(), 5_000, AccountId::generate(), "m");
    assert!(model.put_message(small.clone()));
    let mut grown = heavy(scene.text_place(), 5_000, each);
    grown.id = small.id;
    assert!(model.put_message(grown));
    let timeline = model.timeline(place).unwrap();
    assert!(weight_of(&timeline.messages) <= MOST_LOADED_WEIGHT);
    assert_eq!(timeline.messages.last().unwrap().id, small.id);
    assert_eq!(timeline.messages.len() as u64, fit);

    // The timeline holds the end of the history. A held message near its start gets a
    // heavier copy, such as a reaction: the start gives way, the end stays, and the new
    // messages of the place still arrive.
    let tiny = message(scene.text_place(), 2, AccountId::generate(), "m");
    let base = weight(&heavy(scene.text_place(), 1, 0));
    let heavy_weight = base + each;
    // The first message fills the timeline up to 2 KiB below what it may weigh.
    let filler = MOST_LOADED_WEIGHT - (fit as usize - 1) * heavy_weight - weight(&tiny) - 2048;
    let timeline = model.timeline_mut(place).unwrap();
    timeline.replace(
        [heavy(scene.text_place(), 1, filler - base), tiny]
            .into_iter()
            .chain((3..=fit + 1).map(|position| heavy(scene.text_place(), position, each)))
            .collect(),
        false,
        false,
    );
    assert_eq!(timeline.messages.len() as u64, fit + 1);
    assert!(!timeline.more_after && !timeline.more_before);
    let second = timeline.messages[1].as_ref().clone();
    let mut grown = heavy(scene.text_place(), second.position, each);
    grown.id = second.id;
    assert!(model.put_message(grown));
    let timeline = model.timeline(place).unwrap();
    assert!(weight_of(&timeline.messages) <= MOST_LOADED_WEIGHT);
    assert!(!timeline.more_after, "the end of the history stays");
    assert!(timeline.more_before);
    assert_eq!(timeline.messages.last().unwrap().position, fit + 1);
    assert_eq!(timeline.messages[0].id, second.id);
    let next = message(scene.text_place(), fit + 2, AccountId::generate(), "new");
    assert!(model.put_message(next.clone()));
    assert!(model.timeline(place).unwrap().message(next.id).is_some());
}

#[test]
fn a_timeline_keeps_the_messages_that_the_reader_shows_when_it_passes_a_bound() {
    let scene = scene();
    let place = general(&scene);
    let mut model = scene.model();
    model.open_timeline(place);
    let each = MOST_MESSAGE_WEIGHT - 4096;
    let fit = (MOST_LOADED_WEIGHT / weight(&heavy(scene.text_place(), 1, each))) as u64;
    let many = |positions: std::ops::Range<u64>| -> Vec<Message> {
        positions
            .map(|position| heavy(scene.text_place(), position, each))
            .collect()
    };

    // The timeline holds the end of the history, and the reader shows its first
    // messages. New heavy messages arrive at the end: the messages that the reader shows
    // stay, the end gives way, and the history has more there.
    let timeline = model.timeline_mut(place).unwrap();
    timeline.replace(many(1..fit + 1), false, false);
    assert!(model.anchor_timeline(place, Some(1)));
    assert!(!model.anchor_timeline(place, Some(1)));
    for position in fit + 1..=fit + 5 {
        model.put_message(heavy(scene.text_place(), position, each));
        let timeline = model.timeline(place).unwrap();
        assert!(weight_of(&timeline.messages) <= MOST_LOADED_WEIGHT);
        assert_eq!(timeline.messages[0].position, 1, "the shown start stays");
        assert!(timeline.more_after && !timeline.more_before);
    }
    assert_eq!(positions(&model, place).last(), Some(&fit));

    // The reader shows a position in the middle. A page after the last message and a
    // page before the first one each make the messages give way at the end that holds
    // more of them on its side of that position.
    let middle = 1_000 + fit / 2;
    let timeline = model.timeline_mut(place).unwrap();
    timeline.replace(many(1_000..1_000 + fit), true, true);
    model.anchor_timeline(place, Some(middle));
    let timeline = model.timeline_mut(place).unwrap();
    timeline.add_newer(MessagePage {
        messages: many(1_000 + fit..1_000 + fit + 5),
        has_more: true,
    });
    let first = timeline.messages[0].position;
    timeline.add_older(MessagePage {
        messages: many(first - 6..first),
        has_more: true,
    });
    assert!(weight_of(&timeline.messages) <= MOST_LOADED_WEIGHT);
    assert!(timeline.more_before && timeline.more_after);
    let held = positions(&model, place);
    assert!(held.contains(&middle));
    let before = held.iter().filter(|position| **position < middle).count();
    let after = held.len() - before;
    assert!(
        before.abs_diff(after) <= 1,
        "{before} before, {after} after"
    );

    // The bound of the count: the reader shows the first of the most messages that a
    // timeline holds, and a new message arrives at the end.
    let most = MOST_LOADED as u64;
    let small = |position| message(scene.text_place(), position, AccountId::generate(), "m");
    let timeline = model.timeline_mut(place).unwrap();
    timeline.replace((1..=most).map(small).collect(), false, false);
    model.anchor_timeline(place, Some(1));
    model.put_message(small(most + 1));
    let timeline = model.timeline(place).unwrap();
    assert_eq!(timeline.messages.len(), MOST_LOADED);
    assert_eq!(timeline.messages[0].position, 1);
    assert!(timeline.more_after && !timeline.more_before);

    // The reader follows the end again: the start gives way, as before.
    model.anchor_timeline(place, None);
    let timeline = model.timeline_mut(place).unwrap();
    timeline.add_newer(MessagePage {
        messages: vec![small(most + 1)],
        has_more: false,
    });
    assert_eq!(timeline.messages[0].position, 2);
    assert!(!timeline.more_after && timeline.more_before);
}

#[test]
fn a_pin_list_and_the_named_messages_hold_a_bounded_weight() {
    use junto_protocol::message::{ThreadState, UnreadThreads};

    let scene = scene();
    let mut model = scene.model();
    let each = MOST_MESSAGE_WEIGHT - 4096;
    let pinned = |position: u64| {
        let mut pin = heavy(scene.text_place(), position, each);
        pin.pinned = true;
        pin
    };
    let fit = MOST_PINNED_WEIGHT / weight(&pinned(1));

    // The answer of the pin route holds more than the list may weigh: the oldest pins
    // give way.
    model.open_pins(scene.general);
    let pins: Vec<Message> = (1..=fit as u64 + 4).map(pinned).collect();
    model.set_pins(scene.general, pins);
    let held = |model: &ServerModel| {
        model
            .pin_list(scene.general)
            .and_then(|list| list.messages.loaded())
            .cloned()
            .unwrap()
    };
    assert_eq!(held(&model).len(), fit);
    assert_eq!(held(&model)[0].position, 5);
    // A message that an event names is pinned: it is added, and the oldest pin leaves.
    model.put_message(pinned(1_000));
    assert_eq!(held(&model).len(), fit);
    assert_eq!(held(&model)[0].position, 6);
    assert_eq!(held(&model).last().unwrap().position, 1_000);
    assert!(weight_of(&held(&model)) <= MOST_PINNED_WEIGHT);

    // The roots of the unread threads are named messages. The ones that pass the weight
    // are not held, and their entries say that the read failed.
    let roots: Vec<Message> = (1..=MOST_NAMED_WEIGHT as u64 / weight(&pinned(1)) as u64 + 3)
        .map(|position| heavy(scene.text_place(), 2_000 + position, each))
        .collect();
    model.set_threads(UnreadThreads {
        threads: roots
            .iter()
            .map(|root| ThreadState {
                root_id: root.id,
                last_read: 0,
                latest: 1,
                unread: 1,
                mentions: 0,
                following: true,
                follow_setting: None,
            })
            .collect(),
        more: false,
        next: None,
    });
    for root in &roots {
        model.put_message(root.clone());
    }
    let read = roots
        .iter()
        .filter(|root| matches!(model.named(root.id), Some(Named::Read(_))))
        .count();
    assert_eq!(read, roots.len() - 3);
    assert_eq!(model.named(roots.last().unwrap().id), Some(&Named::Failed));
}
