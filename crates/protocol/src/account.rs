use junto_authz::ServerPermission;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::auth::Proof;
use crate::ids::{AccountId, DeviceId, RecoveryId, ServerRoleId, WorkspaceId};
use crate::keys::PublicKey;
use crate::secret::LinkCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum AccountStatus {
    Active,
    Banned,
    Removed,
}

/// The formats in which the server stores an avatar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub enum ImageType {
    #[serde(rename = "image/png")]
    Png,
    #[serde(rename = "image/jpeg")]
    Jpeg,
    #[serde(rename = "image/webp")]
    Webp,
}

impl ImageType {
    pub const ALL: &'static [Self] = &[Self::Png, Self::Jpeg, Self::Webp];

    /// The media type, as the avatar route sends it in `Content-Type`.
    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }
}

/// The stored avatar of an account. The image itself is read from
/// `GET /api/v1/accounts/{account_id}/avatar`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Avatar {
    /// The SHA-256 hash of the image in lower-case hexadecimal. It changes when the image
    /// changes, so a client keeps the image for as long as it sees the same hash.
    pub hash: String,
    pub media_type: ImageType,
    pub bytes: u32,
    pub width: u32,
    pub height: u32,
}

/// What an account shows of itself to the accounts that may see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct AccountProfile {
    pub id: AccountId,
    pub name: String,
    pub avatar: Option<Avatar>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateProfile {
    pub name: String,
}

/// The account of the caller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct OwnAccount {
    pub id: AccountId,
    pub name: String,
    pub avatar: Option<Avatar>,
    pub is_owner: bool,
    pub server_role_ids: Vec<ServerRoleId>,
    /// The union of the permissions of the account's server roles.
    pub server_permissions: Vec<ServerPermission>,
    pub workspace_ids: Vec<WorkspaceId>,
}

/// An account as a moderator sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum DeviceOrigin {
    OwnerCode,
    Invite,
    JoinRequest,
    Link,
    Recovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RenameDevice {
    pub name: String,
}

/// Sent by a new device that has no account on this server yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RequestDeviceLink {
    pub proof: Proof,
    #[serde(default)]
    pub device_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct DeviceLinkCreated {
    pub code: LinkCode,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct LookUpDeviceLink {
    pub code: LinkCode,
}

/// A device link that waits for the approval of a device of the account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct PendingDeviceLink {
    pub public_key: PublicKey,
    pub device_name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

/// The approval names the public key that the approving device saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApproveDeviceLink {
    pub code: LinkCode,
    pub public_key: PublicKey,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RecoverAccount {
    pub public_key: PublicKey,
    #[serde(default)]
    pub device_name: String,
}

/// One recovery of an account, as the account sees it. A device keeps the notice of a
/// recovery until it acknowledges it (ADR 0112).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RecoveryNotice {
    pub id: RecoveryId,
    pub device_id: Option<DeviceId>,
    /// Absent when the recovery was done with the host command.
    pub performed_by: Option<AccountId>,
    #[serde(with = "time::serde::rfc3339")]
    pub performed_at: OffsetDateTime,
}

/// An optional explanation retained in the moderation audit entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ModerationReason {
    /// At most 1000 characters without control characters. Empty means no reason.
    #[serde(default)]
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_type_is_written_as_its_media_type() {
        for kind in ImageType::ALL {
            assert_eq!(
                serde_json::to_value(kind).unwrap(),
                serde_json::Value::from(kind.media_type())
            );
        }
        assert!(serde_json::from_str::<ImageType>("\"image/gif\"").is_err());
    }
}
