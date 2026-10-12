//! The text of the pages of the records of a server: access tokens, exports and the audit
//! log (ADR 0761).

use std::path::Path;

use junto_client_core::api::{ApiError, ErrorCategory};
use junto_client_core::flow::FlowError;
use junto_client_core::protocol::access_token::TokenAction;
use junto_client_core::protocol::audit::AuditEvent;
use junto_client_core::protocol::discovery::NotificationLevel;
use junto_client_core::protocol::export::ExportFailure;
use junto_client_core::protocol::ids::WorkspaceId;
use junto_client_core::protocol::recording::RecordingStopReason;
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::protocol::{ServerPermission, WorkspacePermission};
use time::OffsetDateTime;

use super::{channel_name, date, date_time, days};
use crate::state::admin::AuditScope;
use crate::state::admin::view::records::{
    About, Actor, ActorChoice, AuditGroup, AuditLine, ExportLine, ExportOf, ExportStatus, Names,
    ScopeChoice, TokenFault, TokenLine,
};

pub const COPY: &str = "Copy";
pub const COPIED: &str = "Copied";
pub const DONE: &str = "Done";

// The access tokens.
pub const TOKENS_HELP: &str = "An access token lets a program use the administration of this server without a session \
     of a device.";
pub const TOKEN_ACTS_AS: &str = "A token acts with the permissions that its creator has at the time of each request. It \
     never acts as the server owner.";
pub const TOKENS: &str = "Access tokens of the server";
pub const NO_TOKENS: &str = "The server lists no access token.";
pub const NEW_TOKEN: &str = "New access token";
pub const CREATE_TOKEN: &str = "Create token";
pub const TOKEN_NAME: &str = "Name of the token";
pub const TOKEN_NAME_HELP: &str = "A name of 1 to 64 characters that says what the token is for.";
pub const TOKEN_ACTIONS: &str = "Actions";
pub const TOKEN_EXPIRY: &str = "Expires after";
pub const TOKEN_EXPIRY_HELP: &str = "A token works for at most 366 days.";
pub const TOKEN_ONCE: &str = "This is the new access token. It is not shown again. Copy it now and keep it where no \
     other person reads it.";
pub const TOKEN_NEEDS: &str = "Only an administrator creates an access token.";
/// The name of the element that shows a new access token, for the accessibility tree.
/// The token itself is the text of the element and never its name.
pub const NEW_TOKEN_LABEL: &str = "The new access token";
pub const OWN_TOKENS: &str = "Your access tokens";
pub const NO_OWN_TOKENS: &str = "Your account has no access token.";
pub const OWN_TOKENS_HELP: &str =
    "The list holds the access tokens that your account created. You can revoke each of them.";
pub const REVOKE: &str = "Revoke";
pub const EXPIRED: &str = "Expired";

/// What the page of the access tokens is for. An administrator sees the tokens of every
/// account; every other person sees the own tokens.
pub const fn tokens_help(administrator: bool) -> &'static str {
    if administrator {
        TOKENS_HELP
    } else {
        OWN_TOKENS_HELP
    }
}

/// The label of the list of tokens.
pub const fn tokens_label(administrator: bool) -> &'static str {
    if administrator { TOKENS } else { OWN_TOKENS }
}

/// The statement of a list without a token.
pub const fn no_tokens(administrator: bool) -> &'static str {
    if administrator {
        NO_TOKENS
    } else {
        NO_OWN_TOKENS
    }
}

/// An action of an access token as a list names it.
pub const fn token_action(action: TokenAction) -> &'static str {
    match action {
        TokenAction::ReadStructure => "read the structure",
        TokenAction::ManageStructure => "change the structure",
        TokenAction::ManageMembers => "manage members",
        TokenAction::ReadAuditLog => "read the audit log",
        TokenAction::Unknown => "an action that this version does not know",
    }
}

/// An action of an access token as the title of its switch.
pub const fn token_action_title(action: TokenAction) -> &'static str {
    match action {
        TokenAction::ReadStructure => "Read the structure",
        TokenAction::ManageStructure => "Change the structure",
        TokenAction::ManageMembers => "Manage members",
        TokenAction::ReadAuditLog => "Read the audit log",
        TokenAction::Unknown => "An action that this version does not know",
    }
}

