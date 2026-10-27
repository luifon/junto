use gpui::{TestAppContext, px, size};

use crate::calls::{connected_call, show_call, voice_channel};
use crate::harness::{Window, World, member};
use crate::messages::document;

/// The least size of the main window (`app.rs`) and the size of the narrow test of the
/// panel buttons, and the least size of the call window (`detached.rs`).
const MAIN_SIZES: [(f32, f32); 2] = [(720.0, 480.0), (720.0, 600.0)];
const CALL_WINDOW_LEAST: (f32, f32) = (480.0, 320.0);

fn resize(window: &mut Window, (width, height): (f32, f32)) {
    window.simulate(|cx| cx.simulate_resize(size(px(width), px(height))));
    window.settle();
}

/// The buttons of one drawing of the window that lie past its left or right edge, where only
/// the window clips them (ADR 0730).
fn outside(window: &mut Window) -> Vec<String> {
    // The header measures itself in one drawing and draws the form that fits in the next.
    for _ in 0..2 {
        window.cx.update(|window, _| window.refresh());
        window.settle();
    }
    junto_client::ui::record_controls();
    window.cx.update(|window, _| window.refresh());
    window.settle();
    let controls = junto_client::ui::take_controls();
    let width = window.cx.update(|window, _| window.viewport_size().width);
    junto_client::ui::outside(&controls, width)
        .into_iter()
        .map(|control| format!("{} at {:?}", control.name, control.bounds))
        .collect()
}

#[track_caller]
fn assert_inside(window: &mut Window, what: &str) {
    let outside = outside(window);
    assert!(outside.is_empty(), "{what}: past the edge: {outside:?}");
}

fn right_of(window: &mut Window, selector: &'static str) -> f32 {
    let bounds = window
        .cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    f32::from(bounds.right())
}

fn width_of(window: &mut Window, selector: &'static str) -> f32 {
    let bounds = window
        .cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    f32::from(bounds.size.width)
}

#[gpui::test]
fn the_header_keeps_its_controls_in_the_window_at_its_least_width_without_a_call(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio with a long name");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(
        &world,
        &admin,
        &mut window,
        "Daily stand-up of the whole studio",
    );

    // A wide window shows the labels of the buttons.
    resize(&mut window, (1600.0, 900.0));
    assert_inside(&mut window, "wide, text channel");
    let members = width_of(&mut window, "members-button");
    assert!(
        members > 40.0,
        "the members button has its label: {members}"
    );

    for least in MAIN_SIZES {
        resize(&mut window, least);
        assert_inside(&mut window, &format!("{least:?}, text channel"));
        for selector in ["members-button", "search-field", "channel-settings"] {
            assert!(
                right_of(&mut window, selector) <= least.0,
                "{selector} in {least:?}"
            );
        }
        window.click_channel("Daily stand-up of the whole studio");
        assert_inside(
            &mut window,
            &format!("{least:?}, voice channel outside its call"),
        );
        assert!(right_of(&mut window, "join-call") <= least.0);
        let general = (window.channels().into_iter())
            .find(|row| row.name != "Daily stand-up of the whole studio")
            .unwrap()
            .name;
        window.click_channel(&general);
    }

    // The labels come back when the window is wide again.
    resize(&mut window, (1600.0, 900.0));
    assert_inside(&mut window, "wide again");
    assert_eq!(width_of(&mut window, "members-button"), members);
}

#[gpui::test]
fn the_header_of_a_call_keeps_its_controls_in_the_window_with_the_panel_open_and_closed(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    let daily = (window.channels().into_iter())
        .find(|row| row.name == "Daily")
        .unwrap()
        .id;
    window.click_channel("Daily");
    let call = connected_call(&mut window, "Daily");
    show_call(&mut window, call);
    window.until("the panel of the call", |window| {
        window.shows("call-tab-members")
    });
    window.click("call-tab-members");
    admin.send(&world, daily, document("one"));
    admin.send(&world, daily, document("two"));
    window.until("the chat counts the messages", |window| {
        window.feed().is_some_and(|feed| feed.unread == 2)
    });

    for least in MAIN_SIZES {
        resize(&mut window, least);
        // A narrow window draws the panel over the stage, closed until it is opened.
        window.click_if_shown("close-call-panel");
        window.until("the buttons of the panel", |window| {
            window.shows("open-call-chat")
        });
        assert_inside(&mut window, &format!("{least:?}, call, panel closed"));
        for selector in ["open-call-chat", "open-call-members", "open-call-settings"] {
            assert!(
                right_of(&mut window, selector) <= least.0,
                "{selector} in {least:?}"
            );
        }
        // The count of the unread messages is drawn on the Chat button, inside the window.
        assert!(right_of(&mut window, "open-call-chat-count") <= least.0);
        window.click("open-call-members");
        window.until("the panel is open", |window| window.shows("call-panel"));
        assert_inside(&mut window, &format!("{least:?}, call, panel open"));
        window.click("close-call-panel");
        window.until("the panel is closed", |window| !window.shows("call-panel"));
    }

    // A wide window draws the panel beside the stage; closed, its buttons are in the header.
    resize(&mut window, (1600.0, 900.0));
    window.click_if_shown("open-call-members");
    window.until("the panel beside the stage", |window| {
        window.shows("call-panel")
    });
    assert_inside(&mut window, "wide, call, panel open");
    window.click("close-call-panel");
    window.until("the panel is closed", |window| !window.shows("call-panel"));
    assert_inside(&mut window, "wide, call, panel closed");
}

#[gpui::test]
fn the_call_window_keeps_its_controls_in_the_window_at_its_least_width(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = connected_call(&mut window, "Daily");
    show_call(&mut window, call);
    window.until("the call is drawn", |window| window.shows("detach-call"));

    let main = window.cx.update(|window, _| window.window_handle());
    window.click("detach-call");
    window.settle();
    let handle = (cx.windows().into_iter())
        .find(|handle| *handle != main)
        .unwrap();
    let mut detached = window.window_of(handle);
    for least in [CALL_WINDOW_LEAST, (720.0, 600.0)] {
        resize(&mut detached, least);
        detached.click_if_shown("close-call-panel");
        assert_inside(
            &mut detached,
            &format!("call window {least:?}, panel closed"),
        );
        detached.click("open-call-members");
        detached.until("the panel is open", |window| window.shows("call-panel"));
        assert_inside(&mut detached, &format!("call window {least:?}, panel open"));
        detached.click("close-call-panel");
    }
    // The main window shows that the call is in its own window.
    for least in MAIN_SIZES {
        resize(&mut window, least);
        assert_inside(
            &mut window,
            &format!("main window {least:?}, call in its own window"),
        );
    }
}
