//! The derivation of followed threads (ADR 0412, ADR 0413): the threads that the account
//! follows under the shown channel, the row that offers the further ones, and the control
//! of the thread panel.

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

/// The roots of the threads under a channel, and the row of the further ones.
fn listed(
    server: &ServerSnapshot,
    selection: &Selection,
    known: &Known,
    channel_id: ChannelId,
) -> (Vec<MessageId>, Option<MoreFollowed>) {
    let view = derive(std::slice::from_ref(server), selection, known);
    let row = row(view.workspace().unwrap(), channel_id);
    (
        row.threads.iter().map(|entry| entry.root_id).collect(),
        row.more_followed,
    )
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
fn the_followed_threads_of_the_shown_channel_are_listed_after_its_unread_threads() {
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
    // The unread thread is listed once, before the others. A title comes from the state
    // of the server, from the loaded messages of the channel, or from the root that was
    // read; a root that nothing holds has none.
    assert_eq!(
        entries(workspace, scene.general),
        [
            (unread.id, Some("Retention of recordings"), 2),
            (read_alone.id, Some("Notes of the review"), 0),
            (loaded.id, Some("Tracks or mixdown?"), 0),
            (unknown, None, 0),
        ]
    );
    assert_eq!(row(workspace, scene.general).more_followed, None);
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
    // The state of the server holds every unread thread: this one is read meanwhile.
    assert_eq!(unread(false), [(first, 0), (second, 0)]);
    // The state holds the first pages of the unread threads only.
    assert_eq!(unread(true), [(first, 3), (second, 0)]);
}

#[test]
fn eight_followed_threads_are_listed_and_a_row_offers_the_others() {
    let scene = scene();
    let server = acme(&scene, scene.model());
    let threads = states(11);
    let roots: Vec<MessageId> = threads.iter().map(|state| state.root_id).collect();
    let selection = Selection::default();
    let shown = |known: &Known| listed(&server, &selection, known, scene.general);

    let mut held = known(followed((server.id, scene.general), threads.clone(), false));
    assert_eq!(
        shown(&held),
        (roots[..8].to_vec(), Some(MoreFollowed { count: Some(3) }))
    );
    // The person asks for the others: every held thread is listed, and no row is left.
    assert!(held.followed.list.expand());
    assert_eq!(shown(&held), (roots.clone(), None));

    // The server lists further pages: the row states no count, and it stays after the
    // held threads are all listed.
    let mut held = known(followed((server.id, scene.general), threads, true));
    assert_eq!(
        shown(&held),
        (roots[..8].to_vec(), Some(MoreFollowed { count: None }))
    );
    assert!(held.followed.list.expand());
    assert_eq!(shown(&held), (roots, Some(MoreFollowed { count: None })));
}

#[test]
fn the_followed_threads_are_listed_under_the_shown_channel_only() {
    let scene = scene();
    let server = acme(&scene, scene.model());
    let threads = states(9);
    let roots: Vec<MessageId> = threads.iter().map(|state| state.root_id).collect();
    let of_general = known(followed((server.id, scene.general), threads.clone(), false));
    let shows = |channel_id: ChannelId| Selection {
        channels: [((server.id, scene.studio), channel_id)]
            .into_iter()
            .collect(),
        ..Selection::default()
    };
    let general = shows(scene.general);
    assert_eq!(
        listed(&server, &general, &of_general, scene.general).0,
        roots[..8]
    );
    assert_eq!(
        listed(&server, &general, &of_general, scene.random),
        (Vec::new(), None)
    );

    // Another channel is shown while the window still holds the list of the first one:
    // the list is drawn under no channel.
    let random = shows(scene.random);
    for channel_id in [scene.general, scene.random, scene.daily] {
        assert_eq!(
            listed(&server, &random, &of_general, channel_id),
            (Vec::new(), None)
        );
    }
    // The list of the channel that is shown now is drawn under it.
    let of_random = known(followed((server.id, scene.random), threads.clone(), false));
    assert_eq!(
        listed(&server, &random, &of_random, scene.random).0,
        roots[..8]
    );
    // A list that was read on another server is not drawn.
    let other_server = known(followed(
        (ServerId::generate(), scene.general),
        threads,
        false,
    ));
    assert_eq!(
        listed(&server, &general, &other_server, scene.general),
        (Vec::new(), None)
    );
}

#[test]
fn the_open_thread_is_listed_once_among_the_followed_threads() {
    let scene = scene();
    let server = acme(&scene, scene.model());
    let threads = states(11);
    let roots: Vec<MessageId> = threads.iter().map(|state| state.root_id).collect();
    let mut held = known(followed((server.id, scene.general), threads, false));
    let open = |held: &Known, root_id: MessageId| {
        let selection = Selection {
            thread: Some((server.id, scene.general, root_id)),
            ..Selection::default()
        };
        let view = derive(std::slice::from_ref(&server), &selection, held);
        let row = row(view.workspace().unwrap(), scene.general);
        let entries: Vec<(MessageId, bool)> = row
            .threads
            .iter()
            .map(|entry| (entry.root_id, entry.open))
            .collect();
        (entries, row.more_followed)
    };

    // The open thread is one of the listed ones: its row is the open one.
    let (entries, more) = open(&held, roots[1]);
    assert_eq!(entries.len(), 8);
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(*entry, (roots[index], index == 1));
    }
    assert_eq!(more, Some(MoreFollowed { count: Some(3) }));

    // The open thread is one of the further ones: it has a row of its own after the
    // listed ones, and the row of the others does not count it.
    let (entries, more) = open(&held, roots[9]);
    assert_eq!(entries.len(), 9);
    assert_eq!(entries[8], (roots[9], true));
    assert!(entries[..8].iter().all(|(_, open)| !open));
    assert_eq!(more, Some(MoreFollowed { count: Some(2) }));

    // After the person asked for the others it is listed at its place.
    assert!(held.followed.list.expand());
    let (entries, more) = open(&held, roots[9]);
    assert_eq!(entries.len(), 11);
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(*entry, (roots[index], index == 9));
    }
    assert_eq!(more, None);

    // A thread that the account does not follow is listed after the followed ones.
    let other = MessageId::generate();
    let (entries, _) = open(&held, other);
    assert_eq!(entries.len(), 12);
    assert_eq!(entries[11], (other, true));
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
    // The archived channel is read as its messages are: its followed threads are listed.
    let shown = workspace.channel.as_ref().unwrap();
    let entries: Vec<(MessageId, bool)> = shown
        .threads
        .iter()
        .map(|entry| (entry.root_id, entry.open))
        .collect();
    assert_eq!(entries, [(root_id, true), (other, false)]);
    assert_eq!(shown.more_followed, None);

    // The same thread in a channel that is not archived takes a change.
    let open = acme(&scene, scene.model());
    let mut followed = Followed::default();
    told(&mut followed, (open.id, root_id), (true, None));
    let drawn = control(&open, &scene, root_id, &known(followed)).unwrap();
    assert!(!drawn.read_only);
}
