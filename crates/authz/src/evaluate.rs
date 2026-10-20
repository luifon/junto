use crate::model::{
    Acquaintance, Actor, AuditScope, Authorship, ChannelContent, ChannelGrant, ChannelScope,
    Contact, Decision, Denial, Membership, MessagePlace, Participation, Permanence, Position,
    Reach, Request, Target, WorkspaceScope,
};
use crate::permissions::{ServerPermission, WorkspacePermission, WorkspacePermissions};

/// Decides one request.
///
/// The limits run first and bind every actor. The administrator permission passes what is left.
/// The roles and overrides decide for every other actor.
pub fn authorize(actor: &Actor, request: &Request<'_>) -> Decision {
    let check = match apply_limits(actor, request) {
        Limited::Settled(decision) => return decision,
        Limited::Open(check) => check,
    };
    if actor.is_administrator() {
        return Decision::Allow;
    }
    match apply_roles(actor, &check) {
        Some(denial) => Decision::Deny(denial),
        None => Decision::Allow,
    }
}

enum Limited<'a> {
    Settled(Decision),
    Open(Check<'a>),
}

enum Check<'a> {
    Administrator,
    Server(ServerPermission),
    Admit {
        permission: ServerPermission,
        reach: Reach,
    },
    Membership(&'a WorkspaceScope),
    InWorkspace {
        scope: &'a WorkspaceScope,
        permission: WorkspacePermission,
        granting: WorkspacePermissions,
        in_channels: &'a [ChannelGrant<'a>],
    },
    InChannel {
        scope: &'a ChannelScope,
        permission: WorkspacePermission,
        granting: WorkspacePermissions,
    },
}

fn apply_limits<'a>(actor: &Actor, request: &Request<'a>) -> Limited<'a> {
    let deny = |denial| Limited::Settled(Decision::Deny(denial));
    let allow = Limited::Settled(Decision::Allow);
    let owner_only = || match actor.position {
        Position::Owner => Limited::Settled(Decision::Allow),
        Position::Member => Limited::Settled(Decision::Deny(Denial::OwnerOnly)),
    };
    let nothing = WorkspacePermissions::EMPTY;

    match *request {
        Request::DirectMessage(Participation::Participant) => allow,
        Request::DirectMessage(Participation::Outsider) => deny(Denial::NotParticipant),
        Request::Contact(Contact::SharedWorkspace | Contact::Administrator) => allow,
        Request::Contact(Contact::Unrelated) => Limited::Open(Check::Administrator),

        Request::EditMessage {
            permanence: Permanence::Permanent,
            ..
        } => deny(Denial::PermanentMessage),
        Request::EditMessage {
            place: MessagePlace::DirectMessage(Participation::Outsider),
            ..
        } => deny(Denial::NotParticipant),
        Request::EditMessage {
            authorship: Authorship::Other,
            ..
        } => deny(Denial::NotAuthor),
        Request::EditMessage {
            place: MessagePlace::DirectMessage(Participation::Participant),
            ..
        } => allow,
        Request::EditMessage {
            place: MessagePlace::Channel(scope),
            ..
        } => Limited::Open(Check::InChannel {
            scope,
            permission: WorkspacePermission::ViewChannel,
            granting: nothing,
        }),

        Request::OwnerOnly(_) => owner_only(),
        Request::DeleteChannel {
            content: ChannelContent::PermanentMessages,
            ..
        } => owner_only(),
        Request::DeleteChannel {
            scope,
            content: ChannelContent::NoPermanentMessages,
        } => Limited::Open(Check::InChannel {
            scope,
            permission: WorkspacePermission::ManageChannel,
            granting: nothing,
        }),

        Request::DeleteWorkspace {
            content: ChannelContent::PermanentMessages,
        } => owner_only(),
        Request::DeleteWorkspace {
            content: ChannelContent::NoPermanentMessages,
        } => Limited::Open(Check::Administrator),

        Request::Moderate {
            target: Target::Owner,
        } => deny(Denial::OwnerProtected),
        Request::Moderate {
            target: Target::Actor,
        } => deny(Denial::OwnAccountTarget),
        Request::Moderate {
            target: Target::Other,
        } => Limited::Open(Check::Server(ServerPermission::BanMembers)),

        Request::OwnAccount => allow,
        // The profile of every account that the actor may contact (ADR 0307): an actor who
        // administers the server passes the check of an unrelated account.
        Request::ViewAccount(
            Acquaintance::Own | Acquaintance::SharedWorkspace | Acquaintance::Administrator,
        ) => allow,
        Request::ViewAccount(Acquaintance::Unrelated) => {
            Limited::Open(Check::Server(ServerPermission::BanMembers))
        }

        Request::Server(permission) => Limited::Open(Check::Server(permission)),
        Request::ManageServerRoles { .. } => Limited::Open(Check::Administrator),
        Request::Admit { admission, reach } => Limited::Open(Check::Admit {
            permission: admission.permission(),
            reach,
        }),
        Request::ViewWorkspace(scope) => Limited::Open(Check::Membership(scope)),
        Request::Workspace { scope, permission } => Limited::Open(Check::InWorkspace {
            scope,
            permission,
            granting: nothing,
            in_channels: &[],
        }),
        Request::ManageWorkspaceRoles {
            scope,
            granting,
            in_channels,
        } => Limited::Open(Check::InWorkspace {
            scope,
            permission: WorkspacePermission::ManageRoles,
            granting,
            in_channels,
        }),
        Request::Channel { scope, permission } => Limited::Open(Check::InChannel {
            scope,
            permission,
            granting: nothing,
        }),
        Request::ManageChannelOverrides { scope, granting } => Limited::Open(Check::InChannel {
            scope,
            permission: WorkspacePermission::ManageChannel,
            granting,
        }),
        Request::AuditEntry(AuditScope::Server) => {
            Limited::Open(Check::Server(ServerPermission::ViewServerAuditLog))
        }
        Request::AuditEntry(AuditScope::Workspace(scope)) => Limited::Open(Check::InWorkspace {
            scope,
            permission: WorkspacePermission::ViewAuditEntries,
            granting: nothing,
            in_channels: &[],
        }),
        Request::AuditEntry(AuditScope::Channel(scope)) => Limited::Open(Check::InChannel {
            scope,
            permission: WorkspacePermission::ViewAuditEntries,
            granting: nothing,
        }),
        Request::AuditEntry(AuditScope::DeletedChannel | AuditScope::DeletedWorkspace) => {
            Limited::Open(Check::Administrator)
        }
    }
}

