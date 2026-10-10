use std::collections::{HashMap, HashSet};

use junto_authz::Participation;
use junto_authz::{
    Acquaintance, Actor, ChannelScope, Contact, Membership, Override, Request, RoleOverride,
    ServerPermission, WorkspacePermission, WorkspaceScope, authorize,
};
use junto_protocol::account::AccountStatus;
use junto_protocol::call::CallPlace;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::{AccountId, ChannelId, ServerRoleId, WorkspaceId, WorkspaceRoleId};
use serde::{Deserialize, Serialize};

use crate::access::{role_key, stored_override};
use crate::db::StoredText;
use crate::events::hub::CallKey;

/// What a connection level receives is decided by the class of an event (ADR 0107).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Class {
    Control,
    General,
    DirectMessage,
    Mention,
    CallInvite,
}

impl Class {
    pub const ALL: [Self; 5] = [
        Self::Control,
        Self::General,
        Self::DirectMessage,
        Self::Mention,
        Self::CallInvite,
    ];

    /// The classes that a connection of `level` receives outside a call.
    pub fn received_at(level: Level) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|class| class.reaches(level))
            .collect()
    }

    pub const fn reaches(self, level: Level) -> bool {
        match level {
            Level::Full => true,
            Level::Background => !matches!(self, Self::General),
        }
    }

    pub const fn of(event: &Event) -> Self {
        match event {
            Event::DeviceAdded { .. }
            | Event::DeviceRenamed { .. }
            | Event::DeviceRemoved { .. }
            | Event::DeviceLoggedIn { .. }
            | Event::DeviceLoggedOut { .. }
            | Event::AccountRecovered { .. }
            | Event::PermissionsChanged { .. }
            | Event::WorkspaceMemberRemoved { .. }
            | Event::ChannelDeleted { .. }
            | Event::CallMoveRequested { .. }
            | Event::CallMoveResolved { .. }
            | Event::CallEntryAnswered { .. }
            | Event::OwnCallChanged { .. }
            | Event::CallAccessEnded {}
            | Event::DirectMuteSet { .. }
            | Event::DirectMuteRemoved { .. } => Self::Control,
            Event::CallInvited { .. } | Event::CallInvitationResolved { .. } => Self::CallInvite,
            Event::AccountAdded { .. }
            | Event::AccountProfileUpdated { .. }
            | Event::AccountStatusChanged { .. }
            | Event::AccountServerRolesChanged { .. }
            | Event::OwnerChanged { .. }
            | Event::ServerRoleCreated { .. }
            | Event::ServerRoleUpdated { .. }
            | Event::InviteCreated { .. }
            | Event::InviteRevoked { .. }
            | Event::InviteUsed { .. }
            | Event::InvitesOfCreatorRevoked { .. }
            | Event::JoinRequestCreated { .. }
            | Event::JoinRequestDecided { .. }
            | Event::WorkspaceCreated { .. }
            | Event::WorkspaceUpdated { .. }
            | Event::WorkspaceMemberAdded { .. }
            | Event::WorkspaceMemberRolesChanged { .. }
            | Event::WorkspaceRoleCreated { .. }
            | Event::WorkspaceRoleUpdated { .. }
            | Event::CategoryCreated { .. }
            | Event::CategoryUpdated { .. }
            | Event::ChannelCreated { .. }
            | Event::ChannelUpdated { .. }
            | Event::ChannelOverrideSet { .. }
            | Event::CallStarted { .. }
            | Event::CallEnded { .. }
            | Event::CallParticipantJoined { .. }
            | Event::CallParticipantUpdated { .. }
            | Event::CallParticipantLeft { .. }
            | Event::CallLockChanged { .. }
            | Event::CallMeetingStarted { .. }
            | Event::CallMeetingEnded { .. }
            | Event::CallSpeaking { .. }
            | Event::CallEntryRequested { .. }
            | Event::CallEntryResolved { .. } => Self::General,
        }
    }
}

impl StoredText for Class {
    fn stored(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::General => "general",
            Self::DirectMessage => "direct_message",
            Self::Mention => "mention",
            Self::CallInvite => "call_invite",
        }
    }

    fn from_stored(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.stored() == text)
    }
}

/// The override that a role had in a channel when the channel was deleted, with the
/// permission sets as their stored bits (ADR 0304).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptOverride {
    pub role_id: WorkspaceRoleId,
    pub is_everyone: bool,
    pub allow: i64,
    pub deny: i64,
}

/// How far an event about the call of a voice channel reaches (ADR 0504, ADR 0510).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallReach {
    /// The accounts that may see who is inside: every account that may see the channel, or
    /// only the accounts inside when the channel hides who is inside.
    Occupancy,
    /// The accounts inside the call.
    Inside,
}

/// One voice channel that an event for one account names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seen {
    pub workspace_id: WorkspaceId,
    pub channel_id: ChannelId,
}

/// Who may receive an event. It is stored with the event and decided for each recipient when
/// the event is delivered (ADR 0011).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "to", rename_all = "snake_case")]
pub enum Audience {
    Account {
        account_id: AccountId,
    },
    /// One account, while it may see every channel in `channels` and may reach every
    /// account in `accounts` (ADR 0307). An event for one account that is about a voice
    /// channel or about a direct-message call has this audience, so that it does not tell an
    /// account about a place that it can no longer be at.
    AccountSeeing {
        account_id: AccountId,
        #[serde(default)]
        channels: Vec<Seen>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        accounts: Vec<AccountId>,
    },
    /// The accounts that may see the profile of the account: the account itself, the members
    /// of a workspace that it is a member of, and the accounts with `ban_members`.
    AccountViewers {
        account_id: AccountId,
    },
    ServerPermission {
        permission: ServerPermission,
    },
    ServerRoleHolders {
        role_id: ServerRoleId,
    },
    Workspace {
        workspace_id: WorkspaceId,
    },
    /// The members of the workspace who hold the role; every member for the "everyone" role.
    WorkspaceRoleHolders {
        workspace_id: WorkspaceId,
        role_id: WorkspaceRoleId,
    },
    Channel {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        permission: WorkspacePermission,
    },
    /// The accounts that could see the channel with the overrides it had at its deletion.
    DeletedChannel {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        overrides: Vec<KeptOverride>,
    },
    /// Accounts that may see the voice channel, as far as `reach` states. The setting that
    /// hides who is inside and the people inside are read when the event is delivered.
    Call {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        reach: CallReach,
    },
    /// The two participants of a direct message.
    Direct {
        low_account_id: AccountId,
        high_account_id: AccountId,
    },
}

