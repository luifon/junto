#![allow(clippy::unwrap_used, clippy::expect_used)]

use junto_authz::{
    Actor, Admission, AuditScope, Authorship, ChannelContent, ChannelGrant, ChannelScope, Decision,
    Denial, HeldRole, Membership, MessagePlace, Override, OwnerAction, Participation, Permanence,
    Position, Reach, Request, RoleKey, RoleOverride, ServerPermission, ServerPermissions, Target,
    WorkspacePermission, WorkspacePermissions, WorkspaceScope, authorize,
};
use proptest::prelude::*;
use proptest::sample::{Index, select};

fn workspace_permissions() -> impl Strategy<Value = WorkspacePermissions> {
    (0u64..=WorkspacePermissions::ALL.bits())
        .prop_map(|bits| WorkspacePermissions::from_bits(bits).unwrap())
}

fn server_permissions() -> impl Strategy<Value = ServerPermissions> {
    (0u64..=ServerPermissions::ALL.bits())
        .prop_map(|bits| ServerPermissions::from_bits(bits).unwrap())
}

fn workspace_permission() -> impl Strategy<Value = WorkspacePermission> {
    select(WorkspacePermission::ALL)
}

fn server_permission() -> impl Strategy<Value = ServerPermission> {
    select(ServerPermission::ALL)
}

fn any_override() -> impl Strategy<Value = Override> {
    (workspace_permissions(), workspace_permissions())
        .prop_map(|(allow, deny)| Override::new(allow, deny.difference(allow)).unwrap())
}

fn allow_only_override() -> impl Strategy<Value = Override> {
    workspace_permissions()
        .prop_map(|allow| Override::new(allow, WorkspacePermissions::EMPTY).unwrap())
}

fn membership() -> impl Strategy<Value = Membership> {
    prop_oneof![Just(Membership::Member), Just(Membership::NotMember)]
}

fn position() -> impl Strategy<Value = Position> {
    prop_oneof![Just(Position::Owner), Just(Position::Member)]
}

fn actor() -> impl Strategy<Value = Actor> {
    (
        position(),
        prop::collection::vec(server_permissions(), 0..4),
    )
        .prop_map(|(position, server_roles)| Actor {
            server_override: Default::default(),
            position,
            server_roles,
        })
}

fn without_administrator(mut actor: Actor) -> Actor {
    actor.position = Position::Member;
    for role in &mut actor.server_roles {
        *role = role.difference(ServerPermission::Administrator.into());
    }
    actor
}

fn non_administrator() -> impl Strategy<Value = Actor> {
    actor().prop_map(without_administrator)
}

fn administrator_not_owner() -> impl Strategy<Value = Actor> {
    actor().prop_map(|mut actor| {
        actor.position = Position::Member;
        actor
            .server_roles
            .push(ServerPermission::Administrator.into());
        actor
    })
}

fn channel_scope() -> impl Strategy<Value = ChannelScope> {
    (
        membership(),
        workspace_permissions(),
        prop::collection::vec(workspace_permissions(), 0..5),
        any_override(),
        prop::collection::vec((0u128..8, any_override()), 0..8),
    )
        .prop_map(
            |(membership, everyone, role_permissions, everyone_override, overrides)| {
                let roles = match membership {
                    Membership::Member => role_permissions
                        .into_iter()
                        .enumerate()
                        .map(|(index, permissions)| HeldRole {
                            key: RoleKey(index as u128),
                            permissions,
                        })
                        .collect(),
                    Membership::NotMember => Vec::new(),
                };
                let mut role_overrides: Vec<RoleOverride> = Vec::new();
                for (role, rule) in overrides {
                    if role_overrides
                        .iter()
                        .all(|existing| existing.role.0 != role)
                    {
                        role_overrides.push(RoleOverride {
                            role: RoleKey(role),
                            rule,
                        });
                    }
                }
                ChannelScope {
                    person_override: Default::default(),
                    workspace: WorkspaceScope {
                        person_override: Default::default(),
                        membership,
                        everyone,
                        roles,
                    },
                    everyone_override,
                    role_overrides,
                }
            },
        )
}

fn member_channel_scope() -> impl Strategy<Value = ChannelScope> {
    channel_scope().prop_map(|mut scope| {
        scope.workspace.membership = Membership::Member;
        scope
    })
}

fn shuffled<T: Clone>(items: &[T], picks: &[Index]) -> Vec<T> {
    let mut remaining = items.to_vec();
    let mut result = Vec::with_capacity(remaining.len());
    for pick in picks {
        if remaining.is_empty() {
            break;
        }
        result.push(remaining.remove(pick.index(remaining.len())));
    }
    result.append(&mut remaining);
    result
}

