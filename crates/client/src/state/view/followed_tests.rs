//! The derivation of followed threads (ADR 0412, ADR 0413, ADR 0796): the unread threads
//! under the shown channel, and the control of the thread panel.

use std::sync::Arc;

use junto_client_core::model::{Collection, Named, Reading, Timeline};
use junto_client_core::protocol::message::{FollowedThreads, Message, MessagePlace, ThreadState};
use junto_client_core::protocol::structure::ArchivedChannel;

use super::panel_tests::message;
use super::tests::{Scene, logged_in, scene, server, time};
use super::*;
use crate::state::followed::RootAnswer;
use crate::text;

fn place_of(scene: &Scene, channel_id: ChannelId) -> MessagePlace {
    MessagePlace::Channel {
        workspace_id: scene.studio,
        channel_id,
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
        anchor: None,
    }
}

fn state(root_id: MessageId, unread: u64) -> ThreadState {
    ThreadState {
        read_revision: 0,
        root_id,
        last_read: 4,
        latest: 4 + unread,
        unread,
        mentions: 0,
        following: true,
        follow_setting: None,
    }
}

fn states(count: usize) -> Vec<ThreadState> {
    (0..count)
        .map(|_| state(MessageId::generate(), 0))
        .collect()
}

fn acme(scene: &Scene, model: ServerModel) -> ServerSnapshot {
    server("Acme Studio", Some(model), logged_in(scene.own, Vec::new()))
}

/// What the window holds after the server gave one page of the followed threads of a
/// channel. `more`: the server lists further pages.
fn followed(channel: (ServerId, ChannelId), threads: Vec<ThreadState>, more: bool) -> Followed {
    let mut followed = Followed::default();
    followed.list.show(Some(channel));
    let read = followed.list.next_read().unwrap();
    let page = FollowedThreads {
        threads,
        more,
        next: more.then(|| "next".to_owned()),
    };
    assert!(followed.list.answered(read.number, Some(page)));
    followed
}

/// The root of a thread was read for its title.
fn hold(followed: &mut Followed, server_id: ServerId, root: &Message) {
    let reads = followed.roots.next_reads(server_id, &[root.id]);
    let answer = RootAnswer::Read(Arc::new(root.clone()));
    assert!(followed.roots.answered(&reads[0], answer));
}

/// The follow state of a thread was read.
fn told(followed: &mut Followed, thread: (ServerId, MessageId), state: (bool, Option<bool>)) {
    followed.thread.show(Some(thread));
    let ask = followed.thread.next_read().unwrap();
    assert!(followed.thread.read_answered(ask.number, Some(state)));
}

fn known(followed: Followed) -> Known {
    Known {
        followed,
        ..Known::default()
    }
}

fn row(workspace: &WorkspaceView, channel_id: ChannelId) -> &ChannelRow {
    workspace
        .sections
        .iter()
        .flat_map(|section| section.rows.iter())
        .find(|row| row.id == channel_id)
        .unwrap()
}

/// The threads under a channel: the root, the title and the count of the unread replies.
fn entries(
    workspace: &WorkspaceView,
    channel_id: ChannelId,
) -> Vec<(MessageId, Option<&str>, u64)> {
    row(workspace, channel_id)
        .threads
        .iter()
        .map(|entry| (entry.root_id, entry.title.as_deref(), entry.unread))
        .collect()
}

/// The roots of the threads under a channel.
fn listed(
    server: &ServerSnapshot,
    selection: &Selection,
    known: &Known,
    channel_id: ChannelId,
) -> Vec<MessageId> {
    let view = derive(std::slice::from_ref(server), selection, known);
    let row = row(view.workspace().unwrap(), channel_id);
    row.threads.iter().map(|entry| entry.root_id).collect()
}

/// Followed threads that each have one unread reply.
fn unread_states(count: usize) -> Vec<ThreadState> {
    (0..count)
        .map(|_| state(MessageId::generate(), 1))
        .collect()
}

