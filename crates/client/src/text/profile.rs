pub const TITLE: &str = "Profile";
pub const OPEN: &str = "Open profile";
pub const READING: &str = "Reading profile…";
pub const UNAVAILABLE: &str = "This profile is unavailable.";
pub const NOT_READ: &str = "The profile could not be read.";
pub const SERVER_ROLES: &str = "Server roles";
pub const NO_ROLES: &str = "No roles to show.";
pub const MESSAGE: &str = "Message";
pub const CALL: &str = "Call";
pub const YOUR_ACCOUNT: &str = "This is your account.";
pub fn open_for(name: &str) -> String {
    format!("Open profile of {name}")
}

pub const KEYBOARD_HINT: &str =
    "Use Left or Right to choose a person, then Enter to open their profile.";

pub const MORE_ROLES: &str = "Show more roles";
