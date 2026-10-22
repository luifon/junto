#![allow(clippy::unwrap_used, clippy::expect_used)]

use junto_authz::{
    Acquaintance, Actor, Admission, AuditScope, Authorship, ChannelContent, ChannelGrant,
    ChannelScope, Contact, Decision, Denial, Departing, HeldRole, Membership, MessagePlace,
    Override, OwnerAction, Participation, Permanence, Position, Reach, Request, RoleKey,
    RoleOverride, ServerPermission, ServerPermissions, Target, WorkspacePermission,
    WorkspacePermissions, WorkspaceScope, authorize,
};

use Decision::{Allow, Deny};
use WorkspacePermission as W;

fn owner() -> Actor {
    Actor {
        position: Position::Owner,
        server_roles: vec![],
    }
}

fn administrator() -> Actor {
    with_server(&[ServerPermission::Administrator])
}

fn plain() -> Actor {
    with_server(&[])
}

fn with_server(permissions: &[ServerPermission]) -> Actor {
    Actor {
        position: Position::Member,
        server_roles: permissions
            .iter()
            .map(|permission| ServerPermissions::from(*permission))
            .collect(),
    }
}

fn set(permissions: &[W]) -> WorkspacePermissions {
    permissions.iter().copied().collect()
}

fn role(key: u128, permissions: &[W]) -> HeldRole {
    HeldRole {
        key: RoleKey(key),
        permissions: set(permissions),
    }
}

fn workspace(everyone: &[W], roles: Vec<HeldRole>) -> WorkspaceScope {
    WorkspaceScope {
        membership: Membership::Member,
        everyone: set(everyone),
        roles,
    }
}

fn outside(everyone: &[W]) -> WorkspaceScope {
    WorkspaceScope {
        membership: Membership::NotMember,
        everyone: set(everyone),
        roles: vec![],
    }
}

fn rule(allow: &[W], deny: &[W]) -> Override {
    Override::new(set(allow), set(deny)).unwrap()
}

fn role_rule(key: u128, allow: &[W], deny: &[W]) -> RoleOverride {
    RoleOverride {
        role: RoleKey(key),
        rule: rule(allow, deny),
    }
}

fn channel(
    workspace: WorkspaceScope,
    everyone_override: Override,
    role_overrides: Vec<RoleOverride>,
) -> ChannelScope {
    ChannelScope {
        workspace,
        everyone_override,
        role_overrides,
    }
}

fn missing(permission: W) -> Decision {
    Deny(Denial::MissingWorkspacePermission(permission))
}

fn missing_server(permission: ServerPermission) -> Decision {
    Deny(Denial::MissingServerPermission(permission))
}

#[test]
fn override_refuses_a_permission_in_both_sets() {
    assert!(Override::new(set(&[W::LockRoom]), set(&[W::LockRoom])).is_err());
    assert!(Override::new(set(&[W::LockRoom]), set(&[W::ViewChannel])).is_ok());
}

#[test]
fn override_change_reports_what_it_can_add() {
    let cases = [
        (
            rule(&[], &[]),
            rule(&[W::LockRoom], &[]),
            set(&[W::LockRoom]),
        ),
        (
            rule(&[], &[W::ViewChannel]),
            rule(&[], &[]),
            set(&[W::ViewChannel]),
        ),
        (
            rule(&[W::LockRoom], &[]),
            rule(&[], &[W::LockRoom]),
            set(&[]),
        ),
        (
            rule(&[], &[W::ViewChannel, W::LockRoom]),
            rule(&[W::ViewChannel, W::RecordCalls], &[W::LockRoom]),
            set(&[W::ViewChannel, W::RecordCalls]),
        ),
    ];
    for (before, after, expected) in cases {
        assert_eq!(before.granted_by_change_to(after), expected);
    }
}

