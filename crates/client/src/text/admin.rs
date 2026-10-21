//! The text of the administration of a server and of the settings of a channel
//! (ADR 0761).

pub mod glossary;
pub mod notifications;
pub mod overrides;
pub mod people;
pub mod recordings;
pub mod records;
pub mod roles;
pub mod server;

use junto_client_core::api::{ApiError, ErrorCategory};
use junto_client_core::flow::FlowError;
use junto_client_core::model::LoadFailure;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::structure::ChannelKind;
use time::OffsetDateTime;

use crate::assets::Icon;
use crate::state::admin::view::NavTitle;
use crate::state::admin::{AdminPage, AskKind, ChannelPage};

pub const TITLE: &str = "Administration";
pub const CHANNEL_SETTINGS: &str = "Channel settings";
pub const PAGES: &str = "Pages";
pub const OPEN_ADMINISTRATION: &str = "Administration";
pub const OPEN_ADMINISTRATION_HELP: &str = "Workspaces, channels, members and exports";
pub const NAME_FIELD: &str = "Name";
pub const READING: &str = "Reading…";
pub const CREATE: &str = "Create";
pub const RENAME: &str = "Rename";
pub const DELETE: &str = "Delete";
pub const OPEN: &str = "Open";
pub const SETTINGS: &str = "Settings";
pub const NEVER: &str = "Never";
pub const CHANGE: &str = "Change";
pub const SENDING: &str = "Sending…";
pub const UP: &str = "Up";
pub const DOWN: &str = "Down";
pub const FIRST_ALREADY: &str = "It is the first one.";
pub const MOVE_NEEDS: &str =
    "The move changes a channel in which you do not hold “manage channel”.";
pub const CHANNEL_MANAGE_NEEDS: &str = "You do not hold “manage channel” in this channel.";
pub const LAST_ALREADY: &str = "It is the last one.";
pub const NAME_EMPTY: &str = "Type a name.";

pub fn nav_title(title: &NavTitle) -> String {
    match title {
        NavTitle::Server => "Server".to_owned(),
        NavTitle::Workspace(name) => format!("Workspace {name}"),
        NavTitle::Own => "Your account".to_owned(),
    }
}

pub const fn page_name(page: AdminPage) -> &'static str {
    match page {
        AdminPage::Workspaces => "Workspaces",
        AdminPage::Server => "Server",
        AdminPage::People => "People",
        AdminPage::Invites => "Invites",
        AdminPage::JoinRequests => "Join requests",
        AdminPage::Tokens => "Access tokens",
        AdminPage::AuditLog => "Audit log",
        AdminPage::ServerRoles => "Server roles",
        AdminPage::Channels => "Channels",
        AdminPage::Roles => "Roles",
        AdminPage::Archive => "Archive and retention",
        AdminPage::Members => "Members",
        AdminPage::Notifications => "Notifications",
        AdminPage::Recordings => "Recordings",
        AdminPage::Glossary => "Glossary",
        AdminPage::Exports => "Exports",
    }
}

pub const fn page_icon(page: AdminPage) -> Icon {
    match page {
        AdminPage::Workspaces => Icon::List,
        AdminPage::Server => Icon::Gear,
        AdminPage::People | AdminPage::Members => Icon::People,
        AdminPage::Invites => Icon::Invite,
        AdminPage::JoinRequests => Icon::Hand,
        AdminPage::Tokens | AdminPage::ServerRoles | AdminPage::Roles => Icon::Key,
        AdminPage::AuditLog => Icon::Clock,
        AdminPage::Channels => Icon::Hash,
        AdminPage::Archive => Icon::Lock,
        AdminPage::Notifications => Icon::Chat,
        AdminPage::Recordings => Icon::Record,
        AdminPage::Glossary => Icon::List,
        AdminPage::Exports => Icon::Download,
    }
}

/// The name of the row of a page in the list of pages, for a test.
pub const fn page_selector(page: AdminPage) -> &'static str {
    match page {
        AdminPage::Workspaces => "admin-nav-workspaces",
        AdminPage::Server => "admin-nav-server",
        AdminPage::People => "admin-nav-people",
        AdminPage::Invites => "admin-nav-invites",
        AdminPage::JoinRequests => "admin-nav-requests",
        AdminPage::Tokens => "admin-nav-tokens",
        AdminPage::AuditLog => "admin-nav-audit",
        AdminPage::ServerRoles => "admin-nav-server-roles",
        AdminPage::Channels => "admin-nav-channels",
        AdminPage::Roles => "admin-nav-roles",
        AdminPage::Archive => "admin-nav-archive",
        AdminPage::Members => "admin-nav-members",
        AdminPage::Notifications => "admin-nav-notifications",
        AdminPage::Recordings => "admin-nav-recordings",
        AdminPage::Glossary => "admin-nav-glossary",
        AdminPage::Exports => "admin-nav-exports",
    }
}