fn reference_effective(scope: &ChannelScope) -> WorkspacePermissions {
    WorkspacePermission::ALL
        .iter()
        .copied()
        .filter(|permission| {
            let held_overrides: Vec<Override> = scope
                .role_overrides
                .iter()
                .filter(|rule| {
                    scope
                        .workspace
                        .roles
                        .iter()
                        .any(|role| role.key == rule.role)
                })
                .map(|rule| rule.rule)
                .collect();
            if held_overrides
                .iter()
                .any(|rule| rule.allow().contains(*permission))
            {
                return true;
            }
            if held_overrides
                .iter()
                .any(|rule| rule.deny().contains(*permission))
            {
                return false;
            }
            if scope.everyone_override.allow().contains(*permission) {
                return true;
            }
            if scope.everyone_override.deny().contains(*permission) {
                return false;
            }
            scope.workspace.everyone.contains(*permission)
                || scope
                    .workspace
                    .roles
                    .iter()
                    .any(|role| role.permissions.contains(*permission))
        })
        .collect()
}

fn requests_under_limits<'a>(scope: &'a ChannelScope) -> Vec<Request<'a>> {
    let mut requests = vec![
        Request::DirectMessage(Participation::Outsider),
        Request::OwnerOnly(OwnerAction::RecoverAccount),
        Request::OwnerOnly(OwnerAction::AssignOwner),
        Request::DeleteChannel {
            scope,
            content: ChannelContent::PermanentMessages,
        },
        Request::Moderate {
            target: Target::Owner,
        },
        Request::Moderate {
            target: Target::Actor,
        },
    ];
    for place in [
        MessagePlace::Channel(scope),
        MessagePlace::DirectMessage(Participation::Participant),
        MessagePlace::DirectMessage(Participation::Outsider),
    ] {
        for authorship in [Authorship::Author, Authorship::Other] {
            requests.push(Request::EditMessage {
                place,
                permanence: Permanence::Permanent,
                authorship,
            });
        }
        requests.push(Request::EditMessage {
            place,
            permanence: Permanence::Editable,
            authorship: Authorship::Other,
        });
    }
    requests
}

fn requests_for_roles<'a>(
    scope: &'a ChannelScope,
    permission: WorkspacePermission,
    server: ServerPermission,
    granting: WorkspacePermissions,
) -> Vec<Request<'a>> {
    vec![
        Request::Server(server),
        Request::ManageServerRoles {
            granting: server.into(),
        },
        Request::Admit {
            admission: Admission::CreateInvite,
            reach: Reach::WithinMemberships,
        },
        Request::Admit {
            admission: Admission::ApproveJoinRequest,
            reach: Reach::BeyondMemberships,
        },
        Request::Moderate {
            target: Target::Other,
        },
        Request::ViewWorkspace(&scope.workspace),
        Request::Workspace {
            scope: &scope.workspace,
            permission,
        },
        Request::ManageWorkspaceRoles {
            scope: &scope.workspace,
            granting,
            in_channels: &[],
        },
        Request::Channel { scope, permission },
        Request::ManageChannelOverrides { scope, granting },
        Request::DeleteChannel {
            scope,
            content: ChannelContent::NoPermanentMessages,
        },
        Request::EditMessage {
            place: MessagePlace::Channel(scope),
            permanence: Permanence::Editable,
            authorship: Authorship::Author,
        },
        Request::AuditEntry(AuditScope::Server),
        Request::AuditEntry(AuditScope::Workspace(&scope.workspace)),
        Request::AuditEntry(AuditScope::Channel(scope)),
        Request::AuditEntry(AuditScope::DeletedChannel),
    ]
}

