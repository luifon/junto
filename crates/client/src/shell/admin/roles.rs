//! The pages of the roles (ADR 0301, ADR 0309): the roles of the server or of the shown
//! workspace in their order, with a new role, a new name, the permissions, the deletion
//! and the move of a role; and the roles of one account in the lists of people. A role
//! that the person cannot act on states why in its row.

use std::sync::Arc;

use gpui::{
    AnyElement, AppContext as _, Context, Entity, Hsla, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Subscription, Window,
    div, prelude::FluentBuilder,
};
use gpui_base::input::InputState;
use junto_client_core::model::AdminPart;
use junto_client_core::protocol::ids::{AccountId, ServerId};

use super::{line, line_text, named, page_title, refusal, sentence};
use crate::assets::Icon;
use crate::shell::Shell;
use crate::state::admin::view::roles::{
    NoDeletion, PermissionSwitch, RoleChoice, RoleRow, RolesPage,
};
use crate::state::admin::view::{AdminView, Listed, Scope, ServerPage};
use crate::state::admin::{AdminPage, AskKind, AskState, RoleRef};
use crate::text;
use crate::text::admin::roles as txt;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, ButtonKind, Chip, Field, FieldEvent, Notice, StyleExt, Toggle, icon};

/// What the page of the roles shows in place of the actions of a row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Open {
    /// The form of a new role.
    Create,
    /// The form of a new name.
    Rename(RoleRef),
    /// The question before a deletion.
    Delete(RoleRef),
}

/// What the pages of the roles hold of the window itself.
pub(crate) struct Ui {
    /// The name of a new role, or the new name of a role.
    pub(crate) name: Entity<InputState>,
    pub(crate) open: Option<Open>,
    /// The role whose permissions the page shows.
    pub(crate) expanded: Option<RoleRef>,
    /// The account whose roles a list of people shows.
    pub(crate) account: Option<AccountId>,
    /// Why the text of the field was not sent.
    pub(crate) invalid: Option<&'static str>,
    /// The request that the open form or question was sent as.
    pub(crate) sent: Option<AskKind>,
}

impl Ui {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Shell>) -> (Self, Vec<Subscription>) {
        let name = Field::state(text::admin::NAME_FIELD, window, cx);
        let subscriptions = vec![Field::observe(
            &name,
            window,
            cx,
            |this: &mut Shell, event, window, cx| match event {
                FieldEvent::Changed => {
                    this.admin.roles.invalid = None;
                    cx.notify();
                }
                FieldEvent::Submitted => this.submit_role_form(window, cx),
            },
        )];
        let ui = Self {
            name,
            open: None,
            expanded: None,
            account: None,
            invalid: None,
            sent: None,
        };
        (ui, subscriptions)
    }
}

/// A role that the person drags to another place of the order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleDrag {
    pub server_id: ServerId,
    pub role: RoleRef,
}

/// What follows the pointer while a role is dragged: the name of the role.
struct DragLabel(SharedString);

impl Render for DragLabel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = theme(cx);
        div()
            .px(u(10.0))
            .py(u(6.0))
            .rounded(u(theme.metrics.radius))
            .border_1()
            .border_color(Hsla::from(theme.colors.rule))
            .bg(Hsla::from(theme.colors.limestone))
            .typeface(theme.types.name, &theme)
            .ink(theme.colors.ink)
            .child(self.0.clone())
    }
}

/// The roles page of the open administration, when that is the page.
fn roles_page(view: &AdminView) -> Option<&RolesPage> {
    match &view.scope {
        Scope::Server {
            page: ServerPage::Roles(page),
            ..
        } => Some(page),
        _ => None,
    }
}

impl Shell {
    pub(crate) fn reset_roles_ui(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ui = &mut self.admin.roles;
        ui.open = None;
        ui.expanded = None;
        ui.account = None;
        ui.invalid = None;
        ui.sent = None;
        let field = ui.name.clone();
        field.update(cx, |field, cx| field.set_value("", window, cx));
    }

