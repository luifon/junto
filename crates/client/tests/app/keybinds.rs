//! The key bindings of the person (ADR 0777): the Keybinds section of the settings records
//! the keys of an action, shows a conflict, resets one action and all of them, and keeps
//! them over a restart; a binding does its action in the window; a call binding that may
//! work outside the window works from the keyboard of the system.

use std::time::Duration;

use gpui::TestAppContext;
use junto_client::shell::{Open, SettingsSection};
use junto_client::state::KeybindChange;
use junto_client_core::global_keys::{HeldKeys, Key, KeyCombo};
use junto_client_core::keybinds::{KeyAction, Keybinds};

use crate::calls::{connected_call, voice_channel};
use crate::harness::{Window, World, member, open};

fn combo(text: &str) -> KeyCombo {
    text.parse().unwrap()
}

/// The keystroke of gpui for these keys, as `Window::press` takes it.
fn keystroke(keys: &KeyCombo) -> String {
    keys.keys()
        .iter()
        .map(|key| match key {
            Key::Control => "ctrl".to_owned(),
            Key::Alt => "alt".to_owned(),
            Key::Shift => "shift".to_owned(),
            Key::Meta => "cmd".to_owned(),
            Key::Letter(letter) => letter.to_ascii_lowercase().to_string(),
            Key::Digit(digit) => digit.to_string(),
            Key::Function(number) => format!("f{number}"),
            Key::Up => "up".to_owned(),
            Key::Down => "down".to_owned(),
            Key::Left => "left".to_owned(),
            Key::Right => "right".to_owned(),
            Key::Enter => "enter".to_owned(),
            Key::Escape => "escape".to_owned(),
            Key::Backspace => "backspace".to_owned(),
            Key::PageUp => "pageup".to_owned(),
            Key::PageDown => "pagedown".to_owned(),
            Key::Comma => ",".to_owned(),
            Key::Slash => "/".to_owned(),
            Key::Equal => "=".to_owned(),
            Key::Minus => "-".to_owned(),
            Key::BracketLeft => "[".to_owned(),
            Key::BracketRight => "]".to_owned(),
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Presses the default keys of an action on the system of the test.
fn press_default(window: &mut Window, action: KeyAction) {
    let keys = Keybinds::default_binding(action).keys.unwrap();
    window.press(&keystroke(&keys));
}

fn keybinds(window: &mut Window) -> Keybinds {
    window
        .state
        .read_with(&window.cx, |state, _| state.keybinds().clone())
}

fn open_keybinds(window: &mut Window) {
    window.in_shell(|shell, window, cx| {
        shell.open(Open::Settings, window, cx);
        shell.open_settings_section(SettingsSection::Keybinds, window, cx);
    });
    assert!(window.shows("settings-page-settings-section-keybinds"));
}

#[gpui::test]
fn push_to_talk_outside_switch_preserves_the_inherited_key(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let mut window = open(cx, directory.path());
    let inherited = window
        .state
        .read_with(&window.cx, |state, _| state.talk_keys());
    for outside in [false, true] {
        window.state.update(&mut window.cx, |state, cx| {
            state
                .change_keybinds(KeybindChange::Outside(KeyAction::PushToTalk, outside), cx)
                .unwrap();
            assert_eq!(state.talk_keys(), inherited);
        });
    }
}

#[gpui::test]
fn removing_push_to_talk_survives_restart_and_reset_restores_inheritance(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().unwrap();
    let mut window = open(cx, directory.path());
    let inherited = window
        .state
        .read_with(&window.cx, |state, _| state.talk_keys());
    window.state.update(&mut window.cx, |state, cx| {
        state
            .change_keybinds(KeybindChange::Set(KeyAction::PushToTalk, None), cx)
            .unwrap();
        assert_eq!(state.talk_keys(), None);
    });
    let store = window
        .state
        .read_with(&window.cx, |state, _| state.client().settings());
    window.until("the removed binding is written", |_| {
        store
            .load_keybinds()
            .unwrap()
            .is_changed(KeyAction::PushToTalk)
    });
    window.close();
    let mut again = open(cx, directory.path());
    again.state.update(&mut again.cx, |state, cx| {
        assert_eq!(state.talk_keys(), None);
        state
            .change_keybinds(KeybindChange::Reset(KeyAction::PushToTalk), cx)
            .unwrap();
        assert_eq!(state.talk_keys(), inherited);
        state
            .change_keybinds(KeybindChange::Set(KeyAction::PushToTalk, None), cx)
            .unwrap();
        state.change_keybinds(KeybindChange::ResetAll, cx).unwrap();
        assert_eq!(state.talk_keys(), inherited);
    });
}

#[gpui::test]
fn a_binding_is_recorded_its_conflict_shown_and_reset_and_a_second_start_keeps_it(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    open_keybinds(&mut window);

    // Set waits for the keys; a key without a modifier is refused and the wait goes on.
    window.click_in_settings("keybind-set-search");
    assert!(window.shows("keybind-keys-search"));
    window.press("f");
    assert!(window.shows("keybind-refusal"));
    window.press("ctrl-alt-s");
    assert_eq!(
        keybinds(&mut window).get(KeyAction::Search).keys,
        Some(combo("control+alt+s"))
    );
    assert!(!window.shows("keybind-refusal"));

    // Escape ends a wait without a change.
    let notices = keybinds(&mut window).get(KeyAction::Notices).keys;
    window.click_in_settings("keybind-set-notices");
    window.press("escape");
    assert_eq!(keybinds(&mut window).get(KeyAction::Notices).keys, notices);

    // The same keys for a second action: both rows state the conflict.
    window.click_in_settings("keybind-set-notices");
    window.press("ctrl-alt-s");
    assert!(window.shows("keybind-conflict-search") && window.shows("keybind-conflict-notices"));
    // Reset of one action: its default, and the conflict is gone.
    window.click_in_settings("keybind-reset-notices");
    assert!(!keybinds(&mut window).is_changed(KeyAction::Notices));
    assert!(!window.shows("keybind-conflict-search"));

    // A second start reads the binding from the settings file.
    window.settle();
    std::thread::sleep(Duration::from_millis(100));
    window.settle();
    window.close();
    let mut again = open(cx, directory.path());
    again.until_ready();
    assert_eq!(
        keybinds(&mut again).get(KeyAction::Search).keys,
        Some(combo("control+alt+s"))
    );

    // Reset all: every action has its default.
    open_keybinds(&mut again);
    again.click_in_settings("reset-keybinds");
    assert_eq!(keybinds(&mut again), Keybinds::default());
}

#[gpui::test]
fn a_binding_does_its_action_while_the_window_is_in_front(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    admin.text_channel(&world, &mut window, "plans");
    window.click_channel("general");

    // The members list opens and closes.
    assert!(!window.in_shell(|shell, _, _| shell.members_are_open()));
    press_default(&mut window, KeyAction::MembersList);
    assert!(window.in_shell(|shell, _, _| shell.members_are_open()));
    press_default(&mut window, KeyAction::MembersList);
    assert!(!window.in_shell(|shell, _, _| shell.members_are_open()));

    // The next channel of the list, and back.
    let first = window.selected_channel();
    press_default(&mut window, KeyAction::NextChannel);
    window.until("another channel is shown", |window| {
        window.selected_channel() != first
    });
    press_default(&mut window, KeyAction::PreviousChannel);
    window.until("the first channel is shown again", |window| {
        window.selected_channel() == first
    });

    // The server book collapses, and a changed binding does the action with its new keys.
    window.state.update(&mut window.cx, |state, cx| {
        state
            .change_keybinds(
                KeybindChange::Set(KeyAction::ToggleServerBook, Some(combo("control+alt+b"))),
                cx,
            )
            .unwrap();
    });
    window.press("ctrl-alt-b");
    assert!(window.state.read_with(&window.cx, |state, _| {
        state.settings().server_book_collapsed
    }));

    // The settings open at their keys.
    press_default(&mut window, KeyAction::OpenSettings);
    assert!(window.in_shell(|shell, _, _| shell.is_settings_open()));
}

/// The keyboard of the system reads the call bindings that may work while another
/// application is in front, in a call: push to mute closes the microphone while it is held,
/// the sound goes off at its key, and a binding that the person did not let work outside is
/// not read.
#[gpui::test]
fn a_call_binding_works_from_the_keyboard_of_the_system_when_it_may_work_outside(
    cx: &mut TestAppContext,
) {
    use junto_media::calls::CallsView;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let held = HeldKeys::default();
    window.state.update(&mut window.cx, |state, cx| {
        state.install_held_keys(held.clone(), cx);
        state
            .change_keybinds(
                KeybindChange::Set(KeyAction::PushToMute, Some(combo("f13"))),
                cx,
            )
            .unwrap();
    });
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    let call = connected_call(&mut window, "Daily");
    calls.show_view_for_test(CallsView {
        call: Some(call),
        ..CallsView::default()
    });
    window.settle();
    let view = |window: &mut Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.calls().view())
    };

    // Push to mute: closed while held; the release ends the hold, which opens the microphone
    // again at the seat of the call (the call of this test has no media connection to open
    // it in).
    let holds = |window: &mut Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.holds_mute())
    };
    held.hold(Ok(vec![Key::Function(13)]));
    window.until(
        "the held key of push to mute closed the microphone",
        |window| view(window).muted && holds(window),
    );
    held.hold(Ok(Vec::new()));
    window.until("the release ended the hold", |window| !holds(window));
    calls.set_muted(false);

    // The sound goes off at its default keys.
    let sound_off = Keybinds::default_binding(KeyAction::ToggleSoundOff)
        .keys
        .unwrap();
    held.hold(Ok(sound_off.keys().to_vec()));
    window.until("the key of the sound turned it off", |window| {
        view(window).deafened
    });
    held.hold(Ok(Vec::new()));

    // The camera key works only in the window: its keys held on the keyboard of the system
    // change nothing until the person lets it work outside.
    let camera = Keybinds::default_binding(KeyAction::ToggleCamera)
        .keys
        .unwrap();
    let readings = held.readings();
    held.hold(Ok(camera.keys().to_vec()));
    window.until("the keys were read", |_| held.readings() > readings + 3);
    assert!(!window.state.read_with(&window.cx, |state, _| {
        state.system_gives(KeyAction::ToggleCamera)
    }));
    held.hold(Ok(Vec::new()));
    window.state.update(&mut window.cx, |state, cx| {
        state
            .change_keybinds(KeybindChange::Outside(KeyAction::ToggleCamera, true), cx)
            .unwrap();
    });
    window.settle();
    assert!(window.state.read_with(&window.cx, |state, _| {
        state.system_gives(KeyAction::ToggleCamera)
    }));
}