pub const fn channel_page_name(page: ChannelPage) -> &'static str {
    match page {
        ChannelPage::Overview => "Overview",
        ChannelPage::Permissions => "Permissions",
        ChannelPage::Archive => "Archive and deletion",
    }
}

pub const fn channel_page_icon(page: ChannelPage) -> Icon {
    match page {
        ChannelPage::Overview => Icon::Gear,
        ChannelPage::Permissions => Icon::Key,
        ChannelPage::Archive => Icon::Lock,
    }
}

pub const fn channel_page_selector(page: ChannelPage) -> &'static str {
    match page {
        ChannelPage::Overview => "channel-nav-overview",
        ChannelPage::Permissions => "channel-nav-permissions",
        ChannelPage::Archive => "channel-nav-archive",
    }
}

/// A channel as a title names it: `# general` for a text channel, the name alone for a
/// voice channel.
pub fn channel_name(kind: ChannelKind, name: &str) -> String {
    match kind {
        ChannelKind::Text => format!("# {name}"),
        ChannelKind::Voice => name.to_owned(),
    }
}

pub const fn kind_name(kind: ChannelKind) -> &'static str {
    match kind {
        ChannelKind::Text => "Text channel",
        ChannelKind::Voice => "Voice channel",
    }
}

/// A moment in the time zone of the person: `12 October 2026, 14:05`.
pub fn date_time(local: OffsetDateTime) -> String {
    format!(
        "{} {} {}, {:02}:{:02}",
        local.day(),
        local.month(),
        local.year(),
        local.hour(),
        local.minute()
    )
}

/// A date in the time zone of the person: `12 October 2026`.
pub fn date(local: OffsetDateTime) -> String {
    format!("{} {} {}", local.day(), local.month(), local.year())
}

/// A number of days, or never.
pub fn days(days: Option<u32>) -> String {
    match days {
        None => NEVER.to_owned(),
        Some(1) => "1 day".to_owned(),
        Some(days) => format!("{days} days"),
    }
}

/// Why a collection of a page was not read.
pub const fn load_failure(failure: LoadFailure) -> &'static str {
    match failure {
        LoadFailure::NotAllowed => "Your account may not read this.",
        LoadFailure::NotFound => "The server holds this no longer.",
        LoadFailure::Unavailable => "The server cannot give this now. Try again.",
        LoadFailure::NoAnswer => "No answer of the server arrived. Try again.",
    }
}

// The workspaces.
pub const WORKSPACES_HELP: &str =
    "A server holds one or more workspaces. Each has its own channels, members and roles.";
pub const NEW_WORKSPACE: &str = "New workspace";
pub const WORKSPACE_NAME: &str = "Name of the workspace";
pub const DEFAULT_WORKSPACE: &str = "Default workspace";
pub const DEFAULT_WORKSPACE_HELP: &str = "The default workspace is the one that an invite gives when it names no workspace. It \
     cannot be deleted.";
pub const SHOWN_WORKSPACE: &str = "Shown in the window";
pub const NOT_A_MEMBER: &str = "You are not a member";
pub const CREATE_WORKSPACE_NEEDS: &str = "You do not have the “create workspace” permission.";
pub const MANAGE_WORKSPACE_NEEDS: &str = "Only an administrator can rename or delete a workspace.";
pub const ENTER_CODE: &str = "Enter an invite code";
pub const ENTER_CODE_HELP: &str =
    "An invite code for this server adds your account to the workspaces that the code names.";
pub const INVITE_CODE: &str = "Invite code";
pub const ENTER: &str = "Enter";
pub const CODE_EMPTY: &str = "Type the invite code.";

pub fn delete_workspace_question(name: &str) -> String {
    format!(
        "Delete the workspace “{name}” with its channels, messages, files and recordings? \
         This cannot be undone. A workspace with permanent messages can be deleted by the \
         server owner only."
    )
}

pub fn entered(count: usize) -> String {
    match count {
        1 => "Your account entered 1 workspace. It is in the server book now.".to_owned(),
        count => {
            format!("Your account entered {count} workspaces. They are in the server book now.")
        }
    }
}

// The channels of a workspace.
pub const CHANNELS_HELP: &str =
    "The categories and the channels of this workspace, in the order of the channel list.";
