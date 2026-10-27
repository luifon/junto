//! The notices of the call of this device that the person puts away, and the notices of
//! the server in the panel of the call and in the call window, without a media server.

use gpui::TestAppContext;
use junto_client_core::protocol::call::{CallInvitation, CallPlace};
use junto_client_core::protocol::ids::{CallId, CallInvitationId};
use junto_media::calls::{ActiveCall, CallTarget, CallsView, Phase};
use junto_media::engine::{MicrophoneState, OutputState};
use junto_media::share::CaptureError;

use crate::calls::voice_channel;
use crate::harness::{Window, World, member, selector};

/// A connected call of this device in the voice channel with this name, without an own
/// share, and with the statement of an own share that ended by itself, as the media engine
/// states it.
fn call_with_share(window: &mut Window, channel: &str, ended: Option<CaptureError>) -> ActiveCall {
    let view = window.view();
    let row = (window.channels().into_iter())
        .find(|row| row.name == channel)
        .unwrap();
    ActiveCall {
        camera: junto_media::camera::CameraState::Off,
        cameras: Vec::new(),
        target: CallTarget {
            server_id: view.selected.as_ref().unwrap().id,
            place: CallPlace::Channel {
                workspace_id: view.workspace().unwrap().id,
                channel_id: row.id,
            },
        },
        identity: "seat".to_owned(),
        phase: Phase::Connected,
        interrupted_at: None,
        microphone: MicrophoneState::Running,
        access: junto_media::permission::MicrophoneAccess::Allowed,
        input_device: None,
        output: OutputState::Running,
        output_device: None,
        microphone_block: None,
        sound_block: None,
        share_limit: junto_client_core::protocol::call::MAX_SHARES,
        own_shares: std::collections::BTreeMap::new(),
        share_ended: ended,
        shares: Vec::new(),
        watched: None,
    }
}

/// The calls of the window state this call of this device, as a session states it, and the
/// state of the application takes the change as it takes every change of the calls.
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

/// The notice of a share that ended by itself goes with the click of the person. The calls
/// put the ended share away on their own threads; the window does not wait for them, so
/// the notice is not shown again while they do.
#[gpui::test]
fn the_notice_of_an_ended_share_goes_at_the_click_of_the_person(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let ended = call_with_share(&mut window, "Daily", Some(CaptureError::SourceGone));
    give_calls(&mut window, ended.clone());
    assert!(window.shows("share-notice"));

    // The calls have not put the share away: they still state the end.
    window.click("dismiss-share-notice");
    assert!(!window.shows("share-notice"));
    give_calls(&mut window, ended.clone());
    assert!(!window.shows("share-notice"));

    // A later share that ends is told again.
    let running = call_with_share(&mut window, "Daily", None);
    give_calls(&mut window, running);
    give_calls(&mut window, ended);
    assert!(window.shows("share-notice"));
}

/// The notices of the server open in the panel of the call of this device, in place of
/// its tab, also in a narrow window, which draws the panel over the stage (ADR 0742).
#[gpui::test]
fn the_notices_open_in_the_panel_of_the_call(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let (general, own) = (window.selected_channel(), window.own_account());
    voice_channel(&world, &admin, &mut window, "Daily");
    // The notices that are open beside the text channel close when the main area shows
    // the call: its panel shows the chat.
    window.open_notices();
    window.click_channel("Daily");
    let call = call_with_share(&mut window, "Daily", None);
    give_calls(&mut window, call);
    assert!(window.shows("call-panel") && window.shows("call-chat"));
    assert!(!window.shows("notices-panel") && window.notices().is_none());

    // A mention in another channel is counted at the row of the notices.
    admin.mention(&world, general, "look at this, ", own);
    window.until("the notice is counted", |window| {
        window.unread_notices() == 1
    });
    assert!(window.shows("notices-count"));

    // The row opens the notices in the panel of the call, beside the stage.
    window.click("notices-row");
    window.until(
        "the notices are listed in the panel of the call",
        |window| {
            window.shows("notices-panel")
                && window
                    .notices()
                    .is_some_and(|panel| !panel.reading && panel.rows.len() == 1)
        },
    );
    assert!(window.shows("call-panel") && window.shows("call-stage"));
    assert!(!window.shows("call-chat"));
    // A tab of the panel shows its pane again.
    window.click("call-tab-members");
    assert!(!window.shows("notices-panel") && window.notices().is_none());

    // A narrow window draws the panel over the stage while it is open, and the row of the
    // notices opens it at the list.
    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(720.0), gpui::px(600.0))));
    window.settle();
    window.click("close-call-panel");
    assert!(!window.shows("call-panel") && window.shows("notices-count"));
    window.click("notices-row");
    window.until("the notices are listed over the stage", |window| {
        window.shows("call-panel") && window.shows("notices-panel")
    });
    window.click("close-notices");
    assert!(!window.shows("notices-panel") && window.shows("call-panel"));
}