/// Share, marked to work while another application is in front, opens the share dialog of
/// the main window from the keyboard of the system (ADR 0777).
#[gpui::test]
fn share_from_the_keyboard_of_the_system_opens_the_share_dialog(cx: &mut TestAppContext) {
    use junto_media::calls::CallsView;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let held = HeldKeys::default();
    window.state.update(&mut window.cx, |state, cx| {
        state.install_held_keys(held.clone(), cx);
        state
            .change_keybinds(KeybindChange::Outside(KeyAction::ShareScreen, true), cx)
            .unwrap();
    });
    let call = connected_call(&mut window, "Daily");
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    calls.show_view_for_test(CallsView {
        call: Some(call.clone()),
        ..CallsView::default()
    });
    let media = junto_client::state::view::MediaSnapshot {
        calls: CallsView {
            call: Some(call),
            ..CallsView::default()
        },
        can_share: true,
        ..junto_client::state::view::MediaSnapshot::default()
    };
    window.state.update(&mut window.cx, |state, cx| {
        state.show_media_fixture(media, cx)
    });
    window.settle();
    assert!(window.state.read_with(&window.cx, |state, _| {
        state.system_gives(KeyAction::ShareScreen)
    }));
    let share = Keybinds::default_binding(KeyAction::ShareScreen)
        .keys
        .unwrap();
    held.hold(Ok(share.keys().to_vec()));
    window.until("the share dialog is open", |window| {
        window.shows("share-picker")
    });
    held.hold(Ok(Vec::new()));
}

