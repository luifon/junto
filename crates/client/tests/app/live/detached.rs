use super::*;

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn detaching_and_returning_keep_the_call(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Detached call");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    let voice = room(&mut window);
    enter(
        &world,
        &caller,
        CallTarget {
            server_id: admin.handle.id(),
            place: CallPlace::Channel {
                workspace_id: window.view().workspace().unwrap().id,
                channel_id: voice.id,
            },
        },
    );
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    let before = window.state.read_with(&window.cx, |state, _| {
        state.calls().view().call.unwrap().identity
    });
    window.click("detach-call");
    window.until("call detached", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.is_call_detached())
    });
    assert!(window.shows("call-in-separate-window"));
    assert!(window.shows("call-panel"));
    let after = window.state.read_with(&window.cx, |state, _| {
        state.calls().view().call.unwrap().identity
    });
    assert_eq!(before, after);
    window.click("return-call");
    window.until("call returned", |window| {
        !window
            .state
            .read_with(&window.cx, |state, _| state.is_call_detached())
    });
    assert_eq!(cx.windows().len(), 1);
    let main = window.cx.update(|window, _| window.window_handle());
    window.click("detach-call");
    let handle = cx
        .windows()
        .into_iter()
        .find(|handle| *handle != main)
        .unwrap();
    let mut detached = gpui::VisualTestContext::from_window(handle, cx);
    detached.run_until_parked();
    let returning = detached.debug_bounds("return-call").unwrap().center();
    detached.simulate_click(returning, Modifiers::none());
    window.until("the detached Return button closes its window", |window| {
        !window
            .state
            .read_with(&window.cx, |state, _| state.is_call_detached())
    });
    assert_eq!(cx.windows().len(), 1);
    assert!(window.shows("call-stage"));
    assert_eq!(
        before,
        window.state.read_with(&window.cx, |state, _| state
            .calls()
            .view()
            .call
            .unwrap()
            .identity)
    );
    window.click("leave-call");
    world.run(caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn closing_detached_window_returns_and_call_end_closes_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Close call window");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    let voice = room(&mut window);
    enter(
        &world,
        &caller,
        CallTarget {
            server_id: admin.handle.id(),
            place: CallPlace::Channel {
                workspace_id: window.view().workspace().unwrap().id,
                channel_id: voice.id,
            },
        },
    );
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    let before = window.state.read_with(&window.cx, |state, _| {
        state.calls().view().call.unwrap().identity
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
    assert!(detached.debug_bounds("leave-call").is_some());
    assert!(detached.simulate_close());
    detached.update(|window, _| window.remove_window());
    window.until("closing returns call", |window| {
        !window
            .state
            .read_with(&window.cx, |state, _| state.is_call_detached())
    });
    assert_eq!(
        before,
        window.state.read_with(&window.cx, |state, _| state
            .calls()
            .view()
            .call
            .unwrap()
            .identity)
    );
    window.click("detach-call");
    window.click("leave-call");
    window.until("ending call closes detached window", |window| {
        !window
            .state
            .read_with(&window.cx, |state, _| state.is_call_detached())
    });
    assert_eq!(cx.windows().len(), 1);
    world.run(caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn push_to_talk_follows_both_windows(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Two window controls");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    turn_processing_off(&mut window);
    window.state.update(&mut window.cx, |state, cx| {
        state.set_input_mode(junto_client_core::voice::InputMode::PushToTalk, cx);
        state.set_talk_key(junto_client_core::voice::TalkKey::Control, cx);
    });
    let voice = room(&mut window);
    enter(
        &world,
        &caller,
        CallTarget {
            server_id: admin.handle.id(),
            place: CallPlace::Channel {
                workspace_id: window.view().workspace().unwrap().id,
                channel_id: voice.id,
            },
        },
    );
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    let main = window.cx.update(|window, _| window.window_handle());
    let held = Modifiers {
        control: true,
        ..Modifiers::none()
    };
    window.cx.simulate_modifiers_change(held);
    window.until("main key sends", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });
    window.cx.simulate_modifiers_change(Modifiers::none());
    window.until("main key releases", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS
    });
    window.click("detach-call");
    let handle = cx
        .windows()
        .into_iter()
        .find(|handle| *handle != main)
        .unwrap();
    let mut detached = gpui::VisualTestContext::from_window(handle, cx);
    detached.update(|window, _| window.activate_window());
    detached.run_until_parked();
    assert!(detached.debug_bounds("detached-microphone").is_some());
    detached.simulate_modifiers_change(held);
    window.until("detached key sends", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });
    detached.deactivate_window();
    window.until("blur releases the key", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) < ABSENT_DBFS
    });
    let identity = window.state.read_with(&window.cx, |state, _| {
        state.calls().view().call.unwrap().identity
    });
    detached.run_until_parked();
    let fullscreen = detached.debug_bounds("call-fullscreen").unwrap();
    detached.simulate_click(fullscreen.center(), Modifiers::none());
    assert!(detached.update(|window, _| window.is_fullscreen()));
    let top = detached.debug_bounds("call-on-top").unwrap();
    detached.simulate_click(top.center(), Modifiers::none());
    window.until("topmost changes without leaving", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.is_call_detached() && state.call_window_settings().keep_on_top
        })
    });
    assert_eq!(
        identity,
        window.state.read_with(&window.cx, |state, _| state
            .calls()
            .view()
            .call
            .unwrap()
            .identity)
    );
    window.click("leave-call");
    world.run(caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn closing_the_main_window_also_closes_its_detached_call(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Main window close");
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
    let handle = cx
        .update(|cx| {
            junto_client::app::open_main_window(
                window.state.clone(),
                junto_client::app::WindowStart {
                    activate: false,
                    ..Default::default()
                },
                cx,
            )
        })
        .unwrap();
    window.cx.update(|window, _| window.remove_window());
    let mut main = gpui::VisualTestContext::from_window(handle.into(), cx);
    main.run_until_parked();
    let detach = main.debug_bounds("detach-call").unwrap().center();
    main.simulate_click(detach, Modifiers::none());
    main.run_until_parked();
    assert_eq!(cx.windows().len(), 2);
    assert!(main.simulate_close());
    main.update(|window, _| window.remove_window());
    cx.run_until_parked();
    assert!(
        cx.windows().is_empty(),
        "no orphaned call window remains after closing the application"
    );
}

/// ADR 0788: what the person chose in the dialog of a share is shared in the call of the
/// dialog and in no other call.
#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_share_choice_cannot_cross_a_call_move(cx: &mut TestAppContext) {
    use junto_media::engine::ShareState;

    let world = World::new();
    let server = world.server_with_media("Share and move");
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
    // The administrator is in the call of the destination and gets what is shared there.
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
    let share_of_the_window = |calls: &junto_media::Calls| calls.view().call.unwrap().share;

    // The person opens the dialog and chooses a screen with the sound of the system.
    let choose = |window: &mut Window| {
        window.click("call-share");
        window.until("the screens and windows are listed", |window| {
            window.shows("share-sources")
        });
        window.click("share-display:1");
        window.click("share-sound-1");
    };
    choose(&mut window);
    // A person with "move members" moves the person, and the person confirms the dialog
    // of the first call.
    moved_without_a_drawing(&world, &admin, &window, &daily, &focus);
    assert!(
        window
            .shell
            .read_with(&window.cx, |shell, _| shell.share_picker_is_open())
    );
    let shell = window.shell.clone();
    window.cx.update(|window, cx| {
        shell.update(cx, |shell, cx| shell.start_chosen_share(window, cx));
    });
    window.until("the confirmed share has its result", |window| {
        window.shows("call-told") || share_of_the_window(&calls) != ShareState::Off
    });
    assert_eq!(
        share_of_the_window(&calls),
        ShareState::Off,
        "the choice of the first call is shared in the call of the destination"
    );
    assert!(window.shows("call-told"));
    assert!(!window.in_shell(|shell, _, _| shell.share_picker_is_open()));
    stays(&mut window, "nothing is shared", |view| {
        view.call
            .as_ref()
            .is_some_and(|call| call.share == ShareState::Off)
    });
    assert!(window.capture.started().is_empty());
    assert!(caller.calls.view().call.unwrap().shares.is_empty());
    window.click("dismiss-call-told");

    // A dialog that is open when the window is drawn after a move is closed, and the
    // person is told.
    choose(&mut window);
    moved_without_a_drawing(&world, &admin, &window, &focus, &daily);
    window.until("the dialog of the other call is closed", |window| {
        !window.in_shell(|shell, _, _| shell.share_picker_is_open()) && window.shows("call-told")
    });
    assert!(window.capture.started().is_empty());
    window.click("dismiss-call-told");

    // In the call that the dialog was opened for, the choice is shared.
    choose(&mut window);
    window.click("start-share");
    window.until("the share runs", |_| {
        matches!(share_of_the_window(&calls), ShareState::Running(_))
    });
    window.click("leave-call");
    world.run(caller.calls.leave());
}
