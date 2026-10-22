//! The collections of the administration (ADR 0760): what the snapshot does not carry and
//! a person with the permissions reads and changes: accounts, invites, join requests,
//! access tokens, exports, server roles, the overrides of a channel, the archived channels
//! and the settings of a workspace.
//!
//! A collection is read from its route when a reader opens it, follows the events from
//! then on, and is read again after a reload of the state (ADR 0207).

use std::sync::Arc;
use std::time::Instant;

use junto_protocol::WorkspacePermission;
use junto_protocol::access_token::AccessTokenInfo;
use junto_protocol::account::{AccountSummary, ListedAccount};
use junto_protocol::discovery::Retention;
use junto_protocol::events::Event;
use junto_protocol::export::Export;
use junto_protocol::ids::{AccountId, ChannelId, ServerRoleId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::invite::{Invite, JoinRequest, JoinRequestState};
use junto_protocol::message::AttachmentLimits;
use junto_protocol::roles::{ListedServerRole, ListedWorkspaceRole, RoleActions, ServerRole};
use junto_protocol::structure::{ArchivePeriod, ArchivedChannel, Channel, ChannelOverride};

use super::rank::sort_server_roles;
use super::{PlaceId, ServerModel};
use crate::api::{ApiError, ErrorCategory};

/// How many access tokens the client reads at most. An account holds at most 100
/// (ADR 0210), and an administrator sees the tokens of every account.
pub const MOST_TOKENS: usize = 2000;
/// How many terms of the glossary of one workspace the client holds at most. A glossary has
/// at most 200 terms (ADR 0607).
pub const MOST_GLOSSARY_TERMS: usize = 200;

/// Why a collection was not read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LoadFailure {
    /// The account may not read the collection.
    NotAllowed,
    /// The server has no such workspace or channel for the account.
    NotFound,
    /// The server answered that it cannot serve the request now.
    Unavailable,
    /// No answer arrived, or one that the client cannot read.
    NoAnswer,
}

impl LoadFailure {
    pub fn of(error: &ApiError) -> Self {
        match error.category() {
            Some(ErrorCategory::NotAllowed | ErrorCategory::Banned) => Self::NotAllowed,
            Some(ErrorCategory::NotFound) => Self::NotFound,
            Some(_) => Self::Unavailable,
            None => Self::NoAnswer,
        }
    }
}

/// A collection that the snapshot does not carry.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Collection<T> {
    /// No reader has the collection open.
    #[default]
    NotLoaded,
    /// The first read is on its way.
    Loading,
    Loaded(T),
    /// The last read failed. A reader opens the collection again to read it again.
    Failed(LoadFailure),
}

impl<T> Collection<T> {
    pub const fn loaded(&self) -> Option<&T> {
        match self {
            Self::Loaded(value) => Some(value),
            Self::NotLoaded | Self::Loading | Self::Failed(_) => None,
        }
    }

    fn loaded_mut(&mut self) -> Option<&mut T> {
        match self {
            Self::Loaded(value) => Some(value),
            Self::NotLoaded | Self::Loading | Self::Failed(_) => None,
        }
    }

    pub const fn is_not_loaded(&self) -> bool {
        matches!(self, Self::NotLoaded)
    }
}

/// One collection of the administration, as a reader names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AdminPart {
    /// Every account of the server, for a person with "ban members".
    Accounts,
    Invites,
    /// The join requests that wait.
    JoinRequests,
    AccessTokens,
    /// The exports of the account.
    Exports,
    ServerRoles,
    /// The retention of the files of direct messages.
    DirectRetention,
    /// The largest file and the number of files of a message that the server accepts
    /// (ADR 0408).
    AttachmentLimits,
    /// The overrides of the roles in one channel.
    Overrides(ChannelId),
    /// The archived channels of one workspace that the account sees.
    Archived(WorkspaceId),
    /// The archive period and the retention of one workspace.
    WorkspaceSettings(WorkspaceId),
    /// The workspace permissions of the account in one archived channel (ADR 0310). The
    /// snapshot states them for the channels of the channel lists only.
    ChannelPermissions(ChannelId),
    /// What the account may do with each role of one workspace: give it, take it, delete
    /// it (ADR 0311). The roles themselves are in the snapshot.
    RoleActions(WorkspaceId),
    /// The glossary of one workspace (ADR 0603). Every member reads it.
    Glossary(WorkspaceId),
}

/// The glossary of a workspace as the client holds it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GlossaryList {
    /// The most important term first, at most [`MOST_GLOSSARY_TERMS`].
    pub terms: Vec<String>,
    /// The server gave more terms than the client holds. A change made from this list
    /// would remove the others, so a reader makes none.
    pub cut: bool,
}

/// The settings of a workspace that only an administrator reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSettings {
    pub archive_period: ArchivePeriod,
    pub file_retention: Retention,
    pub recording_retention: Retention,
}

/// The answer of the route of one collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdminPayload {
    Accounts(Vec<ListedAccount>),
    Invites(Vec<Invite>),
    JoinRequests(Vec<JoinRequest>),
    AccessTokens {
        tokens: Vec<AccessTokenInfo>,
        /// The server holds more tokens than the client reads.
        more: bool,
    },
    Exports(Vec<Export>),
    ServerRoles(Vec<ListedServerRole>),
    DirectRetention(Retention),
    AttachmentLimits(AttachmentLimits),
    Overrides(Vec<ChannelOverride>),
    Archived(Vec<ArchivedChannel>),
    WorkspaceSettings(WorkspaceSettings),
    ChannelPermissions(Vec<WorkspacePermission>),
    RoleActions(Vec<ListedWorkspaceRole>),
    Glossary(Vec<String>),
}

/// What the account may do with each role of one workspace or of the server, by role.
pub type RoleActionList<R> = Vec<(R, RoleActions)>;

/// What the stream client puts in the place of a collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AdminSet {
    NotLoaded,
    Loading,
    Failed(LoadFailure),
    Loaded(AdminPayload),
}