/// A server whose state holds only the first pages of the unread threads, so that the list
/// of followed threads tells what else is unread.
fn acme_with_more_unread(scene: &Scene) -> ServerSnapshot {
    let mut model = scene.model();
    model.threads_more = true;
    acme(scene, model)
}

/// The control of the thread panel for a thread of "general".
fn control(
    server: &ServerSnapshot,
    scene: &Scene,
    root_id: MessageId,
    known: &Known,
) -> Option<FollowControl> {
    let selection = Selection {
        thread: Some((server.id, scene.general, root_id)),
        ..Selection::default()
    };
    let view = derive(std::slice::from_ref(server), &selection, known);
    let panel = view.workspace().unwrap().thread.as_ref().unwrap();
    assert_eq!(panel.root_id, root_id);
    panel.follow.clone()
}

#[test]
fn a_channel_lists_its_unread_threads_and_no_read_followed_thread() {
    let scene = scene();
    let ana = AccountId::generate();
    let general = place_of(&scene, scene.general);
    let loaded = message(general, 2, ana, "Tracks or mixdown?");
    let unread = message(general, 3, ana, "Retention of recordings");
    let read_alone = message(general, 4, ana, "Notes of the review");
    let unknown = MessageId::generate();
    let elsewhere = message(place_of(&scene, scene.random), 5, ana, "Lunch on Friday");
    let mut model = scene.model();
    model.timelines.push(Arc::new(timeline(
        PlaceId::Channel(scene.general),
        vec![loaded.clone()],
    )));
    model.threads = vec![state(elsewhere.id, 1), state(unread.id, 2)];
    model.named = vec![
        (unread.id, Named::Read(Arc::new(unread.clone()))),
        (elsewhere.id, Named::Read(Arc::new(elsewhere.clone()))),
    ];
    let server = acme(&scene, model);
    // The server states the thread with the newest reply first. One of the followed
    // threads holds something unread.
    let mut followed = followed(
        (server.id, scene.general),
        vec![
            state(read_alone.id, 0),
            state(unread.id, 2),
            state(loaded.id, 0),
            state(unknown, 0),
        ],
        false,
    );
    hold(&mut followed, server.id, &read_alone);

    let view = derive(
        std::slice::from_ref(&server),
        &Selection::default(),
        &known(followed),
    );
    let workspace = view.workspace().unwrap();
    // The unread thread is listed once; the read followed threads have no row under the
    // channel, the side panel "Threads" lists them (ADR 0796).
    assert_eq!(
        entries(workspace, scene.general),
        [(unread.id, Some("Retention of recordings"), 2)]
    );
    assert!(
        row(workspace, scene.general)
            .threads
            .iter()
            .all(|entry| entry.available && !entry.open)
    );
    // Another channel lists its unread thread, and no followed one.
    assert_eq!(
        entries(workspace, scene.random),
        [(elsewhere.id, Some("Lunch on Friday"), 1)]
    );
    // The channel of the main area lists the same threads as its row.
    assert_eq!(
        workspace.channel.as_ref().unwrap().threads,
        row(workspace, scene.general).threads
    );
}

#[test]
fn a_followed_thread_beyond_the_loaded_unread_threads_keeps_the_count_of_its_list() {
    let scene = scene();
    let (first, second) = (MessageId::generate(), MessageId::generate());
    let threads = vec![state(first, 3), state(second, 0)];
    let unread = |more: bool| {
        let mut model = scene.model();
        model.threads_more = more;
        let server = acme(&scene, model);
        let known = known(followed((server.id, scene.general), threads.clone(), false));
        let view = derive(std::slice::from_ref(&server), &Selection::default(), &known);
        entries(view.workspace().unwrap(), scene.general)
            .into_iter()
            .map(|(root_id, _, unread)| (root_id, unread))
            .collect::<Vec<_>>()
    };
    // The state of the server holds every unread thread: this one is read meanwhile, and
    // a read thread has no row.
    assert_eq!(unread(false), []);
    // The state holds the first pages of the unread threads only: the list tells that the
    // first one is unread.
    assert_eq!(unread(true), [(first, 3)]);
}

