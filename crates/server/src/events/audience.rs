use std::collections::{HashMap, HashSet};

use junto_authz::Participation;
use junto_authz::{
    Acquaintance, Actor, ChannelScope, Contact, Membership, Override, Request, RoleOverride,
    ServerPermission, WorkspacePermission, WorkspaceScope, authorize,
};
use junto_protocol::account::AccountStatus;
use junto_protocol::call::CallPlace;
use junto_protocol::events::{Event, Level};
use junto_protocol::ids::{
    AccountId, CallId, CategoryId, ChannelId, ServerRoleId, WorkspaceId, WorkspaceRoleId,
};
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
            | Event::DirectMuteRemoved { .. }
            | Event::ThreadRead { .. }
            | Event::MessageRead { .. }
            | Event::NotificationSettingsChanged { .. }
            | Event::ServerLogoChanged { .. }
            | Event::AttachmentLimitsChanged { .. }
            | Event::WorkspaceDeleted { .. }
            | Event::ExportUpdated { .. }
            | Event::ExportRemoved { .. }
            | Event::NotificationsRead { .. }
            | Event::ThreadFollowChanged { .. }
            | Event::NotificationDefaultChanged { .. } => Self::Control,
            Event::ConversationOpened { .. } => Self::DirectMessage,
            Event::MessageMentioned { .. } | Event::NotificationChanged { .. } => Self::Mention,
            Event::CallInvited { .. } | Event::CallInvitationResolved { .. } => Self::CallInvite,
            Event::MessageChanged { place, .. } => match place {
                junto_protocol::message::MessagePlace::Direct { .. } => Self::DirectMessage,
                _ => Self::General,
            },
            Event::RetentionChanged { .. }
            | Event::AccountAdded { .. }
            | Event::AccountProfileUpdated { .. }
            | Event::AccountDepartureChanged { .. }
            | Event::AccountStatusChanged { .. }
            | Event::AccountServerRolesChanged { .. }
            | Event::OwnerChanged { .. }
            | Event::ServerRolePositionsShifted { .. }
            | Event::WorkspaceRolePositionsShifted { .. }
            | Event::ServerRoleCreated { .. }
            | Event::ServerRoleUpdated { .. }
            | Event::AccessTokenCreated { .. }
            | Event::AccessTokenRevoked { .. }
            | Event::AccessTokensOfCreatorRevoked { .. }
            | Event::InviteCreated { .. }
            | Event::InviteChanged { .. }
            | Event::InviteRevoked { .. }
            | Event::InviteUsed { .. }
            | Event::InvitesOfCreatorRevoked { .. }
            | Event::JoinRequestCreated { .. }
            | Event::JoinRequestDecided { .. }
            | Event::JoinRequestWithdrawn { .. }
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
            | Event::CallEntryResolved { .. }
            | Event::RecordingStarted { .. }
            | Event::RecordingUpdated { .. }
            | Event::RecordingRemoved { .. }
            | Event::TranscriptLineCorrected { .. }
            | Event::TranscriptSpeakerRenamed { .. }
            | Event::GlossaryReplaced { .. }
            | Event::ChannelArchived { .. }
            | Event::ChannelRestored { .. }
            | Event::ChannelDeletionDue { .. }
            | Event::CategoryDeleted { .. }
            | Event::WorkspaceRoleDeleted { .. }
            | Event::ServerRoleDeleted { .. } => Self::General,
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

/// How far an event about the inside of a call of a voice channel reaches (ADR 0517).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallReach {
    /// The accounts that may see who is inside: every account that may see the channel, or
    /// only the accounts inside the call when the call hides who is inside.
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
    /// The affected member, and accounts that currently see the workspace.
    WorkspaceMembership {
        workspace_id: WorkspaceId,
        account_id: AccountId,
    },
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
    Category {
        workspace_id: WorkspaceId,
        category_id: CategoryId,
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
    /// Accounts that may see the voice channel, as far as `reach` states for the call
    /// `call_id`. Whether that call hides who is inside is the state that it started with;
    /// who is inside it is read when the event is delivered. An account that enters a later
    /// call of the channel is not inside this one (ADR 0517). An account identifier further
    /// restricts delivery to that account.
    Call {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        call_id: CallId,
        reach: CallReach,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account_id: Option<AccountId>,
    },
    /// The two participants of a direct message.
    Direct {
        low_account_id: AccountId,
        high_account_id: AccountId,
    },
    ActiveAccounts,
    Notification {
        message_id: junto_protocol::ids::MessageId,
        notice_id: uuid::Uuid,
    },
    SystemMessage {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        call_id: junto_protocol::ids::CallId,
        readers: Option<Vec<AccountId>>,
    },
    Mention {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        message_id: junto_protocol::ids::MessageId,
        #[serde(default)]
        account_id: Option<AccountId>,
        #[serde(default)]
        notice_id: Option<uuid::Uuid>,
    },
    /// The accounts that the rows of `event_receivers` with this identifier list. The
    /// change that recorded the event selected them, because the rule that named them no
    /// longer holds after it: the holders of a deleted role.
    Listed {
        list: uuid::Uuid,
    },
    /// The accounts that could see a workspace when it was deleted: its members, which the
    /// rows of `event_receivers` with the identifier `members` list, and the accounts that
    /// see every workspace.
    DeletedWorkspace {
        workspace_id: WorkspaceId,
        members: uuid::Uuid,
    },
    /// The accounts that read an invite: the holders of `create_invites` who may see every
    /// workspace in `workspace_ids`, and the holder who created it (ADR 0111).
    Invite {
        created_by: AccountId,
        workspace_ids: Vec<WorkspaceId>,
    },
}

