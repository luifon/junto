//! The pages of the records of a server (ADR 0761): the access tokens (ADR 0210), the
//! exports of the person (ADR 0411) and the audit log (ADR 0303, ADR 0305).

use std::path::PathBuf;

use junto_client_core::model::{ServerModel, WorkspaceModel};
use junto_client_core::protocol::access_token::{AccessTokenInfo, CreateAccessToken, TokenAction};
use junto_client_core::protocol::audit::{AuditEntry, AuditEvent};
use junto_client_core::protocol::export::{Export, ExportFailure, ExportState};
use junto_client_core::protocol::ids::{
    AccessTokenId, AccountId, AuditEntryId, CategoryId, ChannelId, ExportId, ServerId,
    ServerRoleId, WorkspaceId, WorkspaceRoleId,
};
use junto_client_core::protocol::message::MessagePlace;
use junto_client_core::protocol::secret::AccessToken;
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};
use junto_client_core::transfers::local_name;
use time::{Date, Duration, OffsetDateTime};

use super::{Listed, holds_server, holds_workspace};
use crate::state::admin::{AdminState, AuditFilter, AuditScope, Made};
use crate::state::view::Known;
use crate::text;

/// The lifetimes of a new access token that the form offers, in days.
pub const TOKEN_EXPIRY_DAYS: [u32; 4] = [7, 30, 90, 366];
/// The lifetime that the form of a new access token shows first.
pub const DEFAULT_TOKEN_EXPIRY_DAYS: u32 = 30;
/// The longest lifetime of an access token (ADR 0210).
pub const LONGEST_TOKEN_EXPIRY_DAYS: u32 = 366;
/// The longest name of an access token, in characters.
pub const LONGEST_TOKEN_NAME: usize = 64;
/// The number of entries that one read of the audit log asks for.
pub const AUDIT_PAGE: u32 = 50;
/// The most entries of the audit log that the page keeps.
pub const MOST_AUDIT_ENTRIES: usize = 2000;

/// The server refuses an expiry that is more than the longest lifetime after its own
/// clock. The longest lifetime is shortened by this time, so that it stays valid when the
/// clock of this device is ahead of the clock of the server.
const CLOCK_MARGIN: Duration = Duration::minutes(10);

/// One access token of the list. The list never holds the token itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenLine {
    pub id: AccessTokenId,
    pub name: String,
    pub created_by: AccountId,
    /// The name of the account that created the token. Absent: the state does not hold
    /// it.
    pub creator: Option<String>,
    /// The person created the token.
    pub own: bool,
    /// In the order in which the server states them. An action of a newer server is
    /// [`TokenAction::Unknown`].
    pub actions: Vec<TokenAction>,
    /// In the time zone of the person.
    pub created: OffsetDateTime,
    pub expires: OffsetDateTime,
    /// The day of the expiry is before today. The server lists a token until it expires
    /// and tells no event at its expiry.
    pub expired: bool,
}

/// The secret of a new access token. It is shown until the person puts it away, and no
/// answer of the server carries it again (ADR 0210).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewToken {
    pub access_token_id: AccessTokenId,
    pub token: AccessToken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokensPage {
    /// The newest first: every token for an administrator, and the own tokens for every
    /// other person. The person may revoke each of them.
    pub tokens: Listed<Vec<TokenLine>>,
    /// The server holds more tokens than the client reads.
    pub more: bool,
    pub made: Option<NewToken>,
    /// The person is an administrator and may create a token. The list then holds the
    /// tokens of every account. Every other person sees and revokes the own tokens, such
    /// as the ones from a time with the administrator permission (ADR 0210).
    pub may_create: bool,
}

/// Why the form of a new access token cannot be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenFault {
    NameEmpty,
    NameTooLong,
    NoAction,
}

/// The expiry that a new access token with a lifetime of `days` asks for at the moment
/// `now`.
pub fn token_expiry(days: u32, now: OffsetDateTime) -> OffsetDateTime {
    let days = days.clamp(1, LONGEST_TOKEN_EXPIRY_DAYS);
    let ahead = Duration::days(i64::from(days));
    if days == LONGEST_TOKEN_EXPIRY_DAYS {
        now + ahead - CLOCK_MARGIN
    } else {
        now + ahead
    }
}

/// The request for a new access token from what the person typed and chose at the moment
/// `now`. The actions are sent in the order of the protocol, each once.
pub fn new_token(
    name: &str,
    actions: &[TokenAction],
    days: u32,
    now: OffsetDateTime,
) -> Result<CreateAccessToken, TokenFault> {
    let name = name.trim();
    if name.is_empty() {
        return Err(TokenFault::NameEmpty);
    }
    if name.chars().count() > LONGEST_TOKEN_NAME {
        return Err(TokenFault::NameTooLong);
    }
    let actions: Vec<TokenAction> = TokenAction::ALL
        .iter()
        .copied()
        .filter(|action| actions.contains(action))
        .collect();
    if actions.is_empty() {
        return Err(TokenFault::NoAction);
    }
    Ok(CreateAccessToken {
        name: name.to_owned(),
        actions,
        expires_at: token_expiry(days, now),
    })
}

/// What an export holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportOf {
    /// A channel that the state holds: one of the channel list, or an archived one.
    Channel {
        name: String,
        kind: ChannelKind,
        /// The name of its workspace.
        workspace: Option<String>,
    },
    /// A channel that the account does not see any more, or an archived channel whose
    /// list is not read.
    ChannelGone,
    /// A direct conversation, with the name of the other account when the state holds
    /// it.
    Direct { other: Option<String> },
}

/// Where an export stands on the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportStatus {
    Waiting,
    /// The server writes the file now.
    Running,
    Ready,
    Failed(ExportFailure),
}

/// The file of an export on this device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExportSave {
    /// The file arrives: the bytes that are written.
    Saving {
        written: u64,
    },
    Saved(PathBuf),
    /// Why nothing was saved, as the page states it.
    Failed(String),
}

/// One export of the person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportLine {
    pub id: ExportId,
    pub of: ExportOf,
    pub status: ExportStatus,
    /// In the time zone of the person.
    pub requested: OffsetDateTime,
    /// The size of the file of a ready export.
    pub bytes: Option<u64>,
    /// When the server removes the export. Present when the export has finished.
    pub expires: Option<OffsetDateTime>,
    /// The export has a file, and no save of it runs.
    pub can_save: bool,
    pub save: Option<ExportSave>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportsPage {
    /// The newest first.
    pub exports: Listed<Vec<ExportLine>>,
}

/// The group of an action of the audit log, for the filter of the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuditGroup {
    /// Members and bans.
    Members,
    /// Invites and join requests.
    Invites,
    /// Roles and permissions.
    Roles,
    /// Workspaces and channels.
    Structure,
    /// Calls and recordings.
    Calls,
    /// Messages and retention.
    Messages,
    /// Access tokens and the server.
    Server,
}

impl AuditGroup {
    pub const ALL: [Self; 7] = [
        Self::Members,
        Self::Invites,
        Self::Roles,
        Self::Structure,
        Self::Calls,
        Self::Messages,
        Self::Server,
    ];

    /// The group of an action. A new action of the protocol does not compile until it
    /// has a group.
    pub const fn of(event: &AuditEvent) -> Self {
        match event {
            AuditEvent::AccountJoined { .. }
            | AuditEvent::AccountRemoved { .. }
            | AuditEvent::AccountBanned { .. }
            | AuditEvent::AccountUnbanned { .. }
            | AuditEvent::AccountRecovered { .. }
            | AuditEvent::WorkspaceMemberAdded { .. }
            | AuditEvent::WorkspaceMemberRemoved { .. } => Self::Members,
            AuditEvent::InviteCreated { .. }
            | AuditEvent::InviteWorkspacesChanged { .. }
            | AuditEvent::InviteRevoked { .. }
            | AuditEvent::InviteRedeemed { .. }
            | AuditEvent::JoinRequestApproved { .. }
            | AuditEvent::JoinRequestDenied { .. } => Self::Invites,
            AuditEvent::ServerRoleCreated { .. }
            | AuditEvent::ServerRoleUpdated { .. }
            | AuditEvent::ServerRoleAssigned { .. }
            | AuditEvent::ServerRoleUnassigned { .. }
            | AuditEvent::ServerRoleDeleted { .. }
            | AuditEvent::WorkspaceRoleCreated { .. }
            | AuditEvent::WorkspaceRoleUpdated { .. }
            | AuditEvent::WorkspaceRoleAssigned { .. }
            | AuditEvent::WorkspaceRoleUnassigned { .. }
            | AuditEvent::WorkspaceRoleDeleted { .. }
            | AuditEvent::ServerRolePositionChanged { .. }
            | AuditEvent::WorkspaceRolePositionChanged { .. }
            | AuditEvent::ChannelOverrideSet { .. } => Self::Roles,
            AuditEvent::WorkspaceCreated { .. }
            | AuditEvent::WorkspaceRenamed { .. }
            | AuditEvent::WorkspaceDeleted { .. }
            | AuditEvent::CategoryCreated { .. }
            | AuditEvent::CategoryUpdated { .. }
            | AuditEvent::CategoryDeleted { .. }
            | AuditEvent::ChannelCreated { .. }
            | AuditEvent::ChannelUpdated { .. }
            | AuditEvent::ChannelDeleted { .. }
            | AuditEvent::ChannelArchived
            | AuditEvent::ChannelRestored => Self::Structure,
            AuditEvent::ChannelCallSettingsUpdated { .. }
            | AuditEvent::CallMicrophoneBlocked { .. }
            | AuditEvent::CallMicrophoneUnblocked { .. }
            | AuditEvent::CallSoundBlocked { .. }
            | AuditEvent::CallSoundUnblocked { .. }
            | AuditEvent::CallMemberDisconnected { .. }
            | AuditEvent::CallMemberMovedOut { .. }
            | AuditEvent::CallMemberMovedIn { .. }
            | AuditEvent::RecordingStarted { .. }
            | AuditEvent::RecordingStopped { .. }
            | AuditEvent::RecordingRemoved { .. }
            | AuditEvent::TranscriptRequested { .. }
            | AuditEvent::GlossaryReplaced { .. } => Self::Calls,
            AuditEvent::AttachmentLimitsChanged { .. }
            | AuditEvent::PermanentMessageDeleted { .. }
            | AuditEvent::NotificationDefaultChanged { .. }
            | AuditEvent::NotificationDefaultRemoved
            | AuditEvent::RecordingRetentionChanged { .. }
            | AuditEvent::RetentionChanged { .. }
            | AuditEvent::ArchivePeriodChanged { .. }
            | AuditEvent::ExportRequested { .. } => Self::Messages,
            AuditEvent::OwnerClaimed { .. }
            | AuditEvent::OwnerAssigned { .. }
            | AuditEvent::OwnerCodeIssued { .. }
            | AuditEvent::OwnerAccessRestored { .. }
            | AuditEvent::EventEpochRenewed
            | AuditEvent::AccessTokenCreated { .. }
            | AuditEvent::AccessTokenRevoked { .. }
            | AuditEvent::ServerLogoChanged { .. }
            | AuditEvent::Unknown => Self::Server,
        }
    }

    /// The actions of the group that the filter offers, by their names in the protocol.
    pub fn actions(self) -> impl Iterator<Item = &'static str> {
        ACTIONS
            .iter()
            .filter(move |(_, group)| *group == self)
            .map(|(name, _)| *name)
    }
}

