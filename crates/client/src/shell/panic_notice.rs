//! The notice of a panic that the application stopped (ADR 0757): what happened and where
//! its report is, over the bottom of the window until the person closes it.

use gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, div};

use super::Shell;
use crate::text;
use crate::theme::u;
use crate::ui::{Button, Notice};

impl Shell {
    pub(crate) fn render_panic_notice(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let notice = self.state.read(cx).panic_notice()?;
        let sentence = text::panic_notice(notice.count, notice.latest.file.as_deref());
        Some(
            div()
                .absolute()
                .left(u(16.0))
                .right(u(16.0))
                .bottom(u(16.0))
                .flex()
                .justify_center()
                .child(
                    div().max_w(u(640.0)).child(
                        Notice::error(sentence).selector("panic-notice").action(
                            Button::new("close-panic-notice")
                                .label(text::DISMISS)
                                .selector("close-panic-notice")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state
                                        .update(cx, |state, cx| state.close_panic_notice(cx));
                                })),
                        ),
                    ),
                )
                .into_any_element(),
        )
    }
}
