//! The notices of a server in the side panel, the entry of the channel column that opens
//! them, the menu of a channel and of a direct conversation, the dialog of the
//! notification settings of a scope, and the quiet times of the application settings
//! (ADR 0742). The design draws none of them; they are built from its components.

use std::ops::Range;

use gpui::{
    AnyElement, Context, Entity, FontWeight, HighlightStyle, Hsla, InteractiveElement, IntoElement,
    ParentElement, Role, SharedString, StatefulInteractiveElement, Styled, StyledText, Window,
    deferred, div, prelude::FluentBuilder,
};
use gpui_base::input::InputState;
use junto_client_core::api::SettingsScope;
use junto_client_core::protocol::discovery::NotificationLevel;
use junto_client_core::protocol::ids::{AccountId, ChannelId, ConversationId, ServerId};
use junto_client_core::protocol::structure::ChannelKind;

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::MentionSwitch;
use crate::state::admin::view::notifications::DefaultChoice;
use crate::state::notices::{
    Agreement, Jump, Mute, MuteFor, NoticeRow, NoticesPanel, NotifyPanel, QuietView, Whereabouts,
    clock_text, parse_clock,
};
use crate::state::view::{ServerLists, ShellView};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{
    Avatar, Button, ButtonKind, ChannelRow, Dialog, Field, Menu, MenuItem, Notice, ScrollArea,
    Segmented, SideHeader, StatePanel, StyleExt, Toggle, UnreadSign, icon, menu_label,
    menu_separator, unread_dot,
};

/// The icon of the place of a message.
pub(crate) fn place_icon(place: &Whereabouts) -> Icon {
    match place {
        Whereabouts::Channel {
            kind: ChannelKind::Voice,
            ..
        } => Icon::Speaker,
        Whereabouts::Channel { .. } | Whereabouts::Unknown => Icon::Hash,
        Whereabouts::Direct { .. } => Icon::People,
    }
}

/// A message outside its list, as a notice and a search result show it (view H of the
/// design): the line of its place, and its author, its time and its text.
pub(crate) struct Excerpt {
    pub icon: Icon,
    pub place: SharedString,
    pub day: SharedString,
    /// The words at the end of the line of the place, such as "Go to message".
    pub action: Option<SharedString>,
    /// The seed of the generated avatar of the author.
    pub seed: u128,
    /// The picture of the author, when the window holds it (ADR 0783).
    pub picture: Option<std::sync::Arc<gpui::RenderImage>>,
    pub author: SharedString,
    pub time: SharedString,
    /// A short word after the time, such as "microphone".
    pub tag: Option<SharedString>,
    pub text: SharedString,
    /// The parts of the text that are marked as matches of a search.
    pub marks: Vec<Range<usize>>,
    /// The person has not read the entry: a dot stands before its place.
    pub unread: bool,
}

