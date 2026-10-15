//! The commands of the administration: workspaces, categories, channels, overrides,
//! members, accounts, invites, access tokens, exports, the audit log, the archive period,
//! retention and the logo of the server (ADR 0101, ADR 0106, ADR 0210, ADR 0304, ADR 0305,
//! ADR 0308, ADR 0410, ADR 0411).

use std::fmt::Display;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use futures_util::StreamExt as _;
use junto_client_core::api::{AvatarImage, BodyStream, ServerApi, Statement};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::access_token::{
    AccessTokenInfo, AccessTokenQuery, CreateAccessToken, TokenAction,
};
use junto_client_core::protocol::account::{AccountSummary, Avatar};
use junto_client_core::protocol::audit::{AuditEntry, AuditEvent, AuditFilterQuery, AuditPage};
use junto_client_core::protocol::call::UpdateCallSettings;
use junto_client_core::protocol::discovery::Retention;
use junto_client_core::protocol::export::Export;
use junto_client_core::protocol::ids::{
    AccessTokenId, AccountId, CategoryId, ChannelId, ConversationId, ExportId, InviteId,
    JoinRequestId, WorkspaceId, WorkspaceRoleId,
};
use junto_client_core::protocol::invite::{Invite, JoinRequestStatus, Redemption};
use junto_client_core::protocol::message::MessagePlace;
use junto_client_core::protocol::secret::InviteCode;
use junto_client_core::protocol::structure::{
    ArchivePeriod, ArchivedChannel, Category, ChangeChannelOverride, Channel, ChannelKind,
    ChannelOverride, CreateCategory, CreateChannel, OverrideState, SetChannelOverride,
    UpdateCategory, UpdateChannel, Workspace, WorkspaceMember,
};
use time::OffsetDateTime;

use crate::{Arguments, Client, Failure, end_session, failed, logged_in, shown, usage};

enum Action {
    Workspaces,
    NewWorkspace(String),
    RenameWorkspace(WorkspaceId, String),
    DeleteWorkspace(WorkspaceId),
    NewCategory(WorkspaceId, CreateCategory),
    ChangeCategory(WorkspaceId, CategoryId, UpdateCategory),
    DeleteCategory(WorkspaceId, CategoryId),
    Channels(WorkspaceId),
    NewChannel(WorkspaceId, CreateChannel),
    ChangeChannel(ChannelId, UpdateChannel),
    DeleteChannel(ChannelId),
    ArchiveChannel(ChannelId),
    RestoreChannel(ChannelId),
    ArchivedChannels(WorkspaceId),
    CallSettings(ChannelId, UpdateCallSettings),
    Overrides(ChannelId),
    SetOverride(ChannelId, WorkspaceRoleId, SetChannelOverride),
    ChangeOverride(ChannelId, WorkspaceRoleId, ChangeChannelOverride),
    Members(WorkspaceId),
    AddMember(WorkspaceId, AccountId),
    RemoveMember(WorkspaceId, AccountId, Statement),
    Accounts,
    RemoveAccount(AccountId, Statement),
    BanAccount(AccountId, Statement),
    UnbanAccount(AccountId),
    Invites,
    RevokeInvite(InviteId),
    DenyRequest(JoinRequestId),
    Tokens(AccessTokenQuery),
    NewToken {
        name: String,
        actions: Vec<TokenAction>,
        days: u32,
    },
    RevokeToken(AccessTokenId),
    Exports,
    ExportChannel(ChannelId),
    ExportConversation(ConversationId),
    ShowExport(ExportId),
    DownloadExport(ExportId, PathBuf),
    RemoveExport(ExportId),
    ServerAudit(AuditFilterQuery),
    WorkspaceAudit(WorkspaceId, AuditFilterQuery),
    ShowArchivePeriod(WorkspaceId),
    SetArchivePeriod(WorkspaceId, ArchivePeriod),
    ShowRetention(WorkspaceId),
    SetFileRetention(WorkspaceId, Retention),
    SetRecordingRetention(WorkspaceId, Retention),
    ShowDirectRetention,
    SetDirectRetention(Retention),
    ShowLogo,
    SetLogo(Vec<u8>),
    RemoveLogo,
}

pub(crate) fn word<'a>(
    words: &'a [String],
    position: usize,
    what: &str,
) -> Result<&'a str, Failure> {
    words
        .get(position)
        .map(String::as_str)
        .ok_or_else(|| usage(format!("{what} is missing")))
}

pub(crate) fn identifier<T: FromStr>(
    words: &[String],
    position: usize,
    what: &str,
) -> Result<T, Failure> {
    word(words, position, what)?
        .parse()
        .map_err(|_| usage(format!("{what} is not an identifier")))
}

/// The identifier that an option names.
fn named<T: FromStr>(arguments: &Arguments, name: &str, what: &str) -> Result<Option<T>, Failure> {
    arguments
        .value(name)
        .map(|text| {
            text.parse()
                .map_err(|_| usage(format!("--{name} needs {what}")))
        })
        .transpose()
}

pub(crate) fn position(text: &str) -> Result<i32, Failure> {
    text.parse()
        .map_err(|_| usage("a position is a whole number"))
}

/// A number of days, or `never` for no time at all.
fn days(text: &str) -> Result<Option<u32>, Failure> {
    if text == "never" {
        return Ok(None);
    }
    text.parse()
        .map(Some)
        .map_err(|_| usage("the time is a number of days or `never`"))
}

fn on_off(arguments: &Arguments, name: &str) -> Result<Option<bool>, Failure> {
    match arguments.value(name) {
        Some("on") => Ok(Some(true)),
        Some("off") => Ok(Some(false)),
        Some(_) => Err(usage(format!("--{name} takes on or off"))),
        None => Ok(None),
    }
}

fn kind(arguments: &Arguments) -> Result<ChannelKind, Failure> {
    match arguments.value("kind") {
        Some("text") => Ok(ChannelKind::Text),
        Some("voice") => Ok(ChannelKind::Voice),
        Some(_) => Err(usage("--kind takes text or voice")),
        None => Err(usage("--kind <text|voice> is missing")),
    }
}

/// A permission of a workspace by the name that the server writes.
pub(crate) fn permission(text: &str) -> Result<WorkspacePermission, Failure> {
    WorkspacePermission::ALL
        .iter()
        .find(|held| held.name() == text)
        .copied()
        .ok_or_else(|| usage(format!("{text} is no permission of a workspace")))
}

fn permissions(arguments: &Arguments, name: &str) -> Result<Vec<WorkspacePermission>, Failure> {
    arguments
        .values(name)
        .iter()
        .map(|text| permission(text))
        .collect()
}

/// What `overrides change` does with one permission of an override.
fn override_state(text: &str) -> Result<OverrideState, Failure> {
    match text {
        "allow" => Ok(OverrideState::Allow),
        "deny" => Ok(OverrideState::Deny),
        "inherit" => Ok(OverrideState::Inherit),
        _ => Err(usage("a permission takes allow, deny or inherit")),
    }
}

/// The reason and the message of a removal or of a ban. The server keeps the reason in the
/// audit entry, and gives the message to the person concerned (ADR 0120).
fn statement(arguments: &Arguments) -> Statement {
    Statement {
        reason: arguments.value("reason").map(str::to_owned),
        message: arguments.value("message").map(str::to_owned),
    }
}

/// An action of an access token by the name that the server writes.
fn token_action(text: &str) -> Result<TokenAction, Failure> {
    TokenAction::ALL
        .iter()
        .find(|held| held.name() == text)
        .copied()
        .ok_or_else(|| usage(format!("{text} is no action of an access token")))
}

/// The options of `channels update`. An option that is absent keeps the value.
fn channel_change(arguments: &Arguments) -> Result<UpdateChannel, Failure> {
    if arguments.value("category").is_some() && arguments.flag("no-category") {
        return Err(usage("--category and --no-category exclude each other"));
    }
    let category_id = match named::<CategoryId>(arguments, "category", "a category")? {
        Some(category_id) => Some(Some(category_id)),
        None if arguments.flag("no-category") => Some(None),
        None => None,
    };
    let change = UpdateChannel {
        name: arguments.value("name").map(str::to_owned),
        category_id,
        position: arguments.value("position").map(position).transpose()?,
        permanent_messages: on_off(arguments, "permanent")?,
    };
    if change == UpdateChannel::default() {
        return Err(usage("channels update needs at least one option"));
    }
    Ok(change)
}

/// The options of `channels call-settings`. An option that is absent keeps the value.
fn call_change(arguments: &Arguments) -> Result<UpdateCallSettings, Failure> {
    if arguments.value("limit").is_some() && arguments.flag("no-limit") {
        return Err(usage("--limit and --no-limit exclude each other"));
    }
    let user_limit = match arguments.number("limit")? {
        Some(limit) => Some(Some(limit)),
        None if arguments.flag("no-limit") => Some(None),
        None => None,
    };
    let change = UpdateCallSettings {
        user_limit,
        hides_occupants: on_off(arguments, "hide")?,
    };
    if change == UpdateCallSettings::default() {
        return Err(usage("channels call-settings needs at least one option"));
    }
    Ok(change)
}

/// The filters and the page of `audit`. An option that is absent filters nothing.
fn audit_query(arguments: &Arguments) -> Result<AuditFilterQuery, Failure> {
    Ok(AuditFilterQuery {
        action: arguments.value("action").map(str::to_owned),
        actor_id: named(arguments, "actor", "an account")?,
        target_id: named(arguments, "target", "an identifier")?,
        since: crate::discovery::time_of(arguments, "since")?,
        until: crate::discovery::time_of(arguments, "until")?,
        before: named(arguments, "before", "an audit entry")?,
        limit: arguments.number("limit")?,
    })
}

