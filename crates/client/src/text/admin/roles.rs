//! The text of the pages of the roles (ADR 0301, ADR 0309): the roles of the server and of
//! a workspace, their order, their permissions, and the roles of an account.

use junto_client_core::protocol::ServerPermission;

use crate::state::admin::roles::{RoleLevel, RolePermission};
use crate::state::admin::view::roles::{
    Fixed, Locked, NoAssignment, NoCreation, NoDeletion, Standing,
};

pub const SERVER_HELP: &str = "The server roles hold the permissions that act on the whole server. A role higher in \
     the list stands above the roles below it.";
pub const ORDER_HELP: &str = "A person changes, moves, gives and takes only the roles below the highest role that \
     the person holds. The server owner stands above every role.";
pub const DRAG_HELP: &str =
    "Drag a role by its handle onto another role to move it there, or use “Up” and “Down”.";
pub const NEW_ROLE: &str = "New role";
pub const ROLE_NAME: &str = "Name of the role";
pub const NEW_ROLE_HELP: &str = "A new role has no permission and is the lowest role. Give it permissions and move it \
     afterwards.";
pub const NO_ROLES: &str = "The server has no role.";
pub const MOVE_UP: &str = "Move up";
pub const MOVE_DOWN: &str = "Move down";
pub const PERMISSIONS: &str = "Permissions";
pub const HIDE_PERMISSIONS: &str = "Hide permissions";
pub const EVERY_MEMBER: &str = "Every member";
pub const YOUR_HIGHEST: &str = "Your highest role";
pub const YOU_HOLD: &str = "You hold it";
pub const ABOVE_YOU: &str = "Above your role";
pub const HANDLE: &str = "Drag to move the role";
pub const ROLES: &str = "Roles";
pub const HIDE_ROLES: &str = "Hide roles";
pub const NO_ROLE_TO_GIVE: &str = "No role exists that could be given.";
pub const ORDER_READING: &str = "The order of the roles is on its way.";
pub const ASSIGN_HELP: &str = "Giving a role gives its permissions. A role at or above your highest role cannot be \
     given or taken.";

pub fn workspace_help(workspace: &str) -> String {
    format!(
        "The roles of the workspace “{workspace}”. A role higher in the list stands above the \
         roles below it. A member holds the permissions of every role that the member has."
    )
}

/// Where the person stands among the roles of a level, and what follows from it.
pub fn standing(standing: &Standing, level: RoleLevel) -> String {
    match (standing, level) {
        (Standing::Owner, _) => {
            "You own the server. You stand above every role and can change each one.".to_owned()
        }
        (Standing::Administrator, _) => {
            "You are an administrator. An administrator stands above every role of a workspace \
             and can change each one."
                .to_owned()
        }
        (Standing::Role(name), _) => format!(
            "Your highest role here is “{name}”. You can change the roles below it. You cannot \
             change “{name}” or a role above it."
        ),
        (Standing::NoRole, RoleLevel::Server) => {
            "You hold no server role, so no role stands below you. You cannot change a role \
             or create one."
                .to_owned()
        }
        (Standing::NoRole, RoleLevel::Workspace(_)) => {
            "You hold no role of this workspace, so no role stands below you. You cannot \
             change a role or create one."
                .to_owned()
        }
    }
}

/// Why the person cannot act on a role, as the row of the role states it.
pub const fn locked(reason: Locked) -> &'static str {
    match reason {
        Locked::OwnHighest => {
            "This is your highest role. You cannot change, move, delete, give or take it."
        }
        Locked::Above => {
            "This role stands above your highest role. You cannot change, move, delete, give or \
             take it."
        }
    }
}

/// Why a role does not move one place.
pub const fn fixed(reason: Fixed, up: bool) -> &'static str {
    match reason {
        Fixed::Locked(reason) => locked(reason),
        Fixed::Everyone => {
            "Every member holds this role. It stays the lowest role and does not move."
        }
        Fixed::NoPlaceAbove if up => "No place above this role is below your highest role.",
        Fixed::NoPlaceBelow if !up => "It is the lowest role that can move.",
        Fixed::NoPlaceAbove | Fixed::NoPlaceBelow => "The role cannot move there.",
        Fixed::Unsettled => {
            "The order of the roles is changing. The role can move when it is read."
        }
        Fixed::OutOfRange => {
            "A role here has a position that a move cannot give. The order of these two \
             roles cannot be changed in this window."
        }
    }
}