impl Excerpt {
    pub(crate) fn element(self, theme: &Theme, cx: &gpui::App) -> gpui::Div {
        let colors = theme.colors;
        let mark = HighlightStyle {
            color: Some(colors.ink.into()),
            background_color: Some(colors.ochre.into()),
            font_weight: Some(FontWeight::BOLD),
            ..HighlightStyle::default()
        };
        // An emoji that the fonts do not draw is shown by its stand-in, and the marks of
        // a search follow it (ADR 0758).
        let shown = crate::emoji::Drawings::current(cx).shown(&self.text);
        let marks: Vec<(Range<usize>, HighlightStyle)> = self
            .marks
            .into_iter()
            .filter(|range| {
                range.start < range.end
                    && self.text.is_char_boundary(range.start)
                    && self.text.is_char_boundary(range.end)
                    && range.end <= self.text.len()
            })
            .map(|range| shown.range(range))
            .fold(Vec::new(), |mut kept: Vec<Range<usize>>, range| {
                // Two marks in one replaced emoji become one range.
                if kept.last().is_none_or(|last| last.end <= range.start) {
                    kept.push(range);
                }
                kept
            })
            .into_iter()
            .map(|range| (range, mark))
            .collect();
        let text = SharedString::from(shown.into_owned());
        div()
            .flex()
            .flex_col()
            .pt(u(8.0))
            .px(u(14.0))
            .pb(u(10.0))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(6.0))
                    .mb(u(4.0))
                    .text_size(u(12.5))
                    .line_height(u(18.0))
                    .ink(colors.ink_2)
                    .whitespace_nowrap()
                    .when(self.unread, |line| line.child(unread_dot(cx)))
                    .child(icon(self.icon, 13.0, colors.ink_3))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(FontWeight::BOLD)
                            .ink(colors.cypress)
                            .child(self.place),
                    )
                    .child(div().flex_none().child(self.day))
                    .child(div().flex_1())
                    .when_some(self.action, |line, action| {
                        line.child(
                            div()
                                .flex_none()
                                .font_weight(FontWeight::BOLD)
                                .ink(colors.cypress)
                                .child(action),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .gap(u(10.0))
                    .child(
                        div()
                            .flex_none()
                            .mt(u(2.0))
                            .child(Avatar::new(self.seed, 30.0).image(self.picture)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(u(8.0))
                                    .line_height(u(19.0))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .font_weight(FontWeight::BOLD)
                                            .text_size(u(14.5))
                                            .ink(colors.ink)
                                            .child(self.author),
                                    )
                                    .child(
                                        div()
                                            .flex_none()
                                            .text_size(u(12.5))
                                            .ink(colors.ink_3)
                                            .child(self.time),
                                    )
                                    .when_some(self.tag, |line, tag| {
                                        line.child(
                                            div()
                                                .flex_none()
                                                .text_size(u(12.5))
                                                .ink(colors.ink_3)
                                                .child(tag),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .text_size(u(14.5))
                                    .line_height(u(21.0))
                                    .ink(colors.ink)
                                    .child(StyledText::new(text).with_highlights(marks)),
                            ),
                    ),
            )
    }
}

/// The frame of a list in the side panel.
pub(crate) fn side_panel(
    theme: &Theme,
    label: impl Into<SharedString>,
    selector: &'static str,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id("side-panel")
        .role(Role::Complementary)
        .aria_label(label.into())
        .debug_selector(move || selector.to_owned())
        .flex()
        .flex_col()
        .flex_none()
        .w(u(theme.metrics.side_panel_width))
        .h_full()
        .bg(Hsla::from(theme.colors.plaster))
        .border_l_1()
        .border_color(Hsla::from(theme.colors.rule))
}

impl Shell {
    /// Opens the notices of the shown server in the side panel, or closes them.
    pub fn toggle_notices(&mut self, cx: &mut Context<Self>) {
        let view = self.state.read(cx).view();
        let Some((server_id, open)) = view
            .selected
            .as_ref()
            .and_then(|server| Some((server.id, view.lists()?.notices.is_some())))
        else {
            return;
        };
        if open {
            self.state.update(cx, |state, cx| state.close_notices(cx));
        } else {
            self.members_open = false;
            self.state
                .update(cx, |state, cx| state.open_notices(server_id, cx));
        }
        cx.notify();
    }

    /// The entry of the channel column that opens the notices of the server, with the
    /// count of the notices that the person has not read. `in_call`: the main area shows
    /// the call of this device, whose side panel is the panel of the call.
    pub(crate) fn render_notices_row(
        &mut self,
        lists: &ServerLists,
        in_call: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let unread = lists.unread_notices;
        let sign = (unread > 0).then(|| UnreadSign {
            mentions: Some(text::unread_count(unread as u64).into()),
            label: text::unread_notices(unread).into(),
        });
        div()
            .flex_none()
            .debug_selector(|| "notices-row".to_owned())
            .child(
                ChannelRow::new("notices-row", Icon::Bell, text::NOTIFICATIONS)
                    .selected(lists.notices.is_some())
                    .unread(sign)
                    .unavailable(in_call.then_some(text::NOTICES_IN_CALL))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_notices(cx))),
            )
            .when(unread > 0, |row| {
                row.child(
                    div()
                        .absolute()
                        .size_0()
                        .debug_selector(|| "notices-count".to_owned()),
                )
            })
            .into_any_element()
    }

    /// Goes to the message of a notice and marks the notice as read.
    fn go_to_notice(&mut self, row: &NoticeRow, cx: &mut Context<Self>) {
        let Some(jump) = row.jump else {
            return;
        };
        let (notice_id, unread) = (row.id, !row.read);
        self.go_to(jump, cx);
        if unread {
            self.state.update(cx, |state, cx| {
                state.mark_notice_read(jump.server_id, notice_id, cx);
            });
        }
    }

    /// Shows a message in its place, as a notice and a search result ask for it.
    pub(crate) fn go_to(&mut self, jump: Jump, cx: &mut Context<Self>) {
        self.members_open = false;
        self.state.update(cx, |state, cx| state.go_to(jump, cx));
        cx.notify();
    }

    /// The side panel of the notices of the shown server.
    pub(crate) fn render_notices_panel(
        &mut self,
        server_id: ServerId,
        panel: &NoticesPanel,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let header = SideHeader::new(text::NOTIFICATIONS)
            .icon(Icon::Bell)
            .note(if panel.unread > 0 {
                text::unread_notices(panel.unread)
            } else {
                String::new()
            })
            .trailing(
                Button::new("close-notices")
                    .kind(ButtonKind::Ghost)
                    .icon(Icon::X)
                    .tooltip(text::CLOSE_NOTIFICATIONS)
                    .selector("close-notices")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.state.update(cx, |state, cx| state.close_notices(cx));
                    })),
            );
        let bar = (panel.unread > 0).then(|| {
            div()
                .flex()
                .flex_none()
                .items_center()
                .px(u(14.0))
                .pt(u(8.0))
                .pb(u(9.0))
                .bg(Hsla::from(colors.limestone))
                .border_b_1()
                .border_color(Hsla::from(colors.rule))
                .child(
                    Button::new("mark-all-read")
                        .icon(Icon::Check)
                        .label(if panel.marking {
                            text::MARKING_ALL_READ
                        } else {
                            text::MARK_ALL_READ
                        })
                        .disabled(panel.marking)
                        .selector("mark-all-read")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.mark_all_notices_read(server_id, cx);
                            });
                        })),
                )
        });
        let failed = panel.failed.clone().map(|reason| {
            div().flex_none().px(u(14.0)).pt(u(10.0)).child(
                Notice::error(reason).selector("notices-failed").action(
                    Button::new("dismiss-notices-failed")
                        .label(text::DISMISS)
                        .selector("dismiss-notices-failed")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.dismiss_notices_failure(server_id, cx);
                            });
                        })),
                ),
            )
        });
        // The notices of archived channels whose list could not be read (ADR 0762).
        let archive_failed = (!panel.archive_failed.is_empty()).then(|| {
            let workspaces = panel.archive_failed.clone();
            div().flex_none().px(u(14.0)).pt(u(10.0)).child(
                Notice::error(text::ARCHIVED_NOTICES_NOT_READ)
                    .selector("notices-archive-failed")
                    .action(
                        Button::new("notices-archive-retry")
                            .label(text::TRY_AGAIN)
                            .selector("notices-archive-retry")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.read_archived_lists_again(server_id, &workspaces, cx);
                                });
                            })),
                    ),
            )
        });
        let body: AnyElement = if panel.rows.is_empty() && panel.reading {
            div()
                .px(u(14.0))
                .pt(u(12.0))
                .typeface(theme.types.small, &theme)
                .ink(colors.ink_2)
                .child(text::NOTICES_READING)
                .into_any_element()
        } else if panel.rows.is_empty() && !panel.more {
            StatePanel::new(text::NO_NOTICES_TITLE, text::NO_NOTICES)
                .selector("no-notices")
                .into_any_element()
        } else {
            let faces = self.faces(server_id);
            let rows = panel.rows.iter().enumerate().map(|(index, row)| {
                let notice_id = row.id;
                let shown = Excerpt {
                    icon: place_icon(&row.place),
                    place: text::place_name(&row.place).into(),
                    day: row.day.clone().into(),
                    action: row.jump.map(|_| text::GO_TO_MESSAGE.into()),
                    seed: row.author_id.map_or(0, |id| id.as_uuid().as_u128()),
                    picture: row.author_id.and_then(|id| faces.image(id)),
                    author: row.author.clone().into(),
                    time: row.time.clone().into(),
                    tag: row.jump.map(|_| text::notice_kind(row.kind).into()),
                    text: row.text.clone().into(),
                    marks: Vec::new(),
                    unread: !row.read,
                };
                let label: SharedString = format!(
                    "{}, {}: {}{}",
                    row.author,
                    text::place_name(&row.place),
                    row.text,
                    if row.read {
                        String::new()
                    } else {
                        format!(", {}", text::NOTICE_UNREAD)
                    }
                )
                .into();
                let ring =
                    crate::ui::focus_ring_inside(colors.terracotta_deep, theme.metrics.focus_ring);
                let held = row.clone();
                div()
                    .id(("notice", index))
                    .role(Role::ListItem)
                    .aria_label(label)
                    .debug_selector(move || format!("notice:{notice_id}"))
                    .flex_none()
                    .when(index > 0, |entry| {
                        entry
                            .border_t_1()
                            .border_color(Hsla::from(colors.rule_soft))
                    })
                    .when(row.jump.is_some(), |entry| {
                        entry
                            .cursor_pointer()
                            .focusable()
                            .tab_stop(true)
                            .hover(move |style| style.bg(Hsla::from(colors.row_hover)))
                            .focus_visible(move |style| style.shadow(ring))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.go_to_notice(&held, cx);
                            }))
                    })
                    .child(shown.element(&theme, cx))
                    .into_any_element()
            });
            let more = panel.more.then(|| {
                div().flex().flex_none().px(u(14.0)).py(u(8.0)).child(
                    Button::new("more-notices")
                        .label(text::SHOW_OLDER)
                        .selector("more-notices")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state
                                .update(cx, |state, _| state.read_more_notices(server_id));
                        })),
                )
            });
            ScrollArea::new("notice-list", &self.notices_scroll, text::NOTIFICATIONS)
                .selector("notice-list")
                .children(rows)
                .children(more)
                .child(div().flex_none().h(u(6.0)))
                .into_any_element()
        };
        side_panel(&theme, text::NOTIFICATIONS, "notices-panel")
            .child(header)
            .child(crate::ui::notice_slot("notices-bar-slot").children(bar))
            .child(
                crate::ui::notice_slot("notices-failed-slot")
                    .children(failed)
                    .children(archive_failed),
            )
            .child(body)
            .into_any_element()
    }

    // The menu of a channel and of a direct conversation.

    /// Opens the menu of a channel of the shown workspace.
    pub fn open_channel_menu(
        &mut self,
        channel_id: ChannelId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_layer(Layer::ChannelMenu(channel_id), window, cx);
    }

    pub fn open_direct_menu(
        &mut self,
        conversation_id: ConversationId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_layer(Layer::DirectMenu(conversation_id), window, cx);
    }

    /// The items that mute a place, or the state of its mute with the item that ends it.
    fn mute_items(
        &mut self,
        mute: &Mute,
        prefix: &'static str,
        cx: &mut Context<Self>,
        set: impl Fn(&mut Self, Option<MuteFor>, &mut Context<Self>) + Clone + 'static,
    ) -> Vec<AnyElement> {
        let theme = theme(cx);
        let mut items: Vec<AnyElement> = Vec::new();
        if let Some(state) = text::mute_state(mute) {
            items.push(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(u(9.0))
                    .px(u(12.0))
                    .pt(u(5.0))
                    .pb(u(3.0))
                    .debug_selector(move || format!("{prefix}-muted-state"))
                    .typeface(theme.types.small_strong, &theme)
                    .ink(theme.colors.ink_2)
                    .child(icon(Icon::BellOff, 15.0, theme.colors.ink_2))
                    .child(SharedString::from(state))
                    .into_any_element(),
            );
            let end = set.clone();
            items.push(
                div()
                    .flex_none()
                    .debug_selector(move || format!("{prefix}-unmute"))
                    .child(
                        MenuItem::new("unmute", text::UNMUTE)
                            .icon(Icon::Bell)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                end(this, None, cx);
                                this.close_layer(window, cx);
                            })),
                    )
                    .into_any_element(),
            );
            return items;
        }
        items.push(menu_label(text::MUTE, false, cx).into_any_element());
        for (index, length) in MuteFor::ALL.into_iter().enumerate() {
            let set = set.clone();
            items.push(
                div()
                    .flex_none()
                    .debug_selector(move || format!("{prefix}-mute-{index}"))
                    .child(
                        MenuItem::new(("mute", index), text::mute_for(length))
                            .icon(Icon::BellOff)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                set(this, Some(length), cx);
                                this.close_layer(window, cx);
                            })),
                    )
                    .into_any_element(),
            );
        }
        items
    }

    /// The menu of a channel, drawn at its row: its notification settings and its mute.
    pub(crate) fn render_channel_menu(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        name: &str,
        mute: &Mute,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut items = vec![
            MenuItem::new("channel-notify-settings", text::NOTIFICATION_SETTINGS_ITEM)
                .icon(Icon::Bell)
                .selector("channel-notify-settings")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.open_notify_settings(
                        server_id,
                        SettingsScope::Channel(channel_id),
                        window,
                        cx,
                    );
                }))
                .into_any_element(),
            menu_separator(cx).into_any_element(),
        ];
        items.extend(
            self.mute_items(mute, "channel", cx, move |this, length, cx| {
                this.state.update(cx, |state, cx| {
                    state.set_channel_mute(server_id, channel_id, length, cx);
                });
            }),
        );
        self.row_menu(
            "channel-menu",
            format!("{}: {name}", text::CHANNEL_MENU),
            items,
            cx,
        )
    }

    /// The menu of a direct conversation, drawn at its row: its mute (ADR 0515).
    pub(crate) fn render_direct_menu(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        name: &str,
        mute: &Mute,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items = self.mute_items(mute, "direct", cx, move |this, length, cx| {
            this.state.update(cx, |state, cx| {
                state.set_direct_mute(server_id, account_id, length, cx);
            });
        });
        self.row_menu(
            "direct-menu",
            format!("{}: {name}", text::CONVERSATION_MENU),
            items,
            cx,
        )
    }

    fn row_menu(
        &mut self,
        selector: &'static str,
        label: String,
        items: Vec<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .absolute()
            .left(u(40.0))
            .top(u(26.0))
            .child(deferred(
                Menu::new(selector, label, self.layer_focus())
                    .selector(selector)
                    .width(232.0)
                    .on_close(Self::handler(cx, |this, window, cx| {
                        this.close_layer(window, cx);
                    }))
                    .children(items)
                    .child(div().h(u(4.0))),
            ))
            .into_any_element()
    }

    // The dialog of the notification settings of a scope.

    /// Shows the notification settings of a scope in their dialog.
    pub fn open_notify_settings(
        &mut self,
        server_id: ServerId,
        scope: SettingsScope,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state.update(cx, |state, cx| {
            state.open_notify_settings(server_id, scope, cx);
        });
        // The menu that asked for the dialog gives its place to it.
        if matches!(self.layer, Layer::None) {
            self.open_layer(Layer::Notify, window, cx);
        } else {
            self.layer = Layer::Notify;
            self.layer_focus().clone().focus(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn close_notify_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.state
            .update(cx, |state, cx| state.close_notify_settings(cx));
        self.close_layer(window, cx);
    }

    pub(crate) fn render_notify_dialog(
        &mut self,
        view: &ShellView,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let Some(panel) = view.notify.clone() else {
            return div().into_any_element();
        };
        let NotifyPanel {
            server_id, scope, ..
        } = panel;
        let group = |label: &'static str| {
            div()
                .mb(u(5.0))
                .typeface(theme.types.label, &theme)
                .ink(colors.bronze)
                .child(label)
        };
        let help = |sentence: SharedString| {
            div()
                .mt(u(5.0))
                .typeface(theme.types.small, &theme)
                .ink(colors.ink_2)
                .child(sentence)
        };
        // The first choice takes the level of the scope above. The server has no scope
        // above the account: its choices are the three levels.
        let levels = [
            NotificationLevel::All,
            NotificationLevel::Mentions,
            NotificationLevel::Nothing,
        ];
        let inherits = scope != SettingsScope::Account;
        let mut options: Vec<SharedString> = Vec::new();
        let mut choices: Vec<Option<NotificationLevel>> = Vec::new();
        if inherits {
            options.push(text::LEVEL_DEFAULT.into());
            choices.push(None);
        }
        for level in levels {
            options.push(text::level_name(level).into());
            choices.push(Some(level));
        }
        // The account holds "mentions only" while it stored no level.
        let stored = match (panel.level.stored, inherits) {
            (None, false) => Some(NotificationLevel::Mentions),
            (stored, _) => stored,
        };
        let selected = choices.iter().position(|choice| *choice == stored);
        let busy = panel.busy;
        let level = Segmented::new("notify-level", text::NOTIFY_LEVEL, options)
            .selected(selected)
            .selector("notify-level")
            .on_select({
                let select = cx.listener(move |this, index: &usize, _, cx| {
                    let Some(level) = choices.get(*index).copied() else {
                        return;
                    };
                    if busy {
                        return;
                    }
                    this.state.update(cx, |state, cx| {
                        state.set_notify_level(server_id, scope, level, cx);
                    });
                });
                move |index, window, cx| select(&index, window, cx)
            });
        let about_level: SharedString = match scope {
            SettingsScope::Account => text::LEVEL_SERVER_HELP.into(),
            SettingsScope::Workspace(_) => text::LEVEL_WORKSPACE_HELP.into(),
            SettingsScope::Channel(_) => text::LEVEL_CHANNEL_HELP.into(),
        };
        let about_default: Option<SharedString> = inherits.then(|| match panel.level.inherited {
            Some(level) => text::level_default_now(level).into(),
            None => text::LEVEL_DEFAULT_UNKNOWN.into(),
        });
        let mut dialog = Dialog::new(
            text::notifications_of(scope, &panel.name),
            self.layer_focus(),
        )
        .selector("notify-dialog")
        .on_close(Self::handler(cx, |this, window, cx| {
            this.close_notify_settings(window, cx);
        }))
        .child(group(text::NOTIFY_LEVEL))
        .child(div().flex().child(level))
        .children(about_default.map(|sentence| {
            help(sentence)
                .id("level-default")
                .debug_selector(|| "level-default".to_owned())
        }))
        .child(help(about_level));

        if let (Some(channel), SettingsScope::Channel(channel_id)) = (&panel.channel, scope) {
            let switches = [
                (
                    MentionSwitch::Roles,
                    channel.role_mentions,
                    text::ROLE_MENTIONS,
                    text::ROLE_MENTIONS_HELP,
                    "role-mentions",
                ),
                (
                    MentionSwitch::Everyone,
                    channel.everyone_mentions,
                    text::EVERYONE_MENTIONS,
                    text::EVERYONE_MENTIONS_HELP,
                    "everyone-mentions",
                ),
            ];
            for (switch, on, title, sentence, selector) in switches {
                dialog = dialog.child(
                    div()
                        .flex()
                        .items_start()
                        .gap(u(10.0))
                        .mt(u(12.0))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .typeface(theme.types.control, &theme)
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .ink(colors.ink)
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .typeface(theme.types.small, &theme)
                                        .ink(colors.ink_2)
                                        .child(sentence),
                                ),
                        )
                        .child(
                            Toggle::new(selector, title, on, (text::ON, text::OFF))
                                .selector(selector)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if busy {
                                        return;
                                    }
                                    this.state.update(cx, |state, cx| {
                                        state.set_mention_switch(
                                            server_id, channel_id, switch, !on, cx,
                                        );
                                    });
                                })),
                        ),
                );
            }
            let muted = channel.mute.is_on();
            let state: SharedString = text::mute_state(&channel.mute)
                .unwrap_or_else(|| text::MUTE_HELP.to_owned())
                .into();
            let mut actions: Vec<AnyElement> = Vec::new();
            if muted {
                actions.push(
                    Button::new("dialog-unmute")
                        .icon(Icon::Bell)
                        .label(text::UNMUTE)
                        .disabled(busy)
                        .selector("dialog-unmute")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.set_channel_mute(server_id, channel_id, None, cx);
                            });
                        }))
                        .into_any_element(),
                );
            } else {
                for (index, length) in MuteFor::ALL.into_iter().enumerate() {
                    actions.push(
                        div()
                            .flex_none()
                            .debug_selector(move || format!("dialog-mute-{index}"))
                            .child(
                                Button::new(("dialog-mute", index))
                                    .label(text::mute_for(length))
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.state.update(cx, |state, cx| {
                                            state.set_channel_mute(
                                                server_id,
                                                channel_id,
                                                Some(length),
                                                cx,
                                            );
                                        });
                                    })),
                            )
                            .into_any_element(),
                    );
                }
            }
            dialog = dialog
                .child(div().mt(u(16.0)).child(group(text::MUTE)))
                .child(div().flex().flex_wrap().gap(u(6.0)).children(actions))
                .child(help(state));
        }

        if let (Some(default), SettingsScope::Workspace(workspace_id)) =
            (panel.workspace_default, scope)
        {
            dialog = dialog.child(div().mt(u(16.0)).child(group(text::WORKSPACE_DEFAULT)));
            let held = default.level;
            dialog = if default.may_set {
                let shown = held;
                let changing = default.changing;
                dialog.child(
                    div().flex().child(
                        Segmented::new(
                            "workspace-default",
                            text::WORKSPACE_DEFAULT,
                            levels.map(|level| SharedString::from(text::level_name(level))),
                        )
                        .selected(
                            held.and_then(|held| levels.iter().position(|level| *level == held)),
                        )
                        .selector("workspace-default")
                        .on_select({
                            let select = cx.listener(move |this, index: &usize, _, cx| {
                                let Some(level) = levels.get(*index).copied() else {
                                    return;
                                };
                                if busy || changing || shown == Some(level) {
                                    return;
                                }
                                this.state.update(cx, |state, cx| {
                                    state.set_workspace_default(server_id, workspace_id, level, cx);
                                });
                            });
                            move |index, window, cx| select(&index, window, cx)
                        }),
                    ),
                )
            } else {
                // A member reads the level and does not set it.
                dialog.child(
                    div()
                        .id("workspace-default-held")
                        .debug_selector(|| "workspace-default-held".to_owned())
                        .typeface(theme.types.control, &theme)
                        .font_weight(FontWeight::SEMIBOLD)
                        .ink(colors.ink)
                        .child(text::admin::notifications::choice(DefaultChoice::of(held))),
                )
            };
            dialog = dialog.child(
                crate::ui::notice_slot("workspace-default-status-slot")
                    .when(default.changing, |slot| {
                        slot.child(
                            help(text::admin::SENDING.into())
                                .debug_selector(|| "workspace-default-sending".to_owned()),
                        )
                    })
                    .when(default.may_set && held.is_none(), |slot| {
                        slot.child(
                            help(text::admin::notifications::stated(DefaultChoice::NotSet).into())
                                .id("workspace-default-not-set")
                                .debug_selector(|| "workspace-default-not-set".to_owned()),
                        )
                    }),
            );
            dialog = dialog.child(help(text::WORKSPACE_DEFAULT_HELP.into()));
        }

        dialog
            .child(
                crate::ui::notice_slot("notify-refused-slot").children(panel.refused.map(
                    |reason| {
                        div()
                            .mt(u(12.0))
                            .child(Notice::error(reason).selector("notify-refused"))
                    },
                )),
            )
            .into_any_element()
    }

    // The quiet times of the application settings.

    /// Puts the two times of the daily quiet time into their fields, as typing does.
    pub fn type_quiet_times(
        &mut self,
        from: &str,
        until: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let set = |field: &Entity<InputState>,
                   value: &str,
                   window: &mut Window,
                   cx: &mut Context<Self>| {
            let value = value.to_owned();
            field.update(cx, |field, cx| field.set_value(value, window, cx));
        };
        set(&self.quiet_from.clone(), from, window, cx);
        set(&self.quiet_until.clone(), until, window, cx);
        self.quiet_refused = false;
        cx.notify();
    }

    /// Fills the two fields and the zone with the stored daily quiet time, when the dialog
    /// opens. Without one stored quiet time the zone of the system is offered (ADR 0798).
    pub(crate) fn show_quiet_times(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let daily = match self.state.read(cx).view().quiet.daily {
            Some(Agreement::All(Some(daily))) => Some(daily),
            _ => None,
        };
        let (from, until) = match &daily {
            Some(daily) => (clock_text(daily.start_minute), clock_text(daily.end_minute)),
            None => (clock_text(22 * 60), clock_text(7 * 60)),
        };
        let system = self.state.read(cx).system_zone_name();
        self.quiet_zone = daily.map_or_else(|| system.to_owned(), |daily| daily.time_zone);
        self.quiet_zone_open = false;
        self.type_quiet_times(&from, &until, window, cx);
    }

    /// Chooses the time zone of the daily quiet time, and closes the list of zones. The
    /// zone is sent with the next change of the quiet time.
    pub fn choose_quiet_zone(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.quiet_zone = name.to_owned();
        self.quiet_zone_open = false;
        self.quiet_zone_search
            .update(cx, |field, cx| field.set_value("", window, cx));
        cx.notify();
    }

    /// Puts text into the search of the list of time zones, as typing does.
    pub fn type_quiet_zone_search(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.to_owned();
        self.quiet_zone_search
            .update(cx, |field, cx| field.set_value(text, window, cx));
        cx.notify();
    }

    /// Opens or closes the list of time zones.
    pub fn toggle_quiet_zones(&mut self, cx: &mut Context<Self>) {
        self.quiet_zone_open = !self.quiet_zone_open;
        cx.notify();
    }

    /// Stores the two typed times as the daily quiet time, in the chosen zone.
    pub fn submit_quiet_times(&mut self, cx: &mut Context<Self>) {
        let from = parse_clock(&self.quiet_from.read(cx).value());
        let until = parse_clock(&self.quiet_until.read(cx).value());
        let zone = self.quiet_zone.clone();
        match (from, until) {
            (Some(from), Some(until)) if from != until => {
                self.quiet_refused = false;
                self.state.update(cx, |state, cx| {
                    state.set_daily_quiet(Some((from, until)), zone, cx);
                });
            }
            _ => self.quiet_refused = true,
        }
        cx.notify();
    }

    /// The time zone of the daily quiet time: its name, what holds when it is not the zone
    /// of the system or not a zone of this build, and the list to choose another one
    /// (ADR 0798).
    fn render_quiet_zone(
        &mut self,
        busy: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let help = |sentence: String| {
            div()
                .typeface(theme.types.small, theme)
                .ink(colors.ink_2)
                .child(sentence)
        };
        let zone = self.quiet_zone.clone();
        let system = self.state.read(cx).system_zone_name().to_owned();
        let mut rows = vec![
            div()
                .flex()
                .items_center()
                .gap(u(8.0))
                .mt(u(8.0))
                .child(
                    div()
                        .id("quiet-zone")
                        .debug_selector(|| "quiet-zone".to_owned())
                        .flex_1()
                        .min_w_0()
                        .typeface(theme.types.control, theme)
                        .ink(colors.ink)
                        .child(text::quiet_zone(&crate::zones::shown(&zone))),
                )
                .child(
                    Button::new("quiet-zone-change")
                        .label(if self.quiet_zone_open {
                            text::CLOSE
                        } else {
                            text::QUIET_ZONE_CHANGE
                        })
                        .disabled(busy)
                        .selector("quiet-zone-change")
                        .h(u(28.0))
                        .px(u(10.0))
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_quiet_zones(cx))),
                )
                .into_any_element(),
        ];
        if !crate::zones::is_known(&zone) {
            rows.push(
                help(text::QUIET_ZONE_UNKNOWN.to_owned())
                    .debug_selector(|| "quiet-zone-unknown".to_owned())
                    .into_any_element(),
            );
        } else if zone != system {
            rows.push(
                help(text::quiet_zone_not_system(&crate::zones::shown(&system)))
                    .debug_selector(|| "quiet-zone-not-system".to_owned())
                    .into_any_element(),
            );
        }
        if !self.quiet_zone_open {
            return rows;
        }
        let typed = self.quiet_zone_search.read(cx).value().to_string();
        let (names, more) = crate::zones::matching(&typed);
        let choices = names.into_iter().enumerate().map(|(index, name)| {
            let chosen = name == zone;
            div()
                .flex_none()
                .debug_selector(move || format!("quiet-zone-choice:{name}"))
                .child(
                    Button::new(("quiet-zone-choice", index))
                        .kind(if chosen {
                            ButtonKind::Primary
                        } else {
                            ButtonKind::Ghost
                        })
                        .label(crate::zones::shown(name))
                        .h(u(26.0))
                        .px(u(8.0))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.choose_quiet_zone(name, window, cx);
                        })),
                )
                .into_any_element()
        });
        rows.push(
            div()
                .flex()
                .flex_col()
                .gap(u(6.0))
                .mt(u(6.0))
                .child(
                    Field::new(&self.quiet_zone_search, text::QUIET_ZONE_SEARCH)
                        .selector("quiet-zone-search"),
                )
                .child(div().flex().flex_wrap().gap(u(4.0)).children(choices))
                .when(more, |list| {
                    list.child(help(text::QUIET_ZONE_MORE.to_owned()))
                })
                .into_any_element(),
        );
        rows
    }

    /// "Do not disturb" and the daily quiet time, in the settings of the application.
    pub(crate) fn render_quiet_settings(
        &mut self,
        quiet: &QuietView,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let help = |sentence: &'static str| {
            div()
                .typeface(theme.types.small, theme)
                .ink(colors.ink_2)
                .child(sentence)
        };
        let title = |title: &'static str| {
            div()
                .typeface(theme.types.control, theme)
                .font_weight(FontWeight::SEMIBOLD)
                .ink(colors.ink)
                .child(title)
        };
        let time_label = |label: &'static str| {
            div()
                .mb(u(3.0))
                .typeface(theme.types.small_strong, theme)
                .ink(colors.ink_2)
                .child(label)
        };
        let (Some(do_not_disturb), Some(daily)) = (&quiet.do_not_disturb, &quiet.daily) else {
            return vec![
                div()
                    .id("quiet-no-server")
                    .debug_selector(|| "quiet-no-server".to_owned())
                    .child(help(text::QUIET_NO_SERVER))
                    .into_any_element(),
            ];
        };
        let busy = quiet.busy;
        let on = *do_not_disturb == Agreement::All(true);
        let daily_on = matches!(daily, Agreement::All(Some(_)));
        let differs = *do_not_disturb == Agreement::Differs || *daily == Agreement::Differs;
        let mut rows =
            vec![
                div()
                    .flex()
                    .items_start()
                    .gap(u(10.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(u(8.0))
                                    .child(title(text::DO_NOT_DISTURB))
                                    .when(quiet.quiet_now, |line| {
                                        line.child(
                                            div()
                                                .id("quiet-now")
                                                .debug_selector(|| "quiet-now".to_owned())
                                                .typeface(theme.types.small_strong, theme)
                                                .ink(colors.terracotta_deep)
                                                .child(text::QUIET_NOW),
                                        )
                                    }),
                            )
                            .child(help(text::DO_NOT_DISTURB_HELP)),
                    )
                    .child(
                        Toggle::new(
                            "do-not-disturb",
                            text::DO_NOT_DISTURB,
                            on,
                            (text::ON, text::OFF),
                        )
                        .selector("do-not-disturb")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if busy {
                                return;
                            }
                            this.state
                                .update(cx, |state, cx| state.set_do_not_disturb(!on, cx));
                        })),
                    )
                    .into_any_element(),
                div()
                    .flex()
                    .items_start()
                    .gap(u(10.0))
                    .mt(u(10.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(title(text::DAILY_QUIET))
                            .child(help(text::DAILY_QUIET_HELP)),
                    )
                    .child(
                        Toggle::new(
                            "daily-quiet",
                            text::DAILY_QUIET,
                            daily_on,
                            (text::ON, text::OFF),
                        )
                        .selector("daily-quiet")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if busy {
                                return;
                            }
                            if daily_on {
                                let zone = this.quiet_zone.clone();
                                this.state.update(cx, |state, cx| {
                                    state.set_daily_quiet(None, zone, cx);
                                });
                            } else {
                                this.submit_quiet_times(cx);
                            }
                        })),
                    )
                    .into_any_element(),
                div()
                    .flex()
                    .items_end()
                    .gap(u(8.0))
                    .mt(u(8.0))
                    .child(div().w(u(96.0)).child(time_label(text::QUIET_FROM)).child(
                        Field::new(&self.quiet_from, text::QUIET_FROM).selector("quiet-from"),
                    ))
                    .child(div().w(u(96.0)).child(time_label(text::QUIET_UNTIL)).child(
                        Field::new(&self.quiet_until, text::QUIET_UNTIL).selector("quiet-until"),
                    ))
                    .child(
                        Button::new("set-daily-quiet")
                            .label(text::QUIET_SET)
                            .disabled(busy)
                            .selector("set-daily-quiet")
                            .h(u(32.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, _, cx| this.submit_quiet_times(cx))),
                    )
                    .into_any_element(),
            ];
        rows.extend(self.render_quiet_zone(busy, theme, cx));
        let mut notices = Vec::new();
        if self.quiet_refused {
            notices.push(
                div()
                    .mt(u(8.0))
                    .child(Notice::error(text::QUIET_TIME_REFUSED).selector("quiet-time-refused"))
                    .into_any_element(),
            );
        }
        if differs {
            notices.push(
                div()
                    .mt(u(8.0))
                    .child(
                        Notice::new(text::QUIET_DIFFERS)
                            .icon(Icon::Warning)
                            .selector("quiet-differs"),
                    )
                    .into_any_element(),
            );
        }
        for (server, reason) in &quiet.failed {
            notices.push(
                div()
                    .mt(u(8.0))
                    .child(
                        Notice::error(text::quiet_failed(server, reason)).selector("quiet-failed"),
                    )
                    .into_any_element(),
            );
        }
        rows.push(
            crate::ui::notice_slot("quiet-notice-slot")
                .children(notices)
                .into_any_element(),
        );
        rows
    }
}
