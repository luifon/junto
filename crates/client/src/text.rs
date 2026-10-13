//! The catalog of the interface text (ADR 0703). The interface language is English; a
//! second language is a second catalog. No other module holds text that a person reads.
//!
//! The text of a failure names what happened and what the person can do. It holds no text
//! of a server, except the quoted sentence of a refusal with a code that this build does
//! not know (ADR 0712).

use junto_client_core::api::{TransportErrorKind, code_name};
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::Notice;
use junto_client_core::protocol::call::{BlockReason, LeaveReason, ShareAudio};
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::version::Incompatibility;
use junto_client_core::voice::{ShareResolution, TalkKey};
use junto_media::calls::EndReason;
use junto_media::permission::MicrophoneAccess;
use junto_media::share::{CaptureError, ShareKind};

use crate::state::view::{
    CallNotice, Closed, RecordingFault, RecordingProblem, ShareCard, SoundWarning,
};

pub mod account;
pub mod admin;
pub mod book;
pub mod people;

pub const APP_NAME: &str = "Junto";
pub const QUIT: &str = "Quit Junto";

// The server book.
pub const SERVER_BOOK: &str = "Server book";
pub const COLLAPSE_SERVER_BOOK: &str = "Collapse the server book to icons";
pub const EXPAND_SERVER_BOOK: &str = "Expand the server book";
pub const ADD_SERVER: &str = "Add server";
pub const ADD_SERVER_BY_ADDRESS: &str = "Add server by address";
pub const ADDRESS_PLACEHOLDER: &str = "host:port";
pub const ADDRESS_FIELD: &str = "Server address";
pub const ADDRESS_HELP: &str = "Enter the address your admin gave you.";
pub const ADDRESS_ASKING: &str = "Asking the server…";
pub const WORKSPACES: &str = "Workspaces";
pub const NO_WORKSPACES: &str = "No workspace is visible to your account.";
pub const SHOW_SERVER: &str = "Show this server";

// The state of a server, as the line under its name.
pub const STATUS_CONNECTING: &str = "connecting…";
pub const STATUS_RECONNECTING: &str = "reconnecting…";
pub const STATUS_WAITING_FOR_APPROVAL: &str = "waiting for approval";
pub const STATUS_WAITING_FOR_LINK: &str = "waiting for a device link";
pub const STATUS_NOT_JOINED: &str = "not joined";
pub const STATUS_REMOVED: &str = "removed from the server";
pub const STATUS_REQUEST_DENIED: &str = "request denied";
pub const STATUS_REQUEST_EXPIRED: &str = "request expired";
pub const STATUS_LINK_EXPIRED: &str = "link expired";
pub const STATUS_KEY_MISSING: &str = "device key missing";
pub const STATUS_BANNED: &str = "banned";
pub const STATUS_UNREACHABLE: &str = "cannot be reached";
pub const STATUS_NEEDS_UPDATE: &str = "needs a newer Junto";
pub const STATUS_TOO_OLD: &str = "server is too old";
pub const STATUS_INCOMPATIBLE: &str = "not compatible";
pub const STATUS_OTHER_SERVER: &str = "another server answers";
pub const STATUS_LOGGED_OUT: &str = "not logged in";
pub const STATUS_RECOVERY: &str = "recovery notice";

// The channel column.
pub const TEXT_CHANNELS: &str = "Text channels";
pub const VOICE_CHANNELS: &str = "Voice channels";
pub const SERVER_MENU: &str = "Server menu";
pub const LOCKED: &str = "locked";
pub const IN_USE: &str = "in use";
pub const FULL: &str = "full";
pub const RECORDING_MARK: &str = "REC";
pub const RECORDING_RUNS: &str = "This call is recorded";
pub const NO_CHANNELS: &str = "No channel is visible to your account.";
pub const YOU: &str = "you";
pub const UNKNOWN_PERSON: &str = "Unknown person";

pub const MICROPHONE_CLOSED: &str = "Microphone closed";
pub const MICROPHONE_OPEN: &str = "Microphone open";
pub const SPEAKING: &str = "Speaking";
pub const SHARING_SCREEN: &str = "Sharing a screen";
pub const HAND_RAISED: &str = "Hand raised";
pub const SOUND_OFF: &str = "Sound output off";

pub fn microphone_held(reason: BlockReason) -> &'static str {
    match reason {
        BlockReason::Moderator => "Microphone closed by a moderator",
        BlockReason::Meeting => "Microphone closed by meeting mode",
    }
}

/// The count of a voice channel with a user limit: "8 / 8 · full".
pub fn occupancy(inside: usize, limit: u32, full: bool) -> String {
    if full {
        format!("{inside} / {limit} · {FULL}")
    } else {
        format!("{inside} / {limit}")
    }
}

// The profile strip.
pub const CLOSE_MICROPHONE: &str = "Microphone. It is open: close it.";
pub const OPEN_MICROPHONE: &str = "Your microphone is closed. Open it.";
pub const MICROPHONE_HELD: &str = "Your microphone was closed by someone else. You cannot open it.";
pub const SOUND_OUTPUT_ON: &str = "Sound output. It is on: turn the sound of calls off.";
pub const SOUND_OUTPUT_OFF: &str = "The sound of calls is off. Turn it on.";
pub const APP_SETTINGS: &str = "App settings";
pub const NO_NAME_YET: &str = "No name yet";

/// The line above the profile strip about the sound of this device: what holds, in strong
/// type, and what follows from it.
pub fn sound_warning(warning: &SoundWarning) -> (String, String) {
    let named = |by: &Option<String>| by.as_ref().map(|name| format!(" ({name})"));
    match warning {
        SoundWarning::MicrophoneHeld {
            reason: BlockReason::Moderator,
            by,
        } => (
            "Microphone closed by a moderator".to_owned(),
            format!(
                "{}. Only a moderator can open it.",
                named(by).unwrap_or_default()
            ),
        ),
        SoundWarning::MicrophoneHeld {
            reason: BlockReason::Meeting,
            by,
        } => (
            "Microphone closed by meeting mode".to_owned(),
            match by {
                Some(name) => format!(". {name} runs it and can open your microphone."),
                None => ". The person who runs it can open your microphone.".to_owned(),
            },
        ),
        SoundWarning::NoMicrophoneAccess => (
            "Junto may not use the microphone.".to_owned(),
            " Allow it in the settings of the system, then try again. You hear the call, and \
             the others do not hear you."
                .to_owned(),
        ),
        SoundWarning::MicrophoneFailed => (
            "The microphone does not work.".to_owned(),
            " Choose another input device in the Settings tab of the call, or connect one \
             and try again."
                .to_owned(),
        ),
        SoundWarning::SoundHeld { by } => (
            "Sound closed by a moderator".to_owned(),
            format!(
                "{}. You do not hear the call until a moderator opens it.",
                named(by).unwrap_or_default()
            ),
        ),
        SoundWarning::OutputFailed => (
            "The sound output does not work.".to_owned(),
            " Choose another output device in the Settings tab of the call.".to_owned(),
        ),
    }
}

// The strip of the person's call.
pub const IN_CALL: &str = "In call";
pub const CALL_RECONNECTING: &str = "Reconnecting…";
pub const CALL_CONNECTING: &str = "Connecting…";
pub const MICROPHONE_PAUSED: &str = "Your microphone is paused.";
pub const LEAVE: &str = "Leave";
pub const LEAVE_CALL: &str = "Leave call";
pub const SHOW_CALL: &str = "Show the call";
pub const CALL_TIME: &str = "Call time";
pub const CAMERA_NOT_AVAILABLE: &str = "Camera. Cameras are not in this version.";
pub const RAISE_HAND: &str = "Raise hand";
pub const LOWER_HAND: &str = "Your hand is raised. Lower it.";

// The rules of the room of a call (ADR 0739).
pub const MEETING_MODE_HELP: &str = "The person sharing can turn this on. All other microphones close; people raise a hand to \
     speak.";
pub const MEETING_NEEDS_SHARE: &str =
    "Share your screen first. The person sharing can turn meeting mode on.";
pub const MEETING_NO_PERMISSION: &str = "You do not have the “run meeting mode” permission.";
pub const MEETING_WHILE_BLOCKED: &str =
    "Your microphone is closed by someone else. You cannot turn meeting mode on.";
pub const MEETING_TURN_OFF: &str =
    "Meeting mode is on. Turn it off: every microphone block of the call ends.";
pub const LOCK_ROOM: &str = "Lock room";
pub const LOCK_ROOM_HELP: &str =
    "While the room is locked, only people who are invited or moved in can enter";
pub const LOCK_ROOM_SETTING_HELP: &str = "Only people who are invited or moved in can enter.";
pub const LOCK_NO_PERMISSION: &str = "You do not have the “lock room” permission.";
pub const USER_LIMIT: &str = "User limit";
pub const USER_LIMIT_HELP: &str = "The most people who can join by themselves. People who are invited or moved in can still \
     enter.";
pub const NO_USER_LIMIT: &str = "off";
pub const LOWER_USER_LIMIT: &str = "Lower the user limit";
pub const RAISE_USER_LIMIT: &str = "Raise the user limit";
pub const HIDE_OCCUPANTS: &str = "Hide who is inside";
pub const HIDE_OCCUPANTS_HELP: &str = "Others see only “in use”.";
pub const HIDE_OCCUPANTS_NEXT_CALL: &str =
    "This call keeps its setting. The change holds from the next call.";
pub const MANAGE_NO_PERMISSION: &str = "You do not have the “manage channel” permission.";
pub const MEETING_IS_ON: &str = "Meeting mode is on";
pub const YOU_RUN_IT: &str = " · you run it";
pub const MEETING_PRESENTER_HELP: &str = "Every other microphone is closed. You allow who speaks.";
pub const MEETING_PARTICIPANT_HELP: &str =
    "Every other microphone is closed. Raise a hand to speak.";
pub const RAISED_A_HAND: &str = " raised a hand";
pub const ALLOW: &str = "Allow";
pub const MAY_SPEAK_NOT_OPEN: &str = " may speak and has not opened the microphone";
pub const YOU_MAY_SPEAK: &str = "You may speak";
pub const UNMUTE: &str = "Unmute";
pub const UNMUTE_HELP: &str = "Open your microphone. The app never opens it by itself.";
pub const MICROPHONE_OPEN_AGAIN: &str = "The block of your microphone ended. Your microphone is open, and the people in the call \
     hear you.";

/// "Room · Daily": the heading of the settings of the room of a call.
pub fn room_section(channel: &str) -> String {
    format!("Room · {channel}")
}

/// " · Carla runs it", after the statement that meeting mode is on.
pub fn runs_it(presenter: Option<&str>) -> String {
    format!(" · {} runs it", presenter.unwrap_or(UNKNOWN_PERSON))
}

/// Why a person who does not run meeting mode cannot turn it off.
pub fn meeting_run_by(presenter: Option<&str>) -> String {
    format!(
        "{} runs meeting mode and turns it off.",
        presenter.unwrap_or(UNKNOWN_PERSON)
    )
}

pub fn allow_help(name: &str) -> String {
    format!("{name} gets the prompt “You may speak” and opens the microphone without your help.")
}

/// "and 2 more", after the first raised hand.
pub fn more_hands(count: usize) -> String {
    format!("and {count} more")
}