fn workspaces(action: &str, words: &[String]) -> Result<Option<Action>, Failure> {
    let workspace = || identifier::<WorkspaceId>(words, 1, "the workspace");
    Ok(Some(match action {
        "list" => Action::Workspaces,
        "create" => Action::NewWorkspace(word(words, 1, "the name")?.to_owned()),
        "rename" => Action::RenameWorkspace(workspace()?, word(words, 2, "the name")?.to_owned()),
        "delete" => Action::DeleteWorkspace(workspace()?),
        _ => return Ok(None),
    }))
}

fn categories(action: &str, words: &[String]) -> Result<Option<Action>, Failure> {
    let workspace = || identifier::<WorkspaceId>(words, 1, "the workspace");
    let category = || identifier::<CategoryId>(words, 2, "the category");
    Ok(Some(match action {
        "create" => Action::NewCategory(
            workspace()?,
            CreateCategory {
                name: word(words, 2, "the name")?.to_owned(),
                position: None,
            },
        ),
        "rename" => Action::ChangeCategory(
            workspace()?,
            category()?,
            UpdateCategory {
                name: Some(word(words, 3, "the name")?.to_owned()),
                position: None,
            },
        ),
        "move" => Action::ChangeCategory(
            workspace()?,
            category()?,
            UpdateCategory {
                name: None,
                position: Some(position(word(words, 3, "the position")?)?),
            },
        ),
        "delete" => Action::DeleteCategory(workspace()?, category()?),
        _ => return Ok(None),
    }))
}

fn channels(
    action: &str,
    words: &[String],
    arguments: &Arguments,
) -> Result<Option<Action>, Failure> {
    let workspace = || identifier::<WorkspaceId>(words, 1, "the workspace");
    let channel = || identifier::<ChannelId>(words, 1, "the channel");
    Ok(Some(match action {
        "list" => Action::Channels(workspace()?),
        "create" => Action::NewChannel(
            workspace()?,
            CreateChannel {
                name: word(words, 2, "the name")?.to_owned(),
                kind: kind(arguments)?,
                category_id: named(arguments, "category", "a category")?,
                position: None,
            },
        ),
        "update" => Action::ChangeChannel(channel()?, channel_change(arguments)?),
        "delete" => Action::DeleteChannel(channel()?),
        "archive" => Action::ArchiveChannel(channel()?),
        "restore" => Action::RestoreChannel(channel()?),
        "archived" => Action::ArchivedChannels(workspace()?),
        "call-settings" => Action::CallSettings(channel()?, call_change(arguments)?),
        _ => return Ok(None),
    }))
}

fn overrides(
    action: &str,
    words: &[String],
    arguments: &Arguments,
) -> Result<Option<Action>, Failure> {
    let channel = || identifier::<ChannelId>(words, 1, "the channel");
    Ok(Some(match action {
        "list" => Action::Overrides(channel()?),
        "set" => Action::SetOverride(
            channel()?,
            identifier(words, 2, "the role")?,
            SetChannelOverride {
                allow: permissions(arguments, "allow")?,
                deny: permissions(arguments, "deny")?,
            },
        ),
        "change" => Action::ChangeOverride(
            channel()?,
            identifier(words, 2, "the role")?,
            ChangeChannelOverride {
                permission: permission(word(words, 3, "the permission")?)?,
                state: override_state(word(words, 4, "allow, deny or inherit")?)?,
            },
        ),
        _ => return Ok(None),
    }))
}

fn members(
    action: &str,
    words: &[String],
    arguments: &Arguments,
) -> Result<Option<Action>, Failure> {
    let workspace = || identifier::<WorkspaceId>(words, 1, "the workspace");
    let account = || identifier::<AccountId>(words, 2, "the account");
    Ok(Some(match action {
        "list" => Action::Members(workspace()?),
        "add" => Action::AddMember(workspace()?, account()?),
        "remove" => Action::RemoveMember(workspace()?, account()?, statement(arguments)),
        _ => return Ok(None),
    }))
}

fn accounts(
    action: &str,
    words: &[String],
    arguments: &Arguments,
) -> Result<Option<Action>, Failure> {
    let account = || identifier::<AccountId>(words, 1, "the account");
    Ok(Some(match action {
        "list" => Action::Accounts,
        "remove" => Action::RemoveAccount(account()?, statement(arguments)),
        "ban" => Action::BanAccount(account()?, statement(arguments)),
        "unban" => Action::UnbanAccount(account()?),
        _ => return Ok(None),
    }))
}

fn invites(action: &str, words: &[String]) -> Result<Option<Action>, Failure> {
    Ok(Some(match action {
        "list" => Action::Invites,
        "revoke" => Action::RevokeInvite(identifier(words, 1, "the invite")?),
        _ => return Ok(None),
    }))
}

fn tokens(
    action: &str,
    words: &[String],
    arguments: &Arguments,
) -> Result<Option<Action>, Failure> {
    Ok(Some(match action {
        "list" => Action::Tokens(AccessTokenQuery {
            before: named(arguments, "before", "an access token")?,
            limit: arguments.number("limit")?,
        }),
        "create" => {
            let name = word(words, 1, "the name of the token")?.to_owned();
            let actions = arguments
                .values("action")
                .iter()
                .map(|text| token_action(text))
                .collect::<Result<Vec<_>, _>>()?;
            if actions.is_empty() {
                return Err(usage("--action <action> is missing"));
            }
            let days = arguments
                .number("days")?
                .ok_or_else(|| usage("--days <n> is missing"))?;
            Action::NewToken {
                name,
                actions,
                days,
            }
        }
        "revoke" => Action::RevokeToken(identifier(words, 1, "the access token")?),
        _ => return Ok(None),
    }))
}

fn exports(action: &str, words: &[String]) -> Result<Option<Action>, Failure> {
    let export = || identifier::<ExportId>(words, 1, "the export");
    Ok(Some(match action {
        "list" => Action::Exports,
        "channel" => Action::ExportChannel(identifier(words, 1, "the channel")?),
        "conversation" => Action::ExportConversation(identifier(words, 1, "the conversation")?),
        "show" => Action::ShowExport(export()?),
        "download" => Action::DownloadExport(export()?, PathBuf::from(word(words, 2, "the file")?)),
        "remove" => Action::RemoveExport(export()?),
        _ => return Ok(None),
    }))
}

fn audit(action: &str, words: &[String], arguments: &Arguments) -> Result<Option<Action>, Failure> {
    Ok(Some(match action {
        "server" => Action::ServerAudit(audit_query(arguments)?),
        "workspace" => Action::WorkspaceAudit(
            identifier(words, 1, "the workspace")?,
            audit_query(arguments)?,
        ),
        _ => return Ok(None),
    }))
}

fn archive_period(action: &str, words: &[String]) -> Result<Option<Action>, Failure> {
    let workspace = || identifier::<WorkspaceId>(words, 1, "the workspace");
    Ok(Some(match action {
        "show" => Action::ShowArchivePeriod(workspace()?),
        "set" => Action::SetArchivePeriod(
            workspace()?,
            ArchivePeriod {
                days: days(word(words, 2, "the time")?)?,
            },
        ),
        _ => return Ok(None),
    }))
}

fn retention(action: &str, words: &[String]) -> Result<Option<Action>, Failure> {
    let workspace = || identifier::<WorkspaceId>(words, 1, "the workspace");
    let kept = |at: usize| -> Result<Retention, Failure> {
        Ok(Retention {
            days: days(word(words, at, "the time")?)?,
        })
    };
    Ok(Some(match action {
        "show" => Action::ShowRetention(workspace()?),
        "files" => Action::SetFileRetention(workspace()?, kept(2)?),
        "recordings" => Action::SetRecordingRetention(workspace()?, kept(2)?),
        "direct" => match word(words, 1, "the action of retention direct")? {
            "show" => Action::ShowDirectRetention,
            "set" => Action::SetDirectRetention(kept(2)?),
            other => {
                return Err(usage(format!(
                    "unknown action of retention direct: {other}"
                )));
            }
        },
        _ => return Ok(None),
    }))
}

fn logo(action: &str, words: &[String]) -> Result<Option<Action>, Failure> {
    Ok(Some(match action {
        "show" => Action::ShowLogo,
        // The file is read before the server is contacted.
        "set" => Action::SetLogo(crate::read_image(word(words, 1, "the file")?)?),
        "remove" => Action::RemoveLogo,
        _ => return Ok(None),
    }))
}

/// Reads the action of one command of the administration from `words`: the name of the
/// action and its arguments.
fn parse(command: &str, words: &[String], arguments: &Arguments) -> Result<Action, Failure> {
    if command == "deny-request" {
        return Ok(Action::DenyRequest(identifier(
            words,
            0,
            "the join request",
        )?));
    }
    let action = word(words, 0, &format!("the action of {command}"))?;
    let parsed = match command {
        "workspaces" => workspaces(action, words)?,
        "categories" => categories(action, words)?,
        "channels" => channels(action, words, arguments)?,
        "overrides" => overrides(action, words, arguments)?,
        "members" => members(action, words, arguments)?,
        "accounts" => accounts(action, words, arguments)?,
        "invites" => invites(action, words)?,
        "tokens" => tokens(action, words, arguments)?,
        "exports" => exports(action, words)?,
        "audit" => audit(action, words, arguments)?,
        "archive-period" => archive_period(action, words)?,
        "retention" => retention(action, words)?,
        "logo" => logo(action, words)?,
        other => return Err(usage(format!("unknown command {other}"))),
    };
    parsed.ok_or_else(|| usage(format!("unknown action of {command}: {action}")))
}

/// A value of the protocol as the server writes it.
fn wire<T: serde::Serialize>(value: &T) -> String {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::String(name)) => name,
        _ => "unknown".to_owned(),
    }
}

pub(crate) fn listed<T: Display>(items: &[T]) -> String {
    if items.is_empty() {
        return "none".to_owned();
    }
    let items: Vec<String> = items.iter().map(ToString::to_string).collect();
    items.join(",")
}

pub(crate) fn names(permissions: &[WorkspacePermission]) -> String {
    let names: Vec<&str> = permissions.iter().map(|held| held.name()).collect();
    listed(&names)
}

