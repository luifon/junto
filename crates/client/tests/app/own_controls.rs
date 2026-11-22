//! The row of the person's own controls under the call, and the header of a call that holds
//! only what everyone in the call shares (ADR 0735, ADR 0772).

use gpui::TestAppContext;

use crate::harness::{Window, World, member};

/// The controls of the row on a wide stage, in their order.
const WIDE_ROW: [&str; 9] = [
    "own-microphone",
    "own-sound",
    "own-camera",
    "own-share",
    "own-view",
    "own-devices",
    "own-detach",
    "own-leave",
    "own-controls",
];

fn show_call(window: &mut Window, call: junto_media::calls::ActiveCall) {
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    calls.show_view_for_test(junto_media::calls::CallsView {
        call: Some(call),
        ..Default::default()
    });
    window.settle();
}

fn left_of(window: &mut Window, selector: &'static str) -> f32 {
    f32::from(window.cx.debug_bounds(selector).unwrap().left())
}

/// The row lies under the stage with every control on a wide stage; the header has no
/// settings button. "View" opens only its menu, which sets the layout and the settings of
/// "Your view"; the devices button opens Settings at "Voice and video". On a narrow stage the
/// row keeps microphone, sound, camera, share and Leave, and its "⋮" holds the others in the
/// order of the row.
#[gpui::test]
fn the_row_of_own_controls_lies_under_the_call_and_narrows_into_its_menu(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    crate::calls::voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = crate::calls::connected_call(&mut window, "Daily");
    show_call(&mut window, call);
    window.until("the row of own controls", |window| {
        window.shows("own-controls")
    });
    for control in WIDE_ROW {
        assert!(window.shows(control), "{control} in the row");
    }
    assert!(!window.shows("own-more") && !window.shows("open-call-settings"));
    let stage = window.cx.debug_bounds("call-stage").unwrap();
    let row = window.cx.debug_bounds("own-controls").unwrap();
    assert!(
        row.top() >= stage.bottom() - gpui::px(1.0),
        "{row:?} under {stage:?}"
    );
    let order: Vec<f32> = WIDE_ROW[..8]
        .iter()
        .map(|control| left_of(&mut window, control))
        .collect();
    assert!(order.windows(2).all(|pair| pair[0] < pair[1]), "{order:?}");

    // "View": its press opens only its menu.
    window.click("own-view");
    assert!(window.shows("view-menu"));
    assert!(window.view().call.is_some());
    window.click("view-hide-without-video");
    assert!(!window.shows("view-menu"));
    assert!(window.state.read_with(&window.cx, |state, _| {
        state.voice().hide_people_without_video
    }));
    window.click("own-view");
    window.click("view-hide-without-video");
    assert!(!window.state.read_with(&window.cx, |state, _| {
        state.voice().hide_people_without_video
    }));
    // The camera is off: "Hide my tile" is shown and disabled.
    window.click("own-view");
    window.click("view-hide-my-tile");
    assert!(
        !window
            .state
            .read_with(&window.cx, |state, _| state.voice().hide_own_camera)
    );
    window.press("escape");

    window.click("own-devices");
    assert!(window.shows_named(format!(
        "settings-page-{}",
        junto_client::shell::SettingsSection::VoiceAndVideo.selector()
    )));
    window.press("escape");

    // A narrow stage: the less used controls are in "⋮".
    window.simulate(|cx| cx.simulate_resize(gpui::size(gpui::px(720.0), gpui::px(600.0))));
    window.settle();
    window.settle();
    for control in [
        "own-microphone",
        "own-sound",
        "own-camera",
        "own-share",
        "own-leave",
    ] {
        assert!(window.shows(control), "{control} on a narrow stage");
    }
    for control in ["own-view", "own-devices", "own-detach"] {
        assert!(!window.shows(control), "{control} on a narrow stage");
    }
    window.click("own-more");
    for item in ["more-view", "more-devices", "more-detach"] {
        assert!(window.shows(item), "{item} in the menu");
    }
    window.press("escape");
    assert!(!window.shows("own-more-menu"));
}