/// The count of a room for the people inside: who is inside and what the limit means.
pub fn occupancy_help(inside: usize, limit: u32) -> String {
    format!(
        "{inside} {} inside, limit {limit}. People who are invited or moved in can enter above \
         the limit. Change the limit in Settings.",
        if inside == 1 { "person" } else { "people" }
    )
}

// A room that a person cannot enter without help, and the requests to enter.
pub const ASK_TO_ENTER: &str = "Ask to enter";
pub const ASK_TO_ENTER_HELP: &str =
    "The people inside get a prompt. You enter when one of them accepts.";
pub const FULL_NOTE: &str = "Full. You can enter when a person leaves or invites you.";
pub const ENTRY_ASKING: &str = "Asking…";
pub const ENTRY_WAITING_TITLE: &str = "You asked to enter";
pub const ENTRY_WAITING: &str =
    "The people inside have your request. It ends after one minute without an answer.";
pub const ENTRY_DENIED_TITLE: &str = "Your request was denied";
pub const ENTRY_DENIED: &str = "A person inside did not let you in.";
pub const ENTRY_EXPIRED_TITLE: &str = "Nobody answered your request";
pub const ENTRY_EXPIRED: &str = "The request ended after one minute.";
pub const WITHDRAW_REQUEST: &str = "Withdraw request";
pub const ASK_AGAIN: &str = "Ask again";
pub const LET_IN: &str = "Let in";
pub const DENY: &str = "Deny";

/// "Focus room is locked", "All hands is full".
pub fn closed_title(channel: &str, closed: Closed) -> String {
    match closed {
        Closed::Locked => format!("{channel} is locked"),
        Closed::Full => format!("{channel} is full"),
    }
}

pub fn closed_help(closed: Closed) -> &'static str {
    match closed {
        Closed::Locked => LOCK_ROOM_SETTING_HELP,
        Closed::Full => "You can enter when a person leaves or invites you.",
    }
}

/// The prompt of the people inside about a person who asks to enter.
pub fn entry_prompt(name: Option<&str>, channel: &str) -> String {
    format!(
        "{} asks to enter {channel}.",
        name.unwrap_or(UNKNOWN_PERSON)
    )
}

// Invitations.
pub const INVITE_A_MEMBER: &str = "Invite a member";
pub const FIND_A_MEMBER: &str = "Find a member";
pub const INVITE: &str = "Invite";
pub const INVITED: &str = "Invited";
pub const WITHDRAW_INVITATION: &str = "Withdraw the invitation";
pub const NO_MEMBER_TO_INVITE: &str = "Every member of the workspace is in the call.";
pub const NO_MEMBER_FOUND: &str = "No member has this name.";
pub const JOIN: &str = "Join";
pub const DECLINE: &str = "Decline";
pub const INVITATION_DECLINED: &str = "Your invitation was declined.";
pub const INVITATION_EXPIRED: &str = "Your invitation got no answer for one minute and ended.";

pub fn invite_to(channel: &str) -> String {
    format!("Invite to {channel}")
}

pub fn invite_help(channel: &str) -> String {
    format!(
        "The member gets a prompt and enters {channel} on accept, also when the room is locked \
         or full."
    )
}

pub fn invite_footer(channel: &str) -> String {
    format!(
        "Members of the workspace who are not in the call. The server refuses a member without \
         access to {channel}."
    )
}

/// The prompt of a person who was invited into a call.
pub fn invitation(by: Option<&str>, channel: Option<&str>) -> String {
    let by = by.unwrap_or(UNKNOWN_PERSON);
    match channel {
        Some(channel) => format!("{by} invites you to {channel}."),
        None => format!("{by} calls you."),
    }
}

// The menu of a person in a voice channel.
pub const INVITE_TO_MY_ROOM: &str = "Invite to my room";
pub const MOVE_TO: &str = "Move to…";
pub const PERMISSION_MOVE: &str = "Permission: move members";
pub const MODERATION: &str = "Moderation";
pub const MUTE_FOR_EVERYONE: &str = "Mute for everyone";
pub const REMOVE_MICROPHONE_BLOCK: &str = "Remove the microphone block";
pub const PERMISSION_MUTE: &str = "Permission: mute members";
pub const BLOCK_SOUND: &str = "Turn the call sound off for this person";
pub const REMOVE_SOUND_BLOCK: &str = "Remove the sound block";
pub const PERMISSION_DEAFEN: &str = "Permission: deafen members";
pub const DISCONNECT: &str = "Disconnect";
pub const PERMISSION_DISCONNECT: &str = "Permission: disconnect members";
pub const PERSON_MENU_KEYS: &str = "Keyboard: Menu or Shift + F10";
pub const BACK: &str = "Back";
pub const OPEN_PERSON_MENU: &str = "Right click or Shift + F10: the menu of this person";

pub fn invite_prompt(name: &str, channel: &str) -> String {
    format!("{name} gets a prompt to enter {channel}")
}

pub fn your_volume_for(name: &str) -> String {
    format!("Your volume for {name}")
}

/// The notice about a request in a call that the server did not perform.
pub fn call_refused(reason: &str) -> String {
    format!("The call was not changed. {reason}")
}

// The share of this device.
pub const SHARE_SCREEN: &str = "Share screen";
pub const SHARE_STARTING: &str = "Your share starts";
pub const SHARE_RUNS: &str = "You are sharing your screen. Press to stop.";
pub const SHARE_NOT_ON_THIS_SYSTEM: &str =
    "Share screen. Screen sharing is not available on this system in this version.";
pub const SHARE_PICKER_TITLE: &str = "Share your screen";
pub const SHARE_READING: &str = "Reading the screens and windows…";
pub const SCREENS: &str = "Screens";
pub const WINDOWS: &str = "Windows";
pub const NO_WINDOWS: &str = "No window of another application is open.";
pub const WHAT_TO_SHARE: &str = "What to share";
pub const SHARE_SOUND: &str = "Sound of this share";
pub const SOUND_NONE: &str = "No sound";
pub const SOUND_SYSTEM: &str = "All system sound";
pub const SOUND_ONE_APPLICATION: &str = "One application";
pub const SOUND_THIS_APPLICATION: &str = "This application";
pub const SOUND_CHOICE_HELP: &str =
    "Choose the sound for each share. The sound of Junto itself is never shared.";
pub const SOUND_SYSTEM_HELP: &str = "The people in the call hear every sound that this computer plays, except the sound of \
     Junto.";
pub const SOUND_NONE_HELP: &str = "The people in the call get the picture only.";
pub const SOUND_APPLICATION_HELP: &str = "The people in the call hear the sound of the application that you choose here, and of \
     no other one.";
pub const NO_APPLICATIONS: &str = "No other application with a window is open.";
pub const SHARE: &str = "Share";
pub const SCREEN_ACCESS_TITLE: &str = "Junto may not record the screen yet";
pub const SCREEN_ACCESS_HELP: &str = if cfg!(target_os = "macos") {
    "macOS gives this permission in System Settings, under Privacy & Security, Screen & \
     System Audio Recording. Turn it on for Junto there. macOS can then ask you to quit \
     and open Junto again."
} else {
    "The system does not allow Junto to record the screen. Allow it in the settings of the \
     system."
};
pub const OPEN_SYSTEM_SETTINGS: &str = "Open System Settings";
pub const SHARE_UNSUPPORTED: &str =
    "Screen sharing is not available on this system in this version of Junto.";

/// The name of a screen in the list of what can be shared.
pub fn screen_name(number: usize) -> String {
    format!("Screen {number}")
}

/// A size in pixels: "1920 × 1200".
pub fn pixels(size: (u32, u32)) -> String {
    format!("{} × {}", size.0, size.1)
}

/// The sound of a shared window when the person chooses its application.
pub fn sound_of_window(application: &str) -> String {
    format!(
        "The people in the call hear the sound of {application}, the application of this \
         window, and of no other one."
    )
}

/// Why a share did not start, or ended by itself.
pub fn capture_error(error: &CaptureError) -> String {
    match error {
        CaptureError::NotAllowed => {
            "The system does not allow Junto to record the screen.".to_owned()
        }
        CaptureError::SourceGone => {
            "The screen, the window or the application is not available any more, or the \
             system stopped the capture."
                .to_owned()
        }
        CaptureError::Unsupported => SHARE_UNSUPPORTED.to_owned(),
        CaptureError::Interrupted => "The connection to the call was lost.".to_owned(),
        CaptureError::Failed(reason) => {
            format!("The system reported: {}", sentence(reason))
        }
    }
}

/// The notice about a share of this device that the person did not end.
pub fn share_ended(error: &CaptureError) -> String {
    format!(
        "Your screen share ended. {} Start the share again when you want to continue.",
        capture_error(error)
    )
}

// The shares of a call.
pub const WATCHING: &str = "Watching";
pub const WATCH: &str = "Watch";
pub const ENTIRE_SCREEN: &str = "Entire screen";
pub const ONE_WINDOW: &str = "Window";
pub const PICTURE_ARRIVES: &str = "The picture arrives…";
pub const SOUND_FOLLOWS: &str = "The sound follows the share you watch.";
pub const SHARE_SOUND_HELD: &str = "Its sound is closed while your microphone is blocked";
pub const SHARE_SOUND_OFF_FOR_YOU: &str = "You turned its sound off";
pub const LISTED_IN_MEMBERS: &str = "They are listed in Members.";

/// "2 shares".
pub fn shares_count(count: usize) -> String {
    format!("{count} shares")
}

/// The name of a share: "Carla's screen", "Bruno's window", "Your screen".
pub fn share_title(share: &ShareCard, is_own: bool) -> String {
    let what = match share.kind {
        Some(ShareKind::Screen) => "screen",
        Some(ShareKind::Window) => "window",
        None => "share",
    };
    if is_own {
        format!("Your {what}")
    } else {
        let name = share.name.as_deref().unwrap_or(UNKNOWN_PERSON);
        format!("{name}'s {what}")
    }
}

/// What a share shows, for the line under its name.
pub fn share_kind(kind: Option<ShareKind>) -> &'static str {
    match kind {
        Some(ShareKind::Screen) => ENTIRE_SCREEN,
        Some(ShareKind::Window) => ONE_WINDOW,
        None => "",
    }
}

/// The measures of a share: "Entire screen · 1920 × 1200 · 30 fps". With `kind` absent
/// the line starts with the size.
pub fn share_measure(
    kind: Option<ShareKind>,
    size: Option<(u32, u32)>,
    frame_rate: Option<u32>,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(kind) = kind {
        parts.push(share_kind(Some(kind)).to_owned());
    }
    if let Some(size) = size {
        parts.push(pixels(size));
    }
    if let Some(rate) = frame_rate {
        parts.push(format!("{rate} fps"));
    }
    parts.join(" · ")
}

/// The sound of a share beside the name of its sharer: "with system audio".
pub fn share_sound(sound: Option<ShareAudio>) -> Option<&'static str> {
    match sound? {
        ShareAudio::System => Some("with system audio"),
        ShareAudio::Application => Some("with the sound of one application"),
        ShareAudio::None => Some("without sound"),
    }
}

/// The sound of the watched share when the call has several: what the person hears.
pub fn watched_sound(sound: Option<ShareAudio>) -> Option<&'static str> {
    match sound? {
        ShareAudio::System => Some("You hear its system audio."),
        ShareAudio::Application => Some("You hear the sound of its application."),
        ShareAudio::None => Some("This share has no sound."),
    }
}

