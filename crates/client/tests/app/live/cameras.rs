use super::*;
use junto_media::camera::CameraState;

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn camera_button_sends_generated_pixels_and_stops(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Camera window");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    let voice = room(&mut window);
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Channel {
            workspace_id: window.view().workspace().unwrap().id,
            channel_id: voice.id,
        },
    };
    enter(&world, &caller, target);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    window.click("call-camera");
    window.until("camera running", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .calls()
                .view()
                .call
                .is_some_and(|call| call.camera == CameraState::Running)
        })
    });
    window.until("peer receives generated camera pixels", |_| {
        caller
            .calls
            .camera_pictures()
            .borrow()
            .values()
            .any(|frame| {
                frame
                    .pixel(frame.width / 4, frame.height / 2)
                    .is_some_and(|(red, _, blue)| red > 150 && blue < 70)
                    && frame
                        .pixel(frame.width * 3 / 4, frame.height / 2)
                        .is_some_and(|(red, _, blue)| blue > 150 && red < 70)
            })
    });
    window.click("call-camera");
    window.until("camera stopped", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .calls()
                .view()
                .call
                .is_some_and(|call| call.camera == CameraState::Off)
        })
    });
    world.run(caller.calls.leave());
    window.until("alone in the call", |window| window.shows("alone-in-call"));
    let stage = window.cx.debug_bounds("call-stage").unwrap();
    let note = window.cx.debug_bounds("alone-in-call").unwrap();
    assert!(
        stage.contains(&note.center()),
        "alone note {note:?} outside stage {stage:?}"
    );
    window.click("leave-call");
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_camera_failure_can_open_settings_or_be_dismissed_in_both_windows(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Camera failure actions");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    window.state.update(&mut window.cx, |state, cx| {
        state.choose_camera(Some("missing-camera".into()), cx)
    });
    window.click("call-camera");
    window.until("failure shown", |window| window.shows("camera-failed"));
    window.click("camera-settings");
    assert!(window.shows("settings-dialog"));
    window.cx.simulate_keystrokes("escape");
    window.click("camera-dismiss");
    window.until("failure dismissed", |window| !window.shows("camera-failed"));
    window.click("call-camera");
    window.until("failure shown again", |window| {
        window.shows("camera-failed")
    });
    let main = window.cx.update(|window, _| window.window_handle());
    window.click("detach-call");
    let handle = cx
        .windows()
        .into_iter()
        .find(|handle| *handle != main)
        .unwrap();
    let mut detached = gpui::VisualTestContext::from_window(handle, cx);
    detached.run_until_parked();
    let settings = detached.debug_bounds("camera-settings").unwrap().center();
    detached.simulate_click(settings, Modifiers::none());
    detached.run_until_parked();
    assert!(detached.debug_bounds("settings-dialog").is_some());
    detached.simulate_keystrokes("escape");
    detached.run_until_parked();
    let dismiss = detached.debug_bounds("camera-dismiss").unwrap().center();
    detached.simulate_click(dismiss, Modifiers::none());
    window.until("detached failure dismissed", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.media_calls().call.unwrap().camera == CameraState::Off
        })
    });
    window.click("leave-call");
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn the_member_list_marks_a_published_camera(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Camera member indicator");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    window.click("call-camera");
    window.until("camera running", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.media_calls().call.unwrap().camera == CameraState::Running
        })
    });
    let own = window.view().person.account_id.unwrap();
    window.click("call-tab-members");
    assert!(window.shows_named(format!("member-camera:{own}")));
    window.click("call-camera");
    window.until("camera indicator disappears", |window| {
        !window.shows_named(format!("member-camera:{own}"))
    });
    window.click("leave-call");
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn only_the_window_drawing_the_call_keeps_camera_pictures(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Camera drawing ownership");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    window.click("call-camera");
    window.until("main window draws the camera", |window| {
        window
            .shell
            .read_with(&window.cx, |shell, _| shell.drawn_camera_count() == 1)
    });
    window.click("detach-call");
    window.until("main window releases its hidden picture", |window| {
        window
            .shell
            .read_with(&window.cx, |shell, _| shell.drawn_camera_count() == 0)
    });
    window.click("return-call");
    window.until("main window resumes drawing", |window| {
        window
            .shell
            .read_with(&window.cx, |shell, _| shell.drawn_camera_count() == 1)
    });
    window.click("leave-call");
}