/// The collections of the administration of one server.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AdminModel {
    /// In the order of their identifiers.
    pub accounts: Collection<Vec<AccountSummary>>,
    /// The banned accounts that the account may unban, as the last read of the accounts
    /// stated them (ADR 0311). They describe the account at that read and are no authority
    /// for a request: the server decides.
    pub unbannable: Vec<AccountId>,
    /// The newest first.
    pub invites: Collection<Vec<Invite>>,
    /// The oldest first.
    pub join_requests: Collection<Vec<JoinRequest>>,
    /// The newest first.
    pub access_tokens: Collection<Vec<AccessTokenInfo>>,
    /// The server holds more tokens than [`MOST_TOKENS`].
    pub access_tokens_more: bool,
    /// The newest first.
    pub exports: Collection<Vec<Export>>,
    /// From the highest position to the lowest (ADR 0309).
    pub server_roles: Collection<Vec<ServerRole>>,
    /// What the account may do with each server role, as the last read of the server roles
    /// stated it (ADR 0311). A role that an event added has none until the next read.
    pub server_role_actions: RoleActionList<ServerRoleId>,
    pub direct_retention: Collection<Retention>,
    pub attachment_limits: Collection<AttachmentLimits>,
    pub overrides: Vec<(ChannelId, Collection<Vec<ChannelOverride>>)>,
    pub archived: Vec<(WorkspaceId, Collection<Vec<ArchivedChannel>>)>,
    pub settings: Vec<(WorkspaceId, Collection<WorkspaceSettings>)>,
    /// The permissions of the account in the archived channels that a reader asked for.
    pub channel_permissions: Vec<(ChannelId, Collection<Vec<WorkspacePermission>>)>,
    /// What the account may do with the roles of the workspaces that a reader asked for.
    pub role_actions: Vec<(WorkspaceId, Collection<RoleActionList<WorkspaceRoleId>>)>,
    /// The glossaries of the workspaces that a reader asked for.
    pub glossaries: Vec<(WorkspaceId, Collection<GlossaryList>)>,
    /// When the newest read of each collection started, and when the read that the
    /// collection shows started. A reader that changed a collection knows from them whether
    /// the collection shows a read that started after its change was answered.
    pub read_times: Vec<(AdminPart, ReadTimes)>,
    /// The collections whose flags an event made stale and whose new read has not
    /// answered: their flags state nothing until then.
    pub flags_stale: Vec<AdminPart>,
    /// The archived channels that the state knows outside the loaded lists: a channel
    /// that was archived while a reader had its messages open, and a channel that a
    /// reload carried for such a reader until the list of its workspace answers.
    pub archived_known: Vec<ArchivedChannel>,
}

/// When the reads of one collection started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadTimes {
    /// The start of the newest read.
    pub started: Option<Instant>,
    /// The start of the read whose answer the collection shows.
    pub shown: Option<Instant>,
}

/// The state of a collection without its content.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionState {
    NotLoaded,
    Loading,
    Loaded,
    Failed(LoadFailure),
}

fn state_of<T>(collection: &Collection<T>) -> CollectionState {
    match collection {
        Collection::NotLoaded => CollectionState::NotLoaded,
        Collection::Loading => CollectionState::Loading,
        Collection::Loaded(_) => CollectionState::Loaded,
        Collection::Failed(failure) => CollectionState::Failed(*failure),
    }
}

fn keyed<K: Copy + PartialEq, T>(list: &[(K, Collection<T>)], key: K) -> &Collection<T> {
    list.iter()
        .find(|(held, _)| *held == key)
        .map_or(&Collection::NotLoaded, |(_, collection)| collection)
}

fn set_keyed<K: Copy + PartialEq, T>(
    list: &mut Vec<(K, Collection<T>)>,
    key: K,
    collection: Collection<T>,
) {
    list.retain(|(held, _)| *held != key);
    if !collection.is_not_loaded() {
        list.push((key, collection));
    }
}

/// Keeps what a collection shows while it is read again.
fn mark_loading<T>(collection: &mut Collection<T>) {
    if collection.loaded().is_none() {
        *collection = Collection::Loading;
    }
}

fn mark_keyed_loading<K: Copy + PartialEq, T>(list: &mut Vec<(K, Collection<T>)>, key: K) {
    match list.iter_mut().find(|(held, _)| *held == key) {
        Some((_, collection)) => mark_loading(collection),
        None => list.push((key, Collection::Loading)),
    }
}

fn upsert<T: Clone>(items: &mut Vec<T>, item: &T, same: impl Fn(&T) -> bool) -> bool {
    match items.iter_mut().find(|held| same(held)) {
        Some(held) => {
            *held = item.clone();
            false
        }
        None => {
            items.push(item.clone());
            true
        }
    }
}

fn sort_archived(channels: &mut [ArchivedChannel]) {
    channels.sort_by_key(|held| (held.archived_at, held.channel.id));
}

impl AdminModel {
    pub fn overrides(&self, channel_id: ChannelId) -> &Collection<Vec<ChannelOverride>> {
        keyed(&self.overrides, channel_id)
    }

    pub fn archived(&self, workspace_id: WorkspaceId) -> &Collection<Vec<ArchivedChannel>> {
        keyed(&self.archived, workspace_id)
    }

    pub fn workspace_settings(&self, workspace_id: WorkspaceId) -> &Collection<WorkspaceSettings> {
        keyed(&self.settings, workspace_id)
    }

    /// The permissions of the account in an archived channel, while a reader has them
    /// open.
    pub fn channel_permissions(
        &self,
        channel_id: ChannelId,
    ) -> &Collection<Vec<WorkspacePermission>> {
        keyed(&self.channel_permissions, channel_id)
    }

    /// What the account may do with the roles of a workspace, while a reader has them
    /// open.
    pub fn role_actions(
        &self,
        workspace_id: WorkspaceId,
    ) -> &Collection<RoleActionList<WorkspaceRoleId>> {
        keyed(&self.role_actions, workspace_id)
    }

    /// When the reads of a collection started.
    pub fn read_times(&self, part: AdminPart) -> ReadTimes {
        self.read_times
            .iter()
            .find(|(held, _)| *held == part)
            .map_or_else(ReadTimes::default, |(_, times)| *times)
    }

