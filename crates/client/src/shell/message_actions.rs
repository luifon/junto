//! The actions of a message (ADR 0740): one list, which the hover bar and the menu of the
//! message both take. The bar shows the frequent actions as icons with a tooltip; its
//! vertical-dots button and a right click open the menu with every action.

use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    StatefulInteractiveElement, Styled, Window, div, point, prelude::FluentBuilder,
};
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::ids::{MessageId, ServerId};

use super::{EmojiTarget, Layer, Pane, Shell};
use crate::assets::Icon;
use crate::emoji::Drawings;
use crate::state::feed::{Feed, MessageRow};
use crate::text;
use crate::theme::{Theme, u};
use crate::ui::OwnPress as _;
use crate::ui::{
    Button, ButtonKind, Menu, MenuItem, StyleExt, icon, ledge, menu_separator, tooltip,
};

/// One action of a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MessageAction {
    React,
    Reply,
    Thread,
    Forward,
    Pin,
    MarkUnread,
    CopyText,
    CopyLink,
    Edit,
    Delete,
}

impl MessageAction {
    /// Every action in the order of the menu, each group after a separator: reply, pin,
    /// copy and change. Reactions stand in a row above them.
    const GROUPS: [&'static [Self]; 4] = [
        &[Self::Reply, Self::Thread, Self::Forward],
        &[Self::Pin, Self::MarkUnread],
        &[Self::CopyText, Self::CopyLink],
        &[Self::Edit, Self::Delete],
    ];

    /// The actions of the hover bar, in its order. Pin, Mark unread, Copy link and Delete
    /// are in the menu alone.
    const BAR: [Self; 6] = [
        Self::React,
        Self::Reply,
        Self::Thread,
        Self::Forward,
        Self::CopyText,
        Self::Edit,
    ];

    /// The name of the action, also its tooltip and its accessible label.
    fn label(self, pinned: bool) -> &'static str {
        match self {
            Self::React => text::REACT,
            Self::Reply => text::REPLY,
            Self::Thread => text::REPLY_IN_THREAD,
            Self::Forward => text::FORWARD_MESSAGE,
            Self::Pin if pinned => text::UNPIN,
            Self::Pin => text::PIN,
            Self::MarkUnread => text::MARK_UNREAD,
            Self::CopyText => text::COPY_TEXT,
            Self::CopyLink => text::COPY_MESSAGE_LINK,
            Self::Edit => text::EDIT,
            Self::Delete => text::DELETE,
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::React => Icon::SmilePlus,
            Self::Reply => Icon::Reply,
            Self::Thread => Icon::Chat,
            Self::Forward => Icon::Forward,
            Self::Pin => Icon::Pin,
            Self::MarkUnread => Icon::Unread,
            Self::CopyText => Icon::Copy,
            Self::CopyLink => Icon::Link,
            Self::Edit => Icon::Edit,
            Self::Delete => Icon::Trash,
        }
    }

    /// The name of the bar button and of the menu item for the tests.
    fn names(self) -> (&'static str, &'static str) {
        match self {
            Self::React => ("react", "message-react"),
            Self::Reply => ("reply", "message-reply"),
            Self::Thread => ("thread", "message-thread"),
            Self::Forward => ("forward", "message-forward"),
            Self::Pin => ("pin", "message-pin"),
            Self::MarkUnread => ("mark-unread", "message-mark-unread"),
            Self::CopyText => ("copy-message", "message-copy-text"),
            Self::CopyLink => ("copy-link", "message-copy-link"),
            Self::Edit => ("edit", "message-edit"),
            Self::Delete => ("delete", "message-delete"),
        }
    }

    /// The actions that the message offers to the person in the list of `pane`. An action
    /// that the permissions of the person do not allow is absent (ADR 0790).
    pub(crate) fn offered(row: &MessageRow, feed: &Feed, pane: Pane) -> Vec<Self> {
        let message = &row.message;
        let text = message.document.is_some() || message.comment.is_some();
        let mut offered = Vec::new();
        if row.can_react && !row.react_refused {
            offered.push(Self::React);
        }
        if row.can_reply {
            offered.push(Self::Reply);
        }
        if crate::state::feed::channel_of(feed.place).is_some()
            && row.can_thread
            && row.thread_refused.is_none()
            && pane == Pane::Main
        {
            offered.push(Self::Thread);
        }
        if message.document.is_some() && !message.deleted {
            offered.push(Self::Forward);
        }
        if row.can_pin {
            offered.push(Self::Pin);
        }
        if !message.deleted && message.position > 0 {
            offered.push(Self::MarkUnread);
        }
        if text {
            offered.push(Self::CopyText);
        }
        if !message.deleted {
            offered.push(Self::CopyLink);
        }
        if row.can_edit {
            offered.push(Self::Edit);
        }
        if row.can_delete {
            offered.push(Self::Delete);
        }
        offered
    }
}

