//! Scrolls the control that takes the keyboard focus into the view of the scrolled area
//! that holds it, such as the content of a dialog (ADR 0731). gpui does not tell an
//! element where the focused element is drawn, so each focusable component of `ui`
//! reports its bounds while it holds the focus, and the area reads the report of its
//! content after the content.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{
    App, Bounds, ElementId, FocusHandle, Global, IntoElement, Pixels, ScrollHandle, Styled,
    WeakFocusHandle, Window, WindowId, canvas, point, px,
};

use crate::theme::u;

/// The space around a control that is shown with it, in pixels of the design: its border
/// and the mark of the keyboard focus.
const MARGIN: f32 = 4.0;

/// The areas whose content is being drawn, by window, from the outermost to the
/// innermost, each with the bounds of the focused control in it.
#[derive(Default)]
struct Areas(HashMap<WindowId, Vec<Option<Bounds<Pixels>>>>);

impl Global for Areas {}

fn window_id(window: &Window) -> WindowId {
    window.window_handle().window_id()
}

/// The focus handle of the control `id`, kept for as long as the control is drawn in
/// consecutive frames, as gpui keeps the handle of a focusable element. The control
/// tracks it and gives it to [`report`].
pub(crate) fn handle(id: &ElementId, window: &mut Window, cx: &mut App) -> FocusHandle {
    window.with_global_id(id.clone(), |global_id, window| {
        window.with_element_state(global_id, |held: Option<FocusHandle>, _| {
            let handle = held.unwrap_or_else(|| cx.focus_handle().tab_stop(true));
            (handle.clone(), handle)
        })
    })
}

/// The focus handle of the control `id` inside the element `outer`, such as one segment
/// of a segmented control.
pub(crate) fn inner_handle(
    outer: &ElementId,
    id: &ElementId,
    window: &mut Window,
    cx: &mut App,
) -> FocusHandle {
    window.with_global_id(outer.clone(), |_, window| handle(id, window, cx))
}

/// An element that covers its parent, a control, and reports the bounds of the control to
/// the innermost area while `focus` is focused.
pub(crate) fn report(focus: &FocusHandle) -> impl IntoElement + use<> {
    let focus = focus.clone();
    canvas(
        move |bounds, window, cx| {
            if !focus.is_focused(window) {
                return;
            }
            if !cx.has_global::<Areas>() {
                return;
            }
            let id = window_id(window);
            if let Some(innermost) = cx
                .global_mut::<Areas>()
                .0
                .get_mut(&id)
                .and_then(|areas| areas.last_mut())
            {
                *innermost = Some(bounds);
            }
        },
        |_, (), _, _| {},
    )
    .absolute()
    .inset_0()
}

/// The first child of the content of an area: the area takes the reports of the controls
/// after it.
pub(crate) fn start() -> impl IntoElement {
    canvas(
        |_, window, cx| {
            let id = window_id(window);
            cx.default_global::<Areas>()
                .0
                .entry(id)
                .or_default()
                .push(None);
        },
        |_, (), _, _| {},
    )
    .absolute()
    .size_0()
}

/// What an area remembers between frames: the control that it brought into view last.
#[derive(Default)]
pub(crate) struct Revealed(Option<WeakFocusHandle>);

/// The last child of the content of an area that `scroll` scrolls. When the focus has
/// moved to a control of the content that lies outside the view, the area scrolls by the
/// least distance that shows the control, and by none when the person scrolls later.
pub(crate) fn end(scroll: &ScrollHandle, revealed: Rc<RefCell<Revealed>>) -> impl IntoElement {
    let scroll = scroll.clone();
    canvas(
        move |_, window, cx| {
            let id = window_id(window);
            let reported = cx
                .default_global::<Areas>()
                .0
                .get_mut(&id)
                .and_then(Vec::pop)
                .flatten();
            let mut held = revealed.borrow_mut();
            // The focus is outside the content: the next control that takes it is shown.
            let Some(control) = reported else {
                held.0 = None;
                return;
            };
            let focused = window.focused(cx);
            if held.0.as_ref().and_then(WeakFocusHandle::upgrade) == focused {
                return;
            }
            held.0 = focused.as_ref().map(FocusHandle::downgrade);
            // The control is shown with its border and its focus mark around it: the report
            // gives the box inside the border.
            let control = control.dilate(u(MARGIN).to_pixels(window.rem_size()));
            let view = scroll.bounds();
            let offset = scroll.offset();
            let shift = if control.top() < view.top() || control.size.height > view.size.height {
                view.top() - control.top()
            } else if control.bottom() > view.bottom() {
                view.bottom() - control.bottom()
            } else {
                return;
            };
            let lowest = -scroll.max_offset().y;
            let y = (offset.y + shift).clamp(lowest, px(0.0));
            let scroll = scroll.clone();
            window.on_next_frame(move |window, _| {
                scroll.set_offset(point(offset.x, y));
                window.refresh();
            });
        },
        |_, (), _, _| {},
    )
    .absolute()
    .size_0()
}
