//! The text of the pages "Permissions" of a channel and of a category, of the workspace
//! permissions of a member, of what a member can use, and of the server permissions of an
//! account (ADR 0301, ADR 0304, ADR 0306).

use junto_client_core::protocol::{ServerPermission, WorkspacePermission};

use crate::assets::Icon;
use crate::state::admin::PersonPage;
use crate::state::admin::view::overrides::{Choice, Locked, Refused, SubjectKind, SyncedCount};

pub const HELP: &str = "For each role and each person, a permission is allowed in this channel, denied in it, \
     or left neutral: the workspace and the other rules decide it.";
pub const CATEGORY_HELP: &str = "For each role and each person, a permission is allowed in the channels of this \
     category, denied in them, or left neutral.";
pub const ORDER: &str = "The rule of the “everyone” role applies first, then every deny of the roles of a member, \
     then every allow of those roles, then the deny and the allow of the member.";
pub const ARCHIVED: &str = "The channel is archived. Its permissions can still be changed.";
pub const SUBJECTS: &str = "Roles and people";
pub const EVERY_MEMBER: &str = "Every member";
pub const ROLE: &str = "Role";
pub const PERSON: &str = "Person";
pub const FORMER_MEMBER: &str = "Former member";
pub const SHOW: &str = "Show";
pub const HIDE: &str = "Hide";
pub const REMOVE: &str = "Remove rule";
pub const REMOVE_HELP: &str = "Every permission of this rule returns to “Neutral”.";
pub const ADD: &str = "Add a role or a person";
pub const ADD_HELP: &str = "Type a part of the name of a role or of a member of the workspace.";
pub const SEARCH: &str = "Role or member";
pub const ADD_ONE: &str = "Add";
pub const NO_MATCH: &str = "No role and no member without a rule has this name.";
pub const MEMBERS_READING: &str = "The members of the workspace are on their way.";
pub const PEOPLE_FULL: &str =
    "This place has rules for 250 people, which is the most. Remove one to add a person.";
pub const UNKNOWN_MEMBER: &str = "A person who is not a member any more";
pub const SYNC_AGAIN: &str = "Sync again";
pub const SYNC_AGAIN_CONFIRM: &str = "Sync";

/// The line of a channel in a category: whether it takes the rules of the category.
pub fn sync_line(category: &str, synced: bool) -> String {
    if synced {
        format!("Synced with {category}")
    } else {
        format!("Not synced with {category}")
    }
}

/// What the line of the sync means.
pub fn sync_help(category: &str, synced: bool) -> String {
    if synced {
        format!(
            "This channel takes the rules of the category “{category}”. A change here gives the \
             channel its own rules, and it is synced no longer."
        )
    } else {
        format!(
            "This channel has its own rules. “Sync again” gives it the rules of the category \
             “{category}”."
        )
    }
}

/// The question before a sync.
pub fn sync_question(category: &str) -> String {
    format!(
        "Sync with “{category}”? The own rules of this channel are removed, and it takes the \
         rules of the category."
    )
}

/// How many channels of a category take its rules.
pub fn synced_count(count: SyncedCount) -> String {
    match count {
        SyncedCount::Reading => {
            "The rules of the channels of this category are on their way.".to_owned()
        }
        SyncedCount::Counted { channels: 0, .. } => "This category has no channel.".to_owned(),
        SyncedCount::Counted {
            synced,
            channels: 1,
        } => match synced {
            1 => "The 1 channel of this category takes these rules.".to_owned(),
            _ => "The 1 channel of this category has its own rules.".to_owned(),
        },
        SyncedCount::Counted { synced, channels } => {
            format!("{synced} of the {channels} channels of this category take these rules.")
        }
    }
}

pub const SYNCED_HELP: &str = "A synced channel takes these rules. A channel with its own rules keeps them until \
     “Sync again” on its page.";

pub const fn kind(kind: SubjectKind) -> &'static str {
    match kind {
        SubjectKind::Everyone => EVERY_MEMBER,
        SubjectKind::Role => ROLE,
        SubjectKind::Person => PERSON,
    }
}