/// Every action of the audit log that the filter by action offers, by its name in the
/// protocol, with its group (ADR 0305). The server matches the name exactly. A test
/// compares the list with the actions of the protocol.
pub const ACTIONS: [(&str, AuditGroup); 66] = [
    ("account_joined", AuditGroup::Members),
    ("account_removed", AuditGroup::Members),
    ("account_banned", AuditGroup::Members),
    ("account_unbanned", AuditGroup::Members),
    ("account_recovered", AuditGroup::Members),
    ("workspace_member_added", AuditGroup::Members),
    ("workspace_member_removed", AuditGroup::Members),
    ("invite_created", AuditGroup::Invites),
    ("invite_workspaces_changed", AuditGroup::Invites),
    ("invite_revoked", AuditGroup::Invites),
    ("invite_redeemed", AuditGroup::Invites),
    ("join_request_approved", AuditGroup::Invites),
    ("join_request_denied", AuditGroup::Invites),
    ("server_role_created", AuditGroup::Roles),
    ("server_role_updated", AuditGroup::Roles),
    ("server_role_assigned", AuditGroup::Roles),
    ("server_role_unassigned", AuditGroup::Roles),
    ("server_role_deleted", AuditGroup::Roles),
    ("server_role_position_changed", AuditGroup::Roles),
    ("workspace_role_created", AuditGroup::Roles),
    ("workspace_role_updated", AuditGroup::Roles),
    ("workspace_role_assigned", AuditGroup::Roles),
    ("workspace_role_unassigned", AuditGroup::Roles),
    ("workspace_role_deleted", AuditGroup::Roles),
    ("workspace_role_position_changed", AuditGroup::Roles),
    ("channel_override_set", AuditGroup::Roles),
    ("workspace_created", AuditGroup::Structure),
    ("workspace_renamed", AuditGroup::Structure),
    ("workspace_deleted", AuditGroup::Structure),
    ("category_created", AuditGroup::Structure),
    ("category_updated", AuditGroup::Structure),
    ("category_deleted", AuditGroup::Structure),
    ("channel_created", AuditGroup::Structure),
    ("channel_updated", AuditGroup::Structure),
    ("channel_deleted", AuditGroup::Structure),
    ("channel_archived", AuditGroup::Structure),
    ("channel_restored", AuditGroup::Structure),
    ("channel_call_settings_updated", AuditGroup::Calls),
    ("call_microphone_blocked", AuditGroup::Calls),
    ("call_microphone_unblocked", AuditGroup::Calls),
    ("call_sound_blocked", AuditGroup::Calls),
    ("call_sound_unblocked", AuditGroup::Calls),
    ("call_member_disconnected", AuditGroup::Calls),
    ("call_member_moved_out", AuditGroup::Calls),
    ("call_member_moved_in", AuditGroup::Calls),
    ("recording_started", AuditGroup::Calls),
    ("recording_stopped", AuditGroup::Calls),
    ("recording_removed", AuditGroup::Calls),
    ("transcript_requested", AuditGroup::Calls),
    ("glossary_replaced", AuditGroup::Calls),
    ("attachment_limits_changed", AuditGroup::Messages),
    ("permanent_message_deleted", AuditGroup::Messages),
    ("notification_default_changed", AuditGroup::Messages),
    ("notification_default_removed", AuditGroup::Messages),
    ("recording_retention_changed", AuditGroup::Messages),
    ("retention_changed", AuditGroup::Messages),
    ("archive_period_changed", AuditGroup::Messages),
    ("export_requested", AuditGroup::Messages),
    ("owner_claimed", AuditGroup::Server),
    ("owner_assigned", AuditGroup::Server),
    ("owner_code_issued", AuditGroup::Server),
    ("owner_access_restored", AuditGroup::Server),
    ("event_epoch_renewed", AuditGroup::Server),
    ("access_token_created", AuditGroup::Server),
    ("access_token_revoked", AuditGroup::Server),
    ("server_logo_changed", AuditGroup::Server),
];

/// The group of an action that the filter offers, by the name of the action.
pub fn group_of_action(action: &str) -> Option<AuditGroup> {
    ACTIONS
        .iter()
        .find(|(name, _)| *name == action)
        .map(|(_, group)| *group)
}

/// The most characters of a reason that a row shows (ADR 0106).
const REASON_CHARACTERS: usize = 1000;

/// The reason that a ban or a removal states. It is text of another person: a control
/// character is not shown, and a text beyond the length that the server takes is cut.
fn stated_reason(event: &AuditEvent) -> Option<String> {
    let reason = match event {
        AuditEvent::AccountRemoved { reason, .. }
        | AuditEvent::AccountBanned { reason, .. }
        | AuditEvent::WorkspaceMemberRemoved { reason, .. } => reason.as_deref()?,
        _ => return None,
    };
    let shown: String = reason
        .chars()
        .filter(|character| !character.is_control())
        .take(REASON_CHARACTERS)
        .collect();
    let shown = shown.trim();
    (!shown.is_empty()).then(|| shown.to_owned())
}

/// What the state knows of the names that an audit entry refers to by an identifier. A
/// name that the state does not hold is absent, and the page writes a neutral text in its
/// place.
pub struct Names<'a> {
    server_id: ServerId,
    model: &'a ServerModel,
    known: &'a Known,
}

impl<'a> Names<'a> {
    pub const fn new(server_id: ServerId, model: &'a ServerModel, known: &'a Known) -> Self {
        Self {
            server_id,
            model,
            known,
        }
    }

    pub fn account(&self, account_id: AccountId) -> Option<String> {
        account_name(self.server_id, account_id, self.model, self.known)
    }

    pub fn workspace(&self, workspace_id: WorkspaceId) -> Option<String> {
        self.model
            .workspace(workspace_id)
            .map(|held| held.workspace.name.clone())
    }

    /// A channel as a title names it. An archived channel is named while the state
    /// knows it.
    pub fn channel(&self, channel_id: ChannelId) -> Option<String> {
        self.model
            .readable_channel(channel_id)
            .map(|held| text::admin::channel_name(held.kind, &held.name))
    }

    pub fn workspace_role(&self, role_id: WorkspaceRoleId) -> Option<String> {
        self.model
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.roles.iter())
            .find(|role| role.id == role_id)
            .map(|role| role.name.clone())
    }

    pub fn server_role(&self, role_id: ServerRoleId) -> Option<String> {
        self.model
            .admin
            .server_roles
            .loaded()
            .and_then(|roles| roles.iter().find(|role| role.id == role_id))
            .map(|role| role.name.clone())
    }

    pub fn category(&self, category_id: CategoryId) -> Option<String> {
        self.model
            .workspaces
            .iter()
            .flat_map(|workspace| workspace.categories.iter())
            .find(|category| category.id == category_id)
            .map(|category| category.name.clone())
    }

    /// A moment in the time zone of the person.
    pub fn local(&self, at: OffsetDateTime) -> OffsetDateTime {
        self.known.zone.local(at)
    }
}

/// A scope of the audit log that the person may read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeChoice {
    pub scope: AuditScope,
    /// The name of the workspace of a workspace scope.
    pub workspace: Option<String>,
}

/// Where the read of the shown scope stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuditRead {
    /// The page has not asked for the scope.
    NotRead,
    /// A read is on its way: the first page, or the page after the loaded entries.
    Reading,
    Loaded,
    /// The last read failed. The loaded entries stay.
    Failed {
        sentence: String,
        /// The read asked for the page after the loaded entries.
        older: bool,
    },
}

/// Who did the action of an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// The server itself acted, or a host command.
    Server,
    Account {
        id: AccountId,
        /// Absent: the state does not hold the name.
        name: Option<String>,
    },
}

/// A workspace or a channel that an entry is about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum About {
    Named(String),
    /// The state does not hold it: it was deleted, or the account does not see it now.
    Unknown,
}

/// One entry of the audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditLine {
    pub id: AuditEntryId,
    /// In the time zone of the person.
    pub at: OffsetDateTime,
    pub actor: Actor,
    /// The request was made with an access token of the actor (ADR 0210).
    pub with_token: bool,
    /// What was done, with the names that the state holds.
    pub sentence: String,
    /// The reason that the person stated for a ban or a removal (ADR 0106).
    pub reason: Option<String>,
    /// The workspace of the entry. Absent for an entry about the server, and in the log
    /// of that workspace.
    pub workspace: Option<About>,
    pub channel: Option<About>,
    pub group: AuditGroup,
}

/// An account that the filters by person offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActorChoice {
    pub id: AccountId,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditPage {
    /// The scopes that the person may read: the server first, then the shown workspace.
    pub scopes: Vec<ScopeChoice>,
    /// The scope that the page shows: the one that the person chose while the person may
    /// read it, and the first one otherwise.
    pub scope: Option<AuditScope>,
    pub state: AuditRead,
    /// The loaded entries, the newest first. The server applied the filters.
    pub rows: Vec<AuditLine>,
    /// The number of loaded entries.
    pub loaded: usize,
    /// The server holds older entries, and the page reads them on request.
    pub older: bool,
    /// The page holds the most entries that it keeps. It reads no older entry.
    pub at_bound: bool,
    /// The filters that the entries were asked for with.
    pub filter: AuditFilter,
    /// The accounts that the two filters by person offer: the accounts and the members
    /// that the state holds, the accounts that acted in an entry that the page read, and
    /// the accounts of the filters. In the order of their names, and after them the
    /// accounts without a known name.
    pub people: Vec<ActorChoice>,
}

/// The name of an account, when the state holds it: the own account, the loaded accounts,
/// the loaded members of a workspace, and the names that were read by themselves.
fn account_name(
    server_id: ServerId,
    account_id: AccountId,
    model: &ServerModel,
    known: &Known,
) -> Option<String> {
    if account_id == model.account.id {
        return Some(model.account.name.clone());
    }
    model
        .admin
        .accounts
        .loaded()
        .and_then(|accounts| accounts.iter().find(|held| held.id == account_id))
        .map(|held| held.name.clone())
        .or_else(|| {
            model
                .workspaces
                .iter()
                .filter_map(|workspace| workspace.members.loaded())
                .flatten()
                .find(|member| member.account_id == account_id)
                .map(|member| member.name.clone())
        })
        .or_else(|| known.people.name(server_id, account_id))
}

fn token_line(
    server_id: ServerId,
    token: &AccessTokenInfo,
    model: &ServerModel,
    known: &Known,
) -> TokenLine {
    let expires = known.zone.local(token.expires_at);
    TokenLine {
        id: token.id,
        name: token.name.clone(),
        created_by: token.created_by,
        creator: account_name(server_id, token.created_by, model, known),
        own: token.created_by == model.account.id,
        actions: token.actions.clone(),
        created: known.zone.local(token.created_at),
        expires,
        // The derivation holds the date of today and no time. A token that expires today
        // counts as valid until the next day.
        expired: known.today.is_some_and(|today| expires.date() < today),
    }
}

/// The page of the access tokens.
pub fn tokens(
    server_id: ServerId,
    model: &ServerModel,
    known: &Known,
    admin: &AdminState,
) -> TokensPage {
    let tokens = Listed::of(&model.admin.access_tokens, |tokens| {
        tokens
            .iter()
            .map(|token| token_line(server_id, token, model, known))
            .collect::<Vec<_>>()
    });
    TokensPage {
        more: tokens.loaded().is_some() && model.admin.access_tokens_more,
        tokens,
        made: match &admin.made {
            Some(Made::Token {
                access_token_id,
                token,
            }) => Some(NewToken {
                access_token_id: *access_token_id,
                token: token.clone(),
            }),
            _ => None,
        },
        may_create: model.is_administrator(),
    }
}

/// What an export holds, with the names that the state holds.
pub(crate) fn export_of(
    server_id: ServerId,
    export: &Export,
    model: &ServerModel,
    known: &Known,
) -> ExportOf {
    match export.place {
        MessagePlace::Channel {
            workspace_id,
            channel_id,
        } => match model.readable_channel(channel_id) {
            Some(channel) => ExportOf::Channel {
                name: channel.name.clone(),
                kind: channel.kind,
                workspace: model
                    .workspace(workspace_id)
                    .map(|held| held.workspace.name.clone()),
            },
            None => ExportOf::ChannelGone,
        },
        MessagePlace::Direct {
            low_account_id,
            high_account_id,
            ..
        } => {
            let other = if low_account_id == model.account.id {
                high_account_id
            } else {
                low_account_id
            };
            ExportOf::Direct {
                other: account_name(server_id, other, model, known),
            }
        }
    }
}

fn export_line(
    server_id: ServerId,
    export: &Export,
    model: &ServerModel,
    known: &Known,
    admin: &AdminState,
) -> ExportLine {
    let status = match export.state {
        ExportState::Waiting => ExportStatus::Waiting,
        ExportState::Running => ExportStatus::Running,
        ExportState::Ready => ExportStatus::Ready,
        ExportState::Failed => {
            ExportStatus::Failed(export.failure.unwrap_or(ExportFailure::Failed))
        }
    };
    let save = admin.exports.files.get(&(server_id, export.id)).cloned();
    let saving = matches!(save, Some(ExportSave::Saving { .. }));
    ExportLine {
        id: export.id,
        of: export_of(server_id, export, model, known),
        status,
        requested: known.zone.local(export.requested_at),
        bytes: export.bytes,
        expires: export.expires_at.map(|at| known.zone.local(at)),
        can_save: status == ExportStatus::Ready && export.bytes.is_some() && !saving,
        save,
    }
}