/// The audience of an event about a call at `place`.
fn of_call(place: &CallPlace, reach: Option<CallReach>) -> Audience {
    match (*place, reach) {
        (
            CallPlace::Channel {
                workspace_id,
                channel_id,
            },
            None,
        ) => Audience::Channel {
            workspace_id,
            channel_id,
            permission: WorkspacePermission::ViewChannel,
        },
        (
            CallPlace::Channel {
                workspace_id,
                channel_id,
            },
            Some(reach),
        ) => Audience::Call {
            workspace_id,
            channel_id,
            reach,
        },
        (
            CallPlace::Direct {
                low_account_id,
                high_account_id,
            },
            _,
        ) => Audience::Direct {
            low_account_id,
            high_account_id,
        },
    }
}

impl Audience {
    pub const fn account_id(&self) -> Option<AccountId> {
        match self {
            Self::Account { account_id } | Self::AccountSeeing { account_id, .. } => {
                Some(*account_id)
            }
            _ => None,
        }
    }

    /// The audience of an event for one account: the account alone, or the account while it
    /// may be at the places that the event names.
    pub fn addressed(account_id: AccountId, event: &Event) -> Self {
        let named: Vec<CallPlace> = match event {
            Event::CallInvited { invitation } => vec![invitation.place],
            Event::CallMoveRequested { call_move } => vec![call_move.from, call_move.to],
            Event::OwnCallChanged {
                call: Some(own), ..
            } => vec![own.place],
            _ => Vec::new(),
        };
        Self::addressed_at(account_id, &named)
    }

    /// The audience of an event for one account about `places`: the account, while it may
    /// see every voice channel among them and may reach the other account of every direct
    /// message among them.
    pub fn addressed_at(account_id: AccountId, places: &[CallPlace]) -> Self {
        let mut channels: Vec<Seen> = Vec::new();
        let mut accounts: Vec<AccountId> = Vec::new();
        for place in places {
            match *place {
                CallPlace::Channel {
                    workspace_id,
                    channel_id,
                } => {
                    let seen = Seen {
                        workspace_id,
                        channel_id,
                    };
                    if !channels.contains(&seen) {
                        channels.push(seen);
                    }
                }
                CallPlace::Direct {
                    low_account_id,
                    high_account_id,
                } => {
                    for other in [low_account_id, high_account_id] {
                        if other != account_id && !accounts.contains(&other) {
                            accounts.push(other);
                        }
                    }
                }
            }
        }
        if channels.is_empty() && accounts.is_empty() {
            Self::Account { account_id }
        } else {
            Self::AccountSeeing {
                account_id,
                channels,
                accounts,
            }
        }
    }

    /// The accounts that a recipient must be able to reach to receive an event with this
    /// audience, other than the recipient.
    pub fn reached_account_ids(&self, recipient: AccountId) -> Vec<AccountId> {
        match self {
            Self::AccountSeeing { accounts, .. } => accounts.clone(),
            Self::Direct {
                low_account_id,
                high_account_id,
            } => [*low_account_id, *high_account_id]
                .into_iter()
                .filter(|account_id| *account_id != recipient)
                .collect(),
            _ => Vec::new(),
        }
    }

    /// The workspaces and channels whose standing a decision about this audience reads.
    pub fn places(&self) -> Vec<(WorkspaceId, Option<ChannelId>)> {
        match self {
            Self::AccountSeeing { channels, .. } => channels
                .iter()
                .map(|seen| (seen.workspace_id, Some(seen.channel_id)))
                .collect(),
            _ => self
                .workspace_id()
                .map(|workspace_id| (workspace_id, self.channel_id()))
                .into_iter()
                .collect(),
        }
    }

    pub const fn workspace_id(&self) -> Option<WorkspaceId> {
        match self {
            Self::Workspace { workspace_id }
            | Self::WorkspaceRoleHolders { workspace_id, .. }
            | Self::Channel { workspace_id, .. }
            | Self::DeletedChannel { workspace_id, .. }
            | Self::Call { workspace_id, .. } => Some(*workspace_id),
            Self::Account { .. }
            | Self::AccountSeeing { .. }
            | Self::AccountViewers { .. }
            | Self::ServerPermission { .. }
            | Self::ServerRoleHolders { .. }
            | Self::Direct { .. } => None,
        }
    }

    /// The account whose viewers the audience names.
    pub const fn viewed_account_id(&self) -> Option<AccountId> {
        match self {
            Self::AccountViewers { account_id } => Some(*account_id),
            _ => None,
        }
    }

    pub const fn channel_id(&self) -> Option<ChannelId> {
        match self {
            Self::Channel { channel_id, .. }
            | Self::DeletedChannel { channel_id, .. }
            | Self::Call { channel_id, .. } => Some(*channel_id),
            _ => None,
        }
    }

    /// The call whose events the audience carries, for the rule that an account in a call
    /// receives every event of that call (ADR 0107).
    pub const fn call_key(&self) -> Option<CallKey> {
        match self {
            Self::Channel { channel_id, .. } | Self::Call { channel_id, .. } => {
                Some(CallKey::Channel(*channel_id))
            }
            Self::Direct {
                low_account_id,
                high_account_id,
            } => Some(CallKey::Direct(*low_account_id, *high_account_id)),
            _ => None,
        }
    }

