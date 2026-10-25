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

fn tooltip(window: &mut Window) -> &'static str {
    window
        .state
        .read_with(&window.cx, |state, _| state.kept_tray().shown().tooltip())
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
