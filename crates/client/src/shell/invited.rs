//! The invitations into a call that wait for the answer of the person: a card for each one
//! at the top of every window of the application, over what the window shows (ADR 0739).
use super::Shell;
use crate::{
    assets::Icon,
    state::view::{Invited, ShellView},
    text,
    theme::{theme, u},
    ui::{Button, ButtonKind, StyleExt, float_shadow, icon},
};
use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, Role, SharedString,
    StatefulInteractiveElement, Styled, div, prelude::FluentBuilder,
};
use junto_client_core::protocol::call::CallPlace;

/// The width of the card of an invitation, in pixels of the design.
const CARD_WIDTH: f32 = 380.0;

impl Shell {
    /// The cards of the invitations that wait for the person, on every server, placed at
    /// the top centre of the window under its first line at `top` pixels of the design.
    /// Absent while no invitation waits.
    pub(crate) fn render_invitation_cards(
        &mut self,
        view: &ShellView,
        top: f32,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if view.invitations.is_empty() {
            return None;
        }
        let several_servers = view.servers.len() > 1;
        let cards: Vec<AnyElement> = view
            .invitations
            .iter()
            .enumerate()
            .map(|(index, invited)| {
                let server = if several_servers {
                    view.servers
                        .iter()
                        .find(|row| row.id == invited.target.server_id)
                        .map(|row| row.name.clone())
                } else {
                    None
                };
                self.render_invitation_card(invited, server, index, cx)
            })
            .collect();
        // The frame takes no pointer event: only the cards cover what lies under them.
        Some(
            div()
                .absolute()
                .top(u(top))
                .left_0()
                .right_0()
                .flex()
                .flex_col()
                .items_center()
                .gap(u(8.0))
                .px(u(12.0))
                .debug_selector(|| "invitation-cards".to_owned())
                .children(cards)
                .into_any_element(),
        )
    }

    fn render_invitation_card(
        &mut self,
        invited: &Invited,
        server: Option<String>,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let direct = matches!(invited.target.place, CallPlace::Direct { .. });
        let by = invited.by.as_deref().unwrap_or(text::UNKNOWN_PERSON);
        let (title, sentence, selector) = if direct {
            (
                text::INCOMING_CALL,
                text::direct_ring(by),
                "incoming-direct-call",
            )
        } else {
            (
                text::CALL_INVITATION,
                text::invitation(invited.by.as_deref(), invited.channel.as_deref()),
                "call-invitation",
            )
        };
        let sentence = match server {
            Some(server) => format!("{sentence} {}", text::on_server(&server)),
            None => sentence,
        };
        let label: SharedString = format!("{title}. {sentence}").into();
        let (accepted, declined) = (invited.clone(), invited.clone());
        let (accept, decline) = if direct {
            (
                Button::new(("direct-call-accept", index))
                    .label(text::ACCEPT_CALL)
                    .icon(Icon::Phones)
                    .kind(ButtonKind::Primary)
                    .selector("direct-call-accept"),
                Button::new(("direct-call-decline", index))
                    .label(text::DECLINE)
                    .selector("direct-call-decline"),
            )
        } else {
            (
                Button::new(("invitation-join", index))
                    .label(text::JOIN)
                    .icon(Icon::Phones)
                    .kind(ButtonKind::Primary)
                    .selector("invitation-join"),
                Button::new(("invitation-decline", index))
                    .label(text::DECLINE)
                    .selector("invitation-decline"),
            )
        };
        let accept = accept.on_click(cx.listener(move |this, _, _, cx| {
            this.state
                .update(cx, |state, cx| state.accept_invitation(&accepted, cx));
        }));
        let decline = decline.on_click(cx.listener(move |this, _, _, cx| {
            this.state
                .update(cx, |state, cx| state.decline_invitation(&declined, cx));
        }));
        div()
            .id(("invitation-card", index))
            .role(Role::AlertDialog)
            .aria_label(label)
            .debug_selector(move || selector.to_owned())
            // The card takes the pointer events over what it covers.
            .occlude()
            .flex()
            .flex_col()
            .gap(u(10.0))
            .w(u(CARD_WIDTH))
            .max_w_full()
            .p(u(12.0))
            .rounded(u(theme.metrics.radius))
            .bg(Hsla::from(colors.travertine_hi))
            .border_1()
            .border_color(Hsla::from(colors.terracotta))
            .shadow(float_shadow(colors.shadow_deep, 6.0, 24.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(10.0))
                    .child(
                        super::people::avatar(
                            &self.faces(invited.target.server_id),
                            invited.caller,
                            36.0,
                        )
                        .radius(2.0),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(u(6.0))
                                    .typeface(theme.types.control_strong, &theme)
                                    .ink(colors.terracotta_deep)
                                    .child(icon(Icon::Invite, 14.0, colors.terracotta_deep))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .typeface(theme.types.body, &theme)
                                    .line_height(u(20.0))
                                    .ink(colors.ink)
                                    .child(sentence),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(u(6.0))
                    .child(decline)
                    .child(accept),
            )
            .when(direct, |card| card.border_color(Hsla::from(colors.cypress)))
            .into_any_element()
    }
}