fn pair(name: &str, id: impl Display) -> String {
    format!("{name}={id}")
}

/// The reason of a removal or of a ban in quotation marks, when the entry has one.
fn reason_pair(reason: Option<&str>) -> Option<String> {
    reason.map(|reason| format!("reason={:?}", shown(reason)))
}

/// The message to the person of a removal or of a ban in quotation marks (ADR 0120).
fn message_pair(message: Option<&str>) -> Option<String> {
    message.map(|message| format!("message={:?}", shown(message)))
}

fn days_text(days: Option<u32>) -> String {
    match days {
        Some(1) => "1 day".to_owned(),
        Some(days) => format!("{days} days"),
        None => "never".to_owned(),
    }
}

fn workspace_line(workspace: &Workspace) -> String {
    let default = if workspace.is_default {
        "  default"
    } else {
        ""
    };
    format!(
        "workspace: {}{default}  {}",
        workspace.id,
        shown(&workspace.name)
    )
}

fn category_line(category: &Category) -> String {
    format!(
        "category: {}  position {}  {}",
        category.id,
        category.position,
        shown(&category.name)
    )
}

fn channel_line(channel: &Channel) -> String {
    let kind = match channel.kind {
        ChannelKind::Text => "text",
        ChannelKind::Voice => "voice",
    };
    let category = match channel.category_id {
        Some(category_id) => category_id.to_string(),
        None => "none".to_owned(),
    };
    let mut marks = String::new();
    if channel.permanent_messages {
        marks.push_str("  permanent");
    }
    if let Some(limit) = channel.user_limit {
        marks.push_str(&format!("  limit {limit}"));
    }
    if channel.hides_occupants {
        marks.push_str("  hides who is inside");
    }
    format!(
        "channel: {}  {kind}  position {}  category {category}{marks}  {}",
        channel.id,
        channel.position,
        shown(&channel.name)
    )
}

fn archived_line(archived: &ArchivedChannel) -> String {
    let deletion = match archived.deletes_at {
        Some(at) => format!("deleted after {at}"),
        None => "never deleted".to_owned(),
    };
    format!(
        "archived: {}  archived at {}  {deletion}  {}",
        archived.channel.id,
        archived.archived_at,
        shown(&archived.channel.name)
    )
}

fn override_line(held: &ChannelOverride) -> String {
    format!(
        "override: role {}  allow {}  deny {}",
        held.role_id,
        names(&held.allow),
        names(&held.deny)
    )
}

fn member_line(member: &WorkspaceMember) -> String {
    format!(
        "member: {}  roles {}  {}",
        member.account_id,
        listed(&member.role_ids),
        shown(&member.name)
    )
}

fn account_line(account: &AccountSummary) -> String {
    let owner = if account.is_owner { "  owner" } else { "" };
    format!(
        "account: {}  {}{owner}  roles {}  {}",
        account.id,
        wire(&account.status),
        listed(&account.server_role_ids),
        shown(&account.name)
    )
}

fn invite_line(invite: &Invite) -> String {
    let revocation = match &invite.revocation {
        Some(revocation) => wire(revocation),
        None => "not revoked".to_owned(),
    };
    let creator = match &invite.created_by_name {
        Some(name) => format!("  creator {}", shown(name)),
        None => String::new(),
    };
    format!(
        "invite: {}  by {}  uses {} of {}  expires {}  workspaces {}  {revocation}{creator}",
        invite.id,
        invite.created_by,
        invite.uses,
        invite.max_uses,
        invite.expires_at,
        listed(&invite.workspace_ids)
    )
}

fn request_line(status: &JoinRequestStatus) -> String {
    format!("request {}: {}", status.id, wire(&status.state))
}

fn redemption_line(redemption: &Redemption) -> String {
    format!("workspaces: {}", listed(&redemption.workspace_ids))
}

fn token_line(token: &AccessTokenInfo) -> String {
    let actions: Vec<&str> = token.actions.iter().map(|held| held.name()).collect();
    format!(
        "token: {}  by {}  actions {}  expires {}  {}",
        token.id,
        token.created_by,
        listed(&actions),
        token.expires_at,
        shown(&token.name)
    )
}

fn export_line(export: &Export) -> String {
    let place = match export.place {
        MessagePlace::Channel { channel_id, .. } => format!("channel:{channel_id}"),
        MessagePlace::Direct {
            conversation_id, ..
        } => format!("direct:{conversation_id}"),
    };
    let mut marks = String::new();
    if let Some(failure) = &export.failure {
        marks.push_str(&format!("  failure {}", wire(failure)));
    }
    if let Some(bytes) = export.bytes {
        marks.push_str(&format!("  {bytes} bytes"));
    }
    if let Some(at) = export.expires_at {
        marks.push_str(&format!("  removed after {at}"));
    }
    format!(
        "export: {}  {place}  {}  requested {}{marks}",
        export.id,
        wire(&export.state),
        export.requested_at
    )
}

/// The identifiers that an action of the audit log names.
fn audit_details(event: &AuditEvent) -> Vec<String> {
    use AuditEvent as E;
    match event {
        E::OwnerClaimed { account_id }
        | E::AccountUnbanned { account_id }
        | E::WorkspaceMemberAdded { account_id }
        | E::CallMicrophoneBlocked { account_id }
        | E::CallMicrophoneUnblocked { account_id }
        | E::CallSoundBlocked { account_id }
        | E::CallSoundUnblocked { account_id }
        | E::CallMemberDisconnected { account_id }
        | E::CallMemberMovedOut { account_id }
        | E::CallMemberMovedIn { account_id } => vec![pair("account", account_id)],
        E::AccountRemoved {
            account_id,
            reason,
            recipient_message,
        }
        | E::WorkspaceMemberRemoved {
            account_id,
            reason,
            recipient_message,
            ..
        } => {
            let mut pairs = vec![pair("account", account_id)];
            pairs.extend(reason_pair(reason.as_deref()));
            pairs.extend(message_pair(recipient_message.as_deref()));
            pairs
        }
        E::OwnerAssigned {
            account_id,
            previous_owner_id,
        } => {
            let mut pairs = vec![pair("account", account_id)];
            if let Some(previous) = previous_owner_id {
                pairs.push(pair("previous_owner", previous));
            }
            pairs
        }
        E::OwnerAccessRestored {
            account_id,
            device_id,
        }
        | E::AccountRecovered {
            account_id,
            device_id,
        } => vec![pair("account", account_id), pair("device", device_id)],
        E::InviteCreated {
            invite_id,
            workspace_ids,
            ..
        } => vec![
            pair("invite", invite_id),
            pair("workspaces", listed(workspace_ids)),
        ],
        E::InviteWorkspacesChanged {
            invite_id,
            invite_created_by,
            removed_workspace_id,
        } => vec![
            pair("invite", invite_id),
            pair("invite_created_by", invite_created_by),
            pair("removed_workspace", removed_workspace_id),
        ],
        E::InviteRevoked {
            invite_id,
            invite_created_by,
        } => {
            let mut pairs = vec![pair("invite", invite_id)];
            pairs.extend(invite_created_by.map(|by| pair("invite_created_by", by)));
            pairs
        }
        E::AccountJoined {
            account_id,
            invite_id,
            invite_created_by,
        } => vec![
            pair("account", account_id),
            pair("invite", invite_id),
            pair("invite_created_by", invite_created_by),
        ],
        E::InviteRedeemed {
            account_id,
            invite_id,
            invite_created_by,
            workspace_ids,
        } => vec![
            pair("account", account_id),
            pair("invite", invite_id),
            pair("invite_created_by", invite_created_by),
            pair("workspaces", listed(workspace_ids)),
        ],
        E::JoinRequestApproved {
            join_request_id,
            account_id,
            workspace_ids,
        } => vec![
            pair("request", join_request_id),
            pair("account", account_id),
            pair("workspaces", listed(workspace_ids)),
        ],
        E::JoinRequestDenied { join_request_id } => vec![pair("request", join_request_id)],
        E::AccountBanned {
            reason,
            recipient_message,
            account_id,
            cancelled_invite_ids,
            ..
        } => {
            let mut pairs = vec![
                pair("account", account_id),
                pair("cancelled_invites", listed(cancelled_invite_ids)),
            ];
            pairs.extend(reason_pair(reason.as_deref()));
            pairs.extend(message_pair(recipient_message.as_deref()));
            pairs
        }
        E::ServerRoleCreated { role_id, .. }
        | E::ServerRoleUpdated { role_id, .. }
        | E::ServerRoleDeleted { role_id, .. } => vec![pair("role", role_id)],
        E::ServerRolePositionChanged { role_id, position } => {
            vec![pair("role", role_id), pair("position", position)]
        }
        E::WorkspaceRolePositionChanged { role_id, position } => {
            vec![pair("role", role_id), pair("position", position)]
        }
        E::ServerRoleAssigned {
            role_id,
            account_id,
        }
        | E::ServerRoleUnassigned {
            role_id,
            account_id,
        } => vec![pair("role", role_id), pair("account", account_id)],
        E::AccessTokenCreated {
            access_token_id, ..
        } => vec![pair("token", access_token_id)],
        E::AccessTokenRevoked {
            access_token_id,
            created_by,
        } => vec![
            pair("token", access_token_id),
            pair("created_by", created_by),
        ],
        E::WorkspaceCreated { workspace_id, .. }
        | E::WorkspaceRenamed { workspace_id, .. }
        | E::WorkspaceDeleted { workspace_id, .. } => vec![pair("workspace", workspace_id)],
        E::WorkspaceRoleCreated { role_id, .. }
        | E::WorkspaceRoleUpdated { role_id, .. }
        | E::WorkspaceRoleDeleted { role_id, .. }
        | E::ChannelOverrideSet { role_id, .. } => vec![pair("role", role_id)],
        E::WorkspaceRoleAssigned {
            role_id,
            account_id,
        }
        | E::WorkspaceRoleUnassigned {
            role_id,
            account_id,
        } => vec![pair("role", role_id), pair("account", account_id)],
        E::CategoryCreated { category_id, .. }
        | E::CategoryUpdated { category_id, .. }
        | E::CategoryDeleted { category_id, .. } => vec![pair("category", category_id)],
        E::ChannelUpdated { category_id, .. } => category_id
            .iter()
            .map(|category_id| pair("category", category_id))
            .collect(),
        E::RecordingStarted { recording_id }
        | E::RecordingStopped { recording_id, .. }
        | E::RecordingRemoved { recording_id }
        | E::TranscriptRequested { recording_id } => vec![pair("recording", recording_id)],
        E::PermanentMessageDeleted { message_id } => vec![pair("message", message_id)],
        E::ExportRequested { export_id } => vec![pair("export", export_id)],
        E::ServerLogoChanged { logo } => {
            let hash = match logo {
                Some(logo) => shown(&logo.hash),
                None => "none".to_owned(),
            };
            vec![pair("logo", hash)]
        }
        E::OwnerCodeIssued { .. }
        | E::EventEpochRenewed
        | E::ChannelCreated { .. }
        | E::ChannelDeleted { .. }
        | E::ChannelCallSettingsUpdated { .. }
        | E::GlossaryReplaced { .. }
        | E::AttachmentLimitsChanged { .. }
        | E::NotificationDefaultChanged { .. }
        | E::NotificationDefaultRemoved
        | E::RecordingRetentionChanged { .. }
        | E::RetentionChanged { .. }
        | E::ChannelArchived
        | E::ChannelRestored
        | E::ArchivePeriodChanged { .. }
        | E::Unknown => Vec::new(),
    }
}