/// The page of the exports of the person.
pub fn exports(
    server_id: ServerId,
    model: &ServerModel,
    known: &Known,
    admin: &AdminState,
) -> ExportsPage {
    ExportsPage {
        exports: Listed::of(&model.admin.exports, |exports| {
            exports
                .iter()
                .map(|export| export_line(server_id, export, model, known, admin))
                .collect::<Vec<_>>()
        }),
    }
}

/// The name of the file of an export on this device:
/// `junto-export-<channel or "conversation">-<date of the request>.zip`. A character of
/// the channel name that is no letter, no digit, no hyphen and no underscore becomes an
/// underscore. The download gives the file a number when a file has the name.
pub fn export_file_name(of: &ExportOf, requested: Date) -> String {
    let place: String = match of {
        ExportOf::Channel { name, .. } => name
            .chars()
            .map(|character| {
                if character.is_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect(),
        ExportOf::ChannelGone => text::admin::records::FILE_OF_CHANNEL.to_owned(),
        ExportOf::Direct { .. } => text::admin::records::FILE_OF_CONVERSATION.to_owned(),
    };
    local_name(&format!(
        "junto-export-{place}-{:04}-{:02}-{:02}.zip",
        requested.year(),
        u8::from(requested.month()),
        requested.day()
    ))
}

fn about<T>(id: Option<T>, name: impl FnOnce(T) -> Option<String>) -> Option<About> {
    id.map(|id| name(id).map_or(About::Unknown, About::Named))
}

fn audit_line(entry: &AuditEntry, scope: AuditScope, names: &Names<'_>) -> AuditLine {
    let in_shown_workspace = match scope {
        AuditScope::Workspace(shown) => entry.workspace_id == Some(shown),
        AuditScope::Server => false,
    };
    AuditLine {
        id: entry.id,
        at: names.local(entry.occurred_at),
        actor: match entry.actor_id {
            Some(id) => Actor::Account {
                id,
                name: names.account(id),
            },
            None => Actor::Server,
        },
        with_token: entry.access_token_id.is_some(),
        sentence: text::admin::records::audit_sentence(&entry.event, names),
        reason: stated_reason(&entry.event),
        workspace: if in_shown_workspace {
            None
        } else {
            about(entry.workspace_id, |id| names.workspace(id))
        },
        channel: about(entry.channel_id, |id| names.channel(id)),
        group: AuditGroup::of(&entry.event),
    }
}

/// The page of the audit log.
pub fn audit(
    server_id: ServerId,
    model: &ServerModel,
    workspace: Option<&WorkspaceModel>,
    known: &Known,
    admin: &AdminState,
) -> AuditPage {
    let mut scopes = Vec::new();
    if holds_server(model, ServerPermission::ViewServerAuditLog) {
        scopes.push(ScopeChoice {
            scope: AuditScope::Server,
            workspace: None,
        });
    }
    if let Some(workspace) = workspace
        && holds_workspace(workspace, WorkspacePermission::ViewAuditEntries)
    {
        scopes.push(ScopeChoice {
            scope: AuditScope::Workspace(workspace.workspace.id),
            workspace: Some(workspace.workspace.name.clone()),
        });
    }
    let log = &admin.audit;
    let chosen = log
        .shown
        .filter(|(of, _)| *of == server_id)
        .map(|(_, scope)| scope)
        .filter(|scope| scopes.iter().any(|choice| choice.scope == *scope));
    let scope = chosen.or_else(|| scopes.first().map(|choice| choice.scope));
    // The entries of the log belong to the scope that was read. Another scope, and the
    // log of another server, shows none of them.
    let read = scope.filter(|_| chosen.is_some());
    let entries: &[AuditEntry] = match read {
        Some(_) => &log.entries,
        None => &[],
    };
    let names = Names::new(server_id, model, known);
    let filter = match read {
        Some(_) => log.filter,
        None => AuditFilter::default(),
    };
    let mut people: Vec<ActorChoice> = Vec::new();
    if read.is_some() {
        let seen = log.people.iter().copied();
        let accounts = model
            .admin
            .accounts
            .loaded()
            .into_iter()
            .flatten()
            .map(|account| account.id);
        let members = workspace
            .and_then(|workspace| workspace.members.loaded())
            .into_iter()
            .flatten()
            .map(|member| member.account_id);
        for id in seen
            .chain(accounts)
            .chain(members)
            .chain(filter.actor)
            .chain(filter.about)
        {
            if !people.iter().any(|held| held.id == id) {
                people.push(ActorChoice {
                    id,
                    name: names.account(id),
                });
            }
        }
    }
    people.sort_by(|one, other| match (&one.name, &other.name) {
        (Some(one_name), Some(other_name)) => one_name
            .to_lowercase()
            .cmp(&other_name.to_lowercase())
            .then(one.id.cmp(&other.id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => one.id.cmp(&other.id),
    });
    let rows: Vec<AuditLine> = match read {
        Some(scope) => entries
            .iter()
            .map(|entry| audit_line(entry, scope, &names))
            .collect(),
        None => Vec::new(),
    };
    let at_bound = entries.len() >= MOST_AUDIT_ENTRIES;
    let state = match (read, &log.failed) {
        (None, _) => AuditRead::NotRead,
        (Some(_), Some(failed)) => AuditRead::Failed {
            sentence: failed.sentence.clone(),
            older: failed.older,
        },
        (Some(_), None) if log.reading => AuditRead::Reading,
        (Some(_), None) => AuditRead::Loaded,
    };
    AuditPage {
        scopes,
        scope,
        state,
        rows,
        loaded: entries.len(),
        older: read.is_some() && log.next_before.is_some() && !at_bound,
        at_bound,
        filter,
        people,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use junto_client_core::model::{AdminModel, Collection, LoadFailure};
    use junto_client_core::protocol::account::{AccountStatus, AccountSummary, OwnAccount};
    use junto_client_core::protocol::discovery::NotificationLevel;
    use junto_client_core::protocol::events::Cursor;
    use junto_client_core::protocol::ids::{
        ConversationId, DeviceId, InviteId, JoinRequestId, MessageId, RecordingId, StreamEpoch,
    };
    use junto_client_core::protocol::recording::RecordingStopReason;
    use junto_client_core::protocol::roles::{ServerRole, WorkspaceRole};
    use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
    use junto_client_core::protocol::structure::{ArchivedChannel, Category, Channel, Workspace};
    use time::UtcOffset;

    use super::*;
    use crate::state::admin::records::AuditFailure;
    use crate::state::admin::{AuditLog, AuditPeriod, ExportFiles};
    use crate::state::feed::Zone;
    use crate::text::admin::records as txt;

    fn time() -> OffsetDateTime {
        // 15 January 2027, 08:00 UTC.
        OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap()
    }

    fn known() -> Known {
        Known {
            zone: Zone::Fixed(UtcOffset::UTC),
            ..Known::default()
        }
    }

    /// One server with the default workspace "Studio" and the workspace "Lab". "Studio"
    /// has the category "Projects", the text channel "general" and the roles "everyone"
    /// and "leads". The account "Luca" is a member of "Studio", holds the role "leads"
    /// and holds no permission.
    struct Scene {
        server_id: ServerId,
        own: AccountId,
        studio: WorkspaceId,
        lab: WorkspaceId,
        leads: WorkspaceRoleId,
        projects: CategoryId,
        general: ChannelId,
        snapshot: Snapshot,
    }

    fn channel(workspace_id: WorkspaceId, name: &str) -> Channel {
        Channel {
            id: ChannelId::generate(),
            workspace_id,
            category_id: None,
            kind: ChannelKind::Text,
            name: name.to_owned(),
            position: 0,
            permanent_messages: false,
            user_limit: None,
            hides_occupants: false,
        }
    }

    fn scene() -> Scene {
        let own = AccountId::generate();
        let (studio, lab) = (WorkspaceId::generate(), WorkspaceId::generate());
        let projects = CategoryId::generate();
        let role = |name: &str, position| WorkspaceRole {
            position,
            id: WorkspaceRoleId::generate(),
            workspace_id: studio,
            name: name.to_owned(),
            permissions: Vec::new(),
            is_everyone: position == 0,
        };
        let (everyone, leads) = (role("everyone", 0), role("leads", 1));
        let general = channel(studio, "general");
        let workspace = |id, name: &str, is_default| Workspace {
            id,
            name: name.to_owned(),
            is_default,
        };
        Scene {
            server_id: ServerId::generate(),
            own,
            studio,
            lab,
            leads: leads.id,
            projects,
            general: general.id,
            snapshot: Snapshot {
                cursor: Cursor {
                    epoch: StreamEpoch::generate(),
                    position: 1,
                },
                account: OwnAccount {
                    id: own,
                    name: "Luca".to_owned(),
                    avatar: None,
                    is_owner: false,
                    server_role_ids: Vec::new(),
                    server_permissions: Vec::new(),
                    workspace_ids: vec![studio],
                },
                workspaces: vec![
                    WorkspaceState {
                        workspace: workspace(studio, "Studio", true),
                        own_role_ids: vec![leads.id],
                        roles: vec![everyone, leads],
                        categories: vec![Category {
                            id: projects,
                            workspace_id: studio,
                            name: "Projects".to_owned(),
                            position: 0,
                        }],
                        channels: vec![general],
                        calls: Vec::new(),
                        permissions: Vec::new(),
                        channel_permissions: Vec::new(),
                        notification_default: None,
                    },
                    WorkspaceState {
                        workspace: workspace(lab, "Lab", false),
                        own_role_ids: Vec::new(),
                        roles: Vec::new(),
                        categories: Vec::new(),
                        channels: Vec::new(),
                        calls: Vec::new(),
                        permissions: Vec::new(),
                        channel_permissions: Vec::new(),
                        notification_default: None,
                    },
                ],
                own_call: None,
                direct_calls: Vec::new(),
                call_invitations: Vec::new(),
                sent_call_invitations: Vec::new(),
                call_moves: Vec::new(),
                entry_requests: Vec::new(),
                direct_mutes: Vec::new(),
                messages: Vec::new(),
                threads: Vec::new(),
                threads_more: false,
                threads_next: None,
                notifications: Vec::new(),
                notification_settings: Vec::new(),
            },
        }
    }

    impl Scene {
        fn administrator(mut self) -> Self {
            self.snapshot.account.server_permissions = vec![ServerPermission::Administrator];
            self
        }

        /// The role "leads" of the account gives these permissions in "Studio".
        fn leading_with(mut self, permissions: Vec<WorkspacePermission>) -> Self {
            self.snapshot.workspaces[0].roles[1].permissions = permissions;
            self
        }

        fn model(&self, admin: AdminModel) -> ServerModel {
            let mut model =
                ServerModel::from_snapshot(crate::state::fixtures::stated(self.snapshot.clone()));
            model.admin = Arc::new(admin);
            model
        }

        fn in_general(&self) -> MessagePlace {
            MessagePlace::Channel {
                workspace_id: self.studio,
                channel_id: self.general,
            }
        }
    }

    fn token(name: &str, created_by: AccountId) -> AccessTokenInfo {
        AccessTokenInfo {
            id: AccessTokenId::generate(),
            name: name.to_owned(),
            created_by,
            actions: vec![TokenAction::ReadStructure],
            created_at: time(),
            expires_at: time() + Duration::days(30),
        }
    }

    fn with_tokens(listed: Vec<AccessTokenInfo>) -> AdminModel {
        AdminModel {
            access_tokens: Collection::Loaded(listed),
            ..AdminModel::default()
        }
    }

    fn export(place: MessagePlace, state: ExportState) -> Export {
        Export {
            id: ExportId::generate(),
            place,
            state,
            failure: None,
            requested_at: time(),
            finished_at: None,
            expires_at: None,
            bytes: None,
        }
    }

    /// An export with a file of 2 MB that the server removes after 7 days.
    fn ready(place: MessagePlace) -> Export {
        Export {
            finished_at: Some(time() + Duration::hours(1)),
            expires_at: Some(time() + Duration::days(7)),
            bytes: Some(2_000_000),
            ..export(place, ExportState::Ready)
        }
    }

    fn with_exports(listed: Vec<Export>) -> AdminModel {
        AdminModel {
            exports: Collection::Loaded(listed),
            ..AdminModel::default()
        }
    }

    fn entry(actor_id: Option<AccountId>, event: AuditEvent) -> AuditEntry {
        AuditEntry {
            id: AuditEntryId::generate(),
            occurred_at: time(),
            actor_id,
            access_token_id: None,
            workspace_id: None,
            channel_id: None,
            event,
        }
    }

    /// The state of the administration after a read of a log left these entries.
    fn log(server_id: ServerId, scope: AuditScope, entries: Vec<AuditEntry>) -> AdminState {
        AdminState {
            audit: AuditLog {
                shown: Some((server_id, scope)),
                entries,
                ..AuditLog::default()
            },
            ..AdminState::default()
        }
    }

    /// True when the text holds an identifier in its written form.
    fn holds_identifier(text: &str) -> bool {
        let characters: Vec<char> = text.chars().collect();
        characters.windows(36).any(|window| {
            window.iter().enumerate().all(|(at, character)| match at {
                8 | 13 | 18 | 23 => *character == '-',
                _ => character.is_ascii_hexdigit(),
            })
        })
    }

    /// The place of each action of the protocol in the list of the test. A new action
    /// does not compile until it has a place, and the test fails until the list holds
    /// it.
    fn place(event: &AuditEvent) -> usize {
        match event {
            AuditEvent::OwnerClaimed { .. } => 0,
            AuditEvent::OwnerAssigned { .. } => 1,
            AuditEvent::OwnerCodeIssued { .. } => 2,
            AuditEvent::OwnerAccessRestored { .. } => 3,
            AuditEvent::EventEpochRenewed => 4,
            AuditEvent::InviteCreated { .. } => 5,
            AuditEvent::InviteRevoked { .. } => 6,
            AuditEvent::AccountJoined { .. } => 7,
            AuditEvent::InviteRedeemed { .. } => 8,
            AuditEvent::JoinRequestApproved { .. } => 9,
            AuditEvent::JoinRequestDenied { .. } => 10,
            AuditEvent::AccountRemoved { .. } => 11,
            AuditEvent::AccountBanned { .. } => 12,
            AuditEvent::AccountUnbanned { .. } => 13,
            AuditEvent::AccountRecovered { .. } => 14,
            AuditEvent::ServerRoleCreated { .. } => 15,
            AuditEvent::ServerRoleUpdated { .. } => 16,
            AuditEvent::ServerRoleAssigned { .. } => 17,
            AuditEvent::AccessTokenCreated { .. } => 18,
            AuditEvent::AccessTokenRevoked { .. } => 19,
            AuditEvent::ServerRoleUnassigned { .. } => 20,
            AuditEvent::WorkspaceCreated { .. } => 21,
            AuditEvent::WorkspaceRenamed { .. } => 22,
            AuditEvent::WorkspaceRoleCreated { .. } => 23,
            AuditEvent::WorkspaceRoleUpdated { .. } => 24,
            AuditEvent::WorkspaceRoleAssigned { .. } => 25,
            AuditEvent::WorkspaceRoleUnassigned { .. } => 26,
            AuditEvent::WorkspaceMemberAdded { .. } => 27,
            AuditEvent::CategoryCreated { .. } => 28,
            AuditEvent::CategoryUpdated { .. } => 29,
            AuditEvent::ChannelCreated { .. } => 30,
            AuditEvent::ChannelUpdated { .. } => 31,
            AuditEvent::ChannelOverrideSet { .. } => 32,
            AuditEvent::ChannelDeleted { .. } => 33,
            AuditEvent::ChannelCallSettingsUpdated { .. } => 34,
            AuditEvent::CallMicrophoneBlocked { .. } => 35,
            AuditEvent::CallMicrophoneUnblocked { .. } => 36,
            AuditEvent::CallSoundBlocked { .. } => 37,
            AuditEvent::CallSoundUnblocked { .. } => 38,
            AuditEvent::CallMemberDisconnected { .. } => 39,
            AuditEvent::CallMemberMovedOut { .. } => 40,
            AuditEvent::CallMemberMovedIn { .. } => 41,
            AuditEvent::RecordingStarted { .. } => 42,
            AuditEvent::RecordingStopped { .. } => 43,
            AuditEvent::RecordingRemoved { .. } => 44,
            AuditEvent::TranscriptRequested { .. } => 45,
            AuditEvent::GlossaryReplaced { .. } => 46,
            AuditEvent::AttachmentLimitsChanged { .. } => 47,
            AuditEvent::PermanentMessageDeleted { .. } => 48,
            AuditEvent::NotificationDefaultChanged { .. } => 49,
            AuditEvent::RecordingRetentionChanged { .. } => 50,
            AuditEvent::RetentionChanged { .. } => 51,
            AuditEvent::ChannelArchived => 52,
            AuditEvent::ChannelRestored => 53,
            AuditEvent::ArchivePeriodChanged { .. } => 54,
            AuditEvent::CategoryDeleted { .. } => 55,
            AuditEvent::WorkspaceRoleDeleted { .. } => 56,
            AuditEvent::ServerRoleDeleted { .. } => 57,
            AuditEvent::WorkspaceDeleted { .. } => 58,
            AuditEvent::ExportRequested { .. } => 59,
            AuditEvent::ServerRolePositionChanged { .. } => 60,
            AuditEvent::WorkspaceRolePositionChanged { .. } => 61,
            AuditEvent::NotificationDefaultRemoved => 62,
            AuditEvent::Unknown => 63,
            AuditEvent::WorkspaceMemberRemoved { .. } => 64,
            AuditEvent::InviteWorkspacesChanged { .. } => 65,
            AuditEvent::ServerLogoChanged { .. } => 66,
        }
    }

    /// One action of every kind, each with the names that its sentence must hold when
    /// the state knows the account `ana` as "Ana" and the server role `moderators` as
    /// "Moderators".
    fn every_action(
        scene: &Scene,
        ana: AccountId,
        moderators: ServerRoleId,
    ) -> Vec<(AuditEvent, Vec<&'static str>)> {
        let invite_id = InviteId::generate();
        let device_id = DeviceId::generate();
        let recording_id = RecordingId::generate();
        let access_token_id = AccessTokenId::generate();
        let join_request_id = JoinRequestId::generate();
        let account_id = ana;
        vec![
            (AuditEvent::OwnerClaimed { account_id }, vec!["Ana"]),
            (
                AuditEvent::OwnerAssigned {
                    account_id,
                    previous_owner_id: Some(scene.own),
                },
                vec!["Ana", "Luca"],
            ),
            (
                AuditEvent::OwnerCodeIssued {
                    replaced_a_code: true,
                    expires_at: Some(time()),
                },
                vec!["works no longer", "15 January 2027"],
            ),
            (
                AuditEvent::OwnerAccessRestored {
                    account_id,
                    device_id,
                },
                vec!["Ana"],
            ),
            (AuditEvent::EventEpochRenewed, vec![]),
            (
                AuditEvent::InviteCreated {
                    invite_id,
                    expires_at: time(),
                    max_uses: 5,
                    workspace_ids: vec![scene.studio, scene.lab],
                },
                vec!["15 January 2027", "5 uses", "“Studio”", "“Lab”"],
            ),
            (
                AuditEvent::InviteRevoked {
                    invite_id,
                    invite_created_by: Some(account_id),
                },
                vec!["Ana"],
            ),
            (
                AuditEvent::AccountJoined {
                    account_id,
                    invite_id,
                    invite_created_by: scene.own,
                },
                vec!["Ana", "Luca"],
            ),
            (
                AuditEvent::InviteRedeemed {
                    account_id,
                    invite_id,
                    invite_created_by: scene.own,
                    workspace_ids: vec![scene.lab],
                },
                vec!["Ana", "Luca", "“Lab”"],
            ),
            (
                AuditEvent::JoinRequestApproved {
                    join_request_id,
                    account_id,
                    workspace_ids: Vec::new(),
                },
                vec!["Ana", "the default workspace"],
            ),
            (AuditEvent::JoinRequestDenied { join_request_id }, vec![]),
            (
                AuditEvent::AccountRemoved {
                    reason: None,
                    account_id,
                },
                vec!["Ana"],
            ),
            (
                AuditEvent::AccountBanned {
                    reason: Some("Spam".to_owned()),
                    account_id,
                    cancelled_invite_ids: vec![invite_id, InviteId::generate()],
                },
                vec!["Ana", "2 invites"],
            ),
            (
                AuditEvent::WorkspaceMemberRemoved {
                    account_id,
                    reason: None,
                },
                vec!["Ana", "removed", "from the workspace"],
            ),
            (
                AuditEvent::InviteWorkspacesChanged {
                    invite_id,
                    invite_created_by: account_id,
                    removed_workspace_id: scene.lab,
                },
                vec!["Ana", "“Lab”", "no longer"],
            ),
            (
                AuditEvent::ServerLogoChanged {
                    logo: Some(junto_client_core::protocol::account::Avatar {
                        hash: "a".repeat(64),
                        media_type: junto_client_core::protocol::account::ImageType::Png,
                        bytes: 2048,
                        width: 64,
                        height: 64,
                    }),
                },
                vec!["logo", "set"],
            ),
            (AuditEvent::AccountUnbanned { account_id }, vec!["Ana"]),
            (
                AuditEvent::AccountRecovered {
                    account_id,
                    device_id,
                },
                vec!["Ana"],
            ),
            (
                AuditEvent::ServerRoleCreated {
                    role_id: moderators,
                    name: "Moderators".to_owned(),
                    permissions: vec![
                        ServerPermission::BanMembers,
                        ServerPermission::CreateInvites,
                    ],
                },
                vec!["“Moderators”", "“ban members”", "“create invites”"],
            ),
            (
                AuditEvent::ServerRoleUpdated {
                    role_id: moderators,
                    name: "Moderators".to_owned(),
                    permissions: Vec::new(),
                },
                vec!["“Moderators”", "no permission"],
            ),
            (
                AuditEvent::ServerRoleAssigned {
                    role_id: moderators,
                    account_id,
                },
                vec!["“Moderators”", "Ana"],
            ),
            (
                AuditEvent::AccessTokenCreated {
                    access_token_id,
                    name: "setup".to_owned(),
                    actions: vec![TokenAction::ReadStructure, TokenAction::Unknown],
                    expires_at: time(),
                },
                vec![
                    "“setup”",
                    "read the structure",
                    "an action that this version does not know",
                    "15 January 2027",
                ],
            ),
            (
                AuditEvent::AccessTokenRevoked {
                    access_token_id,
                    created_by: account_id,
                },
                vec!["Ana"],
            ),
            (
                AuditEvent::ServerRoleUnassigned {
                    role_id: moderators,
                    account_id,
                },
                vec!["“Moderators”", "Ana"],
            ),
            (
                AuditEvent::WorkspaceCreated {
                    workspace_id: scene.lab,
                    name: "Lab".to_owned(),
                },
                vec!["“Lab”"],
            ),
            (
                AuditEvent::WorkspaceRenamed {
                    workspace_id: scene.lab,
                    name: "Laboratory".to_owned(),
                },
                vec!["“Laboratory”"],
            ),
            (
                AuditEvent::WorkspaceRoleCreated {
                    role_id: scene.leads,
                    name: "leads".to_owned(),
                    permissions: vec![WorkspacePermission::ManageChannel],
                },
                vec!["“leads”", "“manage channel”"],
            ),
            (
                AuditEvent::WorkspaceRoleUpdated {
                    role_id: scene.leads,
                    name: "leads".to_owned(),
                    permissions: vec![
                        WorkspacePermission::ManageChannel,
                        WorkspacePermission::ViewAuditEntries,
                    ],
                },
                vec!["“leads”", "“manage channel”", "“view audit entries”"],
            ),
            (
                AuditEvent::WorkspaceRoleAssigned {
                    role_id: scene.leads,
                    account_id,
                },
                vec!["“leads”", "Ana"],
            ),
            (
                AuditEvent::WorkspaceRoleUnassigned {
                    role_id: scene.leads,
                    account_id,
                },
                vec!["“leads”", "Ana"],
            ),
            (AuditEvent::WorkspaceMemberAdded { account_id }, vec!["Ana"]),
            (
                AuditEvent::CategoryCreated {
                    category_id: scene.projects,
                    name: "Projects".to_owned(),
                },
                vec!["“Projects”"],
            ),
            (
                AuditEvent::CategoryUpdated {
                    category_id: scene.projects,
                    name: "Projects".to_owned(),
                    position: 3,
                },
                vec!["“Projects”", "position is 3"],
            ),
            (
                AuditEvent::ChannelCreated {
                    name: "Daily".to_owned(),
                    kind: ChannelKind::Voice,
                },
                vec!["voice channel “Daily”"],
            ),
            (
                AuditEvent::ChannelUpdated {
                    name: "general".to_owned(),
                    category_id: Some(scene.projects),
                    position: 1,
                    permanent_messages: true,
                },
                vec!["“general”", "the category “Projects”", "messages are on"],
            ),
            (
                AuditEvent::ChannelOverrideSet {
                    role_id: scene.leads,
                    allow: vec![WorkspacePermission::ViewChannel],
                    deny: vec![WorkspacePermission::ManageMessages],
                },
                vec!["“leads”", "“view channel”", "“manage messages”"],
            ),
            (
                AuditEvent::ChannelDeleted {
                    name: "general".to_owned(),
                },
                vec!["“general”"],
            ),
            (
                AuditEvent::ChannelCallSettingsUpdated {
                    user_limit: Some(4),
                    hides_occupants: true,
                },
                vec!["4 people", "hides"],
            ),
            (
                AuditEvent::CallMicrophoneBlocked { account_id },
                vec!["Ana"],
            ),
            (
                AuditEvent::CallMicrophoneUnblocked { account_id },
                vec!["Ana"],
            ),
            (AuditEvent::CallSoundBlocked { account_id }, vec!["Ana"]),
            (AuditEvent::CallSoundUnblocked { account_id }, vec!["Ana"]),
            (
                AuditEvent::CallMemberDisconnected { account_id },
                vec!["Ana"],
            ),
            (AuditEvent::CallMemberMovedOut { account_id }, vec!["Ana"]),
            (AuditEvent::CallMemberMovedIn { account_id }, vec!["Ana"]),
            (AuditEvent::RecordingStarted { recording_id }, vec![]),
            (
                AuditEvent::RecordingStopped {
                    recording_id,
                    reason: RecordingStopReason::StorageReserve,
                },
                vec!["reserve"],
            ),
            (AuditEvent::RecordingRemoved { recording_id }, vec![]),
            (AuditEvent::TranscriptRequested { recording_id }, vec![]),
            (AuditEvent::GlossaryReplaced { terms: 12 }, vec!["12 terms"]),
            (
                AuditEvent::AttachmentLimitsChanged {
                    bytes_per_file: 50_000_000,
                    files_per_message: 10,
                },
                vec!["50 MB", "10 files"],
            ),
            (
                AuditEvent::PermanentMessageDeleted {
                    message_id: MessageId::generate(),
                },
                vec![],
            ),
            (
                AuditEvent::NotificationDefaultChanged {
                    level: NotificationLevel::Mentions,
                },
                vec!["mentions only"],
            ),
            (
                AuditEvent::RecordingRetentionChanged { days: Some(30) },
                vec!["30 days"],
            ),
            (
                AuditEvent::RetentionChanged { days: None },
                vec!["no longer removed"],
            ),
            (AuditEvent::ChannelArchived, vec![]),
            (AuditEvent::ChannelRestored, vec![]),
            (
                AuditEvent::ArchivePeriodChanged { days: Some(90) },
                vec!["90 days"],
            ),
            (
                AuditEvent::CategoryDeleted {
                    category_id: scene.projects,
                    name: "Projects".to_owned(),
                },
                vec!["“Projects”"],
            ),
            (
                AuditEvent::WorkspaceRoleDeleted {
                    role_id: scene.leads,
                    name: "leads".to_owned(),
                    permissions: vec![WorkspacePermission::ManageChannel],
                    holders: 2,
                },
                vec!["“leads”", "“manage channel”", "2 members held it"],
            ),
            (
                AuditEvent::ServerRoleDeleted {
                    role_id: moderators,
                    name: "Moderators".to_owned(),
                    permissions: Vec::new(),
                    holders: 0,
                },
                vec!["“Moderators”", "no permission", "no account held it"],
            ),
            (
                AuditEvent::WorkspaceDeleted {
                    workspace_id: scene.lab,
                    name: "Lab".to_owned(),
                },
                vec!["“Lab”"],
            ),
            (
                AuditEvent::ExportRequested {
                    export_id: ExportId::generate(),
                },
                vec![],
            ),
            (
                AuditEvent::ServerRolePositionChanged {
                    role_id: moderators,
                    position: 4,
                },
                vec!["“Moderators”", "position 4"],
            ),
            (
                AuditEvent::WorkspaceRolePositionChanged {
                    role_id: scene.leads,
                    position: 2,
                },
                vec!["“leads”", "position 2"],
            ),
            (AuditEvent::NotificationDefaultRemoved, vec!["sets none"]),
            (AuditEvent::Unknown, vec![txt::UNKNOWN_ACTION]),
        ]
    }

    #[test]
    fn the_tokens_are_on_their_way_until_they_are_read_and_a_failed_read_carries_its_failure() {
        let scene = scene().administrator();
        let admin = AdminState::default();
        for collection in [Collection::NotLoaded, Collection::Loading] {
            let model = scene.model(AdminModel {
                access_tokens: collection,
                // The mark of an earlier read tells nothing while no list is shown.
                access_tokens_more: true,
                ..AdminModel::default()
            });
            let page = tokens(scene.server_id, &model, &known(), &admin);
            assert_eq!(page.tokens, Listed::Loading);
            assert!(!page.more);
        }
        let model = scene.model(AdminModel {
            access_tokens: Collection::Failed(LoadFailure::NotAllowed),
            ..AdminModel::default()
        });
        assert_eq!(
            tokens(scene.server_id, &model, &known(), &admin).tokens,
            Listed::Failed(LoadFailure::NotAllowed)
        );
        let model = scene.model(with_tokens(Vec::new()));
        assert_eq!(
            tokens(scene.server_id, &model, &known(), &admin).tokens,
            Listed::Loaded(Vec::new())
        );
    }

    #[test]
    fn a_token_carries_its_name_its_creator_its_actions_and_its_times_in_the_zone_of_the_person() {
        let scene = scene().administrator();
        let ana = AccountId::generate();
        let own = token("setup", scene.own);
        let mut by_ana = token("deploy", ana);
        by_ana.actions = vec![TokenAction::ManageStructure, TokenAction::ReadAuditLog];
        // 23:30 UTC is the next day two hours east of it.
        by_ana.expires_at = time() + Duration::minutes(15 * 60 + 30);
        let by_another = token("old", AccountId::generate());
        let model = scene.model(with_tokens(vec![
            own.clone(),
            by_ana.clone(),
            by_another.clone(),
        ]));
        let offset = UtcOffset::from_hms(2, 0, 0).unwrap();
        let mut known = Known {
            zone: Zone::Fixed(offset),
            ..Known::default()
        };
        known.people.read_name(scene.server_id, ana, "Ana");
        let page = tokens(scene.server_id, &model, &known, &AdminState::default());
        let rows = page.tokens.loaded().unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.id, row.name.as_str(), row.creator.as_deref(), row.own))
                .collect::<Vec<_>>(),
            [
                (own.id, "setup", Some("Luca"), true),
                (by_ana.id, "deploy", Some("Ana"), false),
                (by_another.id, "old", None, false),
            ]
        );
        assert_eq!(rows[1].created_by, ana);
        assert_eq!(
            rows[1].actions,
            [TokenAction::ManageStructure, TokenAction::ReadAuditLog]
        );
        assert_eq!(
            txt::token_actions(&rows[1].actions),
            "Actions: change the structure, read the audit log"
        );
        assert_eq!(rows[1].created, time());
        assert_eq!(rows[1].created.offset(), offset);
        assert_eq!(rows[1].expires.offset(), offset);
        assert_eq!(rows[1].expires.date(), (time() + Duration::days(1)).date());
        assert_eq!((rows[1].expires.hour(), rows[1].expires.minute()), (1, 30));
        assert!(!rows[1].expired);
        assert!(!page.more);
        assert_eq!(page.made, None);
    }

    #[test]
    fn an_action_of_a_newer_server_is_shown_as_an_action_that_this_version_does_not_know() {
        let scene = scene().administrator();
        let mut newer = token("newer", scene.own);
        newer.actions = vec![TokenAction::ReadStructure, TokenAction::Unknown];
        let model = scene.model(with_tokens(vec![newer]));
        let page = tokens(scene.server_id, &model, &known(), &AdminState::default());
        let row = &page.tokens.loaded().unwrap()[0];
        assert_eq!(
            row.actions,
            [TokenAction::ReadStructure, TokenAction::Unknown]
        );
        assert_eq!(
            txt::token_action(TokenAction::Unknown),
            "an action that this version does not know"
        );
        assert_eq!(
            txt::token_actions(&row.actions),
            "Actions: read the structure, an action that this version does not know"
        );
    }

    #[test]
    fn the_page_notes_that_the_server_holds_more_tokens_than_it_reads() {
        let scene = scene().administrator();
        let listed = vec![token("one", scene.own), token("two", scene.own)];
        let mut admin = with_tokens(listed);
        let page = tokens(
            scene.server_id,
            &scene.model(admin.clone()),
            &known(),
            &AdminState::default(),
        );
        assert!(!page.more);
        admin.access_tokens_more = true;
        let page = tokens(
            scene.server_id,
            &scene.model(admin),
            &known(),
            &AdminState::default(),
        );
        assert!(page.more);
        assert_eq!(page.tokens.loaded().unwrap().len(), 2);
        assert_eq!(
            txt::more_tokens(2),
            "The server holds more access tokens. The page shows the newest 2."
        );
    }

    #[test]
    fn the_secret_of_a_new_token_is_carried_until_it_is_put_away() {
        let scene = scene().administrator();
        let model = scene.model(with_tokens(Vec::new()));
        let access_token_id = AccessTokenId::generate();
        let admin = AdminState {
            made: Some(Made::Token {
                access_token_id,
                token: AccessToken::new("jat_secret-value"),
            }),
            ..AdminState::default()
        };
        let page = tokens(scene.server_id, &model, &known(), &admin);
        let made = page.made.unwrap();
        assert_eq!(made.access_token_id, access_token_id);
        assert_eq!(made.token.expose(), "jat_secret-value");

        let page = tokens(scene.server_id, &model, &known(), &AdminState::default());
        assert_eq!(page.made, None);
        // What another request made is not the secret of a token.
        let admin = AdminState {
            made: Some(Made::Export(ExportId::generate())),
            ..AdminState::default()
        };
        assert_eq!(tokens(scene.server_id, &model, &known(), &admin).made, None);
    }

    #[test]
    fn an_administrator_may_create_a_token_and_sees_the_tokens_of_every_account() {
        let page = |scene: &Scene| {
            let listed = vec![
                token("setup", scene.own),
                token("deploy", AccountId::generate()),
            ];
            tokens(
                scene.server_id,
                &scene.model(with_tokens(listed)),
                &known(),
                &AdminState::default(),
            )
        };
        let shown = page(&scene().administrator());
        assert!(shown.may_create);
        let rows = shown.tokens.loaded().unwrap();
        assert_eq!(
            rows.iter().map(|row| row.own).collect::<Vec<_>>(),
            [true, false]
        );
        // The server owner is an administrator without the permission.
        let mut owner = scene();
        owner.snapshot.account.is_owner = true;
        assert!(page(&owner).may_create);
        assert_eq!(txt::tokens_help(true), txt::TOKENS_HELP);
        assert_eq!(txt::tokens_label(true), "Access tokens of the server");
    }

    #[test]
    fn a_person_who_is_no_administrator_keeps_the_own_tokens_and_may_not_create_one() {
        // The account was an administrator and created two tokens. The server lists the
        // own tokens for every account, and their creator revokes them.
        let page = |scene: &Scene| {
            let listed = vec![token("setup", scene.own), token("deploy", scene.own)];
            tokens(
                scene.server_id,
                &scene.model(with_tokens(listed)),
                &known(),
                &AdminState::default(),
            )
        };
        let mut inviter = scene();
        inviter.snapshot.account.server_permissions = vec![ServerPermission::CreateInvites];
        for scene in [scene(), inviter] {
            let shown = page(&scene);
            assert!(!shown.may_create);
            let rows = shown.tokens.loaded().unwrap();
            assert_eq!(
                rows.iter()
                    .map(|row| (row.name.as_str(), row.own, row.creator.as_deref()))
                    .collect::<Vec<_>>(),
                [
                    ("setup", true, Some("Luca")),
                    ("deploy", true, Some("Luca"))
                ]
            );
        }
        // The page says that the list holds the own tokens, and why no token is created.
        assert_eq!(txt::tokens_help(false), txt::OWN_TOKENS_HELP);
        assert!(txt::OWN_TOKENS_HELP.contains("that your account created"));
        assert_eq!(txt::tokens_label(false), "Your access tokens");
        assert_eq!(txt::no_tokens(false), "Your account has no access token.");
        assert_eq!(
            txt::TOKEN_NEEDS,
            "Only an administrator creates an access token."
        );
    }

    #[test]
    fn a_token_is_expired_when_the_day_of_its_expiry_is_before_today() {
        let scene = scene().administrator();
        // The token expires on 16 January 2027.
        let mut held = token("short", scene.own);
        held.expires_at = time() + Duration::days(1);
        let model = scene.model(with_tokens(vec![held]));
        let expired = |known: &Known| {
            let page = tokens(scene.server_id, &model, known, &AdminState::default());
            page.tokens.loaded().unwrap()[0].expired
        };
        let day = |days: i64| Some((time() + Duration::days(days)).date());
        // Without the date of today nothing is known about the expiry.
        let mut known = known();
        assert!(!expired(&known));
        known.today = day(0);
        assert!(!expired(&known));
        // On the day of the expiry the token still counts as valid.
        known.today = day(1);
        assert!(!expired(&known));
        known.today = day(2);
        assert!(expired(&known));
    }

    #[test]
    fn a_new_token_needs_a_name_of_1_to_64_characters_an_action_and_at_most_366_days() {
        let now = time();
        let asked = new_token(
            "  setup  ",
            &[
                TokenAction::ReadAuditLog,
                TokenAction::ReadStructure,
                TokenAction::ReadStructure,
                TokenAction::Unknown,
            ],
            30,
            now,
        )
        .unwrap();
        assert_eq!(asked.name, "setup");
        // The actions are sent in the order of the protocol, each once, and an action
        // that this version does not know is not sent.
        assert_eq!(
            asked.actions,
            [TokenAction::ReadStructure, TokenAction::ReadAuditLog]
        );
        assert_eq!(asked.expires_at, now + Duration::days(30));

        let read = [TokenAction::ReadStructure];
        assert_eq!(new_token("   ", &read, 30, now), Err(TokenFault::NameEmpty));
        assert!(new_token(&"n".repeat(LONGEST_TOKEN_NAME), &read, 30, now).is_ok());
        assert_eq!(
            new_token(&"n".repeat(LONGEST_TOKEN_NAME + 1), &read, 30, now),
            Err(TokenFault::NameTooLong)
        );
        assert_eq!(LONGEST_TOKEN_NAME, 64);
        assert_eq!(new_token("setup", &[], 30, now), Err(TokenFault::NoAction));
        assert_eq!(
            new_token("setup", &[TokenAction::Unknown], 30, now),
            Err(TokenFault::NoAction)
        );

        assert_eq!(token_expiry(7, now), now + Duration::days(7));
        let longest = token_expiry(LONGEST_TOKEN_EXPIRY_DAYS, now);
        assert!(longest < now + Duration::days(366));
        assert!(longest > now + Duration::days(365));
        // A lifetime outside the bounds of the server is brought inside them.
        assert_eq!(token_expiry(4000, now), longest);
        assert_eq!(token_expiry(0, now), now + Duration::days(1));
        assert_eq!(TOKEN_EXPIRY_DAYS, [7, 30, 90, 366]);
        assert!(TOKEN_EXPIRY_DAYS.contains(&DEFAULT_TOKEN_EXPIRY_DAYS));
    }

    #[test]
    fn the_exports_are_on_their_way_until_they_are_read_and_a_failed_read_carries_its_failure() {
        let scene = scene();
        let admin = AdminState::default();
        for collection in [Collection::NotLoaded, Collection::Loading] {
            let model = scene.model(AdminModel {
                exports: collection,
                ..AdminModel::default()
            });
            assert_eq!(
                exports(scene.server_id, &model, &known(), &admin).exports,
                Listed::Loading
            );
        }
        let model = scene.model(AdminModel {
            exports: Collection::Failed(LoadFailure::NoAnswer),
            ..AdminModel::default()
        });
        assert_eq!(
            exports(scene.server_id, &model, &known(), &admin).exports,
            Listed::Failed(LoadFailure::NoAnswer)
        );
        let model = scene.model(with_exports(Vec::new()));
        assert_eq!(
            exports(scene.server_id, &model, &known(), &admin).exports,
            Listed::Loaded(Vec::new())
        );
    }

    #[test]
    fn an_export_waits_is_written_or_is_ready_and_only_a_ready_one_with_a_file_can_be_saved() {
        let scene = scene();
        let place = scene.in_general();
        let waiting = export(place, ExportState::Waiting);
        let running = export(place, ExportState::Running);
        let finished = ready(place);
        // The server states the size with the state "ready". Without it nothing is saved.
        let sizeless = Export {
            bytes: None,
            ..ready(place)
        };
        let model = scene.model(with_exports(vec![
            finished.clone(),
            running.clone(),
            waiting.clone(),
            sizeless.clone(),
        ]));
        let offset = UtcOffset::from_hms(-3, 0, 0).unwrap();
        let known = Known {
            zone: Zone::Fixed(offset),
            ..Known::default()
        };
        let page = exports(scene.server_id, &model, &known, &AdminState::default());
        let rows = page.exports.loaded().unwrap();
        assert_eq!(
            rows.iter()
                .map(|row| (row.id, row.status, row.can_save, row.bytes))
                .collect::<Vec<_>>(),
            [
                (finished.id, ExportStatus::Ready, true, Some(2_000_000)),
                (running.id, ExportStatus::Running, false, None),
                (waiting.id, ExportStatus::Waiting, false, None),
                (sizeless.id, ExportStatus::Ready, false, None),
            ]
        );
        assert_eq!(
            rows[0].of,
            ExportOf::Channel {
                name: "general".to_owned(),
                kind: ChannelKind::Text,
                workspace: Some("Studio".to_owned()),
            }
        );
        assert_eq!(rows[0].requested, time());
        assert_eq!(rows[0].requested.offset(), offset);
        assert_eq!(rows[0].requested.hour(), 5);
        assert_eq!(rows[0].expires, Some(time() + Duration::days(7)));
        assert_eq!(rows[0].save, None);
        assert_eq!(rows[1].expires, None);
        assert_eq!(txt::export_of(&rows[0].of), "# general · Studio");
    }

    #[test]
    fn a_failed_export_states_each_reason_of_the_server() {
        let scene = scene();
        let reasons = [
            ExportFailure::StorageLow,
            ExportFailure::AccessEnded,
            ExportFailure::Interrupted,
            ExportFailure::Failed,
        ];
        let failed = |failure| Export {
            failure,
            finished_at: Some(time()),
            expires_at: Some(time() + Duration::days(7)),
            ..export(scene.in_general(), ExportState::Failed)
        };
        let mut listed: Vec<Export> = reasons.iter().map(|reason| failed(Some(*reason))).collect();
        // A failed export without a reason is shown with the general one.
        listed.push(failed(None));
        let model = scene.model(with_exports(listed));
        let page = exports(scene.server_id, &model, &known(), &AdminState::default());
        let rows = page.exports.loaded().unwrap();
        assert_eq!(
            rows.iter().map(|row| row.status).collect::<Vec<_>>(),
            [
                ExportStatus::Failed(ExportFailure::StorageLow),
                ExportStatus::Failed(ExportFailure::AccessEnded),
                ExportStatus::Failed(ExportFailure::Interrupted),
                ExportStatus::Failed(ExportFailure::Failed),
                ExportStatus::Failed(ExportFailure::Failed),
            ]
        );
        assert!(rows.iter().all(|row| !row.can_save && row.bytes.is_none()));
        // Each reason has its own statement.
        let mut statements: Vec<&str> = reasons
            .iter()
            .map(|reason| txt::export_status(ExportStatus::Failed(*reason)))
            .collect();
        assert!(statements.iter().all(|text| text.starts_with("Failed: ")));
        statements.sort_unstable();
        statements.dedup();
        assert_eq!(statements.len(), reasons.len());
    }

    #[test]
    fn an_export_names_an_archived_channel_and_states_a_channel_that_the_account_does_not_see() {
        let scene = scene();
        let archived = channel(scene.studio, "old-plans");
        let of_archived = ready(MessagePlace::Channel {
            workspace_id: scene.studio,
            channel_id: archived.id,
        });
        let of_gone = ready(MessagePlace::Channel {
            workspace_id: scene.studio,
            channel_id: ChannelId::generate(),
        });
        let listed = vec![of_archived, of_gone];
        // The archived list of the workspace is not read: the state does not know the
        // archived channel.
        let model = scene.model(with_exports(listed.clone()));
        let page = exports(scene.server_id, &model, &known(), &AdminState::default());
        let rows = page.exports.loaded().unwrap();
        assert_eq!(rows[0].of, ExportOf::ChannelGone);
        assert_eq!(rows[1].of, ExportOf::ChannelGone);
        assert_eq!(
            txt::export_of(&rows[1].of),
            "A channel that is not in your channel lists"
        );

        let model = scene.model(AdminModel {
            exports: Collection::Loaded(listed),
            archived: vec![(
                scene.studio,
                Collection::Loaded(vec![ArchivedChannel {
                    channel: archived,
                    archived_at: time(),
                    deletes_at: None,
                }]),
            )],
            ..AdminModel::default()
        });
        let page = exports(scene.server_id, &model, &known(), &AdminState::default());
        let rows = page.exports.loaded().unwrap();
        assert_eq!(
            rows[0].of,
            ExportOf::Channel {
                name: "old-plans".to_owned(),
                kind: ChannelKind::Text,
                workspace: Some("Studio".to_owned()),
            }
        );
        assert_eq!(rows[1].of, ExportOf::ChannelGone);
        // A channel that is gone can still be saved and removed.
        assert!(rows[1].can_save);
    }

    #[test]
    fn an_export_of_a_direct_conversation_names_the_other_account_when_the_state_holds_its_name() {
        let scene = scene();
        let (ana, stranger) = (AccountId::generate(), AccountId::generate());
        let direct = |low_account_id, high_account_id| {
            export(
                MessagePlace::Direct {
                    conversation_id: ConversationId::generate(),
                    low_account_id,
                    high_account_id,
                },
                ExportState::Waiting,
            )
        };
        let model = scene.model(with_exports(vec![
            direct(scene.own, ana),
            direct(ana, scene.own),
            direct(scene.own, stranger),
        ]));
        let mut known = known();
        known.people.read_name(scene.server_id, ana, "Ana");
        let page = exports(scene.server_id, &model, &known, &AdminState::default());
        let rows = page.exports.loaded().unwrap();
        let named = ExportOf::Direct {
            other: Some("Ana".to_owned()),
        };
        assert_eq!(rows[0].of, named);
        assert_eq!(rows[1].of, named);
        assert_eq!(rows[2].of, ExportOf::Direct { other: None });
        assert_eq!(txt::export_of(&named), "Direct conversation with Ana");
        assert_eq!(txt::export_of(&rows[2].of), "A direct conversation");
    }

    #[test]
    fn an_export_carries_the_progress_the_path_and_the_failure_of_its_file() {
        let scene = scene();
        let place = scene.in_general();
        let (arriving, saved, failed, untouched) =
            (ready(place), ready(place), ready(place), ready(place));
        let model = scene.model(with_exports(vec![
            arriving.clone(),
            saved.clone(),
            failed.clone(),
            untouched.clone(),
        ]));
        let path = PathBuf::from("/downloads/junto-export-general-2027-01-15.zip");
        let files = HashMap::from([
            (
                (scene.server_id, arriving.id),
                ExportSave::Saving { written: 500_000 },
            ),
            ((scene.server_id, saved.id), ExportSave::Saved(path.clone())),
            (
                (scene.server_id, failed.id),
                ExportSave::Failed(txt::SAVE_ENDED.to_owned()),
            ),
            // The file of an export of another server is not the file of this row.
            (
                (ServerId::generate(), untouched.id),
                ExportSave::Saving { written: 1 },
            ),
        ]);
        let admin = AdminState {
            exports: ExportFiles {
                files,
                ..ExportFiles::default()
            },
            ..AdminState::default()
        };
        let page = exports(scene.server_id, &model, &known(), &admin);
        let rows = page.exports.loaded().unwrap();
        assert_eq!(rows[0].save, Some(ExportSave::Saving { written: 500_000 }));
        // No second save of a file starts while one runs.
        assert!(!rows[0].can_save);
        assert_eq!(rows[1].save, Some(ExportSave::Saved(path.clone())));
        assert!(rows[1].can_save);
        assert_eq!(
            rows[2].save,
            Some(ExportSave::Failed(txt::SAVE_ENDED.to_owned()))
        );
        assert!(rows[2].can_save);
        assert_eq!(rows[3].save, None);
        assert!(rows[3].can_save);
        assert_eq!(
            txt::save_progress(500_000, rows[0].bytes),
            "Saving the file: 25% (500 KB of 2.0 MB)"
        );
        assert_eq!(
            txt::saved_as(&path),
            "Saved as /downloads/junto-export-general-2027-01-15.zip"
        );
    }

    #[test]
    fn the_file_of_an_export_is_named_after_its_channel_and_the_day_of_its_request() {
        let day = time().date();
        let of_channel = |name: &str| ExportOf::Channel {
            name: name.to_owned(),
            kind: ChannelKind::Text,
            workspace: None,
        };
        assert_eq!(
            export_file_name(&of_channel("general"), day),
            "junto-export-general-2027-01-15.zip"
        );
        // A character that a file name cannot hold becomes an underscore.
        assert_eq!(
            export_file_name(&of_channel("a/b c:d\\e"), day),
            "junto-export-a_b_c_d_e-2027-01-15.zip"
        );
        assert_eq!(
            export_file_name(&of_channel("../../etc"), day),
            "junto-export-______etc-2027-01-15.zip"
        );
        assert_eq!(
            export_file_name(&of_channel("Überblick-2"), day),
            "junto-export-Überblick-2-2027-01-15.zip"
        );
        assert_eq!(
            export_file_name(&ExportOf::ChannelGone, day),
            "junto-export-channel-2027-01-15.zip"
        );
        // The name of the other account is not part of the name of the file.
        let direct = ExportOf::Direct {
            other: Some("Ana".to_owned()),
        };
        assert_eq!(
            export_file_name(&direct, day),
            "junto-export-conversation-2027-01-15.zip"
        );
    }

    #[test]
    fn the_scopes_of_the_audit_log_follow_the_permissions_of_the_person() {
        let scopes = |scene: &Scene, shown: Option<WorkspaceId>| {
            let model = scene.model(AdminModel::default());
            let workspace = shown.and_then(|id| model.workspace(id));
            let page = audit(
                scene.server_id,
                &model,
                workspace,
                &known(),
                &AdminState::default(),
            );
            assert_eq!(page.state, AuditRead::NotRead);
            assert_eq!(page.scope, page.scopes.first().map(|choice| choice.scope));
            page.scopes
        };
        let server = ScopeChoice {
            scope: AuditScope::Server,
            workspace: None,
        };
        let studio = |scene: &Scene| ScopeChoice {
            scope: AuditScope::Workspace(scene.studio),
            workspace: Some("Studio".to_owned()),
        };

        // Without a permission the person reads no log.
        let plain = scene();
        assert!(scopes(&plain, Some(plain.studio)).is_empty());

        let mut reader = scene();
        reader.snapshot.account.server_permissions = vec![ServerPermission::ViewServerAuditLog];
        assert_eq!(
            scopes(&reader, Some(reader.studio)),
            std::slice::from_ref(&server)
        );

        // A role of the workspace gives the entries of that workspace only.
        let lead = scene().leading_with(vec![WorkspacePermission::ViewAuditEntries]);
        assert_eq!(scopes(&lead, Some(lead.studio)), [studio(&lead)]);
        assert!(scopes(&lead, Some(lead.lab)).is_empty());
        assert!(scopes(&lead, None).is_empty());
        let other = scene().leading_with(vec![WorkspacePermission::ManageChannel]);
        assert!(scopes(&other, Some(other.studio)).is_empty());

        // An administrator reads both, the server first.
        let administrator = scene().administrator();
        assert_eq!(
            scopes(&administrator, Some(administrator.studio)),
            [server.clone(), studio(&administrator)]
        );
        assert_eq!(scopes(&administrator, None), [server]);
    }

    #[test]
    fn the_audit_log_is_not_read_then_read_then_loaded_and_a_failed_read_keeps_the_entries() {
        let scene = scene().administrator();
        let model = scene.model(AdminModel::default());
        let page = |admin: &AdminState| {
            audit(
                scene.server_id,
                &model,
                model.workspace(scene.studio),
                &known(),
                admin,
            )
        };
        let shown = page(&AdminState::default());
        assert_eq!(shown.state, AuditRead::NotRead);
        assert_eq!(shown.scope, Some(AuditScope::Server));
        assert_eq!((shown.loaded, shown.rows.len()), (0, 0));
        assert!(!shown.older);

        // The first read is on its way.
        let mut admin = log(scene.server_id, AuditScope::Server, Vec::new());
        admin.audit.reading = true;
        assert_eq!(page(&admin).state, AuditRead::Reading);

        // The log has no entry.
        admin.audit.reading = false;
        let shown = page(&admin);
        assert_eq!(shown.state, AuditRead::Loaded);
        assert_eq!(shown.loaded, 0);

        let removal = entry(
            Some(scene.own),
            AuditEvent::AccountRemoved {
                reason: None,
                account_id: AccountId::generate(),
            },
        );
        admin.audit.entries = vec![removal.clone()];
        admin.audit.next_before = Some(removal.id);
        let shown = page(&admin);
        assert_eq!(shown.state, AuditRead::Loaded);
        assert_eq!((shown.loaded, shown.rows.len()), (1, 1));
        assert_eq!(shown.rows[0].id, removal.id);
        assert!(shown.older);

        // The page after the loaded entries is on its way.
        admin.audit.reading = true;
        let shown = page(&admin);
        assert_eq!(shown.state, AuditRead::Reading);
        assert_eq!(shown.rows.len(), 1);

        // A failed read keeps what is loaded.
        admin.audit.reading = false;
        admin.audit.failed = Some(AuditFailure {
            sentence: "No answer of the server arrived.".to_owned(),
            older: true,
        });
        let shown = page(&admin);
        assert_eq!(
            shown.state,
            AuditRead::Failed {
                sentence: "No answer of the server arrived.".to_owned(),
                older: true,
            }
        );
        assert_eq!(shown.rows.len(), 1);

        // After the last page the server holds no older entry.
        admin.audit.failed = None;
        admin.audit.next_before = None;
        assert!(!page(&admin).older);
    }

    #[test]
    fn the_entries_of_another_log_are_not_shown() {
        let scene = scene().administrator();
        let model = scene.model(AdminModel::default());
        let entries = || vec![entry(None, AuditEvent::EventEpochRenewed)];
        let page = |admin: &AdminState, shown: Option<WorkspaceId>| {
            audit(
                scene.server_id,
                &model,
                shown.and_then(|id| model.workspace(id)),
                &known(),
                admin,
            )
        };
        // The log of the workspace that the person chose.
        let mut admin = log(
            scene.server_id,
            AuditScope::Workspace(scene.studio),
            entries(),
        );
        admin.audit.filter.actor = Some(AccountId::generate());
        let shown = page(&admin, Some(scene.studio));
        assert_eq!(shown.scope, Some(AuditScope::Workspace(scene.studio)));
        assert_eq!(shown.state, AuditRead::Loaded);
        assert_eq!(shown.loaded, 1);
        assert!(shown.filter.actor.is_some());

        // The window shows another workspace: the page shows the first scope and none of
        // the entries, and the person of the filter belongs to the log before.
        let shown = page(&admin, Some(scene.lab));
        assert_eq!(shown.scope, Some(AuditScope::Server));
        assert_eq!(shown.state, AuditRead::NotRead);
        assert_eq!((shown.loaded, shown.rows.len()), (0, 0));
        assert_eq!(shown.filter, AuditFilter::default());
        assert!(shown.people.is_empty());

        // The log of another server.
        let admin = log(ServerId::generate(), AuditScope::Server, entries());
        let shown = page(&admin, Some(scene.studio));
        assert_eq!(shown.scope, Some(AuditScope::Server));
        assert_eq!(shown.state, AuditRead::NotRead);
        assert_eq!(shown.loaded, 0);

        // A log that the person may not read any more.
        let mut reader = self::scene();
        reader.snapshot.account.server_permissions = vec![ServerPermission::ViewServerAuditLog];
        let model = reader.model(AdminModel::default());
        let admin = log(
            reader.server_id,
            AuditScope::Workspace(reader.studio),
            entries(),
        );
        let shown = audit(
            reader.server_id,
            &model,
            model.workspace(reader.studio),
            &known(),
            &admin,
        );
        assert_eq!(shown.scope, Some(AuditScope::Server));
        assert_eq!(shown.state, AuditRead::NotRead);
        assert!(shown.rows.is_empty());
    }

    #[test]
    fn every_action_of_the_protocol_has_a_sentence_with_the_known_names_and_without_an_identifier()
    {
        let scene = scene();
        let ana = AccountId::generate();
        let moderators = ServerRoleId::generate();
        let model = scene.model(AdminModel {
            server_roles: Collection::Loaded(vec![ServerRole {
                position: 1,
                id: moderators,
                name: "Moderators".to_owned(),
                permissions: Vec::new(),
            }]),
            ..AdminModel::default()
        });
        let mut known = known();
        known.people.read_name(scene.server_id, ana, "Ana");
        let names = Names::new(scene.server_id, &model, &known);
        // A state that knows none of the accounts, workspaces, channels and roles.
        let stranger = self::scene();
        let empty = stranger.model(AdminModel::default());
        let blank = self::known();
        let nothing = Names::new(stranger.server_id, &empty, &blank);
        assert!(holds_identifier(&format!("the account {ana} was removed")));

        let actions = every_action(&scene, ana, moderators);
        let mut places = Vec::new();
        for (event, named) in &actions {
            places.push(place(event));
            let held = entry(Some(ana), event.clone());
            let line = audit_line(&held, AuditScope::Server, &names);
            assert!(!line.sentence.trim().is_empty(), "{event:?}");
            assert!(!holds_identifier(&line.sentence), "{}", line.sentence);
            for name in named {
                assert!(
                    line.sentence.contains(*name),
                    "“{}” does not hold “{name}”",
                    line.sentence
                );
            }
            assert!(
                line.sentence.chars().next().is_some_and(char::is_uppercase),
                "{}",
                line.sentence
            );
            assert_eq!(line.group, AuditGroup::of(event));

            let unnamed = audit_line(&held, AuditScope::Server, &nothing);
            assert!(!unnamed.sentence.trim().is_empty(), "{event:?}");
            assert!(!holds_identifier(&unnamed.sentence), "{}", unnamed.sentence);
            assert_eq!(
                unnamed.actor,
                Actor::Account {
                    id: ana,
                    name: None
                }
            );
        }
        // The list holds each action of the protocol once.
        places.sort_unstable();
        assert_eq!(places, (0..67).collect::<Vec<usize>>());

        // The filter by action offers each action of the protocol once, by the name that
        // the server matches, in the group of the action.
        let mut offered: Vec<&str> = ACTIONS.iter().map(|(name, _)| *name).collect();
        let mut named: Vec<&str> = actions
            .iter()
            .filter(|(event, _)| *event != AuditEvent::Unknown)
            .map(|(event, _)| event.action())
            .collect();
        for (event, _) in &actions {
            if *event != AuditEvent::Unknown {
                assert_eq!(
                    group_of_action(event.action()),
                    Some(AuditGroup::of(event)),
                    "{event:?}"
                );
            }
        }
        offered.sort_unstable();
        named.sort_unstable();
        assert_eq!(offered, named);
        assert_eq!(group_of_action("unknown"), None);
        assert_eq!(group_of_action("glossary_updated"), None);
        for group in AuditGroup::ALL {
            assert!(group.actions().next().is_some(), "{group:?}");
            assert!(
                group
                    .actions()
                    .all(|action| group_of_action(action) == Some(group))
            );
        }
        assert_eq!(txt::audit_action("account_banned"), "Account banned");
        assert_eq!(
            txt::audit_action("workspace_member_removed"),
            "Workspace member removed"
        );
        assert_eq!(
            txt::audit_sentence(&AuditEvent::ServerLogoChanged { logo: None }, &names),
            "The logo of the server was removed."
        );

        let removal = AuditEvent::AccountRemoved {
            reason: None,
            account_id: ana,
        };
        assert_eq!(
            txt::audit_sentence(&removal, &names),
            "The account “Ana” was removed from the server."
        );
        assert_eq!(
            txt::audit_sentence(&removal, &nothing),
            "An account was removed from the server."
        );
        assert_eq!(
            txt::audit_sentence(&AuditEvent::Unknown, &names),
            "An action that this version of Junto does not know"
        );
        // An entry of a revoked invite from before the server stored the creator.
        let revoked = |invite_created_by| AuditEvent::InviteRevoked {
            invite_id: InviteId::generate(),
            invite_created_by,
        };
        assert_eq!(
            txt::audit_sentence(&revoked(Some(ana)), &names),
            "An invite of the account “Ana” was revoked."
        );
        assert_eq!(
            txt::audit_sentence(&revoked(None), &names),
            "An invite was revoked."
        );
    }

    #[test]
    fn every_group_of_the_filter_holds_an_action() {
        let scene = scene();
        let actions = every_action(&scene, AccountId::generate(), ServerRoleId::generate());
        for group in AuditGroup::ALL {
            assert!(
                actions
                    .iter()
                    .any(|(event, _)| AuditGroup::of(event) == group),
                "{group:?}"
            );
        }
        let of = |event: AuditEvent| AuditGroup::of(&event);
        let account_id = AccountId::generate();
        assert_eq!(
            of(AuditEvent::AccountBanned {
                reason: None,
                account_id,
                cancelled_invite_ids: Vec::new(),
            }),
            AuditGroup::Members
        );
        assert_eq!(
            of(AuditEvent::JoinRequestDenied {
                join_request_id: JoinRequestId::generate(),
            }),
            AuditGroup::Invites
        );
        assert_eq!(
            of(AuditEvent::ChannelOverrideSet {
                role_id: scene.leads,
                allow: Vec::new(),
                deny: Vec::new(),
            }),
            AuditGroup::Roles
        );
        assert_eq!(of(AuditEvent::ChannelArchived), AuditGroup::Structure);
        assert_eq!(
            of(AuditEvent::CallMemberDisconnected { account_id }),
            AuditGroup::Calls
        );
        assert_eq!(
            of(AuditEvent::RetentionChanged { days: Some(30) }),
            AuditGroup::Messages
        );
        assert_eq!(of(AuditEvent::Unknown), AuditGroup::Server);
    }

    #[test]
    fn the_page_shows_every_loaded_entry_and_the_filters_that_the_server_applied() {
        let scene = scene().administrator();
        let (ana, bea, zoe, eli) = (
            AccountId::generate(),
            AccountId::generate(),
            AccountId::generate(),
            AccountId::generate(),
        );
        let account = |id: AccountId, name: &str| AccountSummary {
            id,
            name: name.to_owned(),
            status: AccountStatus::Active,
            is_owner: false,
            server_role_ids: Vec::new(),
            created_at: time(),
        };
        let mut known = known();
        known.people.read_name(scene.server_id, ana, "Ana");
        let ban = entry(
            Some(ana),
            AuditEvent::AccountBanned {
                reason: None,
                account_id: zoe,
                cancelled_invite_ids: Vec::new(),
            },
        );
        let removal = entry(
            Some(bea),
            AuditEvent::AccountRemoved {
                reason: None,
                account_id: zoe,
            },
        );
        let reset = entry(None, AuditEvent::EventEpochRenewed);
        let mut admin = log(
            scene.server_id,
            AuditScope::Server,
            vec![ban.clone(), removal.clone(), reset.clone()],
        );
        admin.audit.people = vec![ana, bea];
        let model = scene.model(AdminModel::default());
        let page = audit(scene.server_id, &model, None, &known, &admin);
        // The server applied the filters: the page hides no loaded entry.
        let ids: Vec<_> = page.rows.iter().map(|row| row.id).collect();
        assert_eq!(ids, [ban.id, removal.id, reset.id]);
        assert_eq!((page.loaded, page.filter), (3, AuditFilter::default()));
        assert!(!page.filter.is_set());
        // The accounts that acted in a page that was read, by name, and after them the
        // ones without a known name.
        assert_eq!(
            page.people,
            [
                ActorChoice {
                    id: ana,
                    name: Some("Ana".to_owned()),
                },
                ActorChoice {
                    id: bea,
                    name: None,
                },
            ]
        );
        assert_eq!(txt::audit_count(3), "3 entries loaded");
        assert_eq!(txt::audit_count(1), "1 entry loaded");

        // The filters of the log are the ones of the page. An account of a filter stays
        // in the list also when no entry names it, so that the filter can be seen and
        // taken away.
        admin.audit.filter = AuditFilter {
            action: Some("account_banned"),
            actor: Some(ana),
            about: Some(zoe),
            period: AuditPeriod::Week,
        };
        admin.audit.entries = vec![ban.clone()];
        let page = audit(scene.server_id, &model, None, &known, &admin);
        assert_eq!(page.filter, admin.audit.filter);
        assert!(page.filter.is_set());
        assert_eq!(page.rows.len(), 1);
        let offered: Vec<AccountId> = page.people.iter().map(|choice| choice.id).collect();
        assert_eq!(offered.len(), 3);
        assert_eq!(offered[0], ana);
        assert!(offered.contains(&bea) && offered.contains(&zoe));

        // The accounts of the server, for a person who lists them, are offered too.
        let model = scene.model(AdminModel {
            accounts: Collection::Loaded(vec![account(eli, "Eli"), account(zoe, "Zoe")]),
            ..AdminModel::default()
        });
        let page = audit(scene.server_id, &model, None, &known, &admin);
        let names: Vec<Option<&str>> = page
            .people
            .iter()
            .map(|choice| choice.name.as_deref())
            .collect();
        assert_eq!(names, [Some("Ana"), Some("Eli"), Some("Zoe"), None]);

        // The filters and their people belong to the log that was read.
        let other = log(ServerId::generate(), AuditScope::Server, vec![ban]);
        let page = audit(scene.server_id, &model, None, &known, &other);
        assert_eq!(page.filter, AuditFilter::default());
        assert!(page.people.is_empty() && page.rows.is_empty());
    }

    #[test]
    fn a_ban_and_a_removal_state_their_reason_without_control_characters() {
        let scene = scene().administrator();
        let model = scene.model(AdminModel::default());
        let known = known();
        let names = Names::new(scene.server_id, &model, &known);
        let zoe = AccountId::generate();
        let line = |event: AuditEvent| audit_line(&entry(None, event), AuditScope::Server, &names);
        let banned = |reason: Option<&str>| AuditEvent::AccountBanned {
            reason: reason.map(str::to_owned),
            account_id: zoe,
            cancelled_invite_ids: Vec::new(),
        };
        assert_eq!(line(banned(None)).reason, None);
        assert_eq!(
            line(banned(Some("Spam in #general"))).reason.as_deref(),
            Some("Spam in #general")
        );
        // The reason is text of another person.
        assert_eq!(
            line(banned(Some(" two\nlines\u{7} "))).reason.as_deref(),
            Some("twolines")
        );
        assert_eq!(line(banned(Some(" \n "))).reason, None);
        let long = "a".repeat(1500);
        let cut = line(banned(Some(long.as_str()))).reason.unwrap();
        assert_eq!(cut.chars().count(), 1000);
        assert_eq!(
            line(AuditEvent::AccountRemoved {
                reason: Some("Left the team".to_owned()),
                account_id: zoe,
            })
            .reason
            .as_deref(),
            Some("Left the team")
        );
        assert_eq!(
            line(AuditEvent::WorkspaceMemberRemoved {
                account_id: zoe,
                reason: Some("Project ended".to_owned()),
            })
            .reason
            .as_deref(),
            Some("Project ended")
        );
        // The sentence of the action does not repeat the reason.
        let stated = line(banned(Some("Spam in #general")));
        assert!(!stated.sentence.contains("Spam"));
        assert_eq!(txt::audit_reason("Spam"), "Reason: Spam");
        // No other action states one.
        assert_eq!(line(AuditEvent::EventEpochRenewed).reason, None);
    }

    #[test]
    fn an_entry_states_an_access_token_the_server_as_its_actor_and_what_it_is_about() {
        let scene = scene().administrator();
        let model = scene.model(AdminModel::default());
        let ana = AccountId::generate();
        let mut known = known();
        known.people.read_name(scene.server_id, ana, "Ana");
        let mut with_token = entry(
            Some(ana),
            AuditEvent::WorkspaceCreated {
                workspace_id: scene.lab,
                name: "Lab".to_owned(),
            },
        );
        with_token.access_token_id = Some(AccessTokenId::generate());
        let mut by_server = entry(
            None,
            AuditEvent::RecordingStopped {
                recording_id: RecordingId::generate(),
                reason: RecordingStopReason::CallEnded,
            },
        );
        by_server.workspace_id = Some(scene.studio);
        by_server.channel_id = Some(scene.general);
        // 23:30 UTC is the next day two hours east of it.
        by_server.occurred_at = time() + Duration::minutes(15 * 60 + 30);
        let mut about_gone = entry(Some(AccountId::generate()), AuditEvent::ChannelArchived);
        about_gone.workspace_id = Some(WorkspaceId::generate());
        about_gone.channel_id = Some(ChannelId::generate());
        let entries = vec![with_token, by_server, about_gone];

        let admin = log(scene.server_id, AuditScope::Server, entries.clone());
        let page = audit(scene.server_id, &model, None, &known, &admin);
        let rows = &page.rows;
        assert_eq!(
            rows[0].actor,
            Actor::Account {
                id: ana,
                name: Some("Ana".to_owned()),
            }
        );
        assert!(rows[0].with_token);
        assert_eq!((&rows[0].workspace, &rows[0].channel), (&None, &None));
        assert_eq!(
            txt::audit_head(&rows[0]),
            "15 January 2027, 08:00 · Ana with an access token"
        );
        assert_eq!(rows[0].sentence, "The workspace “Lab” was created.");

        assert_eq!(rows[1].actor, Actor::Server);
        assert!(!rows[1].with_token);
        assert_eq!(rows[1].workspace, Some(About::Named("Studio".to_owned())));
        assert_eq!(rows[1].channel, Some(About::Named("# general".to_owned())));
        assert_eq!(rows[1].group, AuditGroup::Calls);

        // What the state does not hold is stated without its identifier.
        assert_eq!(rows[2].workspace, Some(About::Unknown));
        assert_eq!(rows[2].channel, Some(About::Unknown));
        assert!(matches!(rows[2].actor, Actor::Account { name: None, .. }));
        let head = txt::audit_head(&rows[2]);
        assert_eq!(
            head,
            "15 January 2027, 08:00 · An account · A workspace · A channel"
        );
        assert!(!holds_identifier(&head));

        // The time is the one of the zone of the person.
        let east = Known {
            zone: Zone::Fixed(UtcOffset::from_hms(2, 0, 0).unwrap()),
            ..Known::default()
        };
        let page = audit(scene.server_id, &model, None, &east, &admin);
        assert_eq!(
            txt::audit_head(&page.rows[1]),
            "16 January 2027, 01:30 · The server · Studio · # general"
        );

        // The log of a workspace does not repeat its workspace in every entry.
        let admin = log(
            scene.server_id,
            AuditScope::Workspace(scene.studio),
            entries,
        );
        let page = audit(
            scene.server_id,
            &model,
            model.workspace(scene.studio),
            &known,
            &admin,
        );
        assert_eq!(page.rows[1].workspace, None);
        assert_eq!(
            page.rows[1].channel,
            Some(About::Named("# general".to_owned()))
        );
    }

    #[test]
    fn the_page_keeps_the_newest_2000_entries_and_reads_no_older_one() {
        let scene = scene().administrator();
        let model = scene.model(AdminModel::default());
        let entries: Vec<AuditEntry> = (0..MOST_AUDIT_ENTRIES)
            .map(|_| entry(None, AuditEvent::EventEpochRenewed))
            .collect();
        let mut admin = log(scene.server_id, AuditScope::Server, entries);
        // The server holds older entries.
        admin.audit.next_before = Some(AuditEntryId::generate());
        let page = audit(scene.server_id, &model, None, &known(), &admin);
        assert!(page.at_bound);
        assert!(!page.older);
        assert_eq!((page.loaded, page.rows.len()), (2000, 2000));
        assert_eq!(MOST_AUDIT_ENTRIES, 2000);
        assert!(txt::AUDIT_BOUND.contains("the newest 2,000 entries"));
        assert_eq!(AUDIT_PAGE, 50);

        // One entry below the bound the page reads the next page on request.
        admin.audit.entries.pop();
        let page = audit(scene.server_id, &model, None, &known(), &admin);
        assert!(!page.at_bound);
        assert!(page.older);
        assert_eq!(page.loaded, 1999);
    }
}
