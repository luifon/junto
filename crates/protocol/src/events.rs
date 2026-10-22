//! The messages of the WebSocket event stream (ADR 0206, ADR 0207).
//!
//! Every message is one JSON object in a text frame, and its field `type` names it. A reader
//! ignores a field that it does not know and skips a message or an event whose `type` it does
//! not know.

// The schema function that the derive macro makes for `Event` is one expression over
// every variant. The crate is compiled with optimization also in the dev profile, where
// that function needs little stack, and a test of the server bounds the stack of all
// schemas together (ADR 0906).
#![cfg_attr(
    feature = "openapi",
    expect(
        clippy::large_stack_frames,
        reason = "one generated schema function for every variant of a large enum"
    )
)]

/// JSON cursor of the last committed event the caller may receive at the full level,
/// with its current access after the commit; `null` when there is no such event.
/// An absent header states no position; it does not undo a successful response.
pub const EVENT_CURSOR_HEADER: &str = "junto-event-cursor";

use junto_authz::WorkspacePermission;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::account::{
    AccountProfile, AccountStatus, AccountSummary, Departure, DeviceOrigin, RecoveryNotice,
};
use crate::call::{
    CallInvitation, CallLock, CallMove, CallParticipant, CallPlace, DirectMute, EntryOutcome,
    EntryRequest, InvitationOutcome, LeaveReason, Meeting, MeetingEnd, MoveOutcome, OwnCall,
};
use crate::ids::{
    AccountId, CallId, CallInvitationId, CallMoveId, ChannelId, DeviceId, EntryRequestId, InviteId,
    JoinRequestId, RecordingId, ServerRoleId, StreamEpoch, WorkspaceId, WorkspaceRoleId,
};
use crate::invite::{Invite, InviteRevocation, JoinRequest, JoinRequestState};
use crate::keys::PublicKey;
use crate::recording::Recording;
use crate::roles::{ServerRole, WorkspaceRole};
use crate::structure::{Category, Channel, Workspace, WorkspaceMember};

/// The path of the event stream. A device opens it with its session token in the
/// `Authorization` header of the upgrade request.
pub const STREAM_PATH: &str = "/api/v1/events";

/// A place in the order of the events of one server. An event with a position at or below the
/// position of a cursor is covered by the state that the cursor belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Cursor {
    pub epoch: StreamEpoch,
    pub position: u64,
}

/// How much a connection receives (ADR 0107).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Level {
    /// Every event that the account may see.
    Full,
    /// Direct messages, mentions, call invites and control events.
    Background,
}

/// What a client sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Starts or restarts the delivery after `cursor` at `level`. Without a cursor the
    /// delivery starts after the newest event.
    Subscribe {
        level: Level,
        #[serde(default)]
        cursor: Option<Cursor>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ResyncReason {
    /// The cursor belongs to another epoch or lies after the newest event.
    UnknownCursor,
    /// More events lie after the cursor than the server replays.
    TooFarBehind,
    /// The server could not send an event that the account may receive. The position of the
    /// connection is after that event.
    SkippedEvent,
}

/// Why the server ended the access of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    Banned,
    Removed,
    DeviceRemoved,
    /// A recovery of the account ended every session of it.
    Recovered,
    /// The session was logged out or has expired.
    SessionEnded,
    /// The account left the last workspace of which it was a member (ADR 0121).
    Inactive,
    /// The person deleted the account (ADR 0121).
    Deleted,
    /// A reason that this build does not know (ADR 0209). A client treats it as an ended
    /// session. It is read and never written.
    #[serde(other, skip_serializing)]
    Unknown,
}

/// What the server sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// The first message of every connection.
    Hello {
        server_version: String,
        api_versions: Vec<u32>,
        epoch: StreamEpoch,
        /// The server sends a ping at this interval and expects a frame from the client within
        /// `idle_seconds`.
        heartbeat_seconds: u32,
        idle_seconds: u32,
    },
    /// The answer to a `subscribe`. Events after `cursor` follow.
    Subscribed { level: Level, cursor: Cursor },
    Event {
        /// The position of the event. It grows from one event to the next; an event with an
        /// `id` at or below the last one that the client has applied is a repeat.
        id: u64,
        #[serde(with = "time::serde::rfc3339")]
        at: OffsetDateTime,
        event: Event,
    },
    /// The connection has received every event up to `cursor` that it may receive.
    Checkpoint { cursor: Cursor },
    /// The server does not continue from the cursor of the client: as the answer to a
    /// `subscribe`, or as the end of a subscription. The connection stays open and delivers
    /// nothing. Whatever the reason, the client discards every collection of this server
    /// that it holds, loads a snapshot and subscribes with the cursor of the snapshot. That
    /// includes the collections that the snapshot does not carry, such as members, accounts,
    /// devices, invites, join requests, server roles and channel overrides: the client loads
    /// them again from their routes when it needs them.
    Resync { reason: ResyncReason },
    /// The last message before the server closes a connection whose session no longer counts.
    Ended {
        reason: EndReason,
        /// Present with the reason `recovered`.
        recovery: Option<RecoveryNotice>,
    },
}

impl ServerMessage {
    pub const NAMES: &'static [&'static str] = &[
        "hello",
        "subscribed",
        "event",
        "checkpoint",
        "resync",
        "ended",
    ];
}

/// The code of the close frame with which the server ends a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum CloseCode {
    /// The server stops. The client connects again with its cursor.
    Stopping = 1001,
    /// A client message was larger than the limit.
    MessageTooBig = 1009,
    /// The server could not serve the connection. The client connects again with its cursor.
    Failed = 1011,
    /// The session no longer counts; an `ended` message came before.
    Ended = 4001,
    /// A newer connection of the same device took the place of this one.
    Replaced = 4002,
    /// The client did not take a message in time. It connects again with its cursor.
    Slow = 4003,
    /// The client sent no frame within the idle time, or no `subscribe` in time.
    Idle = 4004,
    /// A client message was not a known message.
    InvalidMessage = 4005,
    /// The client sent more messages than the limit.
    TooManyMessages = 4006,
}