#[test]
fn server_permissions() {
    let cases = [
        (
            plain(),
            ServerPermission::CreateInvites,
            missing_server(ServerPermission::CreateInvites),
        ),
        (
            with_server(&[ServerPermission::CreateInvites]),
            ServerPermission::CreateInvites,
            Allow,
        ),
        (
            with_server(&[ServerPermission::CreateInvites]),
            ServerPermission::BanMembers,
            missing_server(ServerPermission::BanMembers),
        ),
        (
            with_server(&[
                ServerPermission::CreateInvites,
                ServerPermission::BanMembers,
            ]),
            ServerPermission::BanMembers,
            Allow,
        ),
        (administrator(), ServerPermission::CreateWorkspace, Allow),
        (administrator(), ServerPermission::ViewServerAuditLog, Allow),
        (owner(), ServerPermission::ApproveJoinRequests, Allow),
        (
            plain(),
            ServerPermission::Administrator,
            missing_server(ServerPermission::Administrator),
        ),
    ];
    for (actor, permission, expected) in cases {
        assert_eq!(
            authorize(&actor, &Request::Server(permission)),
            expected,
            "{actor:?} {permission:?}"
        );
    }
}

#[test]
fn server_roles_are_managed_by_administrators_only() {
    let granting = ServerPermissions::from(ServerPermission::CreateInvites);
    let request = Request::ManageServerRoles { granting };
    let every_other = ServerPermission::ALL
        .iter()
        .copied()
        .filter(|permission| *permission != ServerPermission::Administrator)
        .collect::<Vec<_>>();
    assert_eq!(
        authorize(&with_server(&every_other), &request),
        missing_server(ServerPermission::Administrator)
    );
    assert_eq!(authorize(&administrator(), &request), Allow);
    assert_eq!(authorize(&owner(), &request), Allow);
}

#[test]
fn admission_needs_the_permission_and_membership_of_each_granted_workspace() {
    use Admission::{ApproveJoinRequest, CreateInvite};
    use Reach::{BeyondMemberships, WithinMemberships};
    let inviter = with_server(&[ServerPermission::CreateInvites]);
    let approver = with_server(&[ServerPermission::ApproveJoinRequests]);
    let cases = [
        (&inviter, CreateInvite, WithinMemberships, Allow),
        (
            &inviter,
            CreateInvite,
            BeyondMemberships,
            Deny(Denial::NotMember),
        ),
        (
            &inviter,
            ApproveJoinRequest,
            WithinMemberships,
            missing_server(ServerPermission::ApproveJoinRequests),
        ),
        (&approver, ApproveJoinRequest, WithinMemberships, Allow),
        (
            &approver,
            ApproveJoinRequest,
            BeyondMemberships,
            Deny(Denial::NotMember),
        ),
        (
            &approver,
            CreateInvite,
            WithinMemberships,
            missing_server(ServerPermission::CreateInvites),
        ),
    ];
    for (actor, admission, reach, expected) in cases {
        assert_eq!(
            authorize(actor, &Request::Admit { admission, reach }),
            expected,
            "{actor:?} {admission:?} {reach:?}"
        );
    }
    for admission in [CreateInvite, ApproveJoinRequest] {
        let request = Request::Admit {
            admission,
            reach: BeyondMemberships,
        };
        assert_eq!(authorize(&administrator(), &request), Allow);
        assert_eq!(authorize(&owner(), &request), Allow);
    }
}

#[test]
fn moderation_protects_the_owner_and_the_actor() {
    let moderator = with_server(&[ServerPermission::BanMembers]);
    let cases = [
        (
            plain(),
            Target::Other,
            missing_server(ServerPermission::BanMembers),
        ),
        (moderator.clone(), Target::Other, Allow),
        (
            moderator.clone(),
            Target::Owner,
            Deny(Denial::OwnerProtected),
        ),
        (moderator, Target::Actor, Deny(Denial::OwnAccountTarget)),
        (administrator(), Target::Other, Allow),
        (administrator(), Target::Owner, Deny(Denial::OwnerProtected)),
        (
            administrator(),
            Target::Actor,
            Deny(Denial::OwnAccountTarget),
        ),
        (owner(), Target::Other, Allow),
        (owner(), Target::Owner, Deny(Denial::OwnerProtected)),
    ];
    for (actor, target, expected) in cases {
        assert_eq!(
            authorize(&actor, &Request::Moderate { target }),
            expected,
            "{actor:?} {target:?}"
        );
    }
}

