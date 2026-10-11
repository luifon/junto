use junto_authz::{ServerPermission, WorkspacePermission};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::ids::{
    AccountId, AuditEntryId, CategoryId, ChannelId, DeviceId, InviteId, JoinRequestId, RecordingId,
    ServerRoleId, WorkspaceId, WorkspaceRoleId,
};
use crate::recording::RecordingStopReason;
use crate::structure::ChannelKind;

/// What was done. New actions are added; the fields of an existing action do not change.
///
/// An action that this build does not know is read as [`AuditEvent::Unknown`], so that a
/// client can show the entry with its time and its actor (ADR 0209). `Unknown` is never
/// written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum AuditEvent {
    OwnerClaimed {
        account_id: AccountId,
    },
    /// Done with the host command, so the entry has no acting account.
    OwnerAssigned {
        account_id: AccountId,
        previous_owner_id: Option<AccountId>,
    },
    /// The host command stored a new owner code. The entry does not contain the code.
    OwnerCodeIssued {
        /// True when a stored code stopped working because of the new one.
        replaced_a_code: bool,
        #[serde(with = "time::serde::rfc3339::option")]
        expires_at: Option<OffsetDateTime>,
    },
    /// The owner returned with a code that the host command issued.
    OwnerAccessRestored {
        account_id: AccountId,
        device_id: DeviceId,
    },
    /// The host command gave the event stream a new epoch, as after a restore.
    EventEpochRenewed,
    InviteCreated {
        invite_id: InviteId,
        #[serde(with = "time::serde::rfc3339")]
        expires_at: OffsetDateTime,
        max_uses: u32,
        workspace_ids: Vec<WorkspaceId>,
    },
    InviteRevoked {
        invite_id: InviteId,
    },
    /// States which invite created the account and which account created that invite.
    AccountJoined {
        account_id: AccountId,
        invite_id: InviteId,
        invite_created_by: AccountId,
    },
    /// An existing account used an invite to enter further workspaces.
    InviteRedeemed {
        account_id: AccountId,
        invite_id: InviteId,
        invite_created_by: AccountId,
        workspace_ids: Vec<WorkspaceId>,
    },
    JoinRequestApproved {
        join_request_id: JoinRequestId,
        account_id: AccountId,
        workspace_ids: Vec<WorkspaceId>,
    },
    JoinRequestDenied {
        join_request_id: JoinRequestId,
    },
    AccountRemoved {
        account_id: AccountId,
    },
    AccountBanned {
        account_id: AccountId,
        cancelled_invite_ids: Vec<InviteId>,
    },
    AccountUnbanned {
        account_id: AccountId,
    },
    AccountRecovered {
        account_id: AccountId,
        device_id: DeviceId,
    },
    ServerRoleCreated {
        role_id: ServerRoleId,
        name: String,
        permissions: Vec<ServerPermission>,
    },
    ServerRoleUpdated {
        role_id: ServerRoleId,
        name: String,
        permissions: Vec<ServerPermission>,
    },
    ServerRoleAssigned {
        role_id: ServerRoleId,
        account_id: AccountId,
    },
    ServerRoleUnassigned {
        role_id: ServerRoleId,
        account_id: AccountId,
    },
    WorkspaceCreated {
        workspace_id: WorkspaceId,
        name: String,
    },
    WorkspaceRenamed {
        workspace_id: WorkspaceId,
        name: String,
    },
    WorkspaceRoleCreated {
        role_id: WorkspaceRoleId,
        name: String,
        permissions: Vec<WorkspacePermission>,
    },
    WorkspaceRoleUpdated {
        role_id: WorkspaceRoleId,
        name: String,
        permissions: Vec<WorkspacePermission>,
    },
    WorkspaceRoleAssigned {
        role_id: WorkspaceRoleId,
        account_id: AccountId,
    },
    WorkspaceRoleUnassigned {
        role_id: WorkspaceRoleId,
        account_id: AccountId,
    },
    /// An administrator added an existing account to the workspace.
    WorkspaceMemberAdded {
        account_id: AccountId,
    },
    CategoryCreated {
        category_id: CategoryId,
        name: String,
    },
    CategoryUpdated {
        category_id: CategoryId,
        name: String,
        position: i32,
    },
    ChannelCreated {
        name: String,
        kind: ChannelKind,
    },
    ChannelUpdated {
        name: String,
        category_id: Option<CategoryId>,
        position: i32,
        permanent_messages: bool,
    },
    ChannelOverrideSet {
        role_id: WorkspaceRoleId,
        allow: Vec<WorkspacePermission>,
        deny: Vec<WorkspacePermission>,
    },
    ChannelDeleted {
        name: String,
    },
    /// The user limit or the setting that hides who is inside changed.
    ChannelCallSettingsUpdated {
        user_limit: Option<u32>,
        hides_occupants: bool,
    },
    /// A moderator blocked the microphone of a person in the call of the channel.
    CallMicrophoneBlocked {
        account_id: AccountId,
    },
    CallMicrophoneUnblocked {
        account_id: AccountId,
    },
    /// A moderator blocked the sound of a person in the call of the channel.
    CallSoundBlocked {
        account_id: AccountId,
    },
    CallSoundUnblocked {
        account_id: AccountId,
    },
    /// A moderator removed a person from the call of the channel.
    CallMemberDisconnected {
        account_id: AccountId,
    },
    /// A moderator moved a person out of the call of the channel into another voice
    /// channel. The entry does not name that channel.
    CallMemberMovedOut {
        account_id: AccountId,
    },
    /// A moderator moved a person into the call of the channel from another voice channel.
    /// The entry does not name that channel.
    CallMemberMovedIn {
        account_id: AccountId,
    },
    /// A person in the call of the channel started a recording.
    RecordingStarted {
        recording_id: RecordingId,
    },
    /// A recording of the call of the channel was stopped. The entry has no acting account
    /// when the server stopped it.
    RecordingStopped {
        recording_id: RecordingId,
        reason: RecordingStopReason,
    },
    /// A person removed a recording of the channel with its files and its transcript.
    RecordingRemoved {
        recording_id: RecordingId,
    },
    /// A person asked for the transcript of a recording of the channel again.
    TranscriptRequested {
        recording_id: RecordingId,
    },
    /// A person replaced the glossary of the workspace. The entry states the number of
    /// terms, not the terms.
    GlossaryReplaced {
        terms: u32,
    },
    /// An action that this build does not know. It is read and never written.
    #[serde(other, skip_serializing)]
    Unknown,
}