/// The people whom the strip of a call does not show: the part in strong type, and the
/// rest of the sentence.
pub fn people_hidden(count: usize) -> (String, &'static str) {
    if count == 1 {
        ("1 person without video".to_owned(), " is hidden.")
    } else {
        (format!("{count} people without video"), " are hidden.")
    }
}

// The settings of a call.
pub const YOUR_SCREEN_SHARE: &str = "Your screen share";
pub const RESOLUTION: &str = "Resolution";
pub const FRAME_RATE: &str = "Frame rate";
pub const SHARE_SETTINGS_HELP: &str = "Used the next time you share.";
pub const YOUR_VIEW: &str = "Your view";
pub const HIDE_WITHOUT_VIDEO: &str = "Hide people without video";
pub const HIDE_WITHOUT_VIDEO_HELP: &str = "They stay in Members and in the channel list.";
pub const SHARE_VOLUME: &str = "Sound of the share you watch";
pub const SHARE_VOLUME_HELP: &str = "At 0 % you keep the picture without its sound.";

pub fn share_resolution(resolution: ShareResolution) -> &'static str {
    match resolution {
        ShareResolution::P720 => "720p",
        ShareResolution::P1080 => "1080p",
        ShareResolution::Source => "Source",
    }
}

// The recording of a call.
pub const RECORD: &str = "Record";
pub const START_RECORDING: &str = "Start a recording of this call";
pub const START_NEW_RECORDING: &str = "Start a new recording";
pub const STOP: &str = "Stop";
pub const STOP_RECORDING: &str = "Stop recording";
pub const RECORDING_STOPS: &str = "The recording stops";
pub const RECORDING_STARTS: &str = "starting…";
pub const RECORDING_TIME: &str = "Recording time";
pub const NO_RECORD_PERMISSION: &str = "You do not have the “record calls” permission.";
pub const RECORDING_FAILED: &str = "Recording failed";
pub const RECORDING_STOPPED: &str = "Recording stopped";
pub const DETAILS: &str = "Details";
pub const RECORDING_DETAILS: &str = "What happened and what was saved";

/// The recording mark with its time: "REC 00:39:02".
pub fn recording_time(elapsed: std::time::Duration) -> String {
    format!("{RECORDING_MARK} {}", call_time(elapsed))
}

/// The flag of a recording that ended with a problem.
pub fn recording_flag(problem: &RecordingProblem) -> &'static str {
    match problem.fault {
        RecordingFault::StorageReserve | RecordingFault::SegmentLimit => RECORDING_STOPPED,
        _ => RECORDING_FAILED,
    }
}

/// What happened to a recording and what was saved: the sentence in strong type, and the
/// rest.
pub fn recording_details(problem: &RecordingProblem) -> (String, String) {
    let clock = |at: time::OffsetDateTime| {
        let at = at.to_offset(time::UtcOffset::UTC);
        format!("{:02}:{:02} UTC", at.hour(), at.minute())
    };
    let at = problem
        .until
        .map(|at| format!(" at {}", clock(at)))
        .unwrap_or_default();
    let range = match (problem.since, problem.until) {
        (Some(since), Some(until)) => {
            let minutes = (until - since).whole_minutes().max(0);
            format!(
                " The server has what was recorded from {} to {} ({minutes} min) and checks \
                 it.",
                clock(since),
                clock(until)
            )
        }
        _ => String::new(),
    };
    let after = problem
        .until
        .map(|until| format!(" Nothing after {} was recorded.", clock(until)))
        .unwrap_or_default();
    let again = " To continue, start a new recording.";
    match problem.fault {
        RecordingFault::RecorderLost => (
            format!("The recorder lost its connection{at}."),
            format!("{range}{after}{again}"),
        ),
        RecordingFault::StorageReserve => (
            format!("The server stopped the recording{at}."),
            format!(
                " Its free disk space reached the reserve.{range}{after} An administrator \
                 of the server frees disk space before a new recording."
            ),
        ),
        RecordingFault::SegmentLimit => (
            format!("The server stopped the recording{at}."),
            format!(
                " The recording holds as many files as one recording can have.\
                 {range}{after}{again}"
            ),
        ),
        RecordingFault::RecorderUnavailable => (
            "No recorder took the recording.".to_owned(),
            format!(" Nothing was recorded.{again}"),
        ),
        RecordingFault::NothingSaved => (
            "Nothing of the recording was saved.".to_owned(),
            again.to_owned(),
        ),
        RecordingFault::PartlySaved => (
            "A part of the recording was not saved.".to_owned(),
            format!(
                " {} of its {} files failed. The saved part is on the server.",
                problem.failed_segments, problem.segments
            ),
        ),
        RecordingFault::Other => (
            format!("The server stopped the recording{at}."),
            format!("{range}{after}{again}"),
        ),
    }
}

/// The notice about a request for a recording that the server did not take.
pub fn recording_request_failed(reason: &str) -> String {
    format!("The recording was not changed. {reason}")
}
pub const A_VOICE_CHANNEL: &str = "a voice channel";

/// The time of a call as hours, minutes and seconds: "00:42:17".
pub fn call_time(elapsed: std::time::Duration) -> String {
    let seconds = elapsed.as_secs();
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

// A voice channel in the main area.
pub const JOIN_CALL: &str = "Join call";
pub const JOINING_CALL: &str = "Joining…";
pub const ENTER_VOICE_CHANNEL: &str = "Enter the voice channel";
pub const ROOM_CONTROLS_RETURN: &str = "Room controls return when the call is back.";
pub const CONNECTING_TO_CALL: &str = "Connecting to the call…";
pub const CONNECTING_TO_CALL_HELP: &str =
    "Junto connects to the media server. The others hear you when the connection is made.";
pub const ALONE_IN_CALL: &str = "You are the only person in this call.";
pub const SOUND_RETURNS: &str = "The sound of the other people returns when the call is back.";
pub const DISMISS: &str = "Close";

/// The line beside the title of a voice channel that this device is not in.
pub fn voice_channel_topic(inside: usize, hidden: bool) -> String {
    let state = if hidden {
        "It is in use and shows who is inside only to the people inside.".to_owned()
    } else {
        match inside {
            0 => "Nobody is in the call now.".to_owned(),
            1 => "1 person is in the call.".to_owned(),
            inside => format!("{inside} people are in the call."),
        }
    };
    format!("Chat of this voice channel. {state}")
}

/// What the stage tells while the media connection of the call is restored.
pub fn connection_dropped(at: Option<time::OffsetDateTime>, channel: &str) -> String {
    let when = match at {
        Some(at) => {
            let at = at.to_offset(time::UtcOffset::UTC);
            format!(" at {:02}:{:02} UTC", at.hour(), at.minute())
        }
        None => String::new(),
    };
    format!(
        "The connection to the call dropped{when}. Junto tries again every few seconds. You \
         stay in {channel}. Your microphone is paused until the call is back."
    )
}

// The panel of a call.
pub const CHAT: &str = "Chat";
pub const VOICE_CHANNEL_CHAT: &str = "voice channel chat";
pub const CLOSE_PANEL: &str = "Close the panel";
pub const IN_THE_CALL: &str = "In the call";
pub const NOT_IN_THE_CALL: &str = "Not in the call";
pub const VOLUME: &str = "Volume";
pub const MICROPHONE_ON: &str = "Microphone on";
pub const MUTED: &str = "Muted";
pub const SOUND_IS_OFF: &str = "Sound off";

pub fn volume_for(name: &str) -> String {
    format!("Your volume for {name}")
}

pub fn in_channel(name: &str) -> String {
    format!("In {name}")
}

/// The state of a microphone that a moderator or meeting mode closed.
pub fn closed_by(reason: BlockReason, by: Option<&str>) -> String {
    match (reason, by) {
        (BlockReason::Moderator, Some(name)) => format!("Closed by a moderator ({name})"),
        (BlockReason::Moderator, None) => "Closed by a moderator".to_owned(),
        (BlockReason::Meeting, _) => "Closed by meeting mode".to_owned(),
    }
}

// The sound settings of a call.
pub const YOUR_AUDIO: &str = "Your audio";
pub const INPUT_DEVICE: &str = "Input device";
pub const OUTPUT_DEVICE: &str = "Output device";
pub const INPUT_LEVEL: &str = "Input level";
pub const SYSTEM_DEFAULT: &str = "System default";
pub const DEVICE_NOT_CONNECTED: &str = "A device that is not connected";
pub const MORE_SOUND_SETTINGS: &str =
    "Echo cancellation, noise suppression, gain and push to talk are in the app settings.";

/// The default device of the system in a list of devices.
pub fn system_default(name: &str) -> String {
    if name.is_empty() {
        SYSTEM_DEFAULT.to_owned()
    } else {
        format!("{SYSTEM_DEFAULT} ({name})")
    }
}

/// The state of the permission of the system to use the microphone.
pub fn microphone_access(access: MicrophoneAccess) -> &'static str {
    match access {
        MicrophoneAccess::Allowed => "Microphone access: allowed.",
        MicrophoneAccess::NotAsked => {
            "Microphone access: not asked yet. The system asks at the first use."
        }
        MicrophoneAccess::Refused => {
            "Microphone access: refused. Allow Junto in the privacy settings of the system."
        }
    }
}

// The sound settings of the application.
pub const VOICE: &str = "Voice";
pub const INPUT_MODE: &str = "Input mode";
pub const VOICE_ACTIVATION: &str = "Voice activation";
pub const PUSH_TO_TALK: &str = "Push to talk";
pub const TALK_KEY: &str = "Key";
pub const INPUT_MODE_HELP: &str = "Voice activation sends your open microphone. Push to talk \
     sends it only while you hold the key, and only while Junto is the focused application.";
pub const ECHO_CANCELLATION: &str = "Echo cancellation";
pub const ECHO_CANCELLATION_HELP: &str =
    "Removes the sound of the call that your microphone picks up from your loudspeakers.";
pub const NOISE_SUPPRESSION: &str = "Noise suppression";
pub const NOISE_SUPPRESSION_HELP: &str = "Removes steady background noise.";
pub const AUTOMATIC_GAIN: &str = "Automatic gain";
pub const AUTOMATIC_GAIN_HELP: &str = "Keeps your voice at an even level.";

pub fn talk_key(key: TalkKey) -> &'static str {
    match key {
        TalkKey::Alt if cfg!(target_os = "macos") => "Option",
        TalkKey::Alt => "Alt",
        TalkKey::Control => "Control",
    }
}