/// The call window counts and lists the notices of the server of its call, whatever the
/// main window shows, also a notice of an archived channel and the newer ones after it. A
/// press on a row shows the message in the main window, brings it to the front and marks
/// the notice as read (ADR 0742, ADR 0772).
#[gpui::test]
fn the_call_window_counts_and_lists_the_notices(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let (general, own) = (window.selected_channel(), window.own_account());
    let random = admin.text_channel(&world, &mut window, "random");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = call_with_share(&mut window, "Daily", None);
    give_calls(&mut window, call);
    window.click("detach-call");
    let mut detached = window.call_window();
    assert!(detached.shows("open-call-notices") && !detached.shows("open-call-notices-count"));

    admin.mention(&world, general, "first, ", own);
    admin.mention(&world, random, "in the old channel, ", own);
    let last = admin.mention(&world, general, "last, ", own);
    detached.until("the notices are counted in the call window", |window| {
        window.shows("open-call-notices-count") && window.unread_notices() == 3
    });
    // The channel of the second notice is archived: it leaves the channel list, and the
    // state knows it from the archived list of its workspace only.
    world
        .run(admin.handle.api().archive_channel(random))
        .unwrap();
    window.until("the archived channel leaves the list", |window| {
        !window.channel_names().contains(&"random".to_owned())
    });

    // The call window lists the three notices, while the main window shows the place of
    // the call and has no list open.
    detached.click("open-call-notices");
    detached.until("the notices are listed in the call window", |window| {
        window.shows("notices-panel")
            && window
                .notices()
                .is_some_and(|panel| !panel.reading && panel.rows.len() == 3)
    });
    let panel = detached.notices().unwrap();
    let texts: Vec<&str> = panel.rows.iter().map(|row| row.text.as_str()).collect();
    assert_eq!(
        texts,
        ["first, @Luca", "in the old channel, @Luca", "last, @Luca"]
    );
    // The call window draws the newest notice at the bottom, as the main window does. The
    // list opens at its end, so the older of the two newest rows is drawn above it.
    let older = bounds_of(
        &mut detached,
        selector(format!("notice:{}", panel.rows[1].id)),
    );
    let newest = bounds_of(
        &mut detached,
        selector(format!("notice:{}", panel.rows[2].id)),
    );
    assert!(older.bottom() <= newest.top() + gpui::px(1.0));
    assert!(!window.shows("notices-panel"));

    // A press on the newest row: the main window shows the message in its channel, comes
    // to the front, and the notice is read.
    detached.click_named(format!("notice:{}", panel.rows[2].id));
    window.until("the main window shows the message", |window| {
        window.selected_channel() == general
            && window
                .messages()
                .iter()
                .any(|row| row.message.id == last.id && row.target)
    });
    assert!(window.cx.update(|window, _| window.is_window_active()));
    detached.until("the notice is read", |window| {
        window
            .notices()
            .is_some_and(|panel| panel.rows.last().is_some_and(|row| row.read))
    });
    // Both windows count the notices that stay unread: the notice of the archived channel
    // at least, and fewer than before.
    window.until("the main window counts fewer notices", |window| {
        (1..3).contains(&window.unread_notices()) && window.shows("notices-count")
    });
    let unread = window.unread_notices();
    detached.until("the call window counts as many notices", |window| {
        window.unread_notices() == unread && window.shows("open-call-notices-count")
    });

    // A tab of the panel shows its pane again.
    detached.click("call-tab-chat");
    assert!(!detached.shows("notices-panel") && detached.shows("call-chat"));
}

