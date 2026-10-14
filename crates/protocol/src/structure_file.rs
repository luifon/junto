//! The structure of a server as a file, and the changes that apply a file (ADR 0204,
//! ADR 0211, ADR 0212).

// The schema function that the derive macro makes for `StructureChange` is one expression
// over every variant, and clippy counts it at about the bound of `large_stack_frames`:
// above it with some features of the crate and below it with others, so the lint is
// allowed here and not expected. The crate is compiled with optimization also in the dev
// profile, where that function needs little stack, and a test of the server bounds the
// stack of all schemas together (ADR 0906).
#![cfg_attr(
    feature = "openapi",
    allow(
        clippy::large_stack_frames,
        reason = "one generated schema function for every variant of a large enum"
    )
)]

use junto_authz::{ServerPermission, WorkspacePermission};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::ids::StructureId;
use crate::structure::{ChannelKind, present};

/// The format version that this build reads and writes.
pub const STRUCTURE_FORMAT: u32 = 2;

/// The largest number of changes of one request. A request of this size runs in a small
/// part of the time limit of a request (ADR 0211).
pub const MOST_CHANGES: usize = 200;

/// The structure of a server: its server roles and its workspaces with their roles,
/// categories, channels and overrides.
///
/// An entry is matched by its identifier. An entry that is absent is kept as it is. In an
/// entry, an absent optional field keeps its value. A file contains no member, no message
/// and no secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct StructureFile {
    /// The version of the file format: 2. This tool also reads format 1.
    pub format: u32,
    #[serde(default)]
    pub server_roles: Vec<ServerRoleEntry>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
    /// The entries that applying the file deletes. Nothing else is deleted.
    #[serde(default, skip_serializing_if = "Deletions::is_empty")]
    pub delete: Deletions,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct Deletions {
    /// The identifiers of the channels to delete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<StructureId>,
    /// The identifiers of the categories to delete. Their channels stay and have no
    /// category.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<StructureId>,
    /// The identifiers of the workspace roles to delete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub roles: Vec<StructureId>,
    /// The identifiers of the server roles to delete.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub server_roles: Vec<StructureId>,
    /// The identifiers of the workspaces to delete, each with everything that it holds.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub workspaces: Vec<StructureId>,
}