/// The button that shows the permissions of a role or a person, in the accessibility tree.
pub fn show_subject(name: &str) -> String {
    format!("Show the permissions of “{name}”")
}

/// The button that puts the permissions of a role or a person away, in the accessibility
/// tree.
pub fn hide_subject(name: &str) -> String {
    format!("Hide the permissions of “{name}”")
}

/// The button that adds a role or a person, in the accessibility tree.
pub fn add_subject(name: &str, kind: SubjectKind) -> String {
    match kind {
        SubjectKind::Person => format!("Add the person “{name}”"),
        SubjectKind::Everyone | SubjectKind::Role => format!("Add the role “{name}”"),
    }
}

/// The number of further matches that the search does not list.
pub fn more_matches(count: usize) -> String {
    match count {
        1 => "1 more name matches. Type more of the name.".to_owned(),
        count => format!("{count} more names match. Type more of the name."),
    }
}

/// What a rule holds: "2 allowed, 1 denied".
pub fn summary(allowed: usize, denied: usize) -> String {
    match (allowed, denied) {
        (0, 0) => "No rule".to_owned(),
        (allowed, 0) => format!("{allowed} allowed"),
        (0, denied) => format!("{denied} denied"),
        (allowed, denied) => format!("{allowed} allowed, {denied} denied"),
    }
}

pub const fn choice(choice: Choice) -> &'static str {
    match choice {
        Choice::Allow => "Allow",
        Choice::Neutral => "Neutral",
        Choice::Deny => "Deny",
    }
}

/// The word of a choice in the name of its button, for a test.
pub const fn choice_word(choice: Choice) -> &'static str {
    match choice {
        Choice::Allow => "allow",
        Choice::Neutral => "neutral",
        Choice::Deny => "deny",
    }
}

/// One choice of one permission in the accessibility tree: "Mute members: Allow, chosen".
pub fn choice_label(permission: &str, of: Choice, chosen: bool) -> String {
    let of = choice(of);
    if chosen {
        format!("{permission}: {of}, chosen")
    } else {
        format!("{permission}: {of}")
    }
}

/// Where the grant ceiling of a choice applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    Channel,
    Category,
    Workspace,
    Server,
}

const fn place_words(place: Place) -> &'static str {
    match place {
        Place::Channel => "in this channel",
        Place::Category => "in this category",
        Place::Workspace => "in this workspace",
        Place::Server => "on this server",
    }
}

/// Why a rule or the permissions of a person take no change (ADR 0306, ADR 0309).
pub const fn locked(locked: Locked) -> &'static str {
    match locked {
        Locked::Outranked => {
            "This member holds a role at or above your highest role, so you cannot change \
             their permissions."
        }
        Locked::OrderNotRead => "The order of the roles is on its way.",
        Locked::OwnAccount => "You cannot change your own permissions here.",
        Locked::Owner => "The account of the server owner takes no permission rule.",
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
        WorkspacePermission::AdmitToFullCall => "Admit to full call",
        WorkspacePermission::SendMessages => "Send messages",
        WorkspacePermission::SendMessagesInThreads => "Send messages in threads",
        WorkspacePermission::CreateThreads => "Create threads",
        WorkspacePermission::ReadMessageHistory => "Read message history",
        WorkspacePermission::AttachFiles => "Attach files",
        WorkspacePermission::AddReactions => "Add reactions",
        WorkspacePermission::MentionRoles => "Mention roles",
        WorkspacePermission::Connect => "Connect",
        WorkspacePermission::Speak => "Speak",
        WorkspacePermission::Video => "Video",
        WorkspacePermission::ShareScreen => "Share screen",
        WorkspacePermission::UseVoiceActivity => "Use voice activity",
        WorkspacePermission::SetCallUserLimit => "Set call user limit",
        WorkspacePermission::SetVoiceChannelStatus => "Set voice channel status",
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
        WorkspacePermission::SendMessages => "Send messages in the channel.",
        WorkspacePermission::SendMessagesInThreads => "Send replies in threads.",
        WorkspacePermission::CreateThreads => "Start threads in this channel.",
        WorkspacePermission::ReadMessageHistory => "Read stored messages, threads and pins.",
        WorkspacePermission::AttachFiles => "Attach files to messages you may send.",
        WorkspacePermission::AddReactions => "Add reactions to messages.",
        WorkspacePermission::MentionRoles => "Mention workspace roles in messages you may send.",
        WorkspacePermission::Connect => "Join the call in this channel.",
        WorkspacePermission::Speak => "Publish microphone sound in the call.",
        WorkspacePermission::Video => "Publish your camera in the call.",
        WorkspacePermission::ShareScreen => {
            "Share a screen or a window in the call. Its sound also needs “speak”."
        }
        WorkspacePermission::UseVoiceActivity => {
            "Use the microphone without holding the push-to-talk key."
        }
        WorkspacePermission::SetCallUserLimit => {
            "Change the number of people who may join the call."
        }
        WorkspacePermission::SetVoiceChannelStatus => {
            "Change the status text of the voice channel."
        }
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
        WorkspacePermission::AdmitToFullCall => {
            "Get the requests to join the call in this channel while it is full, and let the \
             person in above the user limit or decline."
        }
    }
}