/// Before the state holds the conversation of a direct-message call, the call window has no
/// panel of the call; its bell still opens the notices of the server (ADR 0772).
#[gpui::test]
fn the_bell_of_the_call_window_opens_the_notices_before_the_conversation_exists(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let (general, own) = (window.selected_channel(), window.own_account());
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = call_with_share(&mut window, "Daily", None);
    give_calls(&mut window, call.clone());
    window.click("detach-call");
    // The call of this device becomes a direct-message call with an account whose
    // conversation the state does not hold.
    let direct = ActiveCall {
        target: CallTarget {
            server_id: call.target.server_id,
            place: CallPlace::direct(own, junto_client_core::protocol::ids::AccountId::generate()),
        },
        ..call
    };
    give_calls(&mut window, direct);
    let mut detached = window.call_window();
    assert!(!detached.shows("call-tab-chat"));

    admin.mention(&world, general, "here, ", own);
    detached.until("the bell counts the notice", |window| {
        window.shows("open-call-notices-count")
    });
    detached.click("open-call-notices");
    detached.until("the notices are listed in the call window", |window| {
        window.shows("notices-panel")
    });
}

/// The rules of a system notification count the call window as a window of the
/// application that shows the place of its call (ADR 0742).
#[gpui::test]
fn the_call_window_shows_the_place_of_the_call_to_the_rules_of_a_system_notification(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let (general, own) = (window.selected_channel(), window.own_account());
    let general_name = (window.channels().into_iter())
        .find(|row| row.id == general)
        .unwrap()
        .name;
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let daily = window.selected_channel();
    let call = call_with_share(&mut window, "Daily", None);
    give_calls(&mut window, call);
    window.click("detach-call");
    let mut detached = window.call_window();
    // The main window shows the text channel and is not active; the person looks at the
    // call window.
    window.click_channel(&general_name);
    window.open_notices();
    detached.simulate(|cx| cx.update(|window, _| window.activate_window()));
    detached.settle();

    // A mention in the chat of the call is no system notification, and a mention in the
    // channel that the inactive main window shows is one.
    admin.mention(&world, daily, "in the call, ", own);
    admin.mention(&world, general, "and here, ", own);
    window.until("both notices are listed", |window| {
        window
            .notices()
            .is_some_and(|panel| !panel.reading && panel.rows.len() == 2)
    });
    window.until("the system notification of the text channel", |window| {
        window
            .kept()
            .told
            .iter()
            .any(|alert| alert.body == "and here, @Luca")
    });
    let bodies: Vec<String> = window
        .kept()
        .told
        .iter()
        .map(|alert| alert.body.clone())
        .collect();
    assert_eq!(bodies, ["and here, @Luca"]);
}

/// The bounds of an element that the window draws now.
fn bounds_of(window: &mut Window, selector: &'static str) -> gpui::Bounds<gpui::Pixels> {
    window.settle();
    window
        .cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
}