/// The audience of an event about a call at `place`. `inside` names the call and the reach
/// of an event that tells something of the inside of the call; without it the event tells
/// only what every account at the place may know.
fn of_call(place: &CallPlace, inside: Option<(CallId, CallReach)>) -> Audience {
    match (*place, inside) {
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
            Some((call_id, reach)),
        ) => Audience::Call {
            workspace_id,
            channel_id,
            call_id,
            reach,
            account_id: None,
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

/// The workspace and the voice channel of the recording that an event is about. `None` for
/// an event that is not about a recording.
pub fn recording_place(event: &Event) -> Option<(WorkspaceId, ChannelId)> {
    match event {
        Event::RecordingStarted { recording } | Event::RecordingUpdated { recording } => {
            Some((recording.workspace_id, recording.channel_id))
        }
        Event::RecordingRemoved {
            workspace_id,
            channel_id,
            ..
        }
        | Event::TranscriptLineCorrected {
            workspace_id,
            channel_id,
            ..
        }
        | Event::TranscriptSpeakerRenamed {
            workspace_id,
            channel_id,
            ..
        } => Some((*workspace_id, *channel_id)),
        _ => None,
    }
}

/// The audience of an event about a recording of a voice channel. A recording and its
/// transcript tell who is in its call, so while the call runs the event reaches the
/// accounts that may see who is inside: `running_call` is that call. When the call has
/// ended, the event reaches the accounts that may see the voice channel. This is the rule
/// of the routes that read recordings (ADR 0604).
pub fn of_recording(
    workspace_id: WorkspaceId,
    channel_id: ChannelId,
    running_call: Option<CallId>,
) -> Audience {
    let place = CallPlace::Channel {
        workspace_id,
        channel_id,
    };
    of_call(
        &place,
        running_call.map(|call_id| (call_id, CallReach::Occupancy)),
    )
}

/// The audience of an event for the people inside a call: a request to enter and its end.
/// `None` for another event.
pub fn of_the_inside(event: &Event) -> Option<Audience> {
    let (place, call_id) = match event {
        Event::CallEntryRequested { request } => (&request.place, request.call_id),
        Event::CallEntryResolved { place, call_id, .. } => (place, *call_id),
        _ => return None,
    };
    match place {
        CallPlace::Channel { .. } => Some(of_call(place, Some((call_id, CallReach::Inside)))),
        CallPlace::Direct { .. } => None,
    }
}

impl Audience {
    pub const fn account_id(&self) -> Option<AccountId> {
        match self {
            Self::Mention {
                account_id: Some(account_id),
                ..
            }
            | Self::Call {
                account_id: Some(account_id),
                ..
            }
            | Self::Account { account_id }
            | Self::AccountSeeing { account_id, .. } => Some(*account_id),
            _ => None,
        }
    }

    /// The audience of an event for one account: the account alone, or the account while it
    /// may be at the places that the event names.
    pub fn addressed(account_id: AccountId, event: &Event) -> Self {
        let named: Vec<CallPlace> = match event {
            Event::MessageRead { place, .. } => vec![message_call_place(*place)],
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
            Self::WorkspaceMembership { workspace_id, .. }
            | Self::SystemMessage { workspace_id, .. }
            | Self::Mention { workspace_id, .. }
            | Self::Workspace { workspace_id }
            | Self::Category { workspace_id, .. }
            | Self::WorkspaceRoleHolders { workspace_id, .. }
            | Self::Channel { workspace_id, .. }
            | Self::DeletedChannel { workspace_id, .. }
            | Self::Call { workspace_id, .. } => Some(*workspace_id),
            Self::Notification { .. }
            | Self::ActiveAccounts
            | Self::Listed { .. }
            | Self::DeletedWorkspace { .. }
            | Self::Invite { .. }
            | Self::Account { .. }
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

    /// The list of `event_receivers` that decides who receives the event.
    pub const fn receiver_list(&self) -> Option<uuid::Uuid> {
        match self {
            Self::Listed { list } => Some(*list),
            Self::DeletedWorkspace { members, .. } => Some(*members),
            _ => None,
        }
    }

    /// The workspaces of the invite that the audience names. A decision reads which of
    /// them the recipient is a member of.
    pub fn invite_workspace_ids(&self) -> &[WorkspaceId] {
        match self {
            Self::Invite { workspace_ids, .. } => workspace_ids,
            _ => &[],
        }
    }

    /// The call whose inside the audience names.
    pub const fn inside_of_call(&self) -> Option<CallId> {
        match self {
            Self::Call { call_id, .. } => Some(*call_id),
            _ => None,
        }
    }

    pub const fn channel_id(&self) -> Option<ChannelId> {
        match self {
            Self::SystemMessage { channel_id, .. }
            | Self::Mention { channel_id, .. }
            | Self::Channel { channel_id, .. }
            | Self::DeletedChannel { channel_id, .. }
            | Self::Call { channel_id, .. } => Some(*channel_id),
            _ => None,
        }
    }

    /// The call whose events the audience carries, for the rule that an account in a call
    /// receives every event of that call (ADR 0107).
    pub const fn call_key(&self) -> Option<CallKey> {
        match self {
            Self::SystemMessage { channel_id, .. }
            | Self::Channel { channel_id, .. }
            | Self::Call { channel_id, .. } => Some(CallKey::Channel(*channel_id)),
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
            Event::WorkspaceMemberRemoved {
                workspace_id,
                account_id,
            } => Some(Self::WorkspaceMembership {
                workspace_id: *workspace_id,
                account_id: *account_id,
            }),
            Event::ConversationOpened { conversation } => Some(Self::Direct {
                low_account_id: conversation.low_account_id,
                high_account_id: conversation.high_account_id,
            }),
            Event::ServerLogoChanged { .. } | Event::AttachmentLimitsChanged { .. } => {
                Some(Self::ActiveAccounts)
            }
            // Its writer states the stored list and the moderators (ADR 0121).
            Event::AccountDepartureChanged { .. } => None,
            Event::MessageMentioned {
                workspace_id,
                channel_id,
                message_id,
                ..
            } => Some(Self::Mention {
                workspace_id: *workspace_id,
                channel_id: *channel_id,
                message_id: *message_id,
                account_id: None,
                notice_id: None,
            }),
            Event::MessageChanged { place, .. } => Some(of_call(&message_call_place(*place), None)),
            Event::ThreadRead { .. }
            | Event::MessageRead { .. }
            | Event::NotificationChanged { .. }
            | Event::NotificationSettingsChanged { .. }
            | Event::NotificationsRead { .. }
            | Event::ThreadFollowChanged { .. } => None,
            Event::NotificationDefaultChanged { workspace_id, .. } => workspace(*workspace_id),
            Event::RetentionChanged { workspace_id } => match workspace_id {
                Some(id) => workspace(*id),
                None => permission(ServerPermission::Administrator),
            },
            Event::DeviceAdded { .. }
            | Event::DeviceRenamed { .. }
            | Event::DeviceRemoved { .. }
            | Event::DeviceLoggedIn { .. }
            | Event::DeviceLoggedOut { .. }
            | Event::AccountRecovered { .. }
            | Event::PermissionsChanged { .. }
            | Event::ChannelDeleted { .. }
            | Event::CategoryDeleted { .. }
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
            | Event::DirectMuteRemoved { .. }
            | Event::RecordingStarted { .. }
            | Event::RecordingUpdated { .. }
            | Event::RecordingRemoved { .. }
            | Event::TranscriptLineCorrected { .. }
            | Event::TranscriptSpeakerRenamed { .. }
            | Event::WorkspaceDeleted { .. }
            | Event::ExportUpdated { .. }
            | Event::ExportRemoved { .. } => None,
            Event::ChannelArchived { channel } | Event::ChannelDeletionDue { channel } => {
                Some(Self::Channel {
                    workspace_id: channel.channel.workspace_id,
                    channel_id: channel.channel.id,
                    permission: WorkspacePermission::ViewChannel,
                })
            }
            Event::WorkspaceRoleDeleted { workspace_id, .. } => workspace(*workspace_id),
            Event::GlossaryReplaced { workspace_id } => workspace(*workspace_id),
            Event::CallStarted { place, .. } | Event::CallEnded { place, .. } => {
                Some(of_call(place, None))
            }
            Event::CallParticipantJoined { call_id, place, .. }
            | Event::CallParticipantUpdated { call_id, place, .. }
            | Event::CallParticipantLeft { call_id, place, .. }
            | Event::CallLockChanged { call_id, place, .. }
            | Event::CallMeetingStarted { call_id, place, .. }
            | Event::CallMeetingEnded { call_id, place, .. }
            | Event::CallSpeaking { call_id, place, .. } => {
                Some(of_call(place, Some((*call_id, CallReach::Occupancy))))
            }
            Event::AccountAdded { .. }
            | Event::AccountStatusChanged { .. }
            | Event::AccountServerRolesChanged { .. }
            | Event::OwnerChanged { .. } => permission(ServerPermission::BanMembers),
            Event::AccountProfileUpdated { profile } => Some(Self::AccountViewers {
                account_id: profile.id,
            }),
            Event::ServerRoleCreated { .. }
            | Event::ServerRoleUpdated { .. }
            | Event::ServerRoleDeleted { .. }
            | Event::ServerRolePositionsShifted { .. } => permission(ServerPermission::BanMembers),
            Event::WorkspaceRolePositionsShifted { workspace_id, .. } => workspace(*workspace_id),
            Event::AccessTokenCreated { .. }
            | Event::AccessTokenRevoked { .. }
            | Event::AccessTokensOfCreatorRevoked { .. } => {
                permission(ServerPermission::Administrator)
            }
            Event::InviteCreated { .. }
            | Event::InviteChanged { .. }
            | Event::InviteRevoked { .. }
            | Event::InviteUsed { .. } => None,
            Event::InvitesOfCreatorRevoked { .. } => permission(ServerPermission::CreateInvites),
            Event::JoinRequestCreated { .. }
            | Event::JoinRequestDecided { .. }
            | Event::JoinRequestWithdrawn { .. } => {
                permission(ServerPermission::ApproveJoinRequests)
            }
            Event::WorkspaceCreated { workspace: changed }
            | Event::WorkspaceUpdated { workspace: changed } => workspace(changed.id),
            Event::WorkspaceMemberAdded { workspace_id, .. }
            | Event::WorkspaceMemberRolesChanged { workspace_id, .. } => workspace(*workspace_id),
            Event::WorkspaceRoleCreated { role } | Event::WorkspaceRoleUpdated { role } => {
                workspace(role.workspace_id)
            }
            Event::CategoryCreated { category } | Event::CategoryUpdated { category } => {
                Some(Self::Category {
                    workspace_id: category.workspace_id,
                    category_id: category.id,
                })
            }
            Event::ChannelCreated { channel }
            | Event::ChannelUpdated { channel }
            | Event::ChannelRestored { channel } => Some(Self::Channel {
                workspace_id: channel.workspace_id,
                channel_id: channel.id,
                permission: WorkspacePermission::ViewChannel,
            }),
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
                Event::NotificationChanged { message_id },
                Self::Notification {
                    message_id: subject,
                    ..
                },
            ) => message_id == subject,
            (
                Event::MessageMentioned {
                    workspace_id,
                    channel_id,
                    message_id,
                    ..
                },
                Self::Mention {
                    workspace_id: workspace,
                    channel_id: channel,
                    message_id: message,
                    ..
                },
            ) => workspace_id == workspace && channel_id == channel && message_id == message,
            (
                Event::MessageChanged {
                    place:
                        junto_protocol::message::MessagePlace::Channel {
                            workspace_id,
                            channel_id,
                        },
                    ..
                },
                Self::SystemMessage {
                    workspace_id: w,
                    channel_id: ch,
                    ..
                },
            ) => workspace_id == w && channel_id == ch,
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
                Event::NotificationSettingsChanged { .. },
                Self::Account { .. } | Self::ActiveAccounts | Self::Workspace { .. },
            ) => true,
            (
                Event::PermissionsChanged { .. }
                | Event::CallAccessEnded {}
                | Event::DirectMuteSet { .. }
                | Event::DirectMuteRemoved { .. }
                | Event::NotificationsRead { .. }
                | Event::ThreadFollowChanged { .. },
                Self::Account { .. },
            ) => true,
            // A follow that a reply made by the rule is told while the account sees the
            // channel of the thread (ADR 0412).
            (
                Event::ThreadFollowChanged { .. },
                Self::AccountSeeing {
                    channels, accounts, ..
                },
            ) => channels.len() == 1 && accounts.is_empty(),
            (
                Event::ThreadRead { .. }
                | Event::MessageRead { .. }
                | Event::CallInvited { .. }
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
            (Event::CallEntryRequested { request }, Self::AccountSeeing { .. }) => {
                *self == Self::addressed_at(request.account_id, &[request.place])
            }
            (Event::CallEntryRequested { .. }, Self::Call { account_id, .. }) => {
                let mut inside = of_the_inside(event);
                if let Some(Self::Call {
                    account_id: target, ..
                }) = &mut inside
                {
                    *target = *account_id;
                }
                inside.as_ref() == Some(self)
            }
            (Event::CallEntryRequested { .. } | Event::CallEntryResolved { .. }, _) => {
                of_the_inside(event).as_ref() == Some(self)
            }
            (Event::RecordingStarted { recording } | Event::RecordingUpdated { recording }, _) => {
                [Some(recording.call_id), None].into_iter().any(|call| {
                    *self == of_recording(recording.workspace_id, recording.channel_id, call)
                })
            }
            (
                Event::RecordingRemoved { .. }
                | Event::TranscriptLineCorrected { .. }
                | Event::TranscriptSpeakerRenamed { .. },
                _,
            ) => recording_place(event).is_some_and(|(workspace_id, channel_id)| {
                let running = match self {
                    Self::Call {
                        call_id,
                        reach: CallReach::Occupancy,
                        ..
                    } => Some(*call_id),
                    _ => None,
                };
                *self == of_recording(workspace_id, channel_id, running)
            }),
            (Event::PermissionsChanged { workspace_id: None }, Self::ServerRoleHolders { .. }) => {
                true
            }
            (Event::PermissionsChanged { .. }, Self::Listed { .. }) => true,
            (Event::AccountDepartureChanged { .. }, Self::Listed { .. }) => true,
            (
                Event::AccountDepartureChanged { .. },
                Self::ServerPermission {
                    permission: ServerPermission::BanMembers,
                },
            ) => true,
            // The accounts that see a new workspace, or a channel that appears with
            // overrides (ADR 0310).
            (
                Event::PermissionsChanged {
                    workspace_id: Some(named),
                },
                Self::Workspace { workspace_id }
                | Self::Channel {
                    workspace_id,
                    permission: WorkspacePermission::ViewChannel,
                    ..
                },
            ) => named == workspace_id,
            (
                Event::InviteCreated { invite } | Event::InviteChanged { invite },
                Self::Invite {
                    created_by,
                    workspace_ids,
                },
            ) => invite.created_by == *created_by && invite.workspace_ids == *workspace_ids,
            (Event::InviteRevoked { .. } | Event::InviteUsed { .. }, Self::Invite { .. }) => true,
            (
                Event::WorkspaceDeleted { workspace_id },
                Self::DeletedWorkspace {
                    workspace_id: deleted,
                    ..
                },
            ) => workspace_id == deleted,
            (
                Event::ExportUpdated { .. } | Event::ExportRemoved { .. },
                Self::Account { .. } | Self::AccountSeeing { .. },
            ) => true,
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
            status:
                AccountStatus::Banned
                | AccountStatus::Removed
                | AccountStatus::Inactive
                | AccountStatus::Deleted,
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
    /// direct message or a direct call, and to the accounts of a profile that is no
    /// fellow's. An account that is not active has no entry.
    pub contacts: &'a HashMap<AccountId, Contact>,
    /// The lists of `event_receivers`, of the ones that the audiences of a batch name, that
    /// hold the recipient.
    pub listed: &'a HashSet<uuid::Uuid>,
    /// The workspaces, of the ones that the invites of a batch name, that the recipient is
    /// a member of.
    pub memberships: &'a HashSet<WorkspaceId>,
    /// The calls whose inside the audiences of a batch name, as the recipient stands to
    /// each now. A call without an entry is unknown, and its events reach nobody.
    pub calls: &'a HashMap<CallId, CallStanding>,
}

/// One call as the recipient stands to it now (ADR 0517).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallStanding {
    /// The call shows who is inside only to the people inside: the state that it started
    /// with, which stays stored after its end.
    pub hides_occupants: bool,
    /// The recipient is inside the call. False for a call that has ended.
    pub inside: bool,
}

impl CallStanding {
    /// Whether an event about the inside of the call with `reach` reaches the recipient.
    const fn reached(self, reach: CallReach) -> bool {
        match reach {
            CallReach::Occupancy => !self.hides_occupants || self.inside,
            CallReach::Inside => self.inside,
        }
    }
}

impl Recipient<'_> {
    fn acquaintance(&self, account_id: AccountId) -> Acquaintance {
        if account_id == self.account_id {
            Acquaintance::Own
        } else if self.fellows.contains(&account_id) {
            Acquaintance::SharedWorkspace
        } else if self.contacts.get(&account_id) == Some(&Contact::Administrator) {
            Acquaintance::Administrator
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
}

/// One workspace as the recipient stands in it now.
#[derive(Debug, Clone)]
pub struct WorkspaceStanding {
    pub scope: WorkspaceScope,
    pub everyone_role_id: WorkspaceRoleId,
    pub channels: std::collections::HashMap<ChannelId, ChannelStanding>,
    pub categories: HashMap<CategoryId, (ChannelScope, bool)>,
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
            person_override: Default::default(),
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
        Audience::WorkspaceMembership { account_id, .. } => {
            (*account_id == recipient.account_id && allows(Request::OwnAccount))
                || workspace.is_some_and(|w| allows(Request::ViewWorkspace(&w.scope)))
        }
        Audience::Notification { .. } | Audience::ActiveAccounts => allows(Request::OwnAccount),
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
        Audience::Listed { list } => recipient.listed.contains(list) && allows(Request::OwnAccount),
        Audience::Invite {
            created_by,
            workspace_ids,
        } => crate::access::reads_invite(
            recipient.actor,
            *created_by == recipient.account_id,
            workspace_ids,
            recipient.memberships,
        ),
        Audience::DeletedWorkspace { members, .. } => {
            let scope = WorkspaceScope {
                person_override: Default::default(),
                membership: if recipient.listed.contains(members) {
                    Membership::Member
                } else {
                    Membership::NotMember
                },
                everyone: junto_authz::WorkspacePermissions::EMPTY,
                roles: Vec::new(),
            };
            allows(Request::ViewWorkspace(&scope))
        }
        Audience::ServerPermission { permission } => allows(Request::Server(*permission)),
        Audience::ServerRoleHolders { role_id } => {
            recipient.server_role_ids.contains(role_id) && allows(Request::OwnAccount)
        }
        Audience::Workspace { .. } => {
            workspace.is_some_and(|standing| allows(Request::ViewWorkspace(&standing.scope)))
        }
        Audience::Category { category_id, .. } => workspace.is_some_and(|standing| {
            allows(Request::ViewWorkspace(&standing.scope))
                && standing
                    .categories
                    .get(category_id)
                    .is_some_and(|(scope, child)| {
                        *child
                            || allows(Request::Channel {
                                scope,
                                permission: WorkspacePermission::ManageChannel,
                            })
                    })
        }),
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
        Audience::SystemMessage { channel_id, .. } | Audience::Mention { channel_id, .. } => {
            workspace
                .and_then(|standing| standing.channels.get(channel_id))
                .is_some_and(|channel| {
                    !channel.deleted
                        && allows(Request::Channel {
                            scope: &channel.scope,
                            permission: WorkspacePermission::ViewChannel,
                        })
                })
        }
        Audience::DeletedChannel { overrides, .. } => workspace
            .and_then(|standing| standing.kept_scope(overrides))
            .is_some_and(|scope| {
                allows(Request::Channel {
                    scope: &scope,
                    permission: WorkspacePermission::ViewChannel,
                })
            }),
        Audience::Call {
            channel_id,
            call_id,
            reach,
            account_id,
            ..
        } => workspace
            .and_then(|standing| standing.channels.get(channel_id))
            .is_some_and(|channel| {
                let reached = recipient
                    .calls
                    .get(call_id)
                    .is_some_and(|call| call.reached(*reach));
                reached
                    && account_id.is_none_or(|account_id| account_id == recipient.account_id)
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

fn message_call_place(place: junto_protocol::message::MessagePlace) -> CallPlace {
    match place {
        junto_protocol::message::MessagePlace::Channel {
            workspace_id,
            channel_id,
        } => CallPlace::Channel {
            workspace_id,
            channel_id,
        },
        junto_protocol::message::MessagePlace::Direct {
            low_account_id,
            high_account_id,
            ..
        } => CallPlace::Direct {
            low_account_id,
            high_account_id,
        },
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
            server_override: Default::default(),
            position: Position::Member,
            server_roles: vec![],
        }
    }

    fn administrator() -> Actor {
        Actor {
            server_override: Default::default(),
            position: Position::Member,
            server_roles: vec![ServerPermissions::from(ServerPermission::Administrator)],
        }
    }

    static NO_FELLOWS: std::sync::LazyLock<HashSet<AccountId>> =
        std::sync::LazyLock::new(HashSet::new);
    static NO_CONTACTS: std::sync::LazyLock<HashMap<AccountId, Contact>> =
        std::sync::LazyLock::new(HashMap::new);
    static NO_LISTS: std::sync::LazyLock<HashSet<uuid::Uuid>> =
        std::sync::LazyLock::new(HashSet::new);
    static NO_MEMBERSHIPS: std::sync::LazyLock<HashSet<WorkspaceId>> =
        std::sync::LazyLock::new(HashSet::new);
    static NO_CALLS: std::sync::LazyLock<HashMap<CallId, CallStanding>> =
        std::sync::LazyLock::new(HashMap::new);

    fn recipient(actor: &Actor) -> Recipient<'_> {
        Recipient {
            account_id: AccountId::generate(),
            actor,
            server_role_ids: &[],
            fellows: &NO_FELLOWS,
            contacts: &NO_CONTACTS,
            listed: &NO_LISTS,
            memberships: &NO_MEMBERSHIPS,
            calls: &NO_CALLS,
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
            person_override: Default::default(),
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
            scope: ChannelScope {
                person_override: Default::default(),
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
            categories: HashMap::new(),
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
            server_override: Default::default(),
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
            server_override: Default::default(),
            position: Position::Member,
            server_roles: vec![ServerPermissions::from(ServerPermission::CreateInvites)],
        };
        let owner = Actor {
            server_override: Default::default(),
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
    fn who_is_inside_a_hidden_call_reaches_only_the_people_inside_that_call() {
        let place = place();
        let call_id = CallId::generate();
        let call = |reach| Audience::Call {
            workspace_id: place.workspace_id,
            channel_id: place.channel_id,
            call_id,
            reach,
            account_id: None,
        };
        let plain = member();
        let boss = administrator();
        let sees = standing(&place, Membership::Member, false, false);
        let stands = |hides_occupants, inside| {
            HashMap::from([(
                call_id,
                CallStanding {
                    hides_occupants,
                    inside,
                },
            )])
        };
        let receives = |actor, calls: &HashMap<CallId, CallStanding>, reach, standing| {
            let recipient = Recipient {
                calls,
                ..recipient(actor)
            };
            may_receive(recipient, &call(reach), standing)
        };

        let open = stands(false, false);
        assert!(receives(&plain, &open, CallReach::Occupancy, Some(&sees)));
        assert!(!receives(&plain, &open, CallReach::Inside, Some(&sees)));

        let hidden = stands(true, false);
        for reach in [CallReach::Occupancy, CallReach::Inside] {
            assert!(!receives(&plain, &hidden, reach, Some(&sees)));
            assert!(!receives(&boss, &hidden, reach, Some(&sees)));
        }

        let inside = stands(true, true);
        assert!(receives(&plain, &inside, CallReach::Occupancy, Some(&sees)));
        assert!(receives(&plain, &inside, CallReach::Inside, Some(&sees)));

        // A call that the batch found no record of reaches nobody, whatever its reach.
        for reach in [CallReach::Occupancy, CallReach::Inside] {
            assert!(!receives(&plain, &NO_CALLS, reach, Some(&sees)));
        }

        let without_sight = standing(&place, Membership::Member, false, true);
        assert!(!receives(
            &plain,
            &inside,
            CallReach::Inside,
            Some(&without_sight)
        ));
        let mut deleted = standing(&place, Membership::Member, false, false);
        deleted.channels.get_mut(&place.channel_id).unwrap().deleted = true;
        let open_inside = stands(false, true);
        assert!(!receives(
            &plain,
            &open_inside,
            CallReach::Occupancy,
            Some(&deleted)
        ));
        assert!(!receives(&plain, &open, CallReach::Occupancy, None));
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
            hides_occupants: Some(false),
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
            server_override: Default::default(),
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
    fn an_invite_reaches_the_inviters_who_see_its_workspaces_and_its_creator() {
        let (seen, unseen) = (WorkspaceId::generate(), WorkspaceId::generate());
        let creator = AccountId::generate();
        let audience = Audience::Invite {
            created_by: creator,
            workspace_ids: vec![seen, unseen],
        };
        let inviter = Actor {
            server_override: Default::default(),
            position: Position::Member,
            server_roles: vec![ServerPermissions::from(ServerPermission::CreateInvites)],
        };
        let (one, both) = (HashSet::from([seen]), HashSet::from([seen, unseen]));
        let in_one = Recipient {
            memberships: &one,
            ..recipient(&inviter)
        };
        let in_both = Recipient {
            memberships: &both,
            ..recipient(&inviter)
        };
        let plain = member();
        let without_permission = Recipient {
            memberships: &both,
            ..recipient(&plain)
        };
        assert!(!may_receive(in_one, &audience, None));
        assert!(may_receive(in_both, &audience, None));
        assert!(!may_receive(without_permission, &audience, None));
        assert!(may_receive(recipient(&administrator()), &audience, None));
        let own = Recipient {
            account_id: creator,
            ..recipient(&inviter)
        };
        assert!(may_receive(own, &audience, None));
        let lost = Recipient {
            account_id: creator,
            ..recipient(&plain)
        };
        assert!(!may_receive(lost, &audience, None));
        let nothing_left = Audience::Invite {
            created_by: creator,
            workspace_ids: Vec::new(),
        };
        assert!(may_receive(recipient(&inviter), &nothing_left, None));
        assert_eq!(audience.invite_workspace_ids(), [seen, unseen]);
        let revoked = Event::InviteRevoked {
            invite_id: junto_protocol::ids::InviteId::generate(),
            revocation: junto_protocol::invite::InviteRevocation::Revoked,
        };
        assert!(audience.fits(&revoked));
        assert_eq!(Audience::fixed_by(&revoked), None);
        let of_permission = Audience::ServerPermission {
            permission: ServerPermission::CreateInvites,
        };
        assert!(!of_permission.fits(&revoked));
    }

    #[test]
    fn a_listed_event_reaches_the_listed_accounts_only() {
        let list = uuid::Uuid::now_v7();
        let audience = Audience::Listed { list };
        let plain = member();
        assert!(!may_receive(recipient(&plain), &audience, None));
        assert!(!may_receive(recipient(&administrator()), &audience, None));
        let lists = HashSet::from([list]);
        let listed = Recipient {
            listed: &lists,
            ..recipient(&plain)
        };
        assert!(may_receive(listed, &audience, None));
        let other = Audience::Listed {
            list: uuid::Uuid::now_v7(),
        };
        assert!(!may_receive(listed, &other, None));
        assert!(audience.fits(&Event::PermissionsChanged { workspace_id: None }));
        assert_eq!(audience.receiver_list(), Some(list));
    }

    #[test]
    fn the_deletion_of_a_workspace_reaches_its_former_members_and_the_administrators() {
        let place = place();
        let members = uuid::Uuid::now_v7();
        let audience = Audience::DeletedWorkspace {
            workspace_id: place.workspace_id,
            members,
        };
        let plain = member();
        assert!(!may_receive(recipient(&plain), &audience, None));
        let lists = HashSet::from([members]);
        let former = Recipient {
            listed: &lists,
            ..recipient(&plain)
        };
        assert!(may_receive(former, &audience, None));
        assert!(may_receive(recipient(&administrator()), &audience, None));
        let deleted = Event::WorkspaceDeleted {
            workspace_id: place.workspace_id,
        };
        assert!(audience.fits(&deleted));
        let elsewhere = Event::WorkspaceDeleted {
            workspace_id: WorkspaceId::generate(),
        };
        assert!(!audience.fits(&elsewhere));
        assert!(
            !Audience::Workspace {
                workspace_id: place.workspace_id
            }
            .fits(&deleted)
        );
        assert_eq!(audience.workspace_id(), None);
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
        // The accounts that see a new workspace, and the accounts that see a channel that
        // appears with overrides, are told as well (ADR 0310).
        assert!(workspace.fits(&in_workspace));
        assert!(!workspace.fits(&on_server));
        let elsewhere = Audience::Workspace {
            workspace_id: WorkspaceId::generate(),
        };
        assert!(!elsewhere.fits(&in_workspace));
        assert!(channel_audience(&place, WorkspacePermission::ViewChannel).fits(&in_workspace));
        assert!(!channel_audience(&place, WorkspacePermission::ManageChannel).fits(&in_workspace));
        assert!(!channel_audience(&place, WorkspacePermission::ViewChannel).fits(&on_server));

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
            (AccountStatus::Inactive, Some(account_id)),
            (AccountStatus::Deleted, Some(account_id)),
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
