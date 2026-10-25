//! The notices of the call of this device that the person puts away, and the notices of
//! the server in the panel of the call and in the call window, without a media server.

use gpui::TestAppContext;
use junto_client_core::protocol::call::CallPlace;
use junto_media::calls::{ActiveCall, CallTarget, CallsView, Phase};
use junto_media::engine::{MicrophoneState, OutputState, ShareState};
use junto_media::share::CaptureError;

use crate::calls::voice_channel;
use crate::harness::{Window, World, member};

/// A connected call of this device in the voice channel with this name, with this state of
/// the own share, as the media engine states it.
fn call_with_share(window: &mut Window, channel: &str, share: ShareState) -> ActiveCall {
    let view = window.view();
    let row = (window.channels().into_iter())
        .find(|row| row.name == channel)
        .unwrap();
    ActiveCall {
        camera: junto_media::camera::CameraState::Off,
        cameras: Vec::new(),
        target: CallTarget {
            server_id: view.selected.as_ref().unwrap().id,
            place: CallPlace::Channel {
                workspace_id: view.workspace().unwrap().id,
                channel_id: row.id,
            },
        },
        identity: "seat".to_owned(),
        phase: Phase::Connected,
        interrupted_at: None,
        microphone: MicrophoneState::Running,
        access: junto_media::permission::MicrophoneAccess::Allowed,
        input_device: None,
        output: OutputState::Running,
        output_device: None,
        microphone_block: None,
        sound_block: None,
        share,
        shares: Vec::new(),
        watched: None,
    }
}

/// The calls of the window state this call of this device, as a session states it, and the
/// state of the application takes the change as it takes every change of the calls.
fn give_calls(window: &mut Window, call: ActiveCall) {
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    calls.show_view_for_test(CallsView {
        call: Some(call),
        ..CallsView::default()
    });
    window.settle();
}

/// The notice of a share that ended by itself goes with the click of the person. The calls
/// put the ended share away on their own threads; the window does not wait for them, so
/// the notice is not shown again while they do.
#[gpui::test]
fn the_notice_of_an_ended_share_goes_at_the_click_of_the_person(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let ended = call_with_share(
        &mut window,
        "Daily",
        ShareState::Ended(CaptureError::SourceGone),
    );
    give_calls(&mut window, ended.clone());
    assert!(window.shows("share-notice"));

    // The calls have not put the share away: they still state the end.
    window.click("dismiss-share-notice");
    assert!(!window.shows("share-notice"));
    give_calls(&mut window, ended.clone());
    assert!(!window.shows("share-notice"));

    // A later share that ends is told again.
    let running = call_with_share(&mut window, "Daily", ShareState::Off);
    give_calls(&mut window, running);
    give_calls(&mut window, ended);
    assert!(window.shows("share-notice"));
}

/// The notices of the server open in the panel of the call of this device, in place of
/// its tab, also in a narrow window, which draws the panel over the stage (ADR 0742).
#[gpui::test]
fn the_notices_open_in_the_panel_of_the_call(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let (general, own) = (window.selected_channel(), window.own_account());
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = call_with_share(&mut window, "Daily", ShareState::Off);
    give_calls(&mut window, call);
    assert!(window.shows("call-panel") && window.shows("call-chat"));

    // A mention in another channel is counted at the row of the notices.
    admin.mention(&world, general, "look at this, ", own);
    window.until("the notice is counted", |window| {
        window.unread_notices() == 1
    });
    assert!(window.shows("notices-count"));

    // The row opens the notices in the panel of the call, beside the stage.
    window.click("notices-row");
    window.until(
        "the notices are listed in the panel of the call",
        |window| {
            window.shows("notices-panel")
                && window
                    .notices()
                    .is_some_and(|panel| !panel.reading && panel.rows.len() == 1)
        },
    );
    assert!(window.shows("call-panel") && window.shows("call-stage"));
    assert!(!window.shows("call-chat"));
    // A tab of the panel shows its pane again.
    window.click("call-tab-members");
    assert!(!window.shows("notices-panel") && window.notices().is_none());

    // A narrow window draws the panel over the stage while it is open, and the row of the
    // notices opens it at the list.
    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(720.0), gpui::px(600.0))));
    window.settle();
    window.click("close-call-panel");
    assert!(!window.shows("call-panel") && window.shows("notices-count"));
    window.click("notices-row");
    window.until("the notices are listed over the stage", |window| {
        window.shows("call-panel") && window.shows("notices-panel")
    });
    window.click("close-notices");
    assert!(!window.shows("notices-panel") && window.shows("call-panel"));
}

/// The call window counts and lists the notices of the server of its call, whatever the
/// main window shows (ADR 0742, ADR 0772).
#[gpui::test]
fn the_call_window_counts_and_lists_the_notices(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let (general, own) = (window.selected_channel(), window.own_account());
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = call_with_share(&mut window, "Daily", ShareState::Off);
    give_calls(&mut window, call);
    window.click("detach-call");
    let mut detached = window.call_window();
    assert!(detached.shows("open-call-notices") && !detached.shows("open-call-notices-count"));

    admin.mention(&world, general, "look at this, ", own);
    detached.until("the notice is counted in the call window", |window| {
        window.shows("open-call-notices-count")
    });
    detached.click("open-call-notices");
    detached.until("the notices are listed in the call window", |window| {
        window.shows("notices-panel")
            && window
                .notices()
                .is_some_and(|panel| !panel.reading && panel.rows.len() == 1)
    });
    assert!(!window.shows("notices-panel"));
    // A tab of the panel shows its pane again.
    detached.click("call-tab-chat");
    assert!(!detached.shows("notices-panel") && detached.shows("call-chat"));
}

/// The rules of a system notification count the call window as a window of the
/// application that shows the place of its call (ADR 0742).
#[gpui::test]
fn the_call_window_shows_the_place_of_the_call_to_the_rules_of_a_system_notification(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let (general, own) = (window.selected_channel(), window.own_account());
    let general_name = (window.channels().into_iter())
        .find(|row| row.id == general)
        .unwrap()
        .name;
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let daily = window.selected_channel();
    let call = call_with_share(&mut window, "Daily", ShareState::Off);
    give_calls(&mut window, call);
    window.click("detach-call");
    let mut detached = window.call_window();
    // The main window shows the text channel and is not active; the person looks at the
    // call window.
    window.click_channel(&general_name);
    window.open_notices();
    detached.simulate(|cx| cx.update(|window, _| window.activate_window()));
    detached.settle();

    // A mention in the chat of the call is no system notification, and a mention in the
    // channel that the inactive main window shows is one.
    admin.mention(&world, daily, "in the call, ", own);
    admin.mention(&world, general, "and here, ", own);
    window.until("both notices are listed", |window| {
        window
            .notices()
            .is_some_and(|panel| !panel.reading && panel.rows.len() == 2)
    });
    window.until("the system notification of the text channel", |window| {
        window
            .kept()
            .told
            .iter()
            .any(|alert| alert.body == "and here, @Luca")
    });
    let bodies: Vec<String> = window
        .kept()
        .told
        .iter()
        .map(|alert| alert.body.clone())
        .collect();
    assert_eq!(bodies, ["and here, @Luca"]);
}