    /// The audience that the content of an event fixes. `None` for an event whose audience
    /// the writer states.
    pub fn fixed_by(event: &Event) -> Option<Self> {
        let permission = |permission| Some(Self::ServerPermission { permission });
        let workspace = |workspace_id| Some(Self::Workspace { workspace_id });
        match event {
            Event::DeviceAdded { .. }
            | Event::DeviceRenamed { .. }
            | Event::DeviceRemoved { .. }
            | Event::DeviceLoggedIn { .. }
            | Event::DeviceLoggedOut { .. }
            | Event::AccountRecovered { .. }
            | Event::PermissionsChanged { .. }
            | Event::ChannelDeleted { .. }
            | Event::CallEntryRequested { .. }
            | Event::CallEntryResolved { .. }
            | Event::CallEntryAnswered { .. }
            | Event::CallInvited { .. }
            | Event::CallInvitationResolved { .. }
            | Event::CallMoveRequested { .. }
            | Event::CallMoveResolved { .. }
            | Event::OwnCallChanged { .. }
            | Event::CallAccessEnded {}
            | Event::DirectMuteSet { .. }
            | Event::DirectMuteRemoved { .. } => None,
            Event::CallStarted { place, .. } | Event::CallEnded { place, .. } => {
                Some(of_call(place, None))
            }
            Event::CallParticipantJoined { place, .. }
            | Event::CallParticipantUpdated { place, .. }
            | Event::CallParticipantLeft { place, .. }
            | Event::CallLockChanged { place, .. }
            | Event::CallMeetingStarted { place, .. }
            | Event::CallMeetingEnded { place, .. }
            | Event::CallSpeaking { place, .. } => Some(of_call(place, Some(CallReach::Occupancy))),
            Event::AccountAdded { .. }
            | Event::AccountStatusChanged { .. }
            | Event::AccountServerRolesChanged { .. }
            | Event::OwnerChanged { .. } => permission(ServerPermission::BanMembers),
            Event::AccountProfileUpdated { profile } => Some(Self::AccountViewers {
                account_id: profile.id,
            }),
            Event::ServerRoleCreated { .. } | Event::ServerRoleUpdated { .. } => {
                permission(ServerPermission::Administrator)
            }
            Event::InviteCreated { .. }
            | Event::InviteRevoked { .. }
            | Event::InviteUsed { .. }
            | Event::InvitesOfCreatorRevoked { .. } => permission(ServerPermission::CreateInvites),
            Event::JoinRequestCreated { .. } | Event::JoinRequestDecided { .. } => {
                permission(ServerPermission::ApproveJoinRequests)
            }
            Event::WorkspaceCreated { workspace: changed }
            | Event::WorkspaceUpdated { workspace: changed } => workspace(changed.id),
            Event::WorkspaceMemberAdded { workspace_id, .. }
            | Event::WorkspaceMemberRemoved { workspace_id, .. }
            | Event::WorkspaceMemberRolesChanged { workspace_id, .. } => workspace(*workspace_id),
            Event::WorkspaceRoleCreated { role } | Event::WorkspaceRoleUpdated { role } => {
                workspace(role.workspace_id)
            }
            Event::CategoryCreated { category } | Event::CategoryUpdated { category } => {
                workspace(category.workspace_id)
            }
            Event::ChannelCreated { channel } | Event::ChannelUpdated { channel } => {
                Some(Self::Channel {
                    workspace_id: channel.workspace_id,
                    channel_id: channel.id,
                    permission: WorkspacePermission::ViewChannel,
                })
            }
            Event::ChannelOverrideSet {
                workspace_id,
                channel_id,
                ..
            } => Some(Self::Channel {
                workspace_id: *workspace_id,
                channel_id: *channel_id,
                permission: WorkspacePermission::ManageChannel,
            }),
        }
    }

    /// Whether a writer may state this audience for `event`.
    pub fn fits(&self, event: &Event) -> bool {
        match (event, self) {
            (
                Event::DeviceAdded { .. }
                | Event::DeviceRenamed { .. }
                | Event::DeviceRemoved { .. }
                | Event::DeviceLoggedIn { .. }
                | Event::DeviceLoggedOut { .. }
                | Event::AccountRecovered { .. },
                Self::Account { .. },
            ) => true,
            (
                Event::PermissionsChanged { .. }
                | Event::CallAccessEnded {}
                | Event::DirectMuteSet { .. }
                | Event::DirectMuteRemoved { .. },
                Self::Account { .. },
            ) => true,
            (
                Event::CallInvited { .. }
                | Event::CallMoveRequested { .. }
                | Event::OwnCallChanged { call: Some(_), .. },
                _,
            ) => self
                .account_id()
                .is_some_and(|account_id| *self == Self::addressed(account_id, event)),
            (Event::OwnCallChanged { call: None, .. }, Self::Account { .. }) => true,
            (
                Event::CallInvitationResolved { .. }
                | Event::CallMoveResolved { .. }
                | Event::CallEntryAnswered { .. },
                Self::Account { .. } | Self::AccountSeeing { .. },
            ) => true,
            (
                Event::CallEntryRequested { request },
                Self::Call {
                    reach: CallReach::Inside,
                    ..
                },
            ) => *self == of_call(&request.place, Some(CallReach::Inside)),
            (
                Event::CallEntryResolved { place, .. },
                Self::Call {
                    reach: CallReach::Inside,
                    ..
                },
            ) => *self == of_call(place, Some(CallReach::Inside)),
            (Event::PermissionsChanged { workspace_id: None }, Self::ServerRoleHolders { .. }) => {
                true
            }
            (
                Event::PermissionsChanged {
                    workspace_id: Some(named),
                },
                Self::WorkspaceRoleHolders { workspace_id, .. },
            ) => named == workspace_id,
            (
                Event::ChannelDeleted {
                    workspace_id: deleted_in,
                    channel_id: deleted,
                },
                Self::DeletedChannel {
                    workspace_id,
                    channel_id,
                    ..
                },
            ) => deleted_in == workspace_id && deleted == channel_id,
            _ => Self::fixed_by(event).as_ref() == Some(self),
        }
    }
}