#[test]
fn owner_only_actions_belong_to_the_owner() {
    let everything = with_server(ServerPermission::ALL);
    for action in [OwnerAction::RecoverAccount, OwnerAction::AssignOwner] {
        let request = Request::OwnerOnly(action);
        assert_eq!(authorize(&owner(), &request), Allow);
        assert_eq!(
            authorize(&administrator(), &request),
            Deny(Denial::OwnerOnly)
        );
        assert_eq!(authorize(&everything, &request), Deny(Denial::OwnerOnly));
        assert_eq!(authorize(&plain(), &request), Deny(Denial::OwnerOnly));
    }
}

#[test]
fn own_account_is_open_to_every_actor() {
    for actor in [plain(), administrator(), owner()] {
        assert_eq!(authorize(&actor, &Request::OwnAccount), Allow);
    }
}

#[test]
fn every_actor_may_leave_a_workspace_and_only_a_member_may_leave_the_server() {
    let moderator = with_server(&[ServerPermission::BanMembers]);
    for actor in [plain(), administrator(), moderator.clone(), owner()] {
        assert_eq!(
            authorize(&actor, &Request::Depart(Departing::Workspace)),
            Allow
        );
    }
    for actor in [plain(), administrator(), moderator] {
        assert_eq!(
            authorize(&actor, &Request::Depart(Departing::Server)),
            Allow
        );
    }
    assert_eq!(
        authorize(&owner(), &Request::Depart(Departing::Server)),
        Deny(Denial::OwnerProtected)
    );
}

#[test]
fn the_profile_of_a_departed_account_is_open_to_every_actor() {
    let inviter = with_server(&[ServerPermission::CreateInvites]);
    for actor in [owner(), administrator(), plain(), inviter] {
        assert_eq!(
            authorize(&actor, &Request::ViewAccount(Acquaintance::Departed)),
            Allow
        );
    }
}

#[test]
fn a_profile_is_open_to_its_account_to_fellow_members_to_contacts_and_to_moderators() {
    let moderator = with_server(&[ServerPermission::BanMembers]);
    let inviter = with_server(&[ServerPermission::CreateInvites]);
    for actor in [
        owner(),
        administrator(),
        moderator.clone(),
        plain(),
        inviter.clone(),
    ] {
        for near in [
            Acquaintance::Own,
            Acquaintance::SharedWorkspace,
            Acquaintance::Administrator,
        ] {
            assert_eq!(authorize(&actor, &Request::ViewAccount(near)), Allow);
        }
    }
    let unrelated = Request::ViewAccount(Acquaintance::Unrelated);
    for actor in [owner(), administrator(), moderator] {
        assert_eq!(authorize(&actor, &unrelated), Allow);
    }
    for actor in [plain(), inviter] {
        assert_eq!(
            authorize(&actor, &unrelated),
            missing_server(ServerPermission::BanMembers)
        );
    }
}

#[test]
fn workspace_base_is_everyone_plus_all_roles() {
    let scope = workspace(
        &[W::ViewChannel],
        vec![role(1, &[W::ManageRoles]), role(2, &[W::RecordCalls])],
    );
    let cases = [
        (W::ViewChannel, Allow),
        (W::ManageRoles, Allow),
        (W::RecordCalls, Allow),
        (W::ManageChannel, missing(W::ManageChannel)),
    ];
    for (permission, expected) in cases {
        let request = Request::Workspace {
            scope: &scope,
            permission,
        };
        assert_eq!(authorize(&plain(), &request), expected, "{permission:?}");
        assert_eq!(authorize(&administrator(), &request), Allow);
    }
}

#[test]
fn workspace_is_closed_to_accounts_that_are_not_members() {
    let scope = outside(WorkspacePermission::ALL);
    let requests = [
        Request::ViewWorkspace(&scope),
        Request::Workspace {
            scope: &scope,
            permission: W::ViewChannel,
        },
        Request::ManageWorkspaceRoles {
            scope: &scope,
            granting: WorkspacePermissions::EMPTY,
            in_channels: &[],
        },
        Request::AuditEntry(AuditScope::Workspace(&scope)),
    ];
    for request in requests {
        assert_eq!(authorize(&plain(), &request), Deny(Denial::NotMember));
        assert_eq!(authorize(&administrator(), &request), Allow);
        assert_eq!(authorize(&owner(), &request), Allow);
    }
    let member = workspace(&[], vec![]);
    assert_eq!(authorize(&plain(), &Request::ViewWorkspace(&member)), Allow);
}

