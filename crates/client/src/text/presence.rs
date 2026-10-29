use junto_client_core::protocol::presence::{Presence, PresenceChoice};

pub const TITLE: &str = "Your status";
pub const AUTOMATIC: &str = "Online changes to Idle after five minutes without system input.";
pub const INVISIBLE_HELP: &str =
    "Invisible appears offline to other people. Messages and calls still arrive.";
pub const ACTIVITY_UNAVAILABLE: &str =
    "Automatic idle is unavailable on this desktop. You can choose Idle manually.";
pub const CONNECTING: &str = "Connect to the server to change your status.";
pub const CHECKING: &str = "Reading your status…";
pub const NOT_READ: &str = "Presence could not be read.";
pub const NOT_CHANGED: &str = "Your status could not be changed.";
pub const SAVING: &str = "Changing status…";
pub fn label(presence: Presence) -> &'static str {
    match presence {
        Presence::Online => "Online",
        Presence::Idle => "Idle",
        Presence::DoNotDisturb => "Do not disturb",
        Presence::Invisible => "Invisible",
        Presence::Offline => "Offline",
    }
}
pub fn choice(choice: PresenceChoice) -> &'static str {
    match choice {
        PresenceChoice::Online => "Online",
        PresenceChoice::Idle => "Idle",
        PresenceChoice::DoNotDisturb => "Do not disturb",
        PresenceChoice::Invisible => "Invisible",
    }
}

pub fn mark(presence: Presence) -> &'static str {
    match presence {
        Presence::Online => "●",
        Presence::Idle => "◐",
        Presence::DoNotDisturb => "⊖",
        Presence::Invisible | Presence::Offline => "○",
    }
}
