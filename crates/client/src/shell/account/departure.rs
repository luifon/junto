//! The page of the account layer that leaves a workspace or deletes the account
//! (ADR 0121). Each act has a question in its row that states what stays and what ends.

use gpui::{AnyElement, Context, InteractiveElement, IntoElement, ParentElement, Styled, div};
use junto_client_core::protocol::ids::WorkspaceId;

use super::Mode;
use crate::shell::Shell;
use crate::shell::admin::{group_label, page_title, refusal, sentence};
use crate::state::account::{AccountPage, AccountView, AskKind, DeparturePage};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, ButtonKind, StyleExt};

impl Shell {
    pub(crate) fn render_departure_page(
        &mut self,
        view: &AccountView,
        page: &DeparturePage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let mut column = div()
            .flex()
            .flex_col()
            .child(page_title(
                text::account::page_name(AccountPage::Departure),
                text::account::DEPARTURE_HELP,
                &theme,
            ))
            .child(group_label(text::account::WORKSPACES, &theme));
        if page.workspaces.is_empty() {
            column = column.child(sentence(text::account::NO_WORKSPACE, &theme));
        }
        for workspace in &page.workspaces {
            column = column.child(self.render_departure_row(
                view,
                page,
                workspace.id,
                &workspace.name,
                &theme,
                cx,
            ));
            column = column.children(refusal(
                view.refused(AskKind::LeaveWorkspace(workspace.id)),
                "account-leave-refused",
            ));
        }
        column = column.child(group_label(text::account::DELETE_ACCOUNT_GROUP, &theme));
        column = column.child(self.render_delete_row(view, page, &theme, cx));
        column
            .children(refusal(
                view.refused(AskKind::DeleteAccount),
                "account-delete-refused",
            ))
            .into_any_element()
    }

