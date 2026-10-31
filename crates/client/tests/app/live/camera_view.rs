use super::*;
use junto_media::camera::CameraState;

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn hiding_the_own_camera_keeps_sending_and_is_saved(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Own camera view");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
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
    window.until("the call connects", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    window.click("call-camera");
    window.until("the own camera runs", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .calls()
                .view()
                .call
                .is_some_and(|call| call.camera == CameraState::Running)
        })
    });
    assert!(window.shows("toggle-own-camera-view"));
    let own = window.own_account();
    let tile = format!("call-tile:{own}");
    window.until("the own tile is visible", |window| {
        window.shows_named(tile.clone())
    });
    window.right_click_named(tile.clone());
    window.click("call-person-pin");
    window.until("the own person is pinned", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.pinned_call_person() == Some(own)
        })
    });
    window.tab_to("toggle-own-camera-view");
    window.press("enter");
    window.until("the hidden camera has a mark and no tile", |window| {
        window.shows("own-camera-hidden") && !window.shows_named(tile.clone())
    });
    assert!(
        window
            .state
            .read_with(&window.cx, |state, _| state.pinned_call_person().is_none())
    );
    window.camera.set_colours((30, 30, 200), (200, 30, 30));
    window.until(
        "fresh camera pixels still reach the peer while hidden",
        |_| {
            caller
                .calls
                .camera_pictures()
                .borrow()
                .get(&junto_media::engine::Person::Of(own))
                .is_some_and(|frame| {
                    frame
                        .pixel(frame.width / 4, frame.height / 2)
                        .is_some_and(|(red, _, blue)| blue > 150 && red < 70)
                        && frame
                            .pixel(frame.width * 3 / 4, frame.height / 2)
                            .is_some_and(|(red, _, blue)| red > 150 && blue < 70)
                })
        },
    );
    assert_eq!((window.camera.started(), window.camera.active()), (1, 1));
    let members = if window.shows("open-call-members") {
        "open-call-members"
    } else {
        "call-tab-members"
    };
    window.click(members);
    assert!(window.shows_named(format!("member-camera:{own}")));
    window.click("close-call-panel");
    window.click("show-own-camera");
    window.until("the own tile is restored", |window| {
        window.shows_named(tile.clone())
    });
    window.right_click_named(tile.clone());
    window.click("call-menu-own-camera-view");
    window.until("the context menu hides the tile", |window| {
        !window.shows_named(tile.clone()) && window.shows("own-camera-hidden")
    });
    window.click("detach-call");
    let mut detached = window.call_window();
    assert!(detached.shows("own-camera-hidden") && !detached.shows_named(tile.clone()));
    detached.click("toggle-own-camera-view");
    detached.until("the detached view restores its own tile", |window| {
        window.shows_named(tile.clone())
    });
    detached.right_click_named(tile.clone());
    detached.click("call-menu-own-camera-view");
    detached.until("the detached menu hides the own tile", |window| {
        !window.shows_named(tile.clone()) && window.shows("own-camera-hidden")
    });
    assert_eq!(detached.camera.active(), 1);
    detached.click("return-call");
    drop(detached);
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    calls.start_share(CaptureRequest {
        picture: PictureSource::Display(1),
        sound: SoundChoice::None,
        quality: ShareQuality::default(),
        dialog_parent: None,
    });
    window.until("the share strip also hides the own camera", |window| {
        window.shows("share-header")
            && !window.shows_named(tile.clone())
            && window.shows("own-camera-hidden")
    });
    calls.stop_share(None);
    window.until("the share stops", |window| !window.shows("share-header"));
    window.click("call-camera");
    window.until("an off camera keeps the own avatar", |window| {
        window.shows_named(tile.clone()) && !window.shows("own-camera-hidden")
    });
    window.click("call-camera");
    window.until("the saved view hides a restarted camera", |window| {
        window.shows("own-camera-hidden")
            && !window.shows_named(tile.clone())
            && window.state.read_with(&window.cx, |state, _| {
                state
                    .calls()
                    .view()
                    .call
                    .is_some_and(|call| call.camera == CameraState::Running)
            })
    });
    window.until("the own-camera view preference is saved", |_| {
        std::fs::read_to_string(voice_file(directory.path()))
            .is_ok_and(|stored| stored.contains("\"hide_own_camera\": true"))
    });
    window.click("leave-call");
    window.until("the call ends", |window| window.view().call.is_none());
    window.close();
    let again = crate::harness::open(cx, directory.path());
    assert!(
        again
            .state
            .read_with(&again.cx, |state, _| state.voice().hide_own_camera)
    );
    world.run(caller.calls.leave());
}
