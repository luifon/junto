use crate::{ServerPermission, ServerPermissions};

/// The highest role relevant to an action. Workspace ranks are comparable only when
/// both belong to the workspace of that action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RoleRank {
    Workspace(i32),
    Server(i32),
    Owner,
}

/// Role order is a separate limit: administrator permissions do not bypass it.
pub fn outranks(actor: RoleRank, target: RoleRank) -> bool {
    actor > target
}

/// Derives the position for one action scope; unrelated server permissions never
/// supply a workspace position. `None` means a server action.
pub fn role_rank(
    owner: bool,
    server: &[(i32, ServerPermissions)],
    workspace: Option<&[i32]>,
) -> RoleRank {
    if owner {
        return RoleRank::Owner;
    }
    let server_position = server
        .iter()
        .filter(|(_, permissions)| {
            workspace.is_none() || permissions.contains(ServerPermission::Administrator)
        })
        .map(|(position, _)| *position)
        .max();
    if let Some(position) = server_position {
        return RoleRank::Server(position);
    }
    match workspace {
        None => RoleRank::Server(0),
        Some(positions) => RoleRank::Workspace(positions.iter().copied().max().unwrap_or(0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_order_comparison() {
        use RoleRank::{Owner, Server, Workspace};
        for (actor, target, allowed) in [
            (Server(2), Server(1), true),
            (Server(1), Server(2), false),
            (Server(1), Server(1), false),
            (Server(0), Server(0), false),
            (Workspace(2), Workspace(1), true),
            (Workspace(1), Workspace(2), false),
            (Workspace(1), Workspace(1), false),
            (Workspace(0), Workspace(0), false),
            (Server(1), Workspace(i32::MAX), true),
            (Workspace(i32::MAX), Server(1), false),
            (Owner, Server(i32::MAX), true),
            (Owner, Workspace(i32::MAX), true),
            (Server(i32::MAX), Owner, false),
            (Owner, Owner, false),
        ] {
            assert_eq!(outranks(actor, target), allowed, "{actor:?} -> {target:?}");
        }
    }
    #[test]
    fn workspace_priority_uses_only_administrator_granting_roles() {
        let ordinary = ServerPermissions::from(ServerPermission::CreateInvites);
        let admin = ServerPermissions::from(ServerPermission::Administrator);
        assert_eq!(
            role_rank(false, &[(99, ordinary)], Some(&[3, 7])),
            RoleRank::Workspace(7)
        );
        assert_eq!(
            role_rank(false, &[(99, ordinary)], None),
            RoleRank::Server(99)
        );
        assert_eq!(
            role_rank(
                false,
                &[(99, ordinary), (5, admin), (2, admin)],
                Some(&[100])
            ),
            RoleRank::Server(5)
        );
        assert_eq!(role_rank(false, &[], Some(&[])), RoleRank::Workspace(0));
        assert_eq!(role_rank(false, &[], None), RoleRank::Server(0));
        assert_eq!(role_rank(true, &[], Some(&[])), RoleRank::Owner);
        assert!(!outranks(
            role_rank(false, &[(99, ordinary), (5, admin)], Some(&[])),
            role_rank(false, &[(8, admin)], Some(&[]))
        ));
    }
}
