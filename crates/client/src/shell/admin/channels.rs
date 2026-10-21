//! The page of the categories and channels of a workspace, and the overview of the
//! settings of one channel (ADR 0304, ADR 0402, ADR 0504).

use gpui::{
    AnyElement, AppContext as _, Context, Hsla, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use junto_client_core::protocol::call::UpdateCallSettings;
use junto_client_core::protocol::ids::{CategoryId, ChannelId, ServerId, WorkspaceId};
use junto_client_core::protocol::structure::{ChannelKind, ChannelMove, UpdateChannel};

use super::{
    Form, FormField, Mode, Question, group_label, line, line_text, named, page_title, refusal,
    row_action, sentence,
};
use crate::assets::Icon;
use crate::shell::Shell;
use crate::shell::room::option_row;
use crate::state::admin::view::{
    AdminView, CategoryLine, ChannelGroup, ChannelLine, ChannelSettings, ChannelsPage,
    OverviewPage, Privacy, Scope, ServerPage,
};
use crate::state::admin::{AdminPage, AskKind, AskState, ChannelPage};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, Notice, Segmented, Stepper, StyleExt, Toggle, icon};

/// The largest user limit of a voice channel (ADR 0514).
const USER_LIMIT_MAX: u32 = 99;

const KINDS: [ChannelKind; 2] = [ChannelKind::Text, ChannelKind::Voice];

/// The user limit after one step of its control. Below 1 the limit is off; `None` when
/// the step leads nowhere.
fn stepped_limit(limit: Option<u32>, up: bool) -> Option<Option<u32>> {
    match (limit, up) {
        (None, true) => Some(Some(2)),
        (None, false) => None,
        (Some(limit), true) => (limit < USER_LIMIT_MAX).then_some(Some(limit + 1)),
        (Some(1), false) => Some(None),
        (Some(limit), false) => Some(Some(limit - 1)),
    }
}

const fn kind_icon(kind: ChannelKind) -> Icon {
    match kind {
        ChannelKind::Text => Icon::Hash,
        ChannelKind::Voice => Icon::Speaker,
    }
}

type Click = gpui::ClickEvent;

/// A channel that the person drags to another place or category (ADR 0761).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelDrag {
    pub server_id: ServerId,
    pub channel_id: ChannelId,
}

/// A place that takes a dropped channel: the place `to` of the channels of a category,
/// or of the channels without a category.
fn drop_target(
    element: gpui::Stateful<gpui::Div>,
    page: &ChannelsPage,
    server_id: ServerId,
    category_id: Option<CategoryId>,
    to: usize,
    theme: &Theme,
    cx: &mut Context<Shell>,
) -> gpui::Stateful<gpui::Div> {
    let workspace_id = page.workspace_id;
    let (accepts, hovers) = (page.clone(), page.clone());
    let hover = theme.colors.limestone_hover;
    element
        .can_drop(move |value, _, _| {
            value.downcast_ref::<ChannelDrag>().is_some_and(|drag| {
                drag.server_id == server_id && accepts.takes(drag.channel_id, category_id, to)
            })
        })
        .drag_over::<ChannelDrag>(move |style, drag, _, _| {
            if drag.server_id == server_id && hovers.takes(drag.channel_id, category_id, to) {
                style.bg(Hsla::from(hover))
            } else {
                style
            }
        })
        .on_drop({
            // The drop sends the moves of the page that decided that it takes the drop.
            let plans = page.clone();
            cx.listener(move |this, drag: &ChannelDrag, _, cx| {
                if drag.server_id != server_id {
                    return;
                }
                let channel_id = drag.channel_id;
                if let Ok(moves) = plans.plan(channel_id, category_id, to) {
                    this.state.update(cx, |state, cx| {
                        state.move_channels(server_id, workspace_id, channel_id, moves, cx);
                    });
                }
            })
        })
}

