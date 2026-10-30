use super::*;

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_stalled_capture_reports_stop_failure_and_leave_still_works(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Capture stop");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    window.click_channel(ROOM);
    for wait_for_notice in [false, true] {
        window.click("join-call");
        window.until("connected", |window| {
            window
                .view()
                .call
                .is_some_and(|call| call.phase == Phase::Connected)
        });
        let (release, finished) = window.capture.stall_next_stop();
        window.click("call-share");
        window.until("sources listed", |window| window.shows("share-sources"));
        window.click("share-window:11");
        window.click("share-sound-0");
        window.click("start-share");
        window.until("sharing", |window| own_share(window) == OwnShare::Running);
        window.click("call-share");
        window.click("share-stop:0");
        if wait_for_notice {
            let deadline = std::time::Instant::now() + PATIENCE;
            loop {
                window.settle();
                let view = window
                    .state
                    .read_with(&window.cx, |state, _| state.calls().view());
                if let Some(error) = view.share_stop_failed {
                    assert!(error.to_string().contains("stalled-capture"), "{error}");
                    if window.shows("share-notice") {
                        break;
                    }
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "Stop Sharing did not report its failure"
                );
                std::thread::yield_now();
            }
        }
        assert!(
            finished.try_recv().is_err(),
            "the injected worker ended before Leave"
        );
        window.click("leave-call");
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            window.settle();
            if window
                .state
                .read_with(&window.cx, |state, _| state.calls().view().call.is_none())
                && own(&mut window).is_none()
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Leave waited for the capture worker"
            );
            std::thread::yield_now();
        }
        assert!(
            finished.try_recv().is_err(),
            "Leave ended the injected worker"
        );
        window.until("cleanup failure remains visible after Leave", |window| {
            window.state.read_with(&window.cx, |state, _| {
                let view = state.calls().view();
                assert!(view.call.is_none());
                view.share_stop_failed.is_some()
            }) && window.shows("share-notice")
        });
        window.click("dismiss-share-notice");
        window.until("cleanup failure dismissed outside the call", |window| {
            !window.shows("share-notice")
                && window.state.read_with(&window.cx, |state, _| {
                    state.calls().view().share_stop_failed.is_none()
                })
        });
        let _ = release.send(());
        finished.recv_timeout(PATIENCE).unwrap();
    }
}