/// One entry of the audit log: its time, who acted, the action, the access token, the
/// workspace and the channel of the entry, and the identifiers that the action names.
fn audit_line(entry: &AuditEntry) -> String {
    let actor = match entry.actor_id {
        Some(account_id) => account_id.to_string(),
        None => "server".to_owned(),
    };
    let mut pairs = Vec::new();
    if let Some(access_token_id) = entry.access_token_id {
        pairs.push(pair("with_token", access_token_id));
    }
    if let Some(workspace_id) = entry.workspace_id {
        pairs.push(pair("workspace", workspace_id));
    }
    if let Some(channel_id) = entry.channel_id {
        pairs.push(pair("channel", channel_id));
    }
    for detail in audit_details(&entry.event) {
        if !pairs.contains(&detail) {
            pairs.push(detail);
        }
    }
    let line = format!(
        "audit: {}  {}  {actor}  {}  {}",
        entry.id,
        entry.occurred_at,
        entry.event.action(),
        pairs.join(" ")
    );
    line.trim_end().to_owned()
}

fn audit_lines(page: &AuditPage) -> Vec<String> {
    let mut lines: Vec<String> = page.entries.iter().map(audit_line).collect();
    if let Some(next) = page.next_before {
        lines.push(format!("continue with: --before {next}"));
    }
    lines
}

fn period_line(period: ArchivePeriod) -> String {
    format!("archive period: {}", days_text(period.days))
}

/// `what` is the kind of the content that the server removes by its age.
fn retention_line(what: &str, retention: &Retention) -> String {
    format!("{what} retention: {}", days_text(retention.days))
}

/// The logo as the server describes it after it stored the image.
fn stored_logo_line(logo: &Avatar) -> String {
    format!(
        "logo: {}  {} bytes  {}x{}  {}",
        logo.media_type.media_type(),
        logo.bytes,
        logo.width,
        logo.height,
        shown(&logo.hash)
    )
}

/// The logo as the tool read it: the type that its first bytes state, its size and the
/// hash that the tool computed.
fn logo_line(logo: Option<&AvatarImage>) -> String {
    match logo {
        Some(logo) => format!(
            "logo: {}  {} bytes  {}",
            logo.media_type.media_type(),
            logo.bytes.len(),
            logo.hash
        ),
        None => "logo: none".to_owned(),
    }
}

/// Writes the body of a file of `bytes` bytes into `file`. A body that is longer or
/// shorter than that fails. No byte after the stated size is written, so that a server
/// cannot fill the disk.
async fn write_body(
    mut body: BodyStream,
    bytes: u64,
    file: &mut std::fs::File,
) -> Result<(), Failure> {
    let mut received = 0u64;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(failed)?;
        if chunk.len() as u64 > bytes - received {
            return Err(Failure::Failed(format!(
                "the file is longer than the {bytes} bytes of the export"
            )));
        }
        file.write_all(&chunk).map_err(failed)?;
        received += chunk.len() as u64;
    }
    if received < bytes {
        return Err(Failure::Failed(format!(
            "the file ended after {received} of {bytes} bytes"
        )));
    }
    file.flush().map_err(failed)
}

/// Receives the file of an export of `bytes` bytes into `file`.
async fn receive(
    api: &ServerApi,
    export_id: ExportId,
    bytes: u64,
    file: &mut std::fs::File,
) -> Result<(), Failure> {
    let body = api.export_file(export_id, 0).await.map_err(failed)?.body;
    write_body(body, bytes, file).await
}

/// Writes the file of an export to a new file and returns the number of its bytes. A file
/// that exists is not replaced. The server ends the answer when the access of the account
/// to the export ends (ADR 0411), so a file that does not have the size of the export is
/// removed.
async fn download(api: &ServerApi, export_id: ExportId, path: &Path) -> Result<u64, Failure> {
    let export = api.export(export_id).await.map_err(failed)?;
    let Some(bytes) = export.bytes else {
        return Err(Failure::Failed("the export has no file".to_owned()));
    };
    let mut file = std::fs::File::create_new(path)
        .map_err(|error| Failure::Failed(format!("{}: {error}", path.display())))?;
    let received = receive(api, export_id, bytes, &mut file).await;
    drop(file);
    if received.is_err() {
        let _ = std::fs::remove_file(path);
    }
    received.map(|()| bytes)
}

