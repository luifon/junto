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
    window.click("leave-call");
    world.run(caller.calls.leave());
}