/// What a token with an action can do.
pub const fn token_action_help(action: TokenAction) -> &'static str {
    match action {
        TokenAction::ReadStructure => {
            "Read the workspaces, categories, channels, roles, overrides and members."
        }
        TokenAction::ManageStructure => {
            "Create and change workspaces, categories, channels, roles and overrides."
        }
        TokenAction::ManageMembers => {
            "Assign roles, add members, create invites, decide join requests, remove and ban \
             accounts."
        }
        TokenAction::ReadAuditLog => "Read the audit log of the server and of workspaces.",
        TokenAction::Unknown => "This version cannot give this action to a token.",
    }
}

/// The name of the switch of an action, for a test.
pub const fn token_action_selector(action: TokenAction) -> &'static str {
    match action {
        TokenAction::ReadStructure => "token-action-read-structure",
        TokenAction::ManageStructure => "token-action-manage-structure",
        TokenAction::ManageMembers => "token-action-manage-members",
        TokenAction::ReadAuditLog => "token-action-read-audit-log",
        TokenAction::Unknown => "token-action-unknown",
    }
}

pub const fn token_fault(fault: TokenFault) -> &'static str {
    match fault {
        TokenFault::NameEmpty => "Type a name for the token.",
        TokenFault::NameTooLong => "The name has more than 64 characters. Type a shorter name.",
        TokenFault::NoAction => "Choose at least one action.",
    }
}

/// A lifetime that the form of a new access token offers.
pub fn token_days(count: u32) -> String {
    days(Some(count))
}

/// The note under a list that does not hold every token of the server.
pub fn more_tokens(shown: usize) -> String {
    format!("The server holds more access tokens. The page shows the newest {shown}.")
}

/// The actions of a token.
pub fn token_actions(actions: &[TokenAction]) -> String {
    if actions.is_empty() {
        return "No action".to_owned();
    }
    let names: Vec<&str> = actions.iter().map(|action| token_action(*action)).collect();
    format!("Actions: {}", names.join(", "))
}

/// The second line of the row of a token: who created it, and when it was created and
/// expires.
pub fn token_detail(row: &TokenLine) -> String {
    let creator = match (&row.creator, row.own) {
        (_, true) => "Created by you".to_owned(),
        (Some(name), false) => format!("Created by {name}"),
        (None, false) => "Created by an account whose name is not known here".to_owned(),
    };
    let expiry = if row.expired {
        format!("Expired on {}", date_time(row.expires))
    } else {
        format!("Expires on {}", date_time(row.expires))
    };
    format!("{creator} on {} · {expiry}", date_time(row.created))
}

// The exports.
pub const EXPORTS_HELP: &str = "The exports that your account asked for. An export is one ZIP file with the messages, \
     files, recordings and transcripts that your account can read.";
pub const EXPORT_LIMITS: &str = "At most 3 exports of your account wait or are written at one time, and the server \
     stores at most 10. The server removes an export 7 days after it finished.";
pub const NO_EXPORTS: &str = "Your account has no export. You ask for one in the settings of a channel, or in a \
     direct conversation.";
pub const EXPORT_REQUEST_SENDING: &str = "The request for the export is on its way…";
pub const SAVE: &str = "Save";
pub const REMOVE: &str = "Remove";
pub const SAVING_NOW: &str = "The file is saved now.";
pub const SAVE_FILE_FAILED: &str =
    "The file could not be written to the download directory. Nothing was saved.";
pub const SAVE_ENDED: &str =
    "The server ended the download before the end of the file. Nothing was saved. Try again.";
pub const SAVE_TOO_LONG: &str =
    "The server sent more bytes than the export has. Nothing was saved.";
pub const SAVE_STOPPED: &str = "The download was stopped. Nothing was saved.";
/// The part of the name of a saved file that stands for a direct conversation.
pub const FILE_OF_CONVERSATION: &str = "conversation";
/// The part of the name of a saved file that stands for a channel without a known name.
pub const FILE_OF_CHANNEL: &str = "channel";

