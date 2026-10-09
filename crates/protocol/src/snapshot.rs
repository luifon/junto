use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::account::OwnAccount;
use crate::events::Cursor;
use crate::ids::WorkspaceRoleId;
use crate::roles::WorkspaceRole;
use crate::structure::{Category, Channel, Workspace};

/// The state that a client starts from or returns to. It contains the effect of every event
/// up to `cursor` and of no later one, so a subscription with `cursor` continues it without a
/// gap (ADR 0207). It carries the account, its workspaces, and their roles, categories and
/// channels. The other collections are loaded on demand from their routes: members, accounts,
/// devices, invites, join requests, server roles and channel overrides. A client that loads
/// one of them applies to it the events that arrived since it asked for it, in their order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct Snapshot {
    pub cursor: Cursor,
    pub account: OwnAccount,
    /// The workspaces that the caller can see.
    pub workspaces: Vec<WorkspaceState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct WorkspaceState {
    pub workspace: Workspace,
    /// The roles of the workspace that the caller holds, without the "everyone" role.
    pub own_role_ids: Vec<WorkspaceRoleId>,
    pub roles: Vec<WorkspaceRole>,
    pub categories: Vec<Category>,
    /// The channels that the caller can see.
    pub channels: Vec<Channel>,
}