/// How the last join or call ended, when the person did not end it.
pub fn call_notice(notice: &CallNotice) -> String {
    let named = |channel: &Option<String>| {
        channel
            .clone()
            .unwrap_or_else(|| A_VOICE_CHANNEL.to_owned())
    };
    match notice {
        CallNotice::JoinFailed { channel, error } => {
            let channel = named(channel);
            match error {
                FlowError::Refused {
                    code: ErrorCode::CallLocked,
                    ..
                } => format!(
                    "{channel} is locked. Only people who are invited or moved in can enter."
                ),
                FlowError::Refused {
                    code: ErrorCode::CallFull,
                    ..
                } => {
                    format!("{channel} is full. You can enter when a person leaves or invites you.")
                }
                other => format!("{channel} was not joined. {}", flow_error(other)),
            }
        }
        CallNotice::Moved { channel } => {
            format!("A moderator moved you to {}.", named(channel))
        }
        CallNotice::Ended { channel, reason } => {
            let channel = named(channel);
            match reason {
                EndReason::Server(Some(LeaveReason::Disconnected)) => {
                    format!("A moderator removed you from the call in {channel}.")
                }
                EndReason::Server(Some(LeaveReason::AccessEnded)) => format!(
                    "The call in {channel} ended for you: your account lost its access to the \
                     voice channel."
                ),
                EndReason::Server(Some(
                    LeaveReason::ConnectionLost | LeaveReason::NeverConnected,
                )) => format!(
                    "You are no longer in the call in {channel}: the media connection of \
                     this device was lost for too long."
                ),
                EndReason::Server(Some(LeaveReason::Moved)) => {
                    format!("You were moved out of {channel}.")
                }
                EndReason::Server(_) => {
                    format!("The call in {channel} ended for this device.")
                }
                EndReason::Elsewhere => format!(
                    "This device left the call in {channel}: your account is in a call with \
                     another device."
                ),
                EndReason::Refused(error) => format!(
                    "The connection to the call in {channel} was lost, and the server gave \
                     no new place. {}",
                    flow_error(error)
                ),
            }
        }
    }
}

// The main area.
pub const MEMBERS: &str = "Members";
pub const SHOW_MEMBERS: &str = "Show the members of the workspace";
pub const CLOSE_MEMBERS: &str = "Close the member list";
pub const MEMBERS_LOADING: &str = "Reading the members…";
pub const MEMBERS_FAILED: &str = "The members could not be read.";
pub const TRY_AGAIN: &str = "Try again";
pub const MEETING_MODE: &str = "Meeting mode";
pub const RECORDING_VISIBLE: &str = "visible to everyone";
pub const NO_SERVER_TITLE: &str = "No server yet";
pub const NO_SERVER: &str = "Type the address of a server in the server book. The \
     administrator of the server gives you the address, and a code if the server asks for \
     one.";
pub const NO_CHANNEL_TITLE: &str = "No channel";
pub const STATE_STALE: &str =
    "The connection to this server is interrupted. The lists show the last known state.";

// The states of a server in the main area.
pub const LOGGED_OUT_TITLE: &str = "This device is not logged in";
pub const LOGGED_OUT: &str =
    "The server answers, and the login with the key of this device did not succeed.";
pub const LOCKED_STATE: &str = "Locked";
pub const CONNECTING_TITLE: &str = "Connecting to the server";
pub const CONNECTING: &str = "Junto logs in with the key of this device.";
pub const UNREACHABLE_TITLE: &str = "The server cannot be reached";
pub const UNREACHABLE: &str =
    "Junto tries again by itself. Check the network connection of this computer.";
pub const NOT_JOINED_TITLE: &str = "You have no account on this server";
pub const NOT_JOINED: &str = "Join with a code, or ask an administrator for access.";
pub const REMOVED_TITLE: &str = "Your account or this device was removed";
pub const REMOVED: &str = "The server no longer knows the key of this device. You can join \
     again with a code, or ask for access.";
pub const REQUEST_DENIED_TITLE: &str = "Your request was denied";
pub const REQUEST_DENIED: &str = "An administrator denied the request for access.";
pub const REQUEST_EXPIRED_TITLE: &str = "Your request expired";
pub const REQUEST_EXPIRED: &str = "No administrator answered the request in time.";
pub const LINK_EXPIRED_TITLE: &str = "The device link expired";
pub const LINK_EXPIRED: &str = "No logged-in device approved the link in time.";
pub const KEY_MISSING_TITLE: &str = "The key of this device is missing";
pub const KEY_MISSING: &str = "The server book names an account, and the key file of this \
     device for the server is gone. Join again with a code, or ask for access.";
pub const BANNED_TITLE: &str = "Your account is banned on this server";
pub const BANNED: &str = "An administrator can lift the ban. The key of this device stays, \
     so the account is the same after that.";
pub const WAITING_FOR_LINK_TITLE: &str = "This device waits for a device link";
pub const WAITING_FOR_LINK: &str =
    "Approve the link on a device that is logged in to this account.";
pub const OTHER_SERVER_TITLE: &str = "The address answers as another server";
pub const OTHER_SERVER: &str = "The saved address now reports another server identifier. \
     Junto sends it no proof of the device key and no session.";
pub const NEEDS_UPDATE_TITLE: &str = "This server needs a newer version of Junto";
pub const TOO_OLD_TITLE: &str = "This server is too old for this version of Junto";
pub const INCOMPATIBLE_TITLE: &str = "This server and this version of Junto do not fit";
pub const INCOMPATIBLE: &str = "The other servers of the server book are not affected.";
pub const JOIN_THIS_SERVER: &str = "Join this server";
pub const CHECK_AGAIN: &str = "Check again";
pub const FORGET_ACCOUNT: &str = "Forget this account";
pub const REMOVE_SERVER: &str = "Remove from this device";

pub fn incompatibility(reason: Incompatibility) -> (&'static str, String) {
    match reason {
        Incompatibility::ClientTooOld { server_oldest } => (
            NEEDS_UPDATE_TITLE,
            format!("The oldest API version of the server is {server_oldest}. {INCOMPATIBLE}"),
        ),
        Incompatibility::ServerTooOld { server_newest } => (
            TOO_OLD_TITLE,
            format!("The newest API version of the server is {server_newest}. {INCOMPATIBLE}"),
        ),
        Incompatibility::NoCommonVersion => (
            INCOMPATIBLE_TITLE,
            format!("They have no API version in common. {INCOMPATIBLE}"),
        ),
        Incompatibility::NoVersions => (
            INCOMPATIBLE_TITLE,
            format!("The server reports no API version. {INCOMPATIBLE}"),
        ),
    }
}

/// The heading and the sentence for a device without an account.
pub fn no_account(notice: Option<Notice>) -> (&'static str, &'static str) {
    match notice {
        None => (NOT_JOINED_TITLE, NOT_JOINED),
        Some(Notice::Removed) => (REMOVED_TITLE, REMOVED),
        Some(Notice::RequestDenied) => (REQUEST_DENIED_TITLE, REQUEST_DENIED),
        Some(Notice::RequestExpired) => (REQUEST_EXPIRED_TITLE, REQUEST_EXPIRED),
        Some(Notice::LinkExpired) => (LINK_EXPIRED_TITLE, LINK_EXPIRED),
        Some(Notice::KeyMissing) => (KEY_MISSING_TITLE, KEY_MISSING),
    }
}

pub const fn no_account_status(notice: Option<Notice>) -> &'static str {
    match notice {
        None => STATUS_NOT_JOINED,
        Some(Notice::Removed) => STATUS_REMOVED,
        Some(Notice::RequestDenied) => STATUS_REQUEST_DENIED,
        Some(Notice::RequestExpired) => STATUS_REQUEST_EXPIRED,
        Some(Notice::LinkExpired) => STATUS_LINK_EXPIRED,
        Some(Notice::KeyMissing) => STATUS_KEY_MISSING,
    }
}

// Joining a server.
pub const JOIN_A_SERVER: &str = "Join a server";
pub const CLOSE: &str = "Close";
pub const CANCEL: &str = "Cancel";
pub const SERVER_FOUND: &str = "Server found";
pub const OPERATOR_NOTICE: &str =
    "The person who operates this server has technical access to its data.";
pub const HAVE_A_CODE: &str = "I have a code";
pub const HAVE_A_CODE_HELP: &str =
    "An administrator gave you a code. Your account is created at once.";
pub const CODE_FIELD: &str = "Invite code or owner code";
pub const JOIN_WITH_CODE: &str = "Join with code";
pub const JOINING: &str = "Joining…";
pub const REQUEST_ACCESS: &str = "Request access";
pub const REQUESTING: &str = "Sending…";
pub const REQUEST_SENT_TITLE: &str = "Request sent · an administrator must approve it";
pub const CANCEL_REQUEST: &str = "Cancel request";
pub const ADD_TO_SERVER_BOOK: &str = "Add to the server book";
pub const KEY_OF_OTHER_ADDRESS_TITLE: &str =
    "This device holds a key of this server for another address";
pub const DISCARD_KEY: &str = "Discard the stored key";
pub const SAVING: &str = "Saving…";
pub const CODE_MISSING: &str = "Type the code that the administrator gave you.";
pub const NAME_MISSING: &str = "Choose a name in the settings first.";

/// The name of this device as a server lists it: the kind of system and nothing of the
/// machine or of the person.
pub fn device_name() -> String {
    let system = match std::env::consts::OS {
        "macos" => "macOS",
        "linux" => "Linux",
        "windows" => "Windows",
        other => other,
    };
    format!("{APP_NAME} on {system}")
}

pub fn request_access_help(server: &str) -> String {
    format!(
        "You have no code. An administrator of {server} must approve the request before you \
         can enter."
    )
}

pub fn request_sent(server: &str, name: &str) -> String {
    let sender = if name.is_empty() {
        String::new()
    } else {
        format!("Sent as {name}. ")
    };
    format!(
        "{sender}{server} stays in your server book as “{STATUS_WAITING_FOR_APPROVAL}” and \
         opens when the request is approved. You can close this window."
    )
}

pub fn key_of_other_address(stored: &str) -> String {
    format!(
        "The key was created for {stored}. Junto does not use it at another address, because \
         an address that copies the identifier of a server would get its signatures. Discard \
         the key only when the server moved to this address; this device then joins as a new \
         device."
    )
}

pub fn link_code(code: &str) -> String {
    format!("Link code: {code}")
}

// The first start and the settings.
pub const WELCOME_TITLE: &str = "Your name";
pub const WELCOME: &str = "People on a server see this name. A new account on a server starts \
     with it, and you can change it in the settings.";
pub const NAME_FIELD: &str = "Name";
pub const CONTINUE: &str = "Continue";
pub const SETTINGS: &str = "Settings";
pub const PROFILE: &str = "Profile";
pub const PROFILE_NAME_HELP: &str =
    "A new account on a server starts with this name. Accounts that exist keep their name.";
pub const SAVE: &str = "Save";
pub const SAVED: &str = "Saved";
pub const INTERFACE: &str = "Interface";
pub const SCALE: &str = "Scale";
pub const SCALE_HELP: &str = "The size of text and controls in this window.";
pub const DATA_DIRECTORY: &str = "Data directory";
pub const DATA_DIRECTORY_HELP: &str =
    "The server book, the profile and the device keys of this installation are in this directory.";

pub fn scale_percent(scale: u32) -> String {
    format!("{scale} %")
}

// The menu of a server.
pub const KEEP_CONNECTED: &str = "Keep connected in the background";
pub const KEEP_CONNECTED_HELP: &str = "Off: the server connects only while it is selected.";
pub const ON: &str = "On";
pub const OFF: &str = "Off";
pub const REMOVE_SERVER_HELP: &str = "Deletes the entry and the key of this device.";
pub const REMOVE: &str = "Remove";

pub const REMOVE_SERVER_TITLE: &str = "Remove this server from this device?";

pub const REMOVE_SERVER_CONFIRM: &str = "The entry in the server book and the key of this \
     device for the server are deleted. Your account on the server stays. This device can \
     use it again only through a device link or a recovery.";