/// Back and Forward go through the places that the window showed, and the keys of the direct
/// messages show the newest conversation and open the dialog of a new one.
#[gpui::test]
fn back_forward_and_the_direct_messages_at_their_keys(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let plans = admin.text_channel(&world, &mut window, "plans");
    window.click_channel("general");
    let general = window.selected_channel();
    window.click_channel("plans");
    assert_eq!(window.selected_channel(), plans);

    press_default(&mut window, KeyAction::Back);
    window.until("Back shows general", |window| {
        window.selected_channel() == general
    });
    press_default(&mut window, KeyAction::Forward);
    window.until("Forward shows plans", |window| {
        window.selected_channel() == plans
    });
    // A new visit forgets what Forward held.
    window.click_channel("general");
    press_default(&mut window, KeyAction::Forward);
    window.settle();
    assert_eq!(window.selected_channel(), general);

    let luca = window.own_account();
    let conversation = admin.write_to(&world, luca, "Hello from the administrator");
    window.until("the conversation is listed", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.directs.iter().any(|row| row.id == conversation))
    });
    press_default(&mut window, KeyAction::OpenDirectMessages);
    window.until("the newest conversation is shown", |window| {
        window
            .view()
            .lists()
            .and_then(|lists| lists.conversation.as_ref())
            .is_some_and(|shown| shown.id == conversation)
    });
    // Back returns to the channel.
    press_default(&mut window, KeyAction::Back);
    window.until("Back shows general again", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.conversation.is_none())
            && window.selected_channel() == general
    });
    press_default(&mut window, KeyAction::NewDirectMessage);
    assert!(window.shows("new-direct-dialog"));
}

