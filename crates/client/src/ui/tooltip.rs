//! The text that appears next to a control while the pointer rests on it. A control with an
//! icon and no text carries the same text as its accessible label.

use gpui::{
    AnyView, App, AppContext as _, Context, Hsla, IntoElement, ParentElement, Render, SharedString,
    Styled, Window, div,
};

use super::{StyleExt, float_shadow};
use crate::theme::{theme, u};

struct Tooltip {
    text: SharedString,
}

impl Render for Tooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        div()
            .mt(u(6.0))
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
            .child(self.text.clone())
    }
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
