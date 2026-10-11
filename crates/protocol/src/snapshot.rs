use serde::{Deserialize, Serialize};

use crate::account::OwnAccount;
use crate::call::{Call, CallInvitation, ChannelCall, DirectMute, OwnCall};
use crate::events::Cursor;
use crate::ids::WorkspaceRoleId;
use crate::roles::WorkspaceRole;
use crate::structure::{Category, Channel, Workspace};

/// The state that a client starts from or returns to. It contains the effect of every event
/// up to `cursor` and of no later one, so a subscription with `cursor` continues it without a
/// gap (ADR 0207). It carries the account, its workspaces, and their roles, categories and
/// channels, and the calls in progress that the caller may see. The other collections are
/// loaded on demand from their routes: members, accounts,
/// devices, invites, join requests, server roles and channel overrides. A client that loads
/// one of them applies to it the events that arrived since it asked for it, in their order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Snapshot {
    pub cursor: Cursor,
    pub account: OwnAccount,
    /// The workspaces that the caller can see.
    pub workspaces: Vec<WorkspaceState>,
    /// The call that the account is in.
    pub own_call: Option<OwnCall>,
    /// The direct-message calls of the account that are in progress.
    pub direct_calls: Vec<Call>,
    /// The invitations into calls that wait for an answer of the account.
    pub call_invitations: Vec<CallInvitation>,
    /// The direct-message conversations that the account has muted.
    #[serde(default)]
    pub direct_mutes: Vec<DirectMute>,
    #[serde(default)]
    pub messages: Vec<crate::message::MessageState>,
    /// The newest 200 readable threads with unread replies or mentions.
    #[serde(default)]
    pub threads: Vec<crate::message::ThreadState>,
    /// Whether additional unread threads can be requested.
    #[serde(default)]
    pub threads_more: bool,
    /// Continuation for GET /api/v1/threads/unread.
    #[serde(default)]
    pub threads_next: Option<String>,
    #[serde(default)]
    pub notifications: Vec<crate::discovery::Notification>,
    #[serde(default)]
    pub notification_settings: Vec<crate::discovery::ScopedNotificationSettings>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct WorkspaceState {
    pub workspace: Workspace,
    /// The roles of the workspace that the caller holds, without the "everyone" role.
    pub own_role_ids: Vec<WorkspaceRoleId>,
    pub roles: Vec<WorkspaceRole>,
    pub categories: Vec<Category>,
    /// The channels that the caller can see.
    pub channels: Vec<Channel>,
    /// The voice channels, of the ones the caller can see, that have a call.
    pub calls: Vec<ChannelCall>,
}