/// The navigation and message actions that no other test presses do their action at their
/// default keys: the previous and next workspace and server, the next unread channel, the
/// pinned messages, mark the server as read, reply to the newest message, edit the last own
/// message (a draft in the composer is kept), the administration and the Keybinds section.
#[gpui::test]
fn the_navigation_and_message_actions_at_their_keys(cx: &mut TestAppContext) {
    use junto_client::shell::Pane;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let luca = window.own_account();
    let api = admin.handle.api();
    let lab = world.run(api.create_workspace("Lab".to_owned())).unwrap();
    world.run(api.add_workspace_member(lab.id, luca)).unwrap();
    window.until("the server lists two workspaces", |window| {
        window.view().servers[0].workspaces.len() == 2
    });
    let workspace = |window: &mut Window| window.view().workspace().map(|shown| shown.id);
    let first = workspace(&mut window);
    press_default(&mut window, KeyAction::NextWorkspace);
    window.until("the next workspace is shown", |window| {
        workspace(window) != first
    });
    press_default(&mut window, KeyAction::PreviousWorkspace);
    window.until("the first workspace is shown again", |window| {
        workspace(window) == first
    });

    let birch = world.server("Birch Works");
    let birch_admin = world.owner(&birch);
    window.join_further(&world, &birch, &birch_admin);
    let selected = |window: &mut Window| window.view().selected.as_ref().map(|shown| shown.id);
    let on_birch = selected(&mut window);
    press_default(&mut window, KeyAction::PreviousServer);
    window.until("the previous server is shown", |window| {
        selected(window) != on_birch
    });
    press_default(&mut window, KeyAction::NextServer);
    window.until("the next server is shown", |window| {
        selected(window) == on_birch
    });
    press_default(&mut window, KeyAction::PreviousServer);
    window.until("the first server is shown again", |window| {
        selected(window) != on_birch && window.view().workspace().is_some()
    });

    let plans = admin.text_channel(&world, &mut window, "plans");
    window.click_channel("general");
    let general = window.selected_channel();
    admin.mention(&world, plans, "Plans for the week, ", luca);
    window.until("plans holds the unread mention", |window| {
        window.view().servers[0].unread.any()
    });
    press_default(&mut window, KeyAction::NextUnreadChannel);
    window.until("the unread channel is shown", |window| {
        window.selected_channel() == plans
    });
    press_default(&mut window, KeyAction::PinnedMessages);
    assert!(window.shows("pins-panel"));
    press_default(&mut window, KeyAction::PinnedMessages);
    assert!(!window.shows("pins-panel"));

    window.click_channel("general");
    assert_eq!(window.selected_channel(), general);
    admin.mention(&world, plans, "And the agenda, ", luca);
    // The test window is not the active window, so a shown channel is not read: both
    // mentions are unread before the key.
    window.until("the server holds both mentions", |window| {
        window.view().servers[0].unread.mentions == 2
    });
    press_default(&mut window, KeyAction::MarkServerRead);
    window.until_read(
        "the server holds nothing unread",
        |window| {
            let view = window.view();
            (
                view.servers[0].unread,
                view.workspace().map(|shown| shown.sections.clone()),
            )
        },
        |(unread, _)| !unread.any(),
    );

    window.write("Hello from Luca");
    let own = window.until_message("Hello from Luca").message.id;
    press_default(&mut window, KeyAction::ReplyToNewest);
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main)),
        (Some(own), None)
    );
    window.press("escape");
    press_default(&mut window, KeyAction::EditLastMessage);
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main)),
        (None, Some(own))
    );
    window.press("escape");
    window.in_shell(|shell, window, cx| shell.type_message(Pane::Main, "draft", window, cx));
    press_default(&mut window, KeyAction::EditLastMessage);
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main)),
        (None, None)
    );
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "draft"
    );

    press_default(&mut window, KeyAction::OpenAdministration);
    assert!(window.shows("admin"));
    window.press("escape");
    press_default(&mut window, KeyAction::OpenKeybinds);
    assert!(window.shows("settings-page-settings-section-keybinds"));
}

