//! The derivation of what ADR 0780 adds to the server book: the unread sign of the head of
//! a server, the order of the rows, and the newer build that the window knows of.

use junto_client_core::protocol::ids::{AccountId, ConversationId};
use junto_client_core::protocol::message::{Conversation, MessagePlace, MessageState};
use junto_client_core::updates::{Release, UpdateState, Version};

use super::tests::{logged_in, scene, server};
use super::*;
use crate::state::book::{Available, UpdatesKnown};

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

fn direct(own: AccountId) -> (Conversation, MessagePlace) {
    let other = AccountId::generate();
    let conversation = Conversation {
        id: ConversationId::generate(),
        low_account_id: own.min(other),
        high_account_id: own.max(other),
    };
    let place = MessagePlace::Direct {
        conversation_id: conversation.id,
        low_account_id: conversation.low_account_id,
        high_account_id: conversation.high_account_id,
    };
    (conversation, place)
}

fn row(model: ServerModel, own: AccountId) -> ServerRow {
    let server = server("Acme Studio", Some(model), logged_in(own, Vec::new()));
    derive(
        std::slice::from_ref(&server),
        &Selection::default(),
        &Known::default(),
    )
    .servers
    .remove(0)
}

#[test]
fn the_head_of_a_server_shows_what_no_workspace_row_shows() {
    let scene = scene();
    let general = MessagePlace::Channel {
        workspace_id: scene.studio,
        channel_id: scene.general,
    };

    // Nothing is unread: no sign.
    let quiet = row(scene.model(), scene.own);
    assert!(!quiet.head_unread.any() && !quiet.unread.any());

    // An unread channel is shown by the row of its workspace, and not by the head.
    let mut model = scene.model();
    model.message_states = vec![summary(general, 3, 1)];
    let channel_only = row(model, scene.own);
    assert!(channel_only.unread.any());
    assert!(!channel_only.head_unread.any());
    assert_eq!(channel_only.head_unread, Unread::default());
    let studio = channel_only
        .workspaces
        .iter()
        .find(|workspace| workspace.id == scene.studio)
        .unwrap();
    assert_eq!((studio.unread.messages, studio.unread.mentions), (3, 1));

    // An unread direct conversation is shown by no workspace row: the head shows it.
    let (conversation, place) = direct(scene.own);
    let mut model = scene.model();
    model.conversations = vec![conversation];
    model.message_states = vec![summary(general, 3, 0), summary(place, 2, 0)];
    let with_direct = row(model, scene.own);
    assert!(with_direct.head_unread.any());
    assert_eq!(
        (
            with_direct.head_unread.messages,
            with_direct.head_unread.mentions
        ),
        (2, 0)
    );
    // The sign of the whole server counts both.
    assert_eq!(with_direct.unread.messages, 5);
}

#[test]
fn a_mention_in_a_direct_conversation_outranks_plain_unread_on_the_head() {
    let scene = scene();
    let (first, first_place) = direct(scene.own);
    let (second, second_place) = direct(scene.own);
    let mut model = scene.model();
    model.conversations = vec![first, second];
    model.message_states = vec![summary(first_place, 4, 0), summary(second_place, 2, 2)];
    let row = row(model, scene.own);
    assert_eq!((row.head_unread.messages, row.head_unread.mentions), (6, 2));
    assert!(row.head_unread.any());

    // A muted conversation shows no sign on the head.
    let scene = self::scene();
    let (conversation, place) = direct(scene.own);
    let mut muted = summary(place, 2, 1);
    muted.unread_sign = false;
    muted.notification_mentions = 0;
    muted.muted = true;
    let mut model = scene.model();
    model.conversations = vec![conversation];
    model.message_states = vec![muted];
    assert!(!self::row(model, scene.own).head_unread.any());
}

