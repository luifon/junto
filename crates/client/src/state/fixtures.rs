//! Permission lists for the states that the tests and the screen tool build by hand
//! (ADR 0310). The window reads what the server stated; a state that is built without a
//! server needs the lists that a server would state for it.

use std::sync::Arc;

use junto_client_core::model::ServerModel;
use junto_client_core::protocol::roles::WorkspaceRole;
use junto_client_core::protocol::snapshot::Snapshot;
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

/// The list that a server states for a workspace without channel overrides: every
/// permission for the owner and an administrator, and otherwise the permissions of the
/// "everyone" role and of the held roles.
fn list(
    administrator: bool,
    roles: &[WorkspaceRole],
    held: &[junto_client_core::protocol::ids::WorkspaceRoleId],
) -> Vec<WorkspacePermission> {
    if administrator {
        return WorkspacePermission::ALL.to_vec();
    }
    WorkspacePermission::ALL
        .iter()
        .copied()
        .filter(|permission| {
            roles.iter().any(|role| {
                (role.is_everyone || held.contains(&role.id))
                    && role.permissions.contains(permission)
            })
        })
        .collect()
}

/// A snapshot with the list of each workspace that states none.
pub fn stated(mut snapshot: Snapshot) -> Snapshot {
    let administrator = snapshot.account.is_owner
        || snapshot
            .account
            .server_permissions
            .contains(&ServerPermission::Administrator);
    for workspace in &mut snapshot.workspaces {
        if workspace.permissions.is_empty() {
            workspace.permissions = list(administrator, &workspace.roles, &workspace.own_role_ids);
        }
    }
    snapshot
}

/// States the list of each workspace of a state again, after its roles or the account
/// were changed by hand.
pub fn restate(model: &mut ServerModel) {
    let administrator = model.is_administrator();
    for workspace in &mut model.workspaces {
        let next = list(administrator, &workspace.roles, &workspace.own_role_ids);
        if workspace.permissions != next {
            Arc::make_mut(workspace).permissions = next;
        }
    }
}
