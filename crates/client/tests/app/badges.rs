//! The counts of what waits for the person (ADR 0741, ADR 0761): each place draws its
//! number, read from what the window paints, and the number is correct after a read, a
//! removal and a restart.

use gpui::TestAppContext;
use junto_client::shell::Open;
use junto_client::state::view::ServerSnapshot;
use junto_client_core::model::{PlaceId, ServerModel};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::call::{
    Call, CallParticipant, CallPlace, ChannelCall, EntryRequest, Meeting,
};
use junto_client_core::protocol::ids::{AccountId, CallId, EntryRequestId};
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::protocol::snapshot::Snapshot;
use junto_client_core::protocol::structure::ArchivePeriod;
use junto_media::calls::{ActiveCall, CallsView};

use crate::admin_pages::owner_window;
use crate::calls::{connected_call, voice_channel};
use crate::harness::{Window, World, member, open, selector};
use crate::messages::document;
use crate::notices::mention;
use crate::threads::with_root;

impl Window {
    /// The texts that the window paints inside the element with this name.
    fn drawn_texts(&mut self, selector: &'static str) -> Vec<String> {
        let _record = junto_client::ui::record_painted_text();
        self.cx.update(|window, _| window.refresh());
        self.settle();
        let text = junto_client::ui::take_painted_text();
        let Some(bounds) = self.cx.debug_bounds(selector) else {
            return Vec::new();
        };
        text.into_iter()
            .filter(|text| bounds.contains(&text.bounds.center()))
            .map(|text| text.text.to_string())
            .collect()
    }

    /// The number that a badge or a corner count draws inside the element with this name.
    pub fn drawn_number(&mut self, selector: &'static str) -> Option<String> {
        self.drawn_texts(selector)
            .into_iter()
            .find(|text| is_count(text))
    }

    /// The number of the mention sign, drawn as "@ N", inside the element with this name.
    pub fn drawn_mentions(&mut self, selector: &'static str) -> Option<String> {
        self.drawn_texts(selector).into_iter().find_map(|text| {
            text.strip_prefix("@ ")
                .filter(|count| is_count(count))
                .map(str::to_owned)
        })
    }

    /// The count of the icon of the application and of the title of the main window.
    pub fn badge(&mut self) -> Option<u64> {
        self.in_shell(|shell, _, _| shell.badge())
    }

    fn make_active(&mut self) {
        self.cx.update(|window, _| window.activate_window());
        self.settle();
    }
}

/// A count as a badge draws it: digits, or "99+".
fn is_count(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|letter| letter.is_ascii_digit() || letter == '+')
}

/// The calls of the window state this call of this device, as a session states it.
fn give_calls(window: &mut Window, call: ActiveCall) {
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    calls.show_view_for_test(CallsView {
        call: Some(call),
        ..CallsView::default()
    });
    window.settle();
}

/// What the server states of the call of this device: `raised` other people with a raised
/// hand inside, the requests to enter of `asking` people outside, meeting mode run by the
/// person, and "mute members" for the person.
#[derive(Debug, Clone, Copy, Default)]
struct Stated {
    raised: usize,
    asking: usize,
    presenter: bool,
    mute: bool,
}