#[test]
fn a_server_without_a_listed_workspace_shows_every_unread_place_on_its_head() {
    let mut scene = scene();
    let general = MessagePlace::Channel {
        workspace_id: scene.studio,
        channel_id: scene.general,
    };
    scene.snapshot.workspaces.clear();
    scene.snapshot.account.workspace_ids.clear();
    let mut model = scene.model();
    model.message_states = vec![summary(general, 2, 1)];
    let row = row(model, scene.own);
    assert!(row.workspaces.is_empty());
    assert_eq!((row.head_unread.messages, row.head_unread.mentions), (2, 1));
    assert_eq!(row.head_unread, row.unread);
}

#[test]
fn a_server_that_shows_no_lists_has_no_sign_on_its_head() {
    let scene = scene();
    let (conversation, place) = direct(scene.own);
    let mut model = scene.model();
    model.conversations = vec![conversation];
    model.message_states = vec![summary(place, 2, 0)];
    // The state belongs to another account than the one of the session: it is not shown.
    let other = server(
        "Acme Studio",
        Some(model),
        logged_in(AccountId::generate(), Vec::new()),
    );
    let view = derive(
        std::slice::from_ref(&other),
        &Selection::default(),
        &Known::default(),
    );
    assert_eq!(view.servers[0].head_unread, Unread::default());
}

#[test]
fn the_rows_follow_the_order_of_the_servers() {
    let scene = scene();
    let named = |name: &str| server(name, Some(scene.model()), logged_in(scene.own, Vec::new()));
    let (acme, harbor, studio) = (named("Acme"), named("Harbor"), named("Studio"));
    let names = |servers: &[ServerSnapshot]| -> Vec<String> {
        derive(servers, &Selection::default(), &Known::default())
            .servers
            .into_iter()
            .map(|row| row.name)
            .collect()
    };
    assert_eq!(
        names(&[acme.clone(), harbor.clone(), studio.clone()]),
        ["Acme", "Harbor", "Studio"]
    );
    // The client core gives the servers in the order of the book after a move.
    let moved = [harbor.clone(), acme.clone(), studio.clone()];
    assert_eq!(names(&moved), ["Harbor", "Acme", "Studio"]);
    // The selection is the server and not its place.
    let selection = Selection {
        server: Some(acme.id),
        ..Selection::default()
    };
    let view = derive(&moved, &selection, &Known::default());
    let selected: Vec<bool> = view.servers.iter().map(|row| row.selected).collect();
    assert_eq!(selected, [false, true, false]);
    assert_eq!(view.selected.unwrap().id, acme.id);
}

#[test]
fn the_view_names_a_newer_build_while_the_check_says_available() {
    let scene = scene();
    let server = server(
        "Acme Studio",
        Some(scene.model()),
        logged_in(scene.own, Vec::new()),
    );
    let newer = Version::new(9, 0, 0);
    let known = |updates: UpdatesKnown| Known {
        updates,
        ..Known::default()
    };
    let view = |updates: UpdatesKnown| {
        derive(
            std::slice::from_ref(&server),
            &Selection::default(),
            &known(updates),
        )
        .update
    };
    assert_eq!(view(UpdatesKnown::default()), None);
    let available = UpdatesKnown {
        has_source: true,
        state: UpdateState::Available(Release::new(newer, None)),
        ..UpdatesKnown::default()
    };
    assert_eq!(view(available.clone()), Some(Available { version: newer }));
    // The notice does not depend on a server: the view names the build also while the
    // server book is empty.
    let empty = derive(&[], &Selection::default(), &known(available));
    assert!(empty.servers.is_empty() && empty.selected.is_none());
    assert_eq!(empty.update, Some(Available { version: newer }));
    for state in [UpdateState::Current, UpdateState::Failed] {
        assert_eq!(
            view(UpdatesKnown {
                has_source: true,
                state,
                ..UpdatesKnown::default()
            }),
            None
        );
    }
}