    /// Shows a form or a question of the roles page, with the field empty or holding
    /// `name`, or the actions of every row again.
    pub fn set_roles_open(
        &mut self,
        open: Option<Open>,
        name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.admin.roles.open = open;
        self.admin.roles.invalid = None;
        self.admin.roles.sent = None;
        let name = name.to_owned();
        let field = self.admin.roles.name.clone();
        field.update(cx, |field, cx| field.set_value(name, window, cx));
        self.state
            .update(cx, |state, cx| state.dismiss_admin_ask(cx));
        if matches!(open, Some(Open::Create | Open::Rename(_))) {
            self.focus_field(&field, window, cx);
        }
        cx.notify();
    }

    /// Puts text into the name field of the roles page, as typing does.
    pub fn type_role_name(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let name = name.to_owned();
        self.admin
            .roles
            .name
            .update(cx, |field, cx| field.set_value(name, window, cx));
    }

    /// Shows the permissions of one role under its row, or of none.
    pub fn show_role_permissions(&mut self, role: Option<RoleRef>, cx: &mut Context<Self>) {
        self.admin.roles.expanded = role;
        cx.notify();
    }

    /// Shows the roles of one account in a list of people, or of none.
    pub fn show_account_roles(&mut self, account: Option<AccountId>, cx: &mut Context<Self>) {
        self.admin.roles.account = account;
        self.state
            .update(cx, |state, cx| state.dismiss_admin_ask(cx));
        cx.notify();
    }

    /// Enter in the name field sends the open form.
    pub fn submit_role_form(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        // A form whose request is on its way is not sent a second time.
        if self.admin.roles.sent.is_some() {
            return;
        }
        let Some(view) = self.state.read(cx).admin_view() else {
            return;
        };
        let Some(page) = roles_page(&view) else {
            return;
        };
        let name = self.admin.roles.name.read(cx).value().trim().to_owned();
        let (server_id, level) = (view.server_id, page.level);
        match self.admin.roles.open {
            Some(Open::Create) => {
                if page.create.is_err() {
                    return;
                }
                if name.is_empty() {
                    self.admin.roles.invalid = Some(text::admin::NAME_EMPTY);
                    cx.notify();
                    return;
                }
                self.admin.roles.sent = Some(AskKind::CreateRole(level));
                self.state.update(cx, |state, cx| {
                    state.create_role(server_id, level, name, cx)
                });
            }
            Some(Open::Rename(role)) => {
                let Some(row) = page.row(role) else {
                    return;
                };
                if row.locked.is_some() || !row.named {
                    return;
                }
                if name.is_empty() {
                    self.admin.roles.invalid = Some(text::admin::NAME_EMPTY);
                    cx.notify();
                    return;
                }
                self.admin.roles.sent = Some(AskKind::UpdateRole(role));
                self.state
                    .update(cx, |state, cx| state.rename_role(server_id, role, name, cx));
            }
            Some(Open::Delete(_)) | None => {}
        }
        cx.notify();
    }

    /// The form or the question that was sent goes when the server did the request. A
    /// refusal keeps it, with the reason.
    fn settle_roles(&mut self, view: &AdminView, window: &mut Window, cx: &mut Context<Self>) {
        let Some(what) = self.admin.roles.sent else {
            return;
        };
        match view.asked(what) {
            Some(AskState::Sending) => {}
            Some(AskState::Refused(_)) => self.admin.roles.sent = None,
            None => {
                self.admin.roles.sent = None;
                self.admin.roles.open = None;
                self.admin.roles.invalid = None;
                let field = self.admin.roles.name.clone();
                field.update(cx, |field, cx| field.set_value("", window, cx));
            }
        }
    }