pub const NO_CATEGORY: &str = "Without a category";
pub const NEW_CATEGORY: &str = "New category";
pub const NEW_CHANNEL: &str = "New channel";
pub const CATEGORY_NAME: &str = "Name of the category";
pub const CHANNEL_NAME: &str = "Name of the channel";
pub const KIND: &str = "Kind";
pub const CATEGORY: &str = "Category";
pub const MOVE_TO: &str = "Move to…";
pub const MOVE_TO_HELP: &str = "Choose the category of the channel.";
pub const MOVE_CHANGED: &str = "The order of the channels changed, or a channel was archived, \
     before the move. Nothing was moved; the page shows the order now.";
pub const MOVE_NOT_ALLOWED: &str = "Your account may not change one of the channels that this \
     move changes. Nothing was moved.";
pub const CHANNEL_GROUP_HELP: &str = "Choose the category, and then the place in it.";
pub const CHANNEL_PLACE_HELP: &str = "Choose the place of the channel.";
pub const PLACE_FIRST: &str = "At the start";
pub const OTHER_CATEGORY: &str = "Another category";

/// A place of a channel after another channel of its new category.
pub fn place_after(name: &str) -> String {
    format!("After {name}")
}

pub const CHANNEL_HANDLE: &str =
    "Drag to move the channel to another place or category. Up, Down and Move to do the same.";
pub const ARCHIVE: &str = "Archive";
pub const NO_CHANNELS_HERE: &str = "No channel.";
pub const PERMANENT_MARK: &str = "permanent messages";

pub fn delete_category_question(name: &str) -> String {
    format!("Delete the category “{name}”? Its channels stay and have no category.")
}

pub fn delete_channel_question(name: &str, permanent: bool) -> String {
    if permanent {
        format!(
            "Delete “{name}” with its messages, files and recordings? This cannot be undone. \
             The channel has permanent messages: only the server owner can delete it."
        )
    } else {
        format!("Delete “{name}” with its messages, files and recordings? This cannot be undone.")
    }
}

pub fn archive_channel_question(name: &str) -> String {
    format!(
        "Archive “{name}”? It leaves the channel list, nobody can write in it, and a call in \
         it ends. It can be restored."
    )
}

// The settings of a channel.
pub const OVERVIEW_HELP: &str = "The name, the category and the rules of this channel.";
pub const PERMANENT_MESSAGES_HELP: &str = "Nobody can edit a message that is sent while this is on. Only an administrator can \
     delete one, and each deletion is recorded.";
pub const PRIVATE_CHANNEL: &str = "Private channel";
pub const PRIVATE_CHANNEL_HELP: &str =
    "Only the roles that the Permissions page allows “view channel” can see the channel.";
pub const PRIVATE_UNKNOWN: &str = "The permissions of the channel are not read yet.";
pub const CALL_CONTROLS_HELP: &str =
    "“Lock room” and “Meeting mode” are controls of a running call, in the header of the call.";
pub const ARCHIVED_NO_CHANGE: &str =
    "The channel is archived. Its name, its category and its rules cannot be changed.";

// Archive and deletion of one channel.
pub const CHANNEL_ARCHIVE_HELP: &str = "Archive, export and deletion of this channel.";
pub const ARCHIVE_HELP: &str =
    "An archived channel leaves the channel list and can only be read. It can be restored.";
pub const RESTORE: &str = "Restore";
pub const RESTORE_NEEDS: &str =
    "You can open an archived channel. To restore one you need the “manage channel” permission.";
pub const RESTORE_HELP: &str = "The channel returns to the channel list and takes messages again.";
pub const EXPORT: &str = "Export";
pub const EXPORT_HELP: &str = "The server writes one file with the messages, files, recordings and transcripts that \
     your account can read. The file is under “Exports” for 7 days.";
pub const EXPORT_CONVERSATION: &str = "Export this conversation";
pub const EXPORT_NEEDS: &str = "You do not have the “export channel” permission.";
pub const EXPORT_ASKED: &str = "The export was asked for. Its state is on the page “Exports”.";
pub const SHOW_EXPORTS: &str = "Show exports";
pub const OWNER_DELETES: &str =
    "The channel has permanent messages. Only the server owner can delete it.";
pub const DELETE_CHANNEL: &str = "Delete channel";
pub const DELETE_CHANNEL_HELP: &str = "The channel is removed with its messages, files and recordings. Export it first to keep \
     a copy.";

pub fn archived_since(at: OffsetDateTime) -> String {
    format!("Archived on {}.", date(at))
}

