//! The text of the pages of the people of a server: accounts, members, invites and join
//! requests (ADR 0761).

use time::OffsetDateTime;

use super::{date, date_time, days};
use crate::state::admin::view::people::{Blocked, GrantFault, InviteState, RoleNames};

// The accounts of the server.
pub const PEOPLE_HELP: &str =
    "Every account of the whole server, with its state and its server roles.";
pub const FILTER: &str = "Accounts that are shown";
pub const FILTER_ALL: &str = "All";
pub const NO_ACCOUNTS: &str = "The server has no account.";
pub const NO_BANNED: &str = "No account is banned.";
pub const BANNED: &str = "Banned";
pub const REMOVED: &str = "Removed";
pub const OWNER: &str = "Server owner";
pub const YOU: &str = "You";
pub const REMOVE: &str = "Remove";
pub const BAN: &str = "Ban";
pub const UNBAN: &str = "Unban";
pub const REMOVAL_HELP: &str = "A removed account loses its devices, its sessions, its memberships and its roles. The \
     person can join again with an invite code and gets a new account.";
pub const BAN_HELP: &str = "The keys of a banned account cannot log in or join again. An unban restores the \
     account with its memberships and roles.";

/// The filter that shows the banned accounts, with their number.
pub fn filter_banned(count: usize) -> String {
    format!("Banned ({count})")
}

/// The server roles of an account. Absent for an account without a server role.
pub fn server_roles(roles: &RoleNames) -> Option<String> {
    match roles {
        RoleNames::Named(names) if names.is_empty() => None,
        RoleNames::Named(names) => Some(format!("Server roles: {}", names.join(", "))),
        RoleNames::Count(0) => None,
        RoleNames::Count(1) => Some("1 server role".to_owned()),
        RoleNames::Count(count) => Some(format!("{count} server roles")),
    }
}

/// The second line of the row of an account: its server roles, and the day on which it
/// was created.
pub fn account_detail(roles: &RoleNames, created: OffsetDateTime) -> String {
    let created = format!("Created on {}", date(created));
    match server_roles(roles) {
        Some(roles) => format!("{roles} · {created}"),
        None => created,
    }
}

/// Why an account cannot be removed, banned or unbanned.
pub const fn blocked(reason: Blocked) -> &'static str {
    match reason {
        Blocked::Owner => "The server owner cannot be removed or banned.",
        Blocked::OwnAccount => "You cannot remove or ban your own account.",
        Blocked::Removed => "The account is removed.",
        Blocked::Banned => "The account is banned already.",
        Blocked::NotBanned => "The account is not banned.",
        Blocked::Outranked => OUTRANKED,
        Blocked::OrderNotRead => "The order of the server roles is on its way.",
    }
}

/// The sentence in the row of an account that role order protects (ADR 0309).
pub const OUTRANKED: &str = "This account holds a server role at or above your highest server role. You cannot \
     remove, ban or unban it.";

pub fn remove_question(name: &str) -> String {
    format!(
        "Remove “{name}” from the server? The account loses its devices, its sessions, its \
         memberships and its roles. The person can join again with an invite code and gets a \
         new account."
    )
}

pub fn ban_question(name: &str) -> String {
    format!(
        "Ban “{name}”? The keys of the account cannot log in or join again. Its invites are \
         cancelled, and its sessions and access tokens end. An unban restores the account with \
         its memberships and roles. The cancelled invites stay cancelled."
    )
}

// The members of a workspace.
pub const NO_MEMBERS: &str = "This workspace has no member.";
pub const MEMBERS_NOT_READ: &str = "The members could not be read.";
pub const NOT_MEMBERS: &str = "Accounts that are not members";
pub const ADD: &str = "Add to workspace";
pub const ADD_HELP: &str = "An administrator can add an active account of the server to this workspace. The \
     account gets no role.";
pub const ALL_ARE_MEMBERS: &str =
    "Every active account of the server is a member of this workspace.";
pub const ADD_NEEDS: &str = "Only an administrator can add an account to this workspace. Other accounts enter it \
     with an invite code.";

pub fn members_help(workspace: &str) -> String {
    format!("The members of the workspace “{workspace}” with their roles in it.")
}

// The invites.
pub const INVITES_HELP: &str = "An invite code lets a person join the server. An account of the server enters further \
     workspaces with it.";
