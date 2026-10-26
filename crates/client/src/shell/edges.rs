//! The edges of the panels beside the main area. The person drags an edge, or moves it with
//! the arrow keys while it holds the keyboard focus, to make its panel wider or narrower
//! within the bounds of the panel; the width is kept in the settings (ADR 0703).

use gpui::{
    AnyElement, App, Context, CursorStyle, DispatchPhase, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement,
    Pixels, Role, StatefulInteractiveElement, Styled, Window, canvas, div, prelude::FluentBuilder,
    px,
};
use junto_client_core::settings::Panel;

use super::Shell;
use crate::text;
use crate::theme::{REM_AT_100, theme, u};

/// The side of its panel at which an edge is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    /// A panel at the left of the main area: its right edge moves.
    Right,
    /// A panel at the right of the main area: its left edge moves.
    Left,
}

/// A drag of an edge that is on its way.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Drag {
    panel: Panel,
    side: Side,
    from: Pixels,
    start: f32,
    width: f32,
}

/// The width of the place where a press takes an edge, in pixels of the design.
const GRIP: f32 = 8.0;
/// The change of the width at one press of an arrow key, in pixels of the design.
const STEP: f32 = 16.0;
/// The least width of the main area beside the panels, in pixels of the design.
const MAIN_LEAST: f32 = 360.0;
/// The least width of the stage of a separate call window beside its panel.
const STAGE_LEAST: f32 = 240.0;

pub(crate) fn selector(panel: Panel) -> &'static str {
    match panel {
        Panel::ServerBook => "edge-server-book",
        Panel::Channels => "edge-channels",
        Panel::Thread => "edge-thread",
        Panel::Pins => "edge-pins",
        Panel::Notices => "edge-notices",
        Panel::Search => "edge-search",
        Panel::Members => "edge-members",
        Panel::Transcript => "edge-transcript",
        Panel::Call => "edge-call",
        Panel::CallWindow => "edge-call-window",
    }
}

fn bounds(panel: Panel) -> (f32, f32) {
    let (least, most) = panel.bounds();
    (f32::from(least), f32::from(most))
}

impl Shell {
    /// The width of a panel in pixels of the design: the width of a drag on its way, the
    /// width that the person gave it, or the width of the design.
    pub(crate) fn panel_width(&self, panel: Panel, design: f32, cx: &App) -> f32 {
        let width = match self.edge_drag {
            Some(drag) if drag.panel == panel => drag.width,
            _ => self.state.read(cx).panel_width(panel, design),
        };
        width.min(self.panel_room(panel, cx)).max(design.min(width))
    }

    /// The widest that a panel may be in the window as it is now, so that the main area
    /// keeps [`MAIN_LEAST`] pixels of the design (and the stage of a separate call window
    /// [`STAGE_LEAST`]). A panel is never narrower than its width of the design for it.
    fn panel_room(&self, panel: Panel, cx: &App) -> f32 {
        let window = self.window_width;
        let stored = |panel, design| self.state.read(cx).panel_width(panel, design);
        let metrics = theme(cx).metrics;
        let columns = self.rail_width
            + stored(Panel::Channels, metrics.channel_column_width).min(window * 0.35);
        match panel {
            Panel::ServerBook => window * 0.25,
            Panel::Channels => window * 0.35,
            Panel::CallWindow => window - STAGE_LEAST,
            _ => window - columns - MAIN_LEAST,
        }
    }

    /// The edge of `panel`, to lie over the container of the panel at `at` pixels of the
    /// design from the side `side` of the container. The edge is the last child of the
    /// container, so that a press there reaches it before the panel and the main area.
    pub(crate) fn panel_edge(
        &mut self,
        panel: Panel,
        side: Side,
        at: f32,
        design: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme(cx).colors;
        let focus = self
            .edge_focus
            .entry(panel)
            .or_insert_with(|| cx.focus_handle())
            .clone();
        let dragged = self.edge_drag.is_some_and(|drag| drag.panel == panel);
        let focused = focus.is_focused(window);
        let width = self.panel_width(panel, design, cx);
        div()
            .id(selector(panel))
            .debug_selector(move || selector(panel).to_owned())
            .role(Role::Splitter)
            .aria_label(text::panel_edge(panel))
            .track_focus(&focus)
            .absolute()
            .top_0()
            .bottom_0()
            .w(u(GRIP))
            .map(|edge| match side {
                Side::Right => edge.left(u(at - GRIP / 2.0)),
                Side::Left => edge.right(u(at - GRIP / 2.0)),
            })
            .flex()
            .justify_center()
            .cursor(CursorStyle::ResizeLeftRight)
            .child(
                div()
                    .w(px(2.0))
                    .h_full()
                    .when(dragged || focused, |line| {
                        line.bg(Hsla::from(colors.terracotta))
                    })
                    .hover(|line| line.bg(Hsla::from(colors.bronze))),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.edge_drag = Some(Drag {
                        panel,
                        side,
                        from: event.position.x,
                        start: width,
                        width,
                    });
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                let change = match (event.keystroke.key.as_str(), side) {
                    ("left", Side::Right) | ("right", Side::Left) => -STEP,
                    ("right", Side::Right) | ("left", Side::Left) => STEP,
                    _ => return,
                };
                let (least, most) = bounds(panel);
                let width = (this.panel_width(panel, design, cx) + change).clamp(least, most);
                this.state
                    .update(cx, |state, cx| state.set_panel_width(panel, width, cx));
                cx.stop_propagation();
                cx.notify();
            }))
            .into_any_element()
    }

    /// While an edge is dragged: follows the pointer in the whole window, also outside the
    /// edge, and keeps the width at the release.
    pub(crate) fn edge_drag_tracking(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.edge_drag?;
        let shell = cx.entity().downgrade();
        let released = shell.clone();
        Some(
            canvas(
                |_, _, _| {},
                move |_, (), window, _| {
                    let shell = shell.clone();
                    window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                        if phase == DispatchPhase::Capture {
                            let rem = window.rem_size();
                            let _ = shell.update(cx, |this, cx| {
                                this.drag_edge_to(event.position.x, rem, cx);
                            });
                        }
                    });
                    let released = released.clone();
                    window.on_mouse_event(move |_: &MouseUpEvent, phase, _, cx| {
                        if phase == DispatchPhase::Capture {
                            let _ = released.update(cx, |this, cx| this.end_edge_drag(cx));
                        }
                    });
                },
            )
            .absolute()
            .size_0()
            .into_any_element(),
        )
    }

    fn drag_edge_to(&mut self, x: Pixels, rem: Pixels, cx: &mut Context<Self>) {
        let Some(drag) = self.edge_drag.as_mut() else {
            return;
        };
        let moved = (x - drag.from) / rem * REM_AT_100;
        let width = match drag.side {
            Side::Right => drag.start + moved,
            Side::Left => drag.start - moved,
        };
        let (least, most) = bounds(drag.panel);
        drag.width = width.clamp(least, most);
        cx.notify();
    }

    fn end_edge_drag(&mut self, cx: &mut Context<Self>) {
        if let Some(drag) = self.edge_drag.take() {
            self.state.update(cx, |state, cx| {
                state.set_panel_width(drag.panel, drag.width, cx);
            });
            cx.notify();
        }
    }
}
