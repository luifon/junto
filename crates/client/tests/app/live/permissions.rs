//! The permissions of a voice channel during a call in the window (ADR 0790): a source
//! that a rule takes stops on this device and its control is not available; a rule that
//! is removed gives it back. The rules change through the API while the call runs.

use super::*;
use junto_client::shell::Open;
use junto_media::camera::CameraState;

use crate::channel_use::{everyone, rule};

fn camera(window: &mut Window) -> Option<CameraState> {
    window.state.read_with(&window.cx, |state, _| {
        state.calls().view().call.map(|call| call.camera)
    })
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn the_sources_of_a_call_follow_the_rules_of_the_voice_channel(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, ROOM);
    turn_processing_off(&mut window);
    let workspace_id = window.view().workspace().unwrap().id;
    let role = everyone(&world, &admin, workspace_id);
    let voice = room(&mut window);
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Channel {
            workspace_id,
            channel_id: voice.id,
        },
    };
    enter(&world, &caller, target);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("the call is connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    until_hears(
        &world,
        &mut window,
        &caller,
        Hears::Administrator,
        "the join",
    );
    window.click("call-camera");
    window.until("the camera runs", |window| {
        camera(window) == Some(CameraState::Running)
    });

    // "speak" and "video" are taken: the microphone is closed with the reason, and the
    // camera stops. "set call user limit" is given: the limit takes a change.
    rule(
        &world,
        &admin,
        voice.id,
        role,
        &[WorkspacePermission::SetCallUserLimit],
        &[WorkspacePermission::Speak, WorkspacePermission::Video],
    );
    window.until("the camera stops and is not available", |window| {
        camera(window) == Some(CameraState::Off)
            && window.shows("call-camera-refused")
            && !window.shows("call-camera")
    });
    window.until("the administrator hears nothing", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS
    });
    assert!(window.shows("sound-warning"));
    window.in_shell(|shell, window, cx| shell.open(Open::RoomSettings, window, cx));
    window.click("room-limit-raise");
    window.until("the limit is set without manage channel", |window| {
        window
            .view()
            .lists()
            .and_then(|lists| lists.call.as_ref())
            .and_then(|call| call.room().map(|room| room.user_limit))
            == Some(Some(2))
    });

    // "speak" again, without "use voice activity": the microphone sends only while the
    // key is held. "share screen" is taken.
    rule(
        &world,
        &admin,
        voice.id,
        role,
        &[],
        &[
            WorkspacePermission::UseVoiceActivity,
            WorkspacePermission::ShareScreen,
        ],
    );
    window.until("the share is not available", |window| {
        window.shows("call-share-refused") && !window.shows("call-share")
    });
    window.until("the strip states push-to-talk", |window| {
        window.shows("input-mode-mark")
    });
    std::thread::sleep(Duration::from_millis(600));
    assert!(heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS);
    let held = Modifiers {
        alt: true,
        ..Modifiers::none()
    };
    window.simulate(|cx| cx.simulate_modifiers_change(held));
    window.until("the key sends the microphone", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });
    window.simulate(|cx| cx.simulate_modifiers_change(Modifiers::none()));
    window.until("the release ends it", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS
    });

    // The rule is removed: every control returns, and voice activation sends again.
    rule(&world, &admin, voice.id, role, &[], &[]);
    window.until("the controls return", |window| {
        window.shows("call-share") && window.shows("call-camera") && !window.shows("sound-warning")
    });
    until_hears(
        &world,
        &mut window,
        &caller,
        Hears::Administrator,
        "the rule",
    );

    // A share that runs stops when "share screen" is taken.
    window.click("call-share");
    window.until("the screens are listed", |window| {
        window.shows("share-sources")
    });
    window.click("share-display:1");
    window.click("share-sound-0");
    window.click("start-share");
    window.until("the share runs", |window| {
        own_share(window) == OwnShare::Running
    });
    rule(
        &world,
        &admin,
        voice.id,
        role,
        &[],
        &[WorkspacePermission::ShareScreen],
    );
    window.until("the share stops", |window| {
        own_share(window) == OwnShare::Off && window.shows("call-share-refused")
    });
    window.click("leave-call");
    window.until("the call has ended", |window| window.view().call.is_none());
    world.run(caller.calls.leave());
}
