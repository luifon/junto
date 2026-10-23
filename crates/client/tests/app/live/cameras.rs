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

/// ADR 0788: the camera control does what it showed when it was drawn. A control that
/// showed a running camera stops, and never starts a camera in another call.
#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_rendered_camera_stop_never_starts_a_replacement_call(cx: &mut TestAppContext) {
    use junto_client::state::CameraAction;

    let world = World::new();
    let server = world.server_with_media("Camera and move");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    let workspace_id = window.view().workspace().unwrap().id;
    let daily = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Channel {
            workspace_id,
            channel_id: room(&mut window).id,
        },
    };
    let focus = second_room(&world, &admin, &mut window, "Focus");
    enter(&world, &caller, focus);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    let camera = |calls: &junto_media::Calls| calls.view().call.unwrap().camera;
    let drawn = |window: &Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.camera_action())
            .unwrap()
    };
    // The camera control as it is drawn: the window does no other work before the press.
    let used = |window: &mut Window, _: &CameraAction| window.press_as_drawn("call-camera");
    let camera_runs = |window: &mut Window| {
        window.click("call-camera");
        window.until("camera running", |_| camera(&calls) == CameraState::Running);
    };
    let off = |view: &junto_media::calls::CallsView| {
        view.call
            .as_ref()
            .is_some_and(|call| call.camera == CameraState::Off)
    };

    // The camera runs in the first call: the control that is drawn stops it. A moderator
    // moves the person, and the person uses that control.
    camera_runs(&mut window);
    let stop = drawn(&window);
    assert_eq!(stop, CameraAction::Stop);
    moved_without_a_drawing(&world, &admin, &window, &daily, &focus);
    used(&mut window, &stop);
    stays(
        &mut window,
        "the camera is off in the call of the destination",
        off,
    );
    assert!(caller.calls.view().call.unwrap().cameras.is_empty());
    assert!(!window.shows("call-told"));

    // The camera runs, and another window stops it before this one is drawn again.
    camera_runs(&mut window);
    let stop = drawn(&window);
    calls.stop_camera();
    let deadline = std::time::Instant::now() + PATIENCE;
    while camera(&calls) != CameraState::Off {
        assert!(std::time::Instant::now() < deadline, "the camera runs");
        std::thread::sleep(Duration::from_millis(20));
    }
    used(&mut window, &stop);
    stays(&mut window, "a stop does not start the camera", off);

    // The control that is drawn starts the camera in this call. After a move it starts
    // nothing in the other call, and the person is told.
    let start = drawn(&window);
    assert!(matches!(start, CameraAction::Start(_)));
    moved_without_a_drawing(&world, &admin, &window, &focus, &daily);
    used(&mut window, &start);
    window.until("the person is told", |window| {
        window.shows("call-told") || camera(&calls) != CameraState::Off
    });
    stays(&mut window, "the camera is off in the other call", off);
    assert!(window.shows("call-told"));
    window.click("leave-call");
    world.run(caller.calls.leave());
}

/// The control of the focus: in the grid every tile has one size; with the focus the camera
/// of the person who speaks is drawn large, and the focus follows the person who speaks.
#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn the_focus_draws_the_camera_of_the_person_who_speaks_large_and_the_grid_returns(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server_with_media("Camera focus");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    // A steady tone is taken as speech only without the processing of a voice.
    turn_processing_off(&mut window);
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
    window.until("connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    let ada = admin.handle.account().unwrap();
    let own = window.view().person.account_id.unwrap();
    let speaking = |window: &mut Window, account| {
        occupants(window).into_iter().any(|occupant| {
            occupant.account_id == account
                && matches!(occupant.microphone, Microphone::Open { speaking: true })
        })
    };
    let width = |window: &mut Window, account| {
        window.settle();
        window
            .cx
            .debug_bounds(crate::harness::selector(format!("call-tile:{account}")))
            .map(|bounds| f32::from(bounds.size.width))
    };

    // Both cameras run, and only Ada speaks.
    window.click("microphone");
    window.click("call-camera");
    caller.calls.start_camera(None);
    window.until("both camera pictures are drawn", |window| {
        window
            .shell
            .read_with(&window.cx, |shell, _| shell.drawn_camera_count() == 2)
    });
    window.until("Ada speaks and the window does not", |window| {
        speaking(window, ada) && !speaking(window, own)
    });
    // The grid: both tiles have one size.
    assert!(width(&mut window, ada).is_some());
    assert_eq!(width(&mut window, ada), width(&mut window, own));

    // The focus draws the camera of Ada large.
    window.click("camera-focus");
    window.until("the camera of Ada is focused", |window| {
        width(window, ada) > width(window, own)
    });
    // The person of the window speaks, and Ada does not: the focus follows.
    caller.calls.set_muted(true);
    window.click("microphone");
    window.until("the window speaks and Ada does not", |window| {
        speaking(window, own) && !speaking(window, ada)
    });
    window.until("the own camera is focused", |window| {
        width(window, own) > width(window, ada)
    });

    // The grid again.
    window.click("camera-focus");
    window.until("both tiles have one size", |window| {
        width(window, own).is_some() && width(window, own) == width(window, ada)
    });
    window.click("leave-call");
    world.run(caller.calls.leave());
}
