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