    /// Puts a dragged role at the place of the row that it was dropped on.
    fn drop_role(&mut self, drag: RoleDrag, to: usize, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.move_role(drag.server_id, drag.role, to, cx);
        });
    }

    /// The name field with the two buttons that send the form or put it away.
    fn render_role_form(
        &mut self,
        send: &'static str,
        sending: bool,
        help: Option<&'static str>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let field = self.admin.roles.name.clone();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(u(8.0))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Field::new(&field, txt::ROLE_NAME).selector("role-name")),
                    )
                    .child(
                        Button::new("role-form-cancel")
                            .label(text::CANCEL)
                            .selector("role-form-cancel")
                            .h(u(30.0))
                            .px(u(10.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_roles_open(None, "", window, cx);
                            })),
                    )
                    .child(
                        Button::new("role-form-send")
                            .kind(ButtonKind::Primary)
                            .label(send)
                            .selector("role-form-send")
                            .disabled(sending)
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.submit_role_form(window, cx);
                            })),
                    ),
            )
            .child(
                crate::ui::notice_slot("role-form-invalid-slot").children(
                    self.admin
                        .roles
                        .invalid
                        .map(|invalid| Notice::error(invalid).selector("role-form-invalid")),
                ),
            )
            .when_some(help, |column, help| {
                column.child(sentence(help, theme).mt(u(5.0)))
            })
    }

    /// One permission of a role: its name, what it gives, and its switch.
    fn render_role_permission(
        &mut self,
        view: &AdminView,
        row: &RoleRow,
        switch: &PermissionSwitch,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let (server_id, role) = (view.server_id, row.role);
        // A change of this permission is on its way, or the server did it and the state
        // does not hold it yet: the switch shows the asked state (ADR 0791, ADR 0311).
        let asked = view.permission_asked(role, switch.permission);
        let changing = asked.is_some();
        let (permission, on) = (switch.permission, asked.unwrap_or(switch.on));
        // A permission that the person does not hold cannot be given; taking it needs
        // the managing permission only (ADR 0304).
        let beyond = (!on && switch.beyond_own).then(|| txt::beyond_own(permission));
        let held: Option<SharedString> = row
            .locked
            .map(|reason| txt::locked(reason).into())
            .or_else(|| beyond.clone().map(SharedString::from))
            .or_else(|| changing.then(|| text::admin::SENDING.into()));
        let toggle = Toggle::new(
            ("role-permission", index),
            txt::permission_label(permission, &row.name),
            on,
            (text::ON, text::OFF),
        )
        .held(held)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.state.update(cx, |state, cx| {
                state.set_role_permission(server_id, role, permission, !on, cx);
            });
        }));
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
                            .child(txt::permission_name(permission)),
                    )
                    .child(
                        div()
                            .typeface(theme.types.small, theme)
                            .ink(colors.ink_2)
                            .child(txt::permission_help(permission)),
                    )
                    .children(beyond.map(|reason| {
                        let selector = format!("role-ceiling-{}", permission.name());
                        sentence(reason, theme)
                            .mt(u(3.0))
                            .ink(colors.ink)
                            .debug_selector(move || selector)
                    })),
            )
            .child(named(
                format!("role-permission-{}", permission.name()),
                toggle,
            ))
            .into_any_element()
    }

    /// The actions of the row of a role.
    fn render_role_actions(
        &mut self,
        view: &AdminView,
        row: &RoleRow,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let (server_id, role) = (view.server_id, row.role);
        let number = role.number();
        let written = role.written();
        let moving = view.sending(AskKind::MoveRole(role));
        let locked = row.locked.map(txt::locked);
        let mut actions = Vec::new();
        for (up, place) in [(true, row.up), (false, row.down)] {
            let held = place
                .err()
                .map(|reason| txt::fixed(reason, up))
                .or(moving.then_some(text::admin::SENDING));
            let mut button = Button::new((if up { "role-up" } else { "role-down" }, number))
                .label(if up {
                    text::admin::UP
                } else {
                    text::admin::DOWN
                })
                .accessible_label(txt::move_label(&row.name, up))
                .h(u(28.0))
                .px(u(8.0))
                .disabled(held.is_some());
            match (held, place) {
                (Some(reason), _) => button = button.tooltip(reason),
                (None, Ok(to)) => {
                    button = button
                        .tooltip(if up { txt::MOVE_UP } else { txt::MOVE_DOWN })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.state.update(cx, |state, cx| {
                                state.move_role(server_id, role, to, cx);
                            });
                        }));
                }
                (None, Err(_)) => {}
            }
            let word = if up { "up" } else { "down" };
            actions.push(named(format!("role-{word}-{written}"), button).into_any_element());
        }
        if row.named {
            let name = row.name.clone();
            let mut rename = Button::new(("role-rename", number))
                .label(text::admin::RENAME)
                .h(u(28.0))
                .px(u(8.0))
                .disabled(locked.is_some());
            match locked {
                Some(reason) => rename = rename.tooltip(reason),
                None => {
                    rename = rename.on_click(cx.listener(move |this, _, window, cx| {
                        this.set_roles_open(Some(Open::Rename(role)), &name, window, cx);
                    }));
                }
            }
            actions.push(named(format!("role-rename-{written}"), rename).into_any_element());
        }
        let toggle = Button::new(("role-permissions", number))
            .icon(if expanded {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            })
            .label(txt::PERMISSIONS)
            .accessible_label(if expanded {
                txt::HIDE_PERMISSIONS
            } else {
                txt::PERMISSIONS
            })
            .h(u(28.0))
            .px(u(8.0))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.show_role_permissions((!expanded).then_some(role), cx);
            }));
        actions.push(named(format!("role-permissions-{written}"), toggle).into_any_element());
        if row.named {
            let held = row.deletion().err().map(txt::no_deletion);
            let mut delete = Button::new(("role-delete", number))
                .label(text::admin::DELETE)
                .h(u(28.0))
                .px(u(8.0))
                .disabled(held.is_some());
            match held {
                Some(reason) => delete = delete.tooltip(reason),
                None => {
                    delete = delete.on_click(cx.listener(move |this, _, window, cx| {
                        this.set_roles_open(Some(Open::Delete(role)), "", window, cx);
                    }));
                }
            }
            actions.push(named(format!("role-delete-{written}"), delete).into_any_element());
        }
        actions
    }

    /// One role: its row, the reason why the person cannot act on it, and its
    /// permissions while they are shown.
    fn render_role(
        &mut self,
        view: &AdminView,
        page: &Arc<RolesPage>,
        row: &RoleRow,
        index: usize,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = theme.colors;
        let (server_id, role) = (view.server_id, row.role);
        let written = role.written();
        let expanded = self.admin.roles.expanded == Some(role);
        // A form or a question of a role that the person cannot change any more is put
        // away for good: it does not return when the role can be changed again.
        let stale = match self.admin.roles.open {
            Some(Open::Rename(of)) if of == role => row.locked.is_some() || !row.named,
            Some(Open::Delete(of)) if of == role => {
                !row.named
                    || row
                        .deletion()
                        .is_err_and(|reason| reason != NoDeletion::Reading)
            }
            _ => false,
        };
        if stale {
            self.admin.roles.open = None;
            self.admin.roles.sent = None;
        }
        let content: AnyElement = match self.admin.roles.open {
            // A role that became locked while its form or its question was open shows
            // its row again: nothing is asked about a role that the person cannot change.
            Some(Open::Rename(of)) if of == role && row.locked.is_none() && row.named => self
                .render_role_form(
                    text::admin::RENAME,
                    view.sending(AskKind::UpdateRole(role)),
                    None,
                    theme,
                    cx,
                )
                .into_any_element(),
            Some(Open::Delete(of)) if of == role && row.named => div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap(u(8.0))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .typeface(theme.types.small, theme)
                        .ink(colors.ink)
                        .child(txt::delete_question(&row.name, page.level)),
                )
                .child(
                    Button::new("role-question-cancel")
                        .label(text::CANCEL)
                        .selector("role-cancel")
                        .h(u(28.0))
                        .px(u(10.0))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_roles_open(None, "", window, cx);
                        })),
                )
                .child(
                    Button::new("role-question-confirm")
                        .kind(ButtonKind::Primary)
                        .label(text::admin::DELETE)
                        .selector("role-confirm")
                        // While the statement of the server is read again the question
                        // stays and its confirmation waits.
                        .disabled(
                            view.sending(AskKind::DeleteRole(role)) || row.deletion().is_err(),
                        )
                        .h(u(28.0))
                        .px(u(10.0))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.admin.roles.sent = Some(AskKind::DeleteRole(role));
                            this.state.update(cx, |state, cx| {
                                state.delete_role(server_id, role, cx);
                            });
                        })),
                )
                .into_any_element(),
            _ => {
                // The handle is the place where a drag starts. A role that the person
                // cannot move has none.
                let free = row.locked.is_none() && !row.is_everyone;
                let name: SharedString = row.name.clone().into();
                let handle = div()
                    .id(("role-handle", role.number()))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(u(22.0))
                    .when(free, |handle| {
                        let label = name.clone();
                        handle
                            .cursor_grab()
                            .tooltip(crate::ui::tooltip(txt::HANDLE))
                            .on_drag(RoleDrag { server_id, role }, move |_, _, _, cx| {
                                cx.new(|_| DragLabel(label.clone()))
                            })
                            .child(icon(Icon::List, 14.0, colors.ink_2))
                    })
                    .debug_selector({
                        let selector = format!("role-handle-{written}");
                        move || selector
                    });
                let mut content = div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(u(6.0))
                    .child(handle)
                    .child(line_text(
                        row.name.clone(),
                        Some(txt::summary(row.granted()).into()),
                        theme,
                    ));
                if row.is_everyone {
                    content = content.child(Chip::new(txt::EVERY_MEMBER));
                }
                match row.locked {
                    Some(crate::state::admin::view::roles::Locked::OwnHighest) => {
                        content = content.child(Chip::new(txt::YOUR_HIGHEST).icon(Icon::Lock));
                    }
                    Some(crate::state::admin::view::roles::Locked::Above) => {
                        content = content.child(Chip::new(txt::ABOVE_YOU).icon(Icon::Lock));
                    }
                    None if row.held && !row.is_everyone => {
                        content = content.child(Chip::new(txt::YOU_HOLD));
                    }
                    None => {}
                }
                content
                    .children(self.render_role_actions(view, row, expanded, cx))
                    .into_any_element()
            }
        };
        let refused = view
            .refused(AskKind::UpdateRole(role))
            .or(view.refused(AskKind::DeleteRole(role)))
            .or(view.refused(AskKind::MoveRole(role)));
        let accepts = page.clone();
        let hovers = page.clone();
        let mut block = div()
            .id(("role-drop", index))
            .flex()
            .flex_col()
            .flex_none()
            .can_drop(move |value, _, _| {
                value.downcast_ref::<RoleDrag>().is_some_and(|drag| {
                    drag.server_id == server_id && accepts.takes(drag.role, index)
                })
            })
            .drag_over::<RoleDrag>(move |style, drag, _, _| {
                if drag.server_id == server_id && hovers.takes(drag.role, index) {
                    style.bg(Hsla::from(colors.limestone_hover))
                } else {
                    style
                }
            })
            .on_drop(cx.listener(move |this, drag: &RoleDrag, _, cx| {
                this.drop_role(*drag, index, cx);
            }))
            .child(named(format!("role-row-{written}"), line(theme).child(content)).flex_1());
        if let Some(reason) = row.locked {
            let selector = format!("role-locked-{written}");
            block = block.child(
                sentence(txt::locked(reason), theme)
                    .mt(u(3.0))
                    .mb(u(4.0))
                    .pl(u(28.0))
                    .ink(colors.ink)
                    .debug_selector(move || selector),
            );
        } else if row.is_everyone {
            block = block.child(
                sentence(txt::EVERYONE_STAYS, theme)
                    .mt(u(3.0))
                    .mb(u(4.0))
                    .pl(u(28.0)),
            );
        }
        if expanded {
            let mut permissions = div().flex().flex_col().flex_none().pl(u(28.0));
            for (at, switch) in row.permissions.iter().enumerate() {
                permissions = permissions
                    .child(self.render_role_permission(view, row, switch, at, theme, cx));
            }
            block = block.child(permissions);
        }
        block
            .children(refusal(refused, "role-refused"))
            .into_any_element()
    }

    pub(crate) fn render_roles_page(
        &mut self,
        view: &AdminView,
        page: &RolesPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.settle_roles(view, window, cx);
        let theme = theme(cx);
        let level = page.level;
        let (title, help): (&'static str, SharedString) = match &page.workspace_name {
            Some(name) => (
                text::admin::page_name(AdminPage::Roles),
                txt::workspace_help(name).into(),
            ),
            None => (
                text::admin::page_name(AdminPage::ServerRoles),
                txt::SERVER_HELP.into(),
            ),
        };
        let mut column = div()
            .flex()
            .flex_col()
            .child(page_title(title, help, &theme))
            .child(sentence(txt::ORDER_HELP, &theme));
        if let Some(standing) = &page.standing {
            column = column.child(
                div().flex_none().mt(u(8.0)).child(
                    Notice::new(txt::standing(standing, level))
                        .icon(Icon::Key)
                        .selector("role-standing"),
                ),
            );
        }
        // A new role.
        let creating = self.admin.roles.open == Some(Open::Create);
        let create = if creating {
            line(&theme)
                .mt(u(8.0))
                .child(self.render_role_form(
                    text::admin::CREATE,
                    view.sending(AskKind::CreateRole(level)),
                    Some(txt::NEW_ROLE_HELP),
                    &theme,
                    cx,
                ))
                .into_any_element()
        } else {
            let held = page
                .create
                .err()
                .map(|reason| txt::no_creation(reason, level));
            let mut button = Button::new("role-new")
                .icon(Icon::Plus)
                .label(txt::NEW_ROLE)
                .selector("role-new")
                .h(u(30.0))
                .px(u(12.0))
                .disabled(held.is_some());
            if held.is_none() {
                button = button.on_click(cx.listener(|this, _, window, cx| {
                    this.set_roles_open(Some(Open::Create), "", window, cx);
                }));
            }
            let mut row = div()
                .flex()
                .flex_none()
                .items_center()
                .gap(u(10.0))
                .mt(u(10.0))
                .child(button);
            if let Some(reason) = held {
                row = row.child(
                    sentence(reason, &theme)
                        .flex_1()
                        .ink(theme.colors.ink)
                        .debug_selector(|| "role-new-held".to_owned()),
                );
            }
            row.into_any_element()
        };
        column = column.child(create).children(refusal(
            view.refused(AskKind::CreateRole(level)),
            "role-create-refused",
        ));
        let part = AdminPart::ServerRoles;
        let rows = match &page.roles {
            Listed::Loaded(rows) => rows,
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
        column = column.child(sentence(txt::DRAG_HELP, &theme).mt(u(12.0)).mb(u(2.0)));
        if rows.is_empty() {
            column = column.child(
                sentence(txt::NO_ROLES, &theme)
                    .py(u(10.0))
                    .debug_selector(|| "roles-none".to_owned()),
            );
        }
        let shared = Arc::new(page.clone());
        for (index, row) in rows.iter().enumerate() {
            column = column.child(self.render_role(view, &shared, row, index, &theme, cx));
        }
        column.into_any_element()
    }

    /// The roles that an account holds or can be given, one switch each, under the row
    /// of the account in a list of people.
    pub(crate) fn render_role_choices(
        &mut self,
        view: &AdminView,
        account_id: AccountId,
        account: &str,
        choices: &[RoleChoice],
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = view.server_id;
        let colors = theme.colors;
        let mut column = div()
            .flex()
            .flex_col()
            .flex_none()
            .pl(u(34.0))
            .pb(u(6.0))
            .debug_selector(move || format!("account-roles-{account_id}"))
            .child(sentence(txt::ASSIGN_HELP, theme).mt(u(4.0)).mb(u(2.0)));
        if choices.is_empty() {
            column = column.child(sentence(txt::NO_ROLE_TO_GIVE, theme).py(u(6.0)));
        }
        let mut refused = None;
        for choice in choices {
            let role = choice.role;
            let give = !choice.held;
            let what = AskKind::AssignRole(role, account_id);
            refused = refused.or(view.refused(what));
            let blocked = choice.blocked.map(txt::no_assignment);
            let held: Option<&'static str> =
                blocked.or(view.sending(what).then_some(text::admin::SENDING));
            let toggle = Toggle::new(
                ("account-role", role.number()),
                txt::role_label(&choice.name, account),
                choice.held,
                (text::ON, text::OFF),
            )
            .held(held)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.assign_role(server_id, role, account_id, give, cx);
                });
            }));
            let written = role.written();
            let mut row = div()
                .flex()
                .flex_none()
                .items_center()
                .gap(u(10.0))
                .min_h(u(30.0))
                .child(named(
                    format!("account-role-{account_id}-{written}"),
                    toggle,
                ))
                .child(
                    div()
                        .flex_none()
                        .typeface(theme.types.control, theme)
                        .ink(colors.ink)
                        .child(choice.name.clone()),
                );
            if let Some(reason) = blocked {
                let selector = format!("account-role-held-{account_id}-{written}");
                row = row.child(
                    sentence(reason, theme)
                        .flex_1()
                        .min_w_0()
                        .debug_selector(move || selector),
                );
            }
            column = column.child(row);
        }
        column
            .children(refusal(refused, "account-role-refused"))
            .into_any_element()
    }
}
