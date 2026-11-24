//! A direct conversation outside its call: the button that starts a direct-message call, and
//! the outcome of the last call.
use super::Shell;
use crate::{
    assets::Icon,
    text,
    text::conversations as words,
    theme::u,
    ui::{Button, Notice},
};
use gpui::{AnyElement, Context, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder};
use junto_client_core::protocol::{call::CallPlace, ids::ServerId};

impl Shell {
    /// What a conversation shows above its messages while the call window draws its call:
    /// the statement with Return. Otherwise nothing: the call starts from "Call" in the
    /// header, and the outcome of a call is a line of the messages (ADR 0741).
    pub(crate) fn render_conversation_call(
        &self,
        server: ServerId,
        conversation: &crate::state::view::DirectRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let state = self.state.read(cx);
        let separate = state.is_call_detached()
            && state.media_calls().call.is_some_and(|call| {
                call.target.server_id == server
                    && match (conversation.account_id, call.target.place) {
                        (None, CallPlace::Group { conversation_id }) => {
                            conversation_id == conversation.id
                        }
                        (
                            Some(other),
                            CallPlace::Direct {
                                low_account_id,
                                high_account_id,
                            },
                        ) => low_account_id == other || high_account_id == other,
                        _ => false,
                    }
            });
        if separate {
            return div()
                .flex_none()
                .p(u(12.0))
                .child(
                    Notice::new(text::CALL_SEPARATE)
                        .selector("call-in-separate-window")
                        .action(self.return_call_button(cx)),
                )
                .into_any_element();
        }
        div().flex_none().into_any_element()
    }

    /// The buttons of the header of a conversation (ADR 0741): "Call", and the profile of the
    /// other person or the people of a group. A closed conversation offers its buttons
    /// disabled with the reason; while a call of the conversation rings here, no Call.
    pub(crate) fn render_conversation_buttons(
        &mut self,
        view: &crate::state::view::ShellView,
        server: ServerId,
        conversation: &crate::state::view::DirectRow,
        people_open: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = crate::theme::theme(cx).colors;
        let id = conversation.id;
        let other = conversation.account_id;
        let owner = self
            .state
            .read(cx)
            .server(server)
            .and_then(|server| server.owner());
        let closed = conversation.departure.is_some()
            || view
                .lists()
                .and_then(|lists| lists.feed.as_ref())
                .is_some_and(|feed| {
                    feed.failed == Some(junto_client_core::model::ReadFailure::NotFound)
                });
        let incoming = view.invitations.iter().any(|invited| {
            invited.target.server_id == server
                && match (other, invited.target.place) {
                    (
                        Some(other),
                        CallPlace::Direct {
                            low_account_id,
                            high_account_id,
                        },
                    ) => low_account_id == other || high_account_id == other,
                    (None, CallPlace::Group { conversation_id }) => conversation_id == id,
                    _ => false,
                }
        });
        let mut buttons = Vec::new();
        let own = self
            .state
            .read(cx)
            .server(server)
            .and_then(|server| server.account());
        let target = match other {
            Some(other) => own.map(|own| CallPlace::direct(own, other)),
            None => Some(CallPlace::Group {
                conversation_id: id,
            }),
        }
        .map(|place| junto_media::calls::CallTarget {
            server_id: server,
            place,
        });
        if !incoming {
            let call = Button::new("conversation-call")
                .icon(Icon::Phone)
                .tooltip(if closed {
                    words::CLOSED
                } else {
                    text::DIRECT_CALL
                })
                .accessible_label(text::DIRECT_CALL)
                .disabled(closed)
                .selector("conversation-call")
                .on_click(cx.listener(move |this, _, _, cx| {
                    if owner.is_none()
                        || this
                            .state
                            .read(cx)
                            .server(server)
                            .and_then(|server| server.owner())
                            != owner
                    {
                        return;
                    }
                    this.state.update(cx, |state, cx| match other {
                        Some(other) => state.start_direct_call(server, other, cx),
                        None => state.start_group_call(server, id, cx),
                    });
                }));
            // The arrow of the join control of the calls (ADR 0735) opens the device check
            // before the call; a closed conversation has no call to check.
            match target.filter(|_| !closed) {
                Some(target) => buttons.push(self.join_control(
                    call,
                    crate::ui::ButtonKind::Default,
                    super::device_checks::JoinCheck {
                        target,
                        invitation: None,
                        arrow: "conversation-devices",
                    },
                    cx,
                )),
                None => buttons.push(call.into_any_element()),
            }
        }
        match other {
            Some(other) => buttons.push(
                Button::new("conversation-profile")
                    .icon(Icon::Person)
                    .tooltip(words::PROFILE)
                    .accessible_label(words::PROFILE)
                    .selector("conversation-profile")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.show_profile(
                            crate::ui::OpenProfile {
                                server,
                                account: other,
                            },
                            window,
                            cx,
                        );
                    }))
                    .into_any_element(),
            ),
            None => buttons.push(
                Button::new("conversation-people")
                    .icon(Icon::People)
                    .tooltip(words::PEOPLE)
                    .accessible_label(words::PEOPLE)
                    .selector("conversation-people")
                    .when(people_open, |button| {
                        button
                            .bg(gpui::Hsla::from(colors.limestone_lo))
                            .shadow_none()
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.toggle_group_people(server, id, window, cx);
                    }))
                    .into_any_element(),
            ),
        }
        buttons
    }
}