impl CloseCode {
    pub const ALL: &'static [Self] = &[
        Self::Stopping,
        Self::MessageTooBig,
        Self::Failed,
        Self::Ended,
        Self::Replaced,
        Self::Slow,
        Self::Idle,
        Self::InvalidMessage,
        Self::TooManyMessages,
    ];

    pub const fn code(self) -> u16 {
        self as u16
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Stopping => "stopping",
            Self::MessageTooBig => "message_too_big",
            Self::Failed => "failed",
            Self::Ended => "ended",
            Self::Replaced => "replaced",
            Self::Slow => "slow",
            Self::Idle => "idle",
            Self::InvalidMessage => "invalid_message",
            Self::TooManyMessages => "too_many_messages",
        }
    }

    pub fn from_code(code: u16) -> Option<Self> {
        Self::ALL.iter().copied().find(|known| known.code() == code)
    }
}

/// One change of the state of a server. New events are added; the fields of an existing event
/// are not changed or removed within one API version (ADR 0206).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A device was added to the account of the recipient.
    DeviceAdded {
        device_id: DeviceId,
        name: String,
        public_key: PublicKey,
        origin: DeviceOrigin,
        #[serde(with = "time::serde::rfc3339")]
        created_at: OffsetDateTime,
    },
    DeviceRenamed {
        device_id: DeviceId,
        name: String,
    },
    /// A device of the account was removed. Its sessions have ended.
    DeviceRemoved {
        device_id: DeviceId,
    },
    DeviceLoggedIn {
        device_id: DeviceId,
        #[serde(with = "time::serde::rfc3339")]
        logged_in_at: OffsetDateTime,
    },
    DeviceLoggedOut {
        device_id: DeviceId,
    },
    /// The server owner gave a new device access to the account of the recipient.
    AccountRecovered {
        recovery: RecoveryNotice,
    },
    /// What the recipient may see or do has changed: in one workspace, or on the server when
    /// no workspace is named. The client loads a snapshot.
    PermissionsChanged {
        workspace_id: Option<WorkspaceId>,
    },
    AccountAdded {
        account: AccountSummary,
    },
    /// The name or the avatar of an account that the recipient may see has changed.
    AccountProfileUpdated {
        profile: AccountProfile,
    },
    AccountStatusChanged {
        account_id: AccountId,
        status: AccountStatus,
    },
    AccountServerRolesChanged {
        account_id: AccountId,
        server_role_ids: Vec<ServerRoleId>,
    },
    OwnerChanged {
        account_id: AccountId,
        previous_owner_id: Option<AccountId>,
    },
    /// Replaces the positions of existing server roles before the following creation.
    ServerRolePositionsShifted {
        positions: Vec<crate::roles::ServerRolePosition>,
    },
    ServerRoleCreated {
        role: ServerRole,
    },
    ServerRoleUpdated {
        role: ServerRole,
    },
    /// An administrator created an access token (ADR 0210). The event never carries the
    /// token.
    AccessTokenCreated {
        access_token: crate::access_token::AccessTokenInfo,
    },
    /// An access token was revoked, or it ended with the access of its creator. A token
    /// that reaches its expiry has no event.
    AccessTokenRevoked {
        access_token_id: crate::ids::AccessTokenId,
    },
    /// Every access token of an account ended, because the account was banned, removed or
    /// recovered.
    AccessTokensOfCreatorRevoked {
        created_by: AccountId,
    },
    InviteCreated {
        invite: Invite,
    },
    /// The workspaces granted by an existing invite were narrowed.
    InviteChanged {
        invite: Invite,
    },
    InviteRevoked {
        invite_id: InviteId,
        revocation: InviteRevocation,
    },
    InviteUsed {
        invite_id: InviteId,
        uses: u32,
    },
    /// Every invite that the account created and that was not revoked is revoked now.
    InvitesOfCreatorRevoked {
        created_by: AccountId,
        revocation: InviteRevocation,
    },
    JoinRequestCreated {
        join_request: JoinRequest,
    },
    JoinRequestDecided {
        join_request_id: JoinRequestId,
        state: JoinRequestState,
        account_id: Option<AccountId>,
    },
    WorkspaceCreated {
        workspace: Workspace,
    },
    WorkspaceUpdated {
        workspace: Workspace,
    },
    WorkspaceMemberAdded {
        workspace_id: WorkspaceId,
        member: WorkspaceMember,
    },
    WorkspaceMemberRemoved {
        workspace_id: WorkspaceId,
        account_id: AccountId,
    },
    WorkspaceMemberRolesChanged {
        workspace_id: WorkspaceId,
        account_id: AccountId,
        role_ids: Vec<WorkspaceRoleId>,
    },
    /// Replaces the positions of existing non-everyone roles before creation.
    WorkspaceRolePositionsShifted {
        workspace_id: WorkspaceId,
        positions: Vec<crate::roles::WorkspaceRolePosition>,
    },
    WorkspaceRoleCreated {
        role: WorkspaceRole,
    },
    WorkspaceRoleUpdated {
        role: WorkspaceRole,
    },
    CategoryCreated {
        category: Category,
    },
    CategoryUpdated {
        category: Category,
    },
    ChannelCreated {
        channel: Channel,
    },
    ChannelUpdated {
        channel: Channel,
    },
    ChannelDeleted {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
    },
    /// The allow set and the deny set of one role in a channel. Two empty sets mean that the
    /// role has no override in the channel.
    ChannelOverrideSet {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        role_id: WorkspaceRoleId,
        allow: Vec<WorkspacePermission>,
        deny: Vec<WorkspacePermission>,
    },
    /// The first person is in the room of a call: the voice channel is in use, or the
    /// direct-message call has begun.
    CallStarted {
        #[serde(default)]
        hides_occupants: Option<bool>,
        call_id: CallId,
        place: CallPlace,
        #[serde(with = "time::serde::rfc3339")]
        started_at: OffsetDateTime,
    },
    /// Nobody is in the call any more. Its lock, its blocks, its requests and its
    /// invitations have ended with it.
    CallEnded {
        call_id: CallId,
        place: CallPlace,
    },
    /// The media server lists a person whom the server allows in the call.
    CallParticipantJoined {
        call_id: CallId,
        place: CallPlace,
        participant: CallParticipant,
    },
    /// The own settings, the blocks, the raised hand or the screen share of a person in a
    /// call changed. The event carries the whole participant.
    CallParticipantUpdated {
        call_id: CallId,
        place: CallPlace,
        participant: CallParticipant,
    },
    CallParticipantLeft {
        call_id: CallId,
        place: CallPlace,
        account_id: AccountId,
        reason: LeaveReason,
    },
    CallLockChanged {
        call_id: CallId,
        place: CallPlace,
        /// Absent: the voice channel is not locked.
        lock: Option<CallLock>,
    },
    /// Meeting mode is on. Every participant except the presenter who had no microphone block
    /// has one now, with the reason `meeting`.
    CallMeetingStarted {
        call_id: CallId,
        place: CallPlace,
        meeting: Meeting,
    },
    /// Meeting mode is off. Every microphone block in the call is removed, the ones that a
    /// moderator set included.
    CallMeetingEnded {
        call_id: CallId,
        place: CallPlace,
        reason: MeetingEnd,
    },
    /// The people in a voice channel who speak now, as the clients inside report it. The
    /// list replaces the one before. It is not sent for a channel that hides who is inside.
    CallSpeaking {
        call_id: CallId,
        place: CallPlace,
        account_ids: Vec<AccountId>,
    },
    /// A person outside asks to enter a locked or full voice channel. Anyone inside answers.
    CallEntryRequested {
        request: EntryRequest,
    },
    /// A request to enter has ended. This event goes to the people inside the call.
    CallEntryResolved {
        request_id: EntryRequestId,
        call_id: CallId,
        place: CallPlace,
        account_id: AccountId,
        outcome: EntryOutcome,
        /// The person inside who answered.
        by: Option<AccountId>,
    },
    /// The request of the recipient to enter a voice channel has ended. The event names the
    /// request and nothing of the channel, so it also reaches an account that can no longer
    /// see the channel.
    CallEntryAnswered {
        request_id: EntryRequestId,
        outcome: EntryOutcome,
    },
    /// The recipient is invited into a call, or the other participant of a direct message
    /// calls. An account that cannot see the voice channel any more does not receive it.
    CallInvited {
        invitation: CallInvitation,
    },
    /// An invitation that the recipient got or made has ended. The event names the
    /// invitation and nothing of the call.
    CallInvitationResolved {
        invitation_id: CallInvitationId,
        outcome: InvitationOutcome,
    },
    /// A moderator moves the recipient into another voice channel. The device in the call
    /// asks for the join of the destination and joins it. An account that cannot see both
    /// voice channels does not receive it.
    CallMoveRequested {
        #[serde(rename = "move")]
        call_move: CallMove,
    },
    /// A move of the recipient, or one that the recipient asked for, has ended. The event
    /// names the move and nothing of the two channels.
    CallMoveResolved {
        move_id: CallMoveId,
        outcome: MoveOutcome,
    },
    /// The call that the account of the recipient is in has changed. An event that names a
    /// voice channel reaches the account only while it can see that channel; the event of a
    /// leave names no place.
    OwnCallChanged {
        /// Absent: the account is in no call.
        call: Option<OwnCall>,
        /// Present when the account left a call: why.
        left: Option<LeaveReason>,
    },
    /// The account of the recipient lost the access to a voice channel, or can no longer
    /// reach the other account of a direct message, while something of it waited there: an
    /// invitation that it got or made, a request to enter, a move, or a direct-message call.
    /// Those have ended, and no outcome of them follows. The event names nothing: the client
    /// discards what it holds for the places that it can no longer see.
    CallAccessEnded {},
    /// The account of the recipient muted its direct-message conversation with another
    /// account, or changed the end of the mute.
    DirectMuteSet {
        mute: DirectMute,
    },
    /// The account of the recipient turned the mute of a direct-message conversation off.
    DirectMuteRemoved {
        account_id: AccountId,
    },
    /// A recording of the call of a voice channel was asked for. The event carries the
    /// recording without its segments.
    RecordingStarted {
        recording: Box<Recording>,
    },
    /// The state of a recording changed, a stop was asked for, or a segment was added or
    /// failed. The event carries the whole recording without its segments; the segments
    /// are read from the route of the recording.
    RecordingUpdated {
        recording: Box<Recording>,
    },
    /// A recording was removed with its files and its transcript.
    RecordingRemoved {
        recording_id: RecordingId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
    },
    /// A person corrected the line at the position `line` of the transcript of a
    /// recording. The event carries no text, no author and no time, because the log of
    /// events keeps an event after its recording was removed (ADR 0607). The line is read
    /// from the route of the lines.
    TranscriptLineCorrected {
        recording_id: RecordingId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        line: u32,
    },
    /// A person gave a name to the voice with the label `Speaker <speaker>` in the shared
    /// sound of a recording. The event carries no name, for the same reason. The label is
    /// read from the route of the transcript.
    TranscriptSpeakerRenamed {
        recording_id: RecordingId,
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        speaker: u32,
    },
    /// The glossary of a workspace was replaced. The terms are read from its route.
    GlossaryReplaced {
        workspace_id: WorkspaceId,
    },
    ConversationOpened {
        conversation: crate::message::Conversation,
    },
    ServerLogoChanged {
        logo: Option<crate::account::Avatar>,
    },
    AttachmentLimitsChanged {
        limits: crate::message::AttachmentLimits,
    },
    MessageMentioned {
        workspace_id: WorkspaceId,
        channel_id: ChannelId,
        message_id: crate::ids::MessageId,
        position: u64,
    },
    MessageChanged {
        place: crate::message::MessagePlace,
        message_id: crate::ids::MessageId,
        position: u64,
        deleted: bool,
    },
    NotificationChanged {
        message_id: crate::ids::MessageId,
    },
    NotificationSettingsChanged {},
    RetentionChanged {
        workspace_id: Option<WorkspaceId>,
    },
    ThreadRead {
        root_id: crate::ids::MessageId,
        position: u64,
    },
    MessageRead {
        place: crate::message::MessagePlace,
        position: u64,
    },
    /// A channel was archived (ADR 0410). It is read-only and leaves the channel list; the
    /// call of a voice channel ends.
    ChannelArchived {
        channel: crate::structure::ArchivedChannel,
    },
    /// An archived channel is a channel of the list again.
    ChannelRestored {
        channel: Channel,
    },
    /// The notice that the server deletes an archived channel in 7 days, at `deletes_at`.
    ChannelDeletionDue {
        channel: crate::structure::ArchivedChannel,
    },
    /// A category was deleted. Its channels stay and have no category.
    CategoryDeleted {
        workspace_id: WorkspaceId,
        category_id: crate::ids::CategoryId,
    },
    /// A workspace role was deleted. No member holds it, and no channel has an override
    /// of it.
    WorkspaceRoleDeleted {
        workspace_id: WorkspaceId,
        role_id: WorkspaceRoleId,
    },
    /// A server role was deleted. No account holds it.
    ServerRoleDeleted {
        role_id: ServerRoleId,
    },
    /// A workspace was deleted with everything that it held. The event goes to the accounts
    /// that were members of it and to the administrators.
    WorkspaceDeleted {
        workspace_id: WorkspaceId,
    },
    /// The state of an export of the recipient changed (ADR 0411).
    ExportUpdated {
        export: crate::export::Export,
    },
    /// An export of the recipient was removed with its file.
    ExportRemoved {
        export_id: crate::ids::ExportId,
        reason: crate::export::ExportRemoval,
    },
    /// Every visible notice created at or before `until` is read at `read_at` (ADR 0412).
    NotificationsRead {
        #[serde(with = "time::serde::rfc3339")]
        until: OffsetDateTime,
        #[serde(with = "time::serde::rfc3339")]
        read_at: OffsetDateTime,
    },
    /// The account of the recipient chose to follow a thread or not, or removed its choice.
    /// `following` is what holds now; `follow_setting` is the choice, absent when none is
    /// set (ADR 0412).
    ThreadFollowChanged {
        root_id: crate::ids::MessageId,
        following: bool,
        follow_setting: Option<bool>,
    },
    /// The notification default of a workspace was set, changed or removed. `level` is
    /// absent when the workspace has none now.
    NotificationDefaultChanged {
        workspace_id: WorkspaceId,
        level: Option<crate::discovery::NotificationLevel>,
    },
    /// An account became inactive, was deleted, or returned (`departure` absent) (ADR 0121).
    /// Every active account receives it.
    AccountDepartureChanged {
        account_id: AccountId,
        departure: Option<Departure>,
    },
}

