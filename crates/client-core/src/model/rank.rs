//! Role order as the state of a server knows it (ADR 0309): where the account and another
//! account stand, decided by the function of `junto-authz` that the server uses. The
//! server decides every request; this is what a screen states before a person tries.

use std::cmp::Reverse;

use junto_authz::{ServerPermissions, role_rank};
use junto_protocol::ServerPermission;
use junto_protocol::account::AccountSummary;
use junto_protocol::ids::{ServerRoleId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::roles::{ServerRole, WorkspaceRole};

use super::ServerModel;

pub use junto_authz::{RoleRank, outranks};

/// The order of the roles of a workspace in the state: the "everyone" role first, then
/// from the highest position to the lowest.
pub(super) fn sort_workspace_roles(roles: &mut [WorkspaceRole]) {
    roles.sort_by_key(|held| (!held.is_everyone, Reverse(held.position), held.id));
}

/// The order of the server roles in the state: from the highest position to the lowest.
pub(super) fn sort_server_roles(roles: &mut [ServerRole]) {
    roles.sort_by_key(|held| (Reverse(held.position), held.id));
}

impl ServerModel {
    /// The positions and the permissions of the server roles `held`, when the state knows
    /// them. No role needs no list. `None` while the server roles are not loaded, and
    /// when the loaded list does not hold one of the roles: the list of the accounts can
    /// name a role before the list of the roles does, and a role that is not known is
    /// not taken as no role.
    fn server_roles_of(&self, held: &[ServerRoleId]) -> Option<Vec<(i32, ServerPermissions)>> {
        if held.is_empty() {
            return Some(Vec::new());
        }
        let roles = self.admin.server_roles.loaded()?;
        held.iter()
            .map(|role_id| {
                roles
                    .iter()
                    .find(|role| role.id == *role_id)
                    .map(|role| (role.position, role.permissions.iter().copied().collect()))
            })
            .collect()
    }

    /// Where the account stands for an action on the server: a removal, a ban, a change
    /// of a server role. `None` while the account holds server roles and their list is
    /// not loaded.
    pub fn server_rank(&self) -> Option<RoleRank> {
        if self.account.is_owner {
            return Some(RoleRank::Owner);
        }
        let held = self.server_roles_of(&self.account.server_role_ids)?;
        Some(role_rank(false, &held, None))
    }

    /// Where another account stands for an action on the server.
    pub fn server_rank_of(&self, account: &AccountSummary) -> Option<RoleRank> {
        if account.is_owner {
            return Some(RoleRank::Owner);
        }
        let held = self.server_roles_of(&account.server_role_ids)?;
        Some(role_rank(false, &held, None))
    }

    /// Where the account stands for an action on the roles of a workspace. An
    /// administrator stands above every workspace role, at the position of its highest
    /// role that gives the administrator permission; every other account stands at its
    /// highest role of the workspace. `None` for a workspace that the state does not
    /// hold, for an administrator while the server roles are not loaded, and when the
    /// account holds a role that the workspace does not list.
    pub fn workspace_rank(&self, workspace_id: WorkspaceId) -> Option<RoleRank> {
        if self.account.is_owner {
            return Some(RoleRank::Owner);
        }
        let workspace = self.workspace(workspace_id)?;
        let administrator = self
            .account
            .server_permissions
            .contains(&ServerPermission::Administrator);
        // A server role without the administrator permission gives no position in a
        // workspace, so an account that is no administrator needs no list of them.
        let server = if administrator {
            self.server_roles_of(&self.account.server_role_ids)?
        } else {
            Vec::new()
        };
        // A held role that the workspace does not list is not taken as no role.
        let positions: Vec<i32> = workspace
            .own_role_ids
            .iter()
            .map(|role_id| {
                workspace
                    .roles
                    .iter()
                    .find(|role| role.id == *role_id && !role.is_everyone)
                    .map(|role| role.position)
            })
            .collect::<Option<_>>()?;
        Some(role_rank(false, &server, Some(&positions)))
    }

    /// The server role with the identifier, while the server roles are loaded.
    pub fn server_role(&self, role_id: ServerRoleId) -> Option<&ServerRole> {
        self.admin
            .server_roles
            .loaded()?
            .iter()
            .find(|role| role.id == role_id)
    }

    /// The role of a workspace with the identifier.
    pub fn workspace_role(
        &self,
        workspace_id: WorkspaceId,
        role_id: WorkspaceRoleId,
    ) -> Option<&WorkspaceRole> {
        self.workspace(workspace_id)?
            .roles
            .iter()
            .find(|role| role.id == role_id)
    }
}
