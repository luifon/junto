use super::*;

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_stalled_capture_reports_stop_failure_and_leave_still_works(cx: &mut TestAppContext) {
    stalled_capture(cx, false);
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_stalled_camera_reports_stop_failure_and_leave_still_works(cx: &mut TestAppContext) {
    stalled_capture(cx, true);
}

fn stalled_capture(cx: &mut TestAppContext, camera: bool) {
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
        let (release, finished) = if camera {
            let held = window.camera.stall_next_stop();
            window.click("call-camera");
            window.until("camera running", |window| {
                window.state.read_with(&window.cx, |state, _| {
                    state.media_calls().call.is_some_and(|call| {
                        call.camera == junto_media::camera::CameraState::Running
                    })
                })
            });
            window.click("call-camera");
            held
        } else {
            let held = window.capture.stall_next_stop();
            window.click("call-share");
            window.until("sources listed", |window| window.shows("share-sources"));
            window.click("share-window:11");
            window.click("share-sound-0");
            window.click("start-share");
            window.until("sharing", |window| own_share(window) == OwnShare::Running);
            window.click("call-share");
            window.click("share-stop:0");
            held
        };
        let worker = if camera {
            "generated-camera"
        } else {
            "stalled-capture"
        };
        if wait_for_notice {
            let deadline = std::time::Instant::now() + PATIENCE;
            loop {
                window.settle();
                let view = window
                    .state
                    .read_with(&window.cx, |state, _| state.calls().view());
                if let Some(error) = view.share_stop_failed {
                    assert!(error.to_string().contains(worker), "{error}");
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

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_stalled_microphone_or_output_reports_cleanup_after_leave(cx: &mut TestAppContext) {
    use junto_media::engine::{MicrophoneState, OutputState};
    use junto_media::share::CaptureKind;
    let world = World::new();
    let server = world.server_with_media("Sound stop");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    window.click_channel(ROOM);
    for input in [true, false] {
        let (release, finished) = window.sound.stall_next_stop(input);
        window.click("join-call");
        window.until("sound devices running", |window| {
            window.state.read_with(&window.cx, |state, _| {
                state.media_calls().call.is_some_and(|call| {
                    call.phase == Phase::Connected
                        && call.microphone == MicrophoneState::Running
                        && call.output == OutputState::Running
                })
            })
        });
        window.click("leave-call");
        window.until("Leave completes with a held sound destructor", |window| {
            window
                .state
                .read_with(&window.cx, |state, _| state.calls().view().call.is_none())
                && own(window).is_none()
        });
        window.until(
            "sound cleanup failure remains visible after Leave",
            |window| {
                window.state.read_with(&window.cx, |state, _| {
                    state
                        .calls()
                        .view()
                        .share_stop_failed
                        .is_some_and(|notice| {
                            notice.kind
                                == if input {
                                    CaptureKind::Microphone
                                } else {
                                    CaptureKind::SoundOutput
                                }
                        })
                }) && window.shows("share-notice")
            },
        );
        assert!(
            finished.try_recv().is_err(),
            "the native destructor was not held"
        );
        window.click("dismiss-share-notice");
        assert!(!window.shows("share-notice"));
        let _ = release.send(());
        finished.recv_timeout(PATIENCE).unwrap();
    }
}