#[test]
fn nobody_grants_a_workspace_permission_they_do_not_hold() {
    let manager = workspace(
        &[W::ViewChannel],
        vec![role(1, &[W::ManageRoles, W::RecordCalls])],
    );
    let without_manage = workspace(&[W::ViewChannel], vec![role(1, &[W::RecordCalls])]);
    let cases = [
        (&manager, set(&[]), Allow),
        (&manager, set(&[W::RecordCalls, W::ViewChannel]), Allow),
        (&manager, set(&[W::ManageRoles]), Allow),
        (
            &manager,
            set(&[W::ManageChannel]),
            Deny(Denial::ExceedsOwnPermissions),
        ),
        (
            &manager,
            set(&[W::RecordCalls, W::ExportChannel]),
            Deny(Denial::ExceedsOwnPermissions),
        ),
        (&without_manage, set(&[]), missing(W::ManageRoles)),
        (
            &without_manage,
            set(&[W::RecordCalls]),
            missing(W::ManageRoles),
        ),
    ];
    for (scope, granting, expected) in cases {
        let request = Request::ManageWorkspaceRoles {
            scope,
            granting,
            in_channels: &[],
        };
        assert_eq!(authorize(&plain(), &request), expected, "{granting:?}");
    }
    let request = Request::ManageWorkspaceRoles {
        scope: &without_manage,
        granting: WorkspacePermissions::ALL,
        in_channels: &[],
    };
    assert_eq!(authorize(&administrator(), &request), Allow);
}

#[test]
fn a_role_is_granted_only_by_a_holder_of_what_its_channel_overrides_allow() {
    let manager = workspace(
        &[W::ViewChannel],
        vec![role(1, &[W::ManageRoles]), role(2, &[])],
    );
    let open = channel(manager.clone(), rule(&[], &[]), vec![]);
    let hidden = channel(manager.clone(), rule(&[], &[W::ViewChannel]), vec![]);
    let recording = channel(
        manager.clone(),
        rule(&[], &[]),
        vec![role_rule(2, &[W::RecordCalls], &[])],
    );
    let grant = |scope, permissions: &[W]| ChannelGrant {
        scope,
        granting: set(permissions),
    };
    let cases: Vec<(Vec<ChannelGrant<'_>>, Decision)> = vec![
        (vec![], Allow),
        (vec![grant(&open, &[W::ViewChannel])], Allow),
        (vec![grant(&open, &[])], Allow),
        (vec![grant(&hidden, &[])], Allow),
        (
            vec![grant(&recording, &[W::RecordCalls, W::ViewChannel])],
            Allow,
        ),
        (
            vec![grant(&open, &[W::RecordCalls])],
            Deny(Denial::ExceedsOwnPermissions),
        ),
        (
            vec![grant(&hidden, &[W::ViewChannel])],
            Deny(Denial::ExceedsOwnPermissions),
        ),
        (
            vec![grant(&hidden, &[W::ManageRoles])],
            Deny(Denial::ExceedsOwnPermissions),
        ),
        (
            vec![
                grant(&open, &[W::ViewChannel]),
                grant(&recording, &[W::RecordCalls]),
                grant(&hidden, &[W::ViewAuditEntries]),
            ],
            Deny(Denial::ExceedsOwnPermissions),
        ),
    ];
    for (in_channels, expected) in &cases {
        let request = Request::ManageWorkspaceRoles {
            scope: &manager,
            granting: WorkspacePermissions::EMPTY,
            in_channels,
        };
        assert_eq!(authorize(&plain(), &request), *expected, "{in_channels:?}");
        assert_eq!(authorize(&administrator(), &request), Allow);
        assert_eq!(authorize(&owner(), &request), Allow);
    }

    let without_manage = workspace(&[W::ViewChannel], vec![]);
    let request = Request::ManageWorkspaceRoles {
        scope: &without_manage,
        granting: WorkspacePermissions::EMPTY,
        in_channels: &[],
    };
    assert_eq!(authorize(&plain(), &request), missing(W::ManageRoles));
}