    fn note_read(&mut self, part: AdminPart, set: &AdminSet) {
        let index = match self.read_times.iter().position(|(held, _)| *held == part) {
            Some(index) => index,
            None => {
                self.read_times.push((part, ReadTimes::default()));
                self.read_times.len() - 1
            }
        };
        let Some((_, times)) = self.read_times.get_mut(index) else {
            return;
        };
        match set {
            AdminSet::Loading => times.started = Some(Instant::now()),
            AdminSet::Loaded(_) => times.shown = times.started,
            AdminSet::NotLoaded | AdminSet::Failed(_) => {}
        }
    }

    /// The glossary of a workspace, while a reader has it open.
    pub fn glossary(&self, workspace_id: WorkspaceId) -> &Collection<GlossaryList> {
        keyed(&self.glossaries, workspace_id)
    }

    /// What the account may do with one role of a workspace. Absent while the list is not
    /// read, and for a role that the list does not name.
    pub fn workspace_role_actions(
        &self,
        workspace_id: WorkspaceId,
        role_id: WorkspaceRoleId,
    ) -> Option<&RoleActions> {
        if self
            .flags_stale
            .contains(&AdminPart::RoleActions(workspace_id))
        {
            return None;
        }
        self.role_actions(workspace_id)
            .loaded()?
            .iter()
            .find(|(held, _)| *held == role_id)
            .map(|(_, actions)| actions)
    }

    /// What the account may do with one server role. Absent while the server roles are not
    /// read, and for a role that their read did not name.
    pub fn server_role_actions(&self, role_id: ServerRoleId) -> Option<&RoleActions> {
        self.server_roles.loaded()?;
        if self.flags_stale.contains(&AdminPart::ServerRoles) {
            return None;
        }
        self.server_role_actions
            .iter()
            .find(|(held, _)| *held == role_id)
            .map(|(_, actions)| actions)
    }

    /// Whether the last read of the accounts stated that the account may unban this one.
    /// Absent while the accounts are not read.
    pub fn may_unban(&self, account_id: AccountId) -> Option<bool> {
        self.accounts.loaded()?;
        if self.flags_stale.contains(&AdminPart::Accounts) {
            return None;
        }
        Some(self.unbannable.contains(&account_id))
    }

    pub fn state(&self, part: AdminPart) -> CollectionState {
        match part {
            AdminPart::Accounts => state_of(&self.accounts),
            AdminPart::Invites => state_of(&self.invites),
            AdminPart::JoinRequests => state_of(&self.join_requests),
            AdminPart::AccessTokens => state_of(&self.access_tokens),
            AdminPart::Exports => state_of(&self.exports),
            AdminPart::ServerRoles => state_of(&self.server_roles),
            AdminPart::DirectRetention => state_of(&self.direct_retention),
            AdminPart::AttachmentLimits => state_of(&self.attachment_limits),
            AdminPart::Overrides(channel_id) => state_of(self.overrides(channel_id)),
            AdminPart::Archived(workspace_id) => state_of(self.archived(workspace_id)),
            AdminPart::WorkspaceSettings(workspace_id) => {
                state_of(self.workspace_settings(workspace_id))
            }
            AdminPart::ChannelPermissions(channel_id) => {
                state_of(self.channel_permissions(channel_id))
            }
            AdminPart::RoleActions(workspace_id) => state_of(self.role_actions(workspace_id)),
            AdminPart::Glossary(workspace_id) => state_of(self.glossary(workspace_id)),
        }
    }

    /// The archived channels that the state knows in one workspace, or in all when
    /// `scope` is `None`.
    pub(crate) fn archived_in(&self, scope: Option<WorkspaceId>) -> Vec<ChannelId> {
        self.archived
            .iter()
            .filter_map(|(_, list)| list.loaded())
            .flatten()
            .chain(&self.archived_known)
            .filter(|held| scope.is_none_or(|scope| held.channel.workspace_id == scope))
            .map(|held| held.channel.id)
            .collect()
    }

    /// What the account may see changed in one workspace, or on the server when `scope`
    /// is `None` (see [`ServerModel::withdraw`](super::ServerModel::withdraw)): every
    /// loaded collection of that scope shows as being read until the next state reads it
    /// again. `channels` are the channels of the scope.
    pub(crate) fn withdraw(&mut self, scope: Option<WorkspaceId>, channels: &[ChannelId]) {
        fn reading<T>(collection: &mut Collection<T>) {
            if !collection.is_not_loaded() {
                *collection = Collection::Loading;
            }
        }
        fn reading_keyed<K: Copy, T>(list: &mut [(K, Collection<T>)], of: impl Fn(K) -> bool) {
            for (key, collection) in list.iter_mut() {
                if of(*key) {
                    reading(collection);
                }
            }
        }
        if scope.is_none() {
            reading(&mut self.accounts);
            reading(&mut self.invites);
            reading(&mut self.join_requests);
            reading(&mut self.access_tokens);
            self.access_tokens_more = false;
            reading(&mut self.exports);
            reading(&mut self.server_roles);
            reading(&mut self.direct_retention);
        }
        let in_scope = |workspace_id: WorkspaceId| scope.is_none_or(|scope| scope == workspace_id);
        // The archived channels are structure, which stays until the snapshot: the ones
        // of a list that is read again are known by themselves meanwhile, so that a
        // reader of one of them is carried through the snapshot (ADR 0762).
        let listed: Vec<ArchivedChannel> = self
            .archived
            .iter()
            .filter(|(workspace_id, _)| in_scope(*workspace_id))
            .filter_map(|(_, list)| list.loaded())
            .flatten()
            .cloned()
            .collect();
        for channel in listed {
            if !self
                .archived_known
                .iter()
                .any(|held| held.channel.id == channel.channel.id)
            {
                self.archived_known.push(channel);
            }
        }
        reading_keyed(&mut self.archived, in_scope);
        reading_keyed(&mut self.settings, in_scope);
        // What the account may do with the roles of a workspace follows its permissions
        // there (ADR 0764).
        reading_keyed(&mut self.role_actions, in_scope);
        reading_keyed(&mut self.glossaries, in_scope);
        reading_keyed(&mut self.overrides, |channel_id| {
            channels.contains(&channel_id)
        });
        reading_keyed(&mut self.channel_permissions, |channel_id| {
            channels.contains(&channel_id)
        });
    }

