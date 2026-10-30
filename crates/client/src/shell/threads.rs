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
use crate::state::feed::Feed;
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
                Button::new("close-thread")
                    .kind(ButtonKind::Ghost)
                    .icon(Icon::X)
                    .tooltip(text::CLOSE_THREAD)
                    .selector("close-thread")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |state, cx| state.close_thread(cx));
                    })),
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
            vec![
                self.render_fork_bar(thread, &theme, cx),
                self.render_follow_bar(thread, &theme, cx),
                self.render_thread_root(thread, &theme, window, cx),
                self.render_pane(Pane::Thread, &shown, Vec::new(), window, cx),
            ]
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

    /// The controls of a fork, and the channels that the thread was forked into.
    fn render_fork_bar(
        &mut self,
        thread: &ThreadPanel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let root_id = thread.root_id;
        let workspace_id = self
            .view
            .as_ref()
            .and_then(|view| view.workspace())
            .map(|workspace| workspace.id);
        let forked = !thread.forks.is_empty();
        let fork = |id: &'static str, kind: ChannelKind, cx: &mut Context<Self>| {
            Button::new(id)
                .icon(match kind {
                    ChannelKind::Text => Icon::Hash,
                    ChannelKind::Voice => Icon::Speaker,
                })
                .label(kind_name(kind))
                .tooltip(if thread.may_fork {
                    text::FORK_TITLE
                } else {
                    text::FORK_NEEDS_PERMISSION
                })
                .disabled(!thread.may_fork)
                .selector(id)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_fork_dialog(root_id, kind, window, cx);
                }))
        };
        let links = thread
            .forks
            .iter()
            .enumerate()
            .map(|(index, (channel_id, name, kind))| {
                let channel_id = *channel_id;
                Button::new(("fork-link", index))
                    .icon(match kind {
                        ChannelKind::Text => Icon::Hash,
                        ChannelKind::Voice => Icon::Speaker,
                    })
                    .label(name.clone())
                    .tooltip(text::FORKED_INTO)
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
            .id("fork-bar")
            .debug_selector(|| "fork-bar".to_owned())
            .flex()
            .flex_col()
            .flex_none()
            .gap(u(7.0))
            .px(u(14.0))
            .pt(u(9.0))
            .pb(u(10.0))
            .border_b_1()
            .border_color(Hsla::from(colors.rule))
            .bg(Hsla::from(colors.limestone))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(7.0))
                    .child(icon(Icon::Fork, 14.0, colors.terracotta_deep))
                    .child(
                        div()
                            .flex_none()
                            .typeface(theme.types.control_strong, theme)
                            .text_size(u(13.0))
                            .ink(colors.ink)
                            .child(if forked {
                                text::FORKED_INTO
                            } else {
                                text::FORK_THREAD
                            }),
                    )
                    .child(div().flex_1())
                    .when(!forked, |line| {
                        line.child(
                            div()
                                .flex_none()
                                .text_size(u(12.5))
                                .ink(colors.ink_2)
                                .child(text::FORK_COPIES),
                        )
                    }),
            )
            .child(if forked {
                div()
                    .flex()
                    .flex_wrap()
                    .gap(u(6.0))
                    .children(links)
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .gap(u(6.0))
                    .child(
                        div()
                            .flex_1()
                            .child(fork("fork-text", ChannelKind::Text, cx)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(fork("fork-voice", ChannelKind::Voice, cx)),
                    )
                    .into_any_element()
            })
            .into_any_element()
    }

    /// The control that states whether the person follows the thread, with the actions
    /// that change it (ADR 0412). Nothing is drawn until the server has told the state.
    fn render_follow_bar(
        &mut self,
        thread: &ThreadPanel,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let Some(follow) = &thread.follow else {
            return div().flex_none().into_any_element();
        };
        let following = follow.following;
        let state = text::followed::state(following, follow.setting);
        // An archived channel takes no change, and one change is on its way at a time.
        let held = follow.busy || follow.read_only;
        let change = Button::new("thread-follow")
            .icon(if following { Icon::BellOff } else { Icon::Bell })
            .label(text::followed::action(following))
            .disabled(held)
            .when(follow.read_only, |button| {
                button.tooltip(text::followed::ARCHIVED)
            })
            .selector("thread-follow")
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state
                    .update(cx, |state, cx| state.follow_thread(!following, cx));
            }));
        // The choice of the person can be removed: the rule of the server decides again.
        let default = follow.setting.map(|_| {
            Button::new("thread-follow-default")
                .kind(ButtonKind::Ghost)
                .label(text::followed::USE_DEFAULT)
                .tooltip(if follow.read_only {
                    text::followed::ARCHIVED
                } else {
                    text::followed::USE_DEFAULT_HELP
                })
                .disabled(held)
                .when(held, |button| button.opacity(0.55))
                .selector("thread-follow-default")
                .on_click(cx.listener(|this, _, _, cx| {
                    this.state
                        .update(cx, |state, cx| state.clear_thread_follow(cx));
                }))
        });
        div()
            .id("follow-bar")
            .role(Role::Group)
            .aria_label(text::followed::CONTROL)
            .debug_selector(|| "follow-bar".to_owned())
            .flex()
            .flex_col()
            .flex_none()
            .gap(u(7.0))
            .px(u(14.0))
            .pt(u(9.0))
            .pb(u(10.0))
            .border_b_1()
            .border_color(Hsla::from(colors.rule))
            .child(
                div()
                    .id("thread-follow-state")
                    .role(Role::Status)
                    .aria_label(state)
                    .debug_selector(|| "thread-follow-state".to_owned())
                    .typeface(theme.types.small, theme)
                    .ink(colors.ink_2)
                    .child(state),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(6.0))
                    .child(change)
                    .children(default),
            )
            .child(
                crate::ui::notice_slot("thread-follow-refused-slot").children(
                    follow
                        .refused
                        .clone()
                        .map(|reason| Notice::error(reason).selector("thread-follow-refused")),
                ),
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
                    .child(self.render_message(0, root, feed, MessageShape::Bare, window, cx)),
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
                    let message =
                        self.render_message(index, row, feed, MessageShape::Bare, window, cx);
                    shown.push(
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
                                                .on_click(cx.listener(move |this, _, _, cx| {
                                                    this.state.update(cx, |state, cx| {
                                                        state.set_pinned(
                                                            server_id, message_id, false, cx,
                                                        );
                                                    });
                                                })),
                                        )
                                    }),
                            )
                            .into_any_element(),
                    );
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
                div()
                    .flex()
                    .items_center()
                    .gap(u(7.0))
                    .mt(u(12.0))
                    .mb(u(8.0))
                    .typeface(theme.types.control_strong, &theme)
                    .ink(colors.ink)
                    .child(icon(
                        match kind {
                            ChannelKind::Text => Icon::Hash,
                            ChannelKind::Voice => Icon::Speaker,
                        },
                        15.0,
                        colors.terracotta_deep,
                    ))
                    .child(kind_name(kind)),
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
