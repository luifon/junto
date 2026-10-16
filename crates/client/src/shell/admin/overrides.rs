//! The page "Permissions" of the settings of a channel (ADR 0301, ADR 0304): the roles of
//! the workspace with their overrides in the channel. One role at a time shows its
//! permissions, each with the three choices "Allow", "Default" and "Deny".

use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString,
    Styled, Subscription, Window, div, prelude::FluentBuilder,
};
use junto_client_core::model::AdminPart;
use junto_client_core::protocol::ids::{ChannelId, ServerId, WorkspaceRoleId};

use super::{group_label, line, line_text, named, page_title, refusal, sentence};
use crate::assets::Icon;
use crate::shell::Shell;
use crate::state::admin::view::overrides::{Choice, PermissionLine, PermissionsPage, RoleLine};
use crate::state::admin::view::{AdminView, ChannelSettings, Listed};
use crate::state::admin::{AskKind, ChannelPage};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, Chip, StyleExt};

/// The role whose permissions the page shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expanded {
    /// The person chose no role yet: the first role, which is the "everyone" role.
    First,
    Role(WorkspaceRoleId),
    Nothing,
}

/// What the page of the overrides holds of the window itself.
pub(crate) struct Ui {
    expanded: Expanded,
}

impl Ui {
    pub(crate) fn new(_window: &mut Window, _cx: &mut Context<Shell>) -> (Self, Vec<Subscription>) {
        let ui = Self {
            expanded: Expanded::First,
        };
        (ui, Vec::new())
    }
}

/// The role in a channel that a control of the page changes.
#[derive(Clone, Copy)]
struct Target {
    server_id: ServerId,
    channel_id: ChannelId,
    role_id: WorkspaceRoleId,
    /// A change of the override of the role is on its way.
    sending: bool,
}

impl Shell {
    pub(crate) fn reset_overrides_ui(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.admin.overrides.expanded = Expanded::First;
    }

