//! The menu of a server, the menu under the name of the channel column, and the questions
//! that the menu and the state of a server lead to: logging out, removing the server from this
//! device, and forgetting its account.

use std::ops::Not as _;

use gpui::{
    AnyElement, Context, Hsla, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
    prelude::FluentBuilder,
};
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};

use super::{Layer, MenuAt, Shell};
use crate::assets::Icon;
use crate::state::view::{ServerStatus, ShellView};
use crate::text;
use crate::theme::{theme, u};
use crate::ui::{
    Button, ButtonKind, Dialog, Menu, MenuItem, Mosaic, MosaicKind, StyleExt, dialog_footer, icon,
    menu_label, menu_separator, paint_text,
};

impl Shell {
    fn open_invite_people(
        &mut self,
        server_id: ServerId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self
            .state
            .read(cx)
            .view_for_server(server_id)
            .selected
            .is_some_and(|server| server.may_invite_people)
        {
            return;
        }
        self.state
            .update(cx, |state, cx| state.select_server(server_id, cx));
        self.open_server_admin(crate::state::admin::AdminPage::Invites, window, cx);
        self.set_people_open(Some(super::admin::people::Open::NewInvite), window, cx);
    }

    /// Opens the menu of the selected server at its row in the server book.
    pub(crate) fn open_server_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(server_id) = self
            .state
            .read(cx)
            .view()
            .selected
            .as_ref()
            .map(|server| server.id)
        {
            self.open_server_menu_of(server_id, MenuAt::Row, window, cx);
        }
    }

    /// Opens the menu of a server: from its row or icon in the server book, or from the
    /// title of the channel column of a server that shows no workspace.
    pub(crate) fn open_server_menu_of(
        &mut self,
        server_id: ServerId,
        at: MenuAt,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A server of the server book, as the view lists it.
        if !self
            .state
            .read(cx)
            .view()
            .servers
            .iter()
            .any(|row| row.id == server_id)
        {
            return;
        }
        // A move of a server that failed is told in the menu that asked for it.
        self.state
            .update(cx, |state, cx| state.dismiss_move_failure(cx));
        self.open_layer(Layer::ServerMenu(server_id, at), window, cx);
    }

    /// The arrow beside the name at the top of the channel column: the menu of the shown
    /// workspace, or the menu of the server while the server shows no workspace.
    pub(crate) fn open_header_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A click on the button of an open menu closes the menu: the press closed it, and
        // the click must not open it again.
        if std::mem::take(&mut self.server_menu_closed_by_press) {
            return;
        }
        let view = self.state.read(cx).view();
        let Some(server_id) = view.selected.as_ref().map(|server| server.id) else {
            return;
        };
        match view.workspace().map(|workspace| workspace.id) {
            Some(workspace_id) => {
                self.open_workspace_menu_at(server_id, workspace_id, MenuAt::Header, window, cx);
            }
            None => self.open_server_menu_of(server_id, MenuAt::Header, window, cx),
        }
    }

    /// The menu that the arrow of the channel column opened, under the header.
    pub(crate) fn render_header_menu(
        &mut self,
        server_id: Option<ServerId>,
        workspace_id: Option<WorkspaceId>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = theme(cx);
        let menu = match (server_id, workspace_id) {
            (Some(server_id), Some(workspace_id)) => {
                let view = self.state.read(cx).view_for_server(server_id);
                view.servers
                    .iter()
                    .find(|row| row.id == server_id)
                    .cloned()
                    .and_then(|row| {
                        let position = row
                            .workspaces
                            .iter()
                            .position(|workspace| workspace.id == workspace_id)?;
                        Some(self.render_workspace_menu_items(&row, position, MenuAt::Header, cx))
                    })
            }
            (Some(server_id), None) => Some(self.render_server_menu(server_id, cx)),
            _ => None,
        };
        div()
            .absolute()
            .left(u(self.rail_width + 8.0))
            .top(u(theme.metrics.header_height - 4.0))
            .children(menu)
            .into_any_element()
    }

    /// The menu of a server (ADR 0780): the notifications of the server, mark as read,
    /// joining further workspaces, inviting people, Server settings and the account; the
    /// items that arrange the server book; and the items that concern this device.
    pub(crate) fn render_server_menu(
        &mut self,
        server_id: ServerId,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let theme = theme(cx);
        let colors = theme.colors;
        let view = self.state.read(cx).view_for_server(server_id);
        let shown = self
            .state
            .read(cx)
            .view()
            .selected
            .as_ref()
            .map(|server| server.id)
            == Some(server_id);
        let Some(server) = &view.selected else {
            return div().into_any_element();
        };
        let keep = server.keep_connected;
        let can_log_out = server.can_log_out;
        let has_content = !matches!(server.content, crate::state::view::Content::State);
        let forgettable = matches!(
            server.status,
            ServerStatus::WaitingForLink { .. } | ServerStatus::Banned
        );
        let unread = view
            .servers
            .iter()
            .find(|row| row.id == server_id)
            .is_some_and(|row| row.unread.any());
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(9.0))
            .pt(u(6.0))
            .px(u(12.0))
            .pb(u(8.0))
            .mb(u(4.0))
            .border_b_1()
            .border_color(Hsla::from(colors.rule_soft))
            // The logo of the server when the window holds it, as in the server book
            // (ADR 0799).
            .child(
                match view
                    .servers
                    .iter()
                    .enumerate()
                    .find(|(_, row)| row.id == server_id)
                {
                    Some((index, row)) => {
                        super::rail::picture("selected-server-logo", index, row, 26.0, 2.0)
                    }
                    None => Mosaic::new(MosaicKind::Large, server_id.as_uuid().as_u128(), 26.0)
                        .radius(2.0)
                        .into_any_element(),
                },
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .typeface(theme.types.name, &theme)
                            .line_height(u(18.0))
                            .ink(colors.ink)
                            .truncate()
                            .child(server.name.clone()),
                    )
                    .child(
                        div()
                            .typeface(theme.types.mono_small, &theme)
                            .line_height(u(16.0))
                            .ink(colors.ink_2)
                            .truncate()
                            .child(server.address.clone()),
                    )
                    // The version that the server states, and a newer release of the server
                    // (ADR 0780): lines of text, not items of the menu.
                    .when_some(server.release.clone(), |column, release| {
                        column
                            .child(
                                div()
                                    .debug_selector(|| "server-version".to_owned())
                                    .mt(u(2.0))
                                    .typeface(theme.types.small, &theme)
                                    .line_height(u(16.0))
                                    .ink(colors.ink_2)
                                    .truncate()
                                    .child(paint_text(
                                        text::book::server_version(&release.version).into(),
                                        None,
                                    )),
                            )
                            .when_some(release.newer, |column, newer| {
                                column.child(
                                    div()
                                        .debug_selector(|| "server-release-available".to_owned())
                                        .flex()
                                        .items_center()
                                        .gap(u(5.0))
                                        .typeface(theme.types.small_strong, &theme)
                                        .line_height(u(16.0))
                                        .ink(colors.cypress_ink)
                                        .child(icon(Icon::Download, 12.0, colors.cypress_ink))
                                        .child(div().flex_1().min_w_0().truncate().child(
                                            paint_text(
                                                text::book::server_release_available(newer).into(),
                                                None,
                                            ),
                                        )),
                                )
                            })
                            // The server sends what this build does not read (ADR 0206).
                            .when(release.newer_events, |column| {
                                column.child(
                                    div()
                                        .debug_selector(|| "server-events-newer".to_owned())
                                        .typeface(theme.types.small_strong, &theme)
                                        .line_height(u(16.0))
                                        .ink(colors.cinnabar_ink)
                                        .child(paint_text(text::book::NEWER_EVENTS.into(), None)),
                                )
                            })
                    }),
            );
        let mut items: Vec<gpui::AnyElement> = Vec::new();
        if server.status.shows_content() {
            items.push(
                MenuItem::new("server-notifications", text::SERVER_NOTIFICATIONS)
                    .icon(Icon::Bell)
                    .selector("server-notifications")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_own_notifications(server_id, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        // Shown only while something is unread.
        if unread {
            items.push(
                MenuItem::new("mark-server-read", text::book::MARK_SERVER_READ)
                    .icon(Icon::CheckDouble)
                    .when(shown, |item| {
                        item.shortcut(junto_client_core::keybinds::KeyAction::MarkServerRead)
                    })
                    .selector("menu-mark-server-read")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state
                            .update(cx, |state, _| state.mark_server_read(server_id, None));
                        this.close_layer(window, cx);
                    }))
                    .into_any_element(),
            );
        }
        // An invite of the same server adds further workspaces to the account (ADR 0115).
        if has_content {
            items.push(
                MenuItem::new("join-with-invite", text::JOIN_WITH_INVITE)
                    .icon(Icon::Ticket)
                    .selector("menu-join-with-invite")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_add_window(
                            super::add_server::AddTab::Workspace,
                            Some(server_id),
                            window,
                            cx,
                        );
                    }))
                    .into_any_element(),
            );
        }
        if server.may_invite_people {
            items.push(
                MenuItem::new("invite-people", text::INVITE_PEOPLE)
                    .icon(Icon::Invite)
                    .selector("menu-invite-people")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_invite_people(server_id, window, cx)
                    }))
                    .into_any_element(),
            );
        }
        // Server settings and the account need the state of the server.
        if has_content {
            items.push(
                MenuItem::new("administration", text::admin::OPEN_ADMINISTRATION)
                    .icon(Icon::Gear)
                    .when(shown, |item| {
                        item.shortcut(junto_client_core::keybinds::KeyAction::OpenAdministration)
                    })
                    .selector("menu-administration")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state
                            .update(cx, |state, cx| state.select_server(server_id, cx));
                        // The first page that the person may use opens.
                        this.open_server_admin(crate::state::admin::AdminPage::Server, window, cx);
                    }))
                    .into_any_element(),
            );
            items.push(
                MenuItem::new("your-account", text::account::OPEN_ACCOUNT)
                    .icon(Icon::Account)
                    .selector("menu-account")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.state
                            .update(cx, |state, cx| state.select_server(server_id, cx));
                        this.open_account(crate::state::account::AccountPage::Profile, window, cx);
                    }))
                    .into_any_element(),
            );
        }
        let arrange = self.render_move_items(&view, cx);
        let mut device: Vec<gpui::AnyElement> = vec![
            MenuItem::new("keep-connected", text::KEEP_CONNECTED)
                .icon(Icon::Plug)
                .checked(keep)
                .selector("keep-connected")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.state.update(cx, |state, cx| {
                        state.set_keep_connected(server_id, !keep, cx);
                    });
                }))
                .into_any_element(),
            // Drawn in every state, so that a change of the state while the menu is open
            // moves no item under the pointer (ADR 0730).
            MenuItem::new("log-out", text::LOG_OUT)
                .icon(Icon::Leave)
                .offer(&crate::ui::Offer::now(
                    (!can_log_out).then_some(text::LOG_OUT_UNAVAILABLE),
                ))
                .selector("menu-log-out")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.layer = Layer::ConfirmLogOut(server_id);
                    this.layer_focus().clone().focus(window, cx);
                    cx.notify();
                }))
                .into_any_element(),
            MenuItem::new("remove-server", text::REMOVE_SERVER)
                .icon(Icon::Trash)
                .selector("menu-remove-server")
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.layer = Layer::ConfirmRemove(server_id);
                    this.layer_focus().clone().focus(window, cx);
                    cx.notify();
                }))
                .into_any_element(),
        ];
        if forgettable {
            device.push(
                MenuItem::new("forget-account", text::FORGET_ACCOUNT)
                    .icon(Icon::Trash)
                    .selector("menu-forget-account")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.layer = Layer::ConfirmForget(server_id);
                        this.layer_focus().clone().focus(window, cx);
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let separated = !items.is_empty();
        let menu = Menu::new(
            "server-menu",
            text::book::server_menu_of(&server.name),
            self.layer_focus(),
        )
        .selector("server-menu")
        .on_close(Self::handler(cx, |this, window, cx| {
            this.server_menu_closed_by_press = window.last_input_was_keyboard().not();
            this.close_layer(window, cx);
        }))
        .child(header);
        // "Arrange ›" opens its items in the place of the menu, with a way back (ADR 0780).
        if self.menu_arranging {
            return menu
                .child(arrange_back(cx))
                .child(menu_separator(cx))
                .children(arrange)
                .child(self.render_move_error(&view, cx))
                .child(div().h(u(4.0)))
                .into_any_element();
        }
        menu.children(items)
            .when(separated, |menu| menu.child(menu_separator(cx)))
            .child(arrange_open("menu-arrange", cx))
            .child(menu_separator(cx))
            .child(menu_label(text::book::ON_THIS_DEVICE, false, cx))
            .children(device)
            .child(div().h(u(4.0)))
            .into_any_element()
    }

    fn render_question(
        &mut self,
        question: Question,
        server: String,
        cx: &mut Context<Self>,
        confirmed: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> gpui::AnyElement {
        let theme = theme(cx);
        let Question {
            title,
            sentence,
            confirm,
            selector,
        } = question;
        Dialog::new(title, self.layer_focus())
            .selector(selector)
            .on_close(Self::handler(cx, |this, window, cx| {
                this.close_layer(window, cx);
            }))
            // The name comes from the server: it has a line of its own that is cut at the
            // width of the dialog, so that it cannot move a control.
            .child(
                div()
                    .mb(u(6.0))
                    .typeface(theme.types.heading, &theme)
                    .ink(theme.colors.ink)
                    .truncate()
                    .child(server),
            )
            .child(
                div()
                    .typeface(theme.types.notice, &theme)
                    .ink(theme.colors.ink_2)
                    .child(sentence),
            )
            .child(dialog_footer([
                Button::new("cancel")
                    .label(text::CANCEL)
                    .selector("cancel")
                    .h(u(30.0))
                    .px(u(12.0))
                    .on_click(cx.listener(|this, _, window, cx| this.close_layer(window, cx)))
                    .into_any_element(),
                Button::new("confirm")
                    .kind(ButtonKind::Primary)
                    .label(confirm)
                    .selector("confirm")
                    .h(u(30.0))
                    .px(u(12.0))
                    .on_click(cx.listener(move |this, _, window, cx| confirmed(this, window, cx)))
                    .into_any_element(),
            ]))
            .into_any_element()
    }

    pub(crate) fn render_confirm_remove(
        &mut self,
        server_id: ServerId,
        view: &ShellView,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // The key of an inactive account is the way back to it (ADR 0121).
        let inactive = view
            .servers
            .iter()
            .any(|row| row.id == server_id && row.status == ServerStatus::Inactive);
        self.render_question(
            Question {
                title: text::REMOVE_SERVER_TITLE,
                sentence: if inactive {
                    text::REMOVE_INACTIVE_SERVER_CONFIRM
                } else {
                    text::REMOVE_SERVER_CONFIRM
                },
                confirm: text::REMOVE,
                selector: "confirm-remove",
            },
            server_name(view, server_id),
            cx,
            move |this, window, cx| {
                this.state
                    .update(cx, |state, cx| state.remove_server(server_id, cx));
                this.close_layer(window, cx);
            },
        )
    }

    /// The question before a logout, with what the logout ends and what stays (ADR 0730).
    pub(crate) fn render_confirm_log_out(
        &mut self,
        server_id: ServerId,
        view: &ShellView,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.render_question(
            Question {
                title: text::LOG_OUT_TITLE,
                sentence: text::LOG_OUT_CONFIRM,
                confirm: text::LOG_OUT,
                selector: "confirm-log-out",
            },
            server_name(view, server_id),
            cx,
            move |this, window, cx| {
                this.state
                    .update(cx, |state, cx| state.log_out(server_id, cx));
                this.close_layer(window, cx);
            },
        )
    }

    pub(crate) fn render_confirm_forget(
        &mut self,
        server_id: ServerId,
        view: &ShellView,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        self.render_question(
            Question {
                title: text::FORGET_ACCOUNT_TITLE,
                sentence: text::FORGET_ACCOUNT_CONFIRM,
                confirm: text::FORGET,
                selector: "confirm-forget",
            },
            server_name(view, server_id),
            cx,
            move |this, window, cx| {
                this.state
                    .update(cx, |state, cx| state.forget_account(server_id, cx));
                this.close_layer(window, cx);
            },
        )
    }
}

/// A question that a person answers before an act that cannot be undone on the device.
struct Question {
    title: &'static str,
    sentence: &'static str,
    /// The label of the button that confirms.
    confirm: &'static str,
    selector: &'static str,
}

fn server_name(view: &ShellView, server_id: ServerId) -> String {
    view.servers
        .iter()
        .find(|row| row.id == server_id)
        .map(|row| row.name.clone())
        .unwrap_or_default()
}

/// The item "Arrange ›" of the menu of a server or of a workspace.
pub(super) fn arrange_open(selector: &'static str, cx: &mut Context<Shell>) -> AnyElement {
    MenuItem::new(selector, text::book::ARRANGE)
        .icon(Icon::Grip)
        .end_icon(Icon::ChevronRight)
        .selector(selector)
        .on_click(cx.listener(|this, _, _, cx| {
            this.menu_arranging = true;
            cx.notify();
        }))
        .into_any_element()
}

/// The first item of the submenu "Arrange", which returns to the menu.
pub(super) fn arrange_back(cx: &mut Context<Shell>) -> AnyElement {
    MenuItem::new("menu-arrange-back", text::book::ARRANGE)
        .icon(Icon::ChevronLeft)
        .selector("menu-arrange-back")
        .on_click(cx.listener(|this, _, _, cx| {
            this.menu_arranging = false;
            cx.notify();
        }))
        .into_any_element()
}

impl Shell {
    /// Shows the submenu "Arrange" of the open menu. The screen tool uses it.
    pub fn show_arrange_items(&mut self, cx: &mut Context<Self>) {
        self.menu_arranging = true;
        cx.notify();
    }
}
