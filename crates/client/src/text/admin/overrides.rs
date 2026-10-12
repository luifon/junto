//! The text of the page "Permissions" of the settings of a channel: the overrides of the
//! roles in one channel (ADR 0301, ADR 0304).

use junto_client_core::protocol::WorkspacePermission;

use crate::state::admin::view::overrides::{Choice, Refused};

pub const HELP: &str = "For each role, a permission is allowed in this channel, denied in it, or left as the \
     workspace gives it.";
pub const MEMBERS: &str = "A member gets the permissions of the roles that the member holds; a permission cannot \
     be set for a single member.";
pub const ORDER: &str = "The choices for the “everyone” role apply first, then every deny of the roles of a \
     member, then every allow of those roles.";
pub const ARCHIVED: &str = "The channel is archived. Its permissions can still be changed.";
pub const ROLES: &str = "Roles";
pub const EVERY_MEMBER: &str = "Every member";
pub const SHOW: &str = "Show";
pub const HIDE: &str = "Hide";
pub const REMOVE: &str = "Remove override";
pub const REMOVE_HELP: &str = "Every permission of the role returns to “Default” in this channel.";

/// The button that shows the permissions of a role, in the accessibility tree.
pub fn show_role(name: &str) -> String {
    format!("Show the permissions of the role “{name}”")
}

/// The button that puts the permissions of a role away, in the accessibility tree.
pub fn hide_role(name: &str) -> String {
    format!("Hide the permissions of the role “{name}”")
}

/// What the override of a role holds: "2 allowed, 1 denied".
pub fn summary(allowed: usize, denied: usize) -> String {
    match (allowed, denied) {
        (0, 0) => "No override".to_owned(),
        (allowed, 0) => format!("{allowed} allowed"),
        (0, denied) => format!("{denied} denied"),
        (allowed, denied) => format!("{allowed} allowed, {denied} denied"),
    }
}

pub const fn choice(choice: Choice) -> &'static str {
    match choice {
        Choice::Allow => "Allow",
        Choice::Default => "Default",
        Choice::Deny => "Deny",
    }
}

/// One choice of one permission in the accessibility tree: "Mute members: Allow, chosen".
pub fn choice_label(permission: WorkspacePermission, of: Choice, chosen: bool) -> String {
    let (permission, of) = (name(permission), choice(of));
    if chosen {
        format!("{permission}: {of}, chosen")
    } else {
        format!("{permission}: {of}")
    }
}

pub const fn name(permission: WorkspacePermission) -> &'static str {
    match permission {
        WorkspacePermission::ViewChannel => "View channel",
        WorkspacePermission::ManageRoles => "Manage roles",
        WorkspacePermission::ManageChannel => "Manage channel",
        WorkspacePermission::ManageMessages => "Manage messages",
        WorkspacePermission::MentionEveryone => "Mention everyone",
        WorkspacePermission::ManageGlossary => "Manage glossary",
        WorkspacePermission::RecordCalls => "Record calls",
        WorkspacePermission::ManageRecordings => "Manage recordings",
        WorkspacePermission::ExportChannel => "Export channel",
        WorkspacePermission::MoveMembers => "Move members",
        WorkspacePermission::MuteMembers => "Mute members",
        WorkspacePermission::DeafenMembers => "Deafen members",
        WorkspacePermission::DisconnectMembers => "Disconnect members",
        WorkspacePermission::LockRoom => "Lock room",
        WorkspacePermission::RunMeetingMode => "Run meeting mode",
        WorkspacePermission::ViewAuditEntries => "View audit entries",
    }
}

/// A permission as a sentence names it: `“mute members”`.
fn quoted(permission: WorkspacePermission) -> String {
    format!("“{}”", name(permission).to_lowercase())
}