/// What an export holds, as the name of its row.
pub fn export_of(of: &ExportOf) -> String {
    match of {
        ExportOf::Channel {
            name,
            kind,
            workspace,
        } => match workspace {
            Some(workspace) => format!("{} · {workspace}", channel_name(*kind, name)),
            None => channel_name(*kind, name),
        },
        ExportOf::ChannelGone => "A channel that is not in your channel lists".to_owned(),
        ExportOf::Direct { other: Some(name) } => format!("Direct conversation with {name}"),
        ExportOf::Direct { other: None } => "A direct conversation".to_owned(),
    }
}

/// Where an export stands on the server. A failed export states why it has no file.
pub const fn export_status(status: ExportStatus) -> &'static str {
    match status {
        ExportStatus::Waiting => "Waiting",
        ExportStatus::Running => "The server writes the file now",
        ExportStatus::Ready => "Ready",
        ExportStatus::Failed(ExportFailure::StorageLow) => {
            "Failed: the server has too little free disk space for the file"
        }
        ExportStatus::Failed(ExportFailure::AccessEnded) => {
            "Failed: your account lost the permission for the export while the file was written"
        }
        ExportStatus::Failed(ExportFailure::Interrupted) => {
            "Failed: the server stopped three times while it wrote the file"
        }
        ExportStatus::Failed(ExportFailure::Failed) => {
            "Failed: the server could not read a file of the export or could not write the file"
        }
    }
}

/// The line of the row of an export under its state: when it was asked for, the size of
/// its file, and when the server removes it.
pub fn export_detail(row: &ExportLine) -> String {
    let mut parts = vec![format!("Asked for on {}", date_time(row.requested))];
    if let Some(bytes) = row.bytes {
        parts.push(crate::text::file_size(bytes));
    }
    if let Some(expires) = row.expires {
        parts.push(format!("The server removes it on {}", date(expires)));
    }
    parts.join(" · ")
}

/// The progress of the file of an export that is saved.
pub fn save_progress(written: u64, bytes: Option<u64>) -> String {
    match bytes {
        Some(bytes) if bytes > 0 => {
            let percent = written.min(bytes).saturating_mul(100) / bytes;
            format!(
                "Saving the file: {percent}% ({} of {})",
                crate::text::file_size(written.min(bytes)),
                crate::text::file_size(bytes)
            )
        }
        _ => format!("Saving the file: {}", crate::text::file_size(written)),
    }
}

pub fn saved_as(path: &Path) -> String {
    format!("Saved as {}", path.display())
}

pub fn remove_export_question(of: &ExportOf) -> String {
    let what = match of {
        ExportOf::Channel { name, kind, .. } => {
            format!("the export of “{}”", channel_name(*kind, name))
        }
        ExportOf::ChannelGone => "this export".to_owned(),
        ExportOf::Direct { other: Some(name) } => {
            format!("the export of the direct conversation with {name}")
        }
        ExportOf::Direct { other: None } => "the export of the direct conversation".to_owned(),
    };
    format!(
        "Remove {what} with its file from the server? A file that you saved stays on this \
         device."
    )
}

/// Why the server did not give the file of an export.
pub fn save_refused(error: &ApiError) -> String {
    match error.category() {
        Some(ErrorCategory::NotFound) => {
            "The server does not give this file now: the export was removed or has expired, or \
             your account may not read its channel now. Nothing was saved."
                .to_owned()
        }
        Some(ErrorCategory::NotAllowed) => {
            "Your account may not download this export now. Nothing was saved.".to_owned()
        }
        _ => crate::text::flow_error(&FlowError::from(error)),
    }
}

// The audit log.
pub const AUDIT_HELP: &str = "The actions that the server recorded: who did what, and when. Nobody can change or \
     delete an entry.";