/// Draws the server as `base` states it with the call of this device as `stated` states
/// it, in place of what the server told. The server of the window has no media server, so
/// no person can enter the call or raise a hand there. Returns the people with a raised
/// hand.
fn state_call(
    window: &mut Window,
    base: &Snapshot,
    call: &ActiveCall,
    call_id: CallId,
    stated: Stated,
) -> Vec<AccountId> {
    let CallPlace::Channel {
        workspace_id,
        channel_id,
    } = call.target.place
    else {
        unreachable!("a call of a voice channel")
    };
    let own = window.own_account();
    let now = time::OffsetDateTime::now_utc();
    let participant = |account_id: AccountId, hand_raised: bool| CallParticipant {
        avatar: None,
        account_id,
        identity: format!("seat-{account_id}"),
        joined_at: now,
        muted: true,
        deafened: false,
        microphone_block: None,
        sound_block: None,
        hand_raised,
        watching: None,
        screen_share: None,
        screen_shares: Vec::new(),
    };
    let raised: Vec<AccountId> = (0..stated.raised).map(|_| AccountId::generate()).collect();
    let mut snapshot = base.clone();
    let workspace = (snapshot.workspaces.iter_mut())
        .find(|state| state.workspace.id == workspace_id)
        .unwrap();
    if stated.mute {
        workspace.permissions.push(WorkspacePermission::MuteMembers);
        for channel in &mut workspace.channel_permissions {
            channel.permissions.push(WorkspacePermission::MuteMembers);
        }
    }
    workspace.calls.retain(|held| held.channel_id != channel_id);
    workspace.calls.push(ChannelCall {
        workspace_id,
        channel_id,
        in_use: true,
        call: Some(Call {
            id: call_id,
            place: call.target.place,
            started_at: Some(now),
            lock: None,
            meeting: stated.presenter.then_some(Meeting {
                presenter: own,
                started_at: now,
            }),
            hides_occupants: false,
            participants: std::iter::once(participant(own, false))
                .chain(raised.iter().map(|account| participant(*account, true)))
                .collect(),
            recording: None,
        }),
    });
    let mut model = ServerModel::from_snapshot(snapshot);
    model.entry_requests = (0..stated.asking)
        .map(|_| EntryRequest {
            id: EntryRequestId::generate(),
            call_id,
            place: call.target.place,
            account_id: AccountId::generate(),
            created_at: now,
            expires_at: now + time::Duration::minutes(10),
        })
        .collect();
    let handle = window.handle();
    let fixture = ServerSnapshot {
        model: Some(std::sync::Arc::new(model)),
        ..ServerSnapshot::of(&handle)
    };
    let state = window.state.clone();
    window.cx.update(|_, cx| {
        state.update(cx, |state, cx| state.show_fixture(vec![fixture], cx));
    });
    window.settle();
    raised
}

/// Direct messages and mentions are counted on the row of the conversation, on the heading
/// of the direct messages, on the server, on the icon of the application and in the title;
/// a read takes them away, and a restart counts what is still unread.
#[gpui::test]
fn the_messages_for_the_person_are_counted_everywhere_and_a_read_takes_them_away(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    let own = window.own_account();
    let api = admin.handle.api();
    let conversation = world.run(api.open_conversation(own)).unwrap();
    for text in ["one", "two"] {
        world
            .run(api.send_message(
                PlaceId::Direct(conversation.id),
                &SendMessage {
                    document: document(text),
                    reply_to: None,
                    attachments: Vec::new(),
                },
            ))
            .unwrap();
    }
    let row = crate::harness::selector(format!("direct-row:{}", conversation.id));
    window.until("the conversation counts two messages", |window| {
        window.drawn_number(row).as_deref() == Some("2")
    });
    assert_eq!(window.drawn_number("directs-heading").as_deref(), Some("2"));
    assert_eq!(window.badge(), Some(2));
    // A mention in a channel that the window does not look at counts too.
    let random = admin.text_channel(&world, &mut window, "random");
    window.click_channel("random");
    window.until("the other channel is shown", |window| {
        window.selected_channel() == random
    });
    admin.send(&world, general, mention("look, ", own));
    window.until("the mention counts on the icon", |window| {
        window.badge() == Some(3)
    });
    assert_eq!(junto_client::app_badge::title(3), "(3) Junto");
    // The tray takes the same count for its mark.
    assert_eq!(
        window
            .state
            .read_with(&window.cx, |state, _| state.for_person()),
        3
    );

    // A restart counts what is still unread.
    window.close();
    let mut window = open(cx, directory.path());
    window.until("the counts after a restart", |window| {
        window.badge() == Some(3) && window.drawn_number(row).as_deref() == Some("2")
    });

    // The person reads the conversation and the channel: every count goes. The window
    // shows the other channel first, so that the mention stays unread until its channel
    // is shown.
    window.click_channel("random");
    window.until("the other channel is shown again", |window| {
        window.selected_channel() == random
    });
    window.cx.update(|window, _| window.activate_window());
    window.click_named(format!("direct-row:{}", conversation.id));
    window.until("the conversation is read", |window| {
        window.badge() == Some(1) && window.drawn_number(row).is_none()
    });
    assert!(window.drawn_number("directs-heading").is_none());
    let first = window.channel_names()[0].clone();
    window.click_channel(&first);
    window.until("the mention is read", |window| window.badge() == Some(0));
    assert_eq!(junto_client::app_badge::title(0), "Junto");
}

