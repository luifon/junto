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

/// How many cards of invitations are drawn before the line that shows the others.
pub const MOST_CARDS: usize = 3;

/// The room around the cards inside their scrolling list for the shadow of a card, in
/// pixels of the design: the list cuts what lies outside its bounds.
const SHADOW_ROOM: f32 = 24.0;

impl Shell {
    /// The cards of the invitations that wait for the person, on every server, in the order
    /// in which they arrived, at the top of the window at `top` pixels of the design,
    /// centred over the part of the window that starts at `left`, or over the whole window
    /// when that part is narrower than a card. A new invitation is added after the drawn
    /// cards. At most [`MOST_CARDS`] are drawn, the newest ones; a line above them states
    /// how many earlier ones wait and shows them. The cards stay inside the window and
    /// scroll when they do not fit. The ids of a card and of its buttons are
    /// made from the identifier of its invitation, so a press and the keyboard focus stay
    /// with their invitation. Absent while no invitation waits.
    pub(crate) fn render_invitation_cards(
        &mut self,
        view: &ShellView,
        top: f32,
        left: f32,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if view.invitations.len() <= MOST_CARDS {
            self.invitations_open = false;
        }
        if view.invitations.is_empty() {
            return None;
        }
        let several_servers = view.servers.len() > 1;
        let mut arrived: Vec<&Invited> = view.invitations.iter().collect();
        arrived.sort_by_key(|one| (one.created_at, one.id));
        let more = arrived.len().saturating_sub(MOST_CARDS);
        let hidden = if self.invitations_open { 0 } else { more };
        let mut cards: Vec<AnyElement> = Vec::new();
        if more > 0 {
            let open = self.invitations_open;
            cards.push(
                div()
                    .flex()
                    .justify_center()
                    .occlude()
                    .child(
                        Button::new("invitation-more")
                            .label(if open {
                                text::FEWER_INVITATIONS.to_owned()
                            } else {
                                text::more_invitations(more)
                            })
                            .selector("invitation-more")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.invitations_open = !open;
                                cx.notify();
                            })),
                    )
                    .into_any_element(),
            );
        }
        cards.extend(arrived.into_iter().skip(hidden).map(|invited| {
            let server = if several_servers {
                view.servers
                    .iter()
                    .find(|row| row.id == invited.target.server_id)
                    .map(|row| row.name.clone())
            } else {
                None
            };
            self.render_invitation_card(invited, server, cx)
        }));
        // A part of the window narrower than a card and its margins, such as the main area
        // beside the channel column at a large scale, takes the cards over the whole window.
        let left = if super::logical_width(window) - left < CARD_WIDTH + 24.0 {
            0.0
        } else {
            left
        };
        let height = (super::logical_height(window) - top - 12.0 + SHADOW_ROOM).max(0.0);
        // The frame takes no pointer event: only the cards cover what lies under them.
        Some(
            div()
                .absolute()
                .top(u(top - SHADOW_ROOM))
                .left(u(left))
                .right_0()
                .flex()
                .flex_col()
                .items_center()
                .px(u(12.0))
                .debug_selector(|| "invitation-cards".to_owned())
                .child(
                    div()
                        .id("invitation-list")
                        .debug_selector(|| "invitation-list".to_owned())
                        .flex()
                        .flex_col()
                        .gap(u(8.0))
                        .p(u(SHADOW_ROOM))
                        .w(u(CARD_WIDTH + 2.0 * SHADOW_ROOM))
                        .max_w_full()
                        .max_h(u(height))
                        .overflow_y_scroll()
                        .track_scroll(&self.invitations_scroll)
                        .children(cards),
                )
                .into_any_element(),
        )
    }

    fn render_invitation_card(
        &mut self,
        invited: &Invited,
        server: Option<String>,
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
        let invitation_id = invited.id;
        let id = |name: &str| SharedString::from(format!("{name}:{invitation_id}"));
        let (accept, decline) = if direct {
            (
                Button::new(id("direct-call-accept"))
                    .label(text::ACCEPT_CALL)
                    .icon(Icon::Phones)
                    .kind(ButtonKind::Primary)
                    .selector("direct-call-accept"),
                Button::new(id("direct-call-decline"))
                    .label(text::DECLINE)
                    .selector("direct-call-decline"),
            )
        } else {
            (
                Button::new(id("invitation-join"))
                    .label(text::JOIN)
                    .icon(Icon::Phones)
                    .kind(ButtonKind::Primary)
                    .selector("invitation-join"),
                Button::new(id("invitation-decline"))
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
        let card = div()
            .id(id("invitation-card"))
            .role(Role::AlertDialog)
            .aria_label(label)
            .debug_selector(move || selector.to_owned())
            // The card takes the pointer events over what it covers.
            .occlude()
            .flex()
            .flex_col()
            .gap(u(10.0))
            .flex_none()
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
                    .child(
                        div()
                            .debug_selector(move || format!("invitation-decline:{invitation_id}"))
                            .child(decline),
                    )
                    .child(
                        div()
                            .debug_selector(move || format!("invitation-accept:{invitation_id}"))
                            .child(accept),
                    ),
            )
            .when(direct, |card| card.border_color(Hsla::from(colors.cypress)));
        div()
            .flex_none()
            .debug_selector(move || format!("invitation-card:{invitation_id}"))
            .child(card)
            .into_any_element()
    }
}
