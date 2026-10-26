//! The permissions of the operating system in the window: the section "Permissions" of the
//! settings, and the button of a permission where it is used (ADR 0708).

use gpui::{AnyElement, Context, IntoElement, ParentElement, Role, Styled, div};

use super::Shell;
use crate::assets::Icon;
use crate::permissions::{Access, Action, Permission};
use crate::text::permissions as txt;
use crate::theme::{Theme, u};
use crate::ui::{Button, ButtonKind, StyleExt};
use gpui::{InteractiveElement, StatefulInteractiveElement};

/// The name of a permission in a selector.
pub(crate) fn key(permission: Permission) -> &'static str {
    match permission {
        Permission::Microphone => "microphone",
        Permission::Camera => "camera",
        Permission::Screen => "screen",
        Permission::Notifications => "notifications",
        Permission::Downloads => "downloads",
        Permission::LocalNetwork => "local-network",
        Permission::InputMonitoring => "input-monitoring",
    }
}

/// The selector of the button of a permission in the section "Permissions".
fn action_selector(permission: Permission) -> &'static str {
    match permission {
        Permission::Microphone => "permission-action-microphone",
        Permission::Camera => "permission-action-camera",
        Permission::Screen => "permission-action-screen",
        Permission::Notifications => "permission-action-notifications",
        Permission::Downloads => "permission-action-downloads",
        Permission::LocalNetwork => "permission-action-local-network",
        Permission::InputMonitoring => "permission-action-input-monitoring",
    }
}

impl Shell {
    /// The button of the action of a permission, when it needs one: the question of the
    /// system, or the page of its settings.
    pub(crate) fn permission_button(
        &mut self,
        selector: &'static str,
        permission: Permission,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.state.read(cx);
        let action = state.permission_action(permission)?;
        let system = state.permission_system();
        Some(
            Button::new(selector)
                .kind(if action == Action::Ask {
                    ButtonKind::Primary
                } else {
                    ButtonKind::Default
                })
                .label(txt::action_label(system, action))
                .icon(if action == Action::Ask {
                    Icon::Check
                } else {
                    Icon::Gear
                })
                .selector(selector)
                .h(u(30.0))
                .px(u(12.0))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state
                        .update(cx, |state, cx| state.act_on_permission(permission, cx));
                }))
                .into_any_element(),
        )
    }

    /// The section "Permissions": each permission that this system keeps for Junto, what
    /// it is for, its state, and its action.
    pub(crate) fn render_permissions_section(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let colors = theme.colors;
        let state = self.state.read(cx);
        let system = state.permission_system();
        let rows = state.permission_rows();
        let mut parts: Vec<AnyElement> = vec![
            div()
                .mb(u(5.0))
                .typeface(theme.types.label, theme)
                .ink(colors.bronze)
                .child(txt::PERMISSIONS)
                .into_any_element(),
            div()
                .typeface(theme.types.small, theme)
                .ink(colors.ink_2)
                .child(txt::PERMISSIONS_HELP)
                .into_any_element(),
        ];
        for row in rows {
            let permission = row.permission;
            let name = txt::name(system, permission);
            let sentence = txt::state(system, permission, row.access);
            let warned = matches!(row.access, Access::Refused | Access::Restricted);
            let button = self.permission_button(action_selector(permission), permission, cx);
            parts.push(
                div()
                    .id(gpui::SharedString::from(format!("permission-{}", key(permission))))
                    .role(Role::Group)
                    .aria_label(format!("{name}: {sentence}"))
                    .debug_selector(move || format!("permission-{}", key(permission)))
                    .flex()
                    .items_start()
                    .gap(u(12.0))
                    .mt(u(12.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .typeface(theme.types.control, theme)
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .ink(colors.ink)
                                    .child(name),
                            )
                            .child(
                                div()
                                    .typeface(theme.types.small, theme)
                                    .ink(colors.ink_2)
                                    .child(txt::purpose(permission)),
                            )
                            .child(
                                div()
                                    .mt(u(3.0))
                                    .typeface(theme.types.small, theme)
                                    .ink(if warned {
                                        colors.terracotta_deep
                                    } else {
                                        colors.ink_2
                                    })
                                    .debug_selector(move || {
                                        format!("permission-state-{}", key(permission))
                                    })
                                    .child(sentence),
                            ),
                    )
                    .children(button)
                    .into_any_element(),
            );
        }
        parts.push(
            div()
                .flex()
                .mt(u(14.0))
                .child(
                    Button::new("check-permissions")
                        .label(txt::CHECK_AGAIN)
                        .icon(Icon::Retry)
                        .selector("check-permissions")
                        .h(u(30.0))
                        .px(u(12.0))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state.update(cx, |state, cx| state.read_permissions(cx));
                        })),
                )
                .into_any_element(),
        );
        parts
    }

    /// The notification permission in the section "Notifications", while the system does
    /// not allow it or has not asked.
    pub(crate) fn render_notification_permission(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.state.read(cx);
        let system = state.permission_system();
        if !system.permissions().contains(&Permission::Notifications) {
            return None;
        }
        let access = state.permission_access(Permission::Notifications);
        if !matches!(access, Access::Refused | Access::NotAsked) {
            return None;
        }
        let sentence = txt::state(system, Permission::Notifications, access);
        let button = self.permission_button(
            "notifications-permission-action",
            Permission::Notifications,
            cx,
        );
        Some(
            div()
                .flex()
                .items_start()
                .gap(u(12.0))
                .mt(u(10.0))
                .debug_selector(|| "notification-permission".to_owned())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .typeface(theme.types.small, theme)
                        .ink(theme.colors.terracotta_deep)
                        .child(sentence),
                )
                .children(button)
                .into_any_element(),
        )
    }
}