    /// One of the three choices of a permission. The choice of now has the pressed look
    /// and takes no click.
    fn render_override_choice(
        &mut self,
        target: Target,
        permission: &PermissionLine,
        index: usize,
        choice: Choice,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let Target {
            server_id,
            channel_id,
            role_id,
            sending,
        } = target;
        let on = permission.current == choice;
        let (id, word) = match choice {
            Choice::Allow => ("override-allow", "allow"),
            Choice::Default => ("override-default", "default"),
            Choice::Deny => ("override-deny", "deny"),
        };
        let held: Option<SharedString> = permission
            .refused(choice)
            .map(|reason| text::admin::overrides::refused(reason, permission.permission).into())
            .or_else(|| sending.then(|| text::admin::SENDING.into()));
        let pressed = theme.colors.limestone_lo;
        let permission = permission.permission;
        let mut button = Button::new((id, index))
            .label(text::admin::overrides::choice(choice))
            .accessible_label(text::admin::overrides::choice_label(permission, choice, on))
            .h(u(28.0))
            .px(u(10.0))
            .disabled(held.is_some())
            .when(on, |button| button.bg(Hsla::from(pressed)).shadow_none());
        if let Some(reason) = held {
            button = button.tooltip(reason);
        }
        if !on {
            button = button.on_click(cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.set_override_choice(
                        server_id, channel_id, role_id, permission, choice, cx,
                    );
                });
            }));
        }
        named(format!("override-{}-{word}", permission.name()), button)
    }

    /// One permission of the shown role: its name, what it allows, and the three choices.
    /// A choice that the grant rule refuses is disabled, and the row states the reason.
    fn render_override_permission(
        &mut self,
        target: Target,
        permission: &PermissionLine,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let reason = permission
            .reason()
            .map(|reason| text::admin::overrides::refused(reason, permission.permission));
        let mut choices = div().flex().flex_none().gap(u(4.0)).mt(u(1.0));
        for choice in Choice::ALL {
            choices = choices
                .child(self.render_override_choice(target, permission, index, choice, theme, cx));
        }
        div()
            .flex()
            .flex_none()
            .items_start()
            .gap(u(10.0))
            .py(u(6.0))
            .border_b_1()
            .border_color(Hsla::from(colors.rule_soft))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .typeface(theme.types.control_strong, theme)
                            .ink(colors.ink)
                            .child(text::admin::overrides::name(permission.permission)),
                    )
                    .child(
                        div()
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_2)
                            .child(text::admin::overrides::help(permission.permission)),
                    )
                    .children(reason.map(|reason| {
                        sentence(reason, theme)
                            .mt(u(3.0))
                            .ink(colors.ink)
                            .debug_selector(|| "override-ceiling".to_owned())
                    })),
            )
            .child(choices)
            .into_any_element()
    }

    /// One role: its name, what its override holds, and its actions. The shown role has
    /// its permissions under the row.
    fn render_override_role(
        &mut self,
        view: &AdminView,
        channel_id: ChannelId,
        role: &RoleLine,
        shown: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (server_id, role_id) = (view.server_id, role.id);
        let what = AskKind::SetOverride(channel_id, role_id);
        let sending = view.sending(what);
        let target = Target {
            server_id,
            channel_id,
            role_id,
            sending,
        };
        let number = role_id.as_uuid().as_u128() as u64;
        let summary = text::admin::overrides::summary(role.allowed(), role.denied());
        let mut row = line(theme).child(line_text(role.name.clone(), Some(summary.into()), theme));
        if role.is_everyone {
            row = row.child(Chip::new(text::admin::overrides::EVERY_MEMBER));
        }
        // The removal of an override removes its denies: the grant rule decides it.
        let blocked = role.blocks_removal();
        let cannot_remove = (role.has_override && !blocked.is_empty())
            .then(|| text::admin::overrides::removal_refused(&blocked));
        if role.has_override {
            let held: Option<SharedString> = cannot_remove
                .clone()
                .map(SharedString::from)
                .or_else(|| sending.then(|| text::admin::SENDING.into()));
            let button = Button::new(("override-clear", number))
                .label(text::admin::overrides::REMOVE)
                .h(u(28.0))
                .px(u(10.0))
                .disabled(held.is_some())
                .tooltip(held.unwrap_or_else(|| text::admin::overrides::REMOVE_HELP.into()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.clear_override(server_id, channel_id, role_id, cx);
                    });
                }));
            row = row.child(named(format!("override-clear-{role_id}"), button));
        }
        let toggle = Button::new(("override-role", number))
            .icon(if shown {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            })
            .label(if shown {
                text::admin::overrides::HIDE
            } else {
                text::admin::overrides::SHOW
            })
            .accessible_label(if shown {
                text::admin::overrides::hide_role(&role.name)
            } else {
                text::admin::overrides::show_role(&role.name)
            })
            .h(u(28.0))
            .px(u(10.0))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.admin.overrides.expanded = if shown {
                    Expanded::Nothing
                } else {
                    Expanded::Role(role_id)
                };
                cx.notify();
            }));
        row = row.child(named(format!("override-role-{role_id}"), toggle));

        let mut block = div()
            .id(SharedString::from(format!("override-result-{role_id}")))
            .flex()
            .flex_col()
            .flex_none()
            .child(row);
        if let Some(reason) = cannot_remove {
            block = block.child(sentence(reason, theme).mt(u(4.0)).ink(theme.colors.ink));
        }
        if shown {
            let mut permissions = div().flex().flex_col().flex_none().pl(u(14.0));
            for (index, permission) in role.permissions.iter().enumerate() {
                permissions = permissions
                    .child(self.render_override_permission(target, permission, index, theme, cx));
            }
            block = block.child(permissions);
        }
        block
            .children(refusal(view.refused(what), "override-refused"))
            .into_any_element()
    }

    pub(crate) fn render_permissions_page(
        &mut self,
        view: &AdminView,
        settings: &ChannelSettings,
        page: &PermissionsPage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let channel_id = settings.channel_id;
        let mut column = div()
            .flex()
            .flex_col()
            .child(page_title(
                text::admin::channel_page_name(ChannelPage::Permissions),
                text::admin::overrides::HELP,
                &theme,
            ))
            .child(sentence(text::admin::overrides::MEMBERS, &theme))
            .child(sentence(text::admin::overrides::ORDER, &theme).mt(u(3.0)));
        if settings.archived {
            // The overrides of an archived channel can be changed (ADR 0410).
            column = column.child(sentence(text::admin::overrides::ARCHIVED, &theme).mt(u(3.0)));
        }
        let part = AdminPart::Overrides(channel_id);
        let roles = match &page.roles {
            Listed::Loaded(roles) => roles,
            Listed::Loading => {
                return column
                    .child(self.render_reading(view.server_id, part, None, &theme, cx))
                    .into_any_element();
            }
            Listed::Failed(failure) => {
                return column
                    .child(self.render_reading(view.server_id, part, Some(*failure), &theme, cx))
                    .into_any_element();
            }
        };
        let expanded = match self.admin.overrides.expanded {
            Expanded::First => roles.first().map(|role| role.id),
            Expanded::Role(role_id) => Some(role_id),
            Expanded::Nothing => None,
        };
        column = column.child(group_label(text::admin::overrides::ROLES, &theme));
        for role in roles {
            let shown = expanded == Some(role.id);
            column =
                column.child(self.render_override_role(view, channel_id, role, shown, &theme, cx));
        }
        column.into_any_element()
    }
}
