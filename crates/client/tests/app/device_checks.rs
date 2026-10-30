use crate::harness::{World, member};

impl crate::harness::Window {
    pub(crate) fn click_in_device_check(&mut self, selector: &'static str) {
        let scroll = self.in_shell(|shell, _, _| shell.device_check_scroll_handle());
        self.reveal(&scroll, selector);
        self.click(selector);
    }
}

use gpui::TestAppContext;
use junto_client::state::ProcessingPart;
use junto_media::{
    audio::{
        AudioError,
        synthetic::{Tone, tone_level},
    },
    camera::{CameraAccess, CameraDevice},
    device_check::{AudioCheckKind, AudioCheckState},
    permission::MicrophoneAccess,
};
use std::time::Duration;

#[gpui::test]
fn devices_can_be_checked_in_settings_before_a_call(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    window
        .sound
        .add_input("headset", "Headset", Tone::new(880.0));
    window.sound.add_output("headphones", "Headphones");
    let desk = CameraDevice {
        id: "desk".into(),
        name: "Desk camera".into(),
    };
    let rear = CameraDevice {
        id: "rear".into(),
        name: "Rear camera".into(),
    };
    window.camera.set_devices(vec![desk.clone(), rear]);
    window.permission.set(MicrophoneAccess::NotAsked);
    window.state.update(&mut window.cx, |state, cx| {
        state.set_processing(ProcessingPart::EchoCancellation, false, cx);
        state.set_processing(ProcessingPart::NoiseSuppression, false, cx);
        state.set_processing(ProcessingPart::AutomaticGain, false, cx);
    });
    window.click("app-settings");
    window.click("settings-section-voice");
    assert!(
        window.shows("input-device"),
        "microphone choice outside a call"
    );
    assert!(
        window.shows("output-device"),
        "output choice outside a call"
    );
    assert!(
        window.shows("camera-device"),
        "camera choice outside a call"
    );
    assert!(window.shows("test-microphone"));
    assert!(window.shows("test-output"));
    assert!(window.shows("preview-camera"));
    assert_eq!(window.permission.asked(), 0);
    assert_eq!(window.camera.started(), 0);
    window.until("all selected-device choices are read", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.audio_device_choices(true).len() == 3
                && state.audio_device_choices(false).len() == 3
                && state.camera_choices().len() == 3
        })
    });
    for field in ["input-device", "output-device", "camera-device"] {
        window.click_in_settings(field);
        window.click_named("check-device-choice-2".into());
        window.until("device menu closes", |window| {
            !window.shows("check-device-menu")
        });
    }
    window.click_in_settings("test-microphone");
    window.until(
        "the chosen microphone reaches the chosen output",
        |window| {
            let (samples, rate) = window.sound.played(Duration::from_millis(100));
            tone_level(&samples, rate, 880.0) > -35.0 && window.shows("device-test-playing")
        },
    );
    assert_eq!(window.permission.asked(), 1);
    assert_eq!(
        window
            .sound
            .opened_outputs()
            .last()
            .unwrap()
            .as_ref()
            .unwrap()
            .0,
        "headphones"
    );
    window.sound.remove_input("headset");
    window.sound.break_input(AudioError::NoDevice);
    window.until("removed microphone is reported", |window| {
        window.shows("device-test-error")
    });
    window.click_in_settings("refresh-check-devices");
    window.until("saved missing device is retained", |window| {
        window.shows("check-device-disconnected")
    });
    window.click_in_settings("test-microphone");
    assert!(
        window.state.read_with(&window.cx, |state, _| state
            .calls()
            .device_check_view()
            .audio
            .kind()
            == Some(AudioCheckKind::Microphone)),
        "microphone retry starts"
    );
    window.until("missing microphone uses the system default", |window| {
        let (samples, rate) = window.sound.played(Duration::from_millis(100));
        tone_level(&samples, rate, crate::harness::WINDOW_TONE_HZ) > -35.0
    });
    assert_eq!(window.sound.opened_inputs().last(), Some(&None));
    window.click_in_settings("test-microphone");
    window.until("microphone listen-back stops", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.calls().device_check_view().audio == AudioCheckState::Off
        })
    });
    window.sound.played_since_clear();
    window.click_in_settings("test-output");
    window.until("the output test reaches the output", |window| {
        let (samples, rate) = window.sound.played(Duration::from_millis(100));
        tone_level(&samples, rate, 660.0) > -45.0
    });
    window.sound.break_output(AudioError::Refused);
    window.until("an output failure can be read", |window| {
        window.shows("device-test-error")
    });
    window.click_in_settings("test-output");
    window.until("output test retries", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.calls().device_check_view().audio
                == AudioCheckState::Running(AudioCheckKind::Output)
        })
    });
    window.click_in_settings("preview-camera");
    window.until("the local camera picture is drawn", |window| {
        window.shows("camera-preview-picture") && window.camera.active() == 1
    });
    assert!(window.view().call.is_none());
    window.camera.set_devices(vec![desk]);
    window.until("removal of the chosen camera ends its preview", |window| {
        window.shows("camera-preview-error") && !window.shows("camera-preview-picture")
    });
    window.click_in_settings("camera-device");
    window.click_named("check-device-choice-0".into());
    window.click_in_settings("preview-camera");
    window.until("another camera can preview", |window| {
        window.shows("camera-preview-picture")
    });
    window.click("settings-section-profile");
    window.until("leaving Voice closes tests and devices", |window| {
        window.camera.active() == 0
            && window.state.read_with(&window.cx, |state, _| {
                !state.calls().input_test_runs()
                    && state.calls().device_check_view() == Default::default()
            })
    });
    window.press("escape");
    window.close();
    let reopened = crate::harness::open(cx, directory.path());
    let voice = reopened
        .state
        .read_with(&reopened.cx, |state, _| state.voice().clone());
    assert_eq!(voice.input_device.as_deref(), Some("headset"));
    assert_eq!(voice.output_device.as_deref(), Some("headphones"));
    assert_eq!(voice.camera_device, None);
}