    /// An archived channel that the state knows: from a loaded list, or by itself.
    pub fn archived_channel(&self, channel_id: ChannelId) -> Option<&ArchivedChannel> {
        self.archived
            .iter()
            .filter_map(|(_, list)| list.loaded())
            .flatten()
            .chain(&self.archived_known)
            .find(|held| held.channel.id == channel_id)
    }

    fn set(&mut self, part: AdminPart, set: AdminSet) {
        self.note_read(part, &set);
        if set != AdminSet::Loading {
            self.flags_stale.retain(|held| *held != part);
        }
        fn plain<T>(
            set: AdminSet,
            loaded: impl FnOnce(AdminPayload) -> Option<T>,
        ) -> Collection<T> {
            match set {
                AdminSet::NotLoaded => Collection::NotLoaded,
                AdminSet::Loading => Collection::Loading,
                AdminSet::Failed(failure) => Collection::Failed(failure),
                AdminSet::Loaded(payload) => match loaded(payload) {
                    Some(value) => Collection::Loaded(value),
                    None => Collection::Failed(LoadFailure::NoAnswer),
                },
            }
        }
        if set == AdminSet::Loading {
            match part {
                AdminPart::Accounts => mark_loading(&mut self.accounts),
                AdminPart::Invites => mark_loading(&mut self.invites),
                AdminPart::JoinRequests => mark_loading(&mut self.join_requests),
                AdminPart::AccessTokens => mark_loading(&mut self.access_tokens),
                AdminPart::Exports => mark_loading(&mut self.exports),
                AdminPart::ServerRoles => mark_loading(&mut self.server_roles),
                AdminPart::DirectRetention => mark_loading(&mut self.direct_retention),
                AdminPart::AttachmentLimits => mark_loading(&mut self.attachment_limits),
                AdminPart::Overrides(channel_id) => {
                    mark_keyed_loading(&mut self.overrides, channel_id);
                }
                AdminPart::Archived(workspace_id) => {
                    mark_keyed_loading(&mut self.archived, workspace_id);
                }
                AdminPart::WorkspaceSettings(workspace_id) => {
                    mark_keyed_loading(&mut self.settings, workspace_id);
                }
                AdminPart::ChannelPermissions(channel_id) => {
                    mark_keyed_loading(&mut self.channel_permissions, channel_id);
                }
                AdminPart::RoleActions(workspace_id) => {
                    mark_keyed_loading(&mut self.role_actions, workspace_id);
                }
                AdminPart::Glossary(workspace_id) => {
                    mark_keyed_loading(&mut self.glossaries, workspace_id);
                }
            }
            return;
        }
        match part {
            AdminPart::Accounts => {
                let mut unbannable = Vec::new();
                self.accounts = plain(set, |payload| match payload {
                    AdminPayload::Accounts(listed) => {
                        let mut accounts = Vec::with_capacity(listed.len());
                        for held in listed {
                            if held.may_unban {
                                unbannable.push(held.account.id);
                            }
                            accounts.push(held.account);
                        }
                        accounts.sort_by_key(|held| held.id);
                        Some(accounts)
                    }
                    _ => None,
                });
                self.unbannable = unbannable;
            }
            AdminPart::Invites => {
                self.invites = plain(set, |payload| match payload {
                    AdminPayload::Invites(mut invites) => {
                        invites.sort_by_key(|held| std::cmp::Reverse(held.id));
                        Some(invites)
                    }
                    _ => None,
                });
            }
            AdminPart::JoinRequests => {
                self.join_requests = plain(set, |payload| match payload {
                    AdminPayload::JoinRequests(mut requests) => {
                        requests.sort_by_key(|held| (held.created_at, held.id));
                        Some(requests)
                    }
                    _ => None,
                });
            }
            AdminPart::AccessTokens => {
                let mut more = false;
                self.access_tokens = plain(set, |payload| match payload {
                    AdminPayload::AccessTokens {
                        mut tokens,
                        more: goes_on,
                    } => {
                        tokens.sort_by_key(|held| std::cmp::Reverse(held.id));
                        more = goes_on;
                        Some(tokens)
                    }
                    _ => None,
                });
                self.access_tokens_more = more;
            }
            AdminPart::Exports => {
                self.exports = plain(set, |payload| match payload {
                    AdminPayload::Exports(mut exports) => {
                        exports.sort_by_key(|held| std::cmp::Reverse(held.id));
                        Some(exports)
                    }
                    _ => None,
                });
            }
            AdminPart::ServerRoles => {
                let mut actions = Vec::new();
                self.server_roles = plain(set, |payload| match payload {
                    AdminPayload::ServerRoles(listed) => {
                        let mut roles = Vec::with_capacity(listed.len());
                        for held in listed {
                            actions.push((held.role.id, held.actions));
                            roles.push(held.role);
                        }
                        sort_server_roles(&mut roles);
                        Some(roles)
                    }
                    _ => None,
                });
                self.server_role_actions = actions;
            }
            AdminPart::DirectRetention => {
                self.direct_retention = plain(set, |payload| match payload {
                    AdminPayload::DirectRetention(retention) => Some(retention),
                    _ => None,
                });
            }
            AdminPart::AttachmentLimits => {
                self.attachment_limits = plain(set, |payload| match payload {
                    AdminPayload::AttachmentLimits(limits) => Some(limits),
                    _ => None,
                });
            }
            AdminPart::Overrides(channel_id) => {
                let collection = plain(set, |payload| match payload {
                    AdminPayload::Overrides(mut overrides) => {
                        overrides.sort_by_key(|held| held.role_id);
                        Some(overrides)
                    }
                    _ => None,
                });
                set_keyed(&mut self.overrides, channel_id, collection);
            }
            AdminPart::Archived(workspace_id) => {
                let collection = plain(set, |payload| match payload {
                    AdminPayload::Archived(mut channels) => {
                        channels.retain(|held| held.channel.workspace_id == workspace_id);
                        sort_archived(&mut channels);
                        Some(channels)
                    }
                    _ => None,
                });
                if collection.loaded().is_some() {
                    // The list of the server replaces what the state knew by itself.
                    self.archived_known
                        .retain(|held| held.channel.workspace_id != workspace_id);
                }
                set_keyed(&mut self.archived, workspace_id, collection);
            }
            AdminPart::WorkspaceSettings(workspace_id) => {
                let collection = plain(set, |payload| match payload {
                    AdminPayload::WorkspaceSettings(settings) => Some(settings),
                    _ => None,
                });
                set_keyed(&mut self.settings, workspace_id, collection);
            }
            AdminPart::RoleActions(workspace_id) => {
                let collection = plain(set, |payload| match payload {
                    AdminPayload::RoleActions(listed) => Some(
                        listed
                            .into_iter()
                            .filter(|held| held.role.workspace_id == workspace_id)
                            .map(|held| (held.role.id, held.actions))
                            .collect(),
                    ),
                    _ => None,
                });
                set_keyed(&mut self.role_actions, workspace_id, collection);
            }
            AdminPart::ChannelPermissions(channel_id) => {
                let collection = plain(set, |payload| match payload {
                    AdminPayload::ChannelPermissions(permissions) => Some(permissions),
                    _ => None,
                });
                set_keyed(&mut self.channel_permissions, channel_id, collection);
            }
            AdminPart::Glossary(workspace_id) => {
                let collection = plain(set, |payload| match payload {
                    AdminPayload::Glossary(mut terms) => {
                        let cut = terms.len() > MOST_GLOSSARY_TERMS;
                        terms.truncate(MOST_GLOSSARY_TERMS);
                        Some(GlossaryList { terms, cut })
                    }
                    _ => None,
                });
                set_keyed(&mut self.glossaries, workspace_id, collection);
            }
        }
    }

