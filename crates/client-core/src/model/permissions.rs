//! The workspace permissions of the account as the server stated them (ADR 0310): one
//! list per workspace, and one list per channel in which the account holds another one.
//! The client compares no role and no override for them. The server states no list for a
//! category: a page that changes the rules of a category evaluates them with the function
//! of `junto-authz` that the server uses (ADR 0790).

use junto_authz::{
    ChannelScope, HeldRole, Membership, Override, RoleKey, RoleOverride, WorkspacePermissions,
    WorkspaceScope,
};
use junto_protocol::WorkspacePermission;
use junto_protocol::ids::{ChannelId, WorkspaceId};
use junto_protocol::structure::{PermissionRule, RuleSubject};

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

    /// The permissions of the account in a place with these rules, such as a category:
    /// the list of the workspace that the server stated, then the rule of the "everyone"
    /// role, the rules of the roles of the account and the rule of the account, in the
    /// order of the permission function (ADR 0301). Nothing without "view channel", and no
    /// sound, camera or share without "connect". Every permission for an administrator.
    /// `None` for a workspace that the state does not hold.
    pub fn permissions_with_rules(
        &self,
        workspace_id: WorkspaceId,
        rules: &[PermissionRule],
    ) -> Option<Vec<WorkspacePermission>> {
        let workspace = self.workspace(workspace_id)?;
        if self.is_administrator() {
            return Some(WorkspacePermission::ALL.to_vec());
        }
        let key =
            |role_id: junto_protocol::ids::WorkspaceRoleId| RoleKey(role_id.as_uuid().as_u128());
        // A permission in both sets, which the server does not store, counts as denied.
        let rule_of = |rule: &PermissionRule| {
            let deny: WorkspacePermissions = rule.deny.iter().copied().collect();
            let allow: WorkspacePermissions = rule.allow.iter().copied().collect();
            Override::new(allow.difference(deny), deny).unwrap_or(Override::NONE)
        };
        let everyone = workspace.roles.iter().find(|role| role.is_everyone);
        let mut scope = ChannelScope {
            workspace: WorkspaceScope {
                membership: Membership::Member,
                everyone: workspace.permissions.iter().copied().collect(),
                roles: workspace
                    .own_role_ids
                    .iter()
                    .map(|role_id| HeldRole {
                        key: key(*role_id),
                        permissions: WorkspacePermissions::EMPTY,
                    })
                    .collect(),
                person_override: Override::NONE,
            },
            everyone_override: Override::NONE,
            role_overrides: Vec::new(),
            person_override: Override::NONE,
        };
        for rule in rules {
            match rule.subject {
                RuleSubject::Role { role_id }
                    if everyone.is_some_and(|held| held.id == role_id) =>
                {
                    scope.everyone_override = rule_of(rule);
                }
                RuleSubject::Role { role_id } => scope.role_overrides.push(RoleOverride {
                    role: key(role_id),
                    rule: rule_of(rule),
                }),
                RuleSubject::Person { account_id } if account_id == self.account.id => {
                    scope.person_override = rule_of(rule);
                }
                RuleSubject::Person { .. } => {}
            }
        }
        Some(scope.usable_permissions().iter().collect())
    }

    /// True when the account holds the permission in a channel that it reads, as far as
    /// the state knows.
    pub fn holds_in(&self, channel_id: ChannelId, permission: WorkspacePermission) -> bool {
        self.permissions_in(channel_id)
            .is_some_and(|held| held.contains(&permission))
    }
}
