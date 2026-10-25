//! The title bar of the windows (ADR 0703): the strip with the window buttons where the
//! application draws the frame of a window, and the headers that act as the title bar of the
//! main window on macOS. The test window of gpui carries out no request of a title bar, so a
//! test reads the requests (`TitleBarRequests`); the system's answer to them is checked by
//! `packaging/check_windows.py` on Windows and by a person on macOS and Linux.

use gpui::{
    Bounds, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point,
    TestAppContext, point, px,
};
use junto_client::shell::title_bar::{
    DrawnFrameInTests, TitleBarRequest, TitleBarRequests, content_reaches_title_bar,
};

use crate::harness::{Window, World, member, named};

fn record_requests(cx: &mut TestAppContext) -> TitleBarRequests {
    let requests = TitleBarRequests::default();
    let installed = requests.clone();
    cx.update(|cx| cx.set_global(installed));
    requests
}

fn taken(requests: &TitleBarRequests) -> Vec<TitleBarRequest> {
    std::mem::take(&mut *requests.0.borrow_mut())
}

fn bounds(window: &mut Window, selector: &'static str) -> Bounds<Pixels> {
    window.settle();
    window
        .cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
}

/// A press of the left button with this count of presses, as the system reports the second
/// press of a double press.
fn press(window: &mut Window, position: Point<Pixels>, click_count: usize) {
    let modifiers = Modifiers::none();
    window.dispatch(MouseDownEvent {
        button: MouseButton::Left,
        position,
        modifiers,
        click_count,
        first_mouse: false,
    });
    window.dispatch(MouseUpEvent {
        button: MouseButton::Left,
        position,
        modifiers,
        click_count,
    });
}

fn double_press(window: &mut Window, position: Point<Pixels>) {
    press(window, position, 1);
    press(window, position, 2);
}

/// A press, a move of the pointer with the button held, and the release.
fn drag(window: &mut Window, from: Point<Pixels>, to: Point<Pixels>) {
    let modifiers = Modifiers::none();
    window.dispatch(MouseDownEvent {
        button: MouseButton::Left,
        position: from,
        modifiers,
        click_count: 1,
        first_mouse: false,
    });
    window.dispatch(MouseMoveEvent {
        position: to,
        pressed_button: Some(MouseButton::Left),
        modifiers,
    });
    window.dispatch(MouseUpEvent {
        button: MouseButton::Left,
        position: to,
        modifiers,
        click_count: 1,
    });
}

/// The strip spans the top of the window, the buttons minimize, maximize and close stand at
/// its right end in that order with the measures of Windows 11, and the columns start under
/// the strip.
fn assert_strip(window: &mut Window) {
    let width = window.cx.update(|window, _| window.viewport_size().width);
    let strip = bounds(window, "title-strip");
    assert_eq!(strip.origin, point(px(0.0), px(0.0)));
    assert_eq!(strip.size.width, width);
    assert_eq!(strip.size.height, px(32.0));
    let buttons = ["window-minimize", "window-maximize", "window-close"]
        .map(|selector| bounds(window, selector));
    for (index, button) in buttons.iter().enumerate() {
        assert_eq!(button.size.width, px(46.0));
        assert_eq!(button.top(), px(0.0));
        assert_eq!(
            button.right(),
            width - px(46.0) * (buttons.len() - 1 - index) as f32
        );
    }
    assert_eq!(bounds(window, "channel-column").top(), strip.bottom());
}

#[cfg(windows)]
#[gpui::test]
fn the_main_window_on_windows_draws_a_strip_with_its_buttons(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Luca");
    assert_strip(&mut window);
}

#[gpui::test]
fn a_drawn_frame_puts_the_strip_with_the_window_buttons_above_the_columns(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(DrawnFrameInTests(true)));
    let (_directory, mut window) = named(cx, "Luca");
    assert_strip(&mut window);
    // A dialog covers the columns and leaves the strip and its buttons free.
    window.click("app-settings");
    assert!(window.shows("scale-4"));
    assert_strip(&mut window);
}

#[gpui::test]
fn without_a_drawn_frame_the_columns_reach_the_top(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(DrawnFrameInTests(false)));
    let (_directory, mut window) = named(cx, "Luca");
    assert!(!window.shows("title-strip") && !window.shows("window-close"));
    assert_eq!(bounds(&mut window, "channel-column").top(), px(0.0));
}

/// On Linux the application handles the strip; on Windows the system does, by the areas
/// that gpui reports to its hit test.
#[cfg(not(windows))]
#[gpui::test]
fn the_strip_of_a_drawn_frame_minimizes_zooms_moves_and_closes_the_window(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(DrawnFrameInTests(true)));
    let requests = record_requests(cx);
    let (_directory, mut window) = named(cx, "Luca");
    window.click("window-minimize");
    assert_eq!(taken(&requests), [TitleBarRequest::Minimize]);
    window.click("window-maximize");
    assert_eq!(taken(&requests), [TitleBarRequest::Zoom]);
    let title = bounds(&mut window, "title-strip-drag").center();
    double_press(&mut window, title);
    assert_eq!(taken(&requests), [TitleBarRequest::Zoom]);
    drag(&mut window, title, title + point(px(40.0), px(10.0)));
    assert_eq!(taken(&requests), [TitleBarRequest::Move]);
    // A press on a button and a move away from it do not move the window.
    let close = bounds(&mut window, "window-close").center();
    drag(&mut window, close, close - point(px(200.0), px(0.0)));
    assert_eq!(taken(&requests), []);
    assert_eq!(cx.update(|cx| cx.windows().len()), 1);
    let close = bounds(&mut window, "window-close").center();
    window.press_at(close, MouseButton::Left);
    assert_eq!(cx.update(|cx| cx.windows().len()), 0);
    assert_eq!(taken(&requests), []);
}

/// The headers at the top of the main window on macOS: a double press where no control is
/// asks for the action of the system setting, a press and a move there move the window, and a
/// press on a control stays the control's.
#[gpui::test]
fn the_headers_act_as_the_title_bar_where_no_control_is(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Title");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let requests = record_requests(cx);
    // Before the registration of the window, a header is no title bar.
    let column = bounds(&mut window, "channel-column");
    let header = column.origin + point(px(5.0), px(10.0));
    double_press(&mut window, header);
    assert_eq!(taken(&requests), []);
    window
        .cx
        .update(|window, cx| content_reaches_title_bar(window, cx));
    double_press(&mut window, header);
    assert_eq!(taken(&requests), [TitleBarRequest::DoublePress]);
    drag(&mut window, header, header + point(px(30.0), px(0.0)));
    assert_eq!(taken(&requests), [TitleBarRequest::Move]);
    // The strip at the top of the server book, where macOS draws the window buttons.
    let book = point(px(20.0), column.top() + px(4.0));
    double_press(&mut window, book);
    assert_eq!(taken(&requests), [TitleBarRequest::DoublePress]);
    // The button of the menu of the server lies in the header of the channels.
    let menu = bounds(&mut window, "server-menu-button").center();
    drag(&mut window, menu, menu + point(px(30.0), px(0.0)));
    assert_eq!(taken(&requests), []);
    window.press("escape");
    double_press(&mut window, menu);
    assert_eq!(taken(&requests), []);
}
