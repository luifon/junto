use crate::{
    text,
    theme::{theme, u},
};
use gpui::{
    App, Hsla, InteractiveElement, IntoElement, ParentElement, RenderOnce, Rgba, Role,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use junto_client_core::protocol::presence::Presence;

#[derive(IntoElement)]
pub struct PresenceMark {
    presence: Presence,
    size: f32,
    background: Option<Rgba>,
}
impl PresenceMark {
    pub fn color(presence: Presence, theme: &crate::theme::Theme) -> Rgba {
        let colors = theme.colors;
        match presence {
            Presence::Online => colors.cypress_ink,
            Presence::Idle => colors.ochre,
            Presence::DoNotDisturb => colors.cinnabar,
            Presence::Offline | Presence::Invisible => colors.ink_3,
        }
    }
    pub fn new(presence: Presence, size: f32) -> Self {
        Self {
            presence,
            size,
            background: None,
        }
    }
    pub fn background(mut self, color: Rgba) -> Self {
        self.background = Some(color);
        self
    }
}
impl RenderOnce for PresenceMark {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let background = self.background.unwrap_or(colors.travertine);
        let color = Self::color(self.presence, &theme);
        let off = matches!(self.presence, Presence::Offline | Presence::Invisible);
        div()
            .id("presence-mark")
            .debug_selector(move || format!("presence-state:{:?}", self.presence))
            .role(Role::Image)
            .aria_label(text::presence::label(self.presence))
            .flex_none()
            .relative()
            .size(u(self.size))
            .rounded_full()
            .bg(Hsla::from(background))
            .p(u(1.))
            .child(
                div()
                    .relative()
                    .size_full()
                    .rounded_full()
                    .overflow_hidden()
                    .bg(Hsla::from(if off { background } else { color }))
                    .when(off, |dot| dot.border_1().border_color(Hsla::from(color)))
                    .when(self.presence == Presence::Idle, |dot| {
                        dot.child(
                            div()
                                .absolute()
                                .left(u(self.size * 0.4))
                                .top(u(-self.size * 0.2))
                                .size(u(self.size * 0.75))
                                .rounded_full()
                                .bg(Hsla::from(background)),
                        )
                    })
                    .when(self.presence == Presence::DoNotDisturb, |dot| {
                        dot.child(
                            div()
                                .absolute()
                                .left(u(self.size * 0.15))
                                .top(u(self.size * 0.33))
                                .w(u(self.size * 0.5))
                                .h(u(1.5))
                                .bg(Hsla::from(background)),
                        )
                    }),
            )
    }
}