impl Shell {
    /// Does an action of a message.
    fn run_message_action(
        &mut self,
        action: MessageAction,
        (pane, server_id, message_id): (Pane, ServerId, MessageId),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(feed) = self.pane_feed(pane).cloned() else {
            return;
        };
        let Some(row) = self.message_row_of(pane, message_id) else {
            return;
        };
        if feed.server_id != server_id
            || !MessageAction::offered(&row, &feed, pane).contains(&action)
        {
            return;
        }
        match action {
            MessageAction::React => {
                self.open_emoji_picker(EmojiTarget::React(message_id), window, cx);
            }
            MessageAction::Reply => self.reply_to_message(pane, message_id, window, cx),
            MessageAction::Thread => {
                if let Some(channel_id) = crate::state::feed::channel_of(feed.place) {
                    self.open_thread(server_id, channel_id, message_id, window, cx);
                }
            }
            MessageAction::Forward => self.open_forward(server_id, message_id, window, cx),
            MessageAction::Pin => {
                let pinned = row.message.pinned;
                self.state.update(cx, |state, cx| {
                    state.set_pinned(server_id, message_id, !pinned, cx);
                });
            }
            MessageAction::MarkUnread => {
                let place = PlaceId::of_message(&row.message);
                self.state.update(cx, |state, cx| {
                    state.mark_message_unread(server_id, message_id, place, cx)
                });
            }
            MessageAction::CopyText => {
                let copied = self.copied_message(&row.message, &feed.names, cx);
                self.copy_text(copied, cx);
            }
            MessageAction::CopyLink => {
                if let Some(link) = self.state.read(cx).message_link(server_id, message_id) {
                    self.copy_text(link, cx);
                }
            }
            MessageAction::Edit => self.edit_message(pane, message_id, window, cx),
            MessageAction::Delete => {
                self.open_layer(Layer::ConfirmDelete(message_id), window, cx);
            }
        }
    }

    /// The text that "Copy text" copies: the comment of a forward, then the content.
    pub(super) fn copied_message(
        &self,
        message: &junto_client_core::protocol::message::Message,
        names: &crate::state::feed::Names,
        cx: &Context<Self>,
    ) -> String {
        message
            .comment
            .iter()
            .map(|comment| crate::state::feed::copy_text(comment, names))
            .chain(
                message
                    .document
                    .iter()
                    .map(|document| self.copied_document(document, names, cx)),
            )
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The recent emoji that a reaction takes with one press, at most three.
    fn quick_emoji(&self, cx: &mut Context<Self>) -> Vec<&'static emojis::Emoji> {
        let drawings = Drawings::of(cx);
        self.state
            .read(cx)
            .recent_emoji()
            .iter()
            .filter_map(|value| emojis::get(value))
            .filter(|emoji| drawings.drawing(emoji) != crate::emoji::Drawing::Broken)
            .take(3)
            .collect()
    }

    /// A press on a recent emoji adds the reaction of the person.
    fn react_with(
        &mut self,
        server_id: ServerId,
        message_id: MessageId,
        sign: &'static str,
        owner: Option<junto_client_core::Owner>,
        cx: &mut Context<Self>,
    ) {
        if self.state.read(cx).owner(server_id) == owner {
            self.state.update(cx, |state, cx| {
                state.react(server_id, message_id, sign.to_owned(), true, cx)
            });
        }
    }