impl Event {
    pub const NAMES: &'static [&'static str] = &[
        "device_added",
        "device_renamed",
        "device_removed",
        "device_logged_in",
        "device_logged_out",
        "account_recovered",
        "permissions_changed",
        "account_added",
        "account_profile_updated",
        "account_status_changed",
        "account_server_roles_changed",
        "owner_changed",
        "server_role_positions_shifted",
        "workspace_role_positions_shifted",
        "server_role_created",
        "server_role_updated",
        "access_token_created",
        "access_token_revoked",
        "access_tokens_of_creator_revoked",
        "invite_created",
        "invite_changed",
        "invite_revoked",
        "invite_used",
        "invites_of_creator_revoked",
        "join_request_created",
        "join_request_decided",
        "workspace_created",
        "workspace_updated",
        "workspace_member_added",
        "workspace_member_removed",
        "workspace_member_roles_changed",
        "workspace_role_created",
        "workspace_role_updated",
        "category_created",
        "category_updated",
        "channel_created",
        "channel_updated",
        "channel_deleted",
        "channel_override_set",
        "call_started",
        "call_ended",
        "call_participant_joined",
        "call_participant_updated",
        "call_participant_left",
        "call_lock_changed",
        "call_meeting_started",
        "call_meeting_ended",
        "call_speaking",
        "call_entry_requested",
        "call_entry_resolved",
        "call_entry_answered",
        "call_invited",
        "call_invitation_resolved",
        "call_move_requested",
        "call_move_resolved",
        "own_call_changed",
        "call_access_ended",
        "direct_mute_set",
        "direct_mute_removed",
        "recording_started",
        "recording_updated",
        "recording_removed",
        "transcript_line_corrected",
        "transcript_speaker_renamed",
        "glossary_replaced",
        "message_changed",
        "thread_read",
        "message_read",
        "message_mentioned",
        "conversation_opened",
        "server_logo_changed",
        "attachment_limits_changed",
        "notification_changed",
        "notification_settings_changed",
        "retention_changed",
        "channel_archived",
        "channel_restored",
        "channel_deletion_due",
        "category_deleted",
        "workspace_role_deleted",
        "server_role_deleted",
        "workspace_deleted",
        "export_updated",
        "export_removed",
        "notifications_read",
        "thread_follow_changed",
        "notification_default_changed",
        "account_departure_changed",
    ];

    pub const fn name(&self) -> &'static str {
        match self {
            Self::DeviceAdded { .. } => "device_added",
            Self::DeviceRenamed { .. } => "device_renamed",
            Self::DeviceRemoved { .. } => "device_removed",
            Self::DeviceLoggedIn { .. } => "device_logged_in",
            Self::DeviceLoggedOut { .. } => "device_logged_out",
            Self::AccountRecovered { .. } => "account_recovered",
            Self::PermissionsChanged { .. } => "permissions_changed",
            Self::AccountAdded { .. } => "account_added",
            Self::AccountProfileUpdated { .. } => "account_profile_updated",
            Self::AccountDepartureChanged { .. } => "account_departure_changed",
            Self::AccountStatusChanged { .. } => "account_status_changed",
            Self::AccountServerRolesChanged { .. } => "account_server_roles_changed",
            Self::OwnerChanged { .. } => "owner_changed",
            Self::ServerRolePositionsShifted { .. } => "server_role_positions_shifted",
            Self::WorkspaceRolePositionsShifted { .. } => "workspace_role_positions_shifted",
            Self::ServerRoleCreated { .. } => "server_role_created",
            Self::ServerRoleUpdated { .. } => "server_role_updated",
            Self::AccessTokenCreated { .. } => "access_token_created",
            Self::AccessTokenRevoked { .. } => "access_token_revoked",
            Self::AccessTokensOfCreatorRevoked { .. } => "access_tokens_of_creator_revoked",
            Self::InviteCreated { .. } => "invite_created",
            Self::InviteChanged { .. } => "invite_changed",
            Self::InviteRevoked { .. } => "invite_revoked",
            Self::InviteUsed { .. } => "invite_used",
            Self::InvitesOfCreatorRevoked { .. } => "invites_of_creator_revoked",
            Self::JoinRequestCreated { .. } => "join_request_created",
            Self::JoinRequestDecided { .. } => "join_request_decided",
            Self::WorkspaceCreated { .. } => "workspace_created",
            Self::WorkspaceUpdated { .. } => "workspace_updated",
            Self::WorkspaceMemberAdded { .. } => "workspace_member_added",
            Self::WorkspaceMemberRemoved { .. } => "workspace_member_removed",
            Self::WorkspaceMemberRolesChanged { .. } => "workspace_member_roles_changed",
            Self::WorkspaceRoleCreated { .. } => "workspace_role_created",
            Self::WorkspaceRoleUpdated { .. } => "workspace_role_updated",
            Self::CategoryCreated { .. } => "category_created",
            Self::CategoryUpdated { .. } => "category_updated",
            Self::ChannelCreated { .. } => "channel_created",
            Self::ChannelUpdated { .. } => "channel_updated",
            Self::ChannelDeleted { .. } => "channel_deleted",
            Self::ChannelOverrideSet { .. } => "channel_override_set",
            Self::CallStarted { .. } => "call_started",
            Self::CallEnded { .. } => "call_ended",
            Self::CallParticipantJoined { .. } => "call_participant_joined",
            Self::CallParticipantUpdated { .. } => "call_participant_updated",
            Self::CallParticipantLeft { .. } => "call_participant_left",
            Self::CallLockChanged { .. } => "call_lock_changed",
            Self::CallMeetingStarted { .. } => "call_meeting_started",
            Self::CallMeetingEnded { .. } => "call_meeting_ended",
            Self::CallSpeaking { .. } => "call_speaking",
            Self::CallEntryRequested { .. } => "call_entry_requested",
            Self::CallEntryResolved { .. } => "call_entry_resolved",
            Self::CallEntryAnswered { .. } => "call_entry_answered",
            Self::CallInvited { .. } => "call_invited",
            Self::CallInvitationResolved { .. } => "call_invitation_resolved",
            Self::CallMoveRequested { .. } => "call_move_requested",
            Self::CallMoveResolved { .. } => "call_move_resolved",
            Self::OwnCallChanged { .. } => "own_call_changed",
            Self::CallAccessEnded {} => "call_access_ended",
            Self::DirectMuteSet { .. } => "direct_mute_set",
            Self::DirectMuteRemoved { .. } => "direct_mute_removed",
            Self::RecordingStarted { .. } => "recording_started",
            Self::RecordingUpdated { .. } => "recording_updated",
            Self::RecordingRemoved { .. } => "recording_removed",
            Self::TranscriptLineCorrected { .. } => "transcript_line_corrected",
            Self::TranscriptSpeakerRenamed { .. } => "transcript_speaker_renamed",
            Self::GlossaryReplaced { .. } => "glossary_replaced",
            Self::MessageChanged { .. } => "message_changed",
            Self::MessageRead { .. } => "message_read",
            Self::ThreadRead { .. } => "thread_read",
            Self::NotificationChanged { .. } => "notification_changed",
            Self::NotificationSettingsChanged { .. } => "notification_settings_changed",
            Self::RetentionChanged { .. } => "retention_changed",
            Self::MessageMentioned { .. } => "message_mentioned",
            Self::ConversationOpened { .. } => "conversation_opened",
            Self::ServerLogoChanged { .. } => "server_logo_changed",
            Self::AttachmentLimitsChanged { .. } => "attachment_limits_changed",
            Self::ChannelArchived { .. } => "channel_archived",
            Self::ChannelRestored { .. } => "channel_restored",
            Self::ChannelDeletionDue { .. } => "channel_deletion_due",
            Self::CategoryDeleted { .. } => "category_deleted",
            Self::WorkspaceRoleDeleted { .. } => "workspace_role_deleted",
            Self::ServerRoleDeleted { .. } => "server_role_deleted",
            Self::WorkspaceDeleted { .. } => "workspace_deleted",
            Self::ExportUpdated { .. } => "export_updated",
            Self::ExportRemoved { .. } => "export_removed",
            Self::NotificationsRead { .. } => "notifications_read",
            Self::ThreadFollowChanged { .. } => "thread_follow_changed",
            Self::NotificationDefaultChanged { .. } => "notification_default_changed",
        }
    }
}

