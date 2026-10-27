//! The settings of one person (ADR 0306, ADR 0761): the workspace permissions of a member
//! with one line of three choices per permission, what the member can use in each channel
//! that the person sees, and the server permissions of an account.

use std::rc::Rc;

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    Window, div,
};
use junto_client_core::model::AdminPart;
use junto_client_core::protocol::ids::AccountId;
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

use super::list::Row;
use super::overrides::OnChoose;
use super::{line, line_text, page_title, refusal, sentence};
use crate::shell::Shell;
use crate::state::admin::AskKind;
use crate::state::admin::view::person::{
    AccessPage, MemberPermissionsPage, PersonSettings, ServerPermissionsPage,
};
use crate::state::admin::view::{AdminView, Listed};
use crate::text;
use crate::text::admin::overrides::Place;
use crate::theme::{Theme, theme, u};
use crate::ui::StyleExt;

impl Shell {
    /// The statement of a page whose collection is on its way or was not read.
    fn person_reading<T>(
        &mut self,
        view: &AdminView,
        part: AdminPart,
        listed: &Listed<T>,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match listed {
            Listed::Loaded(_) => None,
            Listed::Loading => Some(self.render_reading(view.server_id, part, None, theme, cx)),
            Listed::Failed(failure) => {
                Some(self.render_reading(view.server_id, part, Some(*failure), theme, cx))
            }
        }
    }

    /// The sentence of a page whose changes are locked.
    fn person_locked(reason: Option<&'static str>, theme: &Theme) -> Option<AnyElement> {
        reason.map(|reason| {
            sentence(reason, theme)
                .mt(u(8.0))
                .ink(theme.colors.ink)
                .debug_selector(|| "person-locked".to_owned())
                .into_any_element()
        })
    }

    pub(crate) fn render_member_permissions_page(
        &mut self,
        view: &AdminView,
        settings: &PersonSettings,
        page: &MemberPermissionsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (server_id, account_id) = (view.server_id, settings.account_id);
        let workspace_id = page.workspace_id;
        let what = AskKind::SetMemberPermission(workspace_id, account_id);
        let lines = page.lines.loaded().cloned().unwrap_or_default();
        let rows = std::iter::once(Row::new("person-header", true).part())
            .chain(lines.iter().map(|line| {
                Row::new(
                    format!("member-permission-{}", line.permission.name()),
                    true,
                )
            }))
            .collect();
        self.admin.list.take(Some(view), rows, window, cx);
        let view = view.clone();
        let page = page.clone();
        let locked = page.locked.map(text::admin::overrides::locked);
        let on_choose: OnChoose<WorkspacePermission> =
            Rc::new(move |this: &mut Shell, permission, choice, cx| {
                this.state.update(cx, |state, cx| {
                    state.set_member_permission(
                        server_id,
                        (workspace_id, account_id),
                        permission,
                        choice,
                        cx,
                    );
                });
            });
        self.render_admin_list(
            move |this, index, _, cx| {
                let theme = theme(cx);
                if index == 0 {
                    let part = AdminPart::MemberPermissions(workspace_id, account_id);
                    let mut column = div()
                        .flex()
                        .flex_col()
                        .child(page_title(
                            text::admin::overrides::MEMBER_PERMISSIONS,
                            text::admin::overrides::member_help(&page.workspace_name),
                            &theme,
                        ))
                        .child(sentence(text::admin::overrides::MEMBER_CHANNELS, &theme))
                        .children(Self::person_locked(locked, &theme))
                        .children(refusal(view.refused(what), "person-refused"));
                    if let Some(reading) = this.person_reading(&view, part, &page.lines, &theme, cx)
                    {
                        column = column.child(reading);
                    }
                    return column.into_any_element();
                }
                let held: Option<SharedString> = locked
                    .map(SharedString::from)
                    .or_else(|| view.sending(what).then(|| text::admin::SENDING.into()));
                this.render_rule_permission(
                    &lines[index - 1],
                    index - 1,
                    (Place::Workspace, held),
                    &on_choose,
                    &theme,
                    cx,
                )
            },
            window,
            cx,
        )
    }