    /// The actions of a message at the pointer: icons with a tooltip, and the
    /// vertical-dots button that opens the menu with every action.
    pub(super) fn action_bar(
        &mut self,
        feed: &Feed,
        row: &MessageRow,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let message_id = row.message.id;
        let server_id = feed.server_id;
        let pane = Pane::of(feed.place);
        let pinned = row.message.pinned;
        let owner = self.state.read(cx).owner(server_id);
        let offered = MessageAction::offered(row, feed, pane);
        let quick: Vec<AnyElement> = if offered.contains(&MessageAction::React) {
            self.quick_emoji(cx)
                .into_iter()
                .enumerate()
                .map(|(index, emoji)| {
                    let sign = emoji.as_str();
                    div()
                        .flex_none()
                        .debug_selector(move || format!("quick-emoji:{message_id}:{sign}"))
                        .child(
                            Button::new(("quick-emoji", index))
                                .kind(ButtonKind::Ghost)
                                .label(sign)
                                .tooltip(emoji.name())
                                .h(u(22.0))
                                .min_w(u(26.0))
                                .px(u(1.0))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.react_with(server_id, message_id, sign, owner, cx);
                                })),
                        )
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        let buttons: Vec<AnyElement> = MessageAction::BAR
            .into_iter()
            .filter(|action| offered.contains(action))
            .map(|action| {
                let (name, _) = action.names();
                let label = action.label(pinned);
                div()
                    .id(name)
                    .debug_selector(move || format!("{name}:{message_id}"))
                    .role(gpui::Role::Button)
                    .aria_label(label)
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(u(26.0))
                    .h(u(22.0))
                    .rounded(u(theme.metrics.radius_small))
                    .cursor_pointer()
                    .own_press()
                    .hover(move |style| style.bg(Hsla::from(colors.plaster)))
                    .tooltip(tooltip(label))
                    .child(icon(action.icon(), 15.0, colors.ink_2))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.run_message_action(action, (pane, server_id, message_id), window, cx);
                    }))
                    .into_any_element()
            })
            .collect();
        let (react, rest) = match buttons.split_first() {
            Some(_) if offered.contains(&MessageAction::React) => {
                let mut buttons = buttons;
                let react = buttons.remove(0);
                (Some(react), buttons)
            }
            _ => (None, buttons),
        };
        div()
            .debug_selector(move || format!("message-actions:{message_id}"))
            .absolute()
            .top(u(-10.0))
            .right(u(18.0))
            .flex()
            .flex_wrap()
            .max_w_full()
            .justify_end()
            .items_center()
            .gap(u(1.0))
            .px(u(3.0))
            .py(u(2.0))
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.travertine_hi))
            .shadow(ledge(colors.rule))
            .children(quick)
            .children(react)
            .children(rest)
            // A permanent message has no edit, and no delete for anybody but an
            // administrator (ADR 0402).
            .when(row.message.permanent && !row.message.deleted, |bar| {
                bar.child(
                    div()
                        .id("no-edit")
                        .debug_selector(move || format!("no-edit:{message_id}"))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(u(5.0))
                        .h(u(22.0))
                        .ml(u(3.0))
                        .pl(u(7.0))
                        .pr(u(5.0))
                        .border_l_1()
                        .border_color(Hsla::from(colors.rule_soft))
                        .text_size(u(12.5))
                        .ink(colors.ink_2)
                        .whitespace_nowrap()
                        .child(icon(Icon::EditOff, 14.0, colors.ink_2))
                        .child(if row.can_delete {
                            text::NO_EDIT
                        } else {
                            text::NO_EDIT_OR_DELETE
                        }),
                )
            })
            .child(
                div()
                    .flex_none()
                    .debug_selector(move || format!("message-more:{message_id}"))
                    .child(
                        crate::ui::MenuButton::new("message-more")
                            .shown(true)
                            .on_open(Self::handler(cx, move |this, window, cx| {
                                this.open_message_menu(pane, message_id, window, cx);
                            })),
                    ),
            )
            .into_any_element()
    }

    /// The menu of a message: a row of reactions, then every other action in groups, with
    /// Delete last.
    pub(super) fn render_message_menu(
        &mut self,
        row: &MessageRow,
        feed: &Feed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let message_id = row.message.id;
        let server_id = feed.server_id;
        let pane = Pane::of(feed.place);
        let pinned = row.message.pinned;
        let owner = self.state.read(cx).owner(server_id);
        let offered = MessageAction::offered(row, feed, pane);
        let item = |action: MessageAction, cx: &mut Context<Self>| {
            let (_, selector) = action.names();
            MenuItem::new(selector, action.label(pinned))
                .icon(action.icon())
                .selector(selector)
                .when(action == MessageAction::Delete, MenuItem::danger)
                .on_click(cx.listener(move |this, _, window, cx| {
                    if action != MessageAction::Delete {
                        this.close_layer(window, cx);
                    }
                    this.run_message_action(action, (pane, server_id, message_id), window, cx);
                }))
                .into_any_element()
        };
        let mut items: Vec<AnyElement> = Vec::new();
        if offered.contains(&MessageAction::React) {
            let quick: Vec<AnyElement> = self
                .quick_emoji(cx)
                .into_iter()
                .enumerate()
                .map(|(index, emoji)| {
                    let sign = emoji.as_str();
                    Button::new(("message-quick-emoji", index))
                        .kind(ButtonKind::Ghost)
                        .label(sign)
                        .tooltip(emoji.name())
                        .selector(
                            [
                                "message-quick-emoji-0",
                                "message-quick-emoji-1",
                                "message-quick-emoji-2",
                            ][index],
                        )
                        .h(u(28.0))
                        .min_w(u(32.0))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.close_layer(window, cx);
                            this.react_with(server_id, message_id, sign, owner, cx);
                        }))
                        .into_any_element()
                })
                .collect();
            items.push(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(2.0))
                    .px(u(8.0))
                    .py(u(4.0))
                    .debug_selector(|| "message-reactions-row".to_owned())
                    .children(quick)
                    .child(
                        Button::new("message-react")
                            .kind(ButtonKind::Ghost)
                            .icon(Icon::SmilePlus)
                            .tooltip(text::REACT)
                            .selector("message-react")
                            .h(u(28.0))
                            .min_w(u(32.0))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.close_layer(window, cx);
                                this.run_message_action(
                                    MessageAction::React,
                                    (pane, server_id, message_id),
                                    window,
                                    cx,
                                );
                            })),
                    )
                    .into_any_element(),
            );
        }
        // "Who reacted" is in the group of the reactions. Whether it is there is decided when
        // the menu is first drawn, so a first reaction that arrives while the menu is open
        // moves no item under the pointer (ADR 0730); a message without reactions has none
        // (ADR 0790).
        let key = (!row.message.deleted && owner.is_some())
            .then(|| row.message.reactions.first())
            .flatten()
            .and_then(|reaction| {
                self.reactor_key(
                    server_id,
                    PlaceId::of_message(&row.message),
                    message_id,
                    reaction.emoji.clone(),
                    cx,
                )
            });
        let reacted = *self.message_menu_reacted.get_or_insert(key.is_some());
        if reacted {
            let emojis: Vec<String> = row
                .message
                .reactions
                .iter()
                .map(|reaction| reaction.emoji.clone())
                .collect();
            let first = key.clone();
            items.push(
                MenuItem::new("message-reaction-people", text::WHO_REACTED)
                    .icon(Icon::People)
                    .selector("message-reaction-people")
                    .disabled(first.is_none())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.close_layer(window, cx);
                        if let Some(key) = &first {
                            this.open_reaction_people(key.clone(), emojis.clone(), window, cx);
                        }
                    }))
                    .into_any_element(),
            );
        }
        for group in MessageAction::GROUPS {
            let shown: Vec<MessageAction> = group
                .iter()
                .copied()
                .filter(|action| offered.contains(action))
                .collect();
            if shown.is_empty() {
                continue;
            }
            if !items.is_empty() {
                items.push(menu_separator(cx).into_any_element());
            }
            for action in shown {
                items.push(item(action, cx));
            }
        }
        Menu::new("message-menu", text::MESSAGE_ACTIONS, self.layer_focus())
            .selector("message-menu")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx);
            }))
            .children(items)
            .popup_stable(
                point(
                    u(44.0).to_pixels(window.rem_size()),
                    u(4.0).to_pixels(window.rem_size()),
                ),
                self.message_menu_placement.clone(),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::MessageAction;

    #[test]
    fn the_bar_and_the_menu_take_their_actions_from_one_list() {
        let in_menu: Vec<MessageAction> = MessageAction::GROUPS
            .iter()
            .flat_map(|group| group.iter().copied())
            .collect();
        for action in MessageAction::BAR {
            assert!(
                action == MessageAction::React || in_menu.contains(&action),
                "{action:?}"
            );
        }
        assert_eq!(in_menu.last(), Some(&MessageAction::Delete));
        assert!(!MessageAction::BAR.contains(&MessageAction::Delete));
        assert!(!MessageAction::BAR.contains(&MessageAction::Pin));
        assert!(MessageAction::BAR.contains(&MessageAction::Forward));
        assert!(MessageAction::BAR.contains(&MessageAction::CopyText));
    }
}
