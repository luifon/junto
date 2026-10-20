use super::*;
use junto_client::shell::Pane;

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
    // The choice "keep on top" is drawn only where the system can keep a window above the
    // others (ADR 0507).
    if detached.update(|_, cx| junto_client::window_level::offered(cx)) {
        let top = detached.debug_bounds("call-on-top").unwrap();
        detached.simulate_click(top.center(), Modifiers::none());
        window.until("topmost changes without leaving", |window| {
            window.state.read_with(&window.cx, |state, _| {
                state.is_call_detached() && state.call_window_settings().keep_on_top
            })
        });
    } else {
        assert!(detached.debug_bounds("call-on-top").is_none());
    }
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
    // The button of the dialog as it is drawn: the window does no other work before.
    window.press_as_drawn("start-share");
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

/// The call window tells what the main window tells about the call: the person can use
/// the call window alone (ADR 0772).
#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn the_detached_call_draws_the_notices_and_the_bars_of_its_call(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Detached notices");
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
    let member_id = window.view().person.account_id.unwrap();
    let main = window.cx.update(|window, _| window.window_handle());
    window.click("detach-call");
    let handle = cx
        .windows()
        .into_iter()
        .find(|handle| *handle != main)
        .unwrap();
    let mut detached = gpui::VisualTestContext::from_window(handle, cx);
    let mut shown = |window: &mut Window, what: &str, selector: &'static str| {
        window.until(what, |window| {
            window.settle();
            detached.debug_bounds(selector).is_some()
        });
    };
    let api = admin.handle.api();

    // A moderator blocks the microphone: the call window tells who holds it.
    world
        .run(api.block_microphone(voice.id, member_id))
        .unwrap();
    shown(&mut window, "the warning of the block", "sound-warning");
    // The block ends, and the people hear the person again: the call window tells it.
    world
        .run(api.unblock_microphone(voice.id, member_id))
        .unwrap();
    shown(
        &mut window,
        "the notice of the open microphone",
        "call-told",
    );

    // The administrator shares and runs meeting mode: the call window has the bar.
    caller.calls.start_share(CaptureRequest {
        picture: PictureSource::Display(1),
        sound: SoundChoice::None,
        quality: ShareQuality::default(),
    });
    let started = std::time::Instant::now();
    while let Err(error) = world.run(api.start_meeting(voice.id)) {
        assert!(
            started.elapsed() < PATIENCE,
            "meeting mode did not start: {error}"
        );
        window.settle();
        std::thread::sleep(Duration::from_millis(200));
    }
    shown(&mut window, "the bar of meeting mode", "meeting-bar");
    // The presenter lets the person speak: the statement and its control are there.
    world
        .run(api.unblock_microphone(voice.id, member_id))
        .unwrap();
    shown(&mut window, "the person may speak", "may-speak-unmute");
    world.run(caller.calls.leave());
    window.click("leave-call");
}

/// The people of the call that a window draws in the panel of its call.
fn panel_people(window: &mut Window) -> Vec<Occupant> {
    window
        .view()
        .workspace()
        .and_then(|workspace| workspace.call.clone())
        .map(|panel| panel.people)
        .unwrap_or_default()
}

/// The call window has the chat, the members and the settings of its call, so that a
/// person can use it alone (ADR 0772). The text that its composer holds goes to the main
/// window when it closes.
#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn the_call_window_has_the_chat_the_members_and_the_settings_of_its_call(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Call window panel");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    turn_processing_off(&mut window);
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
    let ada = admin.handle.account().unwrap();
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    window.click("detach-call");
    let mut call = window.call_window();

    // The panel starts closed. The buttons of the header open its tabs.
    assert!(!call.shows("call-panel"));
    call.click("open-call-chat");
    call.until("the chat of the call is shown", |call| {
        call.shows("call-panel") && call.shows("call-chat")
    });
    admin.send(
        &world,
        voice.id,
        crate::messages::document("Hello from Ada"),
    );
    call.until_message("Hello from Ada");
    call.write("Hello from the call window");
    call.until_message("Hello from the call window");

    // The volume of Ada.
    call.click("call-tab-members");
    let slider = crate::harness::selector(format!("volume:{ada}"));
    call.until("the volume control is drawn", |call| call.shows(slider));
    call.click(slider);
    call.press(&["left"; 20].join(" "));
    call.until("Ada is not played", |call| {
        panel_people(call)
            .iter()
            .any(|person| person.account_id == ada && person.volume == 0)
            && heard(&call.sound, ADMIN_TONE_HZ) < ABSENT_DBFS
    });

    // The devices.
    call.click("call-tab-settings");
    call.until("the devices are listed", |call| call.shows("input-device"));
    call.click("input-device");
    call.click("device-choice-1");
    call.until("the microphone is chosen", |call| {
        call.view()
            .workspace()
            .and_then(|workspace| workspace.call.clone())
            .is_some_and(|panel| panel.audio.inputs[1].selected)
    });

    // The panel closes with its button, which is inside the window.
    let close = call.cx.debug_bounds("close-call-panel").unwrap();
    let width = call.cx.update(|window, _| window.viewport_size().width);
    assert!(close.right() <= width, "{close:?} beyond {width:?}");
    call.click("close-call-panel");
    assert!(!call.shows("call-panel"));

    // Text that the composer of the call window holds is in the composer of the main
    // window after the call window closed.
    call.click("open-call-chat");
    call.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "Not sent yet", window, cx);
    });
    call.click("return-call");
    window.until("the call is back in the main window", |window| {
        !window
            .state
            .read_with(&window.cx, |state, _| state.is_call_detached())
    });
    window.until("the text is in the main window", |window| {
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)) == "Not sent yet"
    });
    window.click("leave-call");
    world.run(caller.calls.leave());
}

/// The chat of a direct-message call in the call window is the conversation of its two
/// accounts.
#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn the_call_window_of_a_direct_call_has_its_conversation(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Direct call window");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let server_id = admin.handle.id();
    let other = admin.handle.account().unwrap();
    let own = window.view().person.account_id.unwrap();
    window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation(server_id, other, cx)
    });
    window.until("conversation shown", |window| {
        window.shows("start-direct-call")
    });
    window.click("start-direct-call");
    window.until("the other account receives a ring", |_| {
        admin
            .handle
            .live()
            .view()
            .model
            .is_some_and(|model| model.incoming_invitations().next().is_some())
    });
    let invitation = admin
        .handle
        .live()
        .view()
        .model
        .unwrap()
        .incoming_invitations()
        .next()
        .unwrap()
        .clone();
    world
        .run(caller.calls.join_invited(
            CallTarget {
                server_id,
                place: CallPlace::direct(own, other),
            },
            invitation.id,
        ))
        .unwrap();
    window.until("the other person is in the call", |window| {
        window.shows("active-direct-call")
    });
    window.click("detach-call");
    window.until("the main window shows the separate call", |window| {
        window.shows("call-in-separate-window") && window.shows("call-panel")
    });
    let mut call = window.call_window();
    call.click("open-call-chat");
    call.until("the conversation is the chat", |call| {
        call.shows("call-chat")
            && call
                .view()
                .workspace()
                .and_then(|workspace| workspace.conversation.as_ref().map(|row| row.account_id))
                == Some(other)
    });
    call.write("From the call window");
    call.until_message("From the call window");
    call.click("call-tab-members");
    call.until("the volume of the other person is drawn", |call| {
        call.shows_named(format!("volume:{other}"))
    });
    call.click("leave-call");
    window.until("the call has ended", |window| window.view().call.is_none());
    world.run(caller.calls.leave());
}