    /// Puts an archived channel where the state keeps it: into the loaded list of its
    /// workspace, or beside the lists when `keep` says that a reader needs it.
    fn put_archived(&mut self, archived: &ArchivedChannel, keep: bool) {
        let workspace_id = archived.channel.workspace_id;
        let same = |held: &ArchivedChannel| held.channel.id == archived.channel.id;
        let list = self
            .archived
            .iter_mut()
            .find(|(held, _)| *held == workspace_id)
            .and_then(|(_, list)| list.loaded_mut());
        match list {
            Some(list) => {
                upsert(list, archived, same);
                sort_archived(list);
                self.archived_known.retain(|held| !same(held));
            }
            None if keep || self.archived_known.iter().any(same) => {
                upsert(&mut self.archived_known, archived, same);
            }
            None => {}
        }
    }

    fn forget_archived(&mut self, channel_id: ChannelId) {
        for (_, list) in &mut self.archived {
            if let Some(list) = list.loaded_mut() {
                list.retain(|held| held.channel.id != channel_id);
            }
        }
        self.archived_known
            .retain(|held| held.channel.id != channel_id);
    }

    fn forget_role(&mut self, role_id: WorkspaceRoleId) {
        for (_, list) in &mut self.overrides {
            if let Some(list) = list.loaded_mut() {
                list.retain(|held| held.role_id != role_id);
            }
        }
    }

    fn forget_workspace(&mut self, workspace_id: WorkspaceId, channels: &[ChannelId]) {
        self.archived.retain(|(held, _)| *held != workspace_id);
        self.role_actions.retain(|(held, _)| *held != workspace_id);
        self.glossaries.retain(|(held, _)| *held != workspace_id);
        self.settings.retain(|(held, _)| *held != workspace_id);
        self.overrides.retain(|(held, _)| !channels.contains(held));
        self.channel_permissions
            .retain(|(held, _)| !channels.contains(held));
        self.archived_known
            .retain(|held| held.channel.workspace_id != workspace_id);
    }