pub fn deletes_at(at: Option<OffsetDateTime>) -> String {
    match at {
        Some(at) => format!("The server deletes it on {} or later.", date(at)),
        None => "The server does not delete it.".to_owned(),
    }
}

// Archive and retention of a workspace.
pub const ARCHIVE_PAGE_HELP: &str =
    "The archived channels of this workspace, and how long the server keeps what is old.";
pub const ARCHIVED_CHANNELS: &str = "Archived channels";
pub const NO_ARCHIVED: &str = "No channel of this workspace is archived.";
pub const ARCHIVE_PERIOD: &str = "Archive period";
pub const ARCHIVE_PERIOD_HELP: &str = "The server deletes an archived channel after this time, 7 days after a notice in the \
     channel. A channel with permanent messages is not deleted. From 7 to 36500 days.";
pub const FILE_RETENTION: &str = "Files of messages";
pub const FILE_RETENTION_HELP: &str = "The server removes the files of messages of this workspace after this time. The \
     messages stay. From 1 to 36500 days.";
pub const RECORDING_RETENTION: &str = "Recordings";
pub const RECORDING_RETENTION_HELP: &str =
    "The server removes the recordings of this workspace after this time. From 1 to 36500 days.";
pub const DIRECT_RETENTION: &str = "Files of direct messages";
pub const DIRECT_RETENTION_HELP: &str = "The server removes the files of direct messages of the whole server after this time. \
     From 1 to 36500 days.";
pub const RETENTION: &str = "Retention";
pub const DAYS_FIELD: &str = "Days";
pub const ARCHIVE_DAYS_RANGE: &str = "Type a number of days from 7 to 36500.";
pub const RETENTION_DAYS_RANGE: &str = "Type a number of days from 1 to 36500.";

// The message view of an archived channel.
pub const ARCHIVED_MARK: &str = "Archived";
pub const ARCHIVED_COMPOSER: &str = "This channel is archived. Nobody can write in it.";

fn conflict(what: AskKind) -> &'static str {
    match what {
        AskKind::DeleteWorkspace(_) => {
            "The default workspace cannot be deleted. An invite that names no workspace gives it."
        }
        AskKind::RedeemInvite => "Your account is in every workspace of this invite already.",
        AskKind::UpdateChannel(_) | AskKind::CallSettings(_) | AskKind::ArchiveChannel(_) => {
            "The channel is archived. Its settings cannot be changed."
        }
        AskKind::MoveChannels(_) => MOVE_CHANGED,
        AskKind::AddMember(_) => "The account is banned or removed. It cannot be added.",
        AskKind::RemoveMember(_) => "The account is not a member of this workspace any more.",
        AskKind::RemoveAccount(_) => "The account is removed already.",
        AskKind::BanAccount(_) => "The account is banned or removed already.",
        AskKind::UnbanAccount(_) => "The account is not banned.",
        AskKind::RecoverAccount(_) => "The account is banned or removed. It is not recovered.",
        AskKind::ApproveRequest(_) | AskKind::DenyRequest(_) => {
            "The request was decided already, or it has expired."
        }
        AskKind::RevokeInvite(_) => "The invite is revoked already.",
        AskKind::DeleteRole(_) => "Every member holds the “everyone” role. It is not deleted.",
        AskKind::AssignRole(..) => "The account is banned or removed. It gets no role.",
        AskKind::AddTerm
        | AskKind::ChangeTerm(_)
        | AskKind::RemoveTerm(_)
        | AskKind::MoveTerm(_) => glossary::CHANGED_MEANWHILE,
        _ => {
            "The server refused, because its state has changed. Close this page and open it again."
        }
    }
}

fn limit(what: AskKind) -> &'static str {
    match what {
        AskKind::CreateToken => "Your account holds 100 access tokens. Revoke one first.",
        AskKind::RequestExport => {
            "Your account has 3 exports that wait or 10 stored exports. Remove one first."
        }
        AskKind::CreateRole(_) => "This level has 250 roles, which is the most. Delete one first.",
        AskKind::AssignRole(..) => "The account holds 64 roles, which is the most.",
        AskKind::AddTerm => glossary::FULL,
        _ => "A limit of the server is reached.",
    }
}