/// The administration counts the exports that are ready to save and the archived channels
/// whose deletion is due, beside the join requests (ADR 0761).
#[gpui::test]
fn the_administration_counts_the_ready_exports_and_the_channels_deleted_soon(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let api = owner.api();
    let workspace_id = window.view().workspace().unwrap().id;
    let general = window.selected_channel();
    world.run(api.export_channel(general)).unwrap();
    world
        .run(api.set_archive_period(workspace_id, ArchivePeriod { days: Some(7) }))
        .unwrap();
    let doomed = window
        .channels()
        .into_iter()
        .find(|row| row.id != general)
        .unwrap();
    world.run(api.archive_channel(doomed.id)).unwrap();
    window.open_administration();
    window.until(
        "the navigation counts the export and the archived channel",
        |window| {
            window.drawn_number("admin-nav-exports").as_deref() == Some("1")
                && window.drawn_number("admin-nav-archive").as_deref() == Some("1")
        },
    );
    // The channel is restored: its count goes.
    world.run(api.restore_channel(doomed.id)).unwrap();
    window.until("the restored channel is not counted", |window| {
        window.drawn_number("admin-nav-archive").is_none()
    });
}

/// The row of a channel draws the count of the mentions of the person in that channel, and
/// a read of the channel takes the count away (ADR 0741).
#[gpui::test]
fn the_row_of_a_channel_counts_its_mentions_and_a_read_takes_the_count_away(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let (general, own) = (window.selected_channel(), window.own_account());
    let random = admin.text_channel(&world, &mut window, "random");
    let general_row = selector(format!("channel-row:{general}"));
    let random_row = selector(format!("channel-row:{random}"));
    assert!(window.drawn_mentions(random_row).is_none());

    admin.mention(&world, random, "first, ", own);
    admin.mention(&world, random, "second, ", own);
    window.until("the row of the channel counts two mentions", |window| {
        window.drawn_mentions(random_row).as_deref() == Some("2")
    });
    assert!(window.drawn_mentions(general_row).is_none());

    window.make_active();
    window.click_channel("random");
    window.until("the mentions of the channel are read", |window| {
        window.selected_channel() == random
            && window.shows(random_row)
            && window.drawn_mentions(random_row).is_none()
    });
}

/// The row of a thread under its channel draws the count of the mentions of the person in
/// the thread, for a thread that the person follows and for one that the person does not
/// follow, and the row of the channel counts both. A read of each thread takes its count
/// away; the followed thread stays listed without a count (ADR 0741, ADR 0796).
#[gpui::test]
fn a_thread_row_counts_the_mentions_of_its_thread_and_a_read_takes_the_count_away(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let (_server, admin, _directory, mut window, general, followed) = with_root(cx, &world);
    let own = window.own_account();
    // The person answers in the thread and follows it by the rule of the server.
    let answer = SendMessage {
        document: document("One per speaker."),
        reply_to: None,
        attachments: Vec::new(),
    };
    world
        .run(window.handle().api().reply_in_thread(followed, &answer))
        .unwrap();
    let followed_row = selector(format!("thread-row:{followed}"));
    window.until(
        "the followed thread is listed under its channel",
        |window| window.shows(followed_row),
    );
    assert!(window.drawn_mentions(followed_row).is_none());
    let other = admin.send(&world, general, document("Mixdown when?")).id;
    let other_row = selector(format!("thread-row:{other}"));
    let channel_row = selector(format!("channel-row:{general}"));

    admin.reply(&world, followed, mention("Agreed, ", own));
    admin.reply(&world, other, mention("What do you think, ", own));
    window.until("the followed thread counts its mention", |window| {
        window.drawn_mentions(followed_row).as_deref() == Some("1")
    });
    window.until("the other thread counts its mention", |window| {
        window.drawn_mentions(other_row).as_deref() == Some("1")
    });
    window.until(
        "the row of the channel counts the mentions of its threads",
        |window| window.drawn_mentions(channel_row).as_deref() == Some("2"),
    );

    window.make_active();
    window.click(followed_row);
    window.until("the followed thread is read", |window| {
        window.shows("thread-panel") && window.drawn_mentions(followed_row).is_none()
    });
    assert_eq!(window.drawn_mentions(other_row).as_deref(), Some("1"));
    window.click(other_row);
    window.until("the other thread is read", |window| {
        window.shows(other_row) && window.drawn_mentions(other_row).is_none()
    });
    assert!(window.shows(followed_row));
    assert!(window.drawn_mentions(followed_row).is_none());
    window.until("the row of the channel counts no mention", |window| {
        window.drawn_mentions(channel_row).is_none()
    });
}

