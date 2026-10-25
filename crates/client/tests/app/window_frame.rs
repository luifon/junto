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

fn drawn() -> DrawnFrameInTests {
    DrawnFrameInTests {
        drawn: true,
        ..DrawnFrameInTests::default()
    }
}

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
    cx.update(|cx| cx.set_global(drawn()));
    let (_directory, mut window) = named(cx, "Luca");
    assert_strip(&mut window);
    // A dialog covers the columns and leaves the strip and its buttons free.
    window.click("app-settings");
    assert!(window.shows("scale-4"));
    assert_strip(&mut window);
}

#[gpui::test]
fn without_a_drawn_frame_the_columns_reach_the_top(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(DrawnFrameInTests::default()));
    let (_directory, mut window) = named(cx, "Luca");
    assert!(!window.shows("title-strip") && !window.shows("window-close"));
    assert_eq!(bounds(&mut window, "channel-column").top(), px(0.0));
}

/// On Linux the application handles the strip; on Windows the system does, by the areas
/// that gpui reports to its hit test.
#[cfg(not(windows))]
#[gpui::test]
fn the_strip_of_a_drawn_frame_minimizes_zooms_moves_and_closes_the_window(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(drawn()));
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

/// The strip stays usable under an open dialog: the dialog covers the columns, not the strip.
#[cfg(not(windows))]
#[gpui::test]
fn the_strip_works_while_a_dialog_is_open(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(drawn()));
    let requests = record_requests(cx);
    let (_directory, mut window) = named(cx, "Luca");
    window.click("app-settings");
    assert!(window.shows("scale-4"));
    window.click("window-minimize");
    assert_eq!(taken(&requests), [TitleBarRequest::Minimize]);
    let title = bounds(&mut window, "title-strip-drag").center();
    drag(&mut window, title, title + point(px(40.0), px(0.0)));
    assert_eq!(taken(&requests), [TitleBarRequest::Move]);
    assert!(
        window.shows("scale-4"),
        "a press on the strip closed the dialog"
    );
}

/// A press in each band inside an edge resizes the window from that edge; an edge that the
/// compositor tiled has no band, and the right press on the strip opens the window menu.
#[cfg(not(windows))]
#[gpui::test]
fn the_bands_of_a_drawn_frame_resize_the_window_except_at_a_tiled_edge(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(DrawnFrameInTests {
            tiling: gpui::Tiling {
                left: true,
                ..gpui::Tiling::default()
            },
            ..drawn()
        })
    });
    let requests = record_requests(cx);
    let (_directory, mut window) = named(cx, "Luca");
    for (selector, edge) in [
        ("resize-Top", gpui::ResizeEdge::Top),
        ("resize-Bottom", gpui::ResizeEdge::Bottom),
        ("resize-Right", gpui::ResizeEdge::Right),
        ("resize-TopRight", gpui::ResizeEdge::TopRight),
        ("resize-BottomRight", gpui::ResizeEdge::BottomRight),
    ] {
        let band = bounds(&mut window, selector);
        press(&mut window, band.center(), 1);
        assert_eq!(
            taken(&requests),
            [TitleBarRequest::Resize(edge)],
            "{selector}"
        );
    }
    for selector in ["resize-Left", "resize-TopLeft", "resize-BottomLeft"] {
        assert!(
            !window.shows(selector),
            "{selector} is drawn at a tiled edge"
        );
    }
    let title = bounds(&mut window, "title-strip-drag").center();
    window.press_at(title, MouseButton::Right);
    assert_eq!(taken(&requests), [TitleBarRequest::WindowMenu]);
}

#[cfg(not(windows))]
#[gpui::test]
fn a_maximized_window_has_no_resize_bands(cx: &mut TestAppContext) {
    cx.update(|cx| {
        cx.set_global(DrawnFrameInTests {
            maximized: true,
            ..drawn()
        })
    });
    let (_directory, mut window) = named(cx, "Luca");
    assert!(window.shows("title-strip"));
    for selector in ["resize-Top", "resize-Left", "resize-BottomRight"] {
        assert!(
            !window.shows(selector),
            "{selector} is drawn in a maximized window"
        );
    }
}

/// In the main window on macOS a dialog header is no title bar, and a band at the top over an
/// open layer moves the window and takes its double press without closing the layer.
#[gpui::test]
fn over_a_layer_the_top_band_is_the_title_bar_and_a_dialog_header_is_not(cx: &mut TestAppContext) {
    cx.update(|cx| cx.set_global(DrawnFrameInTests::default()));
    let requests = record_requests(cx);
    let (_directory, mut window) = named(cx, "Luca");
    window
        .cx
        .update(|window, cx| content_reaches_title_bar(window, cx));
    window.click("app-settings");
    assert!(window.shows("scale-4"));
    let band = bounds(&mut window, "title-band");
    assert_eq!(band.top(), px(0.0));
    let dialog = bounds(&mut window, "settings-dialog");
    assert!(
        dialog.top() >= band.bottom(),
        "the dialog lies under the band"
    );
    let header = dialog.origin + point(px(30.0), px(20.0));
    double_press(&mut window, header);
    drag(&mut window, header, header + point(px(30.0), px(0.0)));
    assert_eq!(taken(&requests), []);
    let middle = band.center();
    double_press(&mut window, middle);
    assert_eq!(taken(&requests), [TitleBarRequest::DoublePress]);
    drag(&mut window, middle, middle + point(px(30.0), px(0.0)));
    assert_eq!(taken(&requests), [TitleBarRequest::Move]);
    assert!(
        window.shows("scale-4"),
        "a press on the band closed the dialog"
    );
}

/// A press that closes a menu and whose release the window never gets, as a press on the title
/// bar of Windows that moves the window, does not keep the next press on the menu button from
/// opening the menu.
#[gpui::test]
fn a_press_without_a_release_does_not_keep_a_menu_closed(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Menus");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    window.click("server-menu-button");
    assert!(window.shows("server-menu"));
    let width = window.cx.update(|window, _| window.viewport_size().width);
    let outside = point(width - px(40.0), px(300.0));
    window.dispatch(MouseDownEvent {
        button: MouseButton::Left,
        position: outside,
        modifiers: Modifiers::none(),
        click_count: 1,
        first_mouse: false,
    });
    assert!(!window.shows("server-menu"));
    window.click("server-menu-button");
    assert!(window.shows("server-menu"), "the menu did not open again");
}