/// The account whose connections check their session again when the event is recorded,
/// because the change behind the event can have ended it.
pub fn revalidated_account(audience: &Audience, event: &Event) -> Option<AccountId> {
    match event {
        Event::DeviceRemoved { .. }
        | Event::DeviceLoggedOut { .. }
        | Event::AccountRecovered { .. } => audience.account_id(),
        Event::AccountStatusChanged {
            account_id,
            status: AccountStatus::Banned | AccountStatus::Removed,
        } => Some(*account_id),
        _ => None,
    }
}

/// The recipient of a delivery, as the permission function needs it.
#[derive(Debug, Clone, Copy)]
pub struct Recipient<'a> {
    pub account_id: AccountId,
    pub actor: &'a Actor,
    pub server_role_ids: &'a [ServerRoleId],
    /// The accounts, of the ones that the audiences of a batch name, that are members of a
    /// workspace that the recipient is a member of.
    pub fellows: &'a HashSet<AccountId>,
    /// How the recipient stands to the accounts that the audiences of a batch name for a
    /// direct message or a direct call. An account that is not active has no entry.
    pub contacts: &'a HashMap<AccountId, Contact>,
}

impl Recipient<'_> {
    fn acquaintance(&self, account_id: AccountId) -> Acquaintance {
        if account_id == self.account_id {
            Acquaintance::Own
        } else if self.fellows.contains(&account_id) {
            Acquaintance::SharedWorkspace
        } else {
            Acquaintance::Unrelated
        }
    }
}

/// One channel of a workspace as the recipient stands in it now.
#[derive(Debug, Clone)]
pub struct ChannelStanding {
    pub deleted: bool,
    pub scope: ChannelScope,
    /// The channel shows who is inside only to the people inside.
    pub hides_occupants: bool,
    /// The recipient is inside the call of the channel.
    pub inside: bool,
}

/// One workspace as the recipient stands in it now.
#[derive(Debug, Clone)]
pub struct WorkspaceStanding {
    pub scope: WorkspaceScope,
    pub everyone_role_id: WorkspaceRoleId,
    pub channels: std::collections::HashMap<ChannelId, ChannelStanding>,
}

impl WorkspaceStanding {
    fn holds(&self, role_id: WorkspaceRoleId) -> bool {
        self.scope.membership == Membership::Member
            && (role_id == self.everyone_role_id
                || self
                    .scope
                    .roles
                    .iter()
                    .any(|role| role.key == role_key(role_id)))
    }

    fn kept_scope(&self, overrides: &[KeptOverride]) -> Option<ChannelScope> {
        let mut scope = ChannelScope {
            workspace: self.scope.clone(),
            everyone_override: Override::NONE,
            role_overrides: Vec::new(),
        };
        for kept in overrides {
            let rule = stored_override(kept.allow, kept.deny).ok()?;
            if kept.is_everyone {
                scope.everyone_override = rule;
            } else {
                scope.role_overrides.push(RoleOverride {
                    role: role_key(kept.role_id),
                    rule,
                });
            }
        }
        Some(scope)
    }
}

/// Decides whether `recipient` receives an event with `audience`. `workspace` is the standing
/// of the recipient in the workspace that the audience names, absent when the audience names
/// none or the workspace does not exist. Every permission decision is made by the permission
/// function of ADR 0301.
pub fn may_receive(
    recipient: Recipient<'_>,
    audience: &Audience,
    workspace: Option<&WorkspaceStanding>,
) -> bool {
    receives(recipient, audience, &|_| workspace)
}