    pub(crate) fn render_access_page(
        &mut self,
        view: &AdminView,
        settings: &PersonSettings,
        page: &AccessPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let account_id = settings.account_id;
        let channels = page.channels.loaded().cloned().unwrap_or_default();
        let rows = std::iter::once(Row::new("person-header", false).part())
            .chain(
                channels
                    .iter()
                    .map(|line| Row::new(format!("access-{}", line.channel_id), false)),
            )
            .collect();
        self.admin.list.take(Some(view), rows, window, cx);
        let view = view.clone();
        let page = page.clone();
        let workspace_id = match settings.selected {
            crate::state::admin::PersonPage::Access(workspace_id)
            | crate::state::admin::PersonPage::Permissions(workspace_id) => Some(workspace_id),
            crate::state::admin::PersonPage::Server => None,
        };
        self.render_admin_list(
            move |this, index, _, cx| {
                let theme = theme(cx);
                if index == 0 {
                    let mut column = div().flex().flex_col().child(page_title(
                        text::admin::overrides::ACCESS,
                        text::admin::overrides::access_help(&page.workspace_name),
                        &theme,
                    ));
                    if let Some(workspace_id) = workspace_id {
                        let part = AdminPart::MemberAccess(workspace_id, account_id);
                        if let Some(reading) =
                            this.person_reading(&view, part, &page.channels, &theme, cx)
                        {
                            column = column.child(reading);
                        }
                    }
                    if page.channels.loaded().is_some_and(Vec::is_empty) {
                        column = column.child(
                            sentence(text::admin::overrides::ACCESS_EMPTY, &theme).py(u(10.0)),
                        );
                    }
                    return column.into_any_element();
                }
                let channel = &channels[index - 1];
                let channel_id = channel.channel_id;
                let name = text::admin::channel_name(channel.kind, &channel.name);
                let name = match &channel.category {
                    Some(category) => format!("{name} · {category}"),
                    None => name,
                };
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .debug_selector(move || format!("access-channel-{channel_id}"))
                    .child(
                        line(&theme)
                            .flex_col()
                            .items_start()
                            .child(line_text(name, None, &theme).w_full())
                            .child(
                                sentence(
                                    text::admin::overrides::access_line(&channel.permissions),
                                    &theme,
                                )
                                .w_full(),
                            ),
                    )
                    .into_any_element()
            },
            window,
            cx,
        )
    }

    pub(crate) fn render_server_permissions_page(
        &mut self,
        view: &AdminView,
        settings: &PersonSettings,
        page: &ServerPermissionsPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (server_id, account_id): (_, AccountId) = (view.server_id, settings.account_id);
        let what = AskKind::SetServerPermission(account_id);
        let owner = page.locked == Some(crate::state::admin::view::overrides::Locked::Owner);
        // The account of the owner takes no rule: its page lists no choice.
        let lines = if owner {
            Vec::new()
        } else {
            page.lines.loaded().cloned().unwrap_or_default()
        };
        let rows = std::iter::once(Row::new("person-header", true).part())
            .chain(lines.iter().map(|line| {
                Row::new(
                    format!("server-permission-{}", line.permission.name()),
                    true,
                )
            }))
            .collect();
        self.admin.list.take(Some(view), rows, window, cx);
        let view = view.clone();
        let page = page.clone();
        let locked = page.locked.map(text::admin::overrides::locked);
        let on_choose: OnChoose<ServerPermission> =
            Rc::new(move |this: &mut Shell, permission, choice, cx| {
                this.state.update(cx, |state, cx| {
                    state.set_server_permission(server_id, account_id, permission, choice, cx);
                });
            });
        self.render_admin_list(
            move |this, index, _, cx| {
                let theme = theme(cx);
                if index == 0 {
                    let mut column = div().flex().flex_col().child(page_title(
                        text::admin::overrides::SERVER_PERMISSIONS,
                        text::admin::overrides::SERVER_HELP,
                        &theme,
                    ));
                    if owner {
                        return column
                            .child(
                                sentence(
                                    text::admin::overrides::locked(
                                        crate::state::admin::view::overrides::Locked::Owner,
                                    ),
                                    &theme,
                                )
                                .mt(u(8.0))
                                .ink(theme.colors.ink)
                                .debug_selector(|| "person-owner-no-rules".to_owned()),
                            )
                            .into_any_element();
                    }
                    column = column
                        .children(Self::person_locked(locked, &theme))
                        .children(refusal(view.refused(what), "person-refused"));
                    let part = AdminPart::ServerPermissions(account_id);
                    if let Some(reading) = this.person_reading(&view, part, &page.lines, &theme, cx)
                    {
                        column = column.child(reading);
                    }
                    return column.into_any_element();
                }
                let held: Option<SharedString> = locked
                    .map(SharedString::from)
                    .or_else(|| view.sending(what).then(|| text::admin::SENDING.into()));
                this.render_rule_permission(
                    &lines[index - 1],
                    index - 1,
                    (Place::Server, held),
                    &on_choose,
                    &theme,
                    cx,
                )
            },
            window,
            cx,
        )
    }
}
