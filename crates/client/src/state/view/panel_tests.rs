//! The derivation of what ADR 0741 adds to the window: direct conversations, the thread
//! and the pin list of the side panel, the threads under a channel, and the unread signs
//! that follow the notification settings.

use std::sync::Arc;

use junto_client_core::model::{Loadable, Named, PinList, ReadFailure, Reading, Timeline};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{AccountId, ConversationId, MessageId};
use junto_client_core::protocol::message::{
    Conversation, Message, MessagePlace, MessageState, ThreadState,
};
use junto_client_core::protocol::roles::WorkspaceRole;
use junto_client_core::protocol::structure::{ChannelKind, WorkspaceMember};

use super::tests::{Scene, logged_in, scene, server, time};
use super::*;

pub(super) fn message(
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
        document: Some(crate::compose::parse(text, &[])),
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

fn timeline(place: PlaceId, messages: Vec<Message>) -> Timeline {
    Timeline {
        place,
        messages: messages.into_iter().map(Arc::new).collect(),
        more_before: false,
        more_after: false,
        reading: Reading::default(),
        loaded: true,
        failed: None,
        stale: false,
        unread_after: None,
        root: None,
        forks: Vec::new(),
    }
}

fn summary(place: MessagePlace, unread: u64, mentions: u64) -> MessageState {
    MessageState {
        place,
        last_read: 1,
        latest: 9,
        unread,
        mentions,
        unread_sign: unread > 0,
        notification_mentions: mentions,
        muted: false,
    }
}

impl Scene {
    fn general_place(&self) -> MessagePlace {
        MessagePlace::Channel {
            workspace_id: self.studio,
            channel_id: self.general,
        }
    }
}

fn view_of(
    model: ServerModel,
    own: AccountId,
    selection: &Selection,
    known: &Known,
) -> (ServerId, ShellView) {
    let server = server("Acme Studio", Some(model), logged_in(own, Vec::new()));
    (
        server.id,
        derive(std::slice::from_ref(&server), selection, known),
    )
}

#[test]
fn the_sign_of_a_place_follows_the_notification_settings() {
    let scene = scene();
    let mut model = scene.model();
    let random = MessagePlace::Channel {
        workspace_id: scene.studio,
        channel_id: scene.random,
    };
    let daily = MessagePlace::Channel {
        workspace_id: scene.studio,
        channel_id: scene.daily,
    };
    // Unread messages at the level "mentions only": counted, and no sign.
    let mut quiet = summary(scene.general_place(), 4, 0);
    quiet.unread_sign = false;
    // A muted channel with a mention: nothing is shown, and the row says "muted".
    let mut muted = summary(random, 2, 1);
    muted.unread_sign = false;
    muted.notification_mentions = 0;
    muted.muted = true;
    // A mention that the settings let through.
    let loud = summary(daily, 3, 2);
    model.message_states = vec![quiet, muted, loud];

    let (_, view) = view_of(model, scene.own, &Selection::default(), &Known::default());
    let workspace = view.workspace().unwrap();
    let row = |name: &str| {
        workspace
            .sections
            .iter()
            .flat_map(|section| section.rows.iter())
            .find(|row| row.name == name)
            .unwrap()
            .unread
    };
    assert!(!row("general").any() && row("general").unread());
    assert_eq!(row("general").messages, 4);
    assert!(!row("random").any() && row("random").muted);
    assert_eq!(row("random").mentions, 0);
    assert!(row("Daily").any());
    // The workspace and the server count what shows a sign.
    let studio = view.servers[0]
        .workspaces
        .iter()
        .find(|workspace| workspace.id == scene.studio)
        .unwrap();
    assert_eq!((studio.unread.messages, studio.unread.mentions), (3, 2));
    assert_eq!(view.servers[0].unread.mentions, 2);
    assert!(view.servers[0].unread.any());
}

#[test]
fn a_direct_conversation_is_shown_in_place_of_a_channel() {
    let mut scene = scene();
    let (ana, zoe) = (AccountId::generate(), AccountId::generate());
    scene.studio_mut().roles = Vec::new();
    let mut model = scene.model();
    let conversation = |other: AccountId| Conversation {
        id: ConversationId::generate(),
        low_account_id: scene.own.min(other),
        high_account_id: scene.own.max(other),
    };
    let (with_ana, with_zoe) = (conversation(ana), conversation(zoe));
    model.conversations = vec![with_zoe.clone(), with_ana.clone()];
    let direct = MessagePlace::Direct {
        conversation_id: with_ana.id,
        low_account_id: with_ana.low_account_id,
        high_account_id: with_ana.high_account_id,
    };
    model.message_states = vec![summary(direct, 2, 0)];
    model.timelines.push(Arc::new(timeline(
        PlaceId::Direct(with_ana.id),
        vec![message(direct, 1, ana, "hello")],
    )));
    let server = server("Acme Studio", Some(model), logged_in(scene.own, Vec::new()));
    // The name of a person who is no member of the shown workspace was read by itself.
    let mut known = Known::default();
    known.people.read_name(server.id, ana, "Ana");

    // Without a selection the channel is shown, and the conversations are listed: the
    // one with a known name first.
    let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
    let workspace = view.workspace().unwrap();
    assert!(workspace.conversation.is_none() && workspace.channel.is_some());
    let names: Vec<Option<&str>> = workspace
        .directs
        .iter()
        .map(|row| row.name.as_deref())
        .collect();
    assert_eq!(names, [Some("Ana"), None]);
    assert_eq!(workspace.directs[0].account_id, ana);
    assert!(workspace.directs[0].unread.sign);
    assert_eq!(view.shown().place(), Some(PlaceId::Channel(scene.general)));

    // The selected conversation takes the place of the channel, with its messages.
    let mut selection = Selection::default();
    selection.conversations.insert(server.id, with_ana.id);
    let view = derive(std::slice::from_ref(&server), &selection, &known);
    let workspace = view.workspace().unwrap();
    assert!(workspace.channel.is_none());
    assert_eq!(workspace.conversation.as_ref().unwrap().id, with_ana.id);
    assert!(workspace.directs[0].selected);
    assert_eq!(view.shown().place(), Some(PlaceId::Direct(with_ana.id)));
    let feed = workspace.feed.as_ref().unwrap();
    assert_eq!(feed.place, PlaceId::Direct(with_ana.id));
    // A message of a direct conversation has no thread and no pin.
    let feed::FeedRow::Message(row) = feed
        .rows
        .iter()
        .find(|row| matches!(row, feed::FeedRow::Message(_)))
        .unwrap()
    else {
        unreachable!()
    };
    assert!(!row.can_thread && !row.can_pin);

    // A conversation that the state does not hold any more is not shown.
    let mut selection = Selection::default();
    selection
        .conversations
        .insert(server.id, ConversationId::generate());
    let view = derive(std::slice::from_ref(&server), &selection, &known);
    assert!(view.workspace().unwrap().conversation.is_none());
    assert!(view.workspace().unwrap().channel.is_some());
}

#[test]
fn the_thread_of_the_side_panel_belongs_to_the_shown_channel() {
    let mut scene = scene();
    let ana = AccountId::generate();
    let manage = WorkspaceRole {
        id: junto_client_core::protocol::ids::WorkspaceRoleId::generate(),
        workspace_id: scene.studio,
        name: "everyone".to_owned(),
        permissions: vec![WorkspacePermission::ManageChannel],
        is_everyone: true,
    };
    scene.studio_mut().roles = vec![manage];
    let general = scene.general;
    for channel in &mut scene.studio_mut().channels {
        channel.permanent_messages = channel.id == general;
    }
    let mut model = scene.model();
    let place = scene.general_place();
    let mut root = message(place, 2, ana, "Tracks or mixdown?");
    let mut first = message(place, 3, ana, "One per speaker.");
    first.thread_root = Some(root.id);
    root.reply_count = 1;
    root.last_reply = Some(first.id);
    let other_root = message(place, 4, ana, "Another thread");
    let thread = PlaceId::Thread {
        channel_id: scene.general,
        root_id: root.id,
    };
    model.timelines.push(Arc::new(timeline(
        PlaceId::Channel(scene.general),
        vec![root.clone(), other_root.clone()],
    )));
    let mut replies = timeline(thread, vec![first.clone()]);
    replies.root = Some(Arc::new(root.clone()));
    replies.forks = vec![
        scene.random,
        junto_client_core::protocol::ids::ChannelId::generate(),
    ];
    model.timelines.push(Arc::new(replies));
    // The other thread holds something unread, and its root was read.
    model.threads = vec![ThreadState {
        root_id: other_root.id,
        last_read: 4,
        latest: 6,
        unread: 2,
        mentions: 1,
    }];
    model.named = vec![(other_root.id, Named::Read(Arc::new(other_root.clone())))];
    model.workspaces[1] = {
        let mut workspace = model.workspaces[1].as_ref().clone();
        workspace.members = Loadable::Loaded(vec![WorkspaceMember {
            account_id: ana,
            name: "Ana".to_owned(),
            role_ids: Vec::new(),
        }]);
        Arc::new(workspace)
    };
    let server = server("Acme Studio", Some(model), logged_in(scene.own, Vec::new()));
    let mut selection = Selection {
        thread: Some((server.id, scene.general, root.id)),
        ..Selection::default()
    };

    let view = derive(std::slice::from_ref(&server), &selection, &Known::default());
    let workspace = view.workspace().unwrap();
    let panel = workspace.thread.as_ref().unwrap();
    assert_eq!((panel.channel_id, panel.root_id), (scene.general, root.id));
    assert_eq!(panel.channel_name, "general");
    assert_eq!(panel.root.as_ref().unwrap().message.id, root.id);
    assert!(panel.may_fork && panel.permanent);
    // A fork channel that the state does not hold is not named.
    assert_eq!(
        panel.forks,
        [(scene.random, "random".to_owned(), ChannelKind::Text)]
    );
    assert_eq!(panel.feed.as_ref().unwrap().place, thread);
    assert_eq!(view.shown().thread_place(), Some(thread));

    // The row of the root in the channel counts its replies and names the last one,
    // which the open thread holds.
    let feed = workspace.feed.as_ref().unwrap();
    let marks: Vec<_> = feed
        .rows
        .iter()
        .filter_map(|row| match row {
            feed::FeedRow::Message(row) => Some(row.thread.clone()),
            _ => None,
        })
        .collect();
    let mark = marks[0].as_ref().unwrap();
    assert_eq!((mark.replies, mark.open, mark.unread), (1, true, 0));
    assert_eq!(mark.last.as_ref().unwrap().author, "Ana");
    assert!(marks[1].is_none());

    // The channel column lists the unread thread and the open thread under the channel.
    let general = workspace
        .sections
        .iter()
        .flat_map(|section| section.rows.iter())
        .find(|row| row.id == scene.general)
        .unwrap();
    let entries: Vec<_> = general
        .threads
        .iter()
        .map(|entry| {
            (
                entry.root_id,
                entry.title.as_deref(),
                entry.mentions,
                entry.open,
            )
        })
        .collect();
    assert_eq!(
        entries,
        [
            (other_root.id, Some("Another thread"), 1, false),
            (root.id, Some("Tracks or mixdown?"), 0, true)
        ]
    );

    // Another channel is shown: the thread is not, and the selection of the pin list of
    // the first channel is not either.
    selection.channels.insert(scene.studio, scene.random);
    selection.pins = Some((server.id, scene.general));
    let view = derive(std::slice::from_ref(&server), &selection, &Known::default());
    let workspace = view.workspace().unwrap();
    assert!(workspace.thread.is_none() && workspace.pins.is_none());
    assert_eq!(view.shown().thread, None);
}

#[test]
fn the_pin_list_of_the_side_panel_shows_its_state() {
    let scene = scene();
    let ana = AccountId::generate();
    let place = scene.general_place();
    let mut pinned = message(place, 2, ana, "The checklist is up.");
    pinned.pinned = true;
    let mut model = scene.model();
    model.timelines.push(Arc::new(timeline(
        PlaceId::Channel(scene.general),
        vec![pinned.clone()],
    )));
    let base = model.clone();
    let server_with =
        |model: ServerModel| server("Acme Studio", Some(model), logged_in(scene.own, Vec::new()));
    let state_of = |model: ServerModel| {
        let server = server_with(model);
        let selection = Selection {
            pins: Some((server.id, scene.general)),
            ..Selection::default()
        };
        let view = derive(std::slice::from_ref(&server), &selection, &Known::default());
        view.workspace()
            .unwrap()
            .pins
            .clone()
            .map(|pins| pins.state)
    };

    // The client core does not hold the list yet.
    assert_eq!(state_of(base.clone()), Some(PinsState::Loading));
    let list = |messages, failed| PinList {
        channel_id: scene.general,
        messages,
        failed,
    };
    let mut model = base.clone();
    model.pins = vec![list(Loadable::Loading, None)];
    assert_eq!(state_of(model), Some(PinsState::Loading));
    let mut model = base.clone();
    model.pins = vec![list(Loadable::NotLoaded, Some(ReadFailure::NoAnswer))];
    assert_eq!(state_of(model), Some(PinsState::Failed));
    let mut model = base;
    model.pins = vec![list(Loadable::Loaded(vec![Arc::new(pinned.clone())]), None)];
    let Some(PinsState::Loaded(rows)) = state_of(model) else {
        panic!("the list is not loaded");
    };
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].message.id, pinned.id);
    // The roles of the scene hold no "manage messages".
    assert!(!rows[0].can_pin);
}