pub const fn no_creation(reason: NoCreation, level: RoleLevel) -> &'static str {
    match (reason, level) {
        (NoCreation::NoRole, _) => {
            "A new role must stand below your highest role, and you hold no role here."
        }
        (NoCreation::Full, RoleLevel::Server) => {
            "The server has 250 roles, which is the most. Delete one first."
        }
        (NoCreation::NotRead, _) => ORDER_READING,
        (NoCreation::Full, RoleLevel::Workspace(_)) => {
            "The workspace has 250 roles, which is the most. Delete one first."
        }
    }
}

pub const fn no_assignment(reason: NoAssignment) -> &'static str {
    match reason {
        NoAssignment::Locked(Locked::OwnHighest) => {
            "Your highest role. You cannot give or take it."
        }
        NoAssignment::Locked(Locked::Above) => {
            "Above your highest role. You cannot give or take it."
        }
        NoAssignment::BeyondOwn => {
            "The role holds a permission that you do not hold. You cannot give it."
        }
        NoAssignment::Full => "The account holds 64 roles, which is the most.",
        NoAssignment::NotAllowed => {
            "The server states that you cannot give or take this role. Its overrides in \
             channels allow or deny a permission that you do not hold."
        }
        NoAssignment::Reading => STATEMENT_READING,
    }
}

pub const fn no_deletion(reason: NoDeletion) -> &'static str {
    match reason {
        NoDeletion::Locked(locked) => self::locked(locked),
        NoDeletion::NotAllowed => {
            "The server states that you cannot delete this role. An override of the role in a \
             channel denies a permission that you do not hold."
        }
        NoDeletion::Reading => STATEMENT_READING,
    }
}

/// What the server states that the person may do with the roles is read again after a
/// change of the order of the roles or of an override.
pub const STATEMENT_READING: &str = "What you may do with this role is on its way from the server.";

/// The second line of the row of a role.
pub fn summary(granted: usize) -> String {
    match granted {
        0 => "No permission".to_owned(),
        1 => "1 permission".to_owned(),
        count => format!("{count} permissions"),
    }
}

pub fn delete_question(name: &str, level: RoleLevel) -> String {
    match level {
        RoleLevel::Server => format!(
            "Delete the server role “{name}”? Every account that holds it loses it and its \
             permissions. This cannot be undone."
        ),
        RoleLevel::Workspace(_) => format!(
            "Delete the role “{name}”? Every member who holds it loses it, and its overrides in \
             every channel are removed. The server refuses when one of them denies a permission \
             that you do not hold in that channel. This cannot be undone."
        ),
    }
}

pub const EVERYONE_STAYS: &str =
    "Every member holds this role. It is not renamed, deleted, given or taken.";

/// What a permission of a role is called.
pub fn permission_name(permission: RolePermission) -> &'static str {
    match permission {
        RolePermission::Workspace(permission) => super::overrides::name(permission),
        RolePermission::Server(permission) => server_permission_name(permission),
    }
}

/// What a server permission is called.
pub const fn server_permission_name(permission: ServerPermission) -> &'static str {
    match permission {
        ServerPermission::Administrator => "Administrator",
        ServerPermission::CreateWorkspace => "Create workspace",
        ServerPermission::CreateInvites => "Create invites",
        ServerPermission::ApproveJoinRequests => "Approve join requests",
        ServerPermission::BanMembers => "Ban members",
        ServerPermission::ViewServerAuditLog => "View server audit log",
    }
}

/// What a permission lets the holders of a role do.
pub fn permission_help(permission: RolePermission) -> &'static str {
    match permission {
        RolePermission::Workspace(permission) => workspace_help_of(permission),
        RolePermission::Server(permission) => server_permission_help(permission),
    }
}

/// What a server permission lets an account do.
pub const fn server_permission_help(permission: ServerPermission) -> &'static str {
    match permission {
        ServerPermission::Administrator => {
            "Every permission in every workspace, the server roles, the workspaces and the \
             access tokens. It sees every channel, also a private one. It does not open \
             direct messages."
        }
        ServerPermission::CreateWorkspace => "Create a workspace on this server.",
        ServerPermission::CreateInvites => {
            "Create invite codes for the workspaces that the account is a member of."
        }
        ServerPermission::ApproveJoinRequests => {
            "Approve and deny the requests of people who ask to join the server."
        }
        ServerPermission::BanMembers => {
            "Remove, ban and unban accounts that stand below the account in the order of the \
             server roles."
        }
        ServerPermission::ViewServerAuditLog => "Read the audit log of the server.",
    }
}