#[test]
fn usable_permissions_are_empty_without_sight_of_the_channel() {
    let member = workspace(&[W::ViewChannel, W::LockRoom], vec![]);
    let open = channel(member.clone(), rule(&[], &[]), vec![]);
    let hidden = channel(member, rule(&[], &[W::ViewChannel]), vec![]);
    assert_eq!(
        open.usable_permissions(),
        set(&[W::ViewChannel, W::LockRoom])
    );
    assert_eq!(hidden.effective_permissions(), set(&[W::LockRoom]));
    assert_eq!(hidden.usable_permissions(), set(&[]));
}

#[test]
fn channel_overrides_apply_in_the_stated_order() {
    struct Case {
        name: &'static str,
        everyone: &'static [W],
        roles: Vec<HeldRole>,
        everyone_override: Override,
        role_overrides: Vec<RoleOverride>,
        permission: W,
        expected: Decision,
    }
    let view = &[W::ViewChannel][..];
    let cases = [
        Case {
            name: "base permission passes without overrides",
            everyone: &[W::ViewChannel, W::LockRoom],
            roles: vec![],
            everyone_override: rule(&[], &[]),
            role_overrides: vec![],
            permission: W::LockRoom,
            expected: Allow,
        },
        Case {
            name: "everyone deny removes a base permission",
            everyone: &[W::ViewChannel, W::LockRoom],
            roles: vec![],
            everyone_override: rule(&[], &[W::LockRoom]),
            role_overrides: vec![],
            permission: W::LockRoom,
            expected: missing(W::LockRoom),
        },
        Case {
            name: "everyone deny removes a permission that a role gives",
            everyone: view,
            roles: vec![role(1, &[W::RecordCalls])],
            everyone_override: rule(&[], &[W::RecordCalls]),
            role_overrides: vec![],
            permission: W::RecordCalls,
            expected: missing(W::RecordCalls),
        },
        Case {
            name: "everyone allow adds a permission",
            everyone: view,
            roles: vec![],
            everyone_override: rule(&[W::RecordCalls], &[]),
            role_overrides: vec![],
            permission: W::RecordCalls,
            expected: Allow,
        },
        Case {
            name: "role deny removes what the everyone override allowed",
            everyone: view,
            roles: vec![role(1, &[])],
            everyone_override: rule(&[W::RecordCalls], &[]),
            role_overrides: vec![role_rule(1, &[], &[W::RecordCalls])],
            permission: W::RecordCalls,
            expected: missing(W::RecordCalls),
        },
        Case {
            name: "role allow restores what the everyone override denied",
            everyone: &[W::ViewChannel, W::LockRoom],
            roles: vec![role(1, &[])],
            everyone_override: rule(&[], &[W::LockRoom]),
            role_overrides: vec![role_rule(1, &[W::LockRoom], &[])],
            permission: W::LockRoom,
            expected: Allow,
        },
        Case {
            name: "allow from one role wins over deny from another role",
            everyone: view,
            roles: vec![role(1, &[W::RecordCalls]), role(2, &[])],
            everyone_override: rule(&[], &[]),
            role_overrides: vec![
                role_rule(1, &[], &[W::RecordCalls]),
                role_rule(2, &[W::RecordCalls], &[]),
            ],
            permission: W::RecordCalls,
            expected: Allow,
        },
        Case {
            name: "allow wins whatever the order of the overrides",
            everyone: view,
            roles: vec![role(2, &[]), role(1, &[W::RecordCalls])],
            everyone_override: rule(&[], &[]),
            role_overrides: vec![
                role_rule(2, &[W::RecordCalls], &[]),
                role_rule(1, &[], &[W::RecordCalls]),
            ],
            permission: W::RecordCalls,
            expected: Allow,
        },
        Case {
            name: "override of a role the member does not hold is ignored",
            everyone: view,
            roles: vec![role(1, &[])],
            everyone_override: rule(&[], &[]),
            role_overrides: vec![role_rule(2, &[W::RecordCalls], &[W::ViewChannel])],
            permission: W::RecordCalls,
            expected: missing(W::RecordCalls),
        },
        Case {
            name: "deny of a role the member does not hold is ignored",
            everyone: &[W::ViewChannel, W::LockRoom],
            roles: vec![],
            everyone_override: rule(&[], &[]),
            role_overrides: vec![role_rule(2, &[], &[W::LockRoom])],
            permission: W::LockRoom,
            expected: Allow,
        },
        Case {
            name: "private channel is hidden from a member without an allowed role",
            everyone: &[W::ViewChannel, W::LockRoom],
            roles: vec![role(1, &[])],
            everyone_override: rule(&[], &[W::ViewChannel]),
            role_overrides: vec![role_rule(2, &[W::ViewChannel], &[])],
            permission: W::ViewChannel,
            expected: missing(W::ViewChannel),
        },
        Case {
            name: "private channel is open to a member with an allowed role",
            everyone: &[W::ViewChannel, W::LockRoom],
            roles: vec![role(2, &[])],
            everyone_override: rule(&[], &[W::ViewChannel]),
            role_overrides: vec![role_rule(2, &[W::ViewChannel], &[])],
            permission: W::LockRoom,
            expected: Allow,
        },
        Case {
            name: "a permission in a channel needs sight of the channel",
            everyone: &[W::ViewChannel],
            roles: vec![role(1, &[W::ManageChannel])],
            everyone_override: rule(&[], &[W::ViewChannel]),
            role_overrides: vec![],
            permission: W::ManageChannel,
            expected: missing(W::ViewChannel),
        },
    ];
    for case in cases {
        let scope = channel(
            workspace(case.everyone, case.roles),
            case.everyone_override,
            case.role_overrides,
        );
        let request = Request::Channel {
            scope: &scope,
            permission: case.permission,
        };
        assert_eq!(
            authorize(&plain(), &request),
            case.expected,
            "{}",
            case.name
        );
        assert_eq!(
            authorize(&administrator(), &request),
            Allow,
            "{}",
            case.name
        );
        assert_eq!(authorize(&owner(), &request), Allow, "{}", case.name);
    }
}

