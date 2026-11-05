//! A press whose control moves away from the pointer between the press and the release,
//! because the window draws again with content above it that grew (ADR 0757). The view of
//! the test uses gpui alone: a spacer above a button, whose height the test changes.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    InteractiveElement as _, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, TestAppContext,
    VisualTestContext, div, point, px,
};

use crate::harness::{open_view, view_input, view_redraw};

struct Moving {
    spacer: Rc<Cell<f32>>,
    clicks: Rc<Cell<u32>>,
}

impl gpui::Render for Moving {
    fn render(
        &mut self,
        _: &mut gpui::Window,
        _: &mut gpui::Context<Self>,
    ) -> impl gpui::IntoElement {
        let clicks = self.clicks.clone();
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(div().h(px(self.spacer.get())))
            .child(
                div()
                    .id("moving-button")
                    .w(px(100.0))
                    .h(px(30.0))
                    .on_click(move |_, _, _| clicks.set(clicks.get() + 1)),
            )
    }
}

fn moving(cx: &mut TestAppContext) -> (VisualTestContext, Rc<Cell<f32>>, Rc<Cell<u32>>) {
    let (spacer, clicks) = (Rc::new(Cell::new(0.0)), Rc::new(Cell::new(0)));
    let view = Moving {
        spacer: spacer.clone(),
        clicks: clicks.clone(),
    };
    (open_view(cx, move |_, _| view), spacer, clicks)
}

/// The spacer grows by `by` pixels, and the window draws again, as after a change of the
/// state that arrives during the press.
fn grow(cx: &mut VisualTestContext, spacer: &Cell<f32>, by: f32) {
    spacer.set(spacer.get() + by);
    view_redraw(cx);
}

fn down(cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>) {
    view_input(
        cx,
        MouseMoveEvent {
            position: at,
            pressed_button: None,
            modifiers: gpui::Modifiers::none(),
        },
    );
    view_input(
        cx,
        MouseDownEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: gpui::Modifiers::none(),
            click_count: 1,
            first_mouse: false,
        },
    );
}

fn up(cx: &mut VisualTestContext, at: gpui::Point<gpui::Pixels>) {
    view_input(
        cx,
        MouseUpEvent {
            button: MouseButton::Left,
            position: at,
            modifiers: gpui::Modifiers::none(),
            click_count: 1,
        },
    );
}

#[gpui::test]
fn a_press_is_delivered_when_a_redraw_moves_its_control_before_the_release(
    cx: &mut TestAppContext,
) {
    let (mut cx, spacer, clicks) = moving(cx);
    let at = point(px(50.0), px(10.0));
    down(&mut cx, at);
    grow(&mut cx, &spacer, 20.0);
    up(&mut cx, at);
    assert_eq!(
        clicks.get(),
        1,
        "the press ends on the control that it started on"
    );

    // A press that is released outside both the place where the control was at the press
    // and the place where it is now is no click.
    down(&mut cx, point(px(50.0), px(30.0)));
    grow(&mut cx, &spacer, 40.0);
    up(&mut cx, point(px(50.0), px(100.0)));
    assert_eq!(clicks.get(), 1);

    // A press that ends where it started, on a control that did not move, is one click.
    let on = point(px(50.0), px(75.0));
    down(&mut cx, on);
    up(&mut cx, on);
    assert_eq!(clicks.get(), 2);
}
