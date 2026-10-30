use super::*;
use junto_media::camera::CameraState;

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_prejoin_check_releases_local_devices_when_the_channel_call_starts(cx: &mut TestAppContext) {
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
    window.click("check-call-devices");
    window.click_in_device_check("preview-camera");
    window.until("local preview", |window| {
        window.shows("camera-preview-picture")
    });
    window.click_in_device_check("test-output");
    window.until("output check runs", |window| {
        window.shows("device-test-playing")
    });
    window.click_in_device_check("prejoin-join");
    window.until("real call owns the devices", |window| {
        window.camera.active() == 0
            && window.state.read_with(&window.cx, |state, _| {
                state.calls().device_check_view() == Default::default()
                    && state.media_calls().call.is_some_and(|call| {
                        call.target == target
                            && call.phase == Phase::Connected
                            && call.camera == CameraState::Off
                    })
            })
    });
    assert!(!window.shows("prejoin-dialog"));
    assert!(caller.calls.camera_pictures().borrow().is_empty());
    world.run(caller.calls.leave());
    window.click("leave-call");
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn an_invitation_check_closes_on_cancellation_and_accepts_only_its_current_invitation(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server_with_media("Harbor");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::direct(admin.handle.account().unwrap(), window.own_account()),
    };
    world.run(caller.calls.join(target)).unwrap();
    window.until("incoming invitation can be checked", |window| {
        window.shows("check-invitation-devices")
    });
    window.click("check-invitation-devices");
    window.click_in_device_check("preview-camera");
    window.until("invitation preview", |window| {
        window.shows("camera-preview-picture")
    });
    world.run(caller.calls.leave());
    window.until("cancelled invitation ends its check", |window| {
        !window.shows("prejoin-dialog") && window.camera.active() == 0
    });
    assert!(
        window
            .state
            .read_with(&window.cx, |state, _| state.media_calls().call.is_none())
    );
    world.run(caller.calls.join(target)).unwrap();
    window.until("new invitation", |window| {
        window.shows("check-invitation-devices")
    });
    window.click("check-invitation-devices");
    window.click_in_device_check("prejoin-join");
    window.until("current invitation accepted", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .media_calls()
                .call
                .is_some_and(|call| call.target == target && call.phase == Phase::Connected)
        })
    });
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    world.run(calls.leave());
    world.run(caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_group_invitation_can_be_checked_and_joined(cx: &mut TestAppContext) {
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
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Group {
            conversation_id: group.id,
        },
    };
    world.run(caller.calls.join(target)).unwrap();
    window.until("group invitation device check", |window| {
        window.shows("check-invitation-devices")
    });
    window.click("check-invitation-devices");
    window.click_in_device_check("preview-camera");
    window.until("group invitation preview", |window| {
        window.shows("camera-preview-picture")
    });
    window.click_in_device_check("prejoin-join");
    window.until("group accepted without publishing preview", |window| {
        window.camera.active() == 0
            && window.state.read_with(&window.cx, |state, _| {
                state.media_calls().call.is_some_and(|call| {
                    call.target == target
                        && call.phase == Phase::Connected
                        && call.camera == CameraState::Off
                })
            })
    });
    world.run(caller.calls.leave());
    window.click("leave-call");
}