/// What a permission allows in the channel. The server checks "manage roles" and "manage
/// glossary" against the roles of the workspace only, so an override of them does not
/// change who can use them. It reads the audit entries about a channel for a person who
/// holds "view audit entries" in the workspace and in the channel.
pub const fn help(permission: WorkspacePermission) -> &'static str {
    match permission {
        WorkspacePermission::ViewChannel => {
            "See this channel and read it; every other permission in the channel needs it."
        }
        WorkspacePermission::ManageRoles => {
            "Create, change, assign and delete the roles of the workspace; the server checks \
             it in the workspace, so a choice here does not change who can do this."
        }
        WorkspacePermission::ManageChannel => {
            "Change the settings and the permissions of this channel, archive it, restore it \
             and delete it."
        }
        WorkspacePermission::ManageMessages => {
            "Delete the messages of other people in this channel, and pin messages."
        }
        WorkspacePermission::MentionEveryone => "Mention “@everyone” in a message in this channel.",
        WorkspacePermission::ManageGlossary => {
            "Change the glossary of the workspace for transcripts; the server checks it in \
             the workspace, so a choice here does not change who can do this."
        }
        WorkspacePermission::RecordCalls => {
            "Start and stop a recording of the call in this channel."
        }
        WorkspacePermission::ManageRecordings => {
            "Remove a recording of this channel, and ask for its transcript again."
        }
        WorkspacePermission::ExportChannel => "Ask for an export of this channel.",
        WorkspacePermission::MoveMembers => {
            "Move a person from the call in this channel into another voice channel; the \
             permission is needed in both channels."
        }
        WorkspacePermission::MuteMembers => {
            "Close the microphone of another person in the call in this channel."
        }
        WorkspacePermission::DeafenMembers => {
            "Turn the call sound off for another person in the call in this channel."
        }
        WorkspacePermission::DisconnectMembers => {
            "Remove another person from the call in this channel."
        }
        WorkspacePermission::LockRoom => "Lock and unlock the call in this channel.",
        WorkspacePermission::RunMeetingMode => "Turn meeting mode on in the call in this channel.",
        WorkspacePermission::ViewAuditEntries => {
            "Read the audit entries about this channel; a member also needs this permission \
             in the workspace."
        }
    }
}

/// Why the person may not make a choice: the grant rule (ADR 0304).
pub fn refused(reason: Refused, permission: WorkspacePermission) -> String {
    let permission = quoted(permission);
    match reason {
        Refused::Allows => format!(
            "You do not have the {permission} permission in this channel, so you cannot \
             allow it."
        ),
        Refused::LiftsDeny => format!(
            "You do not have the {permission} permission in this channel, so you cannot \
             remove this deny."
        ),
    }
}

/// Why the person may not remove an override: it denies these permissions, and the
/// removal would give them (ADR 0304).
pub fn removal_refused(permissions: &[WorkspacePermission]) -> String {
    match permissions {
        [permission] => format!(
            "The override denies {}, which you do not have in this channel, so you cannot \
             remove the override.",
            quoted(*permission)
        ),
        permissions => {
            let names: Vec<String> = permissions.iter().copied().map(quoted).collect();
            format!(
                "The override denies permissions that you do not have in this channel ({}), \
                 so you cannot remove the override.",
                names.join(", ")
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_permission_has_a_name_and_a_help_text_of_its_own() {
        let all = WorkspacePermission::ALL;
        for (index, permission) in all.iter().copied().enumerate() {
            assert_eq!(
                name(permission).to_lowercase(),
                permission.name().replace('_', " ")
            );
            assert!(help(permission).ends_with('.'));
            for other in all[..index].iter().copied() {
                assert_ne!(help(permission), help(other));
            }
        }
    }

    #[test]
    fn the_summary_of_an_override_counts_its_allows_and_denies() {
        assert_eq!(summary(0, 0), "No override");
        assert_eq!(summary(2, 0), "2 allowed");
        assert_eq!(summary(0, 1), "1 denied");
        assert_eq!(summary(2, 1), "2 allowed, 1 denied");
    }

    #[test]
    fn a_refused_choice_names_the_permission_and_the_rule() {
        assert_eq!(
            refused(Refused::Allows, WorkspacePermission::MuteMembers),
            "You do not have the “mute members” permission in this channel, so you cannot \
             allow it."
        );
        assert_eq!(
            refused(Refused::LiftsDeny, WorkspacePermission::ViewChannel),
            "You do not have the “view channel” permission in this channel, so you cannot \
             remove this deny."
        );
        assert_eq!(
            removal_refused(&[WorkspacePermission::ViewChannel]),
            "The override denies “view channel”, which you do not have in this channel, so \
             you cannot remove the override."
        );
        assert_eq!(
            removal_refused(&[
                WorkspacePermission::ViewChannel,
                WorkspacePermission::MuteMembers
            ]),
            "The override denies permissions that you do not have in this channel (“view \
             channel”, “mute members”), so you cannot remove the override."
        );
    }
}