/// Why the person may not make a choice: nobody gives what they do not hold, nobody lets
/// a person see what they cannot see, and only the owner denies the administrator
/// permission (ADR 0306).
pub fn refused(reason: Refused, permission: &str, place: Place, view_channel: bool) -> String {
    let quoted = format!("“{}”", permission.to_lowercase());
    let at = place_words(place);
    match reason {
        Refused::Allows if view_channel => format!(
            "You do not have the {quoted} permission {at}, so you cannot let anyone see what \
             you cannot see."
        ),
        Refused::Allows => {
            format!("You do not have the {quoted} permission {at}, so you cannot allow it.")
        }
        Refused::LiftsDeny => {
            format!("You do not have the {quoted} permission {at}, so you cannot remove this deny.")
        }
        Refused::OwnerOnly => format!("Only the server owner can deny the {quoted} permission."),
    }
}

/// Why the person may not remove a rule: it denies these permissions, and the removal
/// would give them (ADR 0306).
pub fn removal_refused(permissions: &[WorkspacePermission], place: Place) -> String {
    let at = place_words(place);
    match permissions {
        [permission] => format!(
            "The rule denies {}, which you do not have {at}, so you cannot remove the rule.",
            quoted(*permission)
        ),
        permissions => {
            let names: Vec<String> = permissions.iter().copied().map(quoted).collect();
            format!(
                "The rule denies permissions that you do not have {at} ({}), so you cannot \
                 remove the rule.",
                names.join(", ")
            )
        }
    }
}

/// The text of the workspace permissions of one member.
pub const MEMBER_PERMISSIONS: &str = "Permissions";
pub fn member_help(workspace: &str) -> String {
    format!(
        "Each permission is allowed to this member in the workspace “{workspace}”, denied, or \
         left neutral: the roles of the member decide it. The rules of a category or a channel \
         apply after these."
    )
}
pub const MEMBER_CHANNELS: &str = "The server also checks each channel: a choice that gives the member a permission in a \
     channel where you do not hold it is refused.";
pub const ACCESS: &str = "What this person can use";
pub fn access_help(workspace: &str) -> String {
    format!(
        "The permissions that this member holds in each channel of the workspace “{workspace}” \
         that you can see. A channel that you cannot see is not listed."
    )
}
pub const ACCESS_NONE: &str = "Does not see this channel.";
pub const ACCESS_EMPTY: &str = "You see no channel of this workspace.";
pub const SERVER_PERMISSIONS: &str = "Server permissions";
pub const SERVER_HELP: &str = "Each server permission is allowed to this account, denied, or left neutral: the server \
     roles of the account decide it.";
pub const BACK: &str = "Back";
pub const PERSON_SETTINGS: &str = "Settings of a person";
pub const CATEGORY_SETTINGS: &str = "Category settings";
pub const PERMISSIONS: &str = "Permissions";

/// The row of the settings of a category or a person that returns to the page that opened
/// them.
pub fn back_to(page: &str) -> String {
    format!("Back to {page}")
}