pub const AUDIT_SCOPE: &str = "Log that is shown";
pub const AUDIT_GROUP: &str = "Actions that are shown";
pub const AUDIT_ACTOR: &str = "Person";
pub const ALL_ACTIONS: &str = "All";
pub const EVERYONE: &str = "Everyone";
pub const UNNAMED_ACTOR: &str = "An account whose name is not known here";
pub const THE_SERVER: &str = "The server";
pub const NO_ENTRIES: &str = "This log has no entry.";
pub const NO_ENTRIES_SHOWN: &str = "No loaded entry passes the filters.";
pub const LOAD_OLDER: &str = "Load older";
pub const READ_AGAIN: &str = "Read again";
pub const AUDIT_BOUND: &str = "The page shows the newest 2,000 entries. It reads no older entry.";
pub const UNKNOWN_ACTION: &str = "An action that this version of Junto does not know";

/// A scope of the audit log as the control names it.
pub fn audit_scope(choice: &ScopeChoice) -> String {
    match (&choice.scope, &choice.workspace) {
        (AuditScope::Server, _) => "Server".to_owned(),
        (AuditScope::Workspace(_), Some(name)) => format!("Workspace {name}"),
        (AuditScope::Workspace(_), None) => "Workspace".to_owned(),
    }
}

/// A group of actions as the filter names it.
pub const fn audit_group(group: AuditGroup) -> &'static str {
    match group {
        AuditGroup::Members => "Members",
        AuditGroup::Invites => "Invites",
        AuditGroup::Roles => "Roles",
        AuditGroup::Structure => "Channels",
        AuditGroup::Calls => "Calls",
        AuditGroup::Messages => "Messages",
        AuditGroup::Server => "Server",
    }
}

/// What a group of actions holds, for the accessibility tree and the help of the filter.
pub const fn audit_group_help(group: AuditGroup) -> &'static str {
    match group {
        AuditGroup::Members => "Members and bans",
        AuditGroup::Invites => "Invites and join requests",
        AuditGroup::Roles => "Roles and permissions",
        AuditGroup::Structure => "Workspaces and channels",
        AuditGroup::Calls => "Calls and recordings",
        AuditGroup::Messages => "Messages and retention",
        AuditGroup::Server => "Access tokens and the server",
    }
}

/// The sentence under the filter that states what the chosen group holds.
pub fn audit_group_shown(group: Option<AuditGroup>) -> String {
    match group {
        Some(group) => format!("Shown: {}.", lower_first(audit_group_help(group))),
        None => "Shown: every action.".to_owned(),
    }
}

/// An account of the filter by the person who acted.
pub fn audit_actor_choice(choice: &ActorChoice) -> String {
    choice
        .name
        .clone()
        .unwrap_or_else(|| UNNAMED_ACTOR.to_owned())
}

pub fn audit_count(shown: usize, loaded: usize) -> String {
    match loaded {
        1 => format!("{shown} of 1 loaded entry shown"),
        loaded => format!("{shown} of {loaded} loaded entries shown"),
    }
}

fn about(about: &About, unknown: &'static str) -> String {
    match about {
        About::Named(name) => name.clone(),
        About::Unknown => unknown.to_owned(),
    }
}

/// The first line of an entry: when it happened, who did it, and the workspace and the
/// channel that it is about.
pub fn audit_head(line: &AuditLine) -> String {
    let actor = match &line.actor {
        Actor::Server => THE_SERVER.to_owned(),
        Actor::Account {
            name: Some(name), ..
        } => name.clone(),
        Actor::Account { name: None, .. } => "An account".to_owned(),
    };
    let actor = if line.with_token {
        format!("{actor} with an access token")
    } else {
        actor
    };
    let mut parts = vec![date_time(line.at), actor];
    if let Some(workspace) = &line.workspace {
        parts.push(about(workspace, "A workspace"));
    }
    if let Some(channel) = &line.channel {
        parts.push(about(channel, "A channel"));
    }
    parts.join(" · ")
}

/// Why a read of the audit log failed.
pub fn audit_failure(error: &ApiError) -> String {
    match error.category() {
        Some(ErrorCategory::NotAllowed) => "Your account may not read this audit log.".to_owned(),
        Some(ErrorCategory::NotFound) => {
            "The server has no such workspace for your account.".to_owned()
        }
        _ => crate::text::flow_error(&FlowError::from(error)),
    }
}

fn lower_first(text: &str) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}