impl Deletions {
    pub fn is_empty(&self) -> bool {
        self.channels.is_empty()
            && self.categories.is_empty()
            && self.roles.is_empty()
            && self.server_roles.is_empty()
            && self.workspaces.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ServerRoleEntry {
    /// Optional relative-order key: larger is higher. Numeric gaps are not significant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    pub id: StructureId,
    pub name: String,
    /// Absent: a new role has no permission, an existing role keeps its permissions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<ServerPermission>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct WorkspaceEntry {
    pub id: StructureId,
    pub name: String,
    /// True for the default workspace. An entry with this mark whose identifier the server
    /// does not know stands for the default workspace of the server.
    #[serde(default, skip_serializing_if = "is_false")]
    pub default: bool,
    #[serde(default)]
    pub roles: Vec<RoleEntry>,
    #[serde(default)]
    pub categories: Vec<CategoryEntry>,
    #[serde(default)]
    pub channels: Vec<ChannelEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct RoleEntry {
    /// Optional relative-order key: larger is higher; everyone is fixed at zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    pub id: StructureId,
    pub name: String,
    /// True for the role that every member holds. An entry with this mark stands for that
    /// role of its workspace, whatever its identifier is.
    #[serde(default, skip_serializing_if = "is_false")]
    pub everyone: bool,
    /// Absent: a new role has no permission, an existing role keeps its permissions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Vec<WorkspacePermission>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct CategoryEntry {
    pub id: StructureId,
    pub name: String,
    /// Absent: a new category goes after the last one, an existing one keeps its position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct ChannelEntry {
    pub id: StructureId,
    pub name: String,
    /// The kind of a channel does not change.
    pub kind: ChannelKind,
    /// The identifier of a category of the same workspace. `null`: no category. Absent: a
    /// new channel has no category, an existing channel keeps its category.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<StructureId>))]
    pub category: Option<Option<StructureId>>,
    /// Absent: a new channel goes after the last one, an existing one keeps its position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permanent_messages: Option<bool>,
    /// For a voice channel only. `null`: no limit.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<u32>))]
    pub user_limit: Option<Option<u32>>,
    /// For a voice channel only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hides_occupants: Option<bool>,
    /// The overrides that the file states. An override of a role that is absent here is
    /// kept; an override with two empty sets is removed.
    #[serde(default)]
    pub overrides: Vec<OverrideEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(deny_unknown_fields)]
pub struct OverrideEntry {
    /// The identifier of a role of the same workspace.
    pub role: StructureId,
    #[serde(default)]
    pub allow: Vec<WorkspacePermission>,
    #[serde(default)]
    pub deny: Vec<WorkspacePermission>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// `GET /api/v1/structure`: the structure and its revision. The revision is a text that is
/// equal for equal structures and differs after every change of the structure; a reader
/// compares it and does not read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StructureExport {
    pub revision: String,
    pub structure: StructureFile,
}

/// One change of the structure. Every identifier is the identifier of an entry: of an object
/// that exists, or of an entry that an earlier change of the same request creates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(tag = "change", rename_all = "snake_case")]
pub enum StructureChange {
    ReorderServerRoles {
        positions: Vec<StructureRolePosition>,
    },
    ReorderWorkspaceRoles {
        workspace: StructureId,
        positions: Vec<StructureRolePosition>,
    },
    CreateServerRole {
        id: StructureId,
        name: String,
        #[serde(default)]
        permissions: Vec<ServerPermission>,
    },
    UpdateServerRole {
        id: StructureId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permissions: Option<Vec<ServerPermission>>,
    },
    /// The account of the request becomes the first member of the workspace. `everyone_role`
    /// is the identifier under which later changes and files name the "everyone" role of
    /// the new workspace.
    CreateWorkspace {
        id: StructureId,
        name: String,
        everyone_role: StructureId,
    },
    RenameWorkspace {
        id: StructureId,
        name: String,
    },
    CreateWorkspaceRole {
        id: StructureId,
        workspace: StructureId,
        name: String,
        #[serde(default)]
        permissions: Vec<WorkspacePermission>,
    },
    UpdateWorkspaceRole {
        id: StructureId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permissions: Option<Vec<WorkspacePermission>>,
    },
    CreateCategory {
        id: StructureId,
        workspace: StructureId,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<i32>,
    },
    UpdateCategory {
        id: StructureId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<i32>,
    },
    CreateChannel {
        id: StructureId,
        workspace: StructureId,
        kind: ChannelKind,
        name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        category: Option<StructureId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<i32>,
    },
    UpdateChannel {
        id: StructureId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[cfg_attr(feature = "openapi", schema(value_type = Option<StructureId>))]
        category: Option<Option<StructureId>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        position: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permanent_messages: Option<bool>,
    },
    /// The settings of the call of a voice channel.
    UpdateCallSettings {
        id: StructureId,
        #[serde(
            default,
            deserialize_with = "present",
            skip_serializing_if = "Option::is_none"
        )]
        #[cfg_attr(feature = "openapi", schema(value_type = Option<u32>))]
        user_limit: Option<Option<u32>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        hides_occupants: Option<bool>,
    },
    /// Two empty sets remove the override.
    SetChannelOverride {
        channel: StructureId,
        role: StructureId,
        #[serde(default)]
        allow: Vec<WorkspacePermission>,
        #[serde(default)]
        deny: Vec<WorkspacePermission>,
    },
    DeleteChannel {
        id: StructureId,
    },
    /// The channels of the category stay and have no category.
    DeleteCategory {
        id: StructureId,
    },
    DeleteWorkspaceRole {
        id: StructureId,
    },
    DeleteServerRole {
        id: StructureId,
    },
    /// Deletes the workspace with its members, roles, categories and channels.
    DeleteWorkspace {
        id: StructureId,
    },
}

impl StructureChange {
    pub const fn name(&self) -> &'static str {
        match self {
            Self::ReorderServerRoles { .. } => "reorder_server_roles",
            Self::ReorderWorkspaceRoles { .. } => "reorder_workspace_roles",
            Self::CreateServerRole { .. } => "create_server_role",
            Self::UpdateServerRole { .. } => "update_server_role",
            Self::CreateWorkspace { .. } => "create_workspace",
            Self::RenameWorkspace { .. } => "rename_workspace",
            Self::CreateWorkspaceRole { .. } => "create_workspace_role",
            Self::UpdateWorkspaceRole { .. } => "update_workspace_role",
            Self::CreateCategory { .. } => "create_category",
            Self::UpdateCategory { .. } => "update_category",
            Self::CreateChannel { .. } => "create_channel",
            Self::UpdateChannel { .. } => "update_channel",
            Self::UpdateCallSettings { .. } => "update_call_settings",
            Self::SetChannelOverride { .. } => "set_channel_override",
            Self::DeleteChannel { .. } => "delete_channel",
            Self::DeleteCategory { .. } => "delete_category",
            Self::DeleteWorkspaceRole { .. } => "delete_workspace_role",
            Self::DeleteServerRole { .. } => "delete_server_role",
            Self::DeleteWorkspace { .. } => "delete_workspace",
        }
    }
}