#[gpui::test]
fn refused_preview_can_retry_and_closing_settings_releases_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    window.camera.set_access(CameraAccess::Refused);
    window.click("app-settings");
    window.click("settings-section-voice");
    window.click_in_settings("preview-camera");
    window.until("refused camera is explained", |window| {
        window.shows("camera-preview-error")
    });
    assert_eq!(window.camera.started(), 0);
    window.camera.set_access(CameraAccess::Allowed);
    window.click_in_settings("preview-camera");
    assert!(
        window.state.read_with(&window.cx, |state, _| matches!(
            state.calls().device_check_view().camera,
            junto_media::camera::CameraState::Starting | junto_media::camera::CameraState::Running
        )),
        "retry starts capture"
    );
    window.until("retry draws the camera", |window| {
        window.shows("camera-preview-picture")
    });
    window.press("escape");
    window.until("closing settings closes camera", |window| {
        window.camera.active() == 0
    });
    assert!(!window.shows("camera-preview-picture"));
}

#[gpui::test]
fn channel_pair_and_group_calls_offer_a_guarded_check_before_joining(cx: &mut TestAppContext) {
    use crate::admin_pages::{invite, joined};
    use junto_client_core::protocol::structure::ChannelKind;
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_third_device, third) = joined(&world, &server, &code, "Third");
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    let channel = window
        .channels()
        .into_iter()
        .find(|channel| channel.kind == ChannelKind::Voice)
        .unwrap();
    window.click_channel(&channel.name);
    window.click("check-call-devices");
    assert!(window.shows("prejoin-dialog"));
    window.click_in_device_check("preview-camera");
    assert!(
        window.state.read_with(&window.cx, |state, _| matches!(
            state.calls().device_check_view().camera,
            junto_media::camera::CameraState::Starting | junto_media::camera::CameraState::Running
        )),
        "prejoin starts capture"
    );
    window.until("prejoin preview is drawn", |window| {
        window.shows("camera-preview-picture")
    });
    window.click_in_device_check("cancel-device-check");
    window.until("cancel closes local camera", |window| {
        window.camera.active() == 0
    });
    assert!(
        window
            .state
            .read_with(&window.cx, |state, _| state.media_calls().call.is_none()
                && state.media_calls().joining.is_none())
    );
    let pair = world
        .run(admin.handle.api().open_conversation(window.own_account()))
        .unwrap();
    window.until("pair row", |window| {
        window.shows_named(format!("direct-row:{}", pair.id))
    });
    window.click_named(format!("direct-row:{}", pair.id));
    window.click("check-direct-devices");
    assert!(window.shows("prejoin-dialog"));
    window.click_in_device_check("cancel-device-check");
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
    window.click("check-group-devices");
    window.click_in_device_check("preview-camera");
    window.until("group prejoin preview", |window| {
        window.shows("camera-preview-picture")
    });
    let account = window.own_account();
    let api = window.state.read_with(&window.cx, |state, _| {
        state.server(admin.handle.id()).unwrap().api_for(account)
    });
    world.run(api.leave_group(group.id)).unwrap();
    window.until("leaving the group closes the stale prejoin", |window| {
        !window.shows("prejoin-dialog") && window.camera.active() == 0
    });
}

