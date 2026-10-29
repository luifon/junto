use super::Shell;
use crate::{
    text,
    theme::{theme, u},
    ui::{Button, Dialog, Notice, PresenceMark, StyleExt},
};
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Styled, div,
    prelude::FluentBuilder,
};
use junto_client_core::{
    activity::ActivityState,
    protocol::{ids::ServerId, presence::PresenceChoice},
};

impl Shell {
    pub(crate) fn render_presence(
        &mut self,
        server: ServerId,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let Some(view) = self.state.read(cx).presence_server(server).cloned() else {
            return div().into_any_element();
        };
        let mut content = div().flex().flex_col().gap(u(8.));
        for (index, choice) in PresenceChoice::ALL.into_iter().enumerate() {
            content = content.child(
                div()
                    .id(("presence-option", index))
                    .flex()
                    .items_center()
                    .gap(u(8.))
                    .child(PresenceMark::new(choice.shown(true, false, true), 14.))
                    .child(
                        Button::new(("presence-choice", index))
                            .label(text::presence::choice(choice))
                            .selected(view.choice == Some(choice))
                            .disabled(view.changing || view.choice.is_none() || !view.connected)
                            .flex_1()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.set_presence_choice(server, choice, cx)
                                });
                            }))
                            .map(|button| {
                                div()
                                    .debug_selector(move || format!("presence-choice:{index}"))
                                    .child(button)
                            }),
                    ),
            );
        }
        content = content
            .child(
                div()
                    .typeface(theme.types.small, &theme)
                    .ink(theme.colors.ink_2)
                    .child(text::presence::AUTOMATIC),
            )
            .child(
                div()
                    .typeface(theme.types.small, &theme)
                    .ink(theme.colors.ink_2)
                    .child(text::presence::INVISIBLE_HELP),
            );
        if self.state.read(cx).presence_activity() == Some(ActivityState::Unavailable) {
            content = content.child(
                Notice::new(text::presence::ACTIVITY_UNAVAILABLE)
                    .selector("presence-activity-unavailable"),
            );
        }
        if !view.connected {
            content = content
                .child(Notice::new(text::presence::CONNECTING).selector("presence-connecting"));
        }
        if view.changing {
            content =
                content.child(Notice::new(text::presence::SAVING).selector("presence-saving"));
        }
        if view.choice.is_none() && view.reading {
            content =
                content.child(Notice::new(text::presence::CHECKING).selector("presence-reading"));
        }
        if let Some(reason) = view.read_failed {
            content = content.child(
                Notice::error(format!("{} {reason}", text::presence::NOT_READ))
                    .selector("presence-read-failed")
                    .action(
                        Button::new("presence-read-retry")
                            .label(text::TRY_AGAIN)
                            .selector("presence-read-retry")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.retry_presence(server, cx))
                            })),
                    ),
            );
        }
        if let Some(reason) = view.failed {
            content = content.child(
                Notice::error(format!("{} {reason}", text::presence::NOT_CHANGED))
                    .selector("presence-write-failed")
                    .action(
                        Button::new("presence-write-retry")
                            .label(text::TRY_AGAIN)
                            .selector("presence-write-retry")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state
                                    .update(cx, |state, cx| state.retry_presence_choice(server, cx))
                            })),
                    ),
            );
        }
        Dialog::new(text::presence::TITLE, &self.layer_focus)
            .selector("presence-dialog")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx)
            }))
            .child(content)
            .into_any_element()
    }
}