pub const FORGET_ACCOUNT_TITLE: &str = "Forget the account on this server?";
pub const ACCOUNT_CHANGED: &str = "This device now belongs to another account on this server \
     than at its last contact. The lists are the lists of that account.";

pub const FORGET_ACCOUNT_CONFIRM: &str = "The key of this device for the server is deleted. \
     The server stays in the server book, and this device can join it as a new account. A \
     banned account stays banned.";
pub const FORGET: &str = "Forget";

// Recovery notices.
pub const RECOVERY_SEEN: &str = "I have seen this";
pub const RECOVERY_SENDING: &str = "Sending…";

pub fn recovery(server: &str, when: &str, by_host: bool) -> String {
    let how = if by_host {
        "with the host command of the server"
    } else {
        "by the server owner"
    };
    format!(
        "A recovery gave a new device access to your account on {server} on {when}, {how}. If \
         you did not ask for it, tell an administrator of the server."
    )
}

/// The sentence for a failed action of a flow.
pub fn flow_error(error: &FlowError) -> String {
    match error {
        FlowError::NotAvailable => "This action is not available now.".to_owned(),
        FlowError::Busy => "Another action for this server is in progress.".to_owned(),
        FlowError::InvalidInput(reason) => sentence(reason),
        FlowError::Unreachable(error) => match error.kind {
            TransportErrorKind::Connect => "No connection to the server. Check the address \
                 and the network connection."
                .to_owned(),
            TransportErrorKind::Timeout => {
                "The server did not answer in time. Try again.".to_owned()
            }
            TransportErrorKind::Interrupted => {
                "The connection to the server was interrupted. Try again.".to_owned()
            }
            TransportErrorKind::TooLarge => {
                "The server sent an answer that is larger than Junto reads.".to_owned()
            }
        },
        FlowError::Unavailable => "The server failed or its answer was lost. Try again.".to_owned(),
        // A ceiling of the server has a sentence of its own; a limit on attempts names
        // the wait (ADR 0111, ADR 0112).
        FlowError::Limited {
            code: code @ (ErrorCode::JoinRequestsFull | ErrorCode::DeviceLinksFull),
            ..
        } => refusal(*code),
        FlowError::Limited { retry_after, .. } => match retry_after {
            Some(wait) => format!(
                "The server limits attempts. Try again in {} seconds.",
                wait.as_secs().max(1)
            ),
            None => "The server limits attempts. Try again later.".to_owned(),
        },
        FlowError::Refused { code, .. } => refusal(*code),
        FlowError::UnknownCode {
            status, message, ..
        } => {
            if message.is_empty() {
                format!(
                    "The server refused with an error that this version of Junto does not \
                     know (status {status})."
                )
            } else {
                format!(
                    "The server refused with an error that this version of Junto does not \
                     know (status {status}). The server says: “{message}”"
                )
            }
        }
        FlowError::OriginRejected => "The server does not accept a login through this \
             address. The address is not one of the public addresses of the server; its \
             administrator sets them."
            .to_owned(),
        FlowError::OtherServer => {
            "The address answers as another server than the saved one.".to_owned()
        }
        FlowError::AlreadySaved { .. } => "This server is already in your server book.".to_owned(),
        FlowError::Incompatible(reason) => {
            let (title, detail) = incompatibility(*reason);
            format!("{title}. {detail}")
        }
        FlowError::Challenge(_) => "The server sent a login challenge that does not match the \
             request. Junto did not sign it."
            .to_owned(),
        FlowError::InvalidResponse => {
            "The server answered in a form that Junto cannot read.".to_owned()
        }
        FlowError::NotLoggedIn => "This device is not logged in to the server.".to_owned(),
        FlowError::AccessLost => "The account lost its access to the server.".to_owned(),
        FlowError::AccountChanged { .. } => "This device now belongs to another account on \
             the server. The action was not done."
            .to_owned(),
        FlowError::KeyOfOtherAddress { stored } => format!(
            "This device holds a key of this server that was created for {stored}. It is not \
             used at this address."
        ),
        FlowError::UnknownServer => "The server is not in the server book.".to_owned(),
        FlowError::Expired => "The code expired.".to_owned(),
        FlowError::InvalidBundle(kind) => format!("The text is not a valid {kind}."),
        FlowError::Incomplete { failed, total } => {
            format!("{failed} of {total} servers did not succeed.")
        }
        FlowError::Storage(reason) => {
            format!("A file of the data directory could not be used: {reason}.")
        }
        FlowError::Stopped => "Junto is shutting down.".to_owned(),
    }
}

/// The sentence for a refusal of a server with a code that this build knows.
pub fn refusal(code: ErrorCode) -> String {
    let known = match code {
        ErrorCode::InviteNotFound => "The server does not know this invite code. Check it.",
        ErrorCode::InviteExpired => "This invite code has expired. Ask for a new one.",
        ErrorCode::InviteExhausted => {
            "This invite code was used as often as it allows. Ask for a new one."
        }
        ErrorCode::InviteRevoked => "This invite code was revoked. Ask for a new one.",
        ErrorCode::OwnerCodeInvalid => {
            "The server does not accept this owner code. A restart of the server prints a new \
             one."
        }
        ErrorCode::OwnerAlreadyClaimed => {
            "The server already has an owner. Join with an invite code."
        }
        ErrorCode::KeyAlreadyRegistered => {
            "The key of this device already belongs to an account on the server."
        }
        ErrorCode::JoinRequestPending => "A request of this device already waits on the server.",
        ErrorCode::JoinRequestsFull => {
            "The server holds as many waiting requests for access as it allows. Try again \
             later, or ask an administrator for an invite code."
        }
        ErrorCode::DeviceLinksFull => {
            "The server holds as many waiting device links as it allows. Try again in a few \
             minutes."
        }
        ErrorCode::RateLimited => "The server limits attempts. Try again later.",
        ErrorCode::AccountBanned => "The account of this device is banned on the server.",
        ErrorCode::UnknownDevice => "The server does not know the key of this device.",
        ErrorCode::LinkCodeInvalid => "The server does not know this link code, or it expired.",
        ErrorCode::PublicKeyMismatch => {
            "The key behind the link code is not the key that was confirmed."
        }
        ErrorCode::Forbidden | ErrorCode::OwnerOnly | ErrorCode::ExceedsOwnPermissions => {
            "Your account may not do this on the server."
        }
        ErrorCode::OwnerProtected | ErrorCode::OwnAccountTarget => {
            "The server does not allow this for that account."
        }
        ErrorCode::NotFound => "The server does not know what was asked for.",
        ErrorCode::Conflict => "The request does not fit the present state of the server.",
        ErrorCode::LimitReached => "A limit of the server is reached.",
        ErrorCode::InvalidRequest => "The server did not accept the request.",
        ErrorCode::ImageTooLarge => "The image is larger than the server stores.",
        ErrorCode::ImageUnsupported => {
            "The image is not a PNG, JPEG or WebP image of an accepted size."
        }
        ErrorCode::CallLocked => "The voice channel is locked.",
        ErrorCode::CallFull => "The voice channel is full.",
        ErrorCode::NotInCall => "This account is not in that call.",
        ErrorCode::MediaUnavailable => {
            "The media server of this server cannot be reached now. Try again in a moment."
        }
        ErrorCode::ChallengeInvalid | ErrorCode::ChallengeExpired | ErrorCode::InvalidSignature => {
            "The server did not accept the proof of the device key. Try again."
        }
        ErrorCode::Unauthenticated => "The session of this device has ended.",
        _ => "",
    };
    if known.is_empty() {
        format!("The server refused the request ({}).", code_name(code))
    } else {
        known.to_owned()
    }
}

// Messages (ADR 0740).
pub const MESSAGES: &str = "Messages";
pub const MESSAGES_LOADING: &str = "Reading the messages…";
pub const MESSAGES_FAILED_TITLE: &str = "The messages could not be read";
pub const MESSAGES_FAILED: &str = "The server did not answer, or it refused the request.";
pub const MESSAGES_GONE: &str = "The server does not give this place to your account.";
pub const MESSAGES_STALE: &str = "These messages may miss recent changes.";
pub const READ_AGAIN: &str = "Read again";
pub const NO_MESSAGES_TITLE: &str = "No messages yet";
pub const NO_MESSAGES: &str = "Write the first message of this place.";
pub const HISTORY_START: &str = "This is the start of the history.";
pub const EARLIER_MESSAGES: &str = "Earlier messages";
pub const READING_EARLIER: &str = "Reading earlier messages…";
pub const LATER_MESSAGES: &str = "Later messages";
pub const READING_LATER: &str = "Reading later messages…";
pub const GO_TO_END: &str = "Go to the newest messages";
pub const NEW_MESSAGES: &str = "New";
pub const MARK_AS_READ: &str = "Mark as read";
pub const MARK_AS_READ_HELP: &str = "Mark every message of this channel as read";
pub const EDITED: &str = "edited";
pub const DELETED_MESSAGE: &str = "This message was deleted.";
pub const QUOTE_DELETED: &str = "Deleted message";
pub const QUOTE_UNKNOWN: &str = "A message that is not loaded";
pub const UNKNOWN_ROLE: &str = "unknown role";
pub const MENTION_EVERYONE: &str = "@everyone";
pub const EVERYONE: &str = "everyone";
pub const EVERYONE_HELP: &str = "Everyone who can read this channel";
pub const SOMEBODY: &str = "somebody";
pub const REPLY: &str = "Reply";
pub const EDIT: &str = "Edit";
pub const DELETE: &str = "Delete";
pub const REACT: &str = "Add a reaction";
pub const MESSAGE_ACTIONS: &str = "Actions of this message";
pub const DELETE_MESSAGE_TITLE: &str = "Delete this message?";
pub const DELETE_MESSAGE_CONFIRM: &str =
    "The message and its files are removed for everyone. This cannot be undone.";
pub const SENDING: &str = "sending…";
pub const WAITING_FOR_FILES: &str = "uploading its files…";
pub const NOT_CONFIRMED: &str = "No answer from the server. The message may have arrived.";
pub const NOT_SENT: &str = "Not sent: no connection to the server.";
pub const FILE_NOT_SENT: &str = "Not sent: a file was not uploaded.";
pub const SEND_AGAIN: &str = "Send again";
pub const DISCARD: &str = "Discard";
pub const SEND: &str = "Send";
pub const SEND_HINT: &str = "Enter sends · Shift + Enter adds a line";
pub const SAVE_EDIT: &str = "Save";
pub const EDITING_MESSAGE: &str = "Editing your message";
pub const BOLD: &str = "Bold";
pub const ITALIC: &str = "Italic";
pub const STRIKETHROUGH: &str = "Strikethrough";
pub const INLINE_CODE: &str = "Inline code";
pub const CODE_BLOCK: &str = "Code block";
pub const LINK: &str = "Link";
pub const LIST: &str = "List";
pub const QUOTE: &str = "Quote";
pub const ATTACH_FILE: &str = "Attach a file";
pub const EMOJI: &str = "Emoji";
pub const FIND_EMOJI: &str = "Find an emoji";
pub const NO_EMOJI_FOUND: &str = "No emoji has this name.";
pub const MENTION_SOMEONE: &str = "Mention a person or a role";
pub const NO_MENTION_FOUND: &str = "No member or role has this name.";
pub const REMOVE_FILE: &str = "Remove this file";
pub const REMOVE_FAILED_FILE: &str = "A file was not uploaded. Remove it, or attach it again.";
pub const MEMBER: &str = "member";
pub const ROLE: &str = "role";
pub const DROP_FILES: &str = "Drop files to attach them";
pub const PASTED_PICTURE: &str = "pasted-picture.png";
pub const DOWNLOAD: &str = "Download";
pub const DOWNLOADING: &str = "Downloading…";
pub const DOWNLOAD_CHECKING: &str = "Checking the file…";
pub const DOWNLOAD_SAVED: &str = "Saved";
pub const SHOW_IN_FOLDER: &str = "Show in folder";
pub const CANCEL_DOWNLOAD: &str = "Cancel the download";
pub const DOWNLOAD_INTERRUPTED: &str = "The connection was cut. The download continues by itself.";
pub const UPLOAD_INTERRUPTED: &str = "The connection was cut. The upload continues by itself.";
pub const UPLOAD_READING: &str = "reading the file…";
pub const UPLOAD_CHECKING: &str = "the server checks the file…";
pub const UPLOAD_COMPLETE: &str = "uploaded";
pub const NO_DOWNLOAD_DIRECTORY: &str =
    "This system names no download directory. The file was not saved.";
