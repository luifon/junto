pub const TITLE: &str = "Group conversation";
pub const NAME: &str = "Conversation name";
pub const PEOPLE: &str = "People";
pub const CLOSE_PEOPLE: &str = "Close the people of this conversation";
pub const PROFILE: &str = "Profile";
pub const START: &str = "Start conversation";
pub const CHOOSE: &str = "Choose";
pub const NAME_OPTIONAL: &str = "Conversation name (optional)";
pub const EXPORT: &str = "Export conversation…";
pub const EXPORT_TITLE: &str = "Export this conversation";
pub const EXPORT_ABOUT: &str = "The server prepares a ZIP file with the messages and files of this conversation, as JSON and as HTML pages. When it is ready, it is listed in Exports of your account on this server, where you download it.";
pub const EXPORT_START: &str = "Export";
pub const CLOSED: &str = "This conversation is closed: you no longer share a workspace with this person.";
/// The name of a group conversation: its name, or the names of its present members when
/// it has none (ADR 0302).
pub fn group_name(name: &str, members: &[String]) -> String {
    let name = name.trim();
    if !name.is_empty() {
        return crate::text::label(name);
    }
    if members.is_empty() {
        return TITLE.to_owned();
    }
    members.join(", ")
}
pub const NEW: &str = "New group conversation";
pub const CREATE: &str = "Create group";
pub const SAVE_NAME: &str = "Save name";
pub const ADD: &str = "Add people";
pub const ADD_SELECTED: &str = "Add selected person";
pub const PEOPLE_HELP: &str =
    "Choose the people in this conversation. Groups have at most ten people.";
pub const NAME_INVALID: &str = "Use a name of at most 80 characters without controls.";
pub const LEAVE: &str = "Leave conversation";
pub const LEAVE_QUESTION: &str =
    "Leave this conversation? Its messages and files will no longer be available to you.";

pub const HIDE: &str = "Hide conversation";
pub const HIDDEN: &str = "Hidden conversations";

pub const OPEN_FAILED: &str = "The conversation could not be opened. Try again.";

pub const MANAGE: &str = "Leave a group";
pub const MEMBERSHIPS_TITLE: &str = "Your group conversations";
pub const MEMBERSHIPS_HELP: &str = "Leave a group here even when its messages are unavailable.";
pub const MEMBERSHIPS_EMPTY: &str = "You are not in any group conversations.";
pub const MEMBERSHIPS_LOADING: &str = "Reading group conversations…";
pub const MEMBERSHIPS_REFRESH: &str = "Refresh groups";