proptest! {
    #[test]
    fn effective_permissions_match_the_rule_stated_per_permission(scope in member_channel_scope()) {
        prop_assert_eq!(scope.effective_permissions(), reference_effective(&scope));
    }

    #[test]
    fn evaluation_is_independent_of_role_order(
        actor in actor(),
        scope in channel_scope(),
        permission in workspace_permission(),
        server in server_permission(),
        granting in workspace_permissions(),
        picks in prop::collection::vec(any::<Index>(), 0..8),
    ) {
        let mut reordered = scope.clone();
        reordered.workspace.roles = shuffled(&scope.workspace.roles, &picks);
        reordered.role_overrides = shuffled(&scope.role_overrides, &picks);
        let mut reordered_actor = actor.clone();
        reordered_actor.server_roles = shuffled(&actor.server_roles, &picks);

        let original = requests_for_roles(&scope, permission, server, granting);
        let changed = requests_for_roles(&reordered, permission, server, granting);
        for (before, after) in original.iter().zip(changed.iter()) {
            prop_assert_eq!(authorize(&actor, before), authorize(&reordered_actor, after));
        }
    }

    #[test]
    fn a_role_that_only_allows_never_removes_a_permission(
        actor in non_administrator(),
        scope in member_channel_scope(),
        added_permissions in workspace_permissions(),
        added_override in allow_only_override(),
        permission in workspace_permission(),
    ) {
        let added_key = RoleKey(1_000);
        let mut extended = scope.clone();
        extended.workspace.roles.push(HeldRole { key: added_key, permissions: added_permissions });
        extended.role_overrides.push(RoleOverride { role: added_key, rule: added_override });

        prop_assert!(extended.effective_permissions().contains_all(scope.effective_permissions()));
        prop_assert!(
            extended.workspace.base_permissions().contains_all(scope.workspace.base_permissions())
        );

        let before = requests_for_roles(&scope, permission, ServerPermission::BanMembers, WorkspacePermissions::EMPTY);
        let after = requests_for_roles(&extended, permission, ServerPermission::BanMembers, WorkspacePermissions::EMPTY);
        for (before, after) in before.iter().zip(after.iter()) {
            if authorize(&actor, before).is_allowed() {
                prop_assert!(authorize(&actor, after).is_allowed());
            }
        }
    }

    #[test]
    fn an_allow_override_of_a_held_role_wins_over_every_deny(
        scope in member_channel_scope(),
        permission in workspace_permission(),
    ) {
        let allowed = scope.role_overrides.iter().any(|rule| {
            rule.rule.allow().contains(permission)
                && scope.workspace.roles.iter().any(|role| role.key == rule.role)
        });
        if allowed {
            prop_assert!(scope.effective_permissions().contains(permission));
        }
    }

    #[test]
    fn a_server_role_never_removes_a_server_permission(
        actor in non_administrator(),
        added in server_permissions(),
        permission in server_permission(),
    ) {
        let mut extended = actor.clone();
        extended.server_roles.push(added);
        if authorize(&actor, &Request::Server(permission)).is_allowed() {
            prop_assert!(authorize(&extended, &Request::Server(permission)).is_allowed());
        }
    }

    #[test]
    fn an_administrator_is_never_allowed_an_owner_only_action(
        actor in administrator_not_owner(),
        scope in channel_scope(),
    ) {
        for request in [
            Request::OwnerOnly(OwnerAction::RecoverAccount),
            Request::OwnerOnly(OwnerAction::AssignOwner),
            Request::DeleteChannel { scope: &scope, content: ChannelContent::PermanentMessages },
        ] {
            prop_assert_eq!(authorize(&actor, &request), Decision::Deny(Denial::OwnerOnly));
        }
    }

    #[test]
    fn the_limits_bind_every_actor_but_the_owner_position(
        actor in actor(),
        scope in channel_scope(),
    ) {
        for request in requests_under_limits(&scope) {
            let owner_action = matches!(
                request,
                Request::OwnerOnly(_) | Request::DeleteChannel { .. }
            );
            let decision = authorize(&actor, &request);
            if owner_action && actor.position == Position::Owner {
                prop_assert_eq!(decision, Decision::Allow);
            } else {
                prop_assert!(!decision.is_allowed(), "{:?} {:?}", actor, request);
            }
        }
    }

    #[test]
    fn the_administrator_permission_passes_every_role_check(
        actor in administrator_not_owner(),
        scope in channel_scope(),
        permission in workspace_permission(),
        server in server_permission(),
        granting in workspace_permissions(),
    ) {
        for request in requests_for_roles(&scope, permission, server, granting) {
            prop_assert_eq!(authorize(&actor, &request), Decision::Allow);
        }
    }

    #[test]
    fn an_account_outside_a_workspace_gets_nothing_in_it(
        actor in non_administrator(),
        scope in channel_scope(),
        permission in workspace_permission(),
        granting in workspace_permissions(),
    ) {
        let mut outside = scope;
        outside.workspace.membership = Membership::NotMember;
        let requests = [
            Request::ViewWorkspace(&outside.workspace),
            Request::Workspace { scope: &outside.workspace, permission },
            Request::ManageWorkspaceRoles { scope: &outside.workspace, granting, in_channels: &[] },
            Request::Channel { scope: &outside, permission },
            Request::ManageChannelOverrides { scope: &outside, granting },
            Request::DeleteChannel { scope: &outside, content: ChannelContent::NoPermanentMessages },
            Request::AuditEntry(AuditScope::Workspace(&outside.workspace)),
            Request::AuditEntry(AuditScope::Channel(&outside)),
        ];
        for request in requests {
            prop_assert_eq!(authorize(&actor, &request), Decision::Deny(Denial::NotMember));
        }
    }

    #[test]
    fn nobody_without_the_administrator_permission_grants_what_they_lack(
        actor in non_administrator(),
        scope in member_channel_scope(),
        granting in workspace_permissions(),
    ) {
        let in_workspace = Request::ManageWorkspaceRoles { scope: &scope.workspace, granting, in_channels: &[] };
        if authorize(&actor, &in_workspace).is_allowed() {
            prop_assert!(scope.workspace.base_permissions().contains_all(granting));
            prop_assert!(scope.workspace.base_permissions().contains(WorkspacePermission::ManageRoles));
        }
        let grants = [ChannelGrant { scope: &scope, granting }];
        let with_overrides = Request::ManageWorkspaceRoles {
            scope: &scope.workspace,
            granting: WorkspacePermissions::EMPTY,
            in_channels: &grants,
        };
        if authorize(&actor, &with_overrides).is_allowed() {
            prop_assert!(scope.effective_permissions().contains_all(granting));
            prop_assert!(
                granting.is_empty()
                    || scope.effective_permissions().contains(WorkspacePermission::ViewChannel)
            );
        }
        let in_channel = Request::ManageChannelOverrides { scope: &scope, granting };
        if authorize(&actor, &in_channel).is_allowed() {
            prop_assert!(scope.effective_permissions().contains_all(granting));
            prop_assert!(scope.effective_permissions().contains(WorkspacePermission::ManageChannel));
        }
        let server_roles = Request::ManageServerRoles { granting: ServerPermissions::EMPTY };
        prop_assert!(!authorize(&actor, &server_roles).is_allowed());
    }

    #[test]
    fn a_channel_permission_is_never_allowed_without_sight_of_the_channel(
        actor in non_administrator(),
        scope in member_channel_scope(),
        permission in workspace_permission(),
    ) {
        if !scope.effective_permissions().contains(WorkspacePermission::ViewChannel) {
            for request in [
                Request::Channel { scope: &scope, permission },
                Request::AuditEntry(AuditScope::Channel(&scope)),
                Request::DeleteChannel { scope: &scope, content: ChannelContent::NoPermanentMessages },
            ] {
                prop_assert!(!authorize(&actor, &request).is_allowed());
            }
        }
    }

    #[test]
    fn nobody_without_the_administrator_permission_lifts_a_deny_by_taking_a_role(
        actor in non_administrator(),
        caller in member_channel_scope(),
        target_roles in prop::collection::vec(workspace_permissions(), 1..5),
        taken in any::<Index>(),
    ) {
        let holding = |roles: Vec<HeldRole>| ChannelScope {
            person_override: Default::default(),
            workspace: WorkspaceScope {
                person_override: Default::default(),
                membership: Membership::Member,
                everyone: caller.workspace.everyone,
                roles,
            },
            everyone_override: caller.everyone_override,
            role_overrides: caller.role_overrides.clone(),
        };
        let lifted = |role: RoleKey| caller
            .role_overrides
            .iter()
            .find(|role_override| role_override.role == role)
            .map_or(Override::NONE, |role_override| role_override.rule)
            .granted_by_change_to(Override::NONE);
        let allowed = |role: RoleKey| {
            let grants = [ChannelGrant { scope: &caller, granting: lifted(role) }];
            authorize(&actor, &Request::ManageWorkspaceRoles {
                scope: &caller.workspace,
                granting: WorkspacePermissions::EMPTY,
                in_channels: &grants,
            })
            .is_allowed()
        };

        // Another member as the target: what the member gets back is what the caller can
        // use in the channel.
        let roles: Vec<HeldRole> = target_roles
            .into_iter()
            .enumerate()
            .map(|(index, permissions)| HeldRole { key: RoleKey(index as u128), permissions })
            .collect();
        let role = roles[taken.index(roles.len())].key;
        let before = holding(roles.clone());
        let after = holding(roles.into_iter().filter(|held| held.key != role).collect());
        let gained = after.effective_permissions().difference(before.effective_permissions());
        prop_assert!(lifted(role).contains_all(gained));
        if allowed(role) {
            prop_assert!(caller.usable_permissions().contains_all(gained));
        }

        // The own account as the target: the caller gets nothing back.
        if !caller.workspace.roles.is_empty() {
            let own = caller.workspace.roles[taken.index(caller.workspace.roles.len())].key;
            if allowed(own) {
                let kept = caller.workspace.roles.iter().copied().filter(|held| held.key != own);
                let without = holding(kept.collect());
                prop_assert!(
                    caller.effective_permissions().contains_all(without.effective_permissions())
                );
            }
        }
    }
}
