//! The routes of role order (ADR 0309) and of the permissions of the account in one
//! channel (ADR 0310).

use junto_protocol::WorkspacePermission;
use junto_protocol::ids::{ChannelId, WorkspaceId};
use junto_protocol::roles::{
    ReorderServerRoles, ReorderWorkspaceRoles, ServerRolePosition, WorkspaceRolePosition,
};
use junto_protocol::structure::ChannelPermissions;

use super::client::encode;
use super::error::ApiError;
use super::session::ServerApi;
use super::transport::Method;

impl ServerApi {
    /// Gives the named server roles new positions in one change. The roles exchange the
    /// positions that they hold among themselves: a position of a role that the request
    /// does not name is refused. Each role and each new position must be below the
    /// highest role of the caller.
    pub async fn reorder_server_roles(
        &self,
        positions: Vec<ServerRolePosition>,
    ) -> Result<(), ApiError> {
        let body = Some(encode(&ReorderServerRoles { positions })?);
        self.call(
            Method::Patch,
            "/server-roles/positions".to_owned(),
            Vec::new(),
            body,
        )
        .await
        .map(|_| ())
    }

    /// Gives the named roles of a workspace new positions in one change, under the rules
    /// of [`Self::reorder_server_roles`]. The "everyone" role does not move.
    pub async fn reorder_workspace_roles(
        &self,
        workspace_id: WorkspaceId,
        positions: Vec<WorkspaceRolePosition>,
    ) -> Result<(), ApiError> {
        let body = Some(encode(&ReorderWorkspaceRoles { positions })?);
        let path = format!("/workspaces/{workspace_id}/roles/positions");
        self.call(Method::Patch, path, Vec::new(), body)
            .await
            .map(|_| ())
    }

    /// The workspace permissions that the caller holds in one channel that it can see. An
    /// archived channel is in no snapshot, and its list is read here. An answer that
    /// names another channel is refused (ADR 0714).
    pub async fn channel_permissions(
        &self,
        channel_id: ChannelId,
    ) -> Result<Vec<WorkspacePermission>, ApiError> {
        let held: ChannelPermissions = self
            .get(format!("/channels/{channel_id}/permissions"))
            .await?;
        if held.channel_id != channel_id {
            return Err(ApiError::InvalidResponse {
                status: 200,
                reason: "the answer names another channel",
            });
        }
        Ok(held.permissions)
    }
}