async fn perform(api: &ServerApi, action: Action) -> Result<Vec<String>, Failure> {
    let done = |()| vec!["done".to_owned()];
    Ok(match action {
        Action::Workspaces => api
            .workspaces()
            .await
            .map_err(failed)?
            .iter()
            .map(workspace_line)
            .collect(),
        Action::NewWorkspace(name) => {
            vec![workspace_line(
                &api.create_workspace(name).await.map_err(failed)?,
            )]
        }
        Action::RenameWorkspace(workspace_id, name) => vec![workspace_line(
            &api.rename_workspace(workspace_id, name)
                .await
                .map_err(failed)?,
        )],
        Action::DeleteWorkspace(workspace_id) => {
            done(api.delete_workspace(workspace_id).await.map_err(failed)?)
        }
        Action::NewCategory(workspace_id, category) => vec![category_line(
            &api.create_category(workspace_id, &category)
                .await
                .map_err(failed)?,
        )],
        Action::ChangeCategory(workspace_id, category_id, change) => vec![category_line(
            &api.update_category(workspace_id, category_id, &change)
                .await
                .map_err(failed)?,
        )],
        Action::DeleteCategory(workspace_id, category_id) => done(
            api.delete_category(workspace_id, category_id)
                .await
                .map_err(failed)?,
        ),
        Action::Channels(workspace_id) => {
            let list = api.channels(workspace_id).await.map_err(failed)?;
            let categories = list.categories.iter().map(category_line);
            categories
                .chain(list.channels.iter().map(channel_line))
                .collect()
        }
        Action::NewChannel(workspace_id, channel) => vec![channel_line(
            &api.create_channel(workspace_id, &channel)
                .await
                .map_err(failed)?,
        )],
        Action::ChangeChannel(channel_id, change) => vec![channel_line(
            &api.update_channel(channel_id, &change)
                .await
                .map_err(failed)?,
        )],
        Action::DeleteChannel(channel_id) => {
            done(api.delete_channel(channel_id).await.map_err(failed)?)
        }
        Action::ArchiveChannel(channel_id) => vec![archived_line(
            &api.archive_channel(channel_id).await.map_err(failed)?,
        )],
        Action::RestoreChannel(channel_id) => vec![channel_line(
            &api.restore_channel(channel_id).await.map_err(failed)?,
        )],
        Action::ArchivedChannels(workspace_id) => api
            .archived_channels(workspace_id)
            .await
            .map_err(failed)?
            .iter()
            .map(archived_line)
            .collect(),
        Action::CallSettings(channel_id, change) => vec![channel_line(
            &api.update_call_settings(channel_id, &change)
                .await
                .map_err(failed)?,
        )],
        Action::Overrides(channel_id) => api
            .channel_overrides(channel_id)
            .await
            .map_err(failed)?
            .iter()
            .map(override_line)
            .collect(),
        Action::SetOverride(channel_id, role_id, sets) => vec![override_line(
            &api.set_channel_override(channel_id, role_id, &sets)
                .await
                .map_err(failed)?,
        )],
        Action::ChangeOverride(channel_id, role_id, change) => vec![override_line(
            &api.change_channel_override(channel_id, role_id, change.permission, change.state)
                .await
                .map_err(failed)?,
        )],
        Action::Members(workspace_id) => api
            .workspace_members(workspace_id)
            .await
            .map_err(failed)?
            .iter()
            .map(member_line)
            .collect(),
        Action::AddMember(workspace_id, account_id) => done(
            api.add_workspace_member(workspace_id, account_id)
                .await
                .map_err(failed)?,
        ),
        Action::RemoveMember(workspace_id, account_id, statement) => done(
            api.remove_workspace_member(workspace_id, account_id, &statement)
                .await
                .map_err(failed)?,
        ),
        Action::Accounts => api
            .accounts()
            .await
            .map_err(failed)?
            .iter()
            .map(account_line)
            .collect(),
        Action::RemoveAccount(account_id, statement) => done(
            api.remove_account_stating(account_id, &statement)
                .await
                .map_err(failed)?,
        ),
        Action::BanAccount(account_id, statement) => done(
            api.ban_account_stating(account_id, &statement)
                .await
                .map_err(failed)?,
        ),
        Action::UnbanAccount(account_id) => {
            done(api.unban_account(account_id).await.map_err(failed)?)
        }
        Action::Invites => api
            .invites()
            .await
            .map_err(failed)?
            .iter()
            .map(invite_line)
            .collect(),
        Action::RevokeInvite(invite_id) => {
            done(api.revoke_invite(invite_id).await.map_err(failed)?)
        }
        Action::DenyRequest(join_request_id) => vec![request_line(
            &api.deny_join_request(join_request_id)
                .await
                .map_err(failed)?,
        )],
        Action::Tokens(query) => {
            let page = api.access_tokens(&query).await.map_err(failed)?;
            let mut lines: Vec<String> = page.access_tokens.iter().map(token_line).collect();
            if let Some(next) = page.next_before {
                lines.push(format!("continue with: --before {next}"));
            }
            lines
        }
        Action::NewToken {
            name,
            actions,
            days,
        } => {
            let token = CreateAccessToken {
                name,
                actions,
                expires_at: OffsetDateTime::now_utc() + time::Duration::days(days.into()),
            };
            let created = api.create_access_token(&token).await.map_err(failed)?;
            vec![
                token_line(&created.access_token),
                shown(created.token.expose()),
                "the line above is the token; it is not shown again".to_owned(),
            ]
        }
        Action::RevokeToken(access_token_id) => done(
            api.revoke_access_token(access_token_id)
                .await
                .map_err(failed)?,
        ),
        Action::Exports => api
            .exports()
            .await
            .map_err(failed)?
            .iter()
            .map(export_line)
            .collect(),
        Action::ExportChannel(channel_id) => vec![export_line(
            &api.export_channel(channel_id).await.map_err(failed)?,
        )],
        Action::ExportConversation(conversation_id) => vec![export_line(
            &api.export_conversation(conversation_id)
                .await
                .map_err(failed)?,
        )],
        Action::ShowExport(export_id) => {
            vec![export_line(&api.export(export_id).await.map_err(failed)?)]
        }
        Action::DownloadExport(export_id, path) => {
            let bytes = download(api, export_id, &path).await?;
            vec![format!("saved: {}  {bytes} bytes", path.display())]
        }
        Action::RemoveExport(export_id) => {
            done(api.remove_export(export_id).await.map_err(failed)?)
        }
        Action::ServerAudit(query) => {
            audit_lines(&api.server_audit_log(&query).await.map_err(failed)?)
        }
        Action::WorkspaceAudit(workspace_id, query) => audit_lines(
            &api.workspace_audit_log(workspace_id, &query)
                .await
                .map_err(failed)?,
        ),
        Action::ShowArchivePeriod(workspace_id) => vec![period_line(
            api.archive_period(workspace_id).await.map_err(failed)?,
        )],
        Action::SetArchivePeriod(workspace_id, period) => vec![period_line(
            api.set_archive_period(workspace_id, period)
                .await
                .map_err(failed)?,
        )],
        Action::ShowRetention(workspace_id) => vec![
            retention_line(
                "file",
                &api.file_retention(workspace_id).await.map_err(failed)?,
            ),
            retention_line(
                "recording",
                &api.recording_retention(workspace_id)
                    .await
                    .map_err(failed)?,
            ),
        ],
        Action::SetFileRetention(workspace_id, retention) => vec![retention_line(
            "file",
            &api.set_file_retention(workspace_id, &retention)
                .await
                .map_err(failed)?,
        )],
        Action::SetRecordingRetention(workspace_id, retention) => vec![retention_line(
            "recording",
            &api.set_recording_retention(workspace_id, &retention)
                .await
                .map_err(failed)?,
        )],
        Action::ShowDirectRetention => vec![retention_line(
            "direct message file",
            &api.direct_message_retention().await.map_err(failed)?,
        )],
        Action::SetDirectRetention(retention) => vec![retention_line(
            "direct message file",
            &api.set_direct_message_retention(&retention)
                .await
                .map_err(failed)?,
        )],
        Action::ShowLogo => {
            let logo = api.server_logo().await.map_err(failed)?;
            vec![logo_line(logo.as_ref())]
        }
        Action::SetLogo(image) => vec![stored_logo_line(
            &api.set_server_logo(image).await.map_err(failed)?,
        )],
        Action::RemoveLogo => done(api.remove_server_logo().await.map_err(failed)?),
    })
}

/// `redeem`: enters the workspaces of an invite with the account of this device.
async fn redeem(client: &Client, arguments: &Arguments) -> Result<(), Failure> {
    // The server is chosen before the code is asked for.
    crate::pick(client, arguments)?;
    let code = InviteCode::new(crate::code("invite code")?.as_str());
    let server = logged_in(client, arguments).await?;
    let redeemed = server.api().redeem_invite(code).await;
    end_session(&server).await;
    crate::live::say(&redemption_line(&redeemed.map_err(failed)?));
    Ok(())
}