    /// Applies an event to the collections that are loaded.
    fn apply(&mut self, event: &Event) {
        match event {
            Event::AttachmentLimitsChanged { limits } => {
                if let Some(held) = self.attachment_limits.loaded_mut() {
                    held.clone_from(limits);
                }
            }
            Event::AccountAdded { account } => {
                if let Some(accounts) = self.accounts.loaded_mut() {
                    upsert(accounts, account, |held| held.id == account.id);
                    accounts.sort_by_key(|held| held.id);
                }
            }
            Event::AccountProfileUpdated { profile } => {
                if let Some(accounts) = self.accounts.loaded_mut() {
                    for held in accounts.iter_mut().filter(|held| held.id == profile.id) {
                        held.name.clone_from(&profile.name);
                    }
                }
            }
            Event::AccountStatusChanged { account_id, status } => {
                if let Some(accounts) = self.accounts.loaded_mut() {
                    for held in accounts.iter_mut().filter(|held| held.id == *account_id) {
                        held.status = *status;
                    }
                }
            }
            Event::AccountServerRolesChanged {
                account_id,
                server_role_ids,
            } => {
                if let Some(accounts) = self.accounts.loaded_mut() {
                    for held in accounts.iter_mut().filter(|held| held.id == *account_id) {
                        held.server_role_ids.clone_from(server_role_ids);
                    }
                }
            }
            Event::OwnerChanged { account_id, .. } => {
                if let Some(accounts) = self.accounts.loaded_mut() {
                    for held in accounts {
                        held.is_owner = held.id == *account_id;
                    }
                }
            }
            Event::ServerRoleCreated { role } | Event::ServerRoleUpdated { role } => {
                if let Some(roles) = self.server_roles.loaded_mut() {
                    upsert(roles, role, |held| held.id == role.id);
                    sort_server_roles(roles);
                }
            }
            Event::ServerRolePositionsShifted { positions } => {
                if let Some(roles) = self.server_roles.loaded_mut() {
                    for held in roles.iter_mut() {
                        if let Some(moved) = positions.iter().find(|moved| moved.role_id == held.id)
                        {
                            held.position = moved.position;
                        }
                    }
                    sort_server_roles(roles);
                }
            }
            Event::ServerRoleDeleted { role_id } => {
                if let Some(roles) = self.server_roles.loaded_mut() {
                    roles.retain(|held| held.id != *role_id);
                }
                if let Some(accounts) = self.accounts.loaded_mut() {
                    for held in accounts {
                        held.server_role_ids.retain(|held| held != role_id);
                    }
                }
            }
            Event::AccessTokenCreated { access_token } => {
                if let Some(tokens) = self.access_tokens.loaded_mut() {
                    upsert(tokens, access_token, |held| held.id == access_token.id);
                    tokens.sort_by_key(|held| std::cmp::Reverse(held.id));
                    if tokens.len() > MOST_TOKENS {
                        tokens.truncate(MOST_TOKENS);
                        self.access_tokens_more = true;
                    }
                }
            }
            Event::AccessTokenRevoked { access_token_id } => {
                if let Some(tokens) = self.access_tokens.loaded_mut() {
                    tokens.retain(|held| held.id != *access_token_id);
                }
            }
            Event::AccessTokensOfCreatorRevoked { created_by } => {
                if let Some(tokens) = self.access_tokens.loaded_mut() {
                    tokens.retain(|held| held.created_by != *created_by);
                }
            }
            // A changed invite is the invite as it is now: it lost a workspace with the
            // removal of its creator from that workspace (ADR 0111).
            Event::InviteCreated { invite } | Event::InviteChanged { invite } => {
                if let Some(invites) = self.invites.loaded_mut() {
                    upsert(invites, invite, |held| held.id == invite.id);
                    invites.sort_by_key(|held| std::cmp::Reverse(held.id));
                }
            }
            Event::InviteRevoked {
                invite_id,
                revocation,
            } => {
                if let Some(invites) = self.invites.loaded_mut() {
                    for held in invites.iter_mut().filter(|held| held.id == *invite_id) {
                        held.revocation = Some(*revocation);
                    }
                }
            }
            Event::InviteUsed { invite_id, uses } => {
                if let Some(invites) = self.invites.loaded_mut() {
                    for held in invites.iter_mut().filter(|held| held.id == *invite_id) {
                        // A replayed event of an earlier use does not lower the count.
                        held.uses = held.uses.max(*uses);
                    }
                }
            }
            Event::InvitesOfCreatorRevoked {
                created_by,
                revocation,
            } => {
                if let Some(invites) = self.invites.loaded_mut() {
                    for held in invites
                        .iter_mut()
                        .filter(|held| held.created_by == *created_by && held.revocation.is_none())
                    {
                        held.revocation = Some(*revocation);
                    }
                }
            }
            Event::JoinRequestCreated { join_request } => {
                if let Some(requests) = self.join_requests.loaded_mut() {
                    upsert(requests, join_request, |held| held.id == join_request.id);
                    requests.sort_by_key(|held| (held.created_at, held.id));
                }
            }
            Event::JoinRequestDecided {
                join_request_id,
                state,
                ..
            } => {
                if *state != JoinRequestState::Pending
                    && let Some(requests) = self.join_requests.loaded_mut()
                {
                    requests.retain(|held| held.id != *join_request_id);
                }
            }
            Event::ExportUpdated { export } => {
                if let Some(exports) = self.exports.loaded_mut() {
                    upsert(exports, export, |held| held.id == export.id);
                    exports.sort_by_key(|held| std::cmp::Reverse(held.id));
                }
            }
            Event::ExportRemoved { export_id, .. } => {
                if let Some(exports) = self.exports.loaded_mut() {
                    exports.retain(|held| held.id != *export_id);
                }
            }
            Event::ChannelOverrideSet {
                channel_id,
                role_id,
                allow,
                deny,
                ..
            } => {
                let list = self
                    .overrides
                    .iter_mut()
                    .find(|(held, _)| held == channel_id)
                    .and_then(|(_, list)| list.loaded_mut());
                if let Some(list) = list {
                    list.retain(|held| held.role_id != *role_id);
                    if !allow.is_empty() || !deny.is_empty() {
                        list.push(ChannelOverride {
                            role_id: *role_id,
                            allow: allow.clone(),
                            deny: deny.clone(),
                        });
                        list.sort_by_key(|held| held.role_id);
                    }
                }
            }
            Event::WorkspaceRoleDeleted { role_id, .. } => self.forget_role(*role_id),
            _ => {}
        }
    }
}

impl AdminPart {
    /// True when `event` changes this collection. The stream client keeps such events
    /// while the collection is read and applies them after the answer (ADR 0207).
    pub(crate) fn concerns(&self, event: &Event) -> bool {
        match self {
            Self::Accounts => matches!(
                event,
                Event::AccountAdded { .. }
                    | Event::AccountProfileUpdated { .. }
                    | Event::AccountStatusChanged { .. }
                    | Event::AccountServerRolesChanged { .. }
                    | Event::OwnerChanged { .. }
                    | Event::ServerRoleDeleted { .. }
            ),
            Self::Invites => matches!(
                event,
                Event::InviteCreated { .. }
                    | Event::InviteChanged { .. }
                    | Event::InviteRevoked { .. }
                    | Event::InviteUsed { .. }
                    | Event::InvitesOfCreatorRevoked { .. }
            ),
            Self::JoinRequests => matches!(
                event,
                Event::JoinRequestCreated { .. } | Event::JoinRequestDecided { .. }
            ),
            Self::AccessTokens => matches!(
                event,
                Event::AccessTokenCreated { .. }
                    | Event::AccessTokenRevoked { .. }
                    | Event::AccessTokensOfCreatorRevoked { .. }
            ),
            Self::Exports => matches!(
                event,
                Event::ExportUpdated { .. } | Event::ExportRemoved { .. }
            ),
            Self::AttachmentLimits => matches!(event, Event::AttachmentLimitsChanged { .. }),
            Self::ServerRoles => matches!(
                event,
                Event::ServerRoleCreated { .. }
                    | Event::ServerRoleUpdated { .. }
                    | Event::ServerRoleDeleted { .. }
                    | Event::ServerRolePositionsShifted { .. }
            ),
            Self::Overrides(channel_id) => match event {
                Event::ChannelOverrideSet { channel_id: of, .. } => of == channel_id,
                Event::WorkspaceRoleDeleted { .. } => true,
                _ => false,
            },
            Self::Archived(workspace_id) => match event {
                Event::ChannelArchived { channel } | Event::ChannelDeletionDue { channel } => {
                    channel.channel.workspace_id == *workspace_id
                }
                Event::ChannelRestored { channel } => channel.workspace_id == *workspace_id,
                Event::ChannelDeleted {
                    workspace_id: of, ..
                } => of == workspace_id,
                _ => false,
            },
            Self::WorkspaceSettings(_)
            | Self::DirectRetention
            | Self::RoleActions(_)
            | Self::Glossary(_) => self.is_stale_after(event),
            // No event carries the list: a change of it is `permissions_changed`, which
            // replaces the state, and every open collection is read again then.
            Self::ChannelPermissions(_) => false,
        }
    }

