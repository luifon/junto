//! The menu of the selected server, and the two questions that it and the state of a
//! server lead to: removing the server from this device, and forgetting its account.

use std::ops::Not as _;

use gpui::{
    Context, Hsla, InteractiveElement, IntoElement, ParentElement, Styled, Window, div,
    prelude::FluentBuilder,
};
use junto_client_core::protocol::ids::ServerId;

use super::{Layer, Shell};
use crate::assets::Icon;
use crate::state::view::{ServerStatus, ShellView};
use crate::text;
use crate::theme::{theme, u};
use crate::ui::{
    Button, ButtonKind, Dialog, Menu, MenuItem, Mosaic, MosaicKind, StyleExt, dialog_footer, icon,
    menu_separator, paint_text,
};

impl Shell {
    pub(crate) fn open_server_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // A click on the button of an open menu closes the menu: the press closed it, and
        // the click must not open it again.
        if std::mem::take(&mut self.server_menu_closed_by_press) {
            return;
        }
        if self.state.read(cx).view().selected.is_some() {
            // A move of a server that failed is told in the menu that asked for it.
            self.state
                .update(cx, |state, cx| state.dismiss_move_failure(cx));
            self.open_layer(Layer::ServerMenu, window, cx);
        }
    }

    pub(crate) fn render_server_menu(
        &mut self,
        view: &ShellView,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = theme(cx);
        let colors = theme.colors;
        let Some(server) = &view.selected else {
            return div().into_any_element();
        };
        let server_id = server.id;
        let keep = server.keep_connected;
        let can_log_out = server.can_log_out;
        let has_content = !matches!(server.content, crate::state::view::Content::State);
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
                    }),
            );
        // The notification settings of the server and of the shown workspace, for a
        // server that shows its lists.
        let workspace_id = view.workspace().map(|workspace| workspace.id);
        let mut notifications: Vec<gpui::AnyElement> = Vec::new();
        if server.status.shows_content() {
            notifications.push(menu_separator(cx).into_any_element());
            notifications.push(
                MenuItem::new("server-notifications", text::SERVER_NOTIFICATIONS)
                    .icon(Icon::Bell)
                    .selector("server-notifications")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_notify_settings(
                            server_id,
                            junto_client_core::api::SettingsScope::Account,
                            window,
                            cx,
                        );
                    }))
                    .into_any_element(),
            );
        }
        if let Some(workspace_id) = workspace_id {
            notifications.push(
                MenuItem::new("workspace-notifications", text::WORKSPACE_NOTIFICATIONS)
                    .icon(Icon::Bell)
                    .selector("workspace-notifications")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_notify_settings(
                            server_id,
                            junto_client_core::api::SettingsScope::Workspace(workspace_id),
                            window,
                            cx,
                        );
                    }))
                    .into_any_element(),
            );
        }
        div()
            .absolute()
            .left(u(self.rail_width + 8.0))
            .top(u(theme.metrics.header_height - 4.0))
            .child(
                Menu::new("server-menu", text::SERVER_MENU, self.layer_focus())
                    .selector("server-menu")
                    .on_close(Self::handler(cx, |this, window, cx| {
                        this.server_menu_closed_by_press = window.last_input_was_keyboard().not();
                        this.close_layer(window, cx);
                    }))
                    .child(header)
                    .child(
                        MenuItem::new("keep-connected", text::KEEP_CONNECTED)
                            .icon(Icon::Plug)
                            .detail(text::KEEP_CONNECTED_HELP)
                            .trailing(if keep { text::ON } else { text::OFF })
                            .selector("keep-connected")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.state.update(cx, |state, cx| {
                                    state.set_keep_connected(server_id, !keep, cx);
                                });
                            })),
                    )
                    .children(notifications)
                    .children(self.render_move_items(view, cx))
                    // The administration needs the state of the server.
                    .when(has_content, |menu| {
                        menu.child(
                            MenuItem::new("administration", text::admin::OPEN_ADMINISTRATION)
                                .icon(Icon::Gear)
                                .detail(text::admin::OPEN_ADMINISTRATION_HELP)
                                .selector("menu-administration")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_server_admin(
                                        crate::state::admin::AdminPage::Workspaces,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                    })
                    // The account layer needs the state of the server too.
                    .when(has_content, |menu| {
                        menu.child(
                            MenuItem::new("your-account", text::account::OPEN_ACCOUNT)
                                .icon(Icon::Account)
                                .detail(text::account::OPEN_ACCOUNT_HELP)
                                .selector("menu-account")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.open_account(
                                        crate::state::account::AccountPage::Profile,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                    })
                    // Leaving the shown workspace opens the page of the account layer that
                    // states what stays and what ends (ADR 0121).
                    .when_some(
                        workspace_id.filter(|_| has_content),
                        |menu, workspace_id| {
                            menu.child(
                                MenuItem::new("leave-workspace", text::account::LEAVE_WORKSPACE)
                                    .icon(Icon::Leave)
                                    .detail(text::account::LEAVE_WORKSPACE_HELP)
                                    .selector("menu-leave-workspace")
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.open_departure(workspace_id, window, cx);
                                    })),
                            )
                        },
                    )
                    .child(menu_separator(cx))
                    // Drawn in every state, so that a change of the state while the menu is
                    // open moves no item under the pointer (ADR 0730).
                    .child(
                        MenuItem::new("log-out", text::LOG_OUT)
                            .icon(Icon::Leave)
                            .detail(if can_log_out {
                                text::LOG_OUT_HELP
                            } else {
                                text::LOG_OUT_UNAVAILABLE
                            })
                            .disabled(!can_log_out)
                            .selector("menu-log-out")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.state
                                    .update(cx, |state, cx| state.log_out(server_id, cx));
                                this.close_layer(window, cx);
                            })),
                    )
                    .child(
                        MenuItem::new("remove-server", text::REMOVE_SERVER)
                            .icon(Icon::Trash)
                            .detail(text::REMOVE_SERVER_HELP)
                            .selector("menu-remove-server")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.layer = Layer::ConfirmRemove(server_id);
                                this.layer_focus().clone().focus(window, cx);
                                cx.notify();
                            })),
                    )
                    .child(div().h(u(4.0))),
            )
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