/// A mention in a channel of a server in the background is counted at the row of its
/// workspace, at the head of the server in the expanded server book, at the picture of the
/// server in the collapsed server book and in the menu of that picture. A read of the
/// channel takes every count away (ADR 0741).
#[gpui::test]
fn a_mention_on_a_server_in_the_background_is_counted_at_its_workspace_its_head_and_its_picture(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let first = world.server("Acme Studio");
    let second = world.server("Harbor");
    let first_admin = world.owner(&first);
    let second_admin = world.owner(&second);
    let (_directory, mut window) = member(cx, &world, &first.address(), &first_admin, "Luca");
    let (general, luca) = (window.selected_channel(), window.own_account());
    let seed = window.view().workspace().unwrap().id.as_uuid().as_u128();
    let workspace_row = selector(format!("workspace-row-{seed}"));
    window.join_further(&world, &second, &second_admin);
    assert!(window.shows(workspace_row));
    assert!(window.drawn_mentions(workspace_row).is_none());
    assert!(window.drawn_number("server-head-unread-0").is_none());

    first_admin.mention(&world, general, "Ping, ", luca);
    window.until("the row of the workspace counts the mention", |window| {
        window.drawn_mentions(workspace_row).as_deref() == Some("1")
    });
    // A mention without a direct message is a message for the person too.
    window.until("the head of the server counts the mention", |window| {
        window.drawn_number("server-head-unread-0").as_deref() == Some("1")
    });
    assert!(window.drawn_number("server-head-unread-1").is_none());

    window.click("collapse-server-book");
    window.until("the server book is collapsed", |window| {
        window.shows("expand-server-book")
    });
    window.until("the picture of the server counts the mention", |window| {
        window.drawn_number("server-unread-0").as_deref() == Some("1")
    });
    assert!(window.drawn_number("server-unread-1").is_none());
    window.in_shell(|shell, window, cx| shell.open(Open::ServerHoverMenu, window, cx));
    assert!(window.shows("server-hover-menu"));
    assert_eq!(window.drawn_mentions(workspace_row).as_deref(), Some("1"));
    window.press("escape");
    assert!(!window.shows("server-hover-menu"));

    // The person shows the channel of the mention in the active window.
    window.make_active();
    window.click("server-0");
    window.until("the first server is shown", |window| {
        let view = window.view();
        view.servers[0].selected && view.workspace().is_some()
    });
    window.click_channel("general");
    window.until("the picture of the server counts nothing", |window| {
        window.shows("server-0") && window.drawn_number("server-unread-0").is_none()
    });
    window.click("expand-server-book");
    window.until("the server book is expanded", |window| {
        window.shows("collapse-server-book")
    });
    assert!(window.shows(workspace_row) && window.shows("server-0"));
    assert!(window.drawn_mentions(workspace_row).is_none());
    assert!(window.drawn_number("server-head-unread-0").is_none());
}