pub const PICTURE_LOADING: &str = "The picture arrives…";
pub const PICTURE_FAILED: &str = "The picture could not be shown. You can download the file.";
pub const OPENS_IN_BROWSER: &str = "Opens in your browser";

// Threads, pins, permanent messages and direct conversations (ADR 0741).
pub const THREAD: &str = "Thread";
pub const CLOSE_THREAD: &str = "Close the thread";
pub const REPLY_IN_THREAD: &str = "Reply in thread";
pub const OPEN_THREAD: &str = "Open the thread";
pub const SHOWN_IN_PANEL: &str = "Shown in panel";
pub const THREAD_LOADING: &str = "Reading the thread…";
pub const THREAD_GONE_TITLE: &str = "This thread does not exist any more";
pub const THREAD_GONE: &str = "Its first message was deleted, or the channel is not open to you.";
pub const THREAD_FAILED: &str = "The thread could not be read.";
pub const NO_REPLIES: &str = "No replies yet. Write the first one.";
pub const THREADS_OF_CHANNEL: &str = "Threads with something new";
pub const THREAD_IN_CALL: &str = "This thread opens after you leave the call of its channel.";
pub const FORK_THREAD: &str = "Fork thread into…";
pub const FORK_COPIES: &str = "replies are copied to it";
pub const FORK_TEXT: &str = "Text channel";
pub const FORK_VOICE: &str = "Voice channel";
pub const FORK_TITLE: &str = "Fork this thread into a channel";
pub const FORK_ABOUT: &str = "The new channel gets a copy of the first message and of every \
     reply, in the category and with the permissions of this channel. The thread stays and \
     shows a link to the channel. A thread can be forked once.";
pub const FORK_NAME: &str = "Name of the new channel";
pub const FORK_CREATE: &str = "Create the channel";
pub const FORK_SENDING: &str = "Creating the channel…";
pub const FORK_NOT_ALLOWED: &str = "Your roles do not allow this. A fork needs \"manage channel\" in this channel and in \
     its workspace.";
pub const FORK_NEEDS_PERMISSION: &str = "A fork needs the permission \"manage channel\".";
pub const FORK_EXISTS: &str = "This thread was forked already. A thread has one fork.";
pub const FORK_THREAD_GONE: &str = "This thread does not exist any more.";
pub const FORK_NAME_REFUSED: &str =
    "The server does not take this name. A name has 1 to 64 characters.";
pub const FORKED_INTO: &str = "Forked into";
pub const PINS: &str = "Pins";
pub const SHOW_PINS: &str = "Show the pinned messages";
pub const CLOSE_PINS: &str = "Close the pinned messages";
pub const PINS_LOADING: &str = "Reading the pinned messages…";
pub const PINS_FAILED: &str = "The pinned messages could not be read.";
pub const NO_PINS_TITLE: &str = "No pinned messages";
pub const NO_PINS: &str = "A person with \"manage messages\" pins a message from its actions.";
pub const PIN: &str = "Pin";
pub const UNPIN: &str = "Unpin";
pub const PINNED: &str = "Pinned";
pub const PIN_LIMIT: &str = "This channel has 100 pinned messages. Unpin one first.";
pub const PIN_NOT_ALLOWED: &str = "Your roles do not allow this. A pin needs \"manage messages\".";
pub const GO_TO_MESSAGE: &str = "Go to message";
pub const PERMANENT_MESSAGES: &str = "Permanent messages";
pub const PERMANENT_ON: &str = "Permanent messages are on.";
pub const PERMANENT_ABOUT: &str = " Nobody can edit a message in this channel. Only an \
     administrator can delete one. Each deletion leaves a line here and an entry in the \
     audit log.";
pub const NO_EDIT_OR_DELETE: &str = "No edit or delete here";
pub const PERMANENT_COMPOSER: &str = "Permanent messages: a sent message cannot be edited";
pub const DIRECT_MESSAGES: &str = "Direct messages";
pub const MESSAGE_PERSON: &str = "Message";
pub const NO_CONTACT: &str =
    "You cannot write to this person: you share no workspace with them on this server.";
pub const CONVERSATION_CLOSED: &str = "You and this person share no workspace on this server any more. The messages are kept \
     and are shown again when you share one.";
pub const NO_DIRECT_MESSAGES: &str = "Write the first message of this conversation.";

pub fn message_person(name: &str) -> String {
    format!("Write a direct message to {name}")
}

/// The text of the field in which a person writes to another person.
pub fn direct_placeholder(name: &str) -> String {
    format!("Message {name}")
}

pub fn replies(count: u64) -> String {
    if count == 1 {
        "1 reply".to_owned()
    } else {
        format!("{count} replies")
    }
}

pub fn last_reply(time: &str) -> String {
    format!("Last reply {time}")
}

pub fn thread_of(channel: &str) -> String {
    format!("Thread in #{channel}")
}

/// What a thread holds that the person has not read, as its sign says it.
pub fn thread_unread(unread: u64, mentions: u64) -> String {
    if mentions > 0 {
        mention_badge(mentions)
    } else {
        format!("{} new", unread_count(unread))
    }
}

/// The text of the field in which a person writes a message of a channel.
pub fn message_placeholder(name: &str, voice: bool) -> String {
    if voice {
        format!("Message {name}")
    } else {
        format!("Message #{name}")
    }
}

pub fn replying_to(name: &str) -> String {
    format!("Replying to {name}")
}

/// A date as a day separator shows it: `Thursday, 8 October`, with the year when it is
/// not the year of `today`.
pub fn day(date: time::Date, today: Option<time::Date>) -> String {
    let named = format!("{}, {} {}", date.weekday(), date.day(), date.month());
    if today.is_some_and(|today| today.year() == date.year()) {
        named
    } else {
        format!("{named} {}", date.year())
    }
}

/// The size of a file with one unit: `1.2 MB`, `284 KB`, `12 bytes`.
pub fn file_size(bytes: u64) -> String {
    const KB: u64 = 1000;
    const MB: u64 = KB * 1000;
    const GB: u64 = MB * 1000;
    let tenths = |unit: u64| {
        let value = bytes as f64 / unit as f64;
        if value >= 10.0 {
            format!("{value:.0}")
        } else {
            format!("{value:.1}")
        }
    };
    if bytes >= GB {
        format!("{} GB", tenths(GB))
    } else if bytes >= MB {
        format!("{} MB", tenths(MB))
    } else if bytes >= KB {
        format!("{} KB", bytes / KB)
    } else if bytes == 1 {
        "1 byte".to_owned()
    } else {
        format!("{bytes} bytes")
    }
}

/// A count of unread messages or mentions. The server counts up to 99 and tells 100 for
/// more (ADR 0408).
pub fn unread_count(count: u64) -> String {
    if count >= 100 {
        "99+".to_owned()
    } else {
        count.to_string()
    }
}

/// The sign of a channel that mentions the person.
pub fn mention_badge(mentions: u64) -> String {
    format!("@ {}", unread_count(mentions))
}

pub fn unread_tooltip(messages: u64, mentions: u64) -> String {
    let plural = |count: u64, one: &str, many: &str| {
        if count == 1 {
            format!("1 {one}")
        } else {
            format!("{} {many}", unread_count(count))
        }
    };
    if mentions > 0 {
        plural(mentions, "mention", "mentions")
    } else {
        plural(messages, "new message", "new messages")
    }
}

pub fn reaction_label(emoji: &str, count: u32, own: bool) -> String {
    if own {
        format!("{emoji} {count}, with yours. Press to take yours away.")
    } else {
        format!("{emoji} {count}. Press to add yours.")
    }
}

pub fn transfer_progress(done: u64, total: u64) -> String {
    format!("{} of {}", file_size(done), file_size(total))
}

/// What a system line of a channel states, and the mark at its start. `{actor}` and
/// `{subject}` stand for the names of the accounts of the line.
pub fn system_line(action: &str) -> (crate::state::feed::SystemKind, &'static str) {
    use crate::state::feed::SystemKind::{Plain, Recording, Start};
    match action {
        "call_started" => (Start, "Call started by {actor}"),
        "call_ended" => (Plain, "Call ended"),
        "screen_share_started" => (Start, "{actor} started sharing"),
        "entry_requested" => (Plain, "{actor} asked to enter"),
        "joined_by_invitation" => (Plain, "{actor} joined by invitation"),
        "meeting_started" => (Start, "{actor} turned meeting mode on"),
        "meeting_ended" => (Plain, "Meeting mode ended"),
        "recording_started" => (Recording, "Recording started by {actor}"),
        "recording_stopped" => (Recording, "Recording stopped"),
        "recording_failed" => (Recording, "Recording failed"),
        "recording_ready" => (Recording, "The recording of the call is ready"),
        "recording_removed" => (Recording, "{actor} removed a recording"),
        "transcript_requested" => (Plain, "{actor} asked for a transcript"),
        "channel_created" => (Plain, "{actor} created this channel"),
        "channel_updated" => (Plain, "{actor} changed this channel"),
        "channel_override_set" => (Plain, "{actor} changed the permissions of this channel"),
        "channel_call_settings_updated" => (Plain, "{actor} changed the settings of the room"),
        "call_microphone_blocked" => (Plain, "{actor} closed the microphone of {subject}"),
        "call_microphone_unblocked" => (Plain, "{actor} removed the microphone block of {subject}"),
        "call_sound_blocked" => (Plain, "{actor} turned the sound of {subject} off"),
        "call_sound_unblocked" => (Plain, "{actor} removed the sound block of {subject}"),
        "call_member_disconnected" => (Plain, "{actor} removed {subject} from the call"),
        "call_member_moved_out" => (Plain, "{actor} moved {subject} to another voice channel"),
        "call_member_moved_in" => (Plain, "{actor} moved {subject} into this voice channel"),
        "permanent_message_deleted" => (Plain, "{actor} deleted a permanent message"),
        "channel_archived" => (Plain, "{actor} archived this channel"),
        "channel_restored" => (Plain, "{actor} restored this channel"),
        "channel_deletion_due" => (Plain, "The server deletes this archived channel in 7 days"),
        "export_requested" => (Plain, "{actor} asked for an export of this channel"),
        // The lines of a call between the two accounts of a direct conversation
        // (ADR 0518). The actor of the first two is the caller.
        "direct_call_started" => (Start, "{actor} started a call"),
        "direct_call_not_answered" => (Plain, "{subject} did not answer the call of {actor}"),
        "direct_call_declined" => (Plain, "{actor} declined the call"),
        "direct_call_ended" => (Plain, "The call ended{duration}"),
        _ => (Plain, "An event that this version of Junto does not know"),
    }
}