/// One command of the administration: one action in one process.
pub(crate) async fn run(
    client: &Client,
    command: &str,
    arguments: &Arguments,
) -> Result<(), Failure> {
    if command == "redeem" {
        return redeem(client, arguments).await;
    }
    let words = arguments.positional.get(1..).unwrap_or_default();
    // An action that the tool does not know is told before the server is contacted.
    let action = parse(command, words, arguments)?;
    let server = logged_in(client, arguments).await?;
    let lines = perform(&server.api(), action).await;
    end_session(&server).await;
    for line in lines? {
        crate::live::say(&line);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek};

    use junto_client_core::api::{TransportError, TransportErrorKind};
    use junto_client_core::protocol::account::{AccountStatus, ImageType};
    use junto_client_core::protocol::export::{ExportFailure, ExportState};
    use junto_client_core::protocol::ids::AuditEntryId;
    use junto_client_core::protocol::invite::{InviteRevocation, JoinRequestState};

    use super::*;

    fn arguments(words: &[&str]) -> Arguments {
        match Arguments::parse(words.iter().map(|word| (*word).to_owned())) {
            Ok(arguments) => arguments,
            Err(_) => panic!("the arguments are not read"),
        }
    }

    /// The action of a command, from the words after the name of the command.
    fn parsed(command: &str, words: &[&str]) -> Result<Action, Failure> {
        let arguments = arguments(words);
        parse(command, &arguments.positional, &arguments)
    }

    fn refused(command: &str, words: &[&str]) -> bool {
        matches!(parsed(command, words), Err(Failure::Usage(_)))
    }

    fn channel(name: &str) -> Channel {
        Channel {
            id: ChannelId::generate(),
            workspace_id: WorkspaceId::generate(),
            category_id: None,
            kind: ChannelKind::Text,
            name: name.to_owned(),
            position: 3,
            permanent_messages: false,
            user_limit: None,
            hides_occupants: false,
        }
    }

    #[test]
    fn a_new_channel_needs_a_kind_that_exists() {
        let workspace = WorkspaceId::generate().to_string();
        let workspace = workspace.as_str();
        let category_id = CategoryId::generate();
        let category = category_id.to_string();
        let Ok(Action::NewChannel(_, voice)) = parsed(
            "channels",
            &["create", workspace, "Daily", "--kind", "voice"],
        ) else {
            panic!("the voice channel is not read");
        };
        assert_eq!(
            (voice.name.as_str(), voice.kind, voice.category_id),
            ("Daily", ChannelKind::Voice, None)
        );
        let Ok(Action::NewChannel(_, text)) = parsed(
            "channels",
            &[
                "create",
                workspace,
                "plans",
                "--kind",
                "text",
                "--category",
                category.as_str(),
            ],
        ) else {
            panic!("the text channel is not read");
        };
        assert_eq!(
            (text.kind, text.category_id),
            (ChannelKind::Text, Some(category_id))
        );
        assert!(refused(
            "channels",
            &["create", workspace, "plans", "--kind", "video"]
        ));
        assert!(refused("channels", &["create", workspace, "plans"]));
        assert!(refused(
            "channels",
            &["create", workspace, "--kind", "text"]
        ));
        assert!(refused("channels", &["create", "plans", "--kind", "text"]));
    }

    #[test]
    fn a_change_of_a_channel_names_a_category_or_no_category_and_not_both() {
        let channel_id = ChannelId::generate().to_string();
        let category_id = CategoryId::generate();
        let category = category_id.to_string();
        let category = category.as_str();
        let change = |options: &[&str]| {
            let mut words = vec!["update", channel_id.as_str()];
            words.extend(options);
            match parsed("channels", &words) {
                Ok(Action::ChangeChannel(_, change)) => Some(change),
                _ => None,
            }
        };
        assert!(change(&["--category", category, "--no-category"]).is_none());
        let Some(moved) = change(&["--category", category]) else {
            panic!("the category is not read");
        };
        assert_eq!(moved.category_id, Some(Some(category_id)));
        assert_eq!((moved.name, moved.position), (None, None));
        let Some(taken_out) = change(&["--no-category"]) else {
            panic!("the absent category is not read");
        };
        assert_eq!(taken_out.category_id, Some(None));
        let Some(other) = change(&["--name", "roadmap", "--position", "-2", "--permanent", "on"])
        else {
            panic!("the other options are not read");
        };
        assert_eq!(
            other,
            UpdateChannel {
                name: Some("roadmap".to_owned()),
                category_id: None,
                position: Some(-2),
                permanent_messages: Some(true),
            }
        );
        assert!(change(&[]).is_none());
        assert!(change(&["--category", "projects"]).is_none());
        assert!(change(&["--permanent", "yes"]).is_none());
        assert!(change(&["--position", "first"]).is_none());
    }

    #[test]
    fn the_call_settings_take_a_limit_or_no_limit_and_not_both() {
        let channel_id = ChannelId::generate().to_string();
        let change = |options: &[&str]| {
            let mut words = vec!["call-settings", channel_id.as_str()];
            words.extend(options);
            match parsed("channels", &words) {
                Ok(Action::CallSettings(_, change)) => Some(change),
                _ => None,
            }
        };
        assert_eq!(
            change(&["--limit", "4", "--hide", "on"]),
            Some(UpdateCallSettings {
                user_limit: Some(Some(4)),
                hides_occupants: Some(true),
            })
        );
        assert_eq!(
            change(&["--no-limit"]),
            Some(UpdateCallSettings {
                user_limit: Some(None),
                hides_occupants: None,
            })
        );
        assert_eq!(change(&["--limit", "4", "--no-limit"]), None);
        assert_eq!(change(&["--hide", "yes"]), None);
        assert_eq!(change(&[]), None);
    }

    #[test]
    fn an_override_takes_permissions_by_the_names_of_the_server() {
        let channel_id = ChannelId::generate().to_string();
        let role_id = WorkspaceRoleId::generate();
        let set = |options: &[&str]| {
            let role = role_id.to_string();
            let mut words = vec!["set", channel_id.as_str(), role.as_str()];
            words.extend(options);
            match parsed("overrides", &words) {
                Ok(Action::SetOverride(_, role_id, sets)) => Some((role_id, sets)),
                _ => None,
            }
        };
        let Some((role, sets)) = set(&[
            "--allow",
            "view_channel",
            "--deny",
            "manage_channel",
            "--deny",
            "lock_room",
        ]) else {
            panic!("the override is not read");
        };
        assert_eq!(role, role_id);
        assert_eq!(sets.allow, [WorkspacePermission::ViewChannel]);
        assert_eq!(
            sets.deny,
            [
                WorkspacePermission::ManageChannel,
                WorkspacePermission::LockRoom
            ]
        );
        // Two empty sets remove the override.
        assert_eq!(
            set(&[]).map(|(_, sets)| sets),
            Some(SetChannelOverride::default())
        );
        assert!(set(&["--allow", "view_channels"]).is_none());
        assert!(set(&["--deny", "ViewChannel"]).is_none());
        assert!(set(&["--deny", "administrator"]).is_none());
        for held in WorkspacePermission::ALL {
            assert!(matches!(permission(held.name()), Ok(read) if read == *held));
        }
    }

    #[test]
    fn a_time_is_a_number_of_days_or_never() {
        assert!(matches!(days("never"), Ok(None)));
        assert!(matches!(days("30"), Ok(Some(30))));
        assert!(days("soon").is_err());
        assert!(days("-1").is_err());
        assert!(days("").is_err());

        let workspace_id = WorkspaceId::generate();
        let workspace = workspace_id.to_string();
        let workspace = workspace.as_str();
        assert!(matches!(
            parsed("archive-period", &["set", workspace, "never"]),
            Ok(Action::SetArchivePeriod(id, ArchivePeriod { days: None })) if id == workspace_id
        ));
        assert!(matches!(
            parsed("archive-period", &["set", workspace, "60"]),
            Ok(Action::SetArchivePeriod(
                _,
                ArchivePeriod { days: Some(60) }
            ))
        ));
        assert!(refused("archive-period", &["set", workspace]));
        assert!(refused("archive-period", &["set", workspace, "always"]));
        assert!(matches!(
            parsed("retention", &["files", workspace, "never"]),
            Ok(Action::SetFileRetention(_, Retention { days: None }))
        ));
        assert!(matches!(
            parsed("retention", &["recordings", workspace, "365"]),
            Ok(Action::SetRecordingRetention(
                _,
                Retention { days: Some(365) }
            ))
        ));
        assert!(matches!(
            parsed("retention", &["direct", "set", "90"]),
            Ok(Action::SetDirectRetention(Retention { days: Some(90) }))
        ));
        assert!(matches!(
            parsed("retention", &["direct", "set", "never"]),
            Ok(Action::SetDirectRetention(Retention { days: None }))
        ));
        assert!(matches!(
            parsed("retention", &["direct", "show"]),
            Ok(Action::ShowDirectRetention)
        ));
        assert!(refused("retention", &["direct", "remove"]));
        assert!(refused("retention", &["direct"]));
    }

    #[test]
    fn a_new_token_needs_a_name_actions_that_exist_and_a_number_of_days() {
        let Ok(Action::NewToken {
            name,
            actions,
            days,
        }) = parsed(
            "tokens",
            &[
                "create",
                "setup",
                "--action",
                "read_structure",
                "--action",
                "manage_members",
                "--days",
                "30",
            ],
        )
        else {
            panic!("the token is not read");
        };
        assert_eq!((name.as_str(), days), ("setup", 30));
        assert_eq!(
            actions,
            [TokenAction::ReadStructure, TokenAction::ManageMembers]
        );
        assert!(refused(
            "tokens",
            &["create", "setup", "--action", "read_structure"]
        ));
        assert!(refused("tokens", &["create", "setup", "--days", "30"]));
        assert!(refused(
            "tokens",
            &["create", "--action", "read_structure", "--days", "30"]
        ));
        for wrong in ["read_everything", "unknown"] {
            assert!(refused(
                "tokens",
                &["create", "setup", "--action", wrong, "--days", "30"]
            ));
        }
        for held in TokenAction::ALL {
            assert!(matches!(token_action(held.name()), Ok(read) if read == *held));
        }
    }

    #[test]
    fn an_action_that_the_tool_does_not_know_is_refused() {
        for command in [
            "workspaces",
            "categories",
            "channels",
            "overrides",
            "members",
            "accounts",
            "invites",
            "tokens",
            "exports",
            "audit",
            "archive-period",
            "retention",
            "logo",
        ] {
            assert!(refused(command, &["destroy"]), "{command}");
            assert!(refused(command, &[]), "{command}");
        }
        assert!(refused("roles", &["list"]));
        assert!(refused("deny-request", &[]));
        assert!(refused("deny-request", &["first"]));
        assert!(refused("workspaces", &["delete", "Acme"]));
        let request_id = JoinRequestId::generate();
        let request = request_id.to_string();
        assert!(matches!(
            parsed("deny-request", &[request.as_str()]),
            Ok(Action::DenyRequest(id)) if id == request_id
        ));
        let entry_id = AuditEntryId::generate();
        let entry = entry_id.to_string();
        let Ok(Action::ServerAudit(page)) = parsed(
            "audit",
            &["server", "--before", entry.as_str(), "--limit", "20"],
        ) else {
            panic!("the page is not read");
        };
        assert_eq!(
            page,
            AuditFilterQuery {
                before: Some(entry_id),
                limit: Some(20),
                ..AuditFilterQuery::default()
            }
        );
        assert!(refused("audit", &["server", "--before", "yesterday"]));
    }

    #[test]
    fn a_removal_and_a_ban_take_a_reason_or_none() {
        let (account_id, workspace_id) = (AccountId::generate(), WorkspaceId::generate());
        let (account, workspace) = (account_id.to_string(), workspace_id.to_string());
        let (account, workspace) = (account.as_str(), workspace.as_str());
        assert!(matches!(
            parsed("accounts", &["remove", account, "--reason", "Spam in #general"]),
            Ok(Action::RemoveAccount(id, Statement { reason: Some(reason), message: None }))
                if id == account_id && reason == "Spam in #general"
        ));
        assert!(matches!(
            parsed("accounts", &["remove", account]),
            Ok(Action::RemoveAccount(id, statement))
                if id == account_id && statement == Statement::default()
        ));
        assert!(matches!(
            parsed("accounts", &["ban", account, "--reason", "Spam", "--message", "Read the rules"]),
            Ok(Action::BanAccount(id, Statement { reason: Some(reason), message: Some(message) }))
                if id == account_id && reason == "Spam" && message == "Read the rules"
        ));
        assert!(matches!(
            parsed("accounts", &["ban", account]),
            Ok(Action::BanAccount(id, statement))
                if id == account_id && statement == Statement::default()
        ));
        assert!(matches!(
            parsed("members", &["remove", workspace, account, "--reason", "Left the team"]),
            Ok(Action::RemoveMember(in_workspace, id, Statement { reason: Some(reason), message: None }))
                if in_workspace == workspace_id && id == account_id && reason == "Left the team"
        ));
        assert!(matches!(
            parsed("members", &["remove", workspace, account]),
            Ok(Action::RemoveMember(in_workspace, id, statement))
                if in_workspace == workspace_id && id == account_id
                    && statement == Statement::default()
        ));
        assert!(refused("members", &["remove", workspace]));
        assert!(refused("members", &["remove", workspace, "first"]));
        assert!(refused("accounts", &["remove"]));
        assert!(refused("accounts", &["ban", "first"]));
    }

    #[test]
    fn a_change_of_an_override_names_one_permission_and_its_state() {
        let (channel_id, role_id) = (ChannelId::generate(), WorkspaceRoleId::generate());
        let (channel, role) = (channel_id.to_string(), role_id.to_string());
        let change = |options: &[&str]| {
            let mut words = vec!["change", channel.as_str(), role.as_str()];
            words.extend(options);
            match parsed("overrides", &words) {
                Ok(Action::ChangeOverride(channel_id, role_id, change)) => {
                    Some((channel_id, role_id, change))
                }
                _ => None,
            }
        };
        for (word, state) in [
            ("allow", OverrideState::Allow),
            ("deny", OverrideState::Deny),
            ("inherit", OverrideState::Inherit),
        ] {
            let expected = ChangeChannelOverride {
                permission: WorkspacePermission::MuteMembers,
                state,
            };
            assert_eq!(
                change(&["mute_members", word]),
                Some((channel_id, role_id, expected))
            );
        }
        assert_eq!(change(&[]), None);
        assert_eq!(change(&["mute_members"]), None);
        assert_eq!(change(&["mute_members", "remove"]), None);
        assert_eq!(change(&["MuteMembers", "allow"]), None);
        assert_eq!(change(&["administrator", "deny"]), None);
        assert!(refused("overrides", &["change", channel.as_str()]));
    }

    #[test]
    fn the_audit_log_takes_filters_with_identifiers_and_times() {
        let (actor_id, target_id) = (AccountId::generate(), ChannelId::generate());
        let workspace_id = WorkspaceId::generate();
        let (actor, target) = (actor_id.to_string(), target_id.to_string());
        let workspace = workspace_id.to_string();
        let since = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let Ok(Action::WorkspaceAudit(in_workspace, query)) = parsed(
            "audit",
            &[
                "workspace",
                workspace.as_str(),
                "--action",
                "account_banned",
                "--actor",
                actor.as_str(),
                "--target",
                target.as_str(),
                "--since",
                "2024-10-04T00:00:00Z",
                "--until",
                "2024-10-05T00:00:00Z",
                "--limit",
                "5",
            ],
        ) else {
            panic!("the filters are not read");
        };
        assert_eq!(in_workspace, workspace_id);
        assert_eq!(
            query,
            AuditFilterQuery {
                action: Some("account_banned".to_owned()),
                actor_id: Some(actor_id),
                target_id: Some(target_id.as_uuid()),
                since: Some(since),
                until: Some(since + time::Duration::days(1)),
                before: None,
                limit: Some(5),
            }
        );
        // No option: no filter.
        assert!(matches!(
            parsed("audit", &["server"]),
            Ok(Action::ServerAudit(query)) if query == AuditFilterQuery::default()
        ));
        assert!(refused("audit", &["server", "--since", "yesterday"]));
        assert!(refused("audit", &["server", "--until", "2024-10-05"]));
        assert!(refused("audit", &["server", "--actor", "first"]));
        assert!(refused("audit", &["server", "--target", "general"]));
        assert!(refused("audit", &["workspace", "--limit", "5"]));
    }

    #[test]
    fn the_logo_is_shown_set_from_a_file_and_removed() {
        const IMAGE: &[u8] = b"\x89PNG\r\n\x1a\nimage";
        assert!(matches!(parsed("logo", &["show"]), Ok(Action::ShowLogo)));
        let removal = parsed("logo", &["remove"]);
        assert!(matches!(removal, Ok(Action::RemoveLogo)));
        assert!(refused("logo", &["set"]));

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("logo.png");
        std::fs::write(&path, IMAGE).unwrap();
        assert!(matches!(
            parsed("logo", &["set", path.to_str().unwrap()]),
            Ok(Action::SetLogo(image)) if image == IMAGE
        ));
        // A file that does not exist fails before the server is contacted.
        let absent = directory.path().join("absent.png");
        assert!(matches!(
            parsed("logo", &["set", absent.to_str().unwrap()]),
            Err(Failure::Failed(_))
        ));

        let stored = Avatar {
            hash: "ab12\u{1b}".to_owned(),
            media_type: ImageType::Png,
            bytes: 2048,
            width: 64,
            height: 32,
        };
        assert_eq!(
            stored_logo_line(&stored),
            "logo: image/png  2048 bytes  64x32  ab12"
        );
        let read = AvatarImage {
            media_type: ImageType::Webp,
            hash: "cd34".to_owned(),
            bytes: IMAGE.to_vec(),
        };
        assert_eq!(
            logo_line(Some(&read)),
            format!("logo: image/webp  {} bytes  cd34", IMAGE.len())
        );
        assert_eq!(logo_line(None), "logo: none");
    }

    #[test]
    fn the_lines_of_the_structure_show_each_object_with_its_identifier() {
        let workspace = Workspace {
            id: WorkspaceId::generate(),
            name: "Acme\u{1b}[2J".to_owned(),
            is_default: true,
        };
        assert_eq!(
            workspace_line(&workspace),
            format!("workspace: {}  default  Acme[2J", workspace.id)
        );
        let other = Workspace {
            is_default: false,
            ..workspace.clone()
        };
        assert_eq!(
            workspace_line(&other),
            format!("workspace: {}  Acme[2J", other.id)
        );

        let category = Category {
            id: CategoryId::generate(),
            workspace_id: workspace.id,
            name: "Projects".to_owned(),
            position: 2,
        };
        assert_eq!(
            category_line(&category),
            format!("category: {}  position 2  Projects", category.id)
        );

        let plain = channel("plans");
        assert_eq!(
            channel_line(&plain),
            format!(
                "channel: {}  text  position 3  category none  plans",
                plain.id
            )
        );
        let voice = Channel {
            category_id: Some(category.id),
            kind: ChannelKind::Voice,
            permanent_messages: true,
            user_limit: Some(4),
            hides_occupants: true,
            ..channel("Daily\r")
        };
        assert_eq!(
            channel_line(&voice),
            format!(
                "channel: {}  voice  position 3  category {}  permanent  limit 4  hides who is \
                 inside  Daily",
                voice.id, category.id
            )
        );

        let at = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let kept = ArchivedChannel {
            channel: plain.clone(),
            archived_at: at,
            deletes_at: None,
        };
        assert_eq!(
            archived_line(&kept),
            format!(
                "archived: {}  archived at {at}  never deleted  plans",
                plain.id
            )
        );
        let later = at + time::Duration::days(30);
        let due = ArchivedChannel {
            deletes_at: Some(later),
            ..kept
        };
        assert_eq!(
            archived_line(&due),
            format!(
                "archived: {}  archived at {at}  deleted after {later}  plans",
                plain.id
            )
        );

        let held = ChannelOverride {
            role_id: WorkspaceRoleId::generate(),
            allow: Vec::new(),
            deny: vec![
                WorkspacePermission::ViewChannel,
                WorkspacePermission::ManageMessages,
            ],
        };
        assert_eq!(
            override_line(&held),
            format!(
                "override: role {}  allow none  deny view_channel,manage_messages",
                held.role_id
            )
        );
    }

    #[test]
    fn the_lines_of_people_show_members_accounts_invites_and_requests() {
        let role_id = WorkspaceRoleId::generate();
        let member = WorkspaceMember {
            avatar: None,
            account_id: AccountId::generate(),
            name: "Grace\u{7}".to_owned(),
            role_ids: vec![role_id],
        };
        assert_eq!(
            member_line(&member),
            format!("member: {}  roles {role_id}  Grace", member.account_id)
        );

        let at = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let owner = AccountSummary {
            id: AccountId::generate(),
            name: "Ada".to_owned(),
            status: AccountStatus::Active,
            is_owner: true,
            server_role_ids: Vec::new(),
            created_at: at,
        };
        assert_eq!(
            account_line(&owner),
            format!("account: {}  active  owner  roles none  Ada", owner.id)
        );
        let banned = AccountSummary {
            status: AccountStatus::Banned,
            is_owner: false,
            ..owner.clone()
        };
        assert_eq!(
            account_line(&banned),
            format!("account: {}  banned  roles none  Ada", banned.id)
        );

        let workspace_id = WorkspaceId::generate();
        let invite = Invite {
            created_by_name: None,
            id: InviteId::generate(),
            created_by: owner.id,
            created_at: at,
            expires_at: at,
            max_uses: 5,
            uses: 2,
            workspace_ids: vec![workspace_id],
            revocation: None,
        };
        assert_eq!(
            invite_line(&invite),
            format!(
                "invite: {}  by {}  uses 2 of 5  expires {at}  workspaces {workspace_id}  not \
                 revoked",
                invite.id, owner.id
            )
        );
        let with_creator = Invite {
            created_by_name: Some("Ada\u{1b}[2J".to_owned()),
            ..invite.clone()
        };
        assert_eq!(
            invite_line(&with_creator),
            format!("{}  creator Ada[2J", invite_line(&invite))
        );
        let revoked = Invite {
            created_by_name: None,
            revocation: Some(InviteRevocation::CreatorBanned),
            ..invite
        };
        assert!(invite_line(&revoked).ends_with("  creator_banned"));

        let status = JoinRequestStatus {
            id: JoinRequestId::generate(),
            state: JoinRequestState::Denied,
            account_id: None,
        };
        assert_eq!(
            request_line(&status),
            format!("request {}: denied", status.id)
        );
        let redemption = Redemption {
            workspace_ids: vec![workspace_id],
        };
        assert_eq!(
            redemption_line(&redemption),
            format!("workspaces: {workspace_id}")
        );
    }

    #[test]
    fn the_lines_of_tokens_and_exports_show_their_state_and_no_secret() {
        let at = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let token = AccessTokenInfo {
            id: AccessTokenId::generate(),
            name: "setup".to_owned(),
            created_by: AccountId::generate(),
            actions: vec![TokenAction::ReadStructure, TokenAction::Unknown],
            created_at: at,
            expires_at: at,
        };
        assert_eq!(
            token_line(&token),
            format!(
                "token: {}  by {}  actions read_structure,unknown  expires {at}  setup",
                token.id, token.created_by
            )
        );

        let channel_id = ChannelId::generate();
        let waiting = Export {
            id: ExportId::generate(),
            place: MessagePlace::Channel {
                workspace_id: WorkspaceId::generate(),
                channel_id,
            },
            state: ExportState::Waiting,
            failure: None,
            requested_at: at,
            finished_at: None,
            expires_at: None,
            bytes: None,
        };
        assert_eq!(
            export_line(&waiting),
            format!(
                "export: {}  channel:{channel_id}  waiting  requested {at}",
                waiting.id
            )
        );
        let ready = Export {
            state: ExportState::Ready,
            finished_at: Some(at),
            expires_at: Some(at),
            bytes: Some(2048),
            ..waiting.clone()
        };
        assert_eq!(
            export_line(&ready),
            format!(
                "export: {}  channel:{channel_id}  ready  requested {at}  2048 bytes  removed \
                 after {at}",
                ready.id
            )
        );
        let conversation_id = ConversationId::generate();
        let broken = Export {
            place: MessagePlace::Direct {
                conversation_id,
                low_account_id: AccountId::generate(),
                high_account_id: AccountId::generate(),
            },
            state: ExportState::Failed,
            failure: Some(ExportFailure::StorageLow),
            ..waiting
        };
        assert_eq!(
            export_line(&broken),
            format!(
                "export: {}  direct:{conversation_id}  failed  requested {at}  failure \
                 storage_low",
                broken.id
            )
        );
    }

    #[test]
    fn a_line_of_the_audit_log_shows_the_time_the_actor_the_action_and_its_identifiers() {
        let at = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let (actor_id, account_id) = (AccountId::generate(), AccountId::generate());
        let (workspace_id, channel_id) = (WorkspaceId::generate(), ChannelId::generate());
        let entry = |event: AuditEvent| AuditEntry {
            id: AuditEntryId::generate(),
            occurred_at: at,
            actor_id: Some(actor_id),
            access_token_id: None,
            workspace_id: None,
            channel_id: None,
            event,
        };

        let banned = entry(AuditEvent::AccountBanned {
            recipient_message: None,
            reason: None,
            account_id,
            cancelled_invite_ids: Vec::new(),
        });
        assert_eq!(
            audit_line(&banned),
            format!(
                "audit: {}  {at}  {actor_id}  account_banned  account={account_id} \
                 cancelled_invites=none",
                banned.id
            )
        );

        // An entry of the server itself, about a channel, with an action without an
        // identifier of its own.
        let archived = AuditEntry {
            actor_id: None,
            workspace_id: Some(workspace_id),
            channel_id: Some(channel_id),
            ..entry(AuditEvent::ChannelArchived)
        };
        assert_eq!(
            audit_line(&archived),
            format!(
                "audit: {}  {at}  server  channel_archived  workspace={workspace_id} \
                 channel={channel_id}",
                archived.id
            )
        );

        // The workspace of the entry and of the action is named once.
        let token_id = AccessTokenId::generate();
        let deleted = AuditEntry {
            access_token_id: Some(token_id),
            workspace_id: Some(workspace_id),
            ..entry(AuditEvent::WorkspaceDeleted {
                workspace_id,
                name: "Second".to_owned(),
            })
        };
        assert_eq!(
            audit_line(&deleted),
            format!(
                "audit: {}  {at}  {actor_id}  workspace_deleted  with_token={token_id} \
                 workspace={workspace_id}",
                deleted.id
            )
        );

        let unknown = entry(AuditEvent::Unknown);
        assert_eq!(
            audit_line(&unknown),
            format!("audit: {}  {at}  {actor_id}  unknown", unknown.id)
        );

        let page = AuditPage {
            next_before: Some(unknown.id),
            entries: vec![unknown],
        };
        let lines = audit_lines(&page);
        assert_eq!(lines.len(), 2);
        assert_eq!(
            lines[1],
            format!("continue with: --before {}", page.entries[0].id)
        );
    }

    #[test]
    fn a_line_of_the_audit_log_shows_a_reason_a_narrowed_invite_and_the_logo() {
        let at = OffsetDateTime::UNIX_EPOCH + time::Duration::days(20_000);
        let (actor_id, account_id) = (AccountId::generate(), AccountId::generate());
        let (workspace_id, invite_id) = (WorkspaceId::generate(), InviteId::generate());
        let entry = |event: AuditEvent| AuditEntry {
            id: AuditEntryId::generate(),
            occurred_at: at,
            actor_id: Some(actor_id),
            access_token_id: None,
            workspace_id: None,
            channel_id: None,
            event,
        };

        // A reason stands in quotation marks, without its control characters.
        let banned = entry(AuditEvent::AccountBanned {
            recipient_message: None,
            reason: Some("Spam in \"general\"\u{1b}[2J".to_owned()),
            account_id,
            cancelled_invite_ids: vec![invite_id],
        });
        assert_eq!(
            audit_line(&banned),
            format!(
                "audit: {}  {at}  {actor_id}  account_banned  account={account_id} \
                 cancelled_invites={invite_id} reason=\"Spam in \\\"general\\\"[2J\"",
                banned.id
            )
        );

        let removed = entry(AuditEvent::AccountRemoved {
            recipient_message: Some("Thank you\u{7}".to_owned()),
            reason: Some("Left the team".to_owned()),
            account_id,
        });
        assert_eq!(
            audit_line(&removed),
            format!(
                "audit: {}  {at}  {actor_id}  account_removed  account={account_id} \
                 reason=\"Left the team\" message=\"Thank you\"",
                removed.id
            )
        );
        let without_reason = entry(AuditEvent::AccountRemoved {
            recipient_message: None,
            reason: None,
            account_id,
        });
        assert_eq!(
            audit_line(&without_reason),
            format!(
                "audit: {}  {at}  {actor_id}  account_removed  account={account_id}",
                without_reason.id
            )
        );

        let left = AuditEntry {
            workspace_id: Some(workspace_id),
            ..entry(AuditEvent::WorkspaceMemberRemoved {
                recipient_message: None,
                account_id,
                reason: Some("Left the team".to_owned()),
            })
        };
        assert_eq!(
            audit_line(&left),
            format!(
                "audit: {}  {at}  {actor_id}  workspace_member_removed  workspace={workspace_id} \
                 account={account_id} reason=\"Left the team\"",
                left.id
            )
        );

        let narrowed = entry(AuditEvent::InviteWorkspacesChanged {
            invite_id,
            invite_created_by: account_id,
            removed_workspace_id: workspace_id,
        });
        assert_eq!(
            audit_line(&narrowed),
            format!(
                "audit: {}  {at}  {actor_id}  invite_workspaces_changed  invite={invite_id} \
                 invite_created_by={account_id} removed_workspace={workspace_id}",
                narrowed.id
            )
        );

        let logo = Avatar {
            hash: "ab12".to_owned(),
            media_type: ImageType::Png,
            bytes: 2048,
            width: 64,
            height: 64,
        };
        let set = entry(AuditEvent::ServerLogoChanged { logo: Some(logo) });
        assert_eq!(
            audit_line(&set),
            format!(
                "audit: {}  {at}  {actor_id}  server_logo_changed  logo=ab12",
                set.id
            )
        );
        let taken = entry(AuditEvent::ServerLogoChanged { logo: None });
        assert_eq!(
            audit_line(&taken),
            format!(
                "audit: {}  {at}  {actor_id}  server_logo_changed  logo=none",
                taken.id
            )
        );
    }

    #[test]
    fn the_lines_of_the_archive_period_and_of_retention_show_days_or_never() {
        assert_eq!(
            period_line(ArchivePeriod { days: Some(30) }),
            "archive period: 30 days"
        );
        assert_eq!(
            period_line(ArchivePeriod { days: None }),
            "archive period: never"
        );
        assert_eq!(
            retention_line("file", &Retention { days: Some(1) }),
            "file retention: 1 day"
        );
        assert_eq!(
            retention_line("recording", &Retention { days: None }),
            "recording retention: never"
        );
    }

    /// A body that arrives in the given parts.
    fn body(parts: Vec<Result<&'static str, TransportError>>) -> BodyStream {
        let parts = parts
            .into_iter()
            .map(|part| part.map(|text| bytes::Bytes::from_static(text.as_bytes())));
        Box::pin(futures_util::stream::iter(parts))
    }

    /// What a file holds.
    fn held(file: &mut std::fs::File) -> Vec<u8> {
        let mut bytes = Vec::new();
        file.rewind().unwrap();
        file.read_to_end(&mut bytes).unwrap();
        bytes
    }

    #[tokio::test]
    async fn a_file_that_is_longer_than_the_export_is_refused_and_not_written_past_its_size() {
        let mut file = tempfile::tempfile().unwrap();
        let longer = body(vec![Ok("abcd"), Ok("efgh"), Ok("ij")]);
        assert!(matches!(
            write_body(longer, 6, &mut file).await,
            Err(Failure::Failed(_))
        ));
        assert_eq!(held(&mut file), b"abcd");

        // One part that is larger than the whole export is not written at all.
        let mut file = tempfile::tempfile().unwrap();
        let one_part = body(vec![Ok("abcdefgh")]);
        assert!(write_body(one_part, 6, &mut file).await.is_err());
        assert!(held(&mut file).is_empty());

        let mut file = tempfile::tempfile().unwrap();
        let no_file = body(vec![Ok("a")]);
        assert!(write_body(no_file, 0, &mut file).await.is_err());
        assert!(held(&mut file).is_empty());
    }

    #[tokio::test]
    async fn a_file_that_ends_before_the_size_of_the_export_is_refused() {
        let mut file = tempfile::tempfile().unwrap();
        let shorter = body(vec![Ok("abc"), Ok("de")]);
        assert!(matches!(
            write_body(shorter, 6, &mut file).await,
            Err(Failure::Failed(_))
        ));

        let mut file = tempfile::tempfile().unwrap();
        assert!(write_body(body(Vec::new()), 6, &mut file).await.is_err());

        // A connection that fails ends the body before its size.
        let mut file = tempfile::tempfile().unwrap();
        let cut = TransportError::new(TransportErrorKind::Interrupted, "");
        let interrupted = body(vec![Ok("abc"), Err(cut), Ok("def")]);
        assert!(write_body(interrupted, 6, &mut file).await.is_err());
        assert_eq!(held(&mut file), b"abc");
    }

    #[tokio::test]
    async fn a_file_of_the_size_of_the_export_is_written_whole() {
        let mut file = tempfile::tempfile().unwrap();
        let whole = body(vec![Ok("abc"), Ok(""), Ok("def")]);
        assert!(write_body(whole, 6, &mut file).await.is_ok());
        assert_eq!(held(&mut file), b"abcdef");

        let mut file = tempfile::tempfile().unwrap();
        assert!(write_body(body(Vec::new()), 0, &mut file).await.is_ok());
        assert!(held(&mut file).is_empty());
    }
}
