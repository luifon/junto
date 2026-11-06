//! The tray menu acts on a real call (ADR 0759): the kept tray of a start without the tray of
//! the system takes the presses.

use gpui::TestAppContext;
use junto_client::state::view::MicrophoneButton;
use junto_client::text::tray as text;
use junto_media::calls::Phase;

use super::{ROOM, own};
use crate::calls::voice_channel;
use crate::harness::{Window, World, member};

fn press(window: &mut Window, label: &str) {
    let tray = window
        .state
        .read_with(&window.cx, |state, _| state.kept_tray());
    let state = window.state.clone();
    let pressed = window.cx.update(|_, cx| tray.press(label, &state, cx));
    assert!(pressed, "the tray menu has no enabled entry {label:?}");
}

fn tooltip(window: &mut Window) -> String {
    window
        .state
        .read_with(&window.cx, |state, _| state.kept_tray().shown().tooltip())
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn closing_and_reopening_the_main_window_keeps_the_real_call(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("the call is connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    let (calls, tray) = window
        .state
        .read_with(cx, |state, _| (state.calls().clone(), state.kept_tray()));
    let seat = calls.view().call.as_ref().unwrap().seat();
    window.simulate(|cx| cx.simulate_keystrokes("secondary-w"));
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    assert_eq!(calls.view().call.as_ref().unwrap().seat(), seat);
    cx.update(|cx| assert!(tray.press(text::OPEN_JUNTO, &window.state, cx)));
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    assert_eq!(calls.view().call.as_ref().unwrap().seat(), seat);
    cx.update(|cx| assert!(tray.press(text::LEAVE_THE_CALL, &window.state, cx)));
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn the_tray_menu_mutes_turns_the_sound_off_and_leaves_a_real_call(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, ROOM);
    window.click_channel(ROOM);
    assert_eq!(tooltip(&mut window), text::NOT_IN_A_CALL);
    window.click("join-call");
    window.until("the call is connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    window.until("the tray shows the call", |window| {
        tooltip(window) == text::IN_A_CALL
    });

    press(&mut window, text::MUTE_MICROPHONE);
    window.until("the microphone is closed", |window| {
        window.view().controls.microphone == MicrophoneButton::Muted
            && tooltip(window) == text::IN_A_CALL_MUTED
    });
    press(&mut window, text::UNMUTE_MICROPHONE);
    window.until("the microphone is open again", |window| {
        window.view().controls.microphone == MicrophoneButton::Open
            && tooltip(window) == text::IN_A_CALL
    });
    press(&mut window, text::TURN_SOUND_OFF);
    window.until("the sound is off", |window| {
        window.view().controls.sound_off && tooltip(window) == text::IN_A_CALL_SOUND_OFF
    });
    press(&mut window, text::TURN_SOUND_ON);
    window.until("the sound is on", |window| {
        !window.view().controls.sound_off && tooltip(window) == text::IN_A_CALL
    });

    press(&mut window, text::LEAVE_THE_CALL);
    window.until("the call has ended", |window| {
        window.view().call.is_none() && tooltip(window) == text::NOT_IN_A_CALL
    });
    window.until("the server lists the person no more", |window| {
        own(window).is_none()
    });
}

/// With "Turning the sound off also mutes the microphone" (on by default), the sound off
/// mutes the microphone of a real call and the sound on returns it to its state before;
/// unmuting with the key turns the sound on. With the setting off the two are apart, and
/// the tray shows the picture of both (ADR 0739, ADR 0759).
#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn turning_the_sound_off_mutes_the_microphone_and_turning_it_on_returns_it(
    cx: &mut TestAppContext,
) {
    use junto_client_core::keybinds::KeyAction;
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, ROOM);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("the call is connected with the microphone on", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
            && window.view().controls.microphone == MicrophoneButton::Open
    });
    let microphone = |window: &mut Window| window.view().controls.microphone;

    // An open microphone: the sound off mutes it, the sound on opens it again.
    press(&mut window, text::TURN_SOUND_OFF);
    window.until("the sound off muted the microphone", |window| {
        window.view().controls.sound_off
            && microphone(window) == MicrophoneButton::Muted
            && tooltip(window) == text::IN_A_CALL_SOUND_OFF
    });
    press(&mut window, text::TURN_SOUND_ON);
    window.until("the microphone is open again", |window| {
        !window.view().controls.sound_off && microphone(window) == MicrophoneButton::Open
    });

    // A muted microphone stays muted after the sound comes back.
    press(&mut window, text::MUTE_MICROPHONE);
    window.until("the microphone is muted", |window| {
        microphone(window) == MicrophoneButton::Muted
    });
    press(&mut window, text::TURN_SOUND_OFF);
    window.until("the sound is off", |window| {
        window.view().controls.sound_off
    });
    press(&mut window, text::TURN_SOUND_ON);
    window.until("the sound is on", |window| {
        !window.view().controls.sound_off
    });
    window.settle();
    assert_eq!(microphone(&mut window), MicrophoneButton::Muted);

    // Unmuting while the sound is off turns the sound on, with the key as with the button.
    press(&mut window, text::UNMUTE_MICROPHONE);
    window.until("the microphone is open", |window| {
        microphone(window) == MicrophoneButton::Open
    });
    press(&mut window, text::TURN_SOUND_OFF);
    window.until("the sound off muted the microphone", |window| {
        window.view().controls.sound_off && microphone(window) == MicrophoneButton::Muted
    });
    window.state.update(&mut window.cx, |state, cx| {
        assert!(state.perform_call_action(KeyAction::ToggleMute, cx));
    });
    window.until(
        "the key opened the microphone and turned the sound on",
        |window| !window.view().controls.sound_off && microphone(window) == MicrophoneButton::Open,
    );

    // Push to mute held, then the sound off: the release keeps the microphone closed, and
    // the sound on opens it.
    window.state.update(&mut window.cx, |state, _| {
        state.hold_action(None, KeyAction::PushToMute, true);
    });
    window.until("the held key closed the microphone", |window| {
        microphone(window) == MicrophoneButton::Muted
    });
    press(&mut window, text::TURN_SOUND_OFF);
    window.until("the sound is off", |window| {
        window.view().controls.sound_off
    });
    window.state.update(&mut window.cx, |state, _| {
        state.hold_action(None, KeyAction::PushToMute, false);
    });
    window.settle();
    assert_eq!(microphone(&mut window), MicrophoneButton::Muted);
    press(&mut window, text::TURN_SOUND_ON);
    window.until("the sound on opened the microphone", |window| {
        !window.view().controls.sound_off && microphone(window) == MicrophoneButton::Open
    });

    // The sound off and on while the key is still held: the microphone stays closed until
    // the release.
    window.state.update(&mut window.cx, |state, _| {
        state.hold_action(None, KeyAction::PushToMute, true);
    });
    window.until("the held key closed the microphone", |window| {
        microphone(window) == MicrophoneButton::Muted
    });
    press(&mut window, text::TURN_SOUND_OFF);
    window.until("the sound is off", |window| {
        window.view().controls.sound_off
    });
    press(&mut window, text::TURN_SOUND_ON);
    window.until("the sound is on", |window| {
        !window.view().controls.sound_off
    });
    window.settle();
    assert_eq!(
        microphone(&mut window),
        MicrophoneButton::Muted,
        "the sound on opened the microphone while the key was held"
    );
    window.state.update(&mut window.cx, |state, _| {
        state.hold_action(None, KeyAction::PushToMute, false);
    });
    window.until("the release opened the microphone", |window| {
        microphone(window) == MicrophoneButton::Open
    });

    // The setting off: the sound off keeps the microphone, and the tray shows both apart.
    press(&mut window, text::MUTE_MICROPHONE);
    press(&mut window, text::TURN_SOUND_OFF);
    window.until("the sound off of both", |window| {
        window.view().controls.sound_off && tooltip(window) == text::IN_A_CALL_SOUND_OFF
    });
    window.state.update(&mut window.cx, |state, cx| {
        state.set_sound_off_mutes(false, cx)
    });
    // The tray is told the setting at once.
    window.settle();
    assert_eq!(tooltip(&mut window), text::IN_A_CALL_MUTED_SOUND_OFF);
    press(&mut window, text::TURN_SOUND_ON);
    press(&mut window, text::UNMUTE_MICROPHONE);
    window.until("the microphone is open", |window| {
        microphone(window) == MicrophoneButton::Open
    });
    press(&mut window, text::TURN_SOUND_OFF);
    window.until("the sound is off", |window| {
        window.view().controls.sound_off
    });
    window.settle();
    assert_eq!(microphone(&mut window), MicrophoneButton::Open);
    press(&mut window, text::MUTE_MICROPHONE);
    window.until("the tray names both", |window| {
        tooltip(window) == text::IN_A_CALL_MUTED_SOUND_OFF
    });
    press(&mut window, text::UNMUTE_MICROPHONE);
    window.until("the microphone is open", |window| {
        microphone(window) == MicrophoneButton::Open
    });
    assert!(window.view().controls.sound_off, "the sound stays off");
    press(&mut window, text::LEAVE_THE_CALL);
}
