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
pub const OWN_MENU: &str = "Your status";
pub const SET_STATUS_TEXT: &str = "Set a status text…";
pub const CHANGE_STATUS_TEXT: &str = "Change status text…";
pub const CLEAR_STATUS_TEXT: &str = "Clear status text";
pub const STATUS_TEXT: &str = "Status text";
pub const STATUS_TEXT_PLACEHOLDER: &str = "Out of office";
pub const STATUS_TEXT_HELP: &str = "People on this server see it with your status.";
pub const CLEAR_AFTER: &str = "Clear after";
pub const STATUS_TEXT_EMPTY: &str = "Type a status text.";
pub const STATUS_TEXT_TOO_LONG: &str = "A status text has at most 100 characters.";
pub const TEXT_NOT_CHANGED: &str = "Your status text could not be changed.";
pub const SAVE: &str = "Save";
pub const END_DATE: &str = "Date";
pub const END_TIME: &str = "Time";
pub const END_DATE_PLACEHOLDER: &str = "2026-11-30";
pub const END_TIME_PLACEHOLDER: &str = "18:00";
pub const END_NOT_READ: &str = "Type the date as 2026-11-30 and the time as 18:00 or 6:00 PM.";
pub const END_PASSED: &str = "That time has passed. Choose a later one.";
pub const CANCEL: &str = "Cancel";

/// A status text with its end, which the caller cuts at the width of its place.
pub fn with_end(text: &str, end: &str) -> String {
    format!("{text} · until {end}")
}

pub fn end_name(end: crate::state::presence::StatusTextEnd) -> &'static str {
    use crate::state::presence::StatusTextEnd;
    match end {
        StatusTextEnd::Never => "Do not clear",
        StatusTextEnd::HalfHour => "30 minutes",
        StatusTextEnd::Hour => "1 hour",
        StatusTextEnd::FourHours => "4 hours",
        StatusTextEnd::Today => "Today",
        StatusTextEnd::ThisWeek => "This week",
        StatusTextEnd::Custom => "Choose a time",
    }
}
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