#[test]
fn channel_is_closed_to_accounts_that_are_not_members() {
    let scope = channel(outside(WorkspacePermission::ALL), rule(&[], &[]), vec![]);
    let request = Request::Channel {
        scope: &scope,
        permission: W::ViewChannel,
    };
    assert_eq!(authorize(&plain(), &request), Deny(Denial::NotMember));
    assert_eq!(authorize(&administrator(), &request), Allow);
}

#[test]
fn nobody_grants_a_channel_permission_they_do_not_hold() {
    let scope = channel(
        workspace(&[W::ViewChannel], vec![role(1, &[W::ManageChannel])]),
        rule(&[], &[]),
        vec![role_rule(1, &[W::RecordCalls], &[])],
    );
    let cases = [
        (set(&[]), Allow),
        (
            set(&[W::RecordCalls, W::ManageChannel, W::ViewChannel]),
            Allow,
        ),
        (
            set(&[W::ExportChannel]),
            Deny(Denial::ExceedsOwnPermissions),
        ),
    ];
    for (granting, expected) in cases {
        let request = Request::ManageChannelOverrides {
            scope: &scope,
            granting,
        };
        assert_eq!(authorize(&plain(), &request), expected, "{granting:?}");
        assert_eq!(authorize(&administrator(), &request), Allow);
    }
    let without_manage = channel(workspace(&[W::ViewChannel], vec![]), rule(&[], &[]), vec![]);
    let request = Request::ManageChannelOverrides {
        scope: &without_manage,
        granting: set(&[]),
    };
    assert_eq!(authorize(&plain(), &request), missing(W::ManageChannel));
}

