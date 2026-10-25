//! A panic in an event handler of the window (ADR 0757): the window continues, the report of
//! the panic is in the data directory, and the window states it.

use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};
use junto_client::panics;
use junto_client::shell::Pane;

use crate::harness::{World, member};
use crate::messages::document;

/// The scroll of the message list panics in the listener of the shell while the list holds
/// its own state, as the scroll of the first live test did. The panic does not reach the
/// test: the window keeps its list, its listeners and its controls.
#[gpui::test]
fn a_panic_in_the_scroll_of_the_message_list_is_reported_and_the_window_continues(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    for number in 0..40 {
        admin.send(&world, general, document(&format!("Line {number}")));
    }
    window.until_message("Line 39");
    window.settle();
    let list = window
        .in_shell(|shell, _, _| shell.feed_bounds(Pane::Main))
        .expect("the list was drawn");
    // The event goes through the input callback of the test window, as a platform gives it,
    // and not through an update of the window that the test makes.
    let wheel = |window: &mut crate::harness::Window, lines: f32| {
        window.simulate(|cx| {
            cx.simulate_event(ScrollWheelEvent {
                position: list.center(),
                delta: ScrollDelta::Pixels(point(px(0.0), px(lines))),
                ..Default::default()
            })
        });
        window.settle();
    };
    let before = panics::panics_on_this_thread();
    window.expect_panics(1);
    panics::test_support::panic_in_the_next(panics::test_support::Handler::FeedScroll);
    wheel(&mut window, 2000.0);
    assert_eq!(panics::panics_on_this_thread(), before + 1);

    let data_dir = window.state.read_with(&window.cx, |state, _| {
        state.client().data_dir().root().to_owned()
    });
    let reports: Vec<_> = std::fs::read_dir(panics::reports_directory(&data_dir))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(reports.len(), 1, "{reports:?}");
    let report = std::fs::read_to_string(&reports[0]).unwrap();
    for part in [
        "Junto panic report",
        "version: ",
        "thread: ",
        "location: ",
        &format!("message: {}", panics::test_support::MESSAGE),
        "backtrace:",
        "feed_scrolled",
    ] {
        assert!(
            report.contains(part),
            "{part:?} is not in the report:\n{report}"
        );
    }

    window.until("the window states the panic", |window| {
        window.shows("panic-notice")
    });
    let notice = window
        .state
        .read_with(&window.cx, |state, _| state.panic_notice().cloned())
        .expect("the state holds the notice");
    assert_eq!(notice.count, 1);
    assert_eq!(notice.latest.file.as_ref(), Some(&reports[0]));
    assert_eq!(notice.latest.message, panics::test_support::MESSAGE);

    // The list scrolls and its listener follows it.
    wheel(&mut window, 2000.0);
    assert!(!window.in_shell(|shell, _, _| shell.feed_is_at_end(Pane::Main)));
    wheel(&mut window, -20000.0);
    window.until("the list is at its end again", |window| {
        window.in_shell(|shell, _, _| shell.feed_is_at_end(Pane::Main))
    });
    // A control of the window works.
    window.click("close-panic-notice");
    window.until("the notice is closed", |window| {
        !window.shows("panic-notice")
    });
    assert_eq!(panics::panics_on_this_thread(), before + 1);
}

/// Starts a draw of every window from a task of the application, which the platform runs
/// from its own callback, after `arm` has run in the task.
fn draw_from_a_task(window: &mut crate::harness::Window, arm: fn()) {
    window.cx.update(|_, cx| {
        cx.spawn(async move |cx| {
            arm();
            cx.update(|cx| cx.refresh_windows());
        })
        .detach();
    });
    window.park();
}

/// A render of the shell panics once inside a draw, while the elements of the window have
/// pushed their ids, offsets and masks. The window draws again: the notice of the panic and
/// a message sent after it are drawn, and no other panic follows.
#[gpui::test]
fn a_panic_in_a_render_is_reported_and_the_window_draws_again(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    for number in 0..5 {
        admin.send(&world, general, document(&format!("Line {number}")));
    }
    window.until_message("Line 4");
    window.settle();
    let before = panics::panics_on_this_thread();
    window.expect_panics(1);
    draw_from_a_task(&mut window, || {
        panics::test_support::panic_in_the_next(panics::test_support::Handler::Render)
    });
    assert_eq!(panics::panics_on_this_thread(), before + 1);
    window.until("the window states the panic", |window| {
        window.shows("panic-notice")
    });
    admin.send(&world, general, document("Line after the panic"));
    window.until_message("Line after the panic");
    window.settle();
    assert!(window.in_shell(|shell, _, _| shell.feed_is_at_end(Pane::Main)));
    assert_eq!(panics::panics_on_this_thread(), before + 1);
}

/// A render of the shell panics at every call. The window stops drawing after three failed
/// draws, also when the notice of the panic changes the state; a press of the person allows
/// another draw, which shows the notice once the render works again.
#[gpui::test]
fn a_render_that_panics_at_every_call_does_not_draw_at_every_frame(cx: &mut TestAppContext) {
    use gpui::{MouseButton, MouseDownEvent, MouseUpEvent};
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.settle();
    let before = panics::panics_on_this_thread();
    window.expect_panics(3);
    draw_from_a_task(&mut window, || {
        panics::test_support::panic_in_every(panics::test_support::Handler::Render)
    });
    for _ in 0..20 {
        window.frame();
        window.park();
    }
    assert_eq!(panics::panics_on_this_thread(), before + 3);
    assert!(!window.shows("panic-notice"));

    panics::test_support::stop_panicking();
    let list = window
        .in_shell(|shell, _, _| shell.feed_bounds(Pane::Main))
        .expect("the list was drawn");
    window.simulate(|cx| {
        cx.simulate_event(MouseDownEvent {
            button: MouseButton::Left,
            position: list.center(),
            click_count: 1,
            ..Default::default()
        })
    });
    window.simulate(|cx| {
        cx.simulate_event(MouseUpEvent {
            button: MouseButton::Left,
            position: list.center(),
            click_count: 1,
            ..Default::default()
        })
    });
    window.until("the window states the panics", |window| {
        window.shows("panic-notice")
    });
    let notice = window
        .state
        .read_with(&window.cx, |state, _| state.panic_notice().cloned())
        .expect("the state holds the notice");
    assert_eq!(notice.count, 3);
    assert_eq!(notice.latest.occurrence, 3);
    assert_eq!(panics::panics_on_this_thread(), before + 3);
}