#[gpui::test]
fn recording_a_binding_suppresses_global_actions_until_the_recording_press_is_released(
    cx: &mut TestAppContext,
) {
    use junto_media::calls::CallsView;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let held = HeldKeys::default();
    window.state.update(&mut window.cx, |state, cx| {
        state.install_held_keys(held.clone(), cx)
    });
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    let call = connected_call(&mut window, "Daily");
    calls.show_view_for_test(CallsView {
        call: Some(call.clone()),
        ..CallsView::default()
    });
    window.settle();
    open_keybinds(&mut window);
    window.click_in_settings("keybind-set-toggle_mute");
    let mute = Keybinds::default_binding(KeyAction::ToggleMute)
        .keys
        .unwrap();
    let readings = held.readings();
    held.hold(Ok(mute.keys().to_vec()));
    window.until("the old shortcut was sampled during recording", |_| {
        held.readings() > readings + 3
    });
    assert!(!calls.view().muted, "recording executed the old binding");
    window.press("ctrl-shift-m");
    window.dispatch(gpui::ModifiersChangedEvent::default());
    let readings = held.readings();
    window.until("the recording keys are still held", |_| {
        held.readings() > readings + 3
    });
    assert!(
        !calls.view().muted,
        "the recorded press executed its binding"
    );
    held.hold(Ok(Vec::new()));
    let readings = held.readings();
    window.until("the recording keys were released", |_| {
        held.readings() > readings + 3
    });
    assert!(!calls.view().muted);
    held.hold(Ok(mute.keys().to_vec()));
    window.until("a fresh press executes the binding", |_| calls.view().muted);
    held.hold(Ok(Vec::new()));
    // Another section or closing the settings ends the recording and its pause. The call of
    // this test has no media connection to open the microphone in, so each case starts
    // with an open microphone and the press closes it.
    for close in [false, true] {
        calls.show_view_for_test(CallsView {
            call: Some(call.clone()),
            ..CallsView::default()
        });
        window.settle();
        window.click_in_settings("keybind-set-toggle_mute");
        if close {
            window.click("dialog-close");
        } else {
            window.in_shell(|shell, window, cx| {
                shell.open_settings_section(SettingsSection::Voice, window, cx)
            });
        }
        let readings = held.readings();
        window.until("the keyboard was read with no key held", |_| {
            held.readings() > readings + 3
        });
        assert!(
            !window
                .state
                .read_with(&window.cx, |state, _| { state.recording_keybind() })
        );
        assert!(!calls.view().muted);
        held.hold(Ok(mute.keys().to_vec()));
        window.until(
            &format!("a press after the recording ended executes the binding (close: {close})"),
            |_| calls.view().muted,
        );
        held.hold(Ok(Vec::new()));
        open_keybinds(&mut window);
    }
}

