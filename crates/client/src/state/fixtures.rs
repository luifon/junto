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

/// A snapshot whose workspaces state the lists that a server gives for their roles and for
/// the account, as after a change of the roles. Every list is replaced, an empty one as
/// well, so an empty list is never taken for a missing one. A list that the snapshot holds
/// already must be the list of the roles: a test that states a list of its own sets it
/// after this call, or builds its scene with `lists_by_hand`.
pub fn from_roles(mut snapshot: Snapshot) -> Snapshot {
    let administrator = snapshot.account.is_owner
        || snapshot
            .account
            .server_permissions
            .contains(&ServerPermission::Administrator);
    for workspace in &mut snapshot.workspaces {
        let stated = list(administrator, &workspace.roles, &workspace.own_role_ids);
        assert!(
            workspace.permissions.is_empty() || workspace.permissions == stated,
            "the workspace {} states a list of its own: set it after from_roles or use \
             lists_by_hand",
            workspace.workspace.name
        );
        workspace.permissions = stated;
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

/// The state of a server without a workspace, whose stream stands at `position`. Tests of
/// the changes that wait for their events compare receipts with it (ADR 0214).
pub fn model_at(position: u64) -> ServerModel {
    use junto_client_core::protocol::account::OwnAccount;
    use junto_client_core::protocol::events::Cursor;
    use junto_client_core::protocol::ids::{AccountId, StreamEpoch};
    // One epoch for every state of a run, so that two states compare by position.
    static EPOCH: std::sync::LazyLock<StreamEpoch> =
        std::sync::LazyLock::new(StreamEpoch::generate);
    ServerModel::from_snapshot(Snapshot {
        cursor: Cursor {
            epoch: *EPOCH,
            position,
        },
        account: OwnAccount {
            id: AccountId::generate(),
            name: "Luca".to_owned(),
            avatar: None,
            is_owner: false,
            server_role_ids: Vec::new(),
            server_permissions: Vec::new(),
            workspace_ids: Vec::new(),
        },
        workspaces: Vec::new(),
        own_call: None,
        direct_calls: Vec::new(),
        call_invitations: Vec::new(),
        sent_call_invitations: Vec::new(),
        call_moves: Vec::new(),
        entry_requests: Vec::new(),
        direct_mutes: Vec::new(),
        messages: Vec::new(),
        threads: Vec::new(),
        threads_more: false,
        threads_next: None,
        notifications: Vec::new(),
        notification_settings: Vec::new(),
    })
}