/// The cards of the invitations (ADR 0739): at most three, the newest first, with a line
/// that shows the others; at the top of the main area over the stage of the call, inside
/// the window; drawn and answered over an open dialog; and a press that began before a card
/// appeared does not answer it.
#[gpui::test]
fn the_cards_of_invitations_are_bounded_and_lie_over_the_call(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let daily = window.selected_channel();
    let call = call_with_share(&mut window, "Daily", None);
    let target = call.target;
    give_calls(&mut window, call);
    let (own, ada) = (window.own_account(), admin.handle.account().unwrap());
    let start = time::OffsetDateTime::now_utc();
    let invitation = |minute: i64| CallInvitation {
        id: CallInvitationId::generate(),
        call_id: CallId::generate(),
        place: CallPlace::Channel {
            workspace_id: match target.place {
                CallPlace::Channel { workspace_id, .. } => workspace_id,
                CallPlace::Direct { .. } => unreachable!("a voice channel"),
            },
            channel_id: daily,
        },
        account_id: own,
        invited_by: ada,
        created_at: start + time::Duration::minutes(minute),
        expires_at: start + time::Duration::hours(1),
    };
    let show = |window: &mut Window, invitations: Vec<CallInvitation>| {
        let state = window.state.clone();
        state.update(&mut window.cx, |state, cx| {
            state.show_invitations_fixture(target.server_id, invitations, cx);
        });
        window.settle();
    };
    let five: Vec<CallInvitation> = (1..=5).map(invitation).collect();
    show(&mut window, five.clone());

    // The three newest are drawn, and a line above them states the two earlier ones.
    let one = bounds_of(&mut window, "call-invitation");
    let all = bounds_of(&mut window, "invitation-cards");
    assert!(all.size.height < one.size.height * 4.0, "{all:?} {one:?}");
    for (index, held) in five.iter().enumerate() {
        assert_eq!(
            window.shows_named(format!("invitation-card:{}", held.id)),
            index >= 2,
            "card {index}"
        );
    }
    assert!(window.shows("invitation-more"));
    let line = bounds_of(&mut window, "invitation-more");
    assert!(line.bottom() <= one.top(), "{line:?} {one:?}");

    // The cards lie over the main area and over the stage of the call, under the header
    // that lies above the stage, inside the window, also in a window of the least width.
    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(720.0), gpui::px(600.0))));
    let card = bounds_of(&mut window, "call-invitation");
    let column = bounds_of(&mut window, "channel-column");
    let stage = bounds_of(&mut window, "call-stage");
    let size = window.cx.update(|window, _| window.viewport_size());
    assert!(card.left() >= column.right(), "{card:?} {column:?}");
    assert!(card.right() <= size.width && card.bottom() <= size.height);
    assert!(card.top() >= stage.top() && card.bottom() <= stage.bottom());

    // The line shows the others; in a small window the list stays inside the window.
    window.click("invitation-more");
    for held in &five {
        assert!(window.shows_named(format!("invitation-card:{}", held.id)));
    }
    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(720.0), gpui::px(480.0))));
    let list = bounds_of(&mut window, "invitation-list");
    assert!(list.bottom() <= gpui::px(480.0), "{list:?}");

    // Over an open dialog the card is drawn, and its button is pressed without closing the
    // dialog.
    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(900.0))));
    window.click("app-settings");
    assert!(window.shows("settings-dialog") && window.shows("call-invitation"));
    window.click("invitation-decline");
    assert!(window.shows("settings-dialog"));
    window.press("escape");
    assert!(!window.shows("settings-dialog"));

    // At the largest scale in a window of the least width the main area is narrower than
    // a card: the card lies over the whole window, with both buttons whole.
    window.state.update(&mut window.cx, |state, cx| {
        state.set_scale(junto_client::theme::SCALE_MAX, cx);
    });
    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(720.0), gpui::px(600.0))));
    let card = bounds_of(&mut window, "call-invitation");
    let size = window.cx.update(|window, _| window.viewport_size());
    assert!(
        card.left() >= gpui::px(0.0) && card.right() <= size.width,
        "{card:?}"
    );
    for button in ["invitation-join", "invitation-decline"] {
        let drawn = bounds_of(&mut window, button);
        assert!(
            drawn.left() >= card.left() && drawn.right() <= card.right(),
            "{button} {drawn:?} {card:?}"
        );
    }
    window
        .state
        .update(&mut window.cx, |state, cx| state.set_scale(100, cx));
    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(1440.0), gpui::px(900.0))));

    // A press that began under the place where a card then appears, on the settings dialog
    // or on its cover, and ends at the same place on the card, answers nothing, and what lay
    // under the card stays as the press left it.
    show(&mut window, vec![invitation(6)]);
    window.click("app-settings");
    let place = bounds_of(&mut window, "invitation-join").center();
    show(&mut window, Vec::new());
    assert!(window.shows("settings-dialog") && !window.shows("invitation-cards"));
    let (down, up) = press_events(place);
    window.dispatch(down);
    let dialog = window.shows("settings-dialog");
    show(&mut window, vec![invitation(6)]);
    assert_eq!(bounds_of(&mut window, "invitation-join").center(), place);
    window.dispatch(up);
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().view());
    assert!(calls.joining.is_none() && calls.outcome.is_none());
    assert!(window.shows("call-invitation"));
    assert_eq!(window.shows("settings-dialog"), dialog);
}

