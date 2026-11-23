//! The side panel of a thread, the side panel of the pinned messages, and the dialog that
//! forks a thread into a channel (ADR 0741, views A and J of the design).

use gpui::{
    AnyElement, Context, FontWeight, Hsla, InteractiveElement, IntoElement, ParentElement, Role,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder, px,
};
use junto_client_core::protocol::ids::{ChannelId, MessageId, ServerId};
use junto_client_core::protocol::structure::ChannelKind;

use super::feed::{MessageShape, PaneShown};
use super::{Layer, Pane, Shell};
use crate::assets::Icon;
use crate::state::feed::{Feed, MessageRow};
use crate::state::notices::Jump;
use crate::state::view::{ForkState, PinsPanel, PinsState, ShellView, ThreadPanel};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Button, ButtonKind, Dialog, Field, Notice, ScrollArea, SideHeader, StatePanel, StyleExt,
    dialog_footer, icon,
};

/// The dialog that forks the thread of the side panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForkDialog {
    pub root_id: MessageId,
    pub kind: ChannelKind,
    /// The request was sent: the dialog closes when the server made the channel.
    pub sent: bool,
}

fn kind_name(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::Text => text::FORK_TEXT,
        ChannelKind::Voice => text::FORK_VOICE,
    }
}

impl Shell {
    /// The side panel of a thread: its header, the controls of a fork, the root, the
    /// count of the replies, and the replies with their composer.
    pub(crate) fn render_thread_panel(
        &mut self,
        thread: &ThreadPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let header = SideHeader::new(text::THREAD)
            .title_bar()
            .icon(Icon::Chat)
            .note(format!("# {}", thread.channel_name))
            .trailing(
                div()
                    .flex()
                    .items_center()
                    .gap(u(4.0))
                    .children(self.render_follow_control(thread, window, cx))
                    .child(self.render_thread_menu_button(thread, window, cx))
                    .child(
                        Button::new("close-thread")
                            .kind(ButtonKind::Ghost)
                            .icon(Icon::X)
                            .tooltip(text::CLOSE_THREAD)
                            .selector("close-thread")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |state, cx| state.close_thread(cx));
                            })),
                    ),
            );
        let gone = thread.feed.as_ref().is_some_and(|feed| {
            feed.failed == Some(junto_client_core::model::ReadFailure::NotFound)
        });
        let body: Vec<AnyElement> = if gone {
            let state = StatePanel::new(text::THREAD_GONE_TITLE, text::THREAD_GONE)
                .selector("thread-gone")
                .into_any_element();
            // The replies that wait keep their rows: the person can discard them.
            match thread.feed.as_ref().filter(|feed| feed.has_waiting()) {
                Some(feed) => vec![state, self.feed_list(feed, true, &theme, cx)],
                None => vec![state],
            }
        } else {
            let shown = PaneShown {
                feed: thread.feed.as_ref(),
                draft: &thread.draft,
                compact: true,
                permanent: thread.permanent,
                gone: text::THREAD_GONE,
                empty: text::NO_REPLIES,
                writing: thread.writing,
                live_only: thread.live_only,
            };
            // The channels that the thread was forked into stay listed under the header
            // (ADR 0741).
            let forks = (!thread.forks.is_empty()).then(|| self.render_forks(thread, &theme, cx));
            let refused = thread
                .follow
                .as_ref()
                .and_then(|follow| follow.refused.clone());
            forks
                .into_iter()
                .chain([
                    crate::ui::notice_slot("thread-follow-refused-slot")
                        .children(
                            refused.map(|reason| {
                                Notice::error(reason).selector("thread-follow-refused")
                            }),
                        )
                        .into_any_element(),
                    self.render_thread_root(thread, &theme, window, cx),
                    self.render_pane(Pane::Thread, &shown, Vec::new(), window, cx),
                ])
                .collect()
        };
        div()
            .id("side-panel")
            .role(Role::Complementary)
            .aria_label(text::thread_of(&thread.channel_name))
            .debug_selector(|| "thread-panel".to_owned())
            .flex()
            .flex_col()
            .flex_none()
            .w(u(self.panel_width(
                junto_client_core::settings::Panel::Thread,
                theme.metrics.side_panel_width,
                cx,
            )))
            .h_full()
            .bg(Hsla::from(colors.plaster))
            .border_l_1()
            .border_color(Hsla::from(colors.rule))
            .child(header)
            .children(body)
            .into_any_element()
    }

    /// The line under the header that names the channels that the thread was forked into.
    fn render_forks(
        &mut self,
        thread: &ThreadPanel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let workspace_id = self
            .view
            .as_ref()
            .and_then(|view| view.workspace())
            .map(|workspace| workspace.id);
        let links = thread
            .forks
            .iter()
            .enumerate()
            .map(|(index, (channel_id, name, kind))| {
                let channel_id = *channel_id;
                Button::new(("fork-link", index))
                    .kind(ButtonKind::Ghost)
                    .icon(match kind {
                        ChannelKind::Text => Icon::Hash,
                        ChannelKind::Voice => Icon::Speaker,
                    })
                    .label(name.clone())
                    .selector("fork-link")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(workspace_id) = workspace_id {
                            this.state.update(cx, |state, cx| {
                                state.select_channel(workspace_id, channel_id, cx);
                            });
                        }
                    }))
                    .into_any_element()
            });
        div()
            .id("thread-forks")
            .debug_selector(|| "thread-forks".to_owned())
            .flex()
            .flex_wrap()
            .flex_none()
            .items_center()
            .gap(u(6.0))
            .px(u(14.0))
            .py(u(6.0))
            .border_b_1()
            .border_color(Hsla::from(colors.rule))
            .typeface(theme.types.small, theme)
            .ink(colors.ink_2)
            .child(icon(Icon::Fork, 14.0, colors.ink_2))
            .child(text::FORKED_INTO)
            .children(links)
            .into_any_element()
    }

    /// The follow control of the thread: a bell that states whether the person follows the
    /// thread, and whose menu changes it or removes the choice (ADR 0412). Nothing is drawn
    /// until the server has told the state.
    fn render_follow_control(
        &mut self,
        thread: &ThreadPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let follow = thread.follow.as_ref()?;
        let following = follow.following;
        // An archived channel takes no change, and one change is on its way at a time.
        let held = follow.busy || follow.read_only;
        let reason = if follow.read_only {
            text::followed::ARCHIVED
        } else {
            text::followed::state(following, follow.setting)
        };
        let mut actions = vec![crate::ui::RowAction {
            icon: Some(if following { Icon::BellOff } else { Icon::Bell }),
            disabled: held,
            detail: Some(reason.into()),
            ..crate::ui::RowAction::new(
                text::followed::action(following),
                cx.listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |state, cx| state.follow_thread(!following, cx));
                }),
            )
        }];
        // The choice of the person can be removed: the rule of the server decides again.
        if follow.setting.is_some() {
            actions.push(crate::ui::RowAction {
                disabled: held,
                detail: Some(text::followed::USE_DEFAULT_HELP.into()),
                ..crate::ui::RowAction::new(
                    text::followed::USE_DEFAULT,
                    cx.listener(|this, _, _, cx| {
                        this.state
                            .update(cx, |state, cx| state.clear_thread_follow(cx));
                    }),
                )
            });
        }
        let frame = div()
            .id("thread-follow-control")
            .role(Role::Group)
            .aria_label(text::followed::CONTROL)
            .debug_selector(|| "thread-follow-control".to_owned());
        Some(
            crate::ui::bind_button_menu(
                frame,
                (
                    "thread-follow-menu",
                    thread.root_id.as_uuid().as_u128() as u64,
                ),
                crate::ui::ButtonMenu {
                    icon: if following { Icon::Bell } else { Icon::BellOff },
                    label: if following {
                        text::followed::FOLLOWING
                    } else {
                        text::followed::FOLLOW
                    }
                    .into(),
                    selector: "thread-follow-button",
                },
                actions,
                window,
                cx,
            )
            .into_any_element(),
        )
    }

    /// The menu of the thread in its header: the fork, the link and the read item.
    fn render_thread_menu_button(
        &mut self,
        thread: &ThreadPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let root_id = thread.root_id;
        let channel_id = thread.channel_id;
        let server_id = self
            .view
            .as_ref()
            .and_then(|view| view.selected.as_ref())
            .map(|server| server.id);
        let mut actions = Vec::new();
        // "Fork into a channel…" is absent without "manage channel" (ADR 0790), and after
        // the fork, because a thread has one fork.
        if thread.may_fork && thread.forks.is_empty() {
            actions.push(crate::ui::RowAction {
                icon: Some(Icon::Fork),
                ..crate::ui::RowAction::new(
                    text::FORK_INTO_CHANNEL,
                    cx.listener(move |this, _, window, cx| {
                        this.open_fork_dialog(root_id, ChannelKind::Text, window, cx);
                    }),
                )
            });
        }
        if let Some(server_id) = server_id {
            actions.push(crate::ui::RowAction {
                icon: Some(Icon::Link),
                ..crate::ui::RowAction::new(
                    text::COPY_TARGET_LINK,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(link) = this.state.read(cx).thread_link(server_id, root_id) {
                            this.copy_text(link, cx);
                        }
                    }),
                )
            });
        }
        // One read item that follows the state; a thread without replies has none
        // (ADR 0741).
        let place = junto_client_core::model::PlaceId::Thread {
            channel_id,
            root_id,
        };
        if let (Some(server_id), Some(feed)) = (server_id, thread.feed.as_ref()) {
            let replies = feed.rows.iter().any(|row| {
                matches!(row, crate::state::feed::FeedRow::Message(row) if !row.message.deleted)
            });
            if feed.unread > 0 {
                actions.push(crate::ui::RowAction {
                    icon: Some(Icon::CheckDouble),
                    ..crate::ui::RowAction::new(
                        text::MARK_AS_READ,
                        cx.listener(move |this, _, _, cx| {
                            this.state
                                .update(cx, |state, _| state.mark_place_as_read(server_id, place));
                        }),
                    )
                });
            } else if replies {
                actions.push(crate::ui::RowAction {
                    icon: Some(Icon::Unread),
                    ..crate::ui::RowAction::new(
                        text::MARK_AS_UNREAD,
                        cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.mark_place_unread(server_id, place, cx)
                            });
                        }),
                    )
                });
            }
        }
        let frame = div()
            .id("thread-menu-frame")
            .flex()
            .items_center()
            .debug_selector(|| "thread-menu-frame".to_owned());
        crate::ui::bind_button_menu(
            frame,
            ("thread-header-menu", root_id.as_uuid().as_u128() as u64),
            crate::ui::ButtonMenu {
                icon: Icon::Dots,
                label: text::MORE_ACTIONS.into(),
                selector: "thread-more",
            },
            actions,
            window,
            cx,
        )
        .into_any_element()
    }

    fn message_preview_menu(
        &mut self,
        row: &MessageRow,
        feed: &Feed,
        target: (SharedString, bool),
        window: &mut Window,
        cx: &mut Context<Self>,
        render: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) -> AnyElement,
    ) -> AnyElement {
        let (key, go_to) = target;
        let (server_id, message_id) = (feed.server_id, row.message.id);
        let owner = self
            .state
            .read(cx)
            .server(server_id)
            .and_then(|server| server.owner());
        let jump = Jump::to(server_id, &row.message);
        let (content, actions) = crate::ui::collect_actions(|| {
            if let Some(owner) = owner
                && !row.message.deleted
            {
                if let Some(document) = &row.message.document {
                    let copied = self.copied_document(document, &feed.names, cx);
                    if !copied.is_empty() {
                        crate::ui::RowAction::new(
                            text::COPY_TEXT,
                            cx.listener(move |this, _, _, cx| {
                                if this
                                    .state
                                    .read(cx)
                                    .server(server_id)
                                    .is_some_and(|server| server.owner() == Some(owner))
                                {
                                    this.copy_text(copied.clone(), cx);
                                }
                            }),
                        )
                        .register();
                    }
                }
                crate::ui::RowAction::new(
                    text::COPY_TARGET_LINK,
                    cx.listener(move |this, _, _, cx| {
                        let state = this.state.read(cx);
                        let link = state
                            .server(server_id)
                            .filter(|server| server.owner() == Some(owner))
                            .and_then(|_| state.message_link(server_id, message_id));
                        if let Some(link) = link {
                            this.copy_text(link, cx);
                        }
                    }),
                )
                .register();
                if go_to {
                    crate::ui::RowAction::new(
                        text::GO_TO_MESSAGE,
                        cx.listener(move |this, _, _, cx| {
                            if this
                                .state
                                .read(cx)
                                .server(server_id)
                                .is_some_and(|server| server.owner() == Some(owner))
                            {
                                this.go_to(jump, cx);
                            }
                        }),
                    )
                    .register();
                }
            }
            render(self, window, cx)
        });
        let theme = theme(cx);
        let ring =
            crate::ui::focus_ring_inside(theme.colors.terracotta_deep, theme.metrics.focus_ring);
        let focus = crate::ui::row_focus(key.clone(), window, cx);
        let focused = focus.is_focused(window);
        let frame = div()
            .id(key.clone())
            .relative()
            .w_full()
            .min_w_0()
            .flex_none()
            .role(Role::Group)
            .aria_label(text::ROW_ACTIONS)
            .track_focus(&focus)
            .focus_visible(move |style| style.shadow(ring))
            .child(content);
        crate::ui::bind_row_action_menu(
            frame,
            key,
            crate::ui::RowMenu::overlay(focused),
            actions,
            window,
            cx,
        )
        .into_any_element()
    }

    /// The first message of the thread and the count of its replies, above the replies.
    fn render_thread_root(
        &mut self,
        thread: &ThreadPanel,
        theme: &Theme,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let (Some(root), Some(feed)) = (&thread.root, &thread.feed) else {
            return div().flex_none().into_any_element();
        };
        let replies = root.message.reply_count;
        self.message_preview_menu(
            root,
            feed,
            (
                format!("thread-root-actions:{}", root.message.id).into(),
                true,
            ),
            window,
            cx,
            |this, window, cx| {
                div()
                    .id("thread-root")
                    .debug_selector(|| "thread-root".to_owned())
                    .flex()
                    .flex_col()
                    .flex_none()
                    .max_h(u(260.0))
                    .child(
                        div()
                            .id("thread-root-message")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .pt(u(8.0))
                            .child(this.render_message(root, feed, MessageShape::Bare, window, cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap(u(8.0))
                            .px(u(14.0))
                            .pt(u(4.0))
                            .pb(u(3.0))
                            .child(
                                div()
                                    .id("thread-replies")
                                    .debug_selector(|| "thread-replies".to_owned())
                                    .flex_none()
                                    .text_size(u(12.5))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .ink(colors.ink_2)
                                    .child(SharedString::from(text::replies(replies))),
                            )
                            .child(div().flex_1().h(px(1.0)).bg(Hsla::from(colors.rule_soft))),
                    )
                    .into_any_element()
            },
        )
    }

    /// The side panel of the pinned messages of the shown channel.
    pub(crate) fn render_pins_panel(
        &mut self,
        pins: &PinsPanel,
        feed: Option<&Feed>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let header = SideHeader::new(text::PINS)
            .title_bar()
            .icon(Icon::Pin)
            .note(format!("# {}", pins.channel_name))
            .trailing(
                Button::new("close-pins")
                    .kind(ButtonKind::Ghost)
                    .icon(Icon::X)
                    .tooltip(text::CLOSE_PINS)
                    .selector("close-pins")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |state, cx| state.close_pins(cx));
                    })),
            );
        let note = |sentence: &'static str| {
            div()
                .px(u(14.0))
                .pt(u(12.0))
                .typeface(theme.types.small, &theme)
                .ink(colors.ink_2)
                .child(sentence)
                .into_any_element()
        };
        let body: AnyElement = match (&pins.state, feed) {
            (PinsState::Loading, _) | (_, None) => note(text::PINS_LOADING),
            (PinsState::Failed, _) => div()
                .flex()
                .flex_col()
                .gap(u(8.0))
                .px(u(14.0))
                .pt(u(12.0))
                .child(Notice::error(text::PINS_FAILED).selector("pins-failed"))
                .child(
                    div().flex().child(
                        Button::new("reload-pins")
                            .label(text::TRY_AGAIN)
                            .icon(Icon::Retry)
                            .selector("reload-pins")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.state.update(cx, |state, cx| state.reload_pins(cx));
                            })),
                    ),
                )
                .into_any_element(),
            (PinsState::Loaded(rows), Some(_)) if rows.is_empty() => {
                StatePanel::new(text::NO_PINS_TITLE, text::NO_PINS)
                    .selector("no-pins")
                    .into_any_element()
            }
            (PinsState::Loaded(rows), Some(feed)) => {
                let server_id = feed.server_id;
                let mut shown: Vec<AnyElement> = Vec::new();
                for (index, row) in rows.iter().enumerate() {
                    let message_id = row.message.id;
                    // The window goes to the message as it goes to the message of a
                    // notice: a pinned reply of a thread is shown in its thread, at its
                    // place, and the list marks it (ADR 0741).
                    let jump = Jump::to(server_id, &row.message);
                    let message = self.render_message(row, feed, MessageShape::Bare, window, cx);
                    shown.push(self.message_preview_menu(
                        row,
                        feed,
                        (format!("pin-actions:{message_id}").into(), false),
                        window,
                        cx,
                        |_, _, cx| {
                            div()
                                .flex_none()
                                .debug_selector(move || format!("pin:{message_id}"))
                                .pt(u(6.0))
                                .pb(u(7.0))
                                .when(index > 0, |entry| {
                                    entry
                                        .border_t_1()
                                        .border_color(Hsla::from(colors.rule_soft))
                                })
                                .child(message)
                                .child(
                                    div()
                                        .flex()
                                        .gap(u(6.0))
                                        .pl(u(58.0))
                                        .pt(u(3.0))
                                        .child(
                                            Button::new(("go-to-pin", index))
                                                .label(text::GO_TO_MESSAGE)
                                                .selector("go-to-pin")
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.go_to(jump, cx);
                                                })),
                                        )
                                        .when(row.can_pin, |actions| {
                                            actions.child(
                                                Button::new(("unpin", index))
                                                    .icon(Icon::Pin)
                                                    .label(text::UNPIN)
                                                    .selector("unpin")
                                                    .on_click(cx.listener(
                                                        move |this, _, _, cx| {
                                                            this.state.update(cx, |state, cx| {
                                                                state.set_pinned(
                                                                    server_id, message_id, false,
                                                                    cx,
                                                                );
                                                            });
                                                        },
                                                    )),
                                            )
                                        }),
                                )
                                .into_any_element()
                        },
                    ));
                }
                ScrollArea::new("pin-list", &self.pins_scroll, text::PINS)
                    .selector("pin-list")
                    .children(shown)
                    .child(div().flex_none().h(u(6.0)))
                    .into_any_element()
            }
        };
        div()
            .id("side-panel")
            .role(Role::Complementary)
            .aria_label(text::PINS)
            .debug_selector(|| "pins-panel".to_owned())
            .flex()
            .flex_col()
            .flex_none()
            .w(u(self.panel_width(
                junto_client_core::settings::Panel::Pins,
                theme.metrics.side_panel_width,
                cx,
            )))
            .h_full()
            .bg(Hsla::from(colors.plaster))
            .border_l_1()
            .border_color(Hsla::from(colors.rule))
            .child(header)
            .child(body)
            .into_any_element()
    }

    /// Opens the dialog that forks a thread into a text or a voice channel.
    pub fn open_fork_dialog(
        &mut self,
        root_id: MessageId,
        kind: ChannelKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| state.dismiss_fork(cx));
        self.fork_name
            .update(cx, |name, cx| name.set_value("", window, cx));
        self.open_layer(
            Layer::Fork(ForkDialog {
                root_id,
                kind,
                sent: false,
            }),
            window,
            cx,
        );
        self.focus_field(&self.fork_name.clone(), window, cx);
    }

    /// The name of the fork changed: what was told about the name before is put away.
    pub(crate) fn fork_name_changed(&mut self, cx: &mut Context<Self>) {
        // A name that the server refused is changed: the request is not on its way any
        // more, and the dialog takes a new one.
        let refused = self
            .view
            .as_ref()
            .and_then(|view| view.workspace())
            .and_then(|workspace| workspace.thread.as_ref())
            .is_some_and(|thread| matches!(thread.fork, Some(ForkState::Refused(_))));
        if let Layer::Fork(dialog) = &mut self.layer
            && (refused || !dialog.sent)
        {
            dialog.sent = false;
            self.state.update(cx, |state, cx| state.dismiss_fork(cx));
        }
        cx.notify();
    }

    /// Puts text into the field of the name of a fork, as typing does.
    pub fn type_fork_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        self.fork_name
            .update(cx, |field, cx| field.set_value(name, window, cx));
        self.fork_name_changed(cx);
    }

    /// Sends the fork of the dialog. The dialog stays until the server answered.
    pub fn submit_fork(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Layer::Fork(dialog) = &mut self.layer else {
            return;
        };
        let name = self.fork_name.read(cx).value().trim().to_owned();
        let server_id = self
            .view
            .as_ref()
            .and_then(|view| view.selected.as_ref())
            .map(|server| server.id);
        let (Some(server_id), false) = (server_id, name.is_empty()) else {
            return;
        };
        dialog.sent = true;
        let (root_id, kind) = (dialog.root_id, dialog.kind);
        self.state.update(cx, |state, cx| {
            state.fork_thread(server_id, root_id, name, kind, cx);
        });
        cx.notify();
    }

    /// True when the dialog of a fork has done its work: the request was sent, and the
    /// view holds no state of it any more.
    pub(crate) fn fork_is_done(dialog: &ForkDialog, view: &ShellView) -> bool {
        dialog.sent
            && view
                .workspace()
                .and_then(|workspace| workspace.thread.as_ref())
                .is_some_and(|thread| thread.fork.is_none())
    }

    pub(crate) fn render_fork_dialog(
        &mut self,
        view: &ShellView,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let Layer::Fork(dialog) = &self.layer else {
            return div().into_any_element();
        };
        let kind = dialog.kind;
        let state = view
            .workspace()
            .and_then(|workspace| workspace.thread.as_ref())
            .and_then(|thread| thread.fork.clone());
        let sending = state == Some(ForkState::Sending);
        let refused = match state {
            Some(ForkState::Refused(reason)) => Some(reason),
            _ => None,
        };
        let empty = self.fork_name.read(cx).value().trim().is_empty();
        Dialog::new(text::FORK_TITLE, self.layer_focus())
            .selector("fork-dialog")
            .on_close(Self::handler(cx, |this, window, cx| {
                this.state.update(cx, |state, cx| state.dismiss_fork(cx));
                this.close_layer(window, cx);
            }))
            .child(
                div()
                    .typeface(theme.types.notice, &theme)
                    .ink(colors.ink_2)
                    .child(text::FORK_ABOUT),
            )
            .child(
                div().flex().mt(u(12.0)).mb(u(8.0)).child(
                    crate::ui::Segmented::new(
                        "fork-kind",
                        text::FORK_KIND,
                        [ChannelKind::Text, ChannelKind::Voice]
                            .map(|kind| SharedString::from(kind_name(kind))),
                    )
                    .selected(Some(match kind {
                        ChannelKind::Text => 0,
                        ChannelKind::Voice => 1,
                    }))
                    .selector("fork-kind")
                    .on_select({
                        let choose = cx.listener(move |this, index: &usize, _, cx| {
                            if let Layer::Fork(dialog) = &mut this.layer
                                && !dialog.sent
                            {
                                dialog.kind = if *index == 0 {
                                    ChannelKind::Text
                                } else {
                                    ChannelKind::Voice
                                };
                                cx.notify();
                            }
                        });
                        move |index, window, cx| choose(&index, window, cx)
                    }),
                ),
            )
            .child(Field::new(&self.fork_name, text::FORK_NAME).selector("fork-name"))
            .child(dialog_footer([
                Button::new("cancel-fork")
                    .label(text::CANCEL)
                    .selector("cancel-fork")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.state.update(cx, |state, cx| state.dismiss_fork(cx));
                        this.close_layer(window, cx);
                    }))
                    .into_any_element(),
                Button::new("create-fork")
                    .kind(ButtonKind::Primary)
                    .label(if sending {
                        text::FORK_SENDING
                    } else {
                        text::FORK_CREATE
                    })
                    .disabled(sending || empty)
                    .selector("create-fork")
                    .on_click(cx.listener(|this, _, window, cx| this.submit_fork(window, cx)))
                    .into_any_element(),
            ]))
            .child(
                crate::ui::notice_slot("fork-refused-slot").children(refused.map(|reason| {
                    div()
                        .mt(u(12.0))
                        .child(Notice::error(reason).selector("fork-refused"))
                })),
            )
            .into_any_element()
    }

    /// Shows the pinned messages of a channel in the side panel.
    pub fn open_pins(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        self.members_open = false;
        self.state
            .update(cx, |state, cx| state.open_pins(server_id, channel_id, cx));
        cx.notify();
    }
}

