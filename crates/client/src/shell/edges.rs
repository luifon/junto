//! The edges of the panels beside the main area. The person drags an edge, or moves it with
//! the arrow keys while it holds the keyboard focus, to make its panel wider or narrower
//! within the bounds of the panel; the width is kept in the settings (ADR 0703).

use gpui::{
    AnyElement, App, Context, CursorStyle, DispatchPhase, Hsla, InteractiveElement, IntoElement,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Role, StatefulInteractiveElement, Styled, Window, canvas, div, prelude::FluentBuilder, px,
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
    /// The width of the design of the panel, which [`Shell::panel_width`] takes.
    design: f32,
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

/// A width that one panel is given in place of the width that the person keeps for it: the
/// width of a drag on its way, or a width that a drag or a key is about to keep.
type Wanted = Option<(Panel, f32)>;

impl Shell {
    fn dragged(&self) -> Wanted {
        self.edge_drag.map(|drag| (drag.panel, drag.width))
    }

    /// The width that the person wants for a panel in pixels of the design: the width of
    /// `wanted` for its panel, the width that the person gave it, or the width of the design.
    fn wanted_width(&self, wanted: Wanted, panel: Panel, design: f32, cx: &App) -> f32 {
        match wanted {
            Some((dragged, width)) if dragged == panel => width,
            _ => self.state.read(cx).panel_width(panel, design),
        }
    }

    /// The widths of the server book and of the channel column that the window draws, in
    /// pixels of the design. Together they leave the main area [`MAIN_LEAST`]: where the
    /// wanted widths do not, the channel column and then the server book give up width down
    /// to their least bounds (review M2).
    fn column_widths(&self, wanted: Wanted, cx: &App) -> (f32, f32) {
        let metrics = theme(cx).metrics;
        let mut book = if self.book_collapsed {
            metrics.rail_collapsed_width
        } else {
            self.wanted_width(wanted, Panel::ServerBook, metrics.rail_width, cx)
        };
        let mut channels =
            self.wanted_width(wanted, Panel::Channels, metrics.channel_column_width, cx);
        let mut over = book + channels - (self.window_width - MAIN_LEAST);
        let give = (channels - bounds(Panel::Channels).0).clamp(0.0, over.max(0.0));
        channels -= give;
        over -= give;
        if !self.book_collapsed {
            book -= (book - bounds(Panel::ServerBook).0).clamp(0.0, over.max(0.0));
        }
        (book, channels)
    }

    /// The room of a panel at the right of the main area, or of the panel of the call window,
    /// in pixels of the design: the window less the columns and the least width of the main
    /// area, or of the stage of the call window ([`STAGE_LEAST`]).
    fn side_room(&self, wanted: Wanted, panel: Panel, cx: &App) -> f32 {
        match panel {
            Panel::CallWindow => self.window_width - STAGE_LEAST,
            _ => {
                let (book, channels) = self.column_widths(wanted, cx);
                self.window_width - book - channels - MAIN_LEAST
            }
        }
    }

    /// The width of a panel in pixels of the design. The columns at the left follow
    /// [`Shell::column_widths`]. A panel at the right takes the width that the person wants
    /// within its room, and gives up width down to its least bound; where even that does not
    /// fit, it lies over the content ([`Shell::side_fits`]) at that least width or the width
    /// of its design, whichever is smaller than the width wanted.
    pub(crate) fn panel_width(&self, panel: Panel, design: f32, cx: &App) -> f32 {
        self.panel_width_with(self.dragged(), panel, design, cx)
    }

    fn panel_width_with(&self, wanted: Wanted, panel: Panel, design: f32, cx: &App) -> f32 {
        let (book, channels) = self.column_widths(wanted, cx);
        match panel {
            Panel::ServerBook => book,
            Panel::Channels => channels,
            _ => {
                let width = self.wanted_width(wanted, panel, design, cx);
                let least = bounds(panel).0;
                if self.side_fits_with(wanted, panel, cx) {
                    width.min(self.side_room(wanted, panel, cx)).max(least)
                } else {
                    width.min(design)
                }
            }
        }
    }

    /// The width that the window draws for `panel` when the person wants `width` for it,
    /// within the bounds of the panel. A drag or a key keeps this width and not a wider one
    /// that the room of the window does not show, so a later wider window draws the panel as
    /// it was seen.
    fn shown_width(&self, panel: Panel, width: f32, design: f32, cx: &App) -> f32 {
        let (least, most) = bounds(panel);
        let width = width.clamp(least, most);
        self.panel_width_with(Some((panel, width)), panel, design, cx)
            .clamp(least, width)
    }

    /// A panel at the right fits beside the main area at its least width; otherwise the window
    /// draws it over the content, as a narrow window does.
    pub(crate) fn side_fits(&self, panel: Panel, cx: &App) -> bool {
        self.side_fits_with(self.dragged(), panel, cx)
    }

    fn side_fits_with(&self, wanted: Wanted, panel: Panel, cx: &App) -> bool {
        self.side_room(wanted, panel, cx) >= bounds(panel).0
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
        let pressed = focus.clone();
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
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    // A press gives the edge the keyboard focus, as on a control.
                    window.focus(&pressed, cx);
                    this.edge_drag = Some(Drag {
                        panel,
                        side,
                        from: event.position.x,
                        start: width,
                        width,
                        design,
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
                let shown = this.panel_width(panel, design, cx);
                let width = this.shown_width(panel, shown + change, design, cx);
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
        let Some(drag) = self.edge_drag else {
            return;
        };
        let moved = (x - drag.from) / rem * REM_AT_100;
        let width = match drag.side {
            Side::Right => drag.start + moved,
            Side::Left => drag.start - moved,
        };
        let shown = self.shown_width(drag.panel, width, drag.design, cx);
        if let Some(drag) = self.edge_drag.as_mut() {
            drag.width = shown;
        }
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