fn apply_roles(actor: &Actor, check: &Check<'_>) -> Option<Denial> {
    let server = |permission| {
        (!actor.server_permissions().contains(permission))
            .then_some(Denial::MissingServerPermission(permission))
    };
    match *check {
        Check::Administrator => server(ServerPermission::Administrator),
        Check::Server(permission) => server(permission),
        Check::Admit { permission, reach } => server(permission).or(match reach {
            Reach::WithinMemberships => None,
            Reach::BeyondMemberships => Some(Denial::NotMember),
        }),
        Check::Membership(scope) => membership(scope),
        Check::InWorkspace {
            scope,
            permission,
            granting,
            in_channels,
        } => membership(scope)
            .or_else(|| holds(scope.base_permissions(), permission, granting))
            .or_else(|| {
                in_channels
                    .iter()
                    .any(|grant| {
                        !grant
                            .scope
                            .usable_permissions()
                            .contains_all(grant.granting)
                    })
                    .then_some(Denial::ExceedsOwnPermissions)
            }),
        Check::InChannel {
            scope,
            permission,
            granting,
        } => membership(&scope.workspace).or_else(|| {
            let held = scope.effective_permissions();
            holds(
                held,
                WorkspacePermission::ViewChannel,
                WorkspacePermissions::EMPTY,
            )
            .or_else(|| holds(held, permission, granting))
        }),
    }
}

fn membership(scope: &WorkspaceScope) -> Option<Denial> {
    match scope.membership {
        Membership::Member => None,
        Membership::NotMember => Some(Denial::NotMember),
    }
}

fn holds(
    held: WorkspacePermissions,
    permission: WorkspacePermission,
    granting: WorkspacePermissions,
) -> Option<Denial> {
    if !held.contains(permission) {
        Some(Denial::MissingWorkspacePermission(permission))
    } else if !held.contains_all(granting) {
        Some(Denial::ExceedsOwnPermissions)
    } else {
        None
    }
}
