use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, Styled, div,
};

use super::Shell;
use crate::{
    text,
    theme::u,
    ui::{Button, Notice},
};

impl Shell {
    pub(crate) fn render_message_link_refusal(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let sentence = self.state.read(cx).message_link_refusal()?;
        Some(
            div()
                .absolute()
                .left(u(16.0))
                .right(u(16.0))
                .top(u(48.0))
                .flex()
                .justify_center()
                .child(
                    div()
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .max_w(u(640.0))
                        .child(
                            Notice::error(sentence)
                                .selector("message-link-refusal")
                                .action(
                                    Button::new("dismiss-message-link-refusal")
                                        .label(text::DISMISS)
                                        .selector("dismiss-message-link-refusal")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.state.update(cx, |state, cx| {
                                                state.dismiss_message_link_refusal(cx)
                                            });
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
