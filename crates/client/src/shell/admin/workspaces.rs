//! The page of the workspaces of a server (ADR 0102, ADR 0115, ADR 0308): the list, a new
//! workspace, a new name, the deletion, and the invite code that enters a further
//! workspace.

use std::path::PathBuf;

use super::list::Row;
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, PathPromptOptions,
    SharedString, StatefulInteractiveElement, Styled, Window, div,
};
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};

use super::{
    Form, FormField, Mode, Question, group_label, line, line_text, named, page_title, refusal,
    row_action, sentence,
};
use crate::shell::Shell;
use crate::state::admin::AdminOpen;
use crate::state::admin::view::{AdminView, WorkspaceLine, WorkspacesPage};
use crate::state::admin::{AskKind, Made};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{Avatar, Button, Notice};

fn detail(row: &WorkspaceLine) -> Option<SharedString> {
    let mut parts = Vec::new();
    if row.is_default {
        parts.push(text::admin::DEFAULT_WORKSPACE);
    }
    if row.shown {
        parts.push(text::admin::SHOWN_WORKSPACE);
    }
    if !row.member {
        parts.push(text::admin::NOT_A_MEMBER);
    }
    (!parts.is_empty()).then(|| parts.join(" · ").into())
}

impl Shell {
    /// The server whose page of the workspaces is open, with its owner.
    fn workspaces_page_owner(
        &self,
        cx: &gpui::App,
    ) -> Option<(ServerId, junto_client_core::Owner)> {
        let state = self.state.read(cx);
        match state.admin_open() {
            Some(AdminOpen::Server {
                server_id,
                page: crate::state::admin::AdminPage::Workspaces,
            }) => state.owner(server_id).map(|owner| (server_id, owner)),
            _ => None,
        }
    }