/// `POST /api/v1/structure/changes`: the changes of one plan. The server applies all of them
/// or none, and none when `revision` is not the current revision of the structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ApplyStructureChanges {
    pub revision: String,
    /// At most 200 changes, applied in their order.
    pub changes: Vec<StructureChange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum StructureKind {
    ServerRole,
    Workspace,
    WorkspaceRole,
    Category,
    Channel,
}

/// An object that a request created: the identifier of its entry and the identifier that the
/// server gave the object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreatedEntry {
    pub kind: StructureKind,
    pub entry: StructureId,
    pub id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StructureApplied {
    /// The revision of the structure after the changes.
    pub revision: String,
    pub created: Vec<CreatedEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct StructureRolePosition {
    pub id: StructureId,
    pub position: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_with_an_unknown_field_is_refused() {
        let known = r#"{"format":1,"workspaces":[{"id":"0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f",
            "name":"Main","channels":[]}]}"#;
        let file: StructureFile = serde_json::from_str(known).unwrap();
        assert_eq!(file.workspaces[0].name, "Main");
        assert!(!file.workspaces[0].default);
        let misspelled = known.replace("\"channels\"", "\"chanels\"");
        assert!(serde_json::from_str::<StructureFile>(&misspelled).is_err());
    }

    #[test]
    fn a_channel_entry_tells_an_absent_field_from_a_cleared_one() {
        let entry = |fields: &str| -> ChannelEntry {
            serde_json::from_str(&format!(
                r#"{{"id":"0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f","name":"hall","kind":"voice"{fields}}}"#
            ))
            .unwrap()
        };
        let absent = entry("");
        assert_eq!((absent.category, absent.user_limit), (None, None));
        let cleared = entry(r#","category":null,"user_limit":null"#);
        assert_eq!(
            (cleared.category, cleared.user_limit),
            (Some(None), Some(None))
        );
        let set = entry(r#","user_limit":8"#);
        assert_eq!(set.user_limit, Some(Some(8)));
        let written = serde_json::to_value(&absent).unwrap();
        assert!(written.get("category").is_none());
        assert_eq!(
            serde_json::to_value(&cleared).unwrap()["category"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn a_change_is_tagged_with_its_name() {
        let id: StructureId = "0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f".parse().unwrap();
        let changes = [
            StructureChange::DeleteChannel { id },
            StructureChange::DeleteCategory { id },
            StructureChange::DeleteWorkspaceRole { id },
            StructureChange::DeleteServerRole { id },
            StructureChange::DeleteWorkspace { id },
            StructureChange::RenameWorkspace {
                id,
                name: "Main".to_owned(),
            },
            StructureChange::UpdateChannel {
                id,
                name: None,
                category: Some(None),
                position: None,
                permanent_messages: None,
            },
        ];
        for change in changes {
            let json = serde_json::to_value(&change).unwrap();
            assert_eq!(json["change"], change.name());
            assert_eq!(
                serde_json::from_value::<StructureChange>(json).unwrap(),
                change
            );
        }
    }
}
