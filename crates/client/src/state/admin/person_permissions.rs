//! The requests of the administration that change the workspace permissions of a member
//! and the server permissions of an account (ADR 0306). Each change names one permission
//! and its new state, so a change of another person to another permission is not undone.

use gpui::Context;
use junto_client_core::protocol::ids::{AccountId, ServerId, WorkspaceId};
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

use super::AskKind;
use super::view::overrides::{Choice, state_of};
use crate::state::AppState;

/// What two sets state about one permission. A permission in both sets counts as denied.
fn choice_in<P: PartialEq>(allow: &[P], deny: &[P], permission: &P) -> Choice {
    if deny.contains(permission) {
        Choice::Deny
    } else if allow.contains(permission) {
        Choice::Allow
    } else {
        Choice::Neutral
    }
}

impl AppState {
    /// Sets one workspace permission of a member to a choice. Nothing is sent while the
    /// permissions of the member are not loaded, and for the choice of now.
    pub fn set_member_permission(
        &mut self,
        server_id: ServerId,
        (workspace_id, account_id): (WorkspaceId, AccountId),
        permission: WorkspacePermission,
        choice: Choice,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model)
        else {
            return;
        };
        let Some(held) = model
            .admin
            .member_permissions(workspace_id, account_id)
            .loaded()
        else {
            return;
        };
        if choice_in(&held.allow, &held.deny, &permission) == choice {
            return;
        }
        let state = state_of(choice);
        self.admin_ask(
            server_id,
            AskKind::SetMemberPermission(workspace_id, account_id),
            |api| async move {
                api.change_member_permission((workspace_id, account_id), permission, state)
                    .await
            },
            |_, _, _| {},
            cx,
        );
    }

    /// Sets one server permission of an account to a choice. Nothing is sent while the
    /// permissions of the account are not loaded, and for the choice of now.
    pub fn set_server_permission(
        &mut self,
        server_id: ServerId,
        account_id: AccountId,
        permission: ServerPermission,
        choice: Choice,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model)
        else {
            return;
        };
        let Some(held) = model.admin.server_permissions(account_id).loaded() else {
            return;
        };
        if choice_in(&held.allow, &held.deny, &permission) == choice {
            return;
        }
        let state = state_of(choice);
        self.admin_ask(
            server_id,
            AskKind::SetServerPermission(account_id),
            |api| async move {
                api.change_server_permission(account_id, permission, state)
                    .await
            },
            |_, _, _| {},
            cx,
        );
    }
}