#[test]
fn the_selected_server_states_its_version_and_a_newer_release_of_the_source() {
    let scene = scene();
    let mut server = server(
        "Acme Studio",
        Some(scene.model()),
        logged_in(scene.own, Vec::new()),
    );
    let release = |server: &ServerSnapshot, updates: UpdatesKnown| {
        let known = Known {
            updates,
            ..Known::default()
        };
        derive(std::slice::from_ref(server), &Selection::default(), &known)
            .selected
            .and_then(|selected| selected.release)
    };
    assert_eq!(release(&server, UpdatesKnown::default()), None);

    let stated = ServerVersion::parse("0.3.0").unwrap();
    server.server_version = Some(stated.clone());
    let shown = |newer: Option<Version>| {
        Some(ServerRelease {
            version: stated.clone(),
            newer,
            newer_events: false,
        })
    };
    // Without a source only the version is known.
    assert_eq!(release(&server, UpdatesKnown::default()), shown(None));
    // A server of a newer revision of the events is stated (ADR 0206).
    server.server_events = Some(junto_client_core::protocol::events::EVENT_REVISION + 1);
    assert!(release(&server, UpdatesKnown::default()).is_some_and(|release| release.newer_events));
    server.server_events = Some(junto_client_core::protocol::events::EVENT_REVISION);
    let newest = |version: Version| UpdatesKnown {
        has_source: true,
        state: UpdateState::Current,
        newest: Some(version),
        ..UpdatesKnown::default()
    };
    let same = Version::new(0, 3, 0);
    assert_eq!(release(&server, newest(same)), shown(None));
    let newer = Version::new(0, 3, 1);
    assert_eq!(release(&server, newest(newer)), shown(Some(newer)));
}

#[test]
fn unread_summary_saturates_hostile_counts() {
    let scene = scene();
    let place = |channel_id| MessagePlace::Channel {
        workspace_id: scene.studio,
        channel_id,
    };
    // Two channels of one workspace: one unread message, and the most that the type holds.
    for (first, second) in [(scene.general, scene.random), (scene.random, scene.general)] {
        let mut model = scene.model();
        model.message_states = vec![
            summary(place(first), 1, 1),
            summary(place(second), u64::MAX, u64::MAX),
        ];
        let row = row(model, scene.own);
        assert_eq!((row.unread.messages, row.unread.mentions), (100, 100));
        let studio = row
            .workspaces
            .iter()
            .find(|workspace| workspace.id == scene.studio)
            .unwrap();
        assert_eq!((studio.unread.messages, studio.unread.mentions), (100, 100));
    }
}

#[test]
fn unread_derivation_visits_each_summary_once() {
    // A server that is not selected, with many direct conversations and no workspace.
    let scene = scene();
    let read = |count: usize| {
        let mut model = scene.model();
        model.workspaces.clear();
        for _ in 0..count {
            let (conversation, place) = direct(scene.own);
            model.conversations.push(conversation);
            model.message_states.push(summary(place, 1, 0));
        }
        let shown = server(
            "Shown",
            Some(scene.model()),
            logged_in(scene.own, Vec::new()),
        );
        let other = server("Other", Some(model), logged_in(scene.own, Vec::new()));
        let selection = Selection {
            server: Some(shown.id),
            ..Selection::default()
        };
        super::SUMMARIES_READ.with(|read| read.set(0));
        let view = derive(&[shown, other], &selection, &Known::default());
        assert_eq!(view.servers[1].head_unread.messages, 100.min(count as u64));
        assert_eq!(view.servers[1].unread.messages, 100.min(count as u64));
        super::SUMMARIES_READ.with(std::cell::Cell::get)
    };
    // One derivation makes one index of the summaries of each server that shows its
    // lists, and of the shown workspace: their number does not grow with the summaries.
    let (once, twice) = (read(500), read(1_000));
    assert!((1..=3).contains(&once), "{once}");
    assert_eq!(twice, once);
}
