pub const PIN: &str = "Pin person for me";
pub const UNPIN: &str = "Unpin person";
pub const SHOW_PERSON: &str = "Show pinned person";
pub const SHOW_SHARES: &str = "Show screen shares";
pub const LAYOUT_HELP: &str = "Unpin the person to return to the automatic layout.";
pub fn pinned(name: &str) -> String {
    format!("Pinned for you: {name}")
}
