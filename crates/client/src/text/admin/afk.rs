//! The text of the AFK channel of a workspace in its overview (ADR 0523).

pub const TITLE: &str = "AFK channel";
pub const SHOW: &str = "Show the AFK settings";
pub const MOVE_IDLE: &str = "Move people who are away";
pub const IDLE_TIME: &str = "After this time without speech and without keyboard or mouse input:";
pub const CHANNEL: &str = "Into this voice channel, where people can only listen:";
pub const EXEMPT: &str = "Meeting mode, direct calls and group calls are exempt.";

pub fn minutes(minutes: u32) -> String {
    format!("{minutes} minutes")
}