/// The row "Notifications" of the channel column and the bell of the call window draw the
/// count of the unread notices, and "Mark all as read" takes the count away (ADR 0742).
#[gpui::test]
fn the_unread_notices_are_counted_on_their_row_and_on_the_bell_of_the_call_window(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let (general, own) = (window.selected_channel(), window.own_account());
    let random = admin.text_channel(&world, &mut window, "random");
    voice_channel(&world, &admin, &mut window, "Daily");
    assert!(window.drawn_number("notices-row").is_none());

    admin.mention(&world, random, "first, ", own);
    admin.mention(&world, random, "second, ", own);
    window.until("the row of the notices counts two", |window| {
        window.drawn_number("notices-row").as_deref() == Some("2")
    });
    window.open_notices();
    window.click("mark-all-read");
    window.until("the notices are read", |window| {
        window.unread_notices() == 0 && window.drawn_number("notices-row").is_none()
    });
    window.click("close-notices");

    window.click_channel("Daily");
    let call = connected_call(&mut window, "Daily");
    give_calls(&mut window, call);
    window.click("detach-call");
    let mut detached = window.call_window();
    assert!(detached.shows("open-call-notices"));
    assert!(detached.drawn_number("open-call-notices-count").is_none());

    admin.mention(&world, general, "in the call, ", own);
    detached.until("the bell of the call window counts the notice", |window| {
        window.drawn_number("open-call-notices-count").as_deref() == Some("1")
    });
    window.until("the row of the notices counts the notice", |window| {
        window.drawn_number("notices-row").as_deref() == Some("1")
    });
    detached.click("open-call-notices");
    detached.until("the notices are listed in the call window", |window| {
        window.shows("notices-panel") && window.notices().is_some_and(|panel| !panel.reading)
    });
    detached.click("mark-all-read");
    detached.until("the bell of the call window counts nothing", |window| {
        window.unread_notices() == 0
            && window.shows("open-call-notices")
            && window.drawn_number("open-call-notices-count").is_none()
    });
    window.until("the row of the notices counts nothing", |window| {
        window.drawn_number("notices-row").is_none()
    });
}

/// The Members tab of the panel of a call counts the requests to enter that wait and, for
/// the presenter of meeting mode and for a person with "mute members", the raised hands of
/// the other people. The tab that is shown draws no count; the button that opens the tab in
/// the call window draws the same count (ADR 0741, ADR 0761).
#[gpui::test]
fn the_members_tab_counts_the_requests_to_enter_and_the_hands_for_who_may_lower_them(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = connected_call(&mut window, "Daily");
    let handle = window.handle();
    let base = world.run(handle.api().snapshot()).unwrap().snapshot;
    give_calls(&mut window, call.clone());
    let call_id = CallId::generate();
    window.until("the panel of the call is drawn", |window| {
        window.shows("call-tab-members")
    });
    window.click("call-tab-chat");

    // A person who may not lower hands is not told of a raised hand.
    let stated = Stated {
        raised: 1,
        ..Stated::default()
    };
    let raised = state_call(&mut window, &base, &call, call_id, stated);
    let tile = selector(format!("call-tile:{}", raised[0]));
    window.until("the person with the raised hand is in the call", |window| {
        window.shows(tile)
    });
    assert!(window.drawn_number("call-tab-members").is_none());

    // Two requests to enter.
    let stated = Stated {
        raised: 1,
        asking: 2,
        ..Stated::default()
    };
    state_call(&mut window, &base, &call, call_id, stated);
    window.until("the tab counts the two requests", |window| {
        window.drawn_number("call-tab-members").as_deref() == Some("2")
    });

    // With "mute members" the raised hand counts too.
    let stated = Stated {
        raised: 1,
        asking: 2,
        mute: true,
        ..Stated::default()
    };
    state_call(&mut window, &base, &call, call_id, stated);
    window.until("the tab counts the requests and the hand", |window| {
        window.drawn_number("call-tab-members").as_deref() == Some("3")
    });

    // The presenter of meeting mode is told of the raised hands.
    let stated = Stated {
        raised: 2,
        presenter: true,
        ..Stated::default()
    };
    state_call(&mut window, &base, &call, call_id, stated);
    window.until("the tab counts the two hands for the presenter", |window| {
        window.drawn_number("call-tab-members").as_deref() == Some("2")
    });
    window.click("call-tab-members");
    assert!(window.shows("call-members"));
    assert!(window.drawn_number("call-tab-members").is_none());
    window.click("call-tab-chat");
    assert_eq!(
        window.drawn_number("call-tab-members").as_deref(),
        Some("2")
    );

    // The call window draws the same count on its button, until the hands are down.
    window.click("detach-call");
    let mut detached = window.call_window();
    detached.until("the call window counts the two hands", |window| {
        window.drawn_number("open-call-members-count").as_deref() == Some("2")
    });
    let stated = Stated {
        presenter: true,
        ..Stated::default()
    };
    state_call(&mut window, &base, &call, call_id, stated);
    detached.until("the call window counts nothing", |window| {
        window.shows("open-call-members")
            && window.drawn_number("open-call-members-count").is_none()
    });
}