impl Shell {
    fn category_listener(
        server_id: ServerId,
        channel_id: ChannelId,
        category_id: Option<CategoryId>,
        cx: &mut Context<Self>,
    ) -> impl Fn(&Click, &mut Window, &mut gpui::App) + 'static {
        cx.listener(move |this, _, _, cx| {
            let change = UpdateChannel {
                category_id: Some(category_id),
                ..UpdateChannel::default()
            };
            this.admin_sent(AskKind::UpdateChannel(channel_id));
            this.state.update(cx, |state, cx| {
                state.update_channel(server_id, channel_id, change, cx);
            });
        })
    }

    fn category_step_listener(
        server_id: ServerId,
        workspace_id: WorkspaceId,
        category_id: CategoryId,
        up: bool,
        cx: &mut Context<Self>,
    ) -> impl Fn(&Click, &mut Window, &mut gpui::App) + 'static {
        cx.listener(move |this, _, _, cx| {
            this.state.update(cx, |state, cx| {
                state.move_category(server_id, workspace_id, category_id, up, cx);
            });
        })
    }

    /// Sends moves that the page computed. `form` states that a form of the row sent them,
    /// so that the row puts its form away when the server did them.
    fn moves_listener(
        server_id: ServerId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        moves: Vec<ChannelMove>,
        form: bool,
        cx: &mut Context<Self>,
    ) -> impl Fn(&Click, &mut Window, &mut gpui::App) + 'static {
        cx.listener(move |this, _, _, cx| {
            if form {
                this.admin_sent(AskKind::MoveChannels(channel_id));
            }
            let moves = moves.clone();
            this.state.update(cx, |state, cx| {
                state.move_channels(server_id, workspace_id, channel_id, moves, cx);
            });
        })
    }

    /// The first step of "Move to" on the page of the channels: the group that the channel
    /// goes to.
    fn render_group_choice(
        &mut self,
        page: &ChannelsPage,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut row = div()
            .flex()
            .flex_1()
            .flex_wrap()
            .min_w_0()
            .items_center()
            .gap(u(6.0))
            .child(sentence(text::admin::CHANNEL_GROUP_HELP, theme));
        for (index, group) in page.groups.iter().enumerate() {
            let category_id = group.category.as_ref().map(|category| category.id);
            let (label, selector) = match &group.category {
                Some(category) => (
                    category.name.clone(),
                    format!("channel-group-{}", category.id),
                ),
                None => (
                    text::admin::NO_CATEGORY.to_owned(),
                    "channel-group-none".to_owned(),
                ),
            };
            row = row.child(named(
                selector,
                Button::new(("channel-group", index))
                    .label(label)
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.admin.move_group = Some(category_id);
                        cx.notify();
                    })),
            ));
        }
        row.child(
            Button::new("channel-move-cancel")
                .label(text::CANCEL)
                .selector("channel-move-cancel")
                .h(u(28.0))
                .px(u(10.0))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_admin_mode(None, "", window, cx);
                })),
        )
    }

    /// The second step of "Move to": every place of the chosen group. A place is enabled
    /// when a drop of the channel there would be taken, so the keyboard reaches every
    /// destination of a drag (ADR 0703).
    fn render_place_choice(
        &mut self,
        view: &AdminView,
        page: &ChannelsPage,
        channel_id: ChannelId,
        category_id: Option<CategoryId>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let (server_id, workspace_id) = (view.server_id, page.workspace_id);
        let sending = view.sending(AskKind::MoveChannels(channel_id));
        let others: Vec<&ChannelLine> = page
            .group(category_id)
            .map(|group| {
                group
                    .channels
                    .iter()
                    .filter(|line| line.id != channel_id)
                    .collect()
            })
            .unwrap_or_default();
        let mut row = div()
            .flex()
            .flex_1()
            .flex_wrap()
            .min_w_0()
            .items_center()
            .gap(u(6.0))
            .child(sentence(text::admin::CHANNEL_PLACE_HELP, theme));
        for to in 0..=others.len() {
            let label = match to.checked_sub(1).and_then(|before| others.get(before)) {
                Some(before) => text::admin::place_after(&before.name),
                None => text::admin::PLACE_FIRST.to_owned(),
            };
            let moves = page.plan(channel_id, category_id, to);
            let mut button = Button::new(("channel-place", to))
                .label(label)
                .disabled(moves.is_err() || sending)
                .h(u(28.0))
                .px(u(10.0));
            match moves {
                Ok(moves) => {
                    button = button.on_click(Self::moves_listener(
                        server_id,
                        workspace_id,
                        channel_id,
                        moves,
                        true,
                        cx,
                    ));
                }
                // A place that is not offered states why.
                Err(reason) => button = button.tooltip(text::admin::unplanned(reason)),
            }
            row = row.child(named(format!("channel-place-{to}"), button));
        }
        row.child(
            Button::new("channel-place-back")
                .label(text::admin::OTHER_CATEGORY)
                .selector("channel-place-back")
                .h(u(28.0))
                .px(u(10.0))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.admin.move_group = None;
                    cx.notify();
                })),
        )
        .child(
            Button::new("channel-move-cancel")
                .label(text::CANCEL)
                .selector("channel-move-cancel")
                .h(u(28.0))
                .px(u(10.0))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_admin_mode(None, "", window, cx);
                })),
        )
    }

    fn user_limit_listener(
        server_id: ServerId,
        channel_id: ChannelId,
        next: Option<u32>,
        cx: &mut Context<Self>,
    ) -> impl Fn(&Click, &mut Window, &mut gpui::App) + 'static {
        cx.listener(move |this, _, _, cx| {
            let change = UpdateCallSettings {
                user_limit: Some(next),
                hides_occupants: None,
            };
            this.state.update(cx, |state, cx| {
                state.set_call_settings(server_id, channel_id, change, cx);
            });
        })
    }

    /// The channels page of the open administration, when that is the page.
    fn channels_page(view: &AdminView) -> Option<&ChannelsPage> {
        match &view.scope {
            Scope::Server {
                page: ServerPage::Channels(page),
                ..
            } => Some(page),
            _ => None,
        }
    }

    /// Sends the form of the open row of the channels page or of the overview of a
    /// channel.
    pub(crate) fn submit_channel_form(
        &mut self,
        view: &AdminView,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let server_id = view.server_id;
        let name = self.admin_name(cx);
        if name.is_empty() {
            self.admin.invalid = Some(text::admin::NAME_EMPTY);
            cx.notify();
            return;
        }
        match (self.admin.mode, &view.scope) {
            (Some(Mode::RenameChannel), Scope::Channel(settings)) => {
                let channel_id = settings.channel_id;
                let change = UpdateChannel {
                    name: Some(name),
                    ..UpdateChannel::default()
                };
                self.admin_sent(AskKind::UpdateChannel(channel_id));
                self.state.update(cx, |state, cx| {
                    state.update_channel(server_id, channel_id, change, cx);
                });
            }
            (Some(mode), _) => {
                let Some(page) = Self::channels_page(view) else {
                    return;
                };
                let workspace_id = page.workspace_id;
                match mode {
                    Mode::CreateCategory => {
                        self.admin_sent(AskKind::CreateCategory);
                        self.state.update(cx, |state, cx| {
                            state.create_category(server_id, workspace_id, name, cx);
                        });
                    }
                    Mode::RenameCategory(category_id) => {
                        self.admin_sent(AskKind::UpdateCategory(category_id));
                        self.state.update(cx, |state, cx| {
                            state.rename_category(server_id, workspace_id, category_id, name, cx);
                        });
                    }
                    Mode::CreateChannel(category_id) => {
                        let kind = self.admin.new_kind;
                        self.admin_sent(AskKind::CreateChannel);
                        self.state.update(cx, |state, cx| {
                            state.create_channel(
                                server_id,
                                workspace_id,
                                name,
                                kind,
                                category_id,
                                cx,
                            );
                        });
                    }
                    _ => {}
                }
            }
            (None, _) => {}
        }
        cx.notify();
    }

    /// The choice of a category for a channel, in the row of the channel.
    fn render_category_choice(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        held: Option<CategoryId>,
        categories: &[(CategoryId, String)],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut row = div()
            .flex()
            .flex_1()
            .flex_wrap()
            .min_w_0()
            .items_center()
            .gap(u(6.0))
            .child(sentence(text::admin::MOVE_TO_HELP, theme));
        row = row.child(named(
            "category-choice-none".to_owned(),
            Button::new("category-choice-none")
                .label(text::admin::NO_CATEGORY)
                .disabled(held.is_none())
                .h(u(28.0))
                .px(u(10.0))
                .on_click(Self::category_listener(server_id, channel_id, None, cx)),
        ));
        for (index, (category_id, name)) in categories.iter().enumerate() {
            row = row.child(named(
                format!("category-choice-{category_id}"),
                Button::new(("category-choice", index))
                    .label(name.clone())
                    .disabled(held == Some(*category_id))
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(Self::category_listener(
                        server_id,
                        channel_id,
                        Some(*category_id),
                        cx,
                    )),
            ));
        }
        row.child(
            Button::new("category-choice-cancel")
                .label(text::CANCEL)
                .selector("category-choice-cancel")
                .h(u(28.0))
                .px(u(10.0))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.set_admin_mode(None, "", window, cx);
                })),
        )
    }

    /// The heading of a group. A channel that is dropped on it goes to the end of the
    /// group, whose channels are `count`.
    fn render_category_header(
        &mut self,
        view: &AdminView,
        page: &ChannelsPage,
        category: Option<&CategoryLine>,
        count: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (server_id, workspace_id) = (view.server_id, page.workspace_id);
        let Some(category) = category else {
            return drop_target(
                div()
                    .id("category-result-none")
                    .flex()
                    .flex_col()
                    .flex_none()
                    .debug_selector(|| "category-row-none".to_owned())
                    .child(group_label(text::admin::NO_CATEGORY, theme)),
                page,
                server_id,
                None,
                count,
                theme,
                cx,
            )
            .into_any_element();
        };
        let category_id = category.id;
        let name = category.name.clone();
        let content: AnyElement = match self.admin.mode {
            Some(Mode::RenameCategory(id)) if id == category_id => self
                .render_form(
                    Form {
                        field: FormField::Name,
                        label: text::admin::CATEGORY_NAME,
                        field_selector: "category-name",
                        send: text::SAVE,
                        sending: view.sending(AskKind::UpdateCategory(category_id)),
                        help: None,
                        held: None,
                        extra: Vec::new(),
                    },
                    theme,
                    cx,
                )
                .into_any_element(),
            Some(Mode::DeleteCategory(id)) if id == category_id => self
                .render_row_question(
                    Question {
                        sentence: text::admin::delete_category_question(&category.name).into(),
                        confirm: text::admin::DELETE,
                        confirm_selector: "category-delete-confirm",
                        cancel_selector: "category-delete-cancel",
                        sending: view.sending(AskKind::DeleteCategory(category_id)),
                    },
                    theme,
                    cx,
                    move |this, _, cx| {
                        this.admin_sent(AskKind::DeleteCategory(category_id));
                        this.state.update(cx, |state, cx| {
                            state.delete_category(server_id, workspace_id, category_id, cx);
                        });
                    },
                )
                .into_any_element(),
            _ => {
                let number = category_id.as_uuid().as_u128() as u64;
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(u(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .typeface(theme.types.label, theme)
                            .ink(theme.colors.bronze)
                            .truncate()
                            .child(category.name.clone()),
                    )
                    .child(row_action(
                        ("category-up", number),
                        text::admin::UP,
                        format!("category-up-{category_id}"),
                        category.first.then_some(text::admin::FIRST_ALREADY),
                        Self::category_step_listener(
                            server_id,
                            workspace_id,
                            category_id,
                            true,
                            cx,
                        ),
                    ))
                    .child(row_action(
                        ("category-down", number),
                        text::admin::DOWN,
                        format!("category-down-{category_id}"),
                        category.last.then_some(text::admin::LAST_ALREADY),
                        Self::category_step_listener(
                            server_id,
                            workspace_id,
                            category_id,
                            false,
                            cx,
                        ),
                    ))
                    .child(row_action(
                        ("category-rename", number),
                        text::admin::RENAME,
                        format!("category-rename-{category_id}"),
                        None,
                        cx.listener(move |this, _, window, cx| {
                            this.set_admin_mode(
                                Some(Mode::RenameCategory(category_id)),
                                &name,
                                window,
                                cx,
                            );
                        }),
                    ))
                    .child(row_action(
                        ("category-delete", number),
                        text::admin::DELETE,
                        format!("category-delete-{category_id}"),
                        None,
                        cx.listener(move |this, _, window, cx| {
                            this.set_admin_mode(
                                Some(Mode::DeleteCategory(category_id)),
                                "",
                                window,
                                cx,
                            );
                        }),
                    ))
                    .into_any_element()
            }
        };
        let refused = view
            .refused(AskKind::UpdateCategory(category_id))
            .or(view.refused(AskKind::DeleteCategory(category_id)));
        let block = div()
            .id(SharedString::from(format!("category-result-{category_id}")))
            .flex()
            .flex_col()
            .flex_none()
            .mt(u(14.0));
        drop_target(block, page, server_id, Some(category_id), count, theme, cx)
            .child(self.mode_row(
                &[
                    Mode::RenameCategory(category_id),
                    Mode::DeleteCategory(category_id),
                ],
                named(
                    format!("category-row-{category_id}"),
                    line(theme).child(content),
                ),
            ))
            .children(refusal(refused, "category-refused"))
            .into_any_element()
    }

    /// The row of a channel at the place `index` of its group. A channel that is dropped
    /// on it takes that place.
    fn render_channel_line(
        &mut self,
        view: &AdminView,
        page: &ChannelsPage,
        row: &ChannelLine,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (server_id, workspace_id) = (view.server_id, page.workspace_id);
        let channel_id = row.id;
        let number = channel_id.as_uuid().as_u128() as u64;
        let refused = page
            .refusal
            .as_ref()
            .filter(|(moved, _)| *moved == channel_id)
            .map(|(_, reason)| reason.clone());
        let content: AnyElement = match self.admin.mode {
            Some(Mode::MoveChannel(id)) if id == channel_id => match self.admin.move_group {
                None => self.render_group_choice(page, theme, cx).into_any_element(),
                Some(category_id) => self
                    .render_place_choice(view, page, channel_id, category_id, theme, cx)
                    .into_any_element(),
            },
            // A refused move is stated in the row of the moved channel, where the page draws
            // it, in place of the actions of the row: the row keeps its height, and no row
            // moves at the refusal (ADR 0730). The form of "Move to" stays until its answer
            // closes it, and its row then keeps the height of the form.
            _ if refused.is_some() => {
                let reason: SharedString = refused.unwrap_or_default().into();
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(u(8.0))
                    .child(icon(kind_icon(row.kind), 15.0, theme.colors.ink_2))
                    .child(
                        div()
                            .flex_none()
                            .typeface(theme.types.control, theme)
                            .ink(theme.colors.ink)
                            .child(row.name.clone()),
                    )
                    .child(
                        div()
                            .id("channel-move-refused")
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .typeface(theme.types.small, theme)
                            .ink(theme.colors.cinnabar_ink)
                            .tooltip(crate::ui::tooltip(reason.clone()))
                            .debug_selector(|| "channel-move-refused".to_owned())
                            .child(reason),
                    )
                    .child(
                        Button::new("channel-move-refused-dismiss")
                            .label(text::DISMISS)
                            .selector("channel-move-refused-dismiss")
                            .h(u(28.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_admin_mode(None, "", window, cx);
                            })),
                    )
                    .into_any_element()
            }
            _ => {
                let detail: Option<SharedString> =
                    row.permanent.then(|| text::admin::PERMANENT_MARK.into());
                // The handle is the place where a drag starts. A channel that the person
                // does not manage has none.
                let label: SharedString = row.name.clone().into();
                let handle = div()
                    .id(("channel-handle", number))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(u(22.0))
                    .when(row.may_manage, |handle| {
                        handle
                            .cursor_grab()
                            .tooltip(crate::ui::tooltip(text::admin::CHANNEL_HANDLE))
                            .on_drag(
                                ChannelDrag {
                                    server_id,
                                    channel_id,
                                },
                                move |_, _, _, cx| {
                                    cx.new(|_| super::roles::DragLabel(label.clone()))
                                },
                            )
                            .child(icon(Icon::List, 14.0, theme.colors.ink_2))
                    })
                    .debug_selector(move || format!("channel-handle-{channel_id}"));
                // Up and Down send the moves of the place before and after, as the page
                // shows them now.
                let step = |up: bool| {
                    let to = if up {
                        index.checked_sub(1)
                    } else {
                        Some(index + 1)
                    };
                    to.and_then(|to| page.plan(channel_id, row.category_id, to).ok())
                        .unwrap_or_default()
                };
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(u(8.0))
                    .child(handle)
                    .child(icon(kind_icon(row.kind), 15.0, theme.colors.ink_2))
                    .child(line_text(row.name.clone(), detail, theme))
                    .child(row_action(
                        ("channel-up", number),
                        text::admin::UP,
                        format!("channel-up-{channel_id}"),
                        row.first
                            .then_some(text::admin::FIRST_ALREADY)
                            .or(row.moves_up.err().map(text::admin::unplanned)),
                        Self::moves_listener(
                            server_id,
                            workspace_id,
                            channel_id,
                            step(true),
                            false,
                            cx,
                        ),
                    ))
                    .child(row_action(
                        ("channel-down", number),
                        text::admin::DOWN,
                        format!("channel-down-{channel_id}"),
                        row.last
                            .then_some(text::admin::LAST_ALREADY)
                            .or(row.moves_down.err().map(text::admin::unplanned)),
                        Self::moves_listener(
                            server_id,
                            workspace_id,
                            channel_id,
                            step(false),
                            false,
                            cx,
                        ),
                    ))
                    .child(row_action(
                        ("channel-move", number),
                        text::admin::MOVE_TO,
                        format!("channel-move-{channel_id}"),
                        (!row.may_manage).then_some(text::admin::CHANNEL_MANAGE_NEEDS),
                        cx.listener(move |this, _, window, cx| {
                            this.set_admin_mode(
                                Some(Mode::MoveChannel(channel_id)),
                                "",
                                window,
                                cx,
                            );
                        }),
                    ))
                    .child(row_action(
                        ("channel-settings", number),
                        text::admin::SETTINGS,
                        format!("channel-settings-{channel_id}"),
                        None,
                        cx.listener(move |this, _, window, cx| {
                            this.open_channel_settings(
                                workspace_id,
                                channel_id,
                                ChannelPage::Overview,
                                window,
                                cx,
                            );
                        }),
                    ))
                    .into_any_element()
            }
        };
        let block = div()
            .id(SharedString::from(format!("channel-result-{channel_id}")))
            .flex()
            .flex_col()
            .flex_none();
        drop_target(block, page, server_id, row.category_id, index, theme, cx)
            .child(self.mode_row(
                &[Mode::MoveChannel(channel_id)],
                named(
                    format!("channel-line-{channel_id}"),
                    line(theme).child(content),
                ),
            ))
            .children(refusal(
                view.refused(AskKind::UpdateChannel(channel_id)),
                "channel-refused",
            ))
            .into_any_element()
    }

    fn render_new_channel(
        &mut self,
        view: &AdminView,
        group: &ChannelGroup,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let category_id = group.category.as_ref().map(|category| category.id);
        let mode = Mode::CreateChannel(category_id);
        let open = self.admin.mode == Some(mode);
        if !open {
            let selector = match category_id {
                Some(category_id) => format!("channel-new-{category_id}"),
                None => "channel-new".to_owned(),
            };
            let button = div().flex().flex_none().mt(u(6.0)).child(named(
                selector,
                Button::new(("channel-new", index))
                    .icon(Icon::Plus)
                    .label(text::admin::NEW_CHANNEL)
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.admin.new_kind = ChannelKind::Text;
                        this.set_admin_mode(Some(mode), "", window, cx);
                    })),
            ));
            return self.mode_row(&[mode], button).into_any_element();
        }
        let kind = Segmented::new(
            "channel-kind",
            text::admin::KIND,
            KINDS.map(|kind| SharedString::from(text::admin::kind_name(kind))),
        )
        .selected(KINDS.iter().position(|kind| *kind == self.admin.new_kind))
        .selector("channel-kind")
        .on_select({
            let select = cx.listener(|this, index: &usize, _, cx| {
                if let Some(kind) = KINDS.get(*index).copied() {
                    this.admin.new_kind = kind;
                    cx.notify();
                }
            });
            move |index, window, cx| select(&index, window, cx)
        });
        let form = div()
            .flex()
            .flex_col()
            .flex_none()
            .mt(u(6.0))
            .child(self.render_form(
                Form {
                    field: FormField::Name,
                    label: text::admin::CHANNEL_NAME,
                    field_selector: "channel-name",
                    send: text::admin::CREATE,
                    sending: view.sending(AskKind::CreateChannel),
                    help: None,
                    held: None,
                    extra: vec![kind.into_any_element()],
                },
                theme,
                cx,
            ))
            .children(refusal(
                view.refused(AskKind::CreateChannel),
                "channel-create-refused",
            ));
        self.mode_row(&[mode], form).into_any_element()
    }

    pub(crate) fn render_channels_page(
        &mut self,
        view: &AdminView,
        page: &ChannelsPage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let mut column = div().flex().flex_col().child(page_title(
            text::admin::page_name(AdminPage::Channels),
            text::admin::CHANNELS_HELP,
            &theme,
        ));
        if !page.may_manage {
            return column
                .child(sentence(text::MANAGE_NO_PERMISSION, &theme))
                .into_any_element();
        }
        for (index, group) in page.groups.iter().enumerate() {
            column = column.child(self.render_category_header(
                view,
                page,
                group.category.as_ref(),
                group.channels.len(),
                &theme,
                cx,
            ));
            if group.channels.is_empty() {
                column = column.child(sentence(text::admin::NO_CHANNELS_HERE, &theme).py(u(6.0)));
            }
            for (at, row) in group.channels.iter().enumerate() {
                column = column.child(self.render_channel_line(view, page, row, at, &theme, cx));
            }
            column = column.child(self.render_new_channel(view, group, index, &theme, cx));
        }
        // A new category.
        column = column.child(group_label(text::admin::NEW_CATEGORY, &theme));
        let new_category = match self.admin.mode {
            Some(Mode::CreateCategory) => self.render_form(
                Form {
                    field: FormField::Name,
                    label: text::admin::CATEGORY_NAME,
                    field_selector: "category-name",
                    send: text::admin::CREATE,
                    sending: view.sending(AskKind::CreateCategory),
                    help: None,
                    held: None,
                    extra: Vec::new(),
                },
                &theme,
                cx,
            ),
            _ => div().flex().flex_none().child(
                Button::new("category-new")
                    .icon(Icon::Plus)
                    .label(text::admin::NEW_CATEGORY)
                    .selector("category-new")
                    .h(u(30.0))
                    .px(u(12.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_admin_mode(Some(Mode::CreateCategory), "", window, cx);
                    })),
            ),
        };
        column = column.child(self.mode_row(&[Mode::CreateCategory], new_category));
        // A refused move of a channel that the page no longer lists, such as one deleted
        // meanwhile, is stated at the end of the page with its Close button: the page offers
        // no move while a refused move is drawn (ADR 0761), and the end of the page grows
        // without moving a row above it (ADR 0730).
        let orphan = page
            .refusal
            .as_ref()
            .filter(|(moved, _)| page.channel(*moved).is_none())
            .map(|(_, reason)| {
                div().flex_none().mt(u(8.0)).child(
                    Notice::error(reason.clone())
                        .selector("channel-move-refused-page")
                        .action(
                            Button::new("channel-move-refused-page-dismiss")
                                .label(text::DISMISS)
                                .selector("channel-move-refused-page-dismiss")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_admin_mode(None, "", window, cx);
                                })),
                        ),
                )
            });
        column
            .children(refusal(
                view.refused(AskKind::CreateCategory),
                "category-create-refused",
            ))
            .children(orphan)
            .into_any_element()
    }

    /// The overview of the settings of one channel.
    pub(crate) fn render_channel_overview(
        &mut self,
        view: &AdminView,
        settings: &ChannelSettings,
        page: &OverviewPage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let server_id = view.server_id;
        let channel_id = settings.channel_id;
        let held = settings.archived.then_some(text::admin::ARCHIVED_NO_CHANGE);
        let mut column = div().flex().flex_col().child(page_title(
            text::admin::channel_page_name(ChannelPage::Overview),
            text::admin::OVERVIEW_HELP,
            &theme,
        ));
        if let Some(held) = held {
            column = column.child(
                div()
                    .flex_none()
                    .mb(u(8.0))
                    .child(Notice::new(held).selector("channel-archived-notice")),
            );
        }
        // The name.
        let name = page.name.clone();
        let name_line = match self.admin.mode {
            Some(Mode::RenameChannel) => line(&theme).child(self.render_form(
                Form {
                    field: FormField::Name,
                    label: text::admin::CHANNEL_NAME,
                    field_selector: "channel-name",
                    send: text::SAVE,
                    sending: view.sending(AskKind::UpdateChannel(channel_id)),
                    help: None,
                    held: None,
                    extra: Vec::new(),
                },
                &theme,
                cx,
            )),
            _ => line(&theme)
                .child(line_text(
                    text::admin::NAME_FIELD,
                    Some(page.name.clone().into()),
                    &theme,
                ))
                .child(row_action(
                    "channel-rename",
                    text::admin::RENAME,
                    "channel-rename".to_owned(),
                    held,
                    cx.listener(move |this, _, window, cx| {
                        this.set_admin_mode(Some(Mode::RenameChannel), &name, window, cx);
                    }),
                )),
        };
        column = column.child(self.mode_row(&[Mode::RenameChannel], name_line));
        // The category.
        let category_name = page
            .category_id
            .and_then(|id| page.categories.iter().find(|(held, _)| *held == id))
            .map_or(text::admin::NO_CATEGORY.to_owned(), |(_, name)| {
                name.clone()
            });
        let category_line = match self.admin.mode {
            Some(Mode::MoveChannel(id)) if id == channel_id => {
                line(&theme).child(self.render_category_choice(
                    server_id,
                    channel_id,
                    page.category_id,
                    &page.categories,
                    &theme,
                    cx,
                ))
            }
            _ => line(&theme)
                .child(line_text(
                    text::admin::CATEGORY,
                    Some(category_name.into()),
                    &theme,
                ))
                .child(row_action(
                    "channel-move",
                    text::admin::MOVE_TO,
                    "channel-move".to_owned(),
                    held,
                    cx.listener(move |this, _, window, cx| {
                        this.set_admin_mode(Some(Mode::MoveChannel(channel_id)), "", window, cx);
                    }),
                )),
        };
        column = column.child(self.mode_row(&[Mode::MoveChannel(channel_id)], category_line));
        // The rules of the channel.
        let permanent = page.permanent;
        column = column.child(
            option_row(
                text::PERMANENT_MESSAGES,
                text::admin::PERMANENT_MESSAGES_HELP,
                Toggle::new(
                    "channel-permanent",
                    text::PERMANENT_MESSAGES,
                    permanent,
                    (text::ON, text::OFF),
                )
                .held(held)
                .selector("channel-permanent")
                .on_click(cx.listener(move |this, _, _, cx| {
                    let change = UpdateChannel {
                        permanent_messages: Some(!permanent),
                        ..UpdateChannel::default()
                    };
                    this.state.update(cx, |state, cx| {
                        state.update_channel(server_id, channel_id, change, cx);
                    });
                }))
                .into_any_element(),
                &theme,
            )
            .px(u(0.0))
            .mt(u(8.0)),
        );
        let private = page.privacy == Privacy::Private;
        let privacy_held = match page.privacy {
            Privacy::Unknown => Some(text::admin::PRIVATE_UNKNOWN),
            Privacy::Open | Privacy::Private => None,
        };
        column = column.child(
            option_row(
                text::admin::PRIVATE_CHANNEL,
                text::admin::PRIVATE_CHANNEL_HELP,
                Toggle::new(
                    "channel-private",
                    text::admin::PRIVATE_CHANNEL,
                    private,
                    (text::ON, text::OFF),
                )
                .held(privacy_held)
                .selector("channel-private")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.set_private(server_id, channel_id, !private, cx);
                    });
                }))
                .into_any_element(),
                &theme,
            )
            .px(u(0.0)),
        );
        if let Some(voice) = page.voice {
            let limit = voice.user_limit;
            let hides = voice.hides_occupants;
            let lower = stepped_limit(limit, false)
                .filter(|_| held.is_none())
                .map(|next| Self::user_limit_listener(server_id, channel_id, next, cx));
            let raise = stepped_limit(limit, true)
                .filter(|_| held.is_none())
                .map(|next| Self::user_limit_listener(server_id, channel_id, next, cx));
            column = column
                .child(
                    option_row(
                        text::USER_LIMIT,
                        text::USER_LIMIT_HELP,
                        Stepper::new(
                            "channel-limit",
                            text::USER_LIMIT,
                            limit.map_or_else(
                                || text::NO_USER_LIMIT.to_owned(),
                                |limit| limit.to_string(),
                            ),
                            text::LOWER_USER_LIMIT,
                            text::RAISE_USER_LIMIT,
                        )
                        .selector("channel-limit")
                        .held(held)
                        .on_lower(lower)
                        .on_raise(raise)
                        .into_any_element(),
                        &theme,
                    )
                    .px(u(0.0)),
                )
                .child(
                    option_row(
                        text::HIDE_OCCUPANTS,
                        text::HIDE_OCCUPANTS_HELP,
                        Toggle::new(
                            "channel-hide",
                            text::HIDE_OCCUPANTS,
                            hides,
                            (text::ON, text::OFF),
                        )
                        .held(held)
                        .selector("channel-hide")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let change = UpdateCallSettings {
                                user_limit: None,
                                hides_occupants: Some(!hides),
                            };
                            this.state.update(cx, |state, cx| {
                                state.set_call_settings(server_id, channel_id, change, cx);
                            });
                        }))
                        .into_any_element(),
                        &theme,
                    )
                    .px(u(0.0)),
                )
                .child(sentence(text::admin::CALL_CONTROLS_HELP, &theme).mt(u(6.0)));
        }
        // What the server refused of this page, whichever control asked.
        let refused = view
            .ask
            .as_ref()
            .and_then(|ask| match (&ask.what, &ask.state) {
                (
                    AskKind::UpdateChannel(of)
                    | AskKind::CallSettings(of)
                    | AskKind::SetOverride(of, _),
                    AskState::Refused(reason),
                ) if *of == channel_id => Some(reason.as_str()),
                _ => None,
            });
        column
            .children(refusal(refused, "channel-settings-refused"))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::stepped_limit;

    #[test]
    fn the_user_limit_steps_between_off_and_the_largest_limit() {
        assert_eq!(stepped_limit(None, true), Some(Some(2)));
        assert_eq!(stepped_limit(None, false), None);
        assert_eq!(stepped_limit(Some(2), false), Some(Some(1)));
        assert_eq!(stepped_limit(Some(1), false), Some(None));
        assert_eq!(stepped_limit(Some(98), true), Some(Some(99)));
        assert_eq!(stepped_limit(Some(99), true), None);
    }
}