/// A server message as a client of this API version reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(
    clippy::large_enum_variant,
    reason = "The ordinary message stays inline; boxing it would allocate for every decoded frame and change the reader API."
)]
pub enum Received {
    Message(ServerMessage),
    /// An event of a type that this version does not know. The client moves its cursor to
    /// `id` and applies nothing.
    UnknownEvent {
        id: u64,
    },
    /// A message of a type that this version does not know. The client skips it.
    UnknownMessage,
}

/// Reads one text frame of the server. A type that this version does not know is not an
/// error (ADR 0206).
pub fn decode(text: &str) -> Result<Received, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(text)?;
    let named = |value: &serde_json::Value| value.get("type")?.as_str().map(str::to_owned);
    let Some(message_type) = named(&value) else {
        return serde_json::from_value(value).map(Received::Message);
    };
    if !ServerMessage::NAMES.contains(&message_type.as_str()) {
        return Ok(Received::UnknownMessage);
    }
    if message_type == "event" {
        let event_type = value.get("event").and_then(named);
        let id = value.get("id").and_then(serde_json::Value::as_u64);
        if let (Some(event_type), Some(id)) = (event_type, id)
            && !Event::NAMES.contains(&event_type.as_str())
        {
            return Ok(Received::UnknownEvent { id });
        }
    }
    serde_json::from_value(value).map(Received::Message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{CategoryId, RecoveryId};
    use crate::structure::ChannelKind;

    fn time() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    /// One event of every type.
    pub(crate) fn samples() -> Vec<Event> {
        let account_id = AccountId::generate();
        let device_id = DeviceId::generate();
        let workspace_id = WorkspaceId::generate();
        let channel_id = ChannelId::generate();
        let call_id = CallId::generate();
        let other_account_id = AccountId::generate();
        let place = CallPlace::Channel {
            workspace_id,
            channel_id,
        };
        let direct = CallPlace::direct(account_id, other_account_id);
        let export_id = crate::ids::ExportId::generate();
        let archived = crate::structure::ArchivedChannel {
            channel: Channel {
                id: channel_id,
                workspace_id,
                category_id: None,
                kind: ChannelKind::Text,
                name: "plans".to_owned(),
                position: 0,
                permanent_messages: false,
                user_limit: None,
                hides_occupants: false,
            },
            archived_at: time(),
            deletes_at: Some(time()),
        };
        let participant = CallParticipant {
            avatar: None,
            account_id,
            identity: "seat".to_owned(),
            joined_at: time(),
            muted: true,
            deafened: false,
            microphone_block: None,
            sound_block: None,
            hand_raised: false,
            screen_share: None,
        };
        let workspace = Workspace {
            id: workspace_id,
            name: "Acme".to_owned(),
            is_default: true,
        };
        let channel = Channel {
            id: channel_id,
            workspace_id,
            category_id: None,
            kind: ChannelKind::Text,
            name: "general".to_owned(),
            position: 0,
            permanent_messages: false,
            user_limit: None,
            hides_occupants: false,
        };
        let category = Category {
            id: CategoryId::generate(),
            workspace_id,
            name: "Projects".to_owned(),
            position: 0,
        };
        let workspace_role = WorkspaceRole {
            position: 1,
            id: WorkspaceRoleId::generate(),
            workspace_id,
            name: "helpers".to_owned(),
            permissions: vec![WorkspacePermission::ViewChannel],
            is_everyone: false,
        };
        let server_role = ServerRole {
            position: 1,
            id: ServerRoleId::generate(),
            name: "moderators".to_owned(),
            permissions: vec![junto_authz::ServerPermission::BanMembers],
        };
        let invite = Invite {
            created_by_name: None,
            id: InviteId::generate(),
            created_by: account_id,
            created_at: time(),
            expires_at: time(),
            max_uses: 1,
            uses: 0,
            workspace_ids: vec![workspace_id],
            revocation: None,
        };
        let recording = Recording {
            id: crate::ids::RecordingId::generate(),
            workspace_id,
            channel_id,
            call_id,
            state: crate::recording::RecordingState::Recording,
            started_by: account_id,
            requested_at: time(),
            recording_since: Some(time()),
            stop: Some(crate::recording::RecordingStop {
                reason: crate::recording::RecordingStopReason::CallEnded,
                by: None,
                at: time(),
            }),
            ended_at: None,
            failure: None,
            language: "auto".to_owned(),
            segments: 3,
            failed_segments: 1,
            bytes: Some(1 << 30),
            timeline_started_at: Some(time()),
            transcript: crate::transcript::TranscriptState::Waiting,
            transcript_lines: 0,
        };
        let recording_id = recording.id;
        vec![
            Event::DeviceAdded {
                device_id,
                name: "desk".to_owned(),
                public_key: PublicKey::from_bytes([7; 32]),
                origin: DeviceOrigin::Link,
                created_at: time(),
            },
            Event::DeviceRenamed {
                device_id,
                name: "laptop".to_owned(),
            },
            Event::DeviceRemoved { device_id },
            Event::DeviceLoggedIn {
                device_id,
                logged_in_at: time(),
            },
            Event::DeviceLoggedOut { device_id },
            Event::AccountRecovered {
                recovery: RecoveryNotice {
                    id: RecoveryId::generate(),
                    device_id: Some(device_id),
                    performed_by: None,
                    performed_at: time(),
                },
            },
            Event::PermissionsChanged {
                workspace_id: Some(workspace_id),
            },
            Event::AccountAdded {
                account: AccountSummary {
                    id: account_id,
                    name: "Newcomer".to_owned(),
                    status: AccountStatus::Active,
                    is_owner: false,
                    server_role_ids: vec![],
                    created_at: time(),
                },
            },
            Event::AccountProfileUpdated {
                profile: AccountProfile {
                    id: account_id,
                    name: "Newcomer".to_owned(),
                    avatar: Some(crate::account::Avatar {
                        hash: "00".repeat(32),
                        media_type: crate::account::ImageType::Png,
                        bytes: 512,
                        width: 64,
                        height: 64,
                    }),
                    departure: None,
                },
            },
            Event::AccountStatusChanged {
                account_id,
                status: AccountStatus::Banned,
            },
            Event::AccountServerRolesChanged {
                account_id,
                server_role_ids: vec![server_role.id],
            },
            Event::OwnerChanged {
                account_id,
                previous_owner_id: None,
            },
            Event::ServerRolePositionsShifted {
                positions: vec![crate::roles::ServerRolePosition {
                    role_id: server_role.id,
                    position: 2,
                }],
            },
            Event::WorkspaceRolePositionsShifted {
                workspace_id,
                positions: vec![crate::roles::WorkspaceRolePosition {
                    role_id: workspace_role.id,
                    position: 2,
                }],
            },
            Event::ServerRoleCreated {
                role: server_role.clone(),
            },
            Event::ServerRoleUpdated { role: server_role },
            Event::AccessTokenCreated {
                access_token: crate::access_token::AccessTokenInfo {
                    id: crate::ids::AccessTokenId::generate(),
                    name: "setup".to_owned(),
                    created_by: account_id,
                    actions: vec![crate::access_token::TokenAction::ReadStructure],
                    created_at: time(),
                    expires_at: time(),
                },
            },
            Event::AccessTokenRevoked {
                access_token_id: crate::ids::AccessTokenId::generate(),
            },
            Event::AccessTokensOfCreatorRevoked {
                created_by: account_id,
            },
            Event::InviteCreated {
                invite: invite.clone(),
            },
            Event::InviteChanged {
                invite: invite.clone(),
            },
            Event::InviteRevoked {
                invite_id: invite.id,
                revocation: InviteRevocation::Revoked,
            },
            Event::InviteUsed {
                invite_id: invite.id,
                uses: 1,
            },
            Event::InvitesOfCreatorRevoked {
                created_by: account_id,
                revocation: InviteRevocation::CreatorBanned,
            },
            Event::JoinRequestCreated {
                join_request: JoinRequest {
                    id: JoinRequestId::generate(),
                    name: "Newcomer".to_owned(),
                    created_at: time(),
                    expires_at: time(),
                    returning_account_id: None,
                },
            },
            Event::JoinRequestDecided {
                join_request_id: JoinRequestId::generate(),
                state: JoinRequestState::Approved,
                account_id: Some(account_id),
            },
            Event::WorkspaceCreated {
                workspace: workspace.clone(),
            },
            Event::WorkspaceUpdated { workspace },
            Event::WorkspaceMemberAdded {
                workspace_id,
                member: WorkspaceMember {
                    avatar: None,
                    account_id,
                    name: "Newcomer".to_owned(),
                    role_ids: vec![],
                },
            },
            Event::WorkspaceMemberRemoved {
                workspace_id,
                account_id,
            },
            Event::WorkspaceMemberRolesChanged {
                workspace_id,
                account_id,
                role_ids: vec![workspace_role.id],
            },
            Event::WorkspaceRoleCreated {
                role: workspace_role.clone(),
            },
            Event::WorkspaceRoleUpdated {
                role: workspace_role.clone(),
            },
            Event::CategoryCreated {
                category: category.clone(),
            },
            Event::CategoryUpdated { category },
            Event::ChannelCreated {
                channel: channel.clone(),
            },
            Event::ChannelUpdated { channel },
            Event::ChannelDeleted {
                workspace_id,
                channel_id,
            },
            Event::ChannelOverrideSet {
                workspace_id,
                channel_id,
                role_id: workspace_role.id,
                allow: vec![],
                deny: vec![WorkspacePermission::ViewChannel],
            },
            Event::CallStarted {
                hides_occupants: Some(false),
                call_id,
                place,
                started_at: time(),
            },
            Event::CallEnded { call_id, place },
            Event::CallParticipantJoined {
                call_id,
                place,
                participant: participant.clone(),
            },
            Event::CallParticipantUpdated {
                call_id,
                place,
                participant,
            },
            Event::CallParticipantLeft {
                call_id,
                place,
                account_id,
                reason: LeaveReason::Moved,
            },
            Event::CallLockChanged {
                call_id,
                place,
                lock: Some(CallLock {
                    by: account_id,
                    at: time(),
                }),
            },
            Event::CallMeetingStarted {
                call_id,
                place,
                meeting: Meeting {
                    presenter: account_id,
                    started_at: time(),
                },
            },
            Event::CallMeetingEnded {
                call_id,
                place,
                reason: MeetingEnd::ShareStopped,
            },
            Event::CallSpeaking {
                call_id,
                place,
                account_ids: vec![account_id],
            },
            Event::CallEntryRequested {
                request: EntryRequest {
                    id: EntryRequestId::generate(),
                    call_id,
                    place,
                    account_id,
                    created_at: time(),
                    expires_at: time(),
                },
            },
            Event::CallEntryResolved {
                request_id: EntryRequestId::generate(),
                call_id,
                place,
                account_id,
                outcome: EntryOutcome::Accepted,
                by: Some(account_id),
            },
            Event::CallEntryAnswered {
                request_id: EntryRequestId::generate(),
                outcome: EntryOutcome::Denied,
            },
            Event::CallInvited {
                invitation: CallInvitation {
                    id: CallInvitationId::generate(),
                    call_id,
                    place: direct,
                    account_id,
                    invited_by: other_account_id,
                    created_at: time(),
                    expires_at: time(),
                },
            },
            Event::CallInvitationResolved {
                invitation_id: CallInvitationId::generate(),
                outcome: InvitationOutcome::Declined,
            },
            Event::CallMoveRequested {
                call_move: CallMove {
                    id: CallMoveId::generate(),
                    account_id,
                    from: place,
                    to: place,
                    moved_by: other_account_id,
                    expires_at: time(),
                },
            },
            Event::CallMoveResolved {
                move_id: CallMoveId::generate(),
                outcome: MoveOutcome::TimedOut,
            },
            Event::OwnCallChanged {
                call: Some(OwnCall {
                    hides_occupants: Some(false),
                    call_id,
                    place,
                    device_id,
                    identity: "seat".to_owned(),
                }),
                left: None,
            },
            Event::CallAccessEnded {},
            Event::DirectMuteSet {
                mute: DirectMute {
                    account_id: other_account_id,
                    until: Some(time()),
                },
            },
            Event::DirectMuteRemoved {
                account_id: other_account_id,
            },
            Event::RecordingStarted {
                recording: Box::new(recording.clone()),
            },
            Event::RecordingUpdated {
                recording: Box::new(recording),
            },
            Event::RecordingRemoved {
                recording_id,
                workspace_id,
                channel_id,
            },
            Event::TranscriptLineCorrected {
                recording_id,
                workspace_id,
                channel_id,
                line: 4,
            },
            Event::TranscriptSpeakerRenamed {
                recording_id,
                workspace_id,
                channel_id,
                speaker: 1,
            },
            Event::GlossaryReplaced { workspace_id },
            Event::MessageChanged {
                place: crate::message::MessagePlace::Channel {
                    workspace_id,
                    channel_id,
                },
                message_id: crate::ids::MessageId::generate(),
                position: 1,
                deleted: false,
            },
            Event::ThreadRead {
                root_id: crate::ids::MessageId::generate(),
                position: 1,
            },
            Event::MessageRead {
                place: crate::message::MessagePlace::Channel {
                    workspace_id,
                    channel_id,
                },
                position: 1,
            },
            Event::MessageMentioned {
                workspace_id,
                channel_id,
                message_id: crate::ids::MessageId::generate(),
                position: 1,
            },
            Event::ConversationOpened {
                conversation: crate::message::Conversation {
                    id: crate::ids::ConversationId::generate(),
                    low_account_id: account_id,
                    high_account_id: other_account_id,
                },
            },
            Event::ServerLogoChanged { logo: None },
            Event::AttachmentLimitsChanged {
                limits: crate::message::AttachmentLimits {
                    bytes_per_file: 100_000_000,
                    files_per_message: 10,
                },
            },
            Event::NotificationChanged {
                message_id: crate::ids::MessageId::generate(),
            },
            Event::NotificationSettingsChanged {},
            Event::RetentionChanged {
                workspace_id: Some(workspace_id),
            },
            Event::ChannelArchived {
                channel: archived.clone(),
            },
            Event::ChannelRestored {
                channel: archived.channel.clone(),
            },
            Event::ChannelDeletionDue { channel: archived },
            Event::CategoryDeleted {
                workspace_id,
                category_id: CategoryId::generate(),
            },
            Event::WorkspaceRoleDeleted {
                workspace_id,
                role_id: WorkspaceRoleId::generate(),
            },
            Event::ServerRoleDeleted {
                role_id: ServerRoleId::generate(),
            },
            Event::WorkspaceDeleted { workspace_id },
            Event::ExportUpdated {
                export: crate::export::Export {
                    id: export_id,
                    place: crate::message::MessagePlace::Channel {
                        workspace_id,
                        channel_id,
                    },
                    state: crate::export::ExportState::Ready,
                    failure: None,
                    requested_at: time(),
                    finished_at: Some(time()),
                    expires_at: Some(time()),
                    bytes: Some(1024),
                },
            },
            Event::ExportRemoved {
                export_id,
                reason: crate::export::ExportRemoval::Expired,
            },
            Event::NotificationsRead {
                until: time(),
                read_at: time(),
            },
            Event::ThreadFollowChanged {
                root_id: crate::ids::MessageId::generate(),
                following: false,
                follow_setting: Some(false),
            },
            Event::NotificationDefaultChanged {
                workspace_id,
                level: Some(crate::discovery::NotificationLevel::All),
            },
            Event::AccountDepartureChanged {
                account_id,
                departure: Some(crate::account::Departure::Inactive),
            },
        ]
    }

    #[test]
    fn every_event_type_has_one_name_that_is_its_wire_tag() {
        let samples = samples();
        let names: Vec<&str> = samples.iter().map(Event::name).collect();
        assert_eq!(names, Event::NAMES);
        for event in samples {
            let json = serde_json::to_value(&event).unwrap();
            assert_eq!(json["type"], event.name());
            assert_eq!(serde_json::from_value::<Event>(json).unwrap(), event);
        }
    }

    #[test]
    fn server_messages_round_trip_and_carry_their_names() {
        let cursor = Cursor {
            epoch: StreamEpoch::generate(),
            position: 41,
        };
        let messages = [
            ServerMessage::Hello {
                server_version: "0.1.0".to_owned(),
                api_versions: vec![1],
                epoch: cursor.epoch,
                heartbeat_seconds: 25,
                idle_seconds: 60,
            },
            ServerMessage::Subscribed {
                level: Level::Background,
                cursor,
            },
            ServerMessage::Event {
                id: 42,
                at: time(),
                event: samples().remove(0),
            },
            ServerMessage::Checkpoint { cursor },
            ServerMessage::Resync {
                reason: ResyncReason::TooFarBehind,
            },
            ServerMessage::Ended {
                reason: EndReason::Banned,
                recovery: None,
            },
        ];
        let mut names = Vec::new();
        for message in messages {
            let text = serde_json::to_string(&message).unwrap();
            let json: serde_json::Value = serde_json::from_str(&text).unwrap();
            names.push(json["type"].as_str().unwrap().to_owned());
            assert_eq!(decode(&text).unwrap(), Received::Message(message));
        }
        assert_eq!(names, ServerMessage::NAMES);
    }

    #[test]
    fn a_subscribe_without_a_cursor_starts_after_the_newest_event() {
        let message: ClientMessage =
            serde_json::from_str(r#"{"type":"subscribe","level":"full"}"#).unwrap();
        assert_eq!(
            message,
            ClientMessage::Subscribe {
                level: Level::Full,
                cursor: None
            }
        );
        assert!(serde_json::from_str::<ClientMessage>(r#"{"type":"listen"}"#).is_err());
    }

    #[test]
    fn unknown_types_are_skipped_and_unknown_fields_are_ignored() {
        let unknown_event = r#"{"type":"event","id":9,"at":"2030-01-01T00:00:00Z",
            "event":{"type":"message_created","text":"hello"}}"#;
        assert_eq!(
            decode(unknown_event).unwrap(),
            Received::UnknownEvent { id: 9 }
        );
        assert_eq!(
            decode(r#"{"type":"notice","text":"hello"}"#).unwrap(),
            Received::UnknownMessage
        );
        let with_new_field = r#"{"type":"event","id":9,"at":"2030-01-01T00:00:00Z","new":1,
            "event":{"type":"device_removed","new":true,
                     "device_id":"0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f"}}"#;
        assert!(matches!(
            decode(with_new_field).unwrap(),
            Received::Message(ServerMessage::Event { id: 9, .. })
        ));
        assert!(decode(r#"{"type":"event","id":9}"#).is_err());
        assert!(decode("[]").is_err());
    }

    #[test]
    fn close_codes_are_distinct_and_named() {
        let mut codes: Vec<u16> = CloseCode::ALL.iter().map(|code| code.code()).collect();
        codes.sort_unstable();
        codes.dedup();
        assert_eq!(codes.len(), CloseCode::ALL.len());
        for code in CloseCode::ALL {
            assert_eq!(CloseCode::from_code(code.code()), Some(*code));
            assert!(!code.name().is_empty());
        }
        assert_eq!(CloseCode::from_code(1000), None);
    }

    #[test]
    fn a_status_and_an_end_reason_of_a_newer_server_are_read_as_unknown_and_never_written() {
        let ended: ServerMessage =
            serde_json::from_str(r#"{"type":"ended","reason":"archived","recovery":null}"#)
                .unwrap();
        assert_eq!(
            ended,
            ServerMessage::Ended {
                reason: EndReason::Unknown,
                recovery: None
            }
        );
        assert!(serde_json::to_string(&ended).is_err());
        let changed: Event = serde_json::from_str(
            r#"{"type":"account_status_changed",
                "account_id":"0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f","status":"archived"}"#,
        )
        .unwrap();
        assert!(matches!(
            changed,
            Event::AccountStatusChanged {
                status: AccountStatus::Unknown,
                ..
            }
        ));
        assert!(serde_json::to_string(&changed).is_err());
        for (status, name) in [
            (AccountStatus::Inactive, "inactive"),
            (AccountStatus::Deleted, "deleted"),
        ] {
            assert_eq!(serde_json::to_value(status).unwrap(), name);
        }
        for (reason, name) in [
            (EndReason::Inactive, "inactive"),
            (EndReason::Deleted, "deleted"),
        ] {
            assert_eq!(serde_json::to_value(reason).unwrap(), name);
        }
    }

    #[test]
    fn a_profile_without_a_departure_is_read_as_an_active_account() {
        let profile: AccountProfile = serde_json::from_str(
            r#"{"id":"0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f","name":"Ana","avatar":null}"#,
        )
        .unwrap();
        assert_eq!(profile.departure, None);
        let departed = AccountProfile {
            departure: Some(Departure::Deleted),
            ..profile
        };
        let json = serde_json::to_value(&departed).unwrap();
        assert_eq!(json["departure"], "deleted");
    }
}
