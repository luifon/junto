use junto_authz::WorkspacePermission;
use serde::{Deserialize, Deserializer, Serialize};

use crate::ids::{AccountId, CategoryId, ChannelId, WorkspaceId, WorkspaceRoleId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    /// True for the workspace that an invite or an approval grants when it names none.
    pub is_default: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateWorkspace {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateWorkspace {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WorkspaceMember {
    pub account_id: AccountId,
    pub name: String,
    pub role_ids: Vec<WorkspaceRoleId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Category {
    pub id: CategoryId,
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub position: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateCategory {
    pub name: String,
    /// Absent: after the last category.
    #[serde(default)]
    pub position: Option<i32>,
}

/// An absent field keeps its value.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateCategory {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub position: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum ChannelKind {
    Text,
    Voice,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Channel {
    pub id: ChannelId,
    pub workspace_id: WorkspaceId,
    pub category_id: Option<CategoryId>,
    pub kind: ChannelKind,
    pub name: String,
    pub position: i32,
    /// The channel setting that makes each message sent under it permanent.
    pub permanent_messages: bool,
    /// The largest number of people in the call of a voice channel. Absent: no limit. A
    /// person who is invited or moved in can pass it (ADR 0504).
    #[serde(default)]
    pub user_limit: Option<u32>,
    /// True when a voice channel shows who is inside only to the people inside (ADR 0504).
    #[serde(default)]
    pub hides_occupants: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct CreateChannel {
    pub name: String,
    pub kind: ChannelKind,
    #[serde(default)]
    pub category_id: Option<CategoryId>,
    /// Absent: after the last channel.
    #[serde(default)]
    pub position: Option<i32>,
}

/// An absent field keeps its value. `category_id: null` takes the channel out of its category.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct UpdateChannel {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    #[cfg_attr(feature = "openapi", schema(value_type = Option<CategoryId>))]
    pub category_id: Option<Option<CategoryId>>,
    #[serde(default)]
    pub position: Option<i32>,
    #[serde(default)]
    pub permanent_messages: Option<bool>,
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

/// The channels and categories of a workspace that the caller can see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChannelList {
    pub categories: Vec<Category>,
    pub channels: Vec<Channel>,
}

/// The allow set and the deny set of one workspace role in one channel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ChannelOverride {
    pub role_id: WorkspaceRoleId,
    pub allow: Vec<WorkspacePermission>,
    pub deny: Vec<WorkspacePermission>,
}

/// Two empty sets remove the override.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct SetChannelOverride {
    #[serde(default)]
    pub allow: Vec<WorkspacePermission>,
    #[serde(default)]
    pub deny: Vec<WorkspacePermission>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_channel_tells_an_absent_category_from_a_cleared_one() {
        let absent: UpdateChannel = serde_json::from_str("{}").unwrap();
        assert_eq!(absent.category_id, None);
        let cleared: UpdateChannel = serde_json::from_str(r#"{"category_id":null}"#).unwrap();
        assert_eq!(cleared.category_id, Some(None));
        let set: UpdateChannel =
            serde_json::from_str(r#"{"category_id":"0198f1c2-7d3a-7b1e-9c55-0a1b2c3d4e5f"}"#)
                .unwrap();
        assert!(matches!(set.category_id, Some(Some(_))));
    }
}
