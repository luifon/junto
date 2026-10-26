use gpui::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, TestAppContext, px};
use junto_client_core::settings::Panel;

use crate::harness::{Window, World, member, open};

/// Drags the edge with this name by `by` pixels of the window to the right.
fn drag(window: &mut Window, edge: &'static str, by: f32) {
    window.settle();
    let start = window
        .cx
        .debug_bounds(edge)
        .unwrap_or_else(|| panic!("{edge} is not drawn"))
        .center();
    let end = gpui::point(start.x + px(by), start.y);
    window.dispatch(MouseDownEvent {
        button: MouseButton::Left,
        position: start,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    for step in 1..=4 {
        window.dispatch(MouseMoveEvent {
            position: gpui::point(start.x + px(by * step as f32 / 4.0), start.y),
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::none(),
        });
    }
    window.dispatch(MouseUpEvent {
        button: MouseButton::Left,
        position: end,
        modifiers: Modifiers::none(),
        click_count: 1,
    });
    window.settle();
}

fn width(window: &mut Window, selector: &'static str) -> f32 {
    f32::from(
        window
            .cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} is not drawn"))
            .size
            .width,
    )
}

fn kept(window: &mut Window, panel: Panel) -> Option<u16> {
    window
        .state
        .read_with(&window.cx, |state, _| state.settings().panel_widths.get(panel))
}

/// The server book, the channel list and a side panel are made wider and narrower by a
/// drag of their edge, within the bounds of each panel, and keep their widths after a
/// restart (finding 12 of the second live test).
#[gpui::test]
fn the_edge_of_a_panel_changes_its_width_within_its_bounds_and_the_width_is_kept(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Panels");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    window.simulate(|cx| cx.simulate_resize(gpui::size(px(1600.0), px(900.0))));
    window.settle();

    let book = width(&mut window, "server-book");
    drag(&mut window, "edge-server-book", 60.0);
    assert_eq!(width(&mut window, "server-book"), book + 60.0);
    assert_eq!(kept(&mut window, Panel::ServerBook), Some((book + 60.0) as u16));

    let channels = width(&mut window, "channel-column");
    drag(&mut window, "edge-channels", 40.0);
    assert_eq!(width(&mut window, "channel-column"), channels + 40.0);

    window.click("members-button");
    window.until("the member list is open", |window| window.shows("side-panel"));
    let members = width(&mut window, "side-panel");
    // The edge of a panel at the right of the main area is its left edge.
    drag(&mut window, "edge-members", -100.0);
    assert_eq!(width(&mut window, "side-panel"), members + 100.0);
    drag(&mut window, "edge-members", 2000.0);
    assert_eq!(
        width(&mut window, "side-panel"),
        f32::from(Panel::Members.bounds().0),
        "the member list is narrower than its least width"
    );
    drag(&mut window, "edge-members", -150.0);
    let chosen = width(&mut window, "side-panel");
    assert_eq!(chosen, f32::from(Panel::Members.bounds().0) + 150.0);

    // Another side panel keeps its own width.
    window.click("members-button");
    window.click("notices-row");
    window.until("the notices are open", |window| window.shows("notices-panel"));
    assert_eq!(width(&mut window, "notices-panel"), members);

    window.run_tasks();
    let stored = std::fs::read_to_string(directory.path().join("junto").join("settings.json"))
        .expect("the settings are written");
    assert!(stored.contains("\"members\""), "{stored}");
    window.close();

    let mut window = open(cx, directory.path());
    window.simulate(|cx| cx.simulate_resize(gpui::size(px(1600.0), px(900.0))));
    window.until_ready();
    assert_eq!(width(&mut window, "server-book"), book + 60.0);
    assert_eq!(width(&mut window, "channel-column"), channels + 40.0);
    window.click("members-button");
    window.until("the member list is open", |window| window.shows("side-panel"));
    assert_eq!(width(&mut window, "side-panel"), chosen);
}

/// The edge holds the keyboard focus and moves with the arrow keys, for a person who does
/// not use a pointer (ADR 0702).
#[gpui::test]
fn the_edge_of_a_panel_moves_with_the_arrow_keys(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Panels");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let channels = width(&mut window, "channel-column");
    window.click("edge-channels");
    window.press("right");
    window.press("right");
    assert_eq!(width(&mut window, "channel-column"), channels + 32.0);
    window.press("left");
    assert_eq!(width(&mut window, "channel-column"), channels + 16.0);
}
