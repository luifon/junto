pub const TITLE: &str = "Quick switcher";
pub const QUERY: &str = "Go to a channel, person, conversation or server";
pub const HELP: &str = "Type a name. Use ↑ and ↓ to choose, Enter to open, Escape to return.";
pub const EMPTY: &str = "No matching destination. Try another name.";
pub const READING: &str = "Looking for people…";
pub const FAILED: &str = "Some servers could not list people. Check the connection and try again.";
pub const LIMITED: &str =
    "Showing the first 500 matches. Type more of the name to narrow the list.";
pub const MORE: &str = "Show more people";
pub const UNREAD: &str = "Unread";

pub fn server_context(server: &str, address: &str) -> String {
    format!("{server} · {address}")
}

pub fn workspace_context(workspace: &str, server: &str, address: &str) -> String {
    format!("{workspace} · {server} · {address}")
}