    /// True when `event` tells that this collection changed without carrying the change:
    /// the collection is read again.
    ///
    /// The authority of the account over accounts and roles (ADR 0311) follows role order,
    /// the roles of the accounts, and for the roles of a workspace the overrides of its
    /// channels: an event that changes one of them makes the flags of the server stale.
    /// `permissions_changed` replaces the state, and every collection is read again then.
    pub(crate) fn is_stale_after(&self, event: &Event) -> bool {
        let server_order = matches!(
            event,
            Event::ServerRoleCreated { .. }
                | Event::ServerRoleUpdated { .. }
                | Event::ServerRoleDeleted { .. }
                | Event::ServerRolePositionsShifted { .. }
                | Event::AccountServerRolesChanged { .. }
                | Event::OwnerChanged { .. }
        );
        match (self, event) {
            (
                Self::WorkspaceSettings(workspace_id),
                Event::RetentionChanged { workspace_id: of },
            ) => *of == Some(*workspace_id),
            (Self::DirectRetention, Event::RetentionChanged { workspace_id: None }) => true,
            // The event names the workspace and carries no term (ADR 0607).
            (Self::Glossary(workspace_id), Event::GlossaryReplaced { workspace_id: of }) => {
                of == workspace_id
            }
            (Self::Accounts, Event::AccountStatusChanged { .. }) => true,
            (Self::Accounts | Self::ServerRoles, _) => server_order,
            (Self::RoleActions(workspace_id), event) => {
                server_order
                    || match event {
                        Event::WorkspaceRoleCreated { role }
                        | Event::WorkspaceRoleUpdated { role } => {
                            role.workspace_id == *workspace_id
                        }
                        Event::WorkspaceRoleDeleted {
                            workspace_id: of, ..
                        }
                        | Event::WorkspaceRolePositionsShifted {
                            workspace_id: of, ..
                        }
                        | Event::WorkspaceMemberRolesChanged {
                            workspace_id: of, ..
                        }
                        | Event::ChannelOverrideSet {
                            workspace_id: of, ..
                        }
                        | Event::ChannelDeleted {
                            workspace_id: of, ..
                        } => of == workspace_id,
                        _ => false,
                    }
            }
            _ => false,
        }
    }

    /// The workspace that the state must hold for this collection.
    pub(crate) const fn workspace(&self) -> Option<WorkspaceId> {
        match self {
            Self::Archived(workspace_id)
            | Self::WorkspaceSettings(workspace_id)
            | Self::RoleActions(workspace_id)
            | Self::Glossary(workspace_id) => Some(*workspace_id),
            _ => None,
        }
    }
}

impl ServerModel {
    fn admin_mut(&mut self) -> &mut AdminModel {
        Arc::make_mut(&mut self.admin)
    }

    pub(crate) fn set_admin(&mut self, part: AdminPart, set: AdminSet) {
        if let AdminPart::Archived(workspace_id) = part
            && matches!(set, AdminSet::NotLoaded | AdminSet::Failed(_))
        {
            // The list goes; a reader of the messages of one of its channels keeps the
            // channel.
            let kept: Vec<ArchivedChannel> = self
                .admin
                .archived(workspace_id)
                .loaded()
                .into_iter()
                .flatten()
                .filter(|held| self.reads_channel(held.channel.id))
                .cloned()
                .collect();
            if !kept.is_empty() {
                self.admin_mut().archived_known.extend(kept);
            }
        }
        self.admin_mut().set(part, set);
    }

