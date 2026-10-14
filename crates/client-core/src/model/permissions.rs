//! The workspace permissions of the account as the server stated them (ADR 0310): one
//! list per workspace, and one list per channel in which the account holds another one.
//! The client compares no role and no override for them.

use junto_protocol::WorkspacePermission;
use junto_protocol::ids::ChannelId;

use super::{ServerModel, WorkspaceModel};

impl WorkspaceModel {
    /// True when the account holds the permission in the workspace.
    pub fn holds(&self, permission: WorkspacePermission) -> bool {
        self.permissions.contains(&permission)
    }

    /// The permissions of the account in a channel of the workspace: the list that the
    /// server stated for the channel, or the list of the workspace when it stated none.
    pub fn permissions_in(&self, channel_id: ChannelId) -> &[WorkspacePermission] {
        self.channel_permissions
            .iter()
            .find(|entry| entry.channel_id == channel_id)
            .map_or(&self.permissions, |entry| &entry.permissions)
    }

    /// True when the account holds the permission in a channel of the workspace.
    pub fn holds_in(&self, channel_id: ChannelId, permission: WorkspacePermission) -> bool {
        self.permissions_in(channel_id).contains(&permission)
    }

    pub(super) fn forget_channel_permissions(&mut self, channel_id: ChannelId) {
        self.channel_permissions
            .retain(|entry| entry.channel_id != channel_id);
    }
}

impl ServerModel {
    /// The permissions of the account in a channel that it reads. A channel of the
    /// channel lists has the list of the snapshot. An archived channel is in no snapshot
    /// (ADR 0410): its list is the collection that a reader opened for it, and `None`
    /// until that collection is loaded. `None` for a channel that the state does not hold.
    pub fn permissions_in(&self, channel_id: ChannelId) -> Option<&[WorkspacePermission]> {
        if let Some(workspace) = self
            .workspaces
            .iter()
            .find(|workspace| workspace.channel(channel_id).is_some())
        {
            return Some(workspace.permissions_in(channel_id));
        }
        self.admin.archived_channel(channel_id)?;
        self.admin
            .channel_permissions(channel_id)
            .loaded()
            .map(Vec::as_slice)
    }

    /// True when the account holds the permission in a channel that it reads, as far as
    /// the state knows.
    pub fn holds_in(&self, channel_id: ChannelId, permission: WorkspacePermission) -> bool {
        self.permissions_in(channel_id)
            .is_some_and(|held| held.contains(&permission))
    }
}