/// Like [`may_receive`], for an audience that can name channels of several workspaces:
/// `standing` gives the standing of the recipient in one workspace.
pub fn receives<'a>(
    recipient: Recipient<'_>,
    audience: &Audience,
    standing: &dyn Fn(WorkspaceId) -> Option<&'a WorkspaceStanding>,
) -> bool {
    let allows = |request: Request<'_>| authorize(recipient.actor, &request).is_allowed();
    let reaches = |other: AccountId| {
        recipient
            .contacts
            .get(&other)
            .is_some_and(|contact| allows(Request::Contact(*contact)))
    };
    let workspace = audience.workspace_id().and_then(standing);
    match audience {
        Audience::Account { account_id } => {
            *account_id == recipient.account_id && allows(Request::OwnAccount)
        }
        Audience::AccountSeeing {
            account_id,
            channels,
            accounts,
        } => {
            *account_id == recipient.account_id
                && allows(Request::OwnAccount)
                && channels.iter().all(|seen| {
                    standing(seen.workspace_id)
                        .and_then(|workspace| workspace.channels.get(&seen.channel_id))
                        .is_some_and(|channel| {
                            !channel.deleted
                                && allows(Request::Channel {
                                    scope: &channel.scope,
                                    permission: WorkspacePermission::ViewChannel,
                                })
                        })
                })
                && accounts.iter().all(|other| reaches(*other))
        }
        Audience::AccountViewers { account_id } => {
            allows(Request::ViewAccount(recipient.acquaintance(*account_id)))
        }
        Audience::ServerPermission { permission } => allows(Request::Server(*permission)),
        Audience::ServerRoleHolders { role_id } => {
            recipient.server_role_ids.contains(role_id) && allows(Request::OwnAccount)
        }
        Audience::Workspace { .. } => {
            workspace.is_some_and(|standing| allows(Request::ViewWorkspace(&standing.scope)))
        }
        Audience::WorkspaceRoleHolders { role_id, .. } => workspace.is_some_and(|standing| {
            standing.holds(*role_id) && allows(Request::ViewWorkspace(&standing.scope))
        }),
        Audience::Channel {
            channel_id,
            permission,
            ..
        } => workspace
            .and_then(|standing| standing.channels.get(channel_id))
            .is_some_and(|channel| {
                !channel.deleted
                    && allows(Request::Channel {
                        scope: &channel.scope,
                        permission: *permission,
                    })
            }),
        Audience::DeletedChannel { overrides, .. } => workspace
            .and_then(|standing| standing.kept_scope(overrides))
            .is_some_and(|scope| {
                allows(Request::Channel {
                    scope: &scope,
                    permission: WorkspacePermission::ViewChannel,
                })
            }),
        Audience::Call {
            channel_id, reach, ..
        } => workspace
            .and_then(|standing| standing.channels.get(channel_id))
            .is_some_and(|channel| {
                let reached = match reach {
                    CallReach::Occupancy => !channel.hides_occupants || channel.inside,
                    CallReach::Inside => channel.inside,
                };
                reached
                    && !channel.deleted
                    && allows(Request::Channel {
                        scope: &channel.scope,
                        permission: WorkspacePermission::ViewChannel,
                    })
            }),
        Audience::Direct {
            low_account_id,
            high_account_id,
        } => {
            let other = match recipient.account_id {
                own if own == *low_account_id => *high_account_id,
                own if own == *high_account_id => *low_account_id,
                _ => return allows(Request::DirectMessage(Participation::Outsider)),
            };
            allows(Request::DirectMessage(Participation::Participant)) && reaches(other)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use junto_authz::{HeldRole, Position, ServerPermissions, WorkspacePermissions};
    use junto_protocol::ids::DeviceId;
    use junto_protocol::structure::{Channel, ChannelKind};

    use super::*;

    fn member() -> Actor {
        Actor {
            position: Position::Member,
            server_roles: vec![],
        }
    }

    fn administrator() -> Actor {
        Actor {
            position: Position::Member,
            server_roles: vec![ServerPermissions::from(ServerPermission::Administrator)],
        }
    }

    static NO_FELLOWS: std::sync::LazyLock<HashSet<AccountId>> =
        std::sync::LazyLock::new(HashSet::new);
    static NO_CONTACTS: std::sync::LazyLock<HashMap<AccountId, Contact>> =
        std::sync::LazyLock::new(HashMap::new);

    fn recipient(actor: &Actor) -> Recipient<'_> {
        Recipient {
            account_id: AccountId::generate(),
            actor,
            server_role_ids: &[],
            fellows: &NO_FELLOWS,
            contacts: &NO_CONTACTS,
        }
    }

    struct Place {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        everyone_role_id: WorkspaceRoleId,
        role_id: WorkspaceRoleId,
    }

    fn place() -> Place {
        Place {
            workspace_id: WorkspaceId::generate(),
            channel_id: ChannelId::generate(),
            everyone_role_id: WorkspaceRoleId::generate(),
            role_id: WorkspaceRoleId::generate(),
        }
    }

    fn deny_view() -> Override {
        Override::new(
            WorkspacePermissions::EMPTY,
            WorkspacePermission::ViewChannel.into(),
        )
        .unwrap()
    }

    fn allow_view() -> Override {
        Override::new(
            WorkspacePermission::ViewChannel.into(),
            WorkspacePermissions::EMPTY,
        )
        .unwrap()
    }

    /// The standing of a member of the workspace, with the role when `with_role`, in a channel
    /// that is private when `private` and open to the role.
    fn standing(
        place: &Place,
        membership: Membership,
        with_role: bool,
        private: bool,
    ) -> WorkspaceStanding {
        let scope = WorkspaceScope {
            membership,
            everyone: WorkspacePermissions::EVERYONE_DEFAULT,
            roles: if with_role {
                vec![HeldRole {
                    key: role_key(place.role_id),
                    permissions: WorkspacePermissions::EMPTY,
                }]
            } else {
                vec![]
            },
        };
        let channel = ChannelStanding {
            deleted: false,
            hides_occupants: false,
            inside: false,
            scope: ChannelScope {
                workspace: scope.clone(),
                everyone_override: if private { deny_view() } else { Override::NONE },
                role_overrides: vec![RoleOverride {
                    role: role_key(place.role_id),
                    rule: allow_view(),
                }],
            },
        };
        WorkspaceStanding {
            scope,
            everyone_role_id: place.everyone_role_id,
            channels: HashMap::from([(place.channel_id, channel)]),
        }
    }

    fn channel_audience(place: &Place, permission: WorkspacePermission) -> Audience {
        Audience::Channel {
            workspace_id: place.workspace_id,
            channel_id: place.channel_id,
            permission,
        }
    }

    #[test]
    fn the_background_level_receives_every_class_except_the_general_one() {
        for class in Class::ALL {
            assert!(class.reaches(Level::Full));
            assert_eq!(class.reaches(Level::Background), class != Class::General);
            assert_eq!(Class::from_stored(class.stored()), Some(class));
        }
        assert_eq!(Class::received_at(Level::Full).len(), 5);
        assert_eq!(
            Class::received_at(Level::Background),
            [
                Class::Control,
                Class::DirectMessage,
                Class::Mention,
                Class::CallInvite
            ]
        );
    }

    #[test]
    fn an_account_event_reaches_only_its_account() {
        let actor = administrator();
        let me = recipient(&actor);
        let mine = Audience::Account {
            account_id: me.account_id,
        };
        let other = Audience::Account {
            account_id: AccountId::generate(),
        };
        assert!(may_receive(me, &mine, None));
        assert!(!may_receive(me, &other, None));
    }

    #[test]
    fn a_profile_event_reaches_the_account_its_fellow_members_and_moderators() {
        let subject = AccountId::generate();
        let audience = Audience::AccountViewers {
            account_id: subject,
        };
        let plain = member();
        let stranger = recipient(&plain);
        assert!(!may_receive(stranger, &audience, None));

        let fellows = HashSet::from([subject]);
        let fellow = Recipient {
            fellows: &fellows,
            ..recipient(&plain)
        };
        assert!(may_receive(fellow, &audience, None));

        let own = Recipient {
            account_id: subject,
            ..recipient(&plain)
        };
        assert!(may_receive(own, &audience, None));

        let moderator = Actor {
            position: Position::Member,
            server_roles: vec![ServerPermissions::from(ServerPermission::BanMembers)],
        };
        assert!(may_receive(recipient(&moderator), &audience, None));
        assert!(may_receive(recipient(&administrator()), &audience, None));
    }

    #[test]
    fn a_server_event_needs_its_server_permission() {
        let audience = Audience::ServerPermission {
            permission: ServerPermission::CreateInvites,
        };
        let plain = member();
        let inviter = Actor {
            position: Position::Member,
            server_roles: vec![ServerPermissions::from(ServerPermission::CreateInvites)],
        };
        let owner = Actor {
            position: Position::Owner,
            server_roles: vec![],
        };
        assert!(!may_receive(recipient(&plain), &audience, None));
        assert!(may_receive(recipient(&inviter), &audience, None));
        assert!(may_receive(recipient(&owner), &audience, None));
        assert!(may_receive(recipient(&administrator()), &audience, None));
    }

    #[test]
    fn a_server_role_notice_reaches_the_holders_of_the_role() {
        let role_id = ServerRoleId::generate();
        let audience = Audience::ServerRoleHolders { role_id };
        let actor = member();
        let mut holder = recipient(&actor);
        assert!(!may_receive(holder, &audience, None));
        let held = [role_id];
        holder.server_role_ids = &held;
        assert!(may_receive(holder, &audience, None));
    }

    #[test]
    fn a_workspace_event_needs_membership_or_the_administrator_permission() {
        let place = place();
        let audience = Audience::Workspace {
            workspace_id: place.workspace_id,
        };
        let inside = standing(&place, Membership::Member, false, false);
        let outside = standing(&place, Membership::NotMember, false, false);
        let plain = member();
        assert!(may_receive(recipient(&plain), &audience, Some(&inside)));
        assert!(!may_receive(recipient(&plain), &audience, Some(&outside)));
        assert!(!may_receive(recipient(&plain), &audience, None));
        assert!(may_receive(
            recipient(&administrator()),
            &audience,
            Some(&outside)
        ));
    }

    #[test]
    fn a_role_notice_reaches_members_who_hold_the_role() {
        let place = place();
        let of_role = Audience::WorkspaceRoleHolders {
            workspace_id: place.workspace_id,
            role_id: place.role_id,
        };
        let of_everyone = Audience::WorkspaceRoleHolders {
            workspace_id: place.workspace_id,
            role_id: place.everyone_role_id,
        };
        let plain = member();
        let me = recipient(&plain);
        let holder = standing(&place, Membership::Member, true, false);
        let other = standing(&place, Membership::Member, false, false);
        let outside = standing(&place, Membership::NotMember, false, false);
        assert!(may_receive(me, &of_role, Some(&holder)));
        assert!(!may_receive(me, &of_role, Some(&other)));
        assert!(may_receive(me, &of_everyone, Some(&other)));
        assert!(!may_receive(me, &of_everyone, Some(&outside)));
        assert!(!may_receive(
            recipient(&administrator()),
            &of_everyone,
            Some(&outside)
        ));
    }

    #[test]
    fn a_channel_event_needs_sight_of_the_channel_and_the_named_permission() {
        let place = place();
        let view = channel_audience(&place, WorkspacePermission::ViewChannel);
        let manage = channel_audience(&place, WorkspacePermission::ManageChannel);
        let plain = member();
        let me = recipient(&plain);

        let public = standing(&place, Membership::Member, false, false);
        assert!(may_receive(me, &view, Some(&public)));
        assert!(!may_receive(me, &manage, Some(&public)));

        let private = standing(&place, Membership::Member, false, true);
        assert!(!may_receive(me, &view, Some(&private)));
        let private_with_role = standing(&place, Membership::Member, true, true);
        assert!(may_receive(me, &view, Some(&private_with_role)));
        assert!(may_receive(
            recipient(&administrator()),
            &view,
            Some(&private)
        ));
        assert!(may_receive(
            recipient(&administrator()),
            &manage,
            Some(&private)
        ));

        let elsewhere = channel_audience(&self::place(), WorkspacePermission::ViewChannel);
        assert!(!may_receive(me, &elsewhere, Some(&public)));
        assert!(!may_receive(me, &view, None));
    }

    #[test]
    fn who_is_inside_a_hidden_voice_channel_reaches_only_the_people_inside() {
        let place = place();
        let call = |reach| Audience::Call {
            workspace_id: place.workspace_id,
            channel_id: place.channel_id,
            reach,
        };
        let plain = member();
        let me = recipient(&plain);
        let standing_with = |hides_occupants, inside| {
            let mut standing = standing(&place, Membership::Member, false, false);
            let channel = standing.channels.get_mut(&place.channel_id).unwrap();
            channel.hides_occupants = hides_occupants;
            channel.inside = inside;
            standing
        };

        let open = standing_with(false, false);
        assert!(may_receive(me, &call(CallReach::Occupancy), Some(&open)));
        assert!(!may_receive(me, &call(CallReach::Inside), Some(&open)));

        let hidden = standing_with(true, false);
        assert!(!may_receive(me, &call(CallReach::Occupancy), Some(&hidden)));
        assert!(!may_receive(me, &call(CallReach::Inside), Some(&hidden)));
        for reach in [CallReach::Occupancy, CallReach::Inside] {
            assert!(!may_receive(
                recipient(&administrator()),
                &call(reach),
                Some(&hidden)
            ));
        }

        let inside = standing_with(true, true);
        assert!(may_receive(me, &call(CallReach::Occupancy), Some(&inside)));
        assert!(may_receive(me, &call(CallReach::Inside), Some(&inside)));

        let mut without_sight = standing(&place, Membership::Member, false, true);
        without_sight
            .channels
            .get_mut(&place.channel_id)
            .unwrap()
            .inside = true;
        assert!(!may_receive(
            me,
            &call(CallReach::Inside),
            Some(&without_sight)
        ));
        let mut deleted = standing_with(false, true);
        deleted.channels.get_mut(&place.channel_id).unwrap().deleted = true;
        assert!(!may_receive(
            me,
            &call(CallReach::Occupancy),
            Some(&deleted)
        ));
        assert!(!may_receive(me, &call(CallReach::Occupancy), None));
    }

    #[test]
    fn an_event_for_one_account_that_names_a_channel_needs_sight_of_that_channel() {
        use junto_protocol::call::{CallInvitation, OwnCall};
        use junto_protocol::ids::{CallId, CallInvitationId, DeviceId};

        let place = place();
        let other = self::place();
        let channel = |place: &Place| CallPlace::Channel {
            workspace_id: place.workspace_id,
            channel_id: place.channel_id,
        };
        let plain = member();
        let me = recipient(&plain);
        let own = |call| Event::OwnCallChanged { call, left: None };
        let joined = own(Some(OwnCall {
            call_id: CallId::generate(),
            place: channel(&place),
            device_id: DeviceId::generate(),
            identity: "seat".to_owned(),
        }));
        let audience = Audience::addressed(me.account_id, &joined);
        assert_eq!(
            audience,
            Audience::AccountSeeing {
                account_id: me.account_id,
                channels: vec![Seen {
                    workspace_id: place.workspace_id,
                    channel_id: place.channel_id
                }],
                accounts: vec![],
            }
        );
        assert_eq!(audience.account_id(), Some(me.account_id));
        assert!(audience.fits(&joined));
        assert!(
            !Audience::Account {
                account_id: me.account_id
            }
            .fits(&joined)
        );
        let left = own(None);
        assert_eq!(
            Audience::addressed(me.account_id, &left),
            Audience::Account {
                account_id: me.account_id
            }
        );
        assert!(!audience.fits(&left));

        let public = standing(&place, Membership::Member, false, false);
        let private = standing(&place, Membership::Member, false, true);
        assert!(may_receive(me, &audience, Some(&public)));
        assert!(!may_receive(me, &audience, Some(&private)));
        assert!(!may_receive(me, &audience, None));
        assert!(!may_receive(recipient(&plain), &audience, Some(&public)));

        let moved = Event::CallMoveRequested {
            call_move: junto_protocol::call::CallMove {
                id: junto_protocol::ids::CallMoveId::generate(),
                account_id: me.account_id,
                from: channel(&place),
                to: channel(&other),
                moved_by: AccountId::generate(),
                expires_at: time::OffsetDateTime::UNIX_EPOCH,
            },
        };
        let both = Audience::addressed(me.account_id, &moved);
        assert_eq!(both.places().len(), 2);
        let elsewhere = standing(&other, Membership::Member, false, false);
        let hidden_elsewhere = standing(&other, Membership::Member, false, true);
        let sees = |second: &WorkspaceStanding| {
            let lookup = |workspace_id| {
                if workspace_id == place.workspace_id {
                    Some(&public)
                } else if workspace_id == other.workspace_id {
                    Some(second)
                } else {
                    None
                }
            };
            receives(me, &both, &lookup)
        };
        assert!(sees(&elsewhere));
        assert!(!sees(&hidden_elsewhere));

        let caller = AccountId::generate();
        let direct = Event::CallInvited {
            invitation: CallInvitation {
                id: CallInvitationId::generate(),
                call_id: CallId::generate(),
                place: CallPlace::direct(me.account_id, caller),
                account_id: me.account_id,
                invited_by: caller,
                created_at: time::OffsetDateTime::UNIX_EPOCH,
                expires_at: time::OffsetDateTime::UNIX_EPOCH,
            },
        };
        let rung = Audience::addressed(me.account_id, &direct);
        assert_eq!(
            rung,
            Audience::AccountSeeing {
                account_id: me.account_id,
                channels: vec![],
                accounts: vec![caller],
            }
        );
        assert!(!may_receive(me, &rung, None));
        for (contact, reaches) in [
            (Contact::SharedWorkspace, true),
            (Contact::Administrator, true),
            (Contact::Unrelated, false),
        ] {
            let contacts = HashMap::from([(caller, contact)]);
            let near = Recipient {
                contacts: &contacts,
                ..me
            };
            assert_eq!(may_receive(near, &rung, None), reaches, "{contact:?}");
        }
    }

    #[test]
    fn an_event_of_a_direct_call_reaches_its_two_participants_while_they_reach_each_other() {
        let (low, high) = (AccountId::generate(), AccountId::generate());
        let audience = Audience::Direct {
            low_account_id: low.min(high),
            high_account_id: low.max(high),
        };
        let plain = member();
        let manager = administrator();
        for (participant, other) in [(low, high), (high, low)] {
            let me = Recipient {
                account_id: participant,
                ..recipient(&plain)
            };
            assert!(!may_receive(me, &audience, None));
            for (contact, reaches) in [
                (Contact::SharedWorkspace, true),
                (Contact::Administrator, true),
                (Contact::Unrelated, false),
            ] {
                let contacts = HashMap::from([(other, contact)]);
                let near = Recipient {
                    contacts: &contacts,
                    ..me
                };
                assert_eq!(may_receive(near, &audience, None), reaches, "{contact:?}");
                let administering = Recipient {
                    actor: &manager,
                    ..near
                };
                assert!(may_receive(administering, &audience, None));
            }
        }
        let owner = Actor {
            position: Position::Owner,
            server_roles: vec![],
        };
        let everyone = HashMap::from([
            (low, Contact::SharedWorkspace),
            (high, Contact::SharedWorkspace),
        ]);
        for outsider in [plain.clone(), administrator(), owner] {
            let near = Recipient {
                contacts: &everyone,
                ..recipient(&outsider)
            };
            assert!(!may_receive(near, &audience, None));
        }
        assert_eq!(
            audience.call_key(),
            Some(CallKey::Direct(low.min(high), low.max(high)))
        );
    }

    #[test]
    fn an_event_about_a_deleted_channel_reaches_nobody_except_its_deletion() {
        let place = place();
        let mut gone = standing(&place, Membership::Member, true, false);
        gone.channels.get_mut(&place.channel_id).unwrap().deleted = true;
        gone.channels
            .get_mut(&place.channel_id)
            .unwrap()
            .scope
            .role_overrides
            .clear();
        let view = crate::db::workspace_bits(WorkspacePermission::ViewChannel.into());
        let seeing = channel_audience(&place, WorkspacePermission::ViewChannel);
        let plain = member();
        assert!(!may_receive(recipient(&plain), &seeing, Some(&gone)));
        assert!(!may_receive(
            recipient(&administrator()),
            &seeing,
            Some(&gone)
        ));

        let deletion = Audience::DeletedChannel {
            workspace_id: place.workspace_id,
            channel_id: place.channel_id,
            overrides: vec![
                KeptOverride {
                    role_id: place.everyone_role_id,
                    is_everyone: true,
                    allow: 0,
                    deny: view,
                },
                KeptOverride {
                    role_id: place.role_id,
                    is_everyone: false,
                    allow: view,
                    deny: 0,
                },
            ],
        };
        let without_role = standing(&place, Membership::Member, false, false);
        assert!(may_receive(recipient(&plain), &deletion, Some(&gone)));
        assert!(!may_receive(
            recipient(&plain),
            &deletion,
            Some(&without_role)
        ));
        assert!(may_receive(
            recipient(&administrator()),
            &deletion,
            Some(&without_role)
        ));

        let broken = Audience::DeletedChannel {
            workspace_id: place.workspace_id,
            channel_id: place.channel_id,
            overrides: vec![KeptOverride {
                role_id: place.role_id,
                is_everyone: false,
                allow: view,
                deny: view,
            }],
        };
        assert!(!may_receive(recipient(&plain), &broken, Some(&gone)));
    }

    #[test]
    fn a_stated_audience_must_fit_its_event() {
        let place = place();
        let account = Audience::Account {
            account_id: AccountId::generate(),
        };
        let workspace = Audience::Workspace {
            workspace_id: place.workspace_id,
        };
        let removed = Event::DeviceRemoved {
            device_id: DeviceId::generate(),
        };
        assert!(account.fits(&removed));
        assert!(!workspace.fits(&removed));

        let in_workspace = Event::PermissionsChanged {
            workspace_id: Some(place.workspace_id),
        };
        let on_server = Event::PermissionsChanged { workspace_id: None };
        let holders = Audience::WorkspaceRoleHolders {
            workspace_id: place.workspace_id,
            role_id: place.role_id,
        };
        let server_holders = Audience::ServerRoleHolders {
            role_id: ServerRoleId::generate(),
        };
        assert!(holders.fits(&in_workspace));
        assert!(!holders.fits(&on_server));
        assert!(server_holders.fits(&on_server));
        assert!(!server_holders.fits(&in_workspace));
        assert!(account.fits(&in_workspace) && account.fits(&on_server));
        assert!(!workspace.fits(&in_workspace));

        let channel = Channel {
            id: place.channel_id,
            workspace_id: place.workspace_id,
            category_id: None,
            kind: ChannelKind::Text,
            name: "plans".to_owned(),
            position: 0,
            permanent_messages: false,
            user_limit: None,
            hides_occupants: false,
        };
        let created = Event::ChannelCreated { channel };
        assert!(channel_audience(&place, WorkspacePermission::ViewChannel).fits(&created));
        assert!(!channel_audience(&place, WorkspacePermission::ManageChannel).fits(&created));
        assert!(!workspace.fits(&created));

        let deleted = Event::ChannelDeleted {
            workspace_id: place.workspace_id,
            channel_id: place.channel_id,
        };
        let kept = |channel_id| Audience::DeletedChannel {
            workspace_id: place.workspace_id,
            channel_id,
            overrides: vec![],
        };
        assert!(kept(place.channel_id).fits(&deleted));
        assert!(!kept(ChannelId::generate()).fits(&deleted));
        assert!(!channel_audience(&place, WorkspacePermission::ViewChannel).fits(&deleted));
    }

    #[test]
    fn the_changes_that_can_end_a_session_name_their_account() {
        let account_id = AccountId::generate();
        let own = Audience::Account { account_id };
        let moderators = Audience::ServerPermission {
            permission: ServerPermission::BanMembers,
        };
        let device_id = DeviceId::generate();
        assert_eq!(
            revalidated_account(&own, &Event::DeviceRemoved { device_id }),
            Some(account_id)
        );
        assert_eq!(
            revalidated_account(&own, &Event::DeviceLoggedOut { device_id }),
            Some(account_id)
        );
        assert_eq!(
            revalidated_account(
                &own,
                &Event::DeviceRenamed {
                    device_id,
                    name: "desk".to_owned()
                }
            ),
            None
        );
        for (status, expected) in [
            (AccountStatus::Banned, Some(account_id)),
            (AccountStatus::Removed, Some(account_id)),
            (AccountStatus::Active, None),
        ] {
            let event = Event::AccountStatusChanged { account_id, status };
            assert_eq!(revalidated_account(&moderators, &event), expected);
        }
    }

    #[test]
    fn an_audience_is_stored_with_its_kind_and_its_identifiers() {
        let account_id = AccountId::generate();
        let stored = serde_json::to_value(Audience::Account { account_id }).unwrap();
        assert_eq!(stored["to"], "account");
        assert_eq!(stored["account_id"], account_id.to_string());
        let audience = Audience::ServerPermission {
            permission: ServerPermission::BanMembers,
        };
        let stored = serde_json::to_value(&audience).unwrap();
        assert_eq!(stored["permission"], "ban_members");
        assert_eq!(
            serde_json::from_value::<Audience>(stored).unwrap(),
            audience
        );
    }
}
