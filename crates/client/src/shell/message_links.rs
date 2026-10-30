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
    /// The in-app router also accepts links delivered by the operating system.
    pub fn open_app_link(
        &mut self,
        link: junto_client_core::links::AppLink,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        use junto_client_core::links::AppLink;
        let origin = match &link {
            AppLink::Message { server, .. }
            | AppLink::Thread { server, .. }
            | AppLink::Channel { server, .. }
            | AppLink::Conversation { server, .. }
            | AppLink::File { server, .. }
            | AppLink::Recording { server, .. } => server.clone(),
        };
        if !matches!(self.layer, super::Layer::None | super::Layer::Welcome) {
            self.close_layer(window, cx);
        }
        let known = self.state.update(cx, |state, cx| match link {
            AppLink::Message { server, id } => state.open_message_link(&server, id, cx),
            AppLink::Thread { server, id } => state.open_thread_link(&server, id, cx),
            AppLink::Channel { server, id } => state.open_channel_link(&server, id, cx),
            AppLink::Conversation { server, id } => state.open_conversation_link(&server, id, cx),
            AppLink::File { server, id } => state.open_file_link(&server, id, cx),
            AppLink::Recording { server, id } => state.open_recording_link(&server, id, cx),
        });
        if !known {
            self.address
                .update(cx, |field, cx| field.set_value(origin.as_str(), window, cx));
            self.address_error = None;
            let narrow =
                super::logical_width(window) < crate::theme::theme(cx).metrics.narrow_below;
            self.show_address_field(narrow, window, cx);
        }
    }

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