    /// Asks the person for a file, and sends it as the logo of a workspace of the page.
    fn choose_workspace_logo(&mut self, workspace_id: WorkspaceId, cx: &mut Context<Self>) {
        let Some(asked) = self.workspaces_page_owner(cx) else {
            return;
        };
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(file) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| {
                    // The dialog does not stop the window: the file is sent only while the
                    // page of the workspaces of the same owner is still open (ADR 0746).
                    if this.workspaces_page_owner(cx) == Some(asked) {
                        this.use_workspace_logo(workspace_id, file, cx);
                    }
                });
            }
        })
        .detach();
    }

    /// Sends the file at this path as the logo of a workspace of the open page of the
    /// workspaces, as the file dialog does. Nothing is sent when the person left that page.
    pub fn use_workspace_logo(
        &mut self,
        workspace_id: WorkspaceId,
        file: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let Some((server_id, _)) = self.workspaces_page_owner(cx) else {
            return;
        };
        self.state.update(cx, |state, cx| {
            state.set_workspace_logo(server_id, workspace_id, file, cx);
        });
    }

    /// Sends the form of the open row of the page of the workspaces.
    pub(crate) fn submit_workspace_form(
        &mut self,
        view: &AdminView,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let server_id = view.server_id;
        match self.admin.mode {
            Some(Mode::CreateWorkspace) => {
                let name = self.admin_name(cx);
                if name.is_empty() {
                    self.admin.invalid = Some(text::admin::NAME_EMPTY);
                } else {
                    self.admin_sent(AskKind::CreateWorkspace);
                    self.state
                        .update(cx, |state, cx| state.create_workspace(server_id, name, cx));
                }
            }
            Some(Mode::RenameWorkspace(workspace_id)) => {
                let name = self.admin_name(cx);
                if name.is_empty() {
                    self.admin.invalid = Some(text::admin::NAME_EMPTY);
                } else {
                    self.admin_sent(AskKind::RenameWorkspace(workspace_id));
                    self.state.update(cx, |state, cx| {
                        state.rename_workspace(server_id, workspace_id, name, cx);
                    });
                }
            }
            Some(Mode::EnterCode) => {
                let code = self.admin_value(cx);
                if code.is_empty() {
                    self.admin.invalid = Some(text::admin::CODE_EMPTY);
                } else {
                    self.admin_sent(AskKind::RedeemInvite);
                    self.state
                        .update(cx, |state, cx| state.redeem_invite(server_id, &code, cx));
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn render_workspace_row(
        &mut self,
        view: &AdminView,
        page: &WorkspacesPage,
        row: &WorkspaceLine,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = view.server_id;
        let workspace_id: WorkspaceId = row.id;
        let name = row.name.clone();
        let content: AnyElement = match self.admin.mode {
            Some(Mode::RenameWorkspace(id)) if id == workspace_id => self
                .render_form(
                    Form {
                        field: FormField::Name,
                        label: text::admin::WORKSPACE_NAME,
                        field_selector: "workspace-name",
                        send: text::SAVE,
                        sending: view.sending(AskKind::RenameWorkspace(workspace_id)),
                        help: None,
                        held: None,
                        extra: Vec::new(),
                    },
                    theme,
                    cx,
                )
                .into_any_element(),
            Some(Mode::RemoveWorkspaceLogo(id)) if id == workspace_id && row.has_logo => self
                .render_row_question(
                    Question {
                        sentence: text::admin::remove_workspace_logo_question(&row.name).into(),
                        confirm: text::admin::REMOVE_LOGO,
                        confirm_selector: "workspace-logo-remove-confirm",
                        cancel_selector: "workspace-logo-remove-cancel",
                        sending: view.sending(AskKind::RemoveWorkspaceLogo(workspace_id)),
                    },
                    theme,
                    cx,
                    move |this, _, cx| {
                        this.admin_sent(AskKind::RemoveWorkspaceLogo(workspace_id));
                        this.state.update(cx, |state, cx| {
                            state.remove_workspace_logo(server_id, workspace_id, cx);
                        });
                    },
                )
                .into_any_element(),
            Some(Mode::DeleteWorkspace(id)) if id == workspace_id => self
                .render_row_question(
                    Question {
                        sentence: text::admin::delete_workspace_question(&row.name).into(),
                        confirm: text::admin::DELETE,
                        confirm_selector: "workspace-delete-confirm",
                        cancel_selector: "workspace-delete-cancel",
                        sending: view.sending(AskKind::DeleteWorkspace(workspace_id)),
                    },
                    theme,
                    cx,
                    move |this, _, cx| {
                        this.admin_sent(AskKind::DeleteWorkspace(workspace_id));
                        this.state.update(cx, |state, cx| {
                            state.delete_workspace(server_id, workspace_id, cx);
                        });
                    },
                )
                .into_any_element(),
            _ => {
                let manage = (!page.may_manage).then_some(text::admin::MANAGE_WORKSPACE_NEEDS);
                let delete = manage.or(row
                    .is_default
                    .then_some(text::admin::DEFAULT_WORKSPACE_HELP));
                div()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap(u(8.0))
                    .child({
                        let logo = self
                            .state
                            .read(cx)
                            .workspace_logo_image(server_id, workspace_id);
                        let shown = logo.is_some();
                        let picture = Avatar::new(workspace_id.as_uuid().as_u128(), 22.0)
                            .small_pattern()
                            .radius(2.0)
                            .image(logo);
                        if shown {
                            div()
                                .id(SharedString::from(format!("workspace-logo-{workspace_id}")))
                                .role(gpui::Role::Image)
                                .aria_label(text::logo_of(&row.name))
                                .debug_selector(move || format!("workspace-logo-{workspace_id}"))
                                .flex_none()
                                .child(picture)
                                .into_any_element()
                        } else {
                            picture.into_any_element()
                        }
                    })
                    .child(line_text(row.name.clone(), detail(row), theme))
                    .child(row_action(
                        ("workspace-logo", workspace_id.as_uuid().as_u128() as u64),
                        text::admin::WORKSPACE_LOGO,
                        format!("workspace-logo-choose-{workspace_id}"),
                        manage,
                        cx.listener(move |this, _, _, cx| {
                            this.choose_workspace_logo(workspace_id, cx);
                        }),
                    ))
                    .children(row.has_logo.then(|| {
                        row_action(
                            (
                                "workspace-logo-remove",
                                workspace_id.as_uuid().as_u128() as u64,
                            ),
                            text::admin::REMOVE_LOGO,
                            format!("workspace-logo-remove-{workspace_id}"),
                            manage,
                            cx.listener(move |this, _, window, cx| {
                                this.set_admin_mode(
                                    Some(Mode::RemoveWorkspaceLogo(workspace_id)),
                                    "",
                                    window,
                                    cx,
                                );
                            }),
                        )
                    }))
                    .child(row_action(
                        ("workspace-rename", workspace_id.as_uuid().as_u128() as u64),
                        text::admin::RENAME,
                        format!("workspace-rename-{workspace_id}"),
                        manage,
                        cx.listener(move |this, _, window, cx| {
                            this.set_admin_mode(
                                Some(Mode::RenameWorkspace(workspace_id)),
                                &name,
                                window,
                                cx,
                            );
                        }),
                    ))
                    .child(row_action(
                        ("workspace-delete", workspace_id.as_uuid().as_u128() as u64),
                        text::admin::DELETE,
                        format!("workspace-delete-{workspace_id}"),
                        delete,
                        cx.listener(move |this, _, window, cx| {
                            this.set_admin_mode(
                                Some(Mode::DeleteWorkspace(workspace_id)),
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
            .refused(AskKind::RenameWorkspace(workspace_id))
            .or(view.refused(AskKind::DeleteWorkspace(workspace_id)))
            .or(view.refused(AskKind::SetWorkspaceLogo(workspace_id)))
            .or(view.refused(AskKind::RemoveWorkspaceLogo(workspace_id)));
        div()
            .id(SharedString::from(format!(
                "workspace-result-{workspace_id}"
            )))
            .flex()
            .flex_col()
            .flex_none()
            .child(
                self.mode_row(
                    &[
                        Mode::RenameWorkspace(workspace_id),
                        Mode::DeleteWorkspace(workspace_id),
                        Mode::RemoveWorkspaceLogo(workspace_id),
                    ],
                    named(
                        format!("workspace-row-{workspace_id}"),
                        line(theme).child(content),
                    )
                    .flex_1(),
                ),
            )
            .children(refusal(refused, "workspace-refused"))
            .into_any_element()
    }

    pub(crate) fn render_workspaces_page(
        &mut self,
        view: &AdminView,
        page: &WorkspacesPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let list = std::iter::once(Row::part("workspaces-header", true))
            .chain(
                page.rows
                    .iter()
                    .map(|row| Row::item(format!("workspace-{}", row.id), true, row.name.clone())),
            )
            .chain(std::iter::once(Row::part("workspaces-footer", true)))
            .collect();
        self.admin.list.take(Some(view), list, window, cx);
        let view = view.clone();
        let page = page.clone();
        self.render_admin_list(
            move |this, index, _, cx| {
                let theme = theme(cx);
                if index == 0 {
                    return page_title(
                        text::admin::page_name(crate::state::admin::AdminPage::Workspaces),
                        text::admin::WORKSPACES_HELP,
                        &theme,
                    )
                    .into_any_element();
                }
                if let Some(row) = page.rows.get(index - 1) {
                    return this.render_workspace_row(&view, &page, row, &theme, cx);
                }
                this.render_workspaces_footer(&view, &page, cx)
            },
            window,
            cx,
        )
    }

    fn render_workspaces_footer(
        &mut self,
        view: &AdminView,
        page: &WorkspacesPage,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let mut column = div().flex().flex_col();
        column = column.child(
            sentence(
                if page.may_manage {
                    text::admin::DEFAULT_WORKSPACE_HELP
                } else {
                    text::admin::MANAGE_WORKSPACE_NEEDS
                },
                &theme,
            )
            .mt(u(6.0)),
        );
        // A new workspace.
        column = column.child(group_label(text::admin::NEW_WORKSPACE, &theme));
        let new_workspace = match self.admin.mode {
            Some(Mode::CreateWorkspace) => self.render_form(
                Form {
                    field: FormField::Name,
                    label: text::admin::WORKSPACE_NAME,
                    field_selector: "workspace-name",
                    send: text::admin::CREATE,
                    sending: view.sending(AskKind::CreateWorkspace),
                    help: None,
                    held: None,
                    extra: Vec::new(),
                },
                &theme,
                cx,
            ),
            _ => {
                let held = (!page.may_create).then_some(text::admin::CREATE_WORKSPACE_NEEDS);
                let button = Button::new("workspace-new")
                    .icon(crate::assets::Icon::Plus)
                    .label(text::admin::NEW_WORKSPACE)
                    .selector("workspace-new")
                    .disabled(held.is_some())
                    .h(u(30.0))
                    .px(u(12.0))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.set_admin_mode(Some(Mode::CreateWorkspace), "", window, cx);
                    }));
                div()
                    .flex()
                    .flex_col()
                    .child(div().flex().flex_none().child(button))
                    .children(held.map(|reason| sentence(reason, &theme).mt(u(5.0))))
            }
        };
        column = column.child(self.mode_row(&[Mode::CreateWorkspace], new_workspace));
        column = column.children(refusal(
            view.refused(AskKind::CreateWorkspace),
            "workspace-create-refused",
        ));
        // A further workspace with an invite code.
        column = column.child(group_label(text::admin::ENTER_CODE, &theme));
        let enter_code = match self.admin.mode {
            Some(Mode::EnterCode) => self.render_form(
                Form {
                    field: FormField::Value,
                    label: text::admin::INVITE_CODE,
                    field_selector: "redeem-code",
                    send: text::admin::ENTER,
                    sending: view.sending(AskKind::RedeemInvite),
                    help: Some(text::admin::ENTER_CODE_HELP),
                    held: None,
                    extra: Vec::new(),
                },
                &theme,
                cx,
            ),
            _ => div()
                .flex()
                .flex_col()
                .child(
                    div().flex().flex_none().child(
                        Button::new("redeem-open")
                            .icon(crate::assets::Icon::Ticket)
                            .label(text::admin::ENTER_CODE)
                            .selector("redeem-open")
                            .h(u(30.0))
                            .px(u(12.0))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.set_admin_mode(Some(Mode::EnterCode), "", window, cx);
                            })),
                    ),
                )
                .child(sentence(text::admin::ENTER_CODE_HELP, &theme).mt(u(5.0))),
        };
        column = column.child(self.mode_row(&[Mode::EnterCode], enter_code));
        column = column.children(refusal(
            view.refused(AskKind::RedeemInvite),
            "redeem-refused",
        ));
        if let Some(workspace_ids) = view.made.iter().find_map(|made| match made {
            Made::Entered(workspace_ids) => Some(workspace_ids),
            _ => None,
        }) {
            column = column.child(div().flex_none().mt(u(8.0)).child(
                Notice::new(text::admin::entered(workspace_ids.len())).selector("redeemed"),
            ));
        }
        column.into_any_element()
    }
}