/// A card of an invitation in a call window of 480 pixels lies inside the window, under its
/// header, with both buttons inside the card (ADR 0739).
#[gpui::test]
fn a_card_of_an_invitation_fits_a_narrow_call_window(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let daily = window.selected_channel();
    let call = call_with_share(&mut window, "Daily", None);
    let target = call.target;
    give_calls(&mut window, call);
    window.click("detach-call");
    let mut detached = window.call_window();
    detached.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(480.0), gpui::px(480.0))));
    let (own, ada) = (window.own_account(), admin.handle.account().unwrap());
    let now = time::OffsetDateTime::now_utc();
    let invitation = CallInvitation {
        id: CallInvitationId::generate(),
        call_id: CallId::generate(),
        place: CallPlace::Channel {
            workspace_id: match target.place {
                CallPlace::Channel { workspace_id, .. } => workspace_id,
                CallPlace::Direct { .. } => unreachable!("a voice channel"),
            },
            channel_id: daily,
        },
        account_id: own,
        invited_by: ada,
        created_at: now,
        expires_at: now + time::Duration::hours(1),
    };
    let state = window.state.clone();
    state.update(&mut window.cx, |state, cx| {
        state.show_invitations_fixture(target.server_id, vec![invitation], cx);
    });
    detached.settle();

    let card = bounds_of(&mut detached, "call-invitation");
    let size = detached.cx.update(|window, _| window.viewport_size());
    assert!(
        card.left() >= gpui::px(0.0) && card.right() <= size.width && card.bottom() <= size.height,
        "{card:?} {size:?}"
    );
    for button in ["invitation-join", "invitation-decline"] {
        let drawn = bounds_of(&mut detached, button);
        assert!(
            drawn.left() >= card.left() && drawn.right() <= card.right(),
            "{button} {drawn:?} {card:?}"
        );
    }
    let header = bounds_of(&mut detached, "open-call-notices");
    assert!(card.top() >= header.bottom(), "{card:?} {header:?}");
}

fn press_events(position: gpui::Point<gpui::Pixels>) -> (gpui::MouseDownEvent, gpui::MouseUpEvent) {
    let modifiers = gpui::Modifiers::none();
    (
        gpui::MouseDownEvent {
            position,
            modifiers,
            button: gpui::MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        },
        gpui::MouseUpEvent {
            position,
            modifiers,
            button: gpui::MouseButton::Left,
            click_count: 1,
        },
    )
}

/// A press on "Join" of one invitation that ends after a newer invitation arrived answers
/// the first invitation: the cards keep the order of arrival, and the ids of a card and of
/// its buttons are made from its invitation (ADR 0739).
#[gpui::test]
fn a_press_on_a_card_answers_its_own_invitation_when_another_arrives(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    voice_channel(&world, &admin, &mut window, "Focus");
    let view = window.view();
    let (server_id, workspace_id) = (
        view.selected.as_ref().unwrap().id,
        view.workspace().unwrap().id,
    );
    let channel = |window: &mut Window, name: &str| {
        (window.channels().into_iter())
            .find(|row| row.name == name)
            .unwrap()
            .id
    };
    let (daily, focus) = (channel(&mut window, "Daily"), channel(&mut window, "Focus"));
    let (own, ada) = (window.own_account(), admin.handle.account().unwrap());
    let start = time::OffsetDateTime::now_utc();
    let invitation = |channel_id, minute: i64| CallInvitation {
        id: CallInvitationId::generate(),
        call_id: CallId::generate(),
        place: CallPlace::Channel {
            workspace_id,
            channel_id,
        },
        account_id: own,
        invited_by: ada,
        created_at: start + time::Duration::minutes(minute),
        expires_at: start + time::Duration::hours(1),
    };
    let (first, second) = (invitation(daily, 1), invitation(focus, 2));
    let show = |window: &mut Window, invitations: Vec<CallInvitation>| {
        let state = window.state.clone();
        state.update(&mut window.cx, |state, cx| {
            state.show_invitations_fixture(server_id, invitations, cx);
        });
        window.settle();
    };

    show(&mut window, vec![first.clone()]);
    let place = bounds_of(&mut window, "invitation-join").center();
    let (down, up) = press_events(place);
    window.dispatch(down);
    show(&mut window, vec![first.clone(), second.clone()]);
    window.dispatch(up);

    // The join of the first invitation is asked for. The server does not know the
    // invented invitation and refuses it.
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    let failed = window.until_value("the join that the press asked for ends", |_| {
        match calls.view().outcome {
            Some(junto_media::calls::Outcome::JoinFailed { target, .. }) => Some(target),
            _ => None,
        }
    });
    assert_eq!(failed.place, first.place);
    // The drawn card stayed in its place; the new one follows it.
    let join = selector(format!("invitation-accept:{}", first.id));
    assert_eq!(bounds_of(&mut window, join).center(), place);
}
