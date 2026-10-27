//! The key bindings of the person (ADR 0777): the Keybinds section of the settings records
//! the keys of an action, shows a conflict, resets one action and all of them, and keeps
//! them over a restart; a binding does its action in the window; a call binding that may
//! work outside the window works from the keyboard of the system.

use std::time::Duration;

use gpui::TestAppContext;
use junto_client::shell::SettingsSection;
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
        shell.open_settings_section(SettingsSection::Keybinds, window, cx);
    });
    assert!(window.shows("settings-page-settings-section-keybinds"));
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

    // Push to mute: closed while held, open again at the release.
    held.hold(Ok(vec![Key::Function(13)]));
    window.until(
        "the held key of push to mute closed the microphone",
        |window| view(window).muted,
    );
    held.hold(Ok(Vec::new()));
    window.until("the release opened it again", |window| !view(window).muted);

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
