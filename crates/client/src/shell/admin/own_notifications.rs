//! The page "Notifications" of the account group (ADR 0742): the level of the server, the
//! level of each workspace, and the channels and group conversations with a setting of
//! their own, each with "Change…" and "Remove".

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    Window, div,
};
use junto_client_core::api::SettingsScope;
use junto_client_core::protocol::discovery::NotificationLevel;

use super::{group_label, line, line_text, named, page_title, sentence};
use crate::shell::Shell;
use crate::state::admin::AdminPage;
use crate::state::admin::view::AdminView;
use crate::state::admin::view::own_notifications::{OwnException, OwnNotificationsPage};
use crate::text;
use crate::theme::{Theme, theme, u};
use crate::ui::{Button, Notice, Segmented};

/// The width of the control of a level, in design pixels.
const LEVEL_WIDTH: f32 = 460.0;

const LEVELS: [NotificationLevel; 3] = [
    NotificationLevel::All,
    NotificationLevel::Mentions,
    NotificationLevel::Nothing,
];

fn scope_number(scope: SettingsScope) -> u64 {
    scope.id().as_u128() as u64
}

impl Shell {
    fn render_own_exception(
        &mut self,
        view: &AdminView,
        exception: &OwnException,
        busy: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let (server_id, scope) = (view.server_id, exception.scope);
        let detail: SharedString = match &exception.workspace {
            Some(workspace) => format!("{workspace} · {}", exception.state).into(),
            None => exception.state.clone().into(),
        };
        let number = scope_number(scope);
        let change = matches!(
            scope,
            SettingsScope::Channel(_) | SettingsScope::Conversation(_)
        )
        .then(|| {
            let id = scope.id();
            div()
                .flex_none()
                .debug_selector(move || format!("own-notify-change-{id}"))
                .child(
                    Button::new(("own-notify-change", number))
                        .label(text::CHANGE_ELLIPSIS)
                        .disabled(busy)
                        .h(u(28.0))
                        .px(u(10.0))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_notify_settings_from_admin(server_id, scope, window, cx);
                        })),
                )
        });
        let remove = Button::new(("own-notify-remove", number))
            .label(text::REMOVE_SETTING)
            .disabled(busy)
            .h(u(28.0))
            .px(u(10.0))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.state.update(cx, |state, cx| {
                    state.reset_notify_scope(server_id, scope, cx)
                });
            }));
        named(
            format!("own-notify-exception-{}", scope.id()),
            line(theme)
                .child(line_text(exception.name.clone(), Some(detail), theme))
                .children(change)
                .child(remove),
        )
        .flex_1()
        .into_any_element()
    }

    pub(crate) fn render_own_notifications_page(
        &mut self,
        view: &AdminView,
        page: &OwnNotificationsPage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let server_id = view.server_id;
        let busy = page.busy;
        let server_level = Segmented::new(
            "own-notify-server",
            text::NOTIFY_LEVEL,
            LEVELS.map(|level| SharedString::from(text::level_name(level))),
        )
        .selected(LEVELS.iter().position(|level| *level == page.server_level))
        .selector("own-notify-server")
        .on_select({
            let shown = page.server_level;
            let select = cx.listener(move |this, index: &usize, _, cx| {
                let Some(level) = LEVELS.get(*index).copied() else {
                    return;
                };
                if busy || level == shown {
                    return;
                }
                this.state.update(cx, |state, cx| {
                    state.set_notify_level(server_id, SettingsScope::Account, Some(level), cx);
                });
            });
            move |index, window, cx| select(&index, window, cx)
        });
        let mut column = div()
            .flex()
            .flex_col()
            .child(page_title(
                text::admin::page_name(AdminPage::OwnNotifications),
                text::OWN_NOTIFICATIONS_HELP,
                &theme,
            ))
            .child(group_label(text::THIS_SERVER, &theme))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .w(u(LEVEL_WIDTH))
                    .child(server_level),
            )
            .child(sentence(text::LEVEL_SERVER_HELP, &theme).mt(u(5.0)));

        if !page.workspaces.is_empty() {
            column = column.child(group_label(text::WORKSPACES, &theme));
        }
        for workspace in &page.workspaces {
            let workspace_id = workspace.workspace_id;
            let scope = SettingsScope::Workspace(workspace_id);
            let default = match workspace.level.inherited {
                Some(level) => text::level_default_named(level),
                None => text::LEVEL_DEFAULT.to_owned(),
            };
            let choices: Vec<Option<NotificationLevel>> =
                std::iter::once(None).chain(LEVELS.map(Some)).collect();
            let options: Vec<SharedString> = std::iter::once(SharedString::from(default))
                .chain(LEVELS.map(|level| SharedString::from(text::level_name(level))))
                .collect();
            let shown = workspace.level.stored;
            let control = Segmented::new(
                ("own-notify-workspace", scope_number(scope)),
                text::NOTIFY_LEVEL,
                options,
            )
            .selected(choices.iter().position(|choice| *choice == shown))
            .selector("own-notify-workspace")
            .on_select({
                let select = cx.listener(move |this, index: &usize, _, cx| {
                    let Some(level) = choices.get(*index).copied() else {
                        return;
                    };
                    if busy || level == shown {
                        return;
                    }
                    this.state.update(cx, |state, cx| {
                        state.set_notify_level(server_id, scope, level, cx);
                    });
                });
                move |index, window, cx| select(&index, window, cx)
            });
            column = column.child(named(
                format!("own-notify-workspace-{workspace_id}"),
                line(&theme)
                    .child(line_text(workspace.name.clone(), None, &theme))
                    .child(div().flex().flex_none().w(u(LEVEL_WIDTH)).child(control)),
            ));
        }

        column = column.child(group_label(text::OWN_EXCEPTIONS, &theme));
        if page.exceptions.is_empty() {
            column = column.child(
                sentence(text::NO_OWN_EXCEPTIONS, &theme)
                    .debug_selector(|| "own-notify-no-exceptions".to_owned()),
            );
        }
        for exception in &page.exceptions {
            column = column.child(self.render_own_exception(view, exception, busy, &theme, cx));
        }
        column
            .child(crate::ui::notice_slot("own-notify-refused-slot").children(
                page.refused.clone().map(|reason| {
                    div()
                        .mt(u(12.0))
                        .child(Notice::error(reason).selector("own-notify-refused"))
                }),
            ))
            .into_any_element()
    }
}