impl AuditEvent {
    pub const fn action(&self) -> &'static str {
        match self {
            Self::OwnerClaimed { .. } => "owner_claimed",
            Self::OwnerAssigned { .. } => "owner_assigned",
            Self::OwnerCodeIssued { .. } => "owner_code_issued",
            Self::OwnerAccessRestored { .. } => "owner_access_restored",
            Self::EventEpochRenewed => "event_epoch_renewed",
            Self::InviteRedeemed { .. } => "invite_redeemed",
            Self::WorkspaceMemberAdded { .. } => "workspace_member_added",
            Self::InviteCreated { .. } => "invite_created",
            Self::InviteRevoked { .. } => "invite_revoked",
            Self::AccountJoined { .. } => "account_joined",
            Self::JoinRequestApproved { .. } => "join_request_approved",
            Self::JoinRequestDenied { .. } => "join_request_denied",
            Self::AccountRemoved { .. } => "account_removed",
            Self::AccountBanned { .. } => "account_banned",
            Self::AccountUnbanned { .. } => "account_unbanned",
            Self::AccountRecovered { .. } => "account_recovered",
            Self::ServerRoleCreated { .. } => "server_role_created",
            Self::ServerRoleUpdated { .. } => "server_role_updated",
            Self::ServerRoleAssigned { .. } => "server_role_assigned",
            Self::ServerRoleUnassigned { .. } => "server_role_unassigned",
            Self::WorkspaceCreated { .. } => "workspace_created",
            Self::WorkspaceRenamed { .. } => "workspace_renamed",
            Self::WorkspaceRoleCreated { .. } => "workspace_role_created",
            Self::WorkspaceRoleUpdated { .. } => "workspace_role_updated",
            Self::WorkspaceRoleAssigned { .. } => "workspace_role_assigned",
            Self::WorkspaceRoleUnassigned { .. } => "workspace_role_unassigned",
            Self::CategoryCreated { .. } => "category_created",
            Self::CategoryUpdated { .. } => "category_updated",
            Self::ChannelCreated { .. } => "channel_created",
            Self::ChannelUpdated { .. } => "channel_updated",
            Self::ChannelOverrideSet { .. } => "channel_override_set",
            Self::ChannelDeleted { .. } => "channel_deleted",
            Self::ChannelCallSettingsUpdated { .. } => "channel_call_settings_updated",
            Self::CallMicrophoneBlocked { .. } => "call_microphone_blocked",
            Self::CallMicrophoneUnblocked { .. } => "call_microphone_unblocked",
            Self::CallSoundBlocked { .. } => "call_sound_blocked",
            Self::CallSoundUnblocked { .. } => "call_sound_unblocked",
            Self::CallMemberDisconnected { .. } => "call_member_disconnected",
            Self::CallMemberMovedOut { .. } => "call_member_moved_out",
            Self::CallMemberMovedIn { .. } => "call_member_moved_in",
            Self::RecordingStarted { .. } => "recording_started",
            Self::RecordingStopped { .. } => "recording_stopped",
            Self::RecordingRemoved { .. } => "recording_removed",
            Self::TranscriptRequested { .. } => "transcript_requested",
            Self::GlossaryReplaced { .. } => "glossary_replaced",
            Self::Unknown => "unknown",
        }
    }
}

