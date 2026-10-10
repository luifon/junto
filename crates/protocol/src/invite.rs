use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::auth::Proof;
use crate::ids::{AccountId, InviteId, JoinRequestId, WorkspaceId};
use crate::secret::InviteCode;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateInvite {
    /// Absent: the server's default lifetime applies.
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    /// Absent: one use.
    #[serde(default)]
    pub max_uses: Option<u32>,
    /// Empty: the default workspace.
    #[serde(default)]
    pub workspace_ids: Vec<WorkspaceId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum InviteRevocation {
    Revoked,
    CreatorBanned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Invite {
    pub id: InviteId,
    pub created_by: AccountId,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
    pub max_uses: u32,
    pub uses: u32,
    pub workspace_ids: Vec<WorkspaceId>,
    pub revocation: Option<InviteRevocation>,
}

/// The only response that carries the code of an invite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct InviteCreated {
    pub invite: Invite,
    pub code: InviteCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Join {
    pub proof: Proof,
    pub invite_code: InviteCode,
    pub name: String,
    #[serde(default)]
    pub device_name: String,
}

/// Sent by a logged-in account to enter the workspaces of an invite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RedeemInvite {
    pub invite_code: InviteCode,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Redemption {
    /// The workspaces that the account entered with this redemption.
    pub workspace_ids: Vec<WorkspaceId>,
}

/// A join request carries a name and nothing else about the person.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RequestToJoin {
    pub proof: Proof,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JoinRequest {
    pub id: JoinRequestId,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JoinRequestStatusQuery {
    pub proof: Proof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum JoinRequestState {
    Pending,
    Approved,
    Denied,
    Expired,
}

/// The result that the waiting device collects with its device key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct JoinRequestStatus {
    pub id: JoinRequestId,
    pub state: JoinRequestState,
    /// Present once the request is approved. The device then logs in.
    pub account_id: Option<AccountId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApproveJoinRequest {
    /// Empty: the default workspace.
    #[serde(default)]
    pub workspace_ids: Vec<WorkspaceId>,
}
