//! The page "Notifications" of a workspace (ADR 0761): the notification level that the
//! workspace sets for its members, with "not set" (ADR 0412).

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    Window, div,
};

use super::{group_label, page_title, refusal, sentence};
use crate::shell::Shell;
use crate::state::admin::view::AdminView;
use crate::state::admin::view::notifications::{DefaultChoice, NotificationsPage};
use crate::state::admin::{AdminPage, AskKind};
use crate::text;
use crate::text::admin::notifications as txt;
use crate::theme::{theme, u};
use crate::ui::{Segmented, StyleExt};

/// The width of the control of the level, in design pixels.
const LEVEL_WIDTH: f32 = 520.0;

impl Shell {
    pub(crate) fn render_notifications_page(
        &mut self,
        view: &AdminView,
        page: &NotificationsPage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = theme(cx);
        let (server_id, workspace_id) = (view.server_id, page.workspace_id);
        let what = AskKind::NotificationDefault(workspace_id);
        let sending = view.sending(what);
        let mut level = Segmented::new(
            "notification-default",
            txt::LEVEL,
            DefaultChoice::ALL
                .iter()
                .map(|choice| SharedString::from(txt::choice(*choice))),
        )
        .selected(
            DefaultChoice::ALL
                .iter()
                .position(|choice| *choice == page.choice),
        )
        .selector("notification-default");
        // One request at a time. Every choice is sent, also the one that the control
        // shows: the server does nothing for the level that it holds.
        if !sending {
            level = level.on_select({
                let select = cx.listener(move |this, index: &usize, _, cx| {
                    if let Some(choice) = DefaultChoice::ALL.get(*index).copied() {
                        this.state.update(cx, |state, cx| {
                            state.set_notification_default(server_id, workspace_id, choice, cx);
                        });
                    }
                });
                move |index, window, cx| select(&index, window, cx)
            });
        }
        let mut column = div()
            .flex()
            .flex_col()
            .child(page_title(
                text::admin::page_name(AdminPage::Notifications),
                txt::help(&page.workspace_name),
                &theme,
            ))
            .child(group_label(txt::LEVEL, &theme))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .w(u(LEVEL_WIDTH))
                    .debug_selector(|| "notification-default".to_owned())
                    .child(level),
            )
            .child(
                sentence(txt::stated(page.choice), &theme)
                    .mt(u(8.0))
                    .ink(theme.colors.ink)
                    .debug_selector(|| "notification-default-stated".to_owned()),
            )
            .child(sentence(txt::HOLDS_FOR, &theme).mt(u(4.0)));
        if sending {
            column = column.child(
                sentence(text::admin::SENDING, &theme)
                    .mt(u(6.0))
                    .debug_selector(|| "notification-default-sending".to_owned()),
            );
        }
        column
            .children(refusal(view.refused(what), "notification-default-refused"))
            .into_any_element()
    }
}