impl Shell {
    /// Opens the followed threads of the shown workspace in the side panel, or closes them.
    pub fn toggle_followed_threads(&mut self, cx: &mut Context<Self>) {
        let view = self.state.read(cx).view();
        let Some((server_id, workspace_id, open)) = view.selected.as_ref().and_then(|server| {
            let workspace = view.workspace()?;
            Some((server.id, workspace.id, workspace.followed.is_some()))
        }) else {
            return;
        };
        self.members_open = false;
        self.state.update(cx, |state, cx| {
            if open {
                state.close_followed_threads(cx);
            } else {
                state.open_followed_threads(server_id, workspace_id, cx);
            }
        });
        cx.notify();
    }

    /// The entry at the top of the channel column that opens the threads of the workspace
    /// that the person follows (ADR 0741). `unread`: the unread threads that the channel
    /// column lists.
    pub(crate) fn render_threads_row(
        &mut self,
        workspace: &crate::state::view::WorkspaceView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let unread = workspace
            .sections
            .iter()
            .flat_map(|section| &section.rows)
            .flat_map(|row| &row.threads)
            .filter(|thread| thread.unread > 0 || thread.mentions > 0)
            .count() as u64;
        let sign = (unread > 0).then(|| crate::ui::UnreadSign {
            mentions: None,
            label: text::unread_count(unread).into(),
        });
        div()
            .flex_none()
            .debug_selector(|| "threads-row".to_owned())
            .child(
                crate::ui::ChannelRow::new("threads-row", Icon::Chat, text::THREADS)
                    .selected(workspace.followed.is_some())
                    .unread(sign)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_followed_threads(cx))),
            )
            .into_any_element()
    }

    /// The side panel of the threads of the shown workspace that the person follows.
    pub(crate) fn render_followed_panel(
        &mut self,
        panel: &crate::state::view::FollowedPanel,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let server_id = self
            .view
            .as_ref()
            .and_then(|view| view.selected.as_ref())
            .map(|server| server.id);
        let workspace_id = self
            .view
            .as_ref()
            .and_then(|view| view.workspace())
            .map(|workspace| workspace.id);
        let header = SideHeader::new(text::THREADS)
            .title_bar()
            .icon(Icon::Chat)
            .trailing(
                Button::new("close-followed")
                    .kind(ButtonKind::Ghost)
                    .icon(Icon::X)
                    .tooltip(text::CLOSE_THREADS)
                    .selector("close-followed")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state
                            .update(cx, |state, cx| state.close_followed_threads(cx));
                    })),
            );
        let body: AnyElement = if let Some(failure) =
            panel.failure.filter(|_| panel.rows.is_empty())
        {
            // An older server does not list these threads; a failed read can be read again
            // (ADR 0796).
            match failure {
                crate::state::followed::ListFailure::Unsupported => StatePanel::new(
                    text::FOLLOWED_THREADS_UNSUPPORTED_TITLE,
                    text::FOLLOWED_THREADS_UNSUPPORTED,
                )
                .selector("followed-unsupported")
                .into_any_element(),
                crate::state::followed::ListFailure::Failed => div()
                    .flex()
                    .flex_col()
                    .gap(u(8.0))
                    .px(u(14.0))
                    .pt(u(12.0))
                    .child(crate::ui::Notice::error(text::FOLLOWED_THREADS_FAILED))
                    .child(
                        div().flex().child(
                            Button::new("followed-retry")
                                .label(text::TRY_AGAIN)
                                .selector("followed-retry")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state
                                        .update(cx, |state, cx| state.retry_followed_threads(cx));
                                })),
                        ),
                    )
                    .into_any_element(),
            }
        } else if panel.rows.is_empty() && panel.reading {
            div()
                .px(u(14.0))
                .pt(u(12.0))
                .typeface(theme.types.small, &theme)
                .ink(colors.ink_2)
                .child(text::FOLLOWED_THREADS_READING)
                .into_any_element()
        } else if panel.rows.is_empty() {
            StatePanel::new(text::NO_FOLLOWED_THREADS_TITLE, text::NO_FOLLOWED_THREADS)
                .selector("no-followed-threads")
                .into_any_element()
        } else {
            // Each row is a row of the channel column: it takes the focus with Tab, shows the
            // focus ring, and opens its thread with Enter or Space (ADR 0796).
            let rows = panel.rows.iter().map(|row| {
                let (root_id, channel_id) = (row.root_id, row.channel_id);
                let title = row.title.clone().unwrap_or_else(|| text::THREAD.to_owned());
                let unread = row.unread.max(row.mentions);
                let sign = (unread > 0).then(|| crate::ui::UnreadSign {
                    mentions: (row.mentions > 0).then(|| text::unread_count(row.mentions).into()),
                    label: text::followed_thread_label(&title, &row.channel_name, unread).into(),
                });
                div()
                    .flex_none()
                    .debug_selector(move || format!("followed-thread:{root_id}"))
                    .child(
                        crate::ui::ChannelRow::new(
                            ("followed-thread", root_id.as_uuid().as_u128() as u64),
                            Icon::Chat,
                            title,
                        )
                        .detail(format!("# {}", row.channel_name))
                        .unread(sign)
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let (Some(server_id), Some(workspace_id)) =
                                    (server_id, workspace_id)
                                else {
                                    return;
                                };
                                this.state.update(cx, |state, cx| {
                                    state.select_channel(workspace_id, channel_id, cx)
                                });
                                this.open_thread(server_id, channel_id, root_id, window, cx);
                            },
                        )),
                    )
                    .into_any_element()
            });
            ScrollArea::new("followed-threads", &self.followed_scroll, text::THREADS)
                .selector("followed-threads")
                .children(rows)
                .when(panel.more, |list| {
                    list.child(
                        div().flex().px(u(14.0)).py(u(8.0)).child(
                            Button::new("more-followed")
                                .label(text::followed::SHOW_MORE)
                                .selector("more-followed")
                                .disabled(panel.reading)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.state.update(cx, |state, cx| {
                                        state.read_more_followed_threads(cx)
                                    });
                                })),
                        ),
                    )
                })
                .into_any_element()
        };
        super::notices::side_panel(
            &theme,
            text::THREADS,
            "followed-panel",
            self.panel_width(
                junto_client_core::settings::Panel::Notices,
                theme.metrics.side_panel_width,
                cx,
            ),
        )
        .child(header)
        .child(body)
        .into_any_element()
    }
}
