//! The notices of the call of this device that the person puts away, without a media server.

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