/// What a workspace permission lets the holders of a role do in every channel of the
/// workspace, unless a channel overrides it.
pub(crate) const fn workspace_help_of(
    permission: junto_client_core::protocol::WorkspacePermission,
) -> &'static str {
    use junto_client_core::protocol::WorkspacePermission as P;
    match permission {
        P::ViewChannel => {
            "See the channels of the workspace; every other permission in a channel needs it."
        }
        P::SendMessages => "Send messages in the channels.",
        P::SendMessagesInThreads => "Send replies in threads.",
        P::CreateThreads => "Start threads under messages.",
        P::ReadMessageHistory => "Read stored messages, threads and pins.",
        P::AttachFiles => "Attach files to messages you may send.",
        P::AddReactions => "Add reactions to messages.",
        P::MentionRoles => "Mention workspace roles in messages you may send.",
        P::Connect => "Join the calls of voice channels.",
        P::Speak => "Publish microphone sound in a call.",
        P::Video => "Publish the camera in a call.",
        P::ShareScreen => "Share a screen or a window in a call. Its sound also needs “speak”.",
        P::UseVoiceActivity => "Use the microphone without holding the push-to-talk key.",
        P::SetCallUserLimit => {
            "Change the user limit of a voice channel, without “manage channel”."
        }
        P::SetVoiceChannelStatus => "Change the status text of a voice channel.",
        P::ManageRoles => {
            "Create, change, delete, give and take the roles below the own highest role."
        }
        P::ManageChannel => {
            "Create, change, archive and delete channels and categories, and set their \
             permission rules."
        }
        P::ManageMessages => "Delete the messages of other people, and pin messages.",
        P::MentionEveryone => "Mention “@everyone” in a message.",
        P::ManageGlossary => "Change the glossary that transcripts of this workspace use.",
        P::RecordCalls => "Start and stop the recording of a call.",
        P::ManageRecordings => "Remove recordings, and correct their transcripts.",
        P::ExportChannel => "Ask for an export of a channel.",
        P::MoveMembers => "Move a person from one voice channel to another.",
        P::MuteMembers => "Block and unblock the microphone of a person in a call.",
        P::DeafenMembers => "Block and unblock the sound of a person in a call.",
        P::DisconnectMembers => "Take a person out of a call.",
        P::LockRoom => "Lock and unlock a call.",
        P::RunMeetingMode => "Turn meeting mode on in a call.",
        P::ViewAuditEntries => "Read the audit entries of the workspace.",
        P::AdmitToFullCall => {
            "Get the requests to join a full call, and let the person in above the user limit."
        }
    }
}

/// The sentence under a permission that the person cannot give to a role.
pub fn beyond_own(permission: RolePermission) -> String {
    format!(
        "You do not hold “{}”, so you cannot give it to a role.",
        permission_name(permission)
    )
}

/// The label of a permission switch for the accessibility tree.
pub fn permission_label(permission: RolePermission, role: &str) -> String {
    format!("{} for the role {role}", permission_name(permission))
}

pub fn move_label(role: &str, up: bool) -> String {
    if up {
        format!("Move the role {role} up")
    } else {
        format!("Move the role {role} down")
    }
}

pub fn roles_of(name: &str) -> String {
    format!("Roles of {name}")
}

pub fn role_label(role: &str, account: &str) -> String {
    format!("The role {role} for {account}")
}

#[cfg(test)]
mod tests {
    use junto_client_core::protocol::WorkspacePermission;

    use super::*;

    #[test]
    fn the_help_of_a_permission_of_a_role_describes_the_workspace() {
        let all = WorkspacePermission::ALL;
        for (index, permission) in all.iter().copied().enumerate() {
            let help = permission_help(RolePermission::Workspace(permission));
            assert!(help.ends_with('.'), "{help}");
            assert!(!help.contains("this channel"), "{help}");
            assert!(!help.contains("overrides"), "{help}");
            for other in all[..index].iter().copied() {
                assert_ne!(help, permission_help(RolePermission::Workspace(other)));
            }
            assert!(!permission_name(RolePermission::Workspace(permission)).is_empty());
        }
        let view = permission_help(RolePermission::Workspace(WorkspacePermission::ViewChannel));
        assert!(!view.contains("write"), "{view}");
        let share = permission_help(RolePermission::Workspace(WorkspacePermission::ShareScreen));
        assert!(share.contains("“speak”"), "{share}");
        for permission in ServerPermission::ALL.iter().copied() {
            assert!(permission_help(RolePermission::Server(permission)).ends_with('.'));
        }
    }
}