/// The length of a call as a system line states it after "The call ended": ` after 4
/// minutes 12 seconds`. Nothing for a line without a length.
pub fn call_length(seconds: Option<u64>) -> String {
    let Some(seconds) = seconds else {
        return String::new();
    };
    let count = |number: u64, one: &str, many: &str| match number {
        1 => format!("1 {one}"),
        number => format!("{number} {many}"),
    };
    let (hours, minutes, rest) = (seconds / 3600, seconds % 3600 / 60, seconds % 60);
    let length = if hours > 0 {
        format!(
            "{} {}",
            count(hours, "hour", "hours"),
            count(minutes, "minute", "minutes")
        )
    } else if minutes > 0 {
        format!(
            "{} {}",
            count(minutes, "minute", "minutes"),
            count(rest, "second", "seconds")
        )
    } else {
        count(rest, "second", "seconds")
    };
    format!(" after {length}")
}

/// Why a document is not sent as it is.
pub fn compose_problem(problem: crate::compose::Problem) -> String {
    use crate::compose::Problem;
    match problem {
        Problem::Empty => "Write a text or attach a file.".to_owned(),
        Problem::TooLong { bytes } => format!(
            "The message is too long: {} of {} that a message can have with its formatting.",
            file_size(bytes as u64),
            file_size(crate::compose::MOST_BYTES as u64)
        ),
        Problem::TooManyBlocks { blocks } => format!(
            "The message has {blocks} paragraphs, lists and blocks. A message can have {}.",
            crate::compose::MOST_BLOCKS
        ),
        Problem::TooManyNodes { nodes } => format!(
            "The message has {nodes} formatted parts. A message can have {}.",
            crate::compose::MOST_NODES
        ),
        Problem::TooManyMentions { mentions } => format!(
            "The message mentions {mentions} people and roles. A message can mention {}.",
            crate::compose::MOST_MENTIONS
        ),
    }
}

/// Why a file was not attached to a message.
pub fn attach_refusal(refusal: &crate::state::AttachRefusal) -> String {
    use crate::state::AttachRefusal;
    match refusal {
        AttachRefusal::TooLarge { name, limit } => format!(
            "{name} is larger than {}, the most that this server takes for a file.",
            file_size(*limit)
        ),
        AttachRefusal::TooMany { limit } => {
            format!("A message on this server has at most {limit} files.")
        }
        AttachRefusal::Unreadable { name } => format!("{name} could not be read."),
    }
}

/// Why a message that waits was not sent.
pub fn send_failure(failure: junto_client_core::model::SendFailure) -> String {
    use junto_client_core::model::SendFailure;
    match failure {
        SendFailure::NotSent => NOT_SENT.to_owned(),
        SendFailure::File => FILE_NOT_SENT.to_owned(),
        SendFailure::Refused { code, status } => format!(
            "Not sent. {}",
            match code {
                ErrorCode::RateLimited => {
                    "The server limits how many messages an account sends in a minute. Send \
                     it again in a moment."
                        .to_owned()
                }
                ErrorCode::Forbidden => {
                    "Your account may not write here, or may not mention everyone.".to_owned()
                }
                ErrorCode::NotFound => {
                    "The server does not give this place to your account, or the message \
                     that you answer is gone."
                        .to_owned()
                }
                ErrorCode::InvalidRequest => {
                    "The server did not accept the message: its text, a mention or a file \
                     is not one that it takes."
                        .to_owned()
                }
                ErrorCode::Unknown => format!(
                    "The server refused with an error that this version of Junto does not \
                     know (status {status})."
                ),
                code => refusal(code),
            }
        ),
    }
}

/// Why a file did not go up or down.
pub fn transfer_failure(failure: junto_client_core::transfers::TransferFailure) -> String {
    use junto_client_core::transfers::TransferFailure;
    match failure {
        TransferFailure::File(_) => "The file could not be read or written.".to_owned(),
        TransferFailure::Empty => "The file is empty. A server stores no empty file.".to_owned(),
        TransferFailure::Changed => {
            "The file changed while it was read. Attach it again.".to_owned()
        }
        TransferFailure::Corrupt => {
            "The bytes that arrived are not the file of the message. Nothing was saved.".to_owned()
        }
        TransferFailure::Refused { code, .. } => match code {
            ErrorCode::StorageLow => {
                "The server has too little free disk space for this file.".to_owned()
            }
            ErrorCode::LimitReached => {
                "Your account has 20 uploads that no message uses. They expire after a day."
                    .to_owned()
            }
            ErrorCode::InvalidRequest => {
                "The server did not accept the file: its name or its size is not one that \
                 it takes."
                    .to_owned()
            }
            ErrorCode::Unavailable => {
                "This server stores no files: its administrator has not set a directory \
                 for them."
                    .to_owned()
            }
            code => refusal(code),
        },
        TransferFailure::Stopped => "The transfer was stopped.".to_owned(),
    }
}

