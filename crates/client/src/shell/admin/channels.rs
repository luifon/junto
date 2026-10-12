//! The page of the categories and channels of a workspace, and the overview of the
//! settings of one channel (ADR 0304, ADR 0402, ADR 0504).

use gpui::{AnyElement, Context, IntoElement, ParentElement, SharedString, Styled, Window, div};
use junto_client_core::protocol::call::UpdateCallSettings;
use junto_client_core::protocol::ids::{CategoryId, ChannelId, ServerId, WorkspaceId};
use junto_client_core::protocol::structure::{ChannelKind, UpdateChannel};

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

    fn channel_step_listener(
        server_id: ServerId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        up: bool,
        cx: &mut Context<Self>,
    ) -> impl Fn(&Click, &mut Window, &mut gpui::App) + 'static {
        cx.listener(move |this, _, _, cx| {
            this.state.update(cx, |state, cx| {
                state.move_channel(server_id, workspace_id, channel_id, up, cx);
            });
        })
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
                    Mode::CreateChannel => {
                        let (kind, category_id) = (self.admin.new_kind, self.admin.new_category);
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

    fn render_category_header(
        &mut self,
        view: &AdminView,
        page: &ChannelsPage,
        category: Option<&CategoryLine>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (server_id, workspace_id) = (view.server_id, page.workspace_id);
        let Some(category) = category else {
            return group_label(text::admin::NO_CATEGORY, theme).into_any_element();
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
        div()
            .flex()
            .flex_col()
            .flex_none()
            .mt(u(14.0))
            .child(named(
                format!("category-row-{category_id}"),
                line(theme).child(content),
            ))
            .children(refusal(refused, "category-refused"))
            .into_any_element()
    }

    fn render_channel_line(
        &mut self,
        view: &AdminView,
        page: &ChannelsPage,
        row: &ChannelLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (server_id, workspace_id) = (view.server_id, page.workspace_id);
        let channel_id = row.id;
        let number = channel_id.as_uuid().as_u128() as u64;
        let content: AnyElement = match self.admin.mode {
            Some(Mode::MoveChannel(id)) if id == channel_id => {
                let categories: Vec<(CategoryId, String)> = page
                    .categories()
                    .map(|category| (category.id, category.name.clone()))
                    .collect();
                self.render_category_choice(
                    server_id,
                    channel_id,
                    row.category_id,
                    &categories,
                    theme,
                    cx,
                )
                .into_any_element()
            }
            _ => {
                let detail: Option<SharedString> =
                    row.permanent.then(|| text::admin::PERMANENT_MARK.into());
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(u(8.0))
                    .child(icon(kind_icon(row.kind), 15.0, theme.colors.ink_2))
                    .child(line_text(row.name.clone(), detail, theme))
                    .child(row_action(
                        ("channel-up", number),
                        text::admin::UP,
                        format!("channel-up-{channel_id}"),
                        row.first.then_some(text::admin::FIRST_ALREADY),
                        Self::channel_step_listener(server_id, workspace_id, channel_id, true, cx),
                    ))
                    .child(row_action(
                        ("channel-down", number),
                        text::admin::DOWN,
                        format!("channel-down-{channel_id}"),
                        row.last.then_some(text::admin::LAST_ALREADY),
                        Self::channel_step_listener(server_id, workspace_id, channel_id, false, cx),
                    ))
                    .child(row_action(
                        ("channel-move", number),
                        text::admin::MOVE_TO,
                        format!("channel-move-{channel_id}"),
                        None,
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
        div()
            .flex()
            .flex_col()
            .flex_none()
            .child(named(
                format!("channel-line-{channel_id}"),
                line(theme).child(content),
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
        let open =
            self.admin.mode == Some(Mode::CreateChannel) && self.admin.new_category == category_id;
        if !open {
            let selector = match category_id {
                Some(category_id) => format!("channel-new-{category_id}"),
                None => "channel-new".to_owned(),
            };
            return div()
                .flex()
                .flex_none()
                .mt(u(6.0))
                .child(named(
                    selector,
                    Button::new(("channel-new", index))
                        .icon(Icon::Plus)
                        .label(text::admin::NEW_CHANNEL)
                        .h(u(28.0))
                        .px(u(10.0))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.admin.new_category = category_id;
                            this.admin.new_kind = ChannelKind::Text;
                            this.set_admin_mode(Some(Mode::CreateChannel), "", window, cx);
                        })),
                ))
                .into_any_element();
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
        div()
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
                    extra: vec![kind.into_any_element()],
                },
                theme,
                cx,
            ))
            .children(refusal(
                view.refused(AskKind::CreateChannel),
                "channel-create-refused",
            ))
            .into_any_element()
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
                &theme,
                cx,
            ));
            if group.channels.is_empty() {
                column = column.child(sentence(text::admin::NO_CHANNELS_HERE, &theme).py(u(6.0)));
            }
            for row in &group.channels {
                column = column.child(self.render_channel_line(view, page, row, &theme, cx));
            }
            column = column.child(self.render_new_channel(view, group, index, &theme, cx));
        }
        // A new category.
        column = column.child(group_label(text::admin::NEW_CATEGORY, &theme));
        column = match self.admin.mode {
            Some(Mode::CreateCategory) => column.child(self.render_form(
                Form {
                    field: FormField::Name,
                    label: text::admin::CATEGORY_NAME,
                    field_selector: "category-name",
                    send: text::admin::CREATE,
                    sending: view.sending(AskKind::CreateCategory),
                    help: None,
                    extra: Vec::new(),
                },
                &theme,
                cx,
            )),
            _ => column.child(
                div().flex().flex_none().child(
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
            ),
        };
        column
            .children(refusal(
                view.refused(AskKind::CreateCategory),
                "category-create-refused",
            ))
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
        column = column.child(match self.admin.mode {
            Some(Mode::RenameChannel) => line(&theme).child(self.render_form(
                Form {
                    field: FormField::Name,
                    label: text::admin::CHANNEL_NAME,
                    field_selector: "channel-name",
                    send: text::SAVE,
                    sending: view.sending(AskKind::UpdateChannel(channel_id)),
                    help: None,
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
        });
        // The category.
        let category_name = page
            .category_id
            .and_then(|id| page.categories.iter().find(|(held, _)| *held == id))
            .map_or(text::admin::NO_CATEGORY.to_owned(), |(_, name)| {
                name.clone()
            });
        column = column.child(match self.admin.mode {
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
        });
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
