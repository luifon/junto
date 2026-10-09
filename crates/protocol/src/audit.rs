use junto_authz::{ServerPermission, WorkspacePermission};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};

use crate::ids::{
    AccountId, AuditEntryId, CategoryId, ChannelId, DeviceId, InviteId, JoinRequestId,
    ServerRoleId, WorkspaceId, WorkspaceRoleId,
};
use crate::structure::ChannelKind;

/// What was done. New actions are added; the fields of an existing action do not change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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
        }
    }
}

/// One line of the audit log: who did it, what was done and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AuditPage {
    /// Newest first.
    pub entries: Vec<AuditEntry>,
    /// Pass as `before` to read the next page. Absent on the last page.
    pub next_before: Option<AuditEntryId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
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
        ];
        for event in events {
            let json = serde_json::to_value(&event).unwrap();
            assert_eq!(json["action"], event.action());
            assert_eq!(serde_json::from_value::<AuditEvent>(json).unwrap(), event);
        }
    }
}