/// A reason that the client core states as a clause, as a sentence.
fn sentence(reason: &str) -> String {
    let mut text = String::with_capacity(reason.len() + 1);
    let mut characters = reason.chars();
    if let Some(first) = characters.next() {
        text.extend(first.to_uppercase());
        text.push_str(characters.as_str());
    }
    if !text.ends_with(['.', '!', '?']) {
        text.push('.');
    }
    text
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use junto_client_core::address::ServerAddress;
    use junto_client_core::api::{ErrorCategory, TransportError, category};
    use junto_client_core::protocol::ids::{AccountId, ServerId};

    use super::*;

    fn every_flow_error() -> Vec<FlowError> {
        let transport = |kind| FlowError::Unreachable(TransportError::new(kind, "detail"));
        let mut all = vec![
            FlowError::NotAvailable,
            FlowError::Busy,
            FlowError::InvalidInput("the address is empty".to_owned()),
            transport(TransportErrorKind::Connect),
            transport(TransportErrorKind::Timeout),
            transport(TransportErrorKind::Interrupted),
            transport(TransportErrorKind::TooLarge),
            FlowError::Unavailable,
            FlowError::Limited {
                code: ErrorCode::RateLimited,
                retry_after: None,
            },
            FlowError::Limited {
                code: ErrorCode::RateLimited,
                retry_after: Some(Duration::from_secs(30)),
            },
            FlowError::UnknownCode {
                status: 409,
                category: ErrorCategory::Conflict,
                retry_after: None,
                message: String::new(),
            },
            FlowError::OriginRejected,
            FlowError::OtherServer,
            FlowError::AlreadySaved {
                server_id: ServerId::generate(),
            },
            FlowError::Incompatible(Incompatibility::ClientTooOld { server_oldest: 3 }),
            FlowError::Incompatible(Incompatibility::ServerTooOld { server_newest: 1 }),
            FlowError::Incompatible(Incompatibility::NoCommonVersion),
            FlowError::Incompatible(Incompatibility::NoVersions),
            FlowError::Challenge(junto_client_core::api::ChallengeMismatch::Operation),
            FlowError::InvalidResponse,
            FlowError::NotLoggedIn,
            FlowError::AccessLost,
            FlowError::AccountChanged {
                was: AccountId::generate(),
                now: AccountId::generate(),
            },
            FlowError::KeyOfOtherAddress {
                stored: ServerAddress::parse("old.example").unwrap(),
            },
            FlowError::UnknownServer,
            FlowError::Expired,
            FlowError::InvalidBundle("link offer"),
            FlowError::Incomplete {
                failed: 1,
                total: 3,
            },
            FlowError::Storage("the file is locked".to_owned()),
            FlowError::Stopped,
        ];
        all.extend(ErrorCode::ALL.iter().map(|code| FlowError::Refused {
            code: *code,
            category: category(*code, code.http_status()),
        }));
        all
    }

    #[test]
    fn every_failure_of_a_flow_has_a_sentence() {
        for error in every_flow_error() {
            let text = flow_error(&error);
            assert!(text.len() > 12, "{error:?}: {text}");
            assert!(text.ends_with(['.', '”']), "{error:?}: {text}");
            assert!(
                text.chars()
                    .next()
                    .is_some_and(|first| first.is_uppercase() || first.is_ascii_digit()),
                "{error:?}: {text}"
            );
        }
    }

    /// A match without a wildcard: a kind of failure that the client core adds does not
    /// compile until it is in the list above.
    #[test]
    fn the_list_of_failures_names_every_kind() {
        let kinds: std::collections::BTreeSet<&str> = every_flow_error()
            .iter()
            .map(|error| match error {
                FlowError::NotAvailable => "not available",
                FlowError::Busy => "busy",
                FlowError::InvalidInput(_) => "invalid input",
                FlowError::Unreachable(_) => "unreachable",
                FlowError::Unavailable => "unavailable",
                FlowError::Limited { .. } => "limited",
                FlowError::Refused { .. } => "refused",
                FlowError::UnknownCode { .. } => "unknown code",
                FlowError::OriginRejected => "origin rejected",
                FlowError::OtherServer => "other server",
                FlowError::AlreadySaved { .. } => "already saved",
                FlowError::Incompatible(_) => "incompatible",
                FlowError::Challenge(_) => "challenge",
                FlowError::InvalidResponse => "invalid response",
                FlowError::NotLoggedIn => "not logged in",
                FlowError::AccessLost => "access lost",
                FlowError::AccountChanged { .. } => "account changed",
                FlowError::KeyOfOtherAddress { .. } => "key of other address",
                FlowError::UnknownServer => "unknown server",
                FlowError::Expired => "expired",
                FlowError::InvalidBundle(_) => "invalid bundle",
                FlowError::Incomplete { .. } => "incomplete",
                FlowError::Storage(_) => "storage",
                FlowError::Stopped => "stopped",
            })
            .collect();
        assert_eq!(kinds.len(), 24);
    }

    #[test]
    fn a_refusal_of_a_join_names_what_the_person_can_do() {
        for (code, word) in [
            (ErrorCode::InviteNotFound, "Check"),
            (ErrorCode::InviteExpired, "new one"),
            (ErrorCode::InviteExhausted, "new one"),
            (ErrorCode::InviteRevoked, "new one"),
            (ErrorCode::OwnerCodeInvalid, "owner code"),
            (ErrorCode::OwnerAlreadyClaimed, "invite code"),
            (ErrorCode::KeyAlreadyRegistered, "already belongs"),
            (ErrorCode::JoinRequestPending, "already waits"),
            (ErrorCode::JoinRequestsFull, "invite code"),
            (ErrorCode::DeviceLinksFull, "few minutes"),
            (ErrorCode::RateLimited, "Try again later"),
            (ErrorCode::AccountBanned, "banned"),
            (ErrorCode::LinkCodeInvalid, "link code"),
        ] {
            assert!(refusal(code).contains(word), "{code:?}");
        }
        // The server tells how long a limit on attempts lasts (ADR 0111): the sentence
        // names the wait.
        assert_eq!(
            flow_error(&FlowError::Limited {
                code: ErrorCode::RateLimited,
                retry_after: Some(Duration::from_secs(42)),
            }),
            "The server limits attempts. Try again in 42 seconds."
        );
        // A ceiling of the server reaches a flow with its code and has its own sentence.
        assert_eq!(
            flow_error(&FlowError::refused(ErrorCode::RateLimited)),
            "The server limits attempts. Try again later."
        );
        for code in [ErrorCode::JoinRequestsFull, ErrorCode::DeviceLinksFull] {
            assert_eq!(
                flow_error(&FlowError::refused(code)),
                refusal(code),
                "{code:?}"
            );
            assert!(!refusal(code).contains("refused the request"), "{code:?}");
        }
        // A code without a sentence of its own is named as the server writes it.
        assert_eq!(
            refusal(ErrorCode::StorageLow),
            "The server refused the request (storage_low)."
        );
    }

    #[test]
    fn the_sentence_of_a_server_is_shown_only_as_a_quotation_of_an_unknown_code() {
        let quoted = flow_error(&FlowError::UnknownCode {
            status: 418,
            category: ErrorCategory::ClientFault,
            retry_after: None,
            message: "teapots only".to_owned(),
        });
        assert!(quoted.contains("status 418") && quoted.ends_with("“teapots only”"));
    }

    #[test]
    fn every_end_of_a_call_and_every_refused_join_has_a_sentence_that_names_the_channel() {
        use junto_client_core::protocol::call::LeaveReason as L;
        let daily = Some("Daily".to_owned());
        let refused = |code: ErrorCode| FlowError::Refused {
            code,
            category: category(code, code.http_status()),
        };
        let mut notices = vec![
            CallNotice::JoinFailed {
                channel: daily.clone(),
                error: refused(ErrorCode::CallLocked),
            },
            CallNotice::JoinFailed {
                channel: daily.clone(),
                error: refused(ErrorCode::CallFull),
            },
            CallNotice::JoinFailed {
                channel: daily.clone(),
                error: refused(ErrorCode::MediaUnavailable),
            },
            CallNotice::JoinFailed {
                channel: daily.clone(),
                error: FlowError::Unavailable,
            },
            CallNotice::Ended {
                channel: daily.clone(),
                reason: EndReason::Server(None),
            },
            CallNotice::Ended {
                channel: daily.clone(),
                reason: EndReason::Elsewhere,
            },
            CallNotice::Ended {
                channel: daily.clone(),
                reason: EndReason::Refused(refused(ErrorCode::CallLocked)),
            },
        ];
        for reason in [
            L::Left,
            L::Disconnected,
            L::Moved,
            L::Switched,
            L::ConnectionLost,
            L::NeverConnected,
            L::AccessEnded,
        ] {
            notices.push(CallNotice::Ended {
                channel: daily.clone(),
                reason: EndReason::Server(Some(reason)),
            });
        }
        for notice in &notices {
            let text = call_notice(notice);
            assert!(text.contains("Daily"), "{notice:?}: {text}");
            assert!(text.ends_with('.'), "{notice:?}: {text}");
        }
        assert!(call_notice(&notices[0]).contains("invited or moved in"));
        assert!(call_notice(&notices[2]).contains("media server"));
        // A channel that the state no longer holds is not named.
        let unnamed = call_notice(&CallNotice::Ended {
            channel: None,
            reason: EndReason::Elsewhere,
        });
        assert!(unnamed.contains(A_VOICE_CHANNEL));
    }

    #[test]
    fn the_line_about_the_sound_of_this_device_names_who_closed_it() {
        let ana = Some("Ana".to_owned());
        let (strong, rest) = sound_warning(&SoundWarning::MicrophoneHeld {
            reason: BlockReason::Moderator,
            by: ana.clone(),
        });
        assert_eq!(strong, "Microphone closed by a moderator");
        assert_eq!(rest, " (Ana). Only a moderator can open it.");
        let (_, rest) = sound_warning(&SoundWarning::MicrophoneHeld {
            reason: BlockReason::Moderator,
            by: None,
        });
        assert_eq!(rest, ". Only a moderator can open it.");
        let (strong, rest) = sound_warning(&SoundWarning::MicrophoneHeld {
            reason: BlockReason::Meeting,
            by: ana.clone(),
        });
        assert!(strong.ends_with("meeting mode") && rest.contains("Ana runs it"));
        for warning in [
            SoundWarning::NoMicrophoneAccess,
            SoundWarning::MicrophoneFailed,
            SoundWarning::SoundHeld { by: ana },
            SoundWarning::OutputFailed,
        ] {
            let (strong, rest) = sound_warning(&warning);
            assert!(!strong.is_empty() && rest.ends_with('.'), "{warning:?}");
        }
        assert_eq!(
            closed_by(BlockReason::Moderator, Some("Ana")),
            "Closed by a moderator (Ana)"
        );
        assert_eq!(
            closed_by(BlockReason::Meeting, Some("Ana")),
            "Closed by meeting mode"
        );
        for access in [
            MicrophoneAccess::Allowed,
            MicrophoneAccess::NotAsked,
            MicrophoneAccess::Refused,
        ] {
            assert!(microphone_access(access).starts_with("Microphone access: "));
        }
    }

    #[test]
    fn the_texts_of_states_and_counts() {
        assert_eq!(occupancy(8, 8, true), "8 / 8 · full");
        assert_eq!(occupancy(5, 8, false), "5 / 8");
        assert_eq!(
            voice_channel_topic(0, false),
            "Chat of this voice channel. Nobody is in the call now."
        );
        assert!(voice_channel_topic(1, false).ends_with("1 person is in the call."));
        assert!(voice_channel_topic(3, false).ends_with("3 people are in the call."));
        assert!(voice_channel_topic(3, true).contains("only to the people inside"));
        assert_eq!(call_time(Duration::from_secs(42 * 60 + 17)), "00:42:17");
        assert_eq!(call_time(Duration::from_secs(26 * 3600 + 5)), "26:00:05");
        assert_eq!(system_default(""), "System default");
        assert_eq!(system_default("Headphones"), "System default (Headphones)");
        assert_eq!(scale_percent(125), "125 %");
        assert_eq!(no_account_status(None), STATUS_NOT_JOINED);
        assert_eq!(no_account(Some(Notice::Removed)).0, REMOVED_TITLE);
        assert!(request_sent("Harbor", "Luca").starts_with("Sent as Luca. Harbor stays"));
        assert!(request_sent("Harbor", "").starts_with("Harbor stays"));
        assert_eq!(sentence("the name is empty"), "The name is empty.");
    }

    #[test]
    fn the_texts_of_a_share() {
        use junto_client_core::protocol::ids::AccountId;
        use junto_media::engine::ShareId;

        let mut share = ShareCard {
            id: ShareId::Of(AccountId::generate()),
            account_id: AccountId::generate(),
            name: Some("Carla".to_owned()),
            kind: Some(ShareKind::Screen),
            sound: Some(ShareAudio::System),
            sound_held: false,
            size: Some((1920, 1200)),
            frame_rate: Some(30),
            watched: true,
        };
        assert_eq!(share_title(&share, false), "Carla's screen");
        assert_eq!(share_title(&share, true), "Your screen");
        share.kind = Some(ShareKind::Window);
        assert_eq!(share_title(&share, false), "Carla's window");
        share.kind = None;
        share.name = None;
        assert_eq!(share_title(&share, false), "Unknown person's share");
        assert_eq!(
            share_measure(Some(ShareKind::Screen), Some((1920, 1200)), Some(30)),
            "Entire screen · 1920 × 1200 · 30 fps"
        );
        assert_eq!(
            share_measure(None, Some((1280, 800)), Some(30)),
            "1280 × 800 · 30 fps"
        );
        assert_eq!(share_measure(Some(ShareKind::Window), None, None), "Window");
        assert_eq!(
            share_sound(Some(ShareAudio::System)),
            Some("with system audio")
        );
        assert_eq!(share_sound(None), None);
        assert_eq!(
            watched_sound(Some(ShareAudio::None)),
            Some("This share has no sound.")
        );
        assert_eq!(
            people_hidden(1),
            ("1 person without video".to_owned(), " is hidden.")
        );
        assert_eq!(
            people_hidden(3),
            ("3 people without video".to_owned(), " are hidden.")
        );
        assert_eq!(shares_count(2), "2 shares");
        assert_eq!(screen_name(1), "Screen 1");
        for error in [
            CaptureError::NotAllowed,
            CaptureError::SourceGone,
            CaptureError::Unsupported,
            CaptureError::Interrupted,
            CaptureError::Failed("the stream stopped".to_owned()),
        ] {
            assert!(capture_error(&error).ends_with('.'), "{error:?}");
            assert!(share_ended(&error).starts_with("Your screen share ended. "));
        }
        assert!(
            capture_error(&CaptureError::Failed("the stream stopped".to_owned()))
                .ends_with("The stream stopped.")
        );
        assert!(sound_of_window("Editor").contains("the sound of Editor,"));
    }

    #[test]
    fn the_texts_of_a_recording() {
        let since = time::OffsetDateTime::from_unix_timestamp(1_800_000_000 + 41 * 60).unwrap();
        let problem = |fault: RecordingFault| RecordingProblem {
            fault,
            since: Some(since),
            until: Some(since + time::Duration::minutes(36)),
            segments: 5,
            failed_segments: 2,
        };
        assert_eq!(
            recording_time(Duration::from_secs(39 * 60 + 2)),
            "REC 00:39:02"
        );
        let (strong, rest) = recording_details(&problem(RecordingFault::RecorderLost));
        assert_eq!(strong, "The recorder lost its connection at 09:17 UTC.");
        assert!(
            rest.contains("from 08:41 UTC to 09:17 UTC (36 min)"),
            "{rest}"
        );
        assert!(
            rest.contains("Nothing after 09:17 UTC was recorded."),
            "{rest}"
        );
        assert!(rest.ends_with("To continue, start a new recording."));
        assert_eq!(
            recording_flag(&problem(RecordingFault::RecorderLost)),
            RECORDING_FAILED
        );
        assert_eq!(
            recording_flag(&problem(RecordingFault::StorageReserve)),
            RECORDING_STOPPED
        );
        let (_, rest) = recording_details(&problem(RecordingFault::PartlySaved));
        assert!(rest.contains("2 of its 5 files failed"), "{rest}");
        // A recording that never started has no times.
        let never = RecordingProblem {
            fault: RecordingFault::RecorderUnavailable,
            since: None,
            until: None,
            segments: 0,
            failed_segments: 0,
        };
        let (strong, rest) = recording_details(&never);
        assert_eq!(strong, "No recorder took the recording.");
        assert_eq!(
            rest,
            " Nothing was recorded. To continue, start a new recording."
        );
        for fault in [
            RecordingFault::RecorderLost,
            RecordingFault::StorageReserve,
            RecordingFault::SegmentLimit,
            RecordingFault::RecorderUnavailable,
            RecordingFault::NothingSaved,
            RecordingFault::PartlySaved,
            RecordingFault::Other,
        ] {
            let (strong, rest) = recording_details(&problem(fault));
            assert!(strong.ends_with('.') && rest.ends_with('.'), "{fault:?}");
            assert!(rest.starts_with(' '), "{fault:?}");
        }
        assert!(recording_request_failed("No.").ends_with(" No."));
    }
}
