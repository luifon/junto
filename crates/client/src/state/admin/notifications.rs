//! The request of the administration that sets or removes the notification level of a
//! workspace (ADR 0412).

use gpui::Context;
use junto_client_core::protocol::ids::{ServerId, WorkspaceId};

use super::view::notifications::DefaultChoice;
use super::{Ask, AskKind, AskState};
use crate::state::AppState;

/// True when a choice of the level of a workspace is sent now: no request of that level
/// is on its way. The level that the state holds does not decide: the answer of a request
/// can arrive before the event of its change, the control shows the level of before for
/// that time, and a choice of that level is a change then. The server does nothing for
/// the level that it holds already.
pub(crate) fn takes_choice(ask: Option<&Ask>, workspace_id: WorkspaceId) -> bool {
    !ask.is_some_and(|ask| {
        ask.what == AskKind::NotificationDefault(workspace_id) && ask.state == AskState::Sending
    })
}

impl AppState {
    /// Sets the level that a workspace gives its members, or with "not set" removes it.
    /// One request is on its way at a time. The change arrives with the event
    /// `notification_default_changed`.
    pub fn set_notification_default(
        &mut self,
        server_id: ServerId,
        workspace_id: WorkspaceId,
        choice: DefaultChoice,
        cx: &mut Context<Self>,
    ) {
        if !takes_choice(self.admin.ask.as_ref(), workspace_id) {
            return;
        }
        self.admin_ask(
            server_id,
            AskKind::NotificationDefault(workspace_id),
            |api| async move {
                match choice {
                    DefaultChoice::Level(level) => api
                        .set_notification_default(workspace_id, level)
                        .await
                        .map(|_| ()),
                    DefaultChoice::NotSet => api.remove_notification_default(workspace_id).await,
                }
            },
            |_, (), _| {},
            cx,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_choice_is_sent_unless_a_request_of_the_level_of_the_workspace_is_on_its_way() {
        let workspace_id = WorkspaceId::generate();
        let what = AskKind::NotificationDefault(workspace_id);
        // Nothing is on its way: every choice is sent, also the one that the control
        // shows, which can be the level of before a change that the server made.
        assert!(takes_choice(None, workspace_id));
        let refused = Ask {
            what,
            state: AskState::Refused("No.".to_owned()),
        };
        assert!(takes_choice(Some(&refused), workspace_id));
        // A request is on its way: a second one could reach the server before it.
        let sending = Ask {
            what,
            state: AskState::Sending,
        };
        assert!(!takes_choice(Some(&sending), workspace_id));
        // The request of another workspace is not one of this workspace.
        assert!(takes_choice(Some(&sending), WorkspaceId::generate()));
    }
}
