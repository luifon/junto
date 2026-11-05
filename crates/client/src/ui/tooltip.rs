//! The text that appears next to a control while the pointer rests on it. A control with an
//! icon and no text carries the same text as its accessible label.

use gpui::{
    AnyView, App, AppContext as _, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, Window, div,
};

use super::{StyleExt, float_shadow};
use crate::theme::{theme, u};

struct Tooltip {
    text: SharedString,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        tooltip_box(self.text.clone(), cx).mt(u(6.0))
    }
}

/// The box of a tooltip with its text.
fn tooltip_box(text: SharedString, cx: &App) -> gpui::Div {
    let theme = theme(cx);
    let colors = theme.colors;
    div()
        .max_w(u(340.0))
        .px(u(10.0))
        .pt(u(7.0))
        .pb(u(8.0))
        .bg(Hsla::from(colors.travertine_hi))
        .border_1()
        .border_color(Hsla::from(colors.rule))
        .rounded(u(theme.metrics.radius))
        .shadow(float_shadow(colors.shadow, 6.0, 16.0))
        .typeface(theme.types.small, &theme)
        .ink(colors.ink_2)
        .child(text)
}

/// The tooltip of an element that holds the keyboard focus, drawn above the element over
/// the window content, for a person who does not use a pointer. It moves no control.
pub fn focused_tooltip(text: impl Into<SharedString>, cx: &App) -> impl IntoElement {
    gpui::deferred(
        gpui::anchored()
            .anchor(gpui::Anchor::BottomLeft)
            .snap_to_window()
            .child(
                div()
                    .pb(u(6.0))
                    .debug_selector(|| "focused-tooltip".to_owned())
                    .child(tooltip_box(text.into(), cx)),
            ),
    )
}

/// Builds the tooltip of a control.
pub fn tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let text = text.into();
    move |_, cx| {
        let text = text.clone();
        cx.new(|_| Tooltip { text }).into()
    }
}