/// The Chat tab of the panel of a call counts the unread messages of the chat of the call
/// while another tab is shown. A read of the chat takes the count away (ADR 0741).
#[gpui::test]
fn the_chat_tab_counts_the_unread_messages_of_the_call_while_another_tab_is_shown(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    let daily = (window.channels().into_iter())
        .find(|row| row.name == "Daily")
        .unwrap()
        .id;
    window.click_channel("Daily");
    let call = connected_call(&mut window, "Daily");
    give_calls(&mut window, call);
    window.until("the panel of the call is drawn", |window| {
        window.shows("call-tab-members")
    });
    window.click("call-tab-members");
    assert!(window.drawn_number("call-tab-chat").is_none());

    admin.send(&world, daily, document("one"));
    admin.send(&world, daily, document("two"));
    window.until("the Chat tab counts the two messages", |window| {
        window.drawn_number("call-tab-chat").as_deref() == Some("2")
    });

    window.until("the panel of the call is drawn again", |window| {
        window.shows("call-tab-chat")
    });
    assert_eq!(window.drawn_number("call-tab-chat").as_deref(), Some("2"));

    // The person reads the chat in the active window.
    window.make_active();
    window.click("call-tab-chat");
    assert!(window.drawn_number("call-tab-chat").is_none());
    window.until("the chat of the call is read", |window| {
        window.feed().is_some_and(|feed| feed.unread == 0)
    });
    window.click("call-tab-members");
    assert!(window.shows("call-members"));
    assert!(window.drawn_number("call-tab-chat").is_none());
}

/// In a window of the least size the buttons of the header that open the panel of a call
/// draw the count of their tab inside the window. Ignored until the header of a call keeps
/// these buttons inside a window of 720 points (the layout branch corrects the header).
#[gpui::test]
#[ignore = "waits for the correction of the call header in a narrow window"]
fn the_buttons_of_the_panel_in_a_narrow_window_draw_the_counts_of_their_tabs(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    let daily = (window.channels().into_iter())
        .find(|row| row.name == "Daily")
        .unwrap()
        .id;
    window.click_channel("Daily");
    let call = connected_call(&mut window, "Daily");
    give_calls(&mut window, call);
    window.until("the panel of the call is drawn", |window| {
        window.shows("call-tab-members")
    });
    window.click("call-tab-members");
    admin.send(&world, daily, document("one"));
    admin.send(&world, daily, document("two"));
    window.until("the Chat tab counts the two messages", |window| {
        window.drawn_number("call-tab-chat").as_deref() == Some("2")
    });

    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(720.0), gpui::px(600.0))));
    window.settle();
    window.click_if_shown("close-call-panel");
    window.until(
        "the narrow window draws the buttons of the panel",
        |window| window.shows("open-call-chat"),
    );
    let size = window.cx.update(|window, _| window.viewport_size());
    for name in ["open-call-chat", "open-call-members", "open-call-settings"] {
        let bounds = window.cx.debug_bounds(name).unwrap();
        assert!(
            bounds.right() <= size.width,
            "{name}: {bounds:?} in {size:?}"
        );
    }
    assert_eq!(
        window.drawn_number("open-call-chat-count").as_deref(),
        Some("2")
    );
}