fn capital(text: String) -> String {
    let mut characters = text.chars();
    match characters.next() {
        Some(first) if first.is_lowercase() => first.to_uppercase().chain(characters).collect(),
        _ => text,
    }
}

/// Names in a sentence: `a`, `a and b`, `a, b and c`.
fn listed(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

fn the_account(name: Option<String>) -> String {
    name.map_or_else(
        || "an account".to_owned(),
        |name| format!("the account “{name}”"),
    )
}

fn the_role(name: Option<String>) -> String {
    name.map_or_else(|| "a role".to_owned(), |name| format!("the role “{name}”"))
}

fn the_server_role(name: Option<String>) -> String {
    name.map_or_else(
        || "a server role".to_owned(),
        |name| format!("the server role “{name}”"),
    )
}

/// The workspaces that an invite or an approval names. None: the default workspace.
fn the_workspaces(ids: &[WorkspaceId], names: &Names<'_>) -> String {
    if ids.is_empty() {
        return "the default workspace".to_owned();
    }
    let known: Vec<String> = ids
        .iter()
        .filter_map(|id| names.workspace(*id))
        .map(|name| format!("“{name}”"))
        .collect();
    let other = ids.len() - known.len();
    match (known.len(), other) {
        (0, 1) => "a workspace".to_owned(),
        (0, other) => format!("{other} workspaces"),
        (1, 0) => format!("the workspace {}", known[0]),
        (_, 0) => format!("the workspaces {}", listed(&known)),
        (1, 1) => format!("the workspace {} and 1 other workspace", known[0]),
        (1, other) => format!("the workspace {} and {other} other workspaces", known[0]),
        (_, 1) => format!("the workspaces {} and 1 other workspace", known.join(", ")),
        (_, other) => format!(
            "the workspaces {} and {other} other workspaces",
            known.join(", ")
        ),
    }
}

/// Permissions as the other texts name them: the wire name with spaces, in quotes.
fn the_permissions(names: Vec<&'static str>) -> String {
    let quoted: Vec<String> = names
        .iter()
        .map(|name| format!("“{}”", name.replace('_', " ")))
        .collect();
    match quoted.len() {
        0 => "no permission".to_owned(),
        1 => format!("the permission {}", quoted[0]),
        _ => format!("the permissions {}", listed(&quoted)),
    }
}

fn server_permissions(permissions: &[ServerPermission]) -> String {
    the_permissions(permissions.iter().map(|held| held.name()).collect())
}

fn workspace_permissions(permissions: &[WorkspacePermission]) -> String {
    the_permissions(permissions.iter().map(|held| held.name()).collect())
}

fn holders(count: u32, one: &'static str, many: &'static str) -> String {
    match count {
        0 => format!("no {one} held it"),
        1 => format!("1 {one} held it"),
        count => format!("{count} {many} held it"),
    }
}

/// When the server removes something by its age. Absent days: never.
fn removed_after(what: &'static str, age: Option<u32>) -> String {
    match age {
        Some(age) => format!("{what} are now removed after {}.", days(Some(age))),
        None => format!("{what} are no longer removed by their age."),
    }
}

const fn stop_reason(reason: RecordingStopReason) -> &'static str {
    match reason {
        RecordingStopReason::Stopped => "a person in the call stopped it",
        RecordingStopReason::CallEnded => "the call ended",
        RecordingStopReason::StorageReserve => {
            "the free disk space of the server reached its reserve"
        }
        RecordingStopReason::SegmentLimit => {
            "the recording has as many parts as a recording can have"
        }
        RecordingStopReason::RecorderLost => "the recorder of the server did not work any more",
        RecordingStopReason::Unknown => "the reason is one that this version does not know",
    }
}

const fn notification_level(level: NotificationLevel) -> &'static str {
    match level {
        NotificationLevel::All => "all messages",
        NotificationLevel::Mentions => "mentions only",
        NotificationLevel::Nothing => "nothing",
    }
}

const fn kind_of(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::Text => "text channel",
        ChannelKind::Voice => "voice channel",
    }
}

fn expiry(at: OffsetDateTime, names: &Names<'_>) -> String {
    date_time(names.local(at))
}