pub const NEW_INVITE: &str = "New invite";
pub const CREATE_INVITE: &str = "Create invite";
pub const EXPIRY: &str = "Expires after";
pub const USES: &str = "Number of uses";
pub const USES_OTHER: &str = "Other";
pub const USES_PLACEHOLDER: &str = "1 to 10000";
pub const USES_RANGE: &str = "Type a number of uses from 1 to 10000.";
pub const WORKSPACES: &str = "Workspaces";
pub const DEFAULT_WORKSPACE: &str = "Default workspace";
pub const INVITE_GRANTS_HELP: &str =
    "A person who joins with the code becomes a member of these workspaces.";
pub const CHOOSE_WORKSPACE: &str =
    "Choose at least one workspace. Your account is not a member of the default workspace.";
pub const TOO_MANY_WORKSPACES: &str = "Choose at most 100 workspaces.";
pub const NO_WORKSPACE_TO_GIVE: &str =
    "Your account is a member of no workspace, so it can give none.";
pub const CODE_ONCE: &str = "This is the code of the new invite. It is not shown again. Copy it now and send it to \
     the person.";
pub const COPY: &str = "Copy";
pub const COPIED: &str = "Copied";
pub const DONE: &str = "Done";
pub const INVITES: &str = "Invites of the server";
pub const NO_INVITES: &str = "The server has no invite.";
pub const REVOKE: &str = "Revoke";
pub const NEW_CODE_MARK: &str = "New";
pub const NEW_CODE_LABEL: &str = "The code of the new invite";
pub const NO_WORKSPACE_LEFT: &str =
    "Every workspace of the invite was deleted. The server refuses its code.";

/// A lifetime that the form of a new invite offers.
pub fn expiry_days(count: u32) -> String {
    days(Some(count))
}

/// What a request gives when the person chose no workspace and may give the default
/// workspace.
pub fn default_is_given(name: &str) -> String {
    format!("No workspace is chosen: the default workspace “{name}” is given.")
}

pub const fn grant_fault(fault: GrantFault) -> &'static str {
    match fault {
        GrantFault::NoneChosen => CHOOSE_WORKSPACE,
        GrantFault::TooMany => TOO_MANY_WORKSPACES,
    }
}

/// Who created an invite. Without a name: the state of this device does not hold it.
pub fn invite_creator(name: Option<&str>) -> String {
    match name {
        Some(name) => format!("Created by {name}"),
        None => "Created by an account whose name is not known here".to_owned(),
    }
}

pub fn invite_times(created: OffsetDateTime, expires: OffsetDateTime) -> String {
    format!(
        "Created on {} · Expires on {}",
        date_time(created),
        date_time(expires)
    )
}

pub fn invite_uses(uses: u32, max_uses: u32) -> String {
    match max_uses {
        1 => format!("{uses} of 1 use"),
        max_uses => format!("{uses} of {max_uses} uses"),
    }
}

/// The third line of the row of an invite: its uses and its workspaces.
pub fn invite_detail(uses: u32, max_uses: u32, names: &[String], other: usize) -> String {
    format!(
        "{} · {}",
        invite_uses(uses, max_uses),
        invite_workspaces(names, other)
    )
}

/// The workspaces of an invite: the names that the state holds, and the number of the
/// others.
pub fn invite_workspaces(names: &[String], other: usize) -> String {
    match (names.is_empty(), other) {
        (true, 0) => NO_WORKSPACE_LEFT.to_owned(),
        (true, other) => format!("Workspaces: {other} other"),
        (false, 0) => format!("Workspaces: {}", names.join(", ")),
        (false, other) => format!("Workspaces: {} and {other} other", names.join(", ")),
    }
}

pub const fn invite_state(state: InviteState) -> &'static str {
    match state {
        InviteState::Usable => "Usable",
        InviteState::Expired => "Expired",
        InviteState::UsedUp => "Used up",
        InviteState::Revoked => "Revoked",
        InviteState::CreatorBanned => "Revoked: creator banned",
    }
}

// The join requests.
pub const REQUESTS_HELP: &str = "People who asked to join without an invite code. An approval creates the account. A \
     request expires after 7 days.";
pub const APPROVAL_WORKSPACES: &str = "Workspaces of an approval";
pub const APPROVAL_GRANTS_HELP: &str = "An approved person becomes a member of these workspaces.";
pub const WAITING: &str = "Requests that wait";
pub const NO_REQUESTS: &str = "No join request waits.";
pub const APPROVE: &str = "Approve";
pub const DENY: &str = "Deny";

pub fn request_times(created: OffsetDateTime, expires: OffsetDateTime) -> String {
    format!(
        "Asked on {} · Expires on {}",
        date_time(created),
        date_time(expires)
    )
}
