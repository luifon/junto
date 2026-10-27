use crate::permissions::{
    ServerPermission, ServerPermissions, WorkspacePermission, WorkspacePermissions,
};
use crate::{Override, ServerOverride};

/// Identifies a workspace role inside one evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RoleKey(pub u128);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    Owner,
    Member,
}

/// The account that asks, with the permission sets of its server roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Actor {
    pub position: Position,
    pub server_roles: Vec<ServerPermissions>,
    pub server_override: ServerOverride,
}

impl Actor {
    pub fn server_permissions(&self) -> ServerPermissions {
        self.server_roles
            .iter()
            .fold(ServerPermissions::EMPTY, |held, role| held.union(*role))
            .difference(self.server_override.deny())
            .union(self.server_override.allow())
    }

    /// The server owner is a position above the administrator permission.
    pub fn is_administrator(&self) -> bool {
        self.position == Position::Owner
            || self
                .server_permissions()
                .contains(ServerPermission::Administrator)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Membership {
    Member,
    NotMember,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldRole {
    pub key: RoleKey,
    pub permissions: WorkspacePermissions,
}

/// One workspace as the actor stands in it: the "everyone" role and the actor's own roles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceScope {
    pub membership: Membership,
    pub everyone: WorkspacePermissions,
    pub roles: Vec<HeldRole>,
    pub person_override: Override,
}

impl WorkspaceScope {
    /// The "everyone" role plus all of the member's roles.
    pub fn base_permissions(&self) -> WorkspacePermissions {
        self.roles
            .iter()
            .fold(self.everyone, |held, role| held.union(role.permissions))
            .difference(self.person_override.deny())
            .union(self.person_override.allow())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlappingOverride;

impl std::fmt::Display for OverlappingOverride {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a permission is in both the allow set and the deny set")
    }
}

impl std::error::Error for OverlappingOverride {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoleOverride {
    pub role: RoleKey,
    pub rule: Override,
}

/// One channel as the actor stands in it. `role_overrides` holds every role override of the
/// channel; the evaluation uses the ones of the roles the actor holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChannelScope {
    pub workspace: WorkspaceScope,
    pub everyone_override: Override,
    pub role_overrides: Vec<RoleOverride>,
    pub person_override: Override,
}

impl ChannelScope {
    /// What a member can use in the channel: nothing without sight of the channel.
    pub fn usable_permissions(&self) -> WorkspacePermissions {
        let effective = self.effective_permissions();
        if !effective.contains(WorkspacePermission::ViewChannel) {
            return WorkspacePermissions::EMPTY;
        }
        if effective.contains(WorkspacePermission::Connect) {
            effective
        } else {
            effective.difference(
                WorkspacePermissions::EMPTY
                    .with(WorkspacePermission::Speak)
                    .with(WorkspacePermission::Video)
                    .with(WorkspacePermission::ShareScreen)
                    .with(WorkspacePermission::UseVoiceActivity),
            )
        }
    }

    /// The override of the "everyone" role applies first, then the denies of the member's
    /// roles, then the allows of the member's roles.
    pub fn effective_permissions(&self) -> WorkspacePermissions {
        let after_everyone = self
            .workspace
            .base_permissions()
            .difference(self.everyone_override.deny())
            .union(self.everyone_override.allow());
        let held = self.role_overrides.iter().filter(|role_override| {
            self.workspace
                .roles
                .iter()
                .any(|role| role.key == role_override.role)
        });
        let (allow, deny) = held.fold(
            (WorkspacePermissions::EMPTY, WorkspacePermissions::EMPTY),
            |(allow, deny), role_override| {
                (
                    allow.union(role_override.rule.allow()),
                    deny.union(role_override.rule.deny()),
                )
            },
        );
        after_everyone
            .difference(deny)
            .union(allow)
            .difference(self.person_override.deny())
            .union(self.person_override.allow())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Participation {
    Participant,
    Outsider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permanence {
    Permanent,
    Editable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authorship {
    Author,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessagePlace<'a> {
    Channel(&'a ChannelScope),
    DirectMessage(Participation),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerAction {
    RecoverAccount,
    AssignOwner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelContent {
    NoPermanentMessages,
    PermanentMessages,
}

/// The account that a removal, a ban or an unban acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Owner,
    Actor,
    Other,
}

/// How the actor stands to an account whose profile it asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acquaintance {
    Own,
    /// The two accounts are members of at least one common workspace.
    SharedWorkspace,
    /// The two accounts have no common workspace, and the other account is active and is
    /// the server owner or holds the administrator permission: the actor may write to it
    /// (ADR 0307).
    Administrator,
    /// The other account is inactive or deleted, and the actor is a member of a workspace
    /// that it left by itself or has a direct conversation with it: the actor reads content
    /// that it wrote, and sees its profile without an avatar (ADR 0121).
    Departed,
    Unrelated,
}

/// What the actor leaves (ADR 0121).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Departing {
    /// A workspace, while the actor stays a member of another.
    Workspace,
    /// The server: the last workspace of the actor, or the account itself. The server owner
    /// must first hand the position to another account.
    Server,
}

/// How the actor stands to the other account of a direct message or a direct call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contact {
    /// The two accounts are members of at least one common workspace.
    SharedWorkspace,
    /// The two accounts have no common workspace, and the other account is the server owner
    /// or holds the administrator permission.
    Administrator,
    /// The two accounts have no common workspace, and the other account does not administer
    /// the server.
    Unrelated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    CreateInvite,
    ApproveJoinRequest,
}

impl Admission {
    pub const fn permission(self) -> ServerPermission {
        match self {
            Self::CreateInvite => ServerPermission::CreateInvites,
            Self::ApproveJoinRequest => ServerPermission::ApproveJoinRequests,
        }
    }
}

/// Whether every workspace that an invite or an approval grants is one the actor is a member of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    WithinMemberships,
    BeyondMemberships,
}

/// The permissions that the channel overrides of a role allow in one channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelGrant<'a> {
    pub scope: &'a ChannelScope,
    pub granting: WorkspacePermissions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditScope<'a> {
    Server,
    Workspace(&'a WorkspaceScope),
    Channel(&'a ChannelScope),
    DeletedChannel,
    /// An entry of a workspace that was deleted.
    DeletedWorkspace,
}

/// What the actor asks to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    /// An action on the actor's own account and devices.
    OwnAccount,
    /// Reading the profile of an account: its name and its avatar.
    ViewAccount(Acquaintance),
    Server(ServerPermission),
    /// A change of server roles that can add `granting` to an account.
    ManageServerRoles {
        granting: ServerPermissions,
    },
    /// An invite or an approval that grants workspaces.
    Admit {
        admission: Admission,
        reach: Reach,
    },
    /// A removal, a ban or an unban.
    Moderate {
        target: Target,
    },
    /// The actor leaves a workspace, or ends its presence on the server: it leaves its last
    /// workspace or deletes its account (ADR 0121).
    Depart(Departing),
    OwnerOnly(OwnerAction),
    ViewWorkspace(&'a WorkspaceScope),
    Workspace {
        scope: &'a WorkspaceScope,
        permission: WorkspacePermission,
    },
    /// A change of workspace roles that can add `granting` to a member in the workspace and
    /// `in_channels` to a member in single channels.
    ManageWorkspaceRoles {
        scope: &'a WorkspaceScope,
        granting: WorkspacePermissions,
        in_channels: &'a [ChannelGrant<'a>],
    },
    Channel {
        scope: &'a ChannelScope,
        permission: WorkspacePermission,
    },
    /// A change of channel overrides that can add `granting` to a member in the channel.
    ManageChannelOverrides {
        scope: &'a ChannelScope,
        granting: WorkspacePermissions,
    },
    DeleteChannel {
        scope: &'a ChannelScope,
        content: ChannelContent,
    },
    /// Deleting a workspace with every channel of it. `content` tells whether one of its
    /// channels contains permanent messages.
    DeleteWorkspace {
        content: ChannelContent,
    },
    DirectMessage(Participation),
    /// Exchanging direct messages or a direct call with another account.
    Contact(Contact),
    EditMessage {
        place: MessagePlace<'a>,
        permanence: Permanence,
        authorship: Authorship,
    },
    AuditEntry(AuditScope<'a>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(Denial),
}

impl Decision {
    pub const fn is_allowed(self) -> bool {
        matches!(self, Self::Allow)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Denial {
    NotParticipant,
    PermanentMessage,
    NotAuthor,
    OwnerOnly,
    OwnerProtected,
    OwnAccountTarget,
    NotMember,
    MissingServerPermission(ServerPermission),
    MissingWorkspacePermission(WorkspacePermission),
    ExceedsOwnPermissions,
}
