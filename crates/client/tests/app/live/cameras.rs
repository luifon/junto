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
    window.simulate(|cx| cx.simulate_keystrokes("escape"));
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
    let mut detached = window.window_of(handle);
    let settings = detached
        .cx
        .debug_bounds("camera-settings")
        .unwrap()
        .center();
    detached.simulate(|cx| cx.simulate_click(settings, Modifiers::none()));
    detached.run_tasks();
    assert!(detached.cx.debug_bounds("settings-dialog").is_some());
    detached.simulate(|cx| cx.simulate_keystrokes("escape"));
    detached.run_tasks();
    let dismiss = detached.cx.debug_bounds("camera-dismiss").unwrap().center();
    detached.simulate(|cx| cx.simulate_click(dismiss, Modifiers::none()));
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
    // The grid: both tiles have one size, side by side, each about half of the stage.
    assert!(width(&mut window, ada).is_some());
    assert_eq!(width(&mut window, ada), width(&mut window, own));
    let stage = window
        .cx
        .debug_bounds(crate::harness::selector("call-stage".to_owned()))
        .map(|bounds| f32::from(bounds.size.width))
        .unwrap();
    let tile = width(&mut window, ada).unwrap();
    assert!(
        tile > stage * 0.4 && tile < stage * 0.5,
        "a tile of {tile} in a stage of {stage}"
    );

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

/// "Hide people without video" leaves out the tiles of the people without a camera in every
/// layout of the call: the grid, the focus, the stage of a share and the call window, also
/// while nobody has a camera, and states how many it leaves out. A camera that starts while
/// the setting is on is drawn.
#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn the_people_without_video_are_hidden_in_every_layout(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Hidden without video");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
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
    window.until("connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    let ada = admin.handle.account().unwrap();
    let own = window.view().person.account_id.unwrap();
    let tile = |window: &mut Window, account| window.shows_named(format!("call-tile:{account}"));
    let says = |window: &mut Window, sentence: &str| {
        window.shows_named(format!("people-hidden-text:{sentence}"))
    };
    let avatar =
        |window: &mut Window, account| window.shows_named(format!("hidden-avatar:{account}"));
    // The tile of the window, without Ada's, with the statement of the hidden person.
    let only_own = |window: &mut Window| {
        tile(window, own)
            && !tile(window, ada)
            && says(window, "1 person without video is hidden.")
            && avatar(window, ada)
            && !avatar(window, own)
    };
    let drawn = |window: &mut Window| {
        window
            .shell
            .read_with(&window.cx, |shell, _| shell.drawn_camera_count())
    };
    window.until("both tiles are drawn", |window| {
        tile(window, ada) && tile(window, own)
    });

    // Nobody has a camera: the setting leaves out both tiles and says so.
    window.click("call-tab-settings");
    window.click("hide-without-video");
    window.until("both tiles are hidden", |window| {
        !tile(window, ada)
            && !tile(window, own)
            && says(window, "2 people without video are hidden.")
            && avatar(window, ada)
            && avatar(window, own)
    });

    // The camera of the window starts: its tile returns.
    window.click("call-camera");
    window.until("the tile of the own camera is drawn", |window| {
        only_own(window) && drawn(window) == 1
    });

    // The camera of Ada starts while the setting is on: her tile and picture are drawn.
    caller.calls.start_camera(None);
    window.until("the tile of the camera of Ada is drawn", |window| {
        tile(window, ada)
            && tile(window, own)
            && !window.shows("people-hidden")
            && drawn(window) == 2
    });
    caller.calls.stop_camera();
    window.until("the tile of Ada leaves with her camera", |window| {
        only_own(window)
    });

    // The focus: the own camera is drawn large, and Ada stays out.
    let width = |window: &mut Window| {
        window.settle();
        window
            .cx
            .debug_bounds(crate::harness::selector(format!("call-tile:{own}")))
            .map(|bounds| f32::from(bounds.size.width))
    };
    // One tile fills the stage in the grid and in the focus.
    let grid = width(&mut window);
    window.click("camera-focus");
    window.until("the focus leaves Ada out", |window| {
        width(window) == grid && only_own(window)
    });
    window.click("camera-focus");
    window.until("the grid returns", |window| width(window) == grid);

    // A share of Ada: the strip under the picture.
    caller.calls.start_share(CaptureRequest {
        picture: PictureSource::Display(1),
        sound: SoundChoice::None,
        quality: ShareQuality::default(),
        dialog_parent: None,
    });
    window.until("the strip of the share leaves Ada out", |window| {
        window.shows("share-viewport") && only_own(window)
    });

    // The call window, with the share and without it.
    window.click("detach-call");
    let mut call = window.call_window();
    call.until("the call window leaves Ada out of the strip", |call| {
        call.shows("share-viewport") && only_own(call)
    });
    caller.calls.stop_share(None);
    call.until("the call window leaves Ada out of the grid", |call| {
        !call.shows("share-viewport") && only_own(call)
    });
    call.click("return-call");
    drop(call);
    window.until("the call returns", |window| {
        !window
            .state
            .read_with(&window.cx, |state, _| state.is_call_detached())
    });

    // The setting is turned off: both tiles return.
    window.click("call-tab-settings");
    window.click("hide-without-video");
    window.until("both tiles return", |window| {
        tile(window, ada) && tile(window, own) && !window.shows("people-hidden")
    });
    window.click("leave-call");
    world.run(caller.calls.leave());
}
