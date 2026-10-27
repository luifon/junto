use junto_authz::{ServerPermission, WorkspacePermission};
use serde::{Deserialize, Serialize};

use crate::ids::{AccountId, ServerRoleId, WorkspaceId, WorkspaceRoleId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ServerRole {
    #[serde(default)]
    pub position: i32,
    pub id: ServerRoleId,
    pub name: String,
    pub permissions: Vec<ServerPermission>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateServerRole {
    pub name: String,
    #[serde(default)]
    pub permissions: Vec<ServerPermission>,
}

/// An absent field keeps its value.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateServerRole {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub permissions: Option<Vec<ServerPermission>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WorkspaceRole {
    #[serde(default)]
    pub position: i32,
    pub id: WorkspaceRoleId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub permissions: Vec<WorkspacePermission>,
    /// True for the role that every member of the workspace holds. It is never assigned.
    pub is_everyone: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateWorkspaceRole {
    pub name: String,
    #[serde(default)]
    pub permissions: Vec<WorkspacePermission>,
}

/// An absent field keeps its value.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateWorkspaceRole {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub permissions: Option<Vec<WorkspacePermission>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ServerRolePosition {
    pub role_id: ServerRoleId,
    pub position: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WorkspaceRolePosition {
    pub role_id: WorkspaceRoleId,
    pub position: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReorderServerRoles {
    pub positions: Vec<ServerRolePosition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ReorderWorkspaceRoles {
    pub positions: Vec<WorkspaceRolePosition>,
}

/// Role-level authority; target membership and assignment-count limits still apply.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct RoleActions {
    #[serde(default)]
    pub may_assign: bool,
    #[serde(default)]
    pub may_remove: bool,
    #[serde(default)]
    pub may_delete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ListedServerRole {
    #[serde(flatten)]
    pub role: ServerRole,
    #[serde(flatten)]
    pub actions: RoleActions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ListedWorkspaceRole {
    #[serde(flatten)]
    pub role: WorkspaceRole,
    #[serde(flatten)]
    pub actions: RoleActions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ChangeServerRolePermission {
    pub permission: ServerPermission,
    pub enabled: bool,
}

/// The server permissions that are allowed to or denied to one account, after its server
/// roles (ADR 0301).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ServerPersonPermissions {
    pub account_id: AccountId,
    pub allow: Vec<ServerPermission>,
    pub deny: Vec<ServerPermission>,
}

/// Two empty sets remove the server permissions of the account.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetServerPersonPermissions {
    #[serde(default)]
    pub allow: Vec<ServerPermission>,
    #[serde(default)]
    pub deny: Vec<ServerPermission>,
}

/// Changes exactly one server permission of an account, preserving the others.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ChangeServerPersonPermission {
    pub permission: ServerPermission,
    pub state: crate::structure::OverrideState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ChangeWorkspaceRolePermission {
    pub permission: WorkspacePermission,
    pub enabled: bool,
}
