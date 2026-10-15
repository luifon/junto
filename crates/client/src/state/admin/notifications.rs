//! The request of the administration that sets or removes the notification level of a
//! workspace (ADR 0412).

use gpui::Context;
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};

use super::AskKind;
use super::view::notifications::DefaultChoice;
use crate::state::AppState;

impl AppState {
    /// Sets the level that a workspace gives its members, or with "not set" removes it.
    /// One change of the level is on its way or waits for the state at a time: the page
    /// shows the chosen level until the state holds the events of the change (ADR 0214),
    /// so a choice is never built on a level that the server changed already.
    pub fn set_notification_default(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        choice: DefaultChoice,
        cx: &mut Context<Self>,
    ) {
        let level = match choice {
            DefaultChoice::Level(level) => Some(level),
            DefaultChoice::NotSet => None,
        };
        if !self.begin_default_change(server_id, workspace_id, level) {
            return;
        }
        let (told, outcome) = futures_channel::oneshot::channel();
        self.admin_ask(
            server_id,
            AskKind::NotificationDefault(workspace_id),
            move |api| async move {
                let changed = match choice {
                    DefaultChoice::Level(level) => api
                        .set_notification_default(workspace_id, level)
                        .await
                        .map(|set| set.receipt),
                    DefaultChoice::NotSet => api.remove_notification_default(workspace_id).await,
                };
                let _ = told.send(changed.as_ref().ok().copied());
                changed.map(|_| ())
            },
            |_, (), _| {},
            cx,
        );
        cx.spawn(async move |this, cx| {
            let receipt = outcome.await.ok().flatten();
            let _ = this.update(cx, |this, cx| {
                this.default_answered(server_id, workspace_id, receipt.ok_or(()), cx);
            });
        })
        .detach();
    }
}