#[test]
fn read_followed_threads_have_no_row_under_their_channel() {
    let scene = scene();
    let server = acme(&scene, scene.model());
    let held = known(followed((server.id, scene.general), states(11), true));
    assert!(listed(&server, &Selection::default(), &held, scene.general).is_empty());
}

#[test]
fn the_followed_threads_are_listed_under_the_shown_channel_only() {
    let scene = scene();
    let server = acme_with_more_unread(&scene);
    let threads = unread_states(9);
    let roots: Vec<MessageId> = threads.iter().map(|state| state.root_id).collect();
    let of_general = known(followed((server.id, scene.general), threads.clone(), false));
    let shows = |channel_id: ChannelId| Selection {
        channels: [((server.id, scene.studio), channel_id)]
            .into_iter()
            .collect(),
        ..Selection::default()
    };
    let general = shows(scene.general);
    assert_eq!(listed(&server, &general, &of_general, scene.general), roots);
    assert_eq!(
        listed(&server, &general, &of_general, scene.random),
        Vec::<MessageId>::new()
    );

    // Another channel is shown while the window still holds the list of the first one:
    // the list is drawn under no channel.
    let random = shows(scene.random);
    for channel_id in [scene.general, scene.random, scene.daily] {
        assert_eq!(
            listed(&server, &random, &of_general, channel_id),
            Vec::<MessageId>::new()
        );
    }
    // The list of the channel that is shown now is drawn under it.
    let of_random = known(followed((server.id, scene.random), threads.clone(), false));
    assert_eq!(listed(&server, &random, &of_random, scene.random), roots);
    // A list that was read on another server is not drawn.
    let other_server = known(followed(
        (ServerId::generate(), scene.general),
        threads,
        false,
    ));
    assert_eq!(
        listed(&server, &general, &other_server, scene.general),
        Vec::<MessageId>::new()
    );
}

#[test]
fn the_open_thread_is_listed_once_among_the_followed_threads() {
    let scene = scene();
    let server = acme_with_more_unread(&scene);
    let threads = unread_states(3);
    let roots: Vec<MessageId> = threads.iter().map(|state| state.root_id).collect();
    let open = |held: &Known, root_id: MessageId| {
        let selection = Selection {
            thread: Some((server.id, scene.general, root_id)),
            ..Selection::default()
        };
        let view = derive(std::slice::from_ref(&server), &selection, held);
        row(view.workspace().unwrap(), scene.general)
            .threads
            .iter()
            .map(|entry| (entry.root_id, entry.open))
            .collect::<Vec<_>>()
    };

    // The open thread is one of the unread followed ones: its row is the open one.
    let held = known(followed((server.id, scene.general), threads, false));
    assert_eq!(
        open(&held, roots[1]),
        [(roots[0], false), (roots[1], true), (roots[2], false)]
    );
    // A thread that has no row of its own is listed after them.
    let other = MessageId::generate();
    assert_eq!(open(&held, other).last(), Some(&(other, true)));
    // A read followed thread that is open has its one row.
    let read = states(1);
    let read_root = read[0].root_id;
    let held = known(followed((server.id, scene.general), read, false));
    assert_eq!(open(&held, read_root), [(read_root, true)]);
}

