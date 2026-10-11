//! The catalog of the interface text (ADR 0703). The interface language is English; a
//! second language is a second catalog. No other module holds text that a person reads.
//!
//! The text of a failure names what happened and what the person can do. It holds no text
//! of a server, except the quoted sentence of a refusal with a code that this build does
//! not know (ADR 0712).

use junto_client_core::api::{TransportErrorKind, code_name};
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::Notice;
use junto_client_core::protocol::call::BlockReason;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::version::Incompatibility;

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
pub const MICROPHONE_NOT_AVAILABLE: &str = "Microphone. Calls are not in this version.";
pub const SOUND_NOT_AVAILABLE: &str = "Sound output. Calls are not in this version.";
pub const APP_SETTINGS: &str = "App settings";
pub const NO_NAME_YET: &str = "No name yet";

// The main area.
pub const MEMBERS: &str = "Members";
pub const SHOW_MEMBERS: &str = "Show the members of the workspace";
pub const CLOSE_MEMBERS: &str = "Close the member list";
pub const MEMBERS_LOADING: &str = "Reading the members…";
pub const MEMBERS_FAILED: &str = "The members could not be read.";
pub const TRY_AGAIN: &str = "Try again";
pub const MESSAGES_NOT_AVAILABLE_TITLE: &str = "Text messages are not in this version";
pub const MESSAGES_NOT_AVAILABLE: &str = "This version of Junto shows your servers, their \
     workspaces and channels, and the people in voice channels. Messages come with a later \
     version.";
pub const CALLS_NOT_AVAILABLE_TITLE: &str = "Calls are not in this version";
pub const CALLS_NOT_AVAILABLE: &str = "This version of Junto shows who is in a voice channel \
     and the state of its call. Joining a call comes with a later version.";
pub const NOBODY_INSIDE: &str = "Nobody is in this voice channel.";
pub const OCCUPANTS_HIDDEN: &str =
    "This voice channel is in use. It shows who is inside only to the people inside.";
pub const MEETING_MODE: &str = "Meeting mode";
pub const RECORDING_VISIBLE: &str = "visible to everyone";
pub const NO_SERVER_TITLE: &str = "No server yet";
pub const NO_SERVER: &str = "Type the address of a server in the server book. The \
     administrator of the server gives you the address, and a code if the server asks for \
     one.";
pub const NO_CHANNEL_TITLE: &str = "No channel";
pub const STATE_STALE: &str =
    "The connection to this server is interrupted. The lists show the last known state.";

pub fn people_inside(count: usize) -> String {
    match count {
        1 => "1 person is inside.".to_owned(),
        count => format!("{count} people are inside."),
    }
}

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
        FlowError::Limited { retry_after } => match retry_after {
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
            FlowError::Limited { retry_after: None },
            FlowError::Limited {
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
        ] {
            assert!(refusal(code).contains(word), "{code:?}");
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
    fn the_texts_of_states_and_counts() {
        assert_eq!(occupancy(8, 8, true), "8 / 8 · full");
        assert_eq!(occupancy(5, 8, false), "5 / 8");
        assert_eq!(people_inside(1), "1 person is inside.");
        assert_eq!(people_inside(3), "3 people are inside.");
        assert_eq!(scale_percent(125), "125 %");
        assert_eq!(no_account_status(None), STATUS_NOT_JOINED);
        assert_eq!(no_account(Some(Notice::Removed)).0, REMOVED_TITLE);
        assert!(request_sent("Harbor", "Luca").starts_with("Sent as Luca. Harbor stays"));
        assert!(request_sent("Harbor", "").starts_with("Harbor stays"));
        assert_eq!(sentence("the name is empty"), "The name is empty.");
    }
}
