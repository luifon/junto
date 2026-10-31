use super::*;
use junto_client_core::global_keys::{HeldKeys, Key};
use junto_client_core::keybinds::KeyAction;
use junto_client_core::voice::InputMode;

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn local_speech_while_muted_offers_unmute_without_bypassing_held_keys(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Muted speech");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    turn_processing_off(&mut window);
    voice_channel(&world, &admin, &mut window, ROOM);
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Channel {
            workspace_id: window.view().workspace().unwrap().id,
            channel_id: room(&mut window).id,
        },
    };
    enter(&world, &caller, target);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("the microphone is heard", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });
    window.click("microphone");
    window.until(
        "local speech is warned about while nothing is sent",
        |window| window.shows("muted-speech") && heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS,
    );
    window.sound.say("tone", &vec![0.0; 96_000], 48_000);
    window.until("remote speech alone does not warn", |window| {
        !window.shows("muted-speech") && heard(&window.sound, ADMIN_TONE_HZ) > HEARD_DBFS
    });
    window.until("local speech starts the notice again", |window| {
        window.shows("muted-speech")
    });
    window.tab_to("unmute-muted-speech");
    window.press("enter");
    window.until(
        "Unmute opens the microphone and hides the notice",
        |window| !window.shows("muted-speech") && heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS,
    );

    window.state.update(&mut window.cx, |state, cx| {
        state.set_input_mode(InputMode::PushToTalk, cx)
    });
    window.until("push-to-talk without its key sends nothing", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS
    });
    assert!(!window.shows("muted-speech"));
    window.click("microphone");
    window.until("explicit mute still warns with push-to-talk", |window| {
        window.shows("muted-speech")
    });
    window.click("unmute-muted-speech");
    window.until(
        "Unmute clears explicit mute while push-to-talk stays closed",
        |window| {
            !window.shows("muted-speech")
                && !window
                    .state
                    .read_with(&window.cx, |state, _| state.calls().view().muted)
        },
    );
    assert!(heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS);
    window.state.update(&mut window.cx, |state, cx| {
        state.set_input_mode(InputMode::VoiceActivation, cx)
    });
    window.until("voice activity sends again", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });

    let held = HeldKeys::default();
    window.state.update(&mut window.cx, |state, cx| {
        state.install_held_keys(held.clone(), cx);
        state
            .change_keybinds(
                junto_client::state::KeybindChange::Set(
                    KeyAction::PushToMute,
                    Some("f13".parse().unwrap()),
                ),
                cx,
            )
            .unwrap();
    });
    held.hold(Ok(vec![Key::Function(13)]));
    window.until("held push-to-mute warns and sends no sound", |window| {
        window.shows("muted-speech") && heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS
    });
    window.click("unmute-muted-speech");
    assert!(
        window
            .state
            .read_with(&window.cx, |state, _| state.calls().view().muted
                && state.holds_mute())
    );
    held.hold(Ok(Vec::new()));
    window.until("releasing push-to-mute resumes sound", |window| {
        !window.shows("muted-speech") && heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });

    window.click("microphone");
    window.until("the notice returns", |window| window.shows("muted-speech"));
    window.click("detach-call");
    let mut detached = window.call_window();
    assert!(detached.shows("muted-speech"));
    detached.tab_to("unmute-muted-speech");
    detached.press("space");
    detached.until("the detached Unmute works", |window| {
        !window.shows("muted-speech") && heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });
    detached.click("return-call");
    drop(detached);
    window.click("leave-call");
    world.run(caller.calls.leave());
}