/// One line of the audit log: who did it, what was done and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AuditEntry {
    pub id: AuditEntryId,
    #[serde(with = "time::serde::rfc3339")]
    pub occurred_at: OffsetDateTime,
    /// Absent when the server itself acted.
    pub actor_id: Option<AccountId>,
    pub workspace_id: Option<WorkspaceId>,
    pub channel_id: Option<ChannelId>,
    pub event: AuditEvent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AuditPage {
    /// Newest first.
    pub entries: Vec<AuditEntry>,
    /// Pass as `before` to read the next page. Absent on the last page.
    pub next_before: Option<AuditEntryId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::IntoParams))]
#[cfg_attr(feature = "openapi", into_params(parameter_in = Query))]
pub struct AuditQuery {
    /// Return entries older than this one.
    #[serde(default)]
    pub before: Option<AuditEntryId>,
    /// The largest number of entries to return, from 1 to 200. Absent: 50.
    #[serde(default)]
    pub limit: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_matches_the_wire_tag() {
        let account_id = AccountId::generate();
        let events = [
            AuditEvent::OwnerClaimed { account_id },
            AuditEvent::AccountBanned {
                account_id,
                cancelled_invite_ids: vec![InviteId::generate()],
            },
            AuditEvent::ChannelDeleted {
                name: "general".to_owned(),
            },
            AuditEvent::ChannelOverrideSet {
                role_id: WorkspaceRoleId::generate(),
                allow: vec![WorkspacePermission::ViewChannel],
                deny: vec![],
            },
            AuditEvent::ChannelCallSettingsUpdated {
                user_limit: Some(4),
                hides_occupants: true,
            },
            AuditEvent::CallMicrophoneBlocked { account_id },
            AuditEvent::CallMicrophoneUnblocked { account_id },
            AuditEvent::CallSoundBlocked { account_id },
            AuditEvent::CallSoundUnblocked { account_id },
            AuditEvent::CallMemberDisconnected { account_id },
            AuditEvent::CallMemberMovedOut { account_id },
            AuditEvent::CallMemberMovedIn { account_id },
            AuditEvent::RecordingStarted {
                recording_id: RecordingId::generate(),
            },
            AuditEvent::RecordingStopped {
                recording_id: RecordingId::generate(),
                reason: RecordingStopReason::StorageReserve,
            },
            AuditEvent::RecordingRemoved {
                recording_id: RecordingId::generate(),
            },
            AuditEvent::TranscriptRequested {
                recording_id: RecordingId::generate(),
            },
            AuditEvent::GlossaryReplaced { terms: 12 },
        ];
        for event in events {
            let json = serde_json::to_value(&event).unwrap();
            assert_eq!(json["action"], event.action());
            assert_eq!(serde_json::from_value::<AuditEvent>(json).unwrap(), event);
        }
    }

    #[test]
    fn an_action_of_a_newer_server_is_read_as_unknown_and_never_written() {
        let entry: AuditEntry = serde_json::from_str(
            r#"{"id":"0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f",
                "occurred_at":"2030-01-01T00:00:00Z",
                "actor_id":null,"workspace_id":null,"channel_id":null,
                "event":{"action":"glossary_updated","terms":3}}"#,
        )
        .unwrap();
        assert_eq!(entry.event, AuditEvent::Unknown);
        assert_eq!(entry.event.action(), "unknown");
        assert!(serde_json::to_string(&entry).is_err());
        assert!(serde_json::to_string(&AuditEvent::Unknown).is_err());
        assert!(serde_json::from_str::<AuditEvent>(r#"{"name":"general"}"#).is_err());
    }
}
