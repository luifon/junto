//! The text of the icon of the application in the menu bar of macOS, the notification area of
//! Windows and the status area of a Linux desktop (ADR 0759).

pub const MUTE_MICROPHONE: &str = "Mute microphone";
pub const UNMUTE_MICROPHONE: &str = "Unmute microphone";
pub const TURN_SOUND_OFF: &str = "Turn sound off";
pub const TURN_SOUND_ON: &str = "Turn sound on";
pub const LEAVE_THE_CALL: &str = "Leave the call";
pub const OPEN_JUNTO: &str = "Open Junto";
pub const QUIT: &str = "Quit";

pub const NOT_IN_A_CALL: &str = "Junto";
pub const IN_A_CALL: &str = "Junto: in a call";
pub const IN_A_CALL_MUTED: &str = "Junto: in a call, microphone muted";
pub const IN_A_CALL_SOUND_OFF: &str = "Junto: in a call, sound off";
pub const IN_A_CALL_HELD: &str = "Junto: in a call, microphone closed by the call";

/// The unread mentions that the tooltip of the icon adds to the state.
pub fn mentions(count: u64) -> String {
    match count {
        1 => "1 unread mention".to_owned(),
        count => format!("{} unread mentions", crate::text::unread_count(count)),
    }
}