    fn render_departure_row(
        &mut self,
        view: &AccountView,
        page: &DeparturePage,
        workspace_id: WorkspaceId,
        name: &str,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let asked = Mode::LeaveWorkspace(workspace_id);
        let row = div()
            .flex()
            .flex_none()
            .items_center()
            .gap(u(8.0))
            .py(u(6.0))
            .debug_selector(move || format!("account-leave-row-{workspace_id}"))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .typeface(theme.types.body, theme)
                    .ink(theme.colors.ink)
                    .child(name.to_owned()),
            );
        if self.account.mode == Some(asked) {
            let sending = view.sending(AskKind::LeaveWorkspace(workspace_id));
            return div()
                .flex()
                .flex_col()
                .flex_none()
                .child(row)
                // The question is fixed text. The name of the workspace stays on its line
                // above it: text of a server is no part of a question.
                .child(
                    div()
                        .typeface(theme.types.small, theme)
                        .ink(theme.colors.ink)
                        .debug_selector(|| "account-leave-question".to_owned())
                        .child(text::account::leave_question(
                            page.leaves_the_server(workspace_id),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .gap(u(8.0))
                        .mt(u(6.0))
                        .child(
                            Button::new("account-leave-cancel")
                                .label(text::CANCEL)
                                .selector("account-leave-cancel")
                                .h(u(28.0))
                                .px(u(10.0))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_account_mode(None, "", window, cx);
                                })),
                        )
                        .child(
                            Button::new("account-leave-confirm")
                                .kind(ButtonKind::Primary)
                                .label(if sending {
                                    text::account::LEAVING
                                } else {
                                    text::account::LEAVE_WORKSPACE
                                })
                                .selector("account-leave-confirm")
                                .disabled(view.busy())
                                .h(u(28.0))
                                .px(u(12.0))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.leave_workspace(workspace_id, cx);
                                })),
                        ),
                )
                .into_any_element();
        }
        let allowed = page.may_leave(workspace_id);
        let row = row.child(
            Button::new(("account-leave", workspace_id.as_uuid().as_u128() as u64))
                .label(text::account::LEAVE)
                .selector("account-leave")
                .disabled(view.busy() || !allowed)
                .h(u(28.0))
                .px(u(10.0))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_account_mode(Some(asked), "", window, cx);
                })),
        );
        if allowed {
            row.into_any_element()
        } else {
            div()
                .flex()
                .flex_col()
                .flex_none()
                .child(row)
                .child(sentence(text::account::OWNER_STAYS, theme))
                .into_any_element()
        }
    }

    fn render_delete_row(
        &mut self,
        view: &AccountView,
        page: &DeparturePage,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if page.owner {
            return div()
                .flex()
                .flex_col()
                .flex_none()
                .child(
                    div().flex().flex_none().child(
                        Button::new("account-delete")
                            .label(text::account::DELETE_ACCOUNT)
                            .selector("account-delete")
                            .disabled(true)
                            .h(u(28.0))
                            .px(u(10.0)),
                    ),
                )
                .child(sentence(text::account::OWNER_STAYS, theme).mt(u(6.0)))
                .into_any_element();
        }
        if self.account.mode == Some(Mode::DeleteAccount) {
            let sending = view.sending(AskKind::DeleteAccount);
            return div()
                .flex()
                .flex_col()
                .flex_none()
                .child(
                    div()
                        .typeface(theme.types.small, theme)
                        .ink(theme.colors.ink)
                        .debug_selector(|| "account-delete-question".to_owned())
                        .child(text::account::DELETE_QUESTION),
                )
                .child(
                    div()
                        .flex()
                        .flex_none()
                        .gap(u(8.0))
                        .mt(u(6.0))
                        .child(
                            Button::new("account-delete-cancel")
                                .label(text::CANCEL)
                                .selector("account-delete-cancel")
                                .h(u(28.0))
                                .px(u(10.0))
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_account_mode(None, "", window, cx);
                                })),
                        )
                        .child(
                            Button::new("account-delete-confirm")
                                .kind(ButtonKind::Primary)
                                .label(if sending {
                                    text::account::DELETING
                                } else {
                                    text::account::DELETE_ACCOUNT_CONFIRM
                                })
                                .selector("account-delete-confirm")
                                .disabled(view.busy())
                                .h(u(28.0))
                                .px(u(12.0))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.delete_account(cx);
                                })),
                        ),
                )
                .into_any_element();
        }
        div()
            .flex()
            .flex_none()
            .child(
                Button::new("account-delete")
                    .label(text::account::DELETE_ACCOUNT)
                    .selector("account-delete")
                    .disabled(view.busy())
                    .h(u(28.0))
                    .px(u(10.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_account_mode(Some(Mode::DeleteAccount), "", window, cx);
                    })),
            )
            .into_any_element()
    }

    /// Opens the page that leaves a workspace, with the question for `workspace_id`.
    pub fn open_departure(
        &mut self,
        workspace_id: WorkspaceId,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) {
        self.open_account(AccountPage::Departure, window, cx);
        self.set_account_mode(Some(Mode::LeaveWorkspace(workspace_id)), "", window, cx);
    }

    /// Leaves a workspace, as the confirmation of its question does. Nothing is sent while
    /// a request of the page is on its way.
    pub fn leave_workspace(&mut self, workspace_id: WorkspaceId, cx: &mut Context<Self>) {
        if self.account.sent.is_some() || self.state.read(cx).account_busy() {
            return;
        }
        self.account.sent = Some(AskKind::LeaveWorkspace(workspace_id));
        self.state
            .update(cx, |state, cx| state.leave_workspace(workspace_id, cx));
        cx.notify();
    }

    /// Deletes the account, as the confirmation of its question does.
    pub fn delete_account(&mut self, cx: &mut Context<Self>) {
        if self.account.sent.is_some() || self.state.read(cx).account_busy() {
            return;
        }
        self.account.sent = Some(AskKind::DeleteAccount);
        self.state.update(cx, |state, cx| state.delete_account(cx));
        cx.notify();
    }
}