    /// Applies again, in their order, the events that arrived while a collection was
    /// read. Only the collections follow them: the rest of the state has followed each
    /// event when it arrived, and a later event of the same object must stay applied.
    pub(crate) fn replay_admin<'a>(
        &mut self,
        part: AdminPart,
        events: impl IntoIterator<Item = &'a Event>,
    ) {
        for event in events {
            if part.concerns(event) {
                self.apply_admin(event);
            }
        }
    }

    /// A channel that the account reads: one of the channel lists, or an archived channel
    /// that the state knows (ADR 0410).
    pub fn readable_channel(&self, channel_id: ChannelId) -> Option<&Channel> {
        self.channel(channel_id)
            .map(|held| &held.channel)
            .or_else(|| {
                self.admin
                    .archived_channel(channel_id)
                    .map(|held| &held.channel)
            })
    }

    /// True while a reader has the messages, a thread or the pin list of the channel
    /// open.
    fn reads_channel(&self, channel_id: ChannelId) -> bool {
        self.timelines
            .iter()
            .any(|held| held.place.container() == PlaceId::Channel(channel_id))
            || self.pins.iter().any(|held| held.channel_id == channel_id)
    }

    /// After the archived list of a workspace arrived and the events of the time of its
    /// read were applied: the messages and the pins of a channel that is in no list any
    /// more are gone. A reader holds a channel that the account reads; a channel that a
    /// reload carried and that the list does not hold was deleted.
    pub(crate) fn close_lost_archived(&mut self) {
        let lost: Vec<PlaceId> = self
            .timelines
            .iter()
            .map(|held| held.place.container())
            .chain(
                self.pins
                    .iter()
                    .map(|held| PlaceId::Channel(held.channel_id)),
            )
            .filter(|place| match place {
                PlaceId::Channel(channel_id) => self.readable_channel(*channel_id).is_none(),
                _ => false,
            })
            .collect();
        for place in lost {
            self.forget_place(place);
        }
    }

    /// The workspaces of the archived channels that the state carried over a reload and
    /// that no loaded archived list holds.
    pub(crate) fn carried_archived_workspaces(&self) -> Vec<WorkspaceId> {
        let mut workspaces: Vec<WorkspaceId> = self
            .admin
            .archived_known
            .iter()
            .map(|held| held.channel.workspace_id)
            .collect();
        workspaces.sort_unstable();
        workspaces.dedup();
        workspaces
    }

    /// Lets go of the archived list of a workspace that the client read without a reader.
    /// A channel of the list stays while a reader has its messages, a thread of it or its
    /// pin list open, or a collection of it (`held`).
    pub(crate) fn release_archived(&mut self, workspace_id: WorkspaceId, held: &[ChannelId]) {
        let kept: Vec<ArchivedChannel> = self
            .admin
            .archived(workspace_id)
            .loaded()
            .into_iter()
            .flatten()
            .filter(|channel| {
                self.reads_channel(channel.channel.id) || held.contains(&channel.channel.id)
            })
            .cloned()
            .collect();
        let admin = self.admin_mut();
        admin.set(AdminPart::Archived(workspace_id), AdminSet::NotLoaded);
        admin.archived_known.extend(kept);
    }

    /// Carries, after a reload, the archived channels whose messages a reader has open.
    /// The snapshot does not name them (ADR 0410); the archived list of their workspace
    /// decides when it answers.
    pub(crate) fn carry_archived(&mut self, before: &Self, wanted: &[PlaceId]) {
        let mut carried = Vec::new();
        for place in wanted {
            let PlaceId::Channel(channel_id) = place.container() else {
                continue;
            };
            if self.channel(channel_id).is_some()
                || carried
                    .iter()
                    .any(|held: &ArchivedChannel| held.channel.id == channel_id)
            {
                continue;
            }
            if let Some(archived) = before.admin.archived_channel(channel_id)
                && self.workspace(archived.channel.workspace_id).is_some()
            {
                carried.push(archived.clone());
            }
        }
        if !carried.is_empty() {
            self.admin_mut().archived_known = carried;
        }
    }

    /// The part of an event that changes the collections of the administration, and the
    /// collections that the event makes stale.
    pub(crate) fn apply_admin(&mut self, event: &Event) -> Vec<AdminPart> {
        let mut stale = Vec::new();
        match event {
            Event::ChannelArchived { channel } => {
                let keep = self.reads_channel(channel.channel.id);
                self.admin_mut().put_archived(channel, keep);
            }
            Event::ChannelDeletionDue { channel } => {
                // The notice replaces a channel that the state knows as archived.
                self.admin_mut().put_archived(channel, false);
            }
            Event::ChannelRestored { channel } => {
                if self.admin.archived_channel(channel.id).is_some() {
                    self.admin_mut().forget_archived(channel.id);
                }
            }
            Event::ChannelDeleted { channel_id, .. } => {
                if self.admin.archived_channel(*channel_id).is_some() {
                    self.admin_mut().forget_archived(*channel_id);
                }
                if self.admin.overrides.iter().any(|(of, _)| of == channel_id) {
                    self.admin_mut()
                        .overrides
                        .retain(|(of, _)| of != channel_id);
                }
                if self
                    .admin
                    .channel_permissions
                    .iter()
                    .any(|(of, _)| of == channel_id)
                {
                    self.admin_mut()
                        .channel_permissions
                        .retain(|(of, _)| of != channel_id);
                }
            }
            Event::RetentionChanged { .. } => {}
            Event::ServerRoleDeleted { role_id } => {
                self.account.server_role_ids.retain(|held| held != role_id);
                self.admin_mut().apply(event);
            }
            _ => {
                let touched = [
                    AdminPart::Accounts,
                    AdminPart::Invites,
                    AdminPart::JoinRequests,
                    AdminPart::AccessTokens,
                    AdminPart::Exports,
                    AdminPart::ServerRoles,
                    AdminPart::AttachmentLimits,
                ]
                .into_iter()
                .any(|part| {
                    part.concerns(event) && self.admin.state(part) == CollectionState::Loaded
                }) || (matches!(
                    event,
                    Event::ChannelOverrideSet { .. } | Event::WorkspaceRoleDeleted { .. }
                ) && !self.admin.overrides.is_empty());
                if touched {
                    self.admin_mut().apply(event);
                }
            }
        }
        // The collections that the event names without carrying the change are read
        // again.
        let parts: Vec<AdminPart> = [
            AdminPart::Accounts,
            AdminPart::ServerRoles,
            AdminPart::DirectRetention,
        ]
        .into_iter()
        .chain(
            self.admin
                .settings
                .iter()
                .map(|(of, _)| AdminPart::WorkspaceSettings(*of)),
        )
        .chain(
            self.admin
                .role_actions
                .iter()
                .map(|(of, _)| AdminPart::RoleActions(*of)),
        )
        .chain(
            self.admin
                .glossaries
                .iter()
                .map(|(of, _)| AdminPart::Glossary(*of)),
        )
        .collect();
        for part in parts {
            if part.is_stale_after(event)
                && self.admin.state(part) != CollectionState::NotLoaded
                && !stale.contains(&part)
            {
                stale.push(part);
                let flagged = matches!(
                    part,
                    AdminPart::Accounts | AdminPart::ServerRoles | AdminPart::RoleActions(_)
                );
                if flagged && !self.admin.flags_stale.contains(&part) {
                    self.admin_mut().flags_stale.push(part);
                }
            }
        }
        stale
    }

    /// A workspace is gone: nothing of it stays in the collections.
    pub(crate) fn forget_admin_workspace(
        &mut self,
        workspace_id: WorkspaceId,
        channels: &[ChannelId],
    ) {
        self.admin_mut().forget_workspace(workspace_id, channels);
    }
}
