//! The requests of the administration that change the overrides of the roles in a channel
//! (ADR 0304).
//!
//! The server stores an override as two whole sets, so a change of one permission sends
//! every permission of the override. The sets are made from the override that the server
//! holds at the time of the request: the state of the window can be older than an answer
//! of the server, and sets made from it would undo the change before.

use gpui::Context;
use junto_client_core::api::{ApiError, ServerApi};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{ChannelId, ServerId, WorkspaceRoleId};
use junto_client_core::protocol::structure::{ChannelOverride, SetChannelOverride};

use super::AskKind;
use super::view::overrides::{Choice, changed, choice_of, private_change};
use crate::state::AppState;

/// The override of a role in a channel as the server holds it now.
async fn stored_override(
    api: &ServerApi,
    channel_id: ChannelId,
    role_id: WorkspaceRoleId,
) -> Result<Option<ChannelOverride>, ApiError> {
    let stored = api.channel_overrides(channel_id).await?;
    Ok(stored.into_iter().find(|held| held.role_id == role_id))
}

async fn set_choice(
    api: ServerApi,
    channel_id: ChannelId,
    role_id: WorkspaceRoleId,
    permission: WorkspacePermission,
    choice: Choice,
) -> Result<ChannelOverride, ApiError> {
    let stored = stored_override(&api, channel_id, role_id).await?;
    let (allow, deny) = changed(stored.as_ref(), permission, choice);
    api.set_channel_override(channel_id, role_id, &SetChannelOverride { allow, deny })
        .await
}

async fn set_privacy(
    api: ServerApi,
    channel_id: ChannelId,
    role_id: WorkspaceRoleId,
    private: bool,
) -> Result<(), ApiError> {
    let stored = stored_override(&api, channel_id, role_id).await?;
    if let Some((allow, deny)) = private_change(stored.as_ref(), private) {
        api.set_channel_override(channel_id, role_id, &SetChannelOverride { allow, deny })
            .await?;
    }
    Ok(())
}

impl AppState {
    /// The overrides of a channel as the state of the server holds them. Absent until
    /// they are loaded.
    fn loaded_overrides(
        &self,
        server_id: ServerId,
        channel_id: ChannelId,
    ) -> Option<Vec<ChannelOverride>> {
        let model = self.client.server(server_id)?.live().view().model?;
        model.admin.overrides(channel_id).loaded().cloned()
    }

    /// Sets one permission of one role in one channel to a choice. The other permissions
    /// of the override stay. Nothing is sent while the overrides are not loaded, and for
    /// the choice that the override states already.
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
        let shown = overrides.iter().find(|held| held.role_id == role_id);
        if choice_of(shown, permission) == choice {
            return;
        }
        self.admin_ask(
            server_id,
            AskKind::SetOverride(channel_id, role_id),
            |api| set_choice(api, channel_id, role_id, permission, choice),
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
        if !overrides.iter().any(|held| held.role_id == role_id) {
            return;
        }
        self.admin_ask(
            server_id,
            AskKind::SetOverride(channel_id, role_id),
            |api| async move {
                api.set_channel_override(channel_id, role_id, &SetChannelOverride::default())
                    .await
            },
            |_, _, _| {},
            cx,
        );
    }

    /// Makes a channel private or open: adds or removes "view channel" in the deny set of
    /// the "everyone" role (ADR 0304). Nothing is sent while the overrides are not
    /// loaded, and for a channel that is private or open already.
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
        let Some(overrides) = model.admin.overrides(channel_id).loaded() else {
            return;
        };
        let everyone = model
            .readable_channel(channel_id)
            .and_then(|channel| model.workspace(channel.workspace_id))
            .and_then(|workspace| workspace.roles.iter().find(|role| role.is_everyone));
        let Some(role_id) = everyone.map(|role| role.id) else {
            return;
        };
        let shown = overrides.iter().find(|held| held.role_id == role_id);
        if private_change(shown, private).is_none() {
            return;
        }
        self.admin_ask(
            server_id,
            AskKind::SetOverride(channel_id, role_id),
            |api| set_privacy(api, channel_id, role_id, private),
            |_, (), _| {},
            cx,
        );
    }
}
