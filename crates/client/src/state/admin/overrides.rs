//! The requests of the administration that change the rules of the roles in a channel
//! (ADR 0304).
//!
//! A change of one permission is one request that names that permission and its new
//! state. The server changes it in the override that it holds and keeps every other
//! permission, so a change of another person to the same override is not undone, and the
//! grant rule is applied to the changed permission only.

use gpui::Context;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{ChannelId, ServerId, WorkspaceRoleId};
use junto_client_core::protocol::structure::{ChannelRules, RuleSubject, SetChannelOverride};

use super::AskKind;
use super::view::overrides::{Choice, choice_of, private_state, state_of};
use crate::state::AppState;

impl AppState {
    /// The overrides of a channel as the state of the server holds them. Absent until
    /// they are loaded.
    fn loaded_overrides(&self, server_id: ServerId, channel_id: ChannelId) -> Option<ChannelRules> {
        let model = self.client.server(server_id)?.live().view().model?;
        model.admin.rules(channel_id).loaded().cloned()
    }

    /// Sets one permission of one role in one channel to a choice, with one request that
    /// names that permission. The other permissions of the override stay as the server
    /// holds them. Nothing is sent while the overrides are not loaded, and for the choice
    /// that the override states already.
    pub fn set_override_choice(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        role_id: WorkspaceRoleId,
        permission: WorkspacePermission,
        choice: Choice,
        cx: &mut Context<Self>,
    ) {
        let Some(overrides) = self.loaded_overrides(server_id, channel_id) else {
            return;
        };
        if choice_of(overrides.of_role(role_id), permission) == choice {
            return;
        }
        let state = state_of(choice);
        self.admin_ask(
            server_id,
            AskKind::SetOverride(channel_id, role_id),
            |api| async move {
                let subject = RuleSubject::Role { role_id };
                api.change_channel_rule(channel_id, subject, permission, state)
                    .await
            },
            |_, _, _| {},
            cx,
        );
    }

    /// Removes the override of one role in one channel (two empty sets). Nothing is sent
    /// while the overrides are not loaded, and for a role without an override.
    pub fn clear_override(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        role_id: WorkspaceRoleId,
        cx: &mut Context<Self>,
    ) {
        let Some(overrides) = self.loaded_overrides(server_id, channel_id) else {
            return;
        };
        if overrides.of_role(role_id).is_none() {
            return;
        }
        self.admin_ask(
            server_id,
            AskKind::SetOverride(channel_id, role_id),
            |api| async move {
                let subject = RuleSubject::Role { role_id };
                api.set_channel_rule(channel_id, subject, &SetChannelOverride::default())
                    .await
            },
            |_, _, _| {},
            cx,
        );
    }

    /// Makes a channel private or open: denies "view channel" to the "everyone" role, or
    /// lets the roles decide it again (ADR 0304). The request names that one permission.
    /// Nothing is sent while the overrides are not loaded, and for a channel that is
    /// private or open already.
    pub fn set_private(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        private: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = self
            .client
            .server(server_id)
            .and_then(|server| server.live().view().model)
        else {
            return;
        };
        let Some(overrides) = model.admin.rules(channel_id).loaded() else {
            return;
        };
        let everyone = model
            .readable_channel(channel_id)
            .and_then(|channel| model.workspace(channel.workspace_id))
            .and_then(|workspace| workspace.roles.iter().find(|role| role.is_everyone));
        let Some(role_id) = everyone.map(|role| role.id) else {
            return;
        };
        let Some(state) = private_state(overrides.of_role(role_id), private) else {
            return;
        };
        self.admin_ask(
            server_id,
            AskKind::SetOverride(channel_id, role_id),
            |api| async move {
                api.change_channel_rule(
                    channel_id,
                    RuleSubject::Role { role_id },
                    WorkspacePermission::ViewChannel,
                    state,
                )
                .await
            },
            |_, _, _| {},
            cx,
        );
    }
}