#[gpui::test]
fn a_window_releases_push_to_mute_on_deactivation_and_destruction(cx: &mut TestAppContext) {
    use junto_media::calls::CallsView;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut main) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut main, "Daily");
    main.click_channel("Daily");
    let call = connected_call(&mut main, "Daily");
    main.state.update(&mut main.cx, |state, cx| {
        state
            .change_keybinds(
                KeybindChange::Set(KeyAction::PushToMute, Some(combo("f13"))),
                cx,
            )
            .unwrap();
        state.calls().show_view_for_test(CallsView {
            call: Some(call),
            ..CallsView::default()
        });
    });
    main.settle();
    main.click("detach-call");
    let mut detached = main.call_window();
    let hold = |window: &mut Window| {
        window.cx.update(|window, _| window.activate_window());
        window.dispatch(gpui::KeyDownEvent {
            keystroke: gpui::Keystroke::parse("f13").unwrap(),
            is_held: false,
            prefer_character_input: false,
        });
        assert!(
            window
                .state
                .read_with(&window.cx, |state, _| state.holds_mute())
        );
    };
    hold(&mut detached);
    main.cx.update(|window, _| window.activate_window());
    main.settle();
    assert!(
        !main
            .state
            .read_with(&main.cx, |state, _| state.holds_mute()),
        "deactivation kept the mute hold"
    );
    hold(&mut detached);
    detached.cx.update(|window, _| window.remove_window());
    drop(detached);
    main.settle();
    assert!(
        !main
            .state
            .read_with(&main.cx, |state, _| state.holds_mute()),
        "destruction kept the mute hold"
    );
}

/// A binding goes before the keys that the focused text field of the composer binds to its
/// own actions (ADR 0777): Alt+Shift+Down adds a cursor in a text field on Linux and goes to
/// the next unread channel here; Command+Up moves to the start of a text field on macOS and
/// edits the last own message here.
#[gpui::test]
fn a_binding_goes_before_the_keys_of_the_focused_composer(cx: &mut TestAppContext) {
    use junto_client::shell::Pane;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let luca = window.own_account();
    let plans = admin.text_channel(&world, &mut window, "plans");
    window.click_channel("general");
    admin.mention(&world, plans, "Plans for the week, ", luca);
    window.until("plans holds the unread mention", |window| {
        window.view().servers[0].unread.any()
    });
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));
    press_default(&mut window, KeyAction::NextUnreadChannel);
    window.until("the unread channel is shown", |window| {
        window.selected_channel() == plans
    });

    window.write("Hello from Luca");
    let own = window.until_message("Hello from Luca").message.id;
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));
    press_default(&mut window, KeyAction::EditLastMessage);
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main)),
        (None, Some(own))
    );
}
