use super::*;

fn received_picture_survives_a_nested_frame(cx: &mut TestAppContext, camera: bool) {
    let world = World::new();
    let server = world.server_with_media("Received pictures");
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
    if camera {
        caller.calls.start_camera(None);
    } else {
        caller.calls.start_share(CaptureRequest {
            picture: PictureSource::Window(11),
            sound: SoundChoice::None,
            quality: ShareQuality::default(),
            dialog_parent: None,
        });
    }
    let deadline = std::time::Instant::now() + PATIENCE;
    loop {
        window.park();
        if window
            .shell
            .read_with(&window.cx, |shell, _| shell.pictures_waiting_for_frame())
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no received picture was prepared"
        );
        std::thread::yield_now();
    }
    // Windows may dispatch WM_PAINT while an application update still holds App.
    window.simulate(|cx| crate::harness::native_frame(cx, true));
    window.until("the main window draws the received picture", |window| {
        window.shell.read_with(&window.cx, |shell, _| {
            if camera {
                shell.drawn_camera_count() == 1
            } else {
                shell.draws().2.is_some()
            }
        })
    });
    if !camera {
        assert!(window.shows("share-picture"));
        assert!(!window.shows("share-waiting"));
    }
    let images = window
        .shell
        .read_with(&window.cx, |shell, _| shell.picture_images());
    #[cfg(not(target_os = "macos"))]
    {
        let image = images
            .iter()
            .find(|image| {
                window
                    .cx
                    .update(|window, _| window.has_image_atlas_entry(image))
            })
            .expect("the received picture was not painted");
        let size = image.size(0);
        let (width, height) = (size.width.0 as usize, size.height.0 as usize);
        let pixels = image.as_bytes(0).unwrap();
        let at = |x| &pixels[(height / 2 * width + x) * 4..][..4];
        let (left, right) = (at(width / 4), at(width * 3 / 4));
        assert!(
            left[2] > 150 && left[0] < 70 && right[0] > 150 && right[2] < 70,
            "the painted picture lost its colours: {left:?}, {right:?}"
        );
    }
    #[cfg(target_os = "macos")]
    let _ = images;
    assert_eq!(window.cx.cx.windows().len(), 1);
    window.click("leave-call");
    world.run(caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn the_main_window_draws_a_received_share_after_a_nested_frame(cx: &mut TestAppContext) {
    received_picture_survives_a_nested_frame(cx, false);
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn the_main_window_draws_a_received_camera_after_a_nested_frame(cx: &mut TestAppContext) {
    received_picture_survives_a_nested_frame(cx, true);
}