#[test]
fn the_control_of_the_thread_panel_states_the_choice_of_the_account_or_the_rule() {
    let scene = scene();
    let server = acme(&scene, scene.model());
    let root_id = MessageId::generate();
    let read = |state: (bool, Option<bool>)| {
        let mut followed = Followed::default();
        told(&mut followed, (server.id, root_id), state);
        known(followed)
    };

    // Until the server has told the state, the panel has no control.
    assert_eq!(control(&server, &scene, root_id, &Known::default()), None);
    let mut reading = Followed::default();
    reading.thread.show(Some((server.id, root_id)));
    assert!(reading.thread.next_read().is_some());
    assert_eq!(control(&server, &scene, root_id, &known(reading)), None);

    for (following, setting, sentence, action) in [
        (
            true,
            Some(true),
            "You follow this thread.",
            "Stop following",
        ),
        (
            false,
            Some(false),
            "You do not follow this thread. Its replies are not counted as unread; a mention \
             still is.",
            "Follow",
        ),
        (
            true,
            None,
            "You follow this thread because you wrote in it or were named in it.",
            "Stop following",
        ),
        (false, None, "You do not follow this thread.", "Follow"),
    ] {
        let drawn = control(&server, &scene, root_id, &read((following, setting))).unwrap();
        assert_eq!(
            drawn,
            FollowControl {
                following,
                setting,
                busy: false,
                read_only: false,
                refused: None,
            }
        );
        assert_eq!(
            text::followed::state(drawn.following, drawn.setting),
            sentence
        );
        assert_eq!(text::followed::action(drawn.following), action);
    }

    // The state of one thread is not shown in the panel of another thread.
    let other = MessageId::generate();
    assert_eq!(control(&server, &scene, other, &read((true, None))), None);

    // A change on its way, and its refusal.
    let mut held = read((false, None));
    let ask = held.followed.thread.ask().unwrap();
    assert!(control(&server, &scene, root_id, &held).unwrap().busy);
    let refusal = text::followed::THREAD_GONE.to_owned();
    assert!(held.followed.thread.ask_answered(
        ask.number,
        Err(refusal),
        junto_client_core::model::Followed::Unknown
    ));
    let drawn = control(&server, &scene, root_id, &held).unwrap();
    assert!(!drawn.busy && !drawn.following);
    assert_eq!(
        drawn.refused.as_deref(),
        Some("This thread no longer exists.")
    );
}

#[test]
fn an_archived_channel_lists_its_followed_threads_and_takes_no_change() {
    let scene = scene();
    let (general, studio) = (scene.general, scene.studio);
    let (root_id, other) = (MessageId::generate(), MessageId::generate());
    let mut model = scene.model();
    let channel = model.channel(general).unwrap().channel.clone();
    Arc::make_mut(&mut model.workspaces[1])
        .channels
        .retain(|held| held.channel.id != general);
    Arc::make_mut(&mut model.admin).archived = vec![(
        studio,
        Collection::Loaded(vec![ArchivedChannel {
            channel,
            archived_at: time(),
            deletes_at: None,
        }]),
    )];
    let server = acme(&scene, model);
    let mut followed = followed(
        (server.id, general),
        vec![state(root_id, 0), state(other, 0)],
        false,
    );
    told(&mut followed, (server.id, root_id), (true, None));
    let held = known(followed);
    let selection = Selection {
        channels: [((server.id, studio), general)].into_iter().collect(),
        thread: Some((server.id, general, root_id)),
        ..Selection::default()
    };

    let view = derive(std::slice::from_ref(&server), &selection, &held);
    let workspace = view.workspace().unwrap();
    assert!(workspace.archived.is_some());
    // The control states what holds, and says that the channel takes no change.
    let drawn = workspace.thread.as_ref().unwrap().follow.clone().unwrap();
    assert!(drawn.read_only && drawn.following && !drawn.busy);
    // The archived channel is read as its messages are: its open thread is listed, and its
    // read followed thread has no row (ADR 0796).
    let shown = workspace.channel.as_ref().unwrap();
    let entries: Vec<(MessageId, bool)> = shown
        .threads
        .iter()
        .map(|entry| (entry.root_id, entry.open))
        .collect();
    assert_eq!(entries, [(root_id, true)]);
    assert!(!entries.iter().any(|(id, _)| *id == other));

    // The same thread in a channel that is not archived takes a change.
    let open = acme(&scene, scene.model());
    let mut followed = Followed::default();
    told(&mut followed, (open.id, root_id), (true, None));
    let drawn = control(&open, &scene, root_id, &known(followed)).unwrap();
    assert!(!drawn.read_only);
}
