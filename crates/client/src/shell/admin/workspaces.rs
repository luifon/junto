//! The page of the workspaces of a server (ADR 0102, ADR 0308): the list, a new workspace,
//! the deletion, and for each workspace its overview; and the overview of a workspace: its
//! name, its logo (ADR 0122), its notification level (ADR 0412) and its AFK channel
//! (ADR 0523).

use std::path::PathBuf;

use super::list::Row;
use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, PathPromptOptions,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder,
};
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};

use super::{
    Form, FormField, Mode, Question, group_label, line, line_text, named, page_title, refusal,
    row_action,
};
use crate::shell::Shell;
use crate::state::admin::AdminOpen;
use crate::state::admin::AskKind;
use crate::state::admin::view::{AdminView, WorkspaceLine, WorkspaceOverviewPage, WorkspacesPage};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{Avatar, Button};

/// Where the row of a workspace is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RowPlace {
    List,
    Overview,
}

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
                page:
                    crate::state::admin::AdminPage::Workspaces
                    | crate::state::admin::AdminPage::WorkspaceOverview,
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
            _ => {}
        }
        cx.notify();
    }

    /// The row of a workspace: on the page of the workspaces with "Open", which opens its
    /// overview, and "Delete"; in the overview of the workspace with its logo and "Rename".
    fn render_workspace_row(
        &mut self,
        view: &AdminView,
        manages: bool,
        row: &WorkspaceLine,
        place: RowPlace,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let server_id = view.server_id;
        let workspace_id: WorkspaceId = row.id;
        let name = row.name.clone();
        let content: AnyElement = match self.admin.mode {
            Some(Mode::RenameWorkspace(id)) if id == workspace_id && page.may_manage => self
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
            Some(Mode::RemoveWorkspaceLogo(id))
                if id == workspace_id && row.has_logo && page.may_manage =>
            {
                self.render_row_question(
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
                .into_any_element()
            }
            Some(Mode::DeleteWorkspace(id)) if id == workspace_id && page.may_manage => self
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
                // Without the permission the actions of a workspace are absent (ADR 0790).
                let in_overview = place == RowPlace::Overview;
                let listed = place == RowPlace::List;
                let manage = None;
                let delete = row
                    .is_default
                    .then_some(text::admin::DEFAULT_WORKSPACE_HELP);
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
                    .when(listed, |line| {
                        line.child(row_action(
                            ("workspace-open", workspace_id.as_uuid().as_u128() as u64),
                            text::admin::OPEN,
                            format!("workspace-open-{workspace_id}"),
                            None,
                            cx.listener(move |this, _, window, cx| {
                                this.open_workspace_overview(workspace_id, window, cx);
                            }),
                        ))
                    })
                    .when(manages && in_overview, |line| {
                        line.child(row_action(
                            ("workspace-logo", workspace_id.as_uuid().as_u128() as u64),
                            text::admin::WORKSPACE_LOGO,
                            format!("workspace-logo-choose-{workspace_id}"),
                            manage,
                            cx.listener(move |this, _, _, cx| {
                                this.choose_workspace_logo(workspace_id, cx);
                            }),
                        ))
                    })
                    .children((manages && in_overview && row.has_logo).then(|| {
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
                    .when(manages && in_overview, |line| {
                        line.child(row_action(
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
                    })
                    .when(manages && listed, |line| {
                        line.child(row_action(
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
                    })
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
        // A form whose permission went is put away: its action is no longer offered
        // (ADR 0790).
        let refused = match self.admin.mode {
            Some(
                Mode::RenameWorkspace(_) | Mode::RemoveWorkspaceLogo(_) | Mode::DeleteWorkspace(_),
            ) => !page.may_manage,
            Some(Mode::CreateWorkspace) => !page.may_create,
            _ => false,
        };
        if refused {
            self.admin.mode = None;
        }
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
                    return this.render_workspace_row(
                        &view,
                        page.may_manage,
                        row,
                        RowPlace::List,
                        &theme,
                        cx,
                    );
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
        // A new workspace, for a person who may create one (ADR 0790).
        if page.may_create {
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
                    let button = Button::new("workspace-new")
                        .icon(crate::assets::Icon::Plus)
                        .label(text::admin::NEW_WORKSPACE)
                        .selector("workspace-new")
                        .h(u(30.0))
                        .px(u(12.0))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_admin_mode(Some(Mode::CreateWorkspace), "", window, cx);
                        }));
                    div()
                        .flex()
                        .flex_col()
                        .child(div().flex().flex_none().child(button))
                }
            };
            column = column.child(self.mode_row(&[Mode::CreateWorkspace], new_workspace));
            column = column.children(refusal(
                view.refused(AskKind::CreateWorkspace),
                "workspace-create-refused",
            ));
        }
        column.into_any_element()
    }

    /// Opens the overview of a workspace in the layer, as "Open" of its row does.
    pub(crate) fn open_workspace_overview(
        &mut self,
        workspace_id: WorkspaceId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = self.state.read(cx).admin_open() else {
            return;
        };
        self.reset_admin_ui(window, cx);
        self.state.update(cx, |state, cx| {
            state.open_admin_in_workspace(
                AdminOpen::Server {
                    server_id: open.server_id(),
                    page: crate::state::admin::AdminPage::WorkspaceOverview,
                },
                workspace_id,
                cx,
            );
        });
    }

    /// The overview of a workspace (ADR 0761).
    pub(crate) fn render_workspace_overview(
        &mut self,
        view: &AdminView,
        page: &WorkspaceOverviewPage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let mut column = div()
            .flex()
            .flex_col()
            .child(page_title(
                text::admin::page_name(crate::state::admin::AdminPage::WorkspaceOverview),
                text::admin::workspace_overview_help(&page.workspace.name),
                &theme,
            ))
            .child(group_label(text::admin::WORKSPACE_NAME_AND_LOGO, &theme))
            .child(self.render_workspace_row(
                view,
                page.may_manage,
                &page.workspace,
                RowPlace::Overview,
                &theme,
                cx,
            ));
        if let Some(notifications) = &page.notifications {
            column = column.child(self.render_notification_default(view, notifications, cx));
        }
        if let Some(channels) = &page.afk_channels {
            column = column.child(self.render_afk_settings(
                view.server_id,
                page.workspace.id,
                channels,
                cx,
            ));
        }
        column.into_any_element()
    }
}