#[gpui::test]
fn removing_the_current_server_cancels_its_open_device_checks(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    window.click("app-settings");
    window.click("settings-section-voice");
    window.click_in_settings("preview-camera");
    window.until("local preview", |window| {
        window.shows("camera-preview-picture")
    });
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.remove_server(admin.handle.id(), cx)
    }));
    window.until("account context removal releases tests", |window| {
        window.camera.active() == 0
            && window.state.read_with(&window.cx, |state, _| {
                !state.device_checks_open()
                    && state.calls().device_check_view() == Default::default()
                    && !state.calls().input_test_runs()
            })
    });
    assert!(!window.shows("camera-preview-picture"));
}

#[gpui::test]
fn a_late_microphone_permission_answer_does_not_reopen_closed_checks(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    window.permission.set(MicrophoneAccess::NotAsked);
    window.permission.hold();
    window.click("app-settings");
    window.click("settings-section-voice");
    window.click_in_settings("test-microphone");
    window.until("the system permission question is pending", |window| {
        window.permission.asked() == 1
    });
    assert!(window.sound.opened_inputs().is_empty());
    window.press("escape");
    window.permission.release();
    window.until(
        "closed checks remain released after the permission answer",
        |window| {
            window.state.read_with(&window.cx, |state, _| {
                !state.device_checks_open()
                    && state.calls().device_check_view() == Default::default()
                    && !state.calls().input_test_runs()
            })
        },
    );
    window.settle();
    assert!(window.sound.opened_inputs().is_empty());
    assert_eq!(window.camera.active(), 0);
    window.click("app-settings");
    window.click("settings-section-voice");
    assert!(!window.shows("device-test-playing"));
    assert!(window.state.read_with(
        &window.cx,
        |state, _| state.calls().device_check_view().audio == AudioCheckState::Off
    ));
}

#[gpui::test]
fn an_actual_account_replacement_releases_checks_and_ignores_late_permission(
    cx: &mut TestAppContext,
) {
    use junto_client_core::{flow::access::AccessState, protocol::account::RecoverAccount};
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let proxy = world.run(crate::support::FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Reader");
    let handle = window.handle();
    let first_device = match handle.access().state {
        AccessState::LoggedIn { device_id, .. } => device_id,
        other => panic!("expected a session: {other:?}"),
    };
    let (_phone, phone) = crate::account::second_device(&world, &server, &handle);
    let code = admin.invite(&world, 1);
    let (_other_device, other) = crate::admin_pages::joined(&world, &server, &code, "Other reader");
    let other = crate::support::logged_in(&other);
    window.permission.set(MicrophoneAccess::NotAsked);
    window.permission.hold();
    window.click("app-settings");
    window.click("settings-section-voice");
    window.click_in_settings("preview-camera");
    window.until("the old account previews its camera", |window| {
        window.camera.active() == 1 && window.shows("camera-preview-picture")
    });
    window.click_in_settings("test-microphone");
    window.until(
        "microphone permission waits for the old account",
        |window| window.permission.asked() == 1,
    );
    let key = handle.device_public_key().unwrap();
    proxy.hold_events();
    world.run(async {
        phone.api().remove_device(first_device).await.unwrap();
        admin
            .handle
            .api()
            .recover_account(
                other,
                &RecoverAccount {
                    public_key: key,
                    device_name: "Laptop".into(),
                },
            )
            .await
            .unwrap();
        let result=handle.api().own_account().await;
        assert!(matches!(result,Err(junto_client_core::api::ApiError::AccountChanged {now,..}) if now==other),"the ended session must renew into the recovered account: {result:?}");
    });
    window.until_read(
        "the same server now has another account and releases old checks",
        |window| {
            (
                handle.account(),
                window.camera.active(),
                window.state.read_with(&window.cx, |state, _| {
                    (
                        state.device_checks_open(),
                        state.calls().device_check_view(),
                        state.calls().input_test_runs(),
                    )
                }),
            )
        },
        |(account, cameras, (open, view, input))| {
            *account == Some(other)
                && *cameras == 0
                && !*open
                && *view == Default::default()
                && !*input
        },
    );
    proxy.release();
    window.permission.release();
    window.settle();
    assert_eq!(handle.account(), Some(other));
    assert!(window.sound.opened_inputs().is_empty());
    assert_eq!(window.camera.active(), 0);
    assert!(!window.shows("camera-preview-picture"));
}
