use junto_authz::ServerPermission;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use utoipa::ToSchema;

use crate::auth::Proof;
use crate::ids::{AccountId, DeviceId, RecoveryId, ServerRoleId, WorkspaceId};
use crate::keys::PublicKey;
use crate::secret::LinkCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    Active,
    Banned,
    Removed,
}

/// The account of the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct OwnAccount {
    pub id: AccountId,
    pub name: String,
    pub is_owner: bool,
    pub server_role_ids: Vec<ServerRoleId>,
    /// The union of the permissions of the account's server roles.
    pub server_permissions: Vec<ServerPermission>,
    pub workspace_ids: Vec<WorkspaceId>,
}

/// An account as a moderator sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AccountSummary {
    pub id: AccountId,
    pub name: String,
    pub status: AccountStatus,
    pub is_owner: bool,
    pub server_role_ids: Vec<ServerRoleId>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

/// How a device key came to belong to its account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceOrigin {
    OwnerCode,
    Invite,
    JoinRequest,
    Link,
    Recovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Device {
    pub id: DeviceId,
    pub name: String,
    pub public_key: PublicKey,
    pub origin: DeviceOrigin,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_login_at: Option<OffsetDateTime>,
    /// True for the device of the session that made the request.
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RenameDevice {
    pub name: String,
}

/// Sent by a new device that has no account on this server yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RequestDeviceLink {
    pub proof: Proof,
    #[serde(default)]
    pub device_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DeviceLinkCreated {
    pub code: LinkCode,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct LookUpDeviceLink {
    pub code: LinkCode,
}

/// A device link that waits for the approval of a device of the account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct PendingDeviceLink {
    pub public_key: PublicKey,
    pub device_name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// The approval names the public key that the approving device saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ApproveDeviceLink {
    pub code: LinkCode,
    pub public_key: PublicKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RecoverAccount {
    pub public_key: PublicKey,
    #[serde(default)]
    pub device_name: String,
}

/// One recovery of an account, as the account sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RecoveryNotice {
    pub id: RecoveryId,
    pub device_id: Option<DeviceId>,
    /// Absent when the recovery was done with the host command.
    pub performed_by: Option<AccountId>,
    #[serde(with = "time::serde::rfc3339")]
    pub performed_at: OffsetDateTime,
}