/// What was done, as one sentence, with the names that the state holds. An identifier
/// that the state does not know is written as a neutral text. A new action of the
/// protocol does not compile until it has a sentence.
pub fn audit_sentence(event: &AuditEvent, names: &Names<'_>) -> String {
    let account = |id| the_account(names.account(id));
    let sentence = match event {
        AuditEvent::OwnerClaimed { account_id } => format!(
            "{} became the server owner with the owner code.",
            account(*account_id)
        ),
        AuditEvent::OwnerAssigned {
            account_id,
            previous_owner_id,
        } => {
            let assigned = format!(
                "the host command made {} the server owner.",
                account(*account_id)
            );
            match previous_owner_id {
                Some(previous) => {
                    format!("{assigned} The owner before was {}.", account(*previous))
                }
                None => assigned,
            }
        }
        AuditEvent::OwnerCodeIssued {
            replaced_a_code,
            expires_at,
        } => {
            let mut text = "the host command stored a new owner code.".to_owned();
            if *replaced_a_code {
                text.push_str(" The code before it works no longer.");
            }
            if let Some(expires_at) = expires_at {
                text = format!(
                    "{text} The new code expires on {}.",
                    expiry(*expires_at, names)
                );
            }
            text
        }
        AuditEvent::OwnerAccessRestored { account_id, .. } => format!(
            "{} returned as the server owner with an owner code of the host command.",
            account(*account_id)
        ),
        AuditEvent::EventEpochRenewed => {
            "the host command reset the event stream, as after a restore of the server.".to_owned()
        }
        AuditEvent::InviteCreated {
            expires_at,
            max_uses,
            workspace_ids,
            ..
        } => format!(
            "an invite was created. It expires on {}, has {} and gives {}.",
            expiry(*expires_at, names),
            match max_uses {
                1 => "1 use".to_owned(),
                uses => format!("{uses} uses"),
            },
            the_workspaces(workspace_ids, names)
        ),
        AuditEvent::InviteRevoked { .. } => "an invite was revoked.".to_owned(),
        AuditEvent::AccountJoined {
            account_id,
            invite_created_by,
            ..
        } => format!(
            "{} joined the server with an invite of {}.",
            account(*account_id),
            account(*invite_created_by)
        ),
        AuditEvent::InviteRedeemed {
            account_id,
            invite_created_by,
            workspace_ids,
            ..
        } => format!(
            "{} entered {} with an invite of {}.",
            account(*account_id),
            the_workspaces(workspace_ids, names),
            account(*invite_created_by)
        ),
        AuditEvent::JoinRequestApproved {
            account_id,
            workspace_ids,
            ..
        } => format!(
            "a join request was approved. The server created {} in {}.",
            account(*account_id),
            the_workspaces(workspace_ids, names)
        ),
        AuditEvent::JoinRequestDenied { .. } => "a join request was denied.".to_owned(),
        AuditEvent::AccountRemoved { account_id } => {
            format!("{} was removed from the server.", account(*account_id))
        }
        AuditEvent::AccountBanned {
            account_id,
            cancelled_invite_ids,
        } => {
            let banned = format!("{} was banned.", account(*account_id));
            match cancelled_invite_ids.len() {
                0 => banned,
                1 => format!("{banned} 1 invite of the account was cancelled."),
                count => format!("{banned} {count} invites of the account were cancelled."),
            }
        }
        AuditEvent::AccountUnbanned { account_id } => {
            format!("the ban of {} was lifted.", account(*account_id))
        }
        AuditEvent::AccountRecovered { account_id, .. } => format!(
            "{} was recovered and has a new device key.",
            account(*account_id)
        ),
        AuditEvent::ServerRoleCreated {
            name, permissions, ..
        } => format!(
            "the server role “{name}” was created with {}.",
            server_permissions(permissions)
        ),
        AuditEvent::ServerRoleUpdated {
            name, permissions, ..
        } => format!(
            "the server role “{name}” was changed. It has {} now.",
            server_permissions(permissions)
        ),
        AuditEvent::ServerRoleAssigned {
            role_id,
            account_id,
        } => format!(
            "{} was given to {}.",
            the_server_role(names.server_role(*role_id)),
            account(*account_id)
        ),
        AuditEvent::AccessTokenCreated {
            name,
            actions,
            expires_at,
            ..
        } => {
            let actions: Vec<String> = actions
                .iter()
                .map(|action| token_action(*action).to_owned())
                .collect();
            format!(
                "the access token “{name}” was created. Its actions: {}. It expires on {}.",
                if actions.is_empty() {
                    "none".to_owned()
                } else {
                    listed(&actions)
                },
                expiry(*expires_at, names)
            )
        }
        AuditEvent::AccessTokenRevoked { created_by, .. } => format!(
            "an access token that {} created was revoked.",
            account(*created_by)
        ),
        AuditEvent::ServerRoleUnassigned {
            role_id,
            account_id,
        } => format!(
            "{} was taken from {}.",
            the_server_role(names.server_role(*role_id)),
            account(*account_id)
        ),
        AuditEvent::WorkspaceCreated { name, .. } => {
            format!("the workspace “{name}” was created.")
        }
        AuditEvent::WorkspaceRenamed { name, .. } => {
            format!("the workspace got the name “{name}”.")
        }
        AuditEvent::WorkspaceRoleCreated {
            name, permissions, ..
        } => format!(
            "the role “{name}” was created with {}.",
            workspace_permissions(permissions)
        ),
        AuditEvent::WorkspaceRoleUpdated {
            name, permissions, ..
        } => format!(
            "the role “{name}” was changed. It has {} now.",
            workspace_permissions(permissions)
        ),
        AuditEvent::WorkspaceRoleAssigned {
            role_id,
            account_id,
        } => format!(
            "{} was given to {}.",
            the_role(names.workspace_role(*role_id)),
            account(*account_id)
        ),
        AuditEvent::WorkspaceRoleUnassigned {
            role_id,
            account_id,
        } => format!(
            "{} was taken from {}.",
            the_role(names.workspace_role(*role_id)),
            account(*account_id)
        ),
        AuditEvent::WorkspaceMemberAdded { account_id } => format!(
            "an administrator added {} to the workspace.",
            account(*account_id)
        ),
        AuditEvent::CategoryCreated { name, .. } => {
            format!("the category “{name}” was created.")
        }
        AuditEvent::CategoryUpdated { name, position, .. } => {
            format!("a category was changed. Its name is “{name}” and its position is {position}.")
        }
        AuditEvent::ChannelCreated { name, kind } => {
            format!("the {} “{name}” was created.", kind_of(*kind))
        }
        AuditEvent::ChannelUpdated {
            name,
            category_id,
            position,
            permanent_messages,
        } => format!(
            "the channel was changed. Its name is “{name}”, it is in {}, its position is \
             {position}, and permanent messages are {}.",
            match category_id {
                Some(category_id) => names.category(*category_id).map_or_else(
                    || "a category".to_owned(),
                    |category| format!("the category “{category}”")
                ),
                None => "no category".to_owned(),
            },
            if *permanent_messages { "on" } else { "off" }
        ),
        AuditEvent::ChannelOverrideSet {
            role_id,
            allow,
            deny,
        } => {
            let role = the_role(names.workspace_role(*role_id));
            if allow.is_empty() && deny.is_empty() {
                format!("the rule of the channel for {role} was removed.")
            } else {
                format!(
                    "the rule of the channel for {role} was set. It allows {} and denies {}.",
                    workspace_permissions(allow),
                    workspace_permissions(deny)
                )
            }
        }
        AuditEvent::ChannelDeleted { name } => format!("the channel “{name}” was deleted."),
        AuditEvent::ChannelCallSettingsUpdated {
            user_limit,
            hides_occupants,
        } => format!(
            "the call settings of the channel were changed. It has {}, and it {} who is \
             inside.",
            match user_limit {
                Some(1) => "a limit of 1 person".to_owned(),
                Some(limit) => format!("a limit of {limit} people"),
                None => "no user limit".to_owned(),
            },
            if *hides_occupants { "hides" } else { "shows" }
        ),
        AuditEvent::CallMicrophoneBlocked { account_id } => format!(
            "the microphone of {} was blocked in the call of the channel.",
            account(*account_id)
        ),
        AuditEvent::CallMicrophoneUnblocked { account_id } => format!(
            "the microphone block of {} was removed in the call of the channel.",
            account(*account_id)
        ),
        AuditEvent::CallSoundBlocked { account_id } => format!(
            "the call sound of {} was blocked in the call of the channel.",
            account(*account_id)
        ),
        AuditEvent::CallSoundUnblocked { account_id } => format!(
            "the sound block of {} was removed in the call of the channel.",
            account(*account_id)
        ),
        AuditEvent::CallMemberDisconnected { account_id } => format!(
            "{} was removed from the call of the channel.",
            account(*account_id)
        ),
        AuditEvent::CallMemberMovedOut { account_id } => format!(
            "{} was moved out of the call of the channel into another voice channel.",
            account(*account_id)
        ),
        AuditEvent::CallMemberMovedIn { account_id } => format!(
            "{} was moved into the call of the channel from another voice channel.",
            account(*account_id)
        ),
        AuditEvent::RecordingStarted { .. } => {
            "a recording of the call of the channel was started.".to_owned()
        }
        AuditEvent::RecordingStopped { reason, .. } => format!(
            "a recording of the call of the channel was stopped: {}.",
            stop_reason(*reason)
        ),
        AuditEvent::RecordingRemoved { .. } => {
            "a recording of the channel was removed with its files and its transcript.".to_owned()
        }
        AuditEvent::TranscriptRequested { .. } => {
            "the transcript of a recording of the channel was asked for again.".to_owned()
        }
        AuditEvent::GlossaryReplaced { terms } => match terms {
            1 => "the glossary of the workspace was replaced. It has 1 term now.".to_owned(),
            terms => {
                format!("the glossary of the workspace was replaced. It has {terms} terms now.")
            }
        },
        AuditEvent::AttachmentLimitsChanged {
            bytes_per_file,
            files_per_message,
        } => format!(
            "the limits for the files of messages were changed: at most {} for each file, \
             and at most {} for each message.",
            crate::text::file_size(*bytes_per_file),
            match files_per_message {
                1 => "1 file".to_owned(),
                files => format!("{files} files"),
            }
        ),
        AuditEvent::PermanentMessageDeleted { .. } => {
            "a message was deleted in a channel with permanent messages.".to_owned()
        }
        AuditEvent::NotificationDefaultChanged { level } => format!(
            "the default notification level was changed to “{}”.",
            notification_level(*level)
        ),
        AuditEvent::RecordingRetentionChanged { days } => removed_after("recordings", *days),
        AuditEvent::RetentionChanged { days } => removed_after("files of messages", *days),
        AuditEvent::ChannelArchived => "the channel was archived.".to_owned(),
        AuditEvent::ChannelRestored => "the channel was restored from the archive.".to_owned(),
        AuditEvent::ArchivePeriodChanged { days: period } => match period {
            Some(period) => format!(
                "archived channels of the workspace are now deleted after {}.",
                days(Some(*period))
            ),
            None => {
                "archived channels of the workspace are no longer deleted by the server.".to_owned()
            }
        },
        AuditEvent::CategoryDeleted { name, .. } => {
            format!("the category “{name}” was deleted. Its channels stay and have no category.")
        }
        AuditEvent::WorkspaceRoleDeleted {
            name,
            permissions,
            holders: count,
            ..
        } => format!(
            "the role “{name}” was deleted. It had {}, and {}.",
            workspace_permissions(permissions),
            holders(*count, "member", "members")
        ),
        AuditEvent::ServerRoleDeleted {
            name,
            permissions,
            holders: count,
            ..
        } => format!(
            "the server role “{name}” was deleted. It had {}, and {}.",
            server_permissions(permissions),
            holders(*count, "account", "accounts")
        ),
        AuditEvent::WorkspaceDeleted { name, .. } => format!(
            "the workspace “{name}” was deleted with its members, roles, categories and \
             channels."
        ),
        AuditEvent::ExportRequested { .. } => "an export of the channel was asked for.".to_owned(),
        AuditEvent::Unknown => UNKNOWN_ACTION.to_owned(),
    };
    capital(sentence)
}