fn invalid(what: AskKind) -> &'static str {
    match what {
        AskKind::CreateWorkspace
        | AskKind::RenameWorkspace(_)
        | AskKind::CreateCategory
        | AskKind::UpdateCategory(_)
        | AskKind::CreateChannel
        | AskKind::UpdateChannel(_)
        | AskKind::CreateRole(_)
        | AskKind::UpdateRole(_)
        | AskKind::CreateToken => {
            "The server did not accept the name. A name has 1 to 64 characters and no line break."
        }
        AskKind::ArchivePeriod => ARCHIVE_DAYS_RANGE,
        AskKind::FileRetention | AskKind::RecordingRetention | AskKind::DirectRetention => {
            RETENTION_DAYS_RANGE
        }
        AskKind::CallSettings(_) => "The server did not accept the user limit.",
        AskKind::AttachmentLimits => server::LIMITS_REFUSED,
        AskKind::MoveRole(_) => {
            "The order of the roles has changed. The role is where the list shows it now."
        }
        AskKind::AddTerm
        | AskKind::ChangeTerm(_)
        | AskKind::RemoveTerm(_)
        | AskKind::MoveTerm(_) => glossary::INVALID,
        _ => "The server did not accept the values.",
    }
}

const fn is_role_ask(what: AskKind) -> bool {
    matches!(
        what,
        AskKind::CreateRole(_)
            | AskKind::UpdateRole(_)
            | AskKind::DeleteRole(_)
            | AskKind::MoveRole(_)
            | AskKind::AssignRole(..)
    )
}

const fn is_moderation(what: AskKind) -> bool {
    matches!(
        what,
        AskKind::RemoveAccount(_)
            | AskKind::BanAccount(_)
            | AskKind::UnbanAccount(_)
            | AskKind::RemoveMember(_)
    )
}

const fn is_logo_ask(what: AskKind) -> bool {
    matches!(what, AskKind::SetLogo | AskKind::RemoveLogo)
}

/// Why the server did not do a request of the administration.
pub fn refusal(what: AskKind, error: &ApiError) -> String {
    let code = error.code();
    let fixed = match (code, error.category()) {
        (Some(ErrorCode::ExceedsOwnPermissions), _) => {
            "This would give a permission that your account does not hold."
        }
        (Some(ErrorCode::OwnerOnly), _) => "Only the server owner can do this.",
        (Some(ErrorCode::OwnerProtected), _) => "This cannot be done to the server owner.",
        (Some(ErrorCode::OwnAccountTarget), _) => "This cannot be done to your own account.",
        (Some(ErrorCode::LimitReached), _) => limit(what),
        (Some(ErrorCode::StorageLow), _) => "The server has too little free disk space for this.",
        (Some(ErrorCode::Unavailable), _) if what == AskKind::RequestExport => {
            "This server has no place for exports. Its administrator sets one."
        }
        (Some(ErrorCode::InvalidRequest), _) if is_moderation(what) => people::REASON_REFUSED,
        (Some(ErrorCode::InvalidRequest), _) => invalid(what),
        (Some(ErrorCode::KeyAlreadyRegistered), _) => {
            "This key belongs to a device of an account already. The new device shows the key \
             of its own that it waits with."
        }
        (Some(ErrorCode::InviteNotFound), _) => "The server knows no invite with this code.",
        (Some(ErrorCode::InviteExpired), _) => "The invite has expired.",
        (Some(ErrorCode::InviteExhausted), _) => "The invite has no use left.",
        (Some(ErrorCode::InviteRevoked), _) => "The invite was revoked.",
        (Some(ErrorCode::ImageTooLarge), _) => server::LOGO_TOO_LARGE,
        (Some(ErrorCode::ImageUnsupported), _) => server::LOGO_UNSUPPORTED,
        (_, Some(ErrorCategory::NotAllowed)) if is_logo_ask(what) => server::LOGO_NOT_ALLOWED,
        // The server states a refusal of role order as `forbidden`, without a detail
        // (ADR 0309).
        (_, Some(ErrorCategory::NotAllowed)) if is_role_ask(what) => {
            "Your account may not do this. A role at or above your highest role cannot be \
             changed, moved, given or taken."
        }
        (_, Some(ErrorCategory::NotAllowed)) if is_moderation(what) => {
            "Your account may not do this. An account with a role at or above your highest \
             role cannot be removed, banned or unbanned."
        }
        (_, Some(ErrorCategory::NotAllowed)) if matches!(what, AskKind::MoveChannels(_)) => {
            MOVE_NOT_ALLOWED
        }
        (_, Some(ErrorCategory::NotAllowed)) => "Your account may not do this.",
        (_, Some(ErrorCategory::NotFound)) if matches!(what, AskKind::RemoveMember(_)) => {
            conflict(what)
        }
        (_, Some(ErrorCategory::NotFound)) => "The server holds this no longer.",
        (_, Some(ErrorCategory::Conflict)) => conflict(what),
        _ => return crate::text::flow_error(&FlowError::from(error)),
    };
    fixed.to_owned()
}