#[test]
fn deleting_a_channel_with_permanent_messages_belongs_to_the_owner() {
    let scope = channel(
        workspace(&[W::ViewChannel], vec![role(1, &[W::ManageChannel])]),
        rule(&[], &[]),
        vec![],
    );
    let unmanaged = channel(workspace(&[W::ViewChannel], vec![]), rule(&[], &[]), vec![]);
    let cases = [
        (plain(), &scope, ChannelContent::NoPermanentMessages, Allow),
        (
            plain(),
            &unmanaged,
            ChannelContent::NoPermanentMessages,
            missing(W::ManageChannel),
        ),
        (
            administrator(),
            &unmanaged,
            ChannelContent::NoPermanentMessages,
            Allow,
        ),
        (
            plain(),
            &scope,
            ChannelContent::PermanentMessages,
            Deny(Denial::OwnerOnly),
        ),
        (
            administrator(),
            &scope,
            ChannelContent::PermanentMessages,
            Deny(Denial::OwnerOnly),
        ),
        (
            owner(),
            &unmanaged,
            ChannelContent::PermanentMessages,
            Allow,
        ),
        (
            owner(),
            &unmanaged,
            ChannelContent::NoPermanentMessages,
            Allow,
        ),
    ];
    for (actor, scope, content, expected) in cases {
        let request = Request::DeleteChannel { scope, content };
        assert_eq!(
            authorize(&actor, &request),
            expected,
            "{actor:?} {content:?}"
        );
    }
}

#[test]
fn two_accounts_reach_each_other_with_a_common_workspace_or_when_one_administers_the_server() {
    let others: Vec<ServerPermission> = ServerPermission::ALL
        .iter()
        .copied()
        .filter(|permission| *permission != ServerPermission::Administrator)
        .collect();
    let everything_else = with_server(&others);
    for actor in [plain(), everything_else.clone(), administrator(), owner()] {
        for near in [Contact::SharedWorkspace, Contact::Administrator] {
            assert_eq!(
                authorize(&actor, &Request::Contact(near)),
                Allow,
                "{actor:?}"
            );
        }
    }
    let unrelated = Request::Contact(Contact::Unrelated);
    for actor in [plain(), everything_else] {
        assert_eq!(
            authorize(&actor, &unrelated),
            missing_server(ServerPermission::Administrator),
            "{actor:?}"
        );
    }
    for actor in [administrator(), owner()] {
        assert_eq!(authorize(&actor, &unrelated), Allow, "{actor:?}");
    }
}

#[test]
fn direct_messages_are_closed_to_everyone_but_their_participants() {
    let everything = with_server(ServerPermission::ALL);
    for actor in [plain(), administrator(), everything, owner()] {
        assert_eq!(
            authorize(&actor, &Request::DirectMessage(Participation::Outsider)),
            Deny(Denial::NotParticipant),
            "{actor:?}"
        );
        assert_eq!(
            authorize(&actor, &Request::DirectMessage(Participation::Participant)),
            Allow,
            "{actor:?}"
        );
    }
}

