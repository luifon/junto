use super::*;

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn stopping_a_share_returns_to_people_and_either_window_can_watch_again(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Share controls");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    turn_processing_off(&mut window);
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
    window.until("the call connects", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    caller.calls.start_share(CaptureRequest {
        picture: PictureSource::Display(1),
        sound: SoundChoice::System,
        quality: ShareQuality::default(),
        dialog_parent: None,
    });
    window.until("the share is watched and heard", |window| {
        window.shows("share-picture") && heard(&window.sound, SYSTEM_TONE_HZ) > SHARE_HEARD_DBFS
    });
    window.click("stop-watching");
    window.until(
        "the stage returns to people without share sound",
        |window| {
            window.shows("call-stage")
                && window.shows("shares-bar")
                && !window.shows("share-picture")
                && heard(&window.sound, SYSTEM_TONE_HZ) < ABSENT_DBFS
        },
    );
    let own = window.own_account();
    window.until("the server no longer lists the viewer", |_| {
        admin.handle.live().view().model.is_some_and(|model| {
            model
                .call_at(&target.place)
                .and_then(|call| call.participant(own))
                .is_some_and(|person| person.watching.is_none())
        })
    });
    let watch = format!("share-thumbnail:{}:0", admin.handle.account().unwrap());
    window.click_named(watch.clone());
    window.until("Watch resumes the picture and sound", |window| {
        window.shows("share-picture") && heard(&window.sound, SYSTEM_TONE_HZ) > SHARE_HEARD_DBFS
    });
    window.click("detach-call");
    let mut detached = window.call_window();
    detached.right_click_named("share-header".to_owned());
    detached.click("call-menu-stop-watching");
    detached.until("the detached window returns to people", |window| {
        window.shows("call-stage") && !window.shows("share-picture")
    });
    detached.click_named(watch.clone());
    detached.until("the detached window watches again", |window| {
        window.shows("share-picture")
    });
    detached.state.update(&mut detached.cx, |state, cx| {
        state
            .change_keybinds(
                junto_client::state::KeybindChange::Set(
                    junto_client_core::keybinds::KeyAction::StopWatching,
                    Some("control+shift+x".parse().unwrap()),
                ),
                cx,
            )
            .unwrap();
    });
    detached.press("ctrl-shift-x");
    detached.until("the assigned key stops watching", |window| {
        window.shows("call-stage") && !window.shows("share-picture")
    });
    detached.click_named(watch);
    detached.until("Watch still resumes after the assigned key", |window| {
        window.shows("share-picture")
    });
    detached.click("return-call");
    drop(detached);
    window.click("leave-call");
    world.run(caller.calls.leave());
}

pub(super) fn share_sound_keyboard_paths(window: &mut Window) {
    let panel_was_open = window.shows("close-call-panel");
    for key in ["enter", "space"] {
        window.tab_to("share-mute-tile");
        window.press(key);
        window.until(
            "the tile key mutes the share and keeps the picture",
            |window| {
                window.shows("share-picture") && heard(&window.sound, PLAYER_TONE_HZ) < ABSENT_DBFS
            },
        );
        assert!(!window.shows("account-menu"));
        let members = if window.shows("open-call-members") {
            "open-call-members"
        } else {
            "call-tab-members"
        };
        window.click(members);
        window.tab_to("member-share-sound");
        window.press(key);
        window.until("the Members key restores share sound", |window| {
            heard(&window.sound, PLAYER_TONE_HZ) > SHARE_HEARD_DBFS
        });
        assert!(!window.shows("account-menu"));
    }
    window.click("close-call-panel");
    let share = stage(window).unwrap().shares[0].clone();
    let before = share.sound_setting.volume;
    let server = window.view().call.unwrap().server_id;
    let microphone = window.state.read_with(&window.cx, |state, _| {
        state.voice().volume(server, share.account_id)
    });
    window.tab_to("share-mute-header");
    window.press("shift-f10");
    window.tab_to("call-menu-volume");
    window.press("left");
    window.until("the share volume responds to its menu key", |window| {
        stage(window).is_some_and(|stage| stage.shares[0].sound_setting.volume == before - 10)
    });
    window.state.read_with(&window.cx, |state, _| {
        assert_eq!(state.voice().volume(server, share.account_id), microphone);
        assert_eq!(
            state.voice().share_sound(server, share.account_id).volume,
            before - 10
        );
    });
    window.press("right");
    window.until("the reverse key restores only the share volume", |window| {
        stage(window).is_some_and(|stage| stage.shares[0].sound_setting.volume == before)
    });
    window.press("escape");
    assert!(!window.shows("account-menu"));
    if panel_was_open {
        window.click("open-call-members");
    }
}
