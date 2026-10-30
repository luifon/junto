use super::*;

fn pinned(window: &mut Window) -> Option<junto_client_core::protocol::ids::AccountId> {
    window
        .state
        .read_with(&window.cx, |state, _| state.pinned_call_person())
}
fn pin(window: &mut Window, account: junto_client_core::protocol::ids::AccountId) {
    window.right_click_named(format!("call-tile:{account}"));
    assert!(
        window.shows("call-person-pin"),
        "the participant menu offers a local pin"
    );
    window.click("call-person-pin");
    window.until("the chosen person fills the stage", |window| {
        pinned(window) == Some(account) && window.shows_named(format!("person-stage:{account}"))
    });
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_person_can_be_pinned_to_the_stage(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Harbor");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
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
    let other = admin.handle.account().unwrap();
    let own = window.own_account();
    window.until("the other person has a tile", |window| {
        window.shows_named(format!("call-tile:{other}"))
    });
    pin(&mut window, other);
    let primary = window
        .cx
        .debug_bounds(crate::harness::selector(format!("call-tile:{other}")))
        .unwrap();
    let secondary = window
        .cx
        .debug_bounds(crate::harness::selector(format!("call-tile:{own}")))
        .unwrap();
    assert!(
        primary.size.width > secondary.size.width && primary.size.height > secondary.size.height,
        "the pinned person is larger than the participant strip"
    );
    caller.calls.mute();
    window.until("the pinned person stops speaking", |window| {
        occupants(window).iter().any(|person| {
            person.account_id == other && matches!(person.microphone, Microphone::Closed)
        })
    });
    assert_eq!(pinned(&mut window), Some(other));
    caller.calls.start_camera(None);
    window.until("the pinned camera starts", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .media_calls()
                .call
                .is_some_and(|call| call.cameras.contains(&other))
        })
    });
    caller.calls.stop_camera();
    window.until("pin remains as an avatar", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .media_calls()
                .call
                .is_some_and(|call| !call.cameras.contains(&other))
        }) && window.shows_named(format!("person-stage:{other}"))
    });
    caller.calls.start_share(CaptureRequest {
        picture: PictureSource::Display(1),
        sound: SoundChoice::None,
        quality: ShareQuality::default(),
        dialog_parent: None,
    });
    window.until(
        "a share is available while the person stays on stage",
        |window| {
            stage(window).is_some_and(|stage| stage.shares.len() == 1)
                && window.shows_named(format!("person-stage:{other}"))
        },
    );
    let share = stage(&mut window).unwrap().shares[0].id;
    window
        .state
        .update(&mut window.cx, |state, _| state.watch_share(share));
    window.until("the share selection is held", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .media_calls()
                .call
                .is_some_and(|call| call.watched == Some(share))
        })
    });
    window.click("pinned-stage-toggle");
    window.until("screen view remains available", |window| {
        window.shows("share-viewport")
    });
    assert_eq!(pinned(&mut window), Some(other));
    window.click("pinned-stage-toggle");
    window.until("return to the pinned person", |window| {
        window.shows_named(format!("person-stage:{other}"))
    });
    assert!(window.state.read_with(
        &window.cx,
        |state, _| state.media_calls().call.unwrap().watched == Some(share)
    ));
    window.click("detach-call");
    let mut call = window.call_window();
    call.until("the detached window keeps the pin", |call| {
        call.shows_named(format!("person-stage:{other}")) && pinned(call) == Some(other)
    });
    call.right_click_named(format!("call-tile:{other}"));
    call.click("call-person-unpin");
    call.until("unpin restores the share stage", |call| {
        pinned(call).is_none() && call.shows("share-viewport")
    });
    assert!(call.state.read_with(
        &call.cx,
        |state, _| state.media_calls().call.unwrap().watched == Some(share)
    ));
    call.until("the watched picture is ready for its menu", |call| {
        call.shows("share-picture")
            && stage(call)
                .is_some_and(|stage| stage.watched().is_some_and(|watched| watched.id == share))
    });
    assert!(
        call.in_shell(|shell, window, cx| gpui::Focusable::focus_handle(shell, cx)
            .contains_focused(window, cx)),
        "unpin keeps usable focus in the call window"
    );
    call.right_click_named("share-viewport".to_owned());
    assert!(
        call.shows("account-menu"),
        "the watched share opens an account menu"
    );
    assert!(
        call.shows_named(format!("account-copy-id:{other}")),
        "the watched share belongs to its participant"
    );
    call.click("call-person-pin");
    call.until(
        "pinning from the watched share selects its person",
        |call| pinned(call) == Some(other) && call.shows_named(format!("person-stage:{other}")),
    );
    call.click("unpin-call-person");
    call.until("banner unpin restores the same watched share", |call| {
        pinned(call).is_none() && call.shows("share-viewport")
    });
    caller.calls.stop_share(None);
    call.until("people stage after the share", |call| {
        call.shows_named(format!("call-tile:{other}")) && !call.shows("share-viewport")
    });
    pin(&mut call, other);
    world.run(caller.calls.leave());
    call.until("departure clears the pin", |call| pinned(call).is_none());
    call.click("return-call");
    drop(call);
    let (owner, old_seat, calls) = window.state.read_with(&window.cx, |state, _| {
        (
            state.server(admin.handle.id()).unwrap().owner().unwrap(),
            state.media_calls().call.unwrap().seat(),
            state.calls().clone(),
        )
    });
    world.run(calls.leave());
    window.until("call ended", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.media_calls().call.is_none())
            && window.shows("join-call")
    });
    window.click("join-call");
    window.until("new seat can pin self", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.media_calls().call.is_some_and(|call| {
                call.phase == Phase::Connected
                    && state.can_pin_call_person(owner, &call.seat(), own)
            })
        })
    });
    assert!(!window.state.update(&mut window.cx, |state, cx| {
        state.pin_call_person(owner, &old_seat, own, true, cx)
    }));
    let seat = window.state.read_with(&window.cx, |state, _| {
        state.media_calls().call.unwrap().seat()
    });
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.pin_call_person(owner, &seat, own, true, cx)
    }));
    let mut obsolete = owner;
    obsolete.account_id = other;
    assert!(!window.state.update(&mut window.cx, |state, cx| {
        state.pin_call_person(obsolete, &seat, own, false, cx)
    }));
    assert_eq!(pinned(&mut window), Some(own));
    window.click("unpin-call-person");
    window.until("the banner unpins self", |window| pinned(window).is_none());
    world.run(calls.leave());
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_hidden_audio_only_person_can_be_pinned_from_the_member_list(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Harbor");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
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
    let other = admin.handle.account().unwrap();
    window.until("connected", |window| {
        window.shows_named(format!("call-tile:{other}"))
    });
    window.state.update(&mut window.cx, |state, cx| {
        state.set_hide_without_video(true, cx)
    });
    window.until("audio-only tiles are hidden", |window| {
        !window.shows_named(format!("call-tile:{other}"))
    });
    window.click("call-tab-members");
    window.right_click_named(format!("call-member:{other}"));
    window.click("call-person-pin");
    window.until("the selected audio-only person is visible", |window| {
        window.shows_named(format!("person-stage:{other}")) && pinned(window) == Some(other)
    });
    window.right_click_named(format!("call-tile:{other}"));
    assert!(window.shows("call-person-unpin"));
    world.run(caller.calls.leave());
    window.until("the stale person menu closes on departure", |window| {
        !window.shows("account-menu") && pinned(window).is_none()
    });
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    world.run(calls.leave());
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_group_call_pin_is_withdrawn_when_its_person_leaves_the_group(cx: &mut TestAppContext) {
    use crate::admin_pages::{invite, joined};
    let world = World::new();
    let server = world.server_with_media("Harbor");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_third_device, third) = joined(&world, &server, &code, "Third");
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    let group = world
        .run(admin.handle.api().create_group(
            "Project".into(),
            vec![window.own_account(), third.account().unwrap()],
        ))
        .unwrap();
    window.until("group row", |window| {
        window.shows_named(format!("direct-row:{}", group.id))
    });
    window.click_named(format!("direct-row:{}", group.id));
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Group {
            conversation_id: group.id,
        },
    };
    world.run(caller.calls.join(target)).unwrap();
    window.until("group invitation", |window| window.shows("invitation-join"));
    window.click("invitation-join");
    let other = admin.handle.account().unwrap();
    window.until("group person tile", |window| {
        window.shows_named(format!("call-tile:{other}"))
    });
    pin(&mut window, other);
    world.run(admin.handle.api().leave_group(group.id)).unwrap();
    window.until("leaving the group withdraws its stage pin", |window| {
        pinned(window).is_none() && !window.shows_named(format!("call-tile:{other}"))
    });
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    world.run(calls.leave());
    world.run(caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_pair_call_can_pin_self_and_clears_the_pin_when_the_call_ends(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Harbor");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    let own = window.own_account();
    let other = admin.handle.account().unwrap();
    let pair = world
        .run(admin.handle.api().open_conversation(own))
        .unwrap();
    window.until("pair row", |window| {
        window.shows_named(format!("direct-row:{}", pair.id))
    });
    window.click_named(format!("direct-row:{}", pair.id));
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::direct(own, other),
    };
    world.run(caller.calls.join(target)).unwrap();
    window.until("pair invitation", |window| {
        window.shows("direct-call-accept")
    });
    window.click("direct-call-accept");
    window.until("pair person tile", |window| {
        window.shows_named(format!("call-tile:{other}"))
    });
    pin(&mut window, other);
    window.right_click_named(format!("call-tile:{other}"));
    window.click("call-person-unpin");
    window.until("unpin from the person menu", |window| {
        pinned(window).is_none()
    });
    pin(&mut window, own);
    world.run(caller.calls.leave());
    window.until("ending the pair call clears the pin", |window| {
        pinned(window).is_none()
            && window
                .state
                .read_with(&window.cx, |state, _| state.media_calls().call.is_none())
    });
}