pub const fn person_page_name(page: PersonPage) -> &'static str {
    match page {
        PersonPage::Permissions(_) => MEMBER_PERMISSIONS,
        PersonPage::Access(_) => ACCESS,
        PersonPage::Server => SERVER_PERMISSIONS,
    }
}

pub const fn person_page_icon(page: PersonPage) -> Icon {
    match page {
        PersonPage::Permissions(_) | PersonPage::Server => Icon::ShieldCheck,
        PersonPage::Access(_) => Icon::Hash,
    }
}

/// The name of the row of a page of the settings of a person, for a test.
pub const fn person_page_selector(page: PersonPage) -> &'static str {
    match page {
        PersonPage::Permissions(_) => "person-nav-permissions",
        PersonPage::Access(_) => "person-nav-access",
        PersonPage::Server => "person-nav-server",
    }
}

/// The permissions of a member in one channel, by their names.
pub fn access_line(permissions: &[WorkspacePermission]) -> String {
    if permissions.is_empty() {
        return ACCESS_NONE.to_owned();
    }
    permissions
        .iter()
        .map(|permission| name(*permission))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The name of a server permission.
pub const fn server_name(permission: ServerPermission) -> &'static str {
    super::roles::server_permission_name(permission)
}

/// What a server permission lets an account do.
pub const fn server_help(permission: ServerPermission) -> &'static str {
    super::roles::server_permission_help(permission)
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
    fn the_summary_of_a_rule_counts_its_allows_and_denies() {
        assert_eq!(summary(0, 0), "No rule");
        assert_eq!(summary(2, 0), "2 allowed");
        assert_eq!(summary(0, 1), "1 denied");
        assert_eq!(summary(2, 1), "2 allowed, 1 denied");
    }

    #[test]
    fn a_refused_choice_names_the_permission_the_place_and_the_rule() {
        assert_eq!(
            refused(Refused::Allows, "Mute members", Place::Channel, false),
            "You do not have the “mute members” permission in this channel, so you cannot \
             allow it."
        );
        assert_eq!(
            refused(Refused::LiftsDeny, "Mute members", Place::Category, false),
            "You do not have the “mute members” permission in this category, so you cannot \
             remove this deny."
        );
        assert_eq!(
            refused(Refused::Allows, "View channel", Place::Workspace, true),
            "You do not have the “view channel” permission in this workspace, so you cannot \
             let anyone see what you cannot see."
        );
        assert_eq!(
            refused(Refused::OwnerOnly, "Administrator", Place::Server, false),
            "Only the server owner can deny the “administrator” permission."
        );
        assert_eq!(
            removal_refused(&[WorkspacePermission::ViewChannel], Place::Channel),
            "The rule denies “view channel”, which you do not have in this channel, so you \
             cannot remove the rule."
        );
        assert_eq!(
            removal_refused(
                &[
                    WorkspacePermission::ViewChannel,
                    WorkspacePermission::MuteMembers
                ],
                Place::Category
            ),
            "The rule denies permissions that you do not have in this category (“view \
             channel”, “mute members”), so you cannot remove the rule."
        );
    }

    #[test]
    fn the_sync_line_names_the_category_and_the_count_names_the_channels() {
        assert_eq!(sync_line("Teams", true), "Synced with Teams");
        assert_eq!(sync_line("Teams", false), "Not synced with Teams");
        assert_eq!(
            synced_count(SyncedCount::Counted {
                synced: 2,
                channels: 3
            }),
            "2 of the 3 channels of this category take these rules."
        );
        assert_eq!(
            synced_count(SyncedCount::Counted {
                synced: 0,
                channels: 0
            }),
            "This category has no channel."
        );
    }

    #[test]
    fn the_access_of_a_member_names_each_permission_in_order() {
        assert_eq!(
            access_line(&[
                WorkspacePermission::ViewChannel,
                WorkspacePermission::SendMessages
            ]),
            "View channel, Send messages"
        );
        assert_eq!(access_line(&[]), ACCESS_NONE);
    }
}