#[test]
fn message_edits() {
    let visible = channel(workspace(&[W::ViewChannel], vec![]), rule(&[], &[]), vec![]);
    let hidden = channel(
        workspace(&[W::ViewChannel], vec![]),
        rule(&[], &[W::ViewChannel]),
        vec![],
    );
    let in_channel = MessagePlace::Channel(&visible);
    let in_hidden = MessagePlace::Channel(&hidden);
    let in_own_dm = MessagePlace::DirectMessage(Participation::Participant);
    let in_other_dm = MessagePlace::DirectMessage(Participation::Outsider);
    use Authorship::{Author, Other};
    use Permanence::{Editable, Permanent};
    let cases = [
        (plain(), in_channel, Editable, Author, Allow),
        (
            plain(),
            in_channel,
            Editable,
            Other,
            Deny(Denial::NotAuthor),
        ),
        (
            plain(),
            in_channel,
            Permanent,
            Author,
            Deny(Denial::PermanentMessage),
        ),
        (
            plain(),
            in_hidden,
            Editable,
            Author,
            missing(W::ViewChannel),
        ),
        (plain(), in_own_dm, Editable, Author, Allow),
        (plain(), in_own_dm, Editable, Other, Deny(Denial::NotAuthor)),
        (
            plain(),
            in_own_dm,
            Permanent,
            Author,
            Deny(Denial::PermanentMessage),
        ),
        (
            plain(),
            in_other_dm,
            Editable,
            Author,
            Deny(Denial::NotParticipant),
        ),
        (
            administrator(),
            in_channel,
            Permanent,
            Author,
            Deny(Denial::PermanentMessage),
        ),
        (
            administrator(),
            in_channel,
            Permanent,
            Other,
            Deny(Denial::PermanentMessage),
        ),
        (
            administrator(),
            in_channel,
            Editable,
            Other,
            Deny(Denial::NotAuthor),
        ),
        (administrator(), in_hidden, Editable, Author, Allow),
        (
            administrator(),
            in_other_dm,
            Editable,
            Author,
            Deny(Denial::NotParticipant),
        ),
        (
            owner(),
            in_channel,
            Permanent,
            Author,
            Deny(Denial::PermanentMessage),
        ),
        (
            owner(),
            in_channel,
            Editable,
            Other,
            Deny(Denial::NotAuthor),
        ),
        (
            owner(),
            in_other_dm,
            Editable,
            Other,
            Deny(Denial::NotParticipant),
        ),
    ];
    for (actor, place, permanence, authorship, expected) in cases {
        let request = Request::EditMessage {
            place,
            permanence,
            authorship,
        };
        assert_eq!(
            authorize(&actor, &request),
            expected,
            "{actor:?} {permanence:?} {authorship:?}"
        );
    }
}

#[test]
fn audit_entries_follow_the_scope_they_are_about() {
    let auditor = workspace(&[W::ViewChannel], vec![role(1, &[W::ViewAuditEntries])]);
    let member = workspace(&[W::ViewChannel], vec![]);
    let open_channel = channel(auditor.clone(), rule(&[], &[]), vec![]);
    let private_channel = channel(auditor.clone(), rule(&[], &[W::ViewChannel]), vec![]);
    let private_with_access = channel(
        auditor.clone(),
        rule(&[], &[W::ViewChannel]),
        vec![role_rule(1, &[W::ViewChannel], &[])],
    );
    let audit_denied_here = channel(
        auditor.clone(),
        rule(&[], &[]),
        vec![role_rule(1, &[], &[W::ViewAuditEntries])],
    );
    let server_auditor = with_server(&[ServerPermission::ViewServerAuditLog]);

    let cases = [
        (
            plain(),
            AuditScope::Server,
            missing_server(ServerPermission::ViewServerAuditLog),
        ),
        (server_auditor.clone(), AuditScope::Server, Allow),
        (administrator(), AuditScope::Server, Allow),
        (plain(), AuditScope::Workspace(&auditor), Allow),
        (
            plain(),
            AuditScope::Workspace(&member),
            missing(W::ViewAuditEntries),
        ),
        (
            server_auditor.clone(),
            AuditScope::Workspace(&member),
            missing(W::ViewAuditEntries),
        ),
        (administrator(), AuditScope::Workspace(&member), Allow),
        (plain(), AuditScope::Channel(&open_channel), Allow),
        (
            plain(),
            AuditScope::Channel(&private_channel),
            missing(W::ViewChannel),
        ),
        (plain(), AuditScope::Channel(&private_with_access), Allow),
        (
            plain(),
            AuditScope::Channel(&audit_denied_here),
            missing(W::ViewAuditEntries),
        ),
        (
            administrator(),
            AuditScope::Channel(&private_channel),
            Allow,
        ),
        (
            plain(),
            AuditScope::DeletedChannel,
            missing_server(ServerPermission::Administrator),
        ),
        (
            server_auditor,
            AuditScope::DeletedChannel,
            missing_server(ServerPermission::Administrator),
        ),
        (administrator(), AuditScope::DeletedChannel, Allow),
        (owner(), AuditScope::DeletedChannel, Allow),
    ];
    for (actor, scope, expected) in cases {
        assert_eq!(
            authorize(&actor, &Request::AuditEntry(scope)),
            expected,
            "{actor:?} {scope:?}"
        );
    }
}
