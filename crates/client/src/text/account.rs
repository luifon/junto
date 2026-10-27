//! The text of the account layer and of device linking (ADR 0781, ADR 0782).

use junto_client_core::api::{ApiError, ErrorCategory};
use junto_client_core::flow::FlowError;
use junto_client_core::profile::{InvalidAvatar, InvalidName};
use junto_client_core::protocol::account::DeviceOrigin;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::keys::PublicKey;
use time::OffsetDateTime;

use crate::assets::Icon;
use crate::state::account::{AccountPage, AskKind};
use crate::state::linking::LinkRole;

// The layer.
pub const TITLE: &str = "Your account";
pub const OPEN_ACCOUNT: &str = "Your account";
pub const OPEN_ACCOUNT_HELP: &str =
    "Your name, your picture, your devices and the recoveries on this server";
pub const PAGES: &str = "Pages";
pub const ON_THIS_SERVER: &str = "On this server";
pub const READING: &str = "Reading…";
pub const SENDING: &str = "Sending…";

pub const fn page_name(page: AccountPage) -> &'static str {
    match page {
        AccountPage::Profile => "Profile",
        AccountPage::Devices => "Devices",
        AccountPage::Recoveries => "Recoveries",
        AccountPage::Departure => "Leave or delete",
    }
}

pub const fn page_icon(page: AccountPage) -> Icon {
    match page {
        AccountPage::Profile => Icon::Account,
        AccountPage::Devices => Icon::Devices,
        AccountPage::Recoveries => Icon::History,
        AccountPage::Departure => Icon::Leave,
    }
}

/// The name of the row of a page in the list of pages, for a test.
pub const fn page_selector(page: AccountPage) -> &'static str {
    match page {
        AccountPage::Profile => "account-nav-profile",
        AccountPage::Devices => "account-nav-devices",
        AccountPage::Recoveries => "account-nav-recoveries",
        AccountPage::Departure => "account-nav-departure",
    }
}

/// A moment as the pages of the account show it: the date and the time in UTC, as the
/// notice of a recovery does.
pub fn utc(at: OffsetDateTime) -> String {
    let at = crate::moment::in_utc(at);
    format!(
        "{} {} {} at {:02}:{:02} UTC",
        at.day(),
        at.month(),
        at.year(),
        at.hour(),
        at.minute()
    )
}

/// The public key of a device as a person compares it on two devices: the text that
/// `junto-cli` prints for the key, in groups of four characters. Without the spaces it is
/// that text.
pub fn key_fingerprint(key: &PublicKey) -> String {
    let text = key.to_string();
    let mut grouped = String::with_capacity(text.len() + text.len() / 4);
    for (index, symbol) in text.chars().enumerate() {
        if index > 0 && index % 4 == 0 {
            grouped.push(' ');
        }
        grouped.push(symbol);
    }
    grouped
}

// The profile.
pub const DEPARTURE_HELP: &str = "Leave a workspace of this server, or delete your account on \
     it. The messages, files, reactions, threads, direct conversations, recordings and \
     transcripts in which you took part stay, and so does every entry of the audit log.";
pub const WORKSPACES: &str = "Workspaces";
pub const NO_WORKSPACE: &str = "Your account is a member of no workspace on this server.";
pub const LEAVE: &str = "Leave";
pub const LEAVE_WORKSPACE: &str = "Leave workspace";
pub const LEAVE_WORKSPACE_HELP: &str = "Shows what stays and what ends before you leave.";
pub const LEAVING: &str = "Leaving…";
pub const DELETE_ACCOUNT_GROUP: &str = "Your account";
pub const DELETE_ACCOUNT: &str = "Delete account";
pub const DELETE_ACCOUNT_CONFIRM: &str = "Delete my account";
pub const DELETING: &str = "Deleting…";
pub const OWNER_STAYS: &str = "You are the server owner. Another account must first become the \
     owner, with the host command of the server; then you can leave the last workspace or \
     delete the account.";

/// What stays and what ends when the account leaves a workspace (ADR 0121).
pub fn leave_question(last: bool) -> &'static str {
    if last {
        "This is your last workspace on this server. What stays: your messages, files, \
         reactions, threads, direct conversations, recordings and transcripts, under your \
         name marked as inactive, and every entry of the audit log. What ends: your \
         membership and roles, your server roles, your invites, and the sessions of every \
         device of yours on this server. Your account becomes inactive; an invite code \
         returns you to the same account."
    } else {
        "What stays: your messages, files, reactions, threads, recordings and transcripts in \
         this workspace, under your name, and every entry of the audit log. What ends: your \
         membership and your roles in this workspace, and this workspace in the invites that \
         you created. To come back you need an invite."
    }
}

pub const DELETE_QUESTION: &str = "What stays: your messages, files, reactions, threads, direct \
     conversations, recordings and transcripts, under your name marked as a deleted account, \
     and every entry of the audit log. What ends: your account, its memberships and roles, \
     its devices and sessions, its invites and its picture. This cannot be undone. The server \
     is removed from the list on this device.";

pub const PROFILE_HELP: &str =
    "The name and the picture that the people on this server see for your account.";
pub const PICTURE: &str = "Picture";
pub const CHOOSE_PICTURE: &str = "Choose a picture…";
pub const REMOVE_PICTURE: &str = "Remove picture";
pub const PICTURE_BOUNDS: &str =
    "A PNG, JPEG or WebP image of at most 256 KiB and at most 1024 by 1024 pixels.";
pub const NO_PICTURE: &str =
    "Your account has no picture. The pattern is made from the identifier of the account.";
pub const PICTURE_SENDING: &str = "Sending the picture…";
pub const PICTURE_NOT_READ: &str = "The file could not be read.";
pub const PAGE_BUSY: &str = "Another change of this page is on its way to the server, so the file was not sent. \
     Choose the picture again when that change has ended.";
pub const NAME: &str = "Name";
pub const NAME_FIELD: &str = "Name on this server";
pub const NAME_HELP: &str =
    "A name has 1 to 64 characters. Your accounts on other servers keep their names.";

/// A number of bytes in the unit of the bound of a picture: whole KiB, rounded up.
fn kib(bytes: u32) -> String {
    match bytes {
        0..1024 => format!("{bytes} bytes"),
        bytes => format!("{} KiB", bytes.div_ceil(1024)),
    }
}

pub fn picture_held(width: u32, height: u32, bytes: u32) -> String {
    format!(
        "The server holds a picture of {width} by {height} pixels, {}.",
        kib(bytes)
    )
}

/// Why a name is not sent.
pub const fn invalid_name(invalid: &InvalidName) -> &'static str {
    match invalid {
        InvalidName::Empty => "Type a name.",
        InvalidName::TooLong => "A name has at most 64 characters.",
        InvalidName::ControlCharacter => {
            "A name holds no line break and no other control character."
        }
    }
}

/// Why a file is not sent as a picture.
pub const fn invalid_picture(invalid: InvalidAvatar) -> &'static str {
    match invalid {
        InvalidAvatar::TooLarge => "The file is larger than 256 KiB. Choose a smaller picture.",
        InvalidAvatar::Unsupported => "The file is not a PNG, JPEG or WebP image.",
    }
}

// The devices.
pub const DEVICES_HELP: &str = "Each device holds its own key for your account on this server. A device that you remove \
     cannot log in again.";
pub const UNNAMED_DEVICE: &str = "Unnamed device";
pub const THIS_DEVICE: &str = "This device";
pub const RENAME: &str = "Rename";
pub const REMOVE: &str = "Remove";
pub const DEVICE_NAME: &str = "Name of the device";
pub const DEVICE_NAME_HELP: &str = "A device name has at most 64 characters. It can be empty.";
pub const LAST_DEVICE: &str =
    "This is the only device of the account. The last device cannot be removed.";
pub const NEVER_LOGGED_IN: &str = "No login yet";
pub const DEVICES_NOT_READ: &str = "The list of devices could not be read.";

pub const fn origin(origin: DeviceOrigin) -> &'static str {
    match origin {
        DeviceOrigin::OwnerCode => "Added with the owner code",
        DeviceOrigin::Invite => "Added with an invite",
        DeviceOrigin::JoinRequest => "Added by an approved join request",
        DeviceOrigin::Link => "Added by a device link",
        DeviceOrigin::Recovery => "Added by a recovery",
    }
}

pub fn added_on(at: OffsetDateTime) -> String {
    format!("Added on {}", utc(at))
}

pub fn last_login(at: Option<OffsetDateTime>) -> String {
    match at {
        Some(at) => format!("Last login on {}", utc(at)),
        None => NEVER_LOGGED_IN.to_owned(),
    }
}

// The recoveries.
pub const RECOVERIES_HELP: &str = "A recovery gives a new device access to your account on this \
     server, and with it to your direct messages. Only the server owner, or the host command of \
     the server, can do it. Every recovery of the account is listed here, also those that this \
     device has seen.";
pub const NO_RECOVERIES: &str = "Your account was never recovered on this server.";
pub const RECOVERIES_NOT_READ: &str = "The list of recoveries could not be read.";
pub const BY_OWNER: &str = "By the server owner";
pub const BY_HOST: &str = "With the host command of the server";
pub const ADDED_THIS_DEVICE: &str = "It added this device.";
pub const NOT_SEEN_HERE: &str = "Not seen on this device yet";

pub fn recovery_line(at: OffsetDateTime, by_host: bool) -> String {
    format!("{} · {}", utc(at), if by_host { BY_HOST } else { BY_OWNER })
}

pub fn more_recoveries(hidden: usize) -> String {
    match hidden {
        1 => "1 older recovery is not shown.".to_owned(),
        hidden => format!("{hidden} older recoveries are not shown."),
    }
}

pub fn more_devices(hidden: usize) -> String {
    match hidden {
        1 => "1 more device is not shown.".to_owned(),
        hidden => format!("{hidden} more devices are not shown."),
    }
}

/// The question before the removal of a device. It holds no name: a name is text of a
/// server, and the row shows it on a line of its own above the question.
pub const fn remove_device_question(this_device: bool) -> &'static str {
    if this_device {
        "Remove this device from your account? Its session ends at once, and this installation \
         loses its access to the account on this server. It can get access again only through \
         a device link or a recovery."
    } else {
        "Remove the device of this row from your account? Its sessions end at once, and it \
         cannot log in again."
    }
}

// The link of a new device on one server, on the device that is logged in.
pub const LINK_NEW_DEVICE: &str = "Link a new device";
pub const LINK_NEW_DEVICE_HELP: &str = "On the new device, add this server and choose “I already have an account: link this \
     device”. It shows a link code. Type the code here.";
pub const LINK_CODE_FIELD: &str = "Link code of the new device";
pub const LOOK_UP: &str = "Look up";
pub const LOOKING_UP: &str = "Looking up…";
pub const LINK_CODE_EMPTY: &str = "Type the link code that the new device shows.";
pub const LINK_CODE_TOO_LONG: &str = "A link code has 8 symbols.";
pub const COMPARE_KEY: &str = "Compare this key with the key that the new device shows. Approve only when every \
     group is the same.";
pub const KEY_OF_NEW_DEVICE: &str = "Key of the new device";
pub const APPROVE: &str = "Approve";
pub const APPROVING: &str = "Approving…";
pub const DEVICE_LINKED: &str = "Device linked";
pub const DONE: &str = "Done";
pub const OUTCOME_UNKNOWN: &str = "The approval was sent, and no answer of the server arrived. The device may be linked. \
     “Check again” reads the list of devices.";
pub const CHECKING: &str = "Checking…";

pub fn new_device_named(name: &str) -> String {
    if name.is_empty() {
        "The new device gave no name.".to_owned()
    } else {
        format!("The new device names itself “{name}”.")
    }
}

pub fn device_linked(name: &str) -> String {
    if name.is_empty() {
        "The new device belongs to your account now. It logs in by itself.".to_owned()
    } else {
        format!("“{name}” belongs to your account now. It logs in by itself.")
    }
}

pub fn code_expires(at: OffsetDateTime) -> String {
    format!("The code expires on {}.", utc(at))
}

// The link of this device on one server, on the new device.
pub const LINK_THIS_DEVICE: &str = "I already have an account: link this device";
pub const LINK_THIS_DEVICE_HELP: &str = "This device gets a link code. You type the code on a device that is logged in to your \
     account, and approve it there.";
pub const REQUEST_LINK: &str = "Link this device";
pub const REQUESTING_LINK: &str = "Asking for a code…";
pub const LINK_CODE: &str = "Link code";
pub const KEY_OF_THIS_DEVICE: &str = "Key of this device";
pub const LINK_WAIT_HELP: &str = "On a device that is logged in to your account, open “Your account”, then “Devices”, \
     and type this code under “Link a new device”. That device shows a key: approve only \
     when it is the key shown here. This device logs in by itself after the approval.";
pub const LINK_WAIT_RECOVERY: &str = "When no device of your account is at hand, give this key to \
     the server owner. The owner can recover your account onto this device, and this device \
     logs in by itself after that.";
pub const LINK_CODE_NOT_SHOWN: &str =
    "The server gave a code in a form that Junto does not show. Ask for a new code.";
pub const NEW_CODE: &str = "New code";
pub const CANCEL_LINK: &str = "Cancel";

// The link on all servers.
pub const DEVICES_SECTION: &str = "Devices";
pub const DEVICES_SECTION_HELP: &str = "A second installation of Junto gets access to your accounts on all your servers with \
     two texts that you carry between the two devices.";
pub const LINK_ALL_FIRST: &str = "Link a new device to all my servers";
pub const LINK_ALL_NEW: &str = "Link this device to my servers";
pub const COPY: &str = "Copy";
pub const COPIED: &str = "Copied";
pub const START_OVER: &str = "Start over";
pub const TEXT_TOO_LONG: &str =
    "This text is longer than 32 KiB. A link text is shorter. The field was emptied.";
pub const OFFER_TITLE: &str = "1. Give the offer to the new device";
pub const OFFER_HELP: &str = "The offer names the servers on which this device has an account. It holds no secret. On \
     the new device, open the settings, choose “Link this device to my servers”, and put \
     the offer into its field.";
pub const OFFER_LABEL: &str = "Link offer";
pub const NO_OFFER: &str = "This device has no account on any server, so it has nothing to offer.";
pub const REQUEST_TITLE: &str = "2. Take the request of the new device";
pub const REQUEST_HELP: &str =
    "The new device answers with a request text. Put it into this field.";
pub const REQUEST_FIELD: &str = "Link request of the new device";
pub const REQUEST_EMPTY: &str = "Put the request text of the new device into the field.";
pub const CONFIRM_TITLE: &str = "3. Compare and approve";
pub const CONFIRM_HELP: &str = "The new device shows a fingerprint. Approve only when it is the same as this one. \
     Nothing is approved before you press “Approve”.";
pub const FINGERPRINT: &str = "Fingerprint";
pub const USE_THIS_ACCOUNT: &str = "Use this account";
pub const ALL_APPROVED: &str =
    "The new device is approved on every server of the request. It logs in by itself.";
pub const OFFER_FIELD_TITLE: &str = "1. Take the offer of the first device";
pub const OFFER_FIELD_HELP: &str = "On a device that is logged in, open the settings and choose “Link a new device to all \
     my servers”. It shows an offer text. Put it into this field.";
pub const OFFER_FIELD: &str = "Link offer of the first device";
pub const OFFER_EMPTY: &str = "Put the offer text of the first device into the field.";
pub const THIS_DEVICE_NAME: &str = "Name of this device";
pub const REQUEST_LINKS: &str = "Request links";
pub const REQUESTING_LINKS: &str = "Asking the servers…";
pub const REQUEST_TEXT_TITLE: &str = "2. Give the request to the first device";
pub const REQUEST_TEXT_HELP: &str = "Put this text into the field on the first device. The first device shows the same \
     fingerprint. The text holds link codes that expire after 10 minutes.";
pub const REQUEST_LABEL: &str = "Link request";
pub const NO_REQUEST: &str = "No server waits for an approval, so there is no request text.";
pub const SERVERS: &str = "Servers";
pub const CHECK: &str = "Check";
pub const ALL_LINKED: &str = "This device is linked on every server of the offer.";
pub const PROGRESS_WORKING: &str = "working…";
pub const PROGRESS_READY: &str = "ready";
pub const PROGRESS_APPROVED: &str = "approved";
pub const PROGRESS_UNKNOWN: &str = "unknown: the answer of the server was lost";
pub const PROGRESS_WAITING: &str = "waiting for the approval";
pub const PROGRESS_LINKED: &str = "linked";
pub const UNSAVED_SERVER: &str = "A server that this device has not saved";
pub const NO_ACCOUNT_SHOWN: &str = "No account read yet";

/// The servers that an offer names. The names are text of the servers: the caller cuts
/// the sentence at the width of its place.
pub fn offer_names(servers: &[String]) -> String {
    match servers {
        [one] => format!("The offer names 1 server: {one}."),
        many => format!(
            "The offer names {} servers: {}.",
            many.len(),
            many.join(", ")
        ),
    }
}

pub const fn role_title(role: LinkRole) -> &'static str {
    match role {
        LinkRole::FirstDevice => LINK_ALL_FIRST,
        LinkRole::NewDevice => LINK_ALL_NEW,
    }
}

pub fn progress_failed(reason: &str) -> String {
    format!("failed: {reason}")
}

pub fn account_line(name: &str) -> String {
    format!("Account: {name}")
}

/// What the line of a server tells when this device has another account there than the
/// one of the line. It holds no name: the line names the account with [`account_now`].
pub const ACCOUNT_CHANGED: &str = "This device now has another account on this server. Nothing is approved here until you \
     choose “Use this account”.";

/// The account that this device has now. The name is text of the server: the line is cut
/// at the width of its place.
pub fn account_now(name: &str) -> String {
    format!("Account now: {name}")
}

/// The length of a text that the layer draws whole or cut. "Copy" takes the whole text.
pub fn text_length(characters: usize) -> String {
    format!("The text has {characters} characters. “{COPY}” takes all of it.")
}

pub fn incomplete(failed: usize, total: usize) -> String {
    match (failed, total) {
        (1, 1) => "The server did not succeed. Its line states why.".to_owned(),
        (1, total) => format!("1 of {total} servers did not succeed. Its line states why."),
        (failed, total) => {
            format!("{failed} of {total} servers did not succeed. Their lines state why.")
        }
    }
}

fn limited(wait: Option<std::time::Duration>, what: &str) -> String {
    match wait {
        Some(wait) => format!("{what} Try again in {} seconds.", wait.as_secs().max(1)),
        None => format!("{what} Try again later."),
    }
}

/// Why the server did not do a request of the account layer.
pub fn refusal(what: AskKind, error: &ApiError) -> String {
    let device = matches!(what, AskKind::RenameDevice(_) | AskKind::RemoveDevice(_));
    let fixed = match (error.code(), error.category()) {
        (Some(ErrorCode::RateLimited), _) => {
            return limited(
                error.retry_after(),
                "The server takes 30 changes of a profile or of a device name in a minute.",
            );
        }
        (Some(ErrorCode::ImageTooLarge), _) => {
            "The server did not take the picture: it is larger than 256 KiB."
        }
        (Some(ErrorCode::ImageUnsupported), _) => {
            "The server did not take the picture. It takes a still PNG, JPEG or WebP image of \
             at most 1024 by 1024 pixels."
        }
        (Some(ErrorCode::InvalidRequest), _) => match what {
            AskKind::SaveName => {
                "The server did not accept the name. A name has 1 to 64 characters and no line \
                 break."
            }
            AskKind::RenameDevice(_) => {
                "The server did not accept the name. A device name has at most 64 characters \
                 and no line break."
            }
            AskKind::SetPicture => "The server did not accept the file as a picture.",
            _ => "The server did not accept the request.",
        },
        // The code `last_device`, and `conflict` of a server from before that code.
        (_, Some(ErrorCategory::Conflict)) if matches!(what, AskKind::RemoveDevice(_)) => {
            LAST_DEVICE
        }
        (_, Some(ErrorCategory::NotFound)) if device => "Your account holds this device no longer.",
        (Some(ErrorCode::OwnerProtected), _) => OWNER_STAYS,
        (_, Some(ErrorCategory::NotFound)) if matches!(what, AskKind::LeaveWorkspace(_)) => {
            "Your account is no longer a member of this workspace."
        }
        _ => return crate::text::flow_error(&FlowError::from(error)),
    };
    fixed.to_owned()
}

/// The sentence for a failed action of a link flow: the lookup and the approval on one
/// server, and the flows over all servers.
pub fn link_error(error: &FlowError) -> String {
    let fixed = match error {
        FlowError::Refused { code, .. } => match code {
            ErrorCode::LinkCodeInvalid => {
                "The server knows no link with this code. A code is used once and expires after \
                 10 minutes. Ask the new device for a new code."
            }
            ErrorCode::PublicKeyMismatch => {
                "The key behind the code is not the key that was shown. Nothing was approved. \
                 Look the code up again."
            }
            ErrorCode::KeyAlreadyRegistered => {
                "The key of this device already belongs to an account on the server."
            }
            _ => return crate::text::flow_error(error),
        },
        FlowError::Limited {
            code: ErrorCode::DeviceLinksFull,
            ..
        } => {
            "The server holds as many waiting device links as it allows. Try again in a few \
             minutes."
        }
        FlowError::Limited { retry_after, .. } => {
            return limited(
                *retry_after,
                "The server limits how many link codes are tried or asked for.",
            );
        }
        FlowError::InvalidBundle("link offer") => {
            "The text is not a link offer. Copy the whole offer text of the first device."
        }
        FlowError::InvalidBundle(_) => {
            "The text is not a link request. Copy the whole request text of the new device."
        }
        FlowError::Incomplete { failed, total } => return incomplete(*failed, *total),
        FlowError::UnknownServer => "This device has not saved this server.",
        FlowError::NotLoggedIn => "This device is not logged in to this server.",
        FlowError::Expired => "The link code expired. “Try again” asks for a new one.",
        FlowError::OtherServer => "The address of the offer answers as another server.",
        FlowError::AlreadySaved { .. } => {
            "This device has saved this server under another address."
        }
        other => return crate::text::flow_error(other),
    };
    fixed.to_owned()
}

/// The results of the forms and questions of the account layer, drawn in the space that a
/// row keeps after the answer (ADR 0730).
pub const DEVICE_NAME_SAVED: &str = "The new name of the device is saved.";
pub const DEVICE_REMOVED: &str = "The device is removed.";
pub const WORKSPACE_LEFT: &str = "You left the workspace.";
pub const ACCOUNT_DELETED: &str = "The account is deleted.";

#[cfg(test)]
mod tests {
    use junto_client_core::api::Rejection;
    use junto_client_core::protocol::ids::DeviceId;

    use super::*;

    fn rejected(code: ErrorCode, retry_after: Option<u64>) -> ApiError {
        ApiError::Rejected(Rejection {
            code,
            status: code.http_status(),
            retry_after: retry_after.map(std::time::Duration::from_secs),
            message: "a sentence of the server".to_owned(),
            left: None,
        })
    }

    #[test]
    fn the_fingerprint_of_a_key_is_the_printed_key_in_groups_of_four() {
        let key = PublicKey::from_bytes([7; 32]);
        let printed = key.to_string();
        let fingerprint = key_fingerprint(&key);
        assert_eq!(printed.len(), 43);
        // Ten groups of four characters and one of three.
        let groups: Vec<&str> = fingerprint.split(' ').collect();
        assert_eq!(groups.len(), 11);
        assert!(groups[..10].iter().all(|group| group.len() == 4));
        assert_eq!(groups[10].len(), 3);
        assert_eq!(fingerprint.replace(' ', ""), printed);
        assert_eq!(
            fingerprint,
            "BwcH BwcH BwcH BwcH BwcH BwcH BwcH BwcH BwcH BwcH Bwc"
        );
        // Another key has another fingerprint, and the same key always the same one.
        assert_ne!(
            key_fingerprint(&PublicKey::from_bytes([8; 32])),
            fingerprint
        );
        assert_eq!(
            key_fingerprint(&PublicKey::from_bytes([7; 32])),
            fingerprint
        );
    }

    #[test]
    fn the_size_of_a_picture_is_stated_in_the_unit_of_its_bound() {
        assert_eq!(
            picture_held(512, 512, 84 * 1024),
            "The server holds a picture of 512 by 512 pixels, 84 KiB."
        );
        assert_eq!(
            picture_held(64, 48, 84 * 1024 + 1),
            "The server holds a picture of 64 by 48 pixels, 85 KiB."
        );
        assert_eq!(
            picture_held(1, 1, 150),
            "The server holds a picture of 1 by 1 pixels, 150 bytes."
        );
        assert!(picture_held(1024, 1024, 256 * 1024).ends_with("256 KiB."));
    }

    #[test]
    fn a_moment_is_written_in_utc() {
        let at = OffsetDateTime::from_unix_timestamp(1_791_626_640).unwrap();
        assert_eq!(utc(at), "10 October 2026 at 10:04 UTC");
        // The same moment in another offset is the same text.
        let east = crate::moment::in_offset(at, time::UtcOffset::from_hms(5, 30, 0).unwrap());
        assert_eq!(utc(east), "10 October 2026 at 10:04 UTC");
        assert_eq!(added_on(at), "Added on 10 October 2026 at 10:04 UTC");
        assert_eq!(
            last_login(Some(at)),
            "Last login on 10 October 2026 at 10:04 UTC"
        );
        assert_eq!(last_login(None), NEVER_LOGGED_IN);
    }

    #[test]
    fn every_refusal_of_a_request_of_the_account_has_its_sentence() {
        let device = DeviceId::generate();
        let kinds = [
            AskKind::SaveName,
            AskKind::SetPicture,
            AskKind::RemovePicture,
            AskKind::RenameDevice(device),
            AskKind::RemoveDevice(device),
        ];
        for what in kinds {
            for code in ErrorCode::ALL {
                let sentence = refusal(what, &rejected(*code, Some(20)));
                assert!(sentence.ends_with('.'), "{what:?} {code:?}: {sentence}");
                assert!(!sentence.contains("sentence of the server"), "{sentence}");
                assert!(!sentence.contains('!'), "{sentence}");
            }
        }
        assert_eq!(
            refusal(
                AskKind::SaveName,
                &rejected(ErrorCode::RateLimited, Some(20))
            ),
            "The server takes 30 changes of a profile or of a device name in a minute. Try \
             again in 20 seconds."
        );
        assert_eq!(
            refusal(AskKind::SaveName, &rejected(ErrorCode::RateLimited, None)),
            "The server takes 30 changes of a profile or of a device name in a minute. Try \
             again later."
        );
        assert!(
            refusal(
                AskKind::SetPicture,
                &rejected(ErrorCode::ImageTooLarge, None)
            )
            .contains("256 KiB")
        );
        assert!(
            refusal(
                AskKind::SetPicture,
                &rejected(ErrorCode::ImageUnsupported, None)
            )
            .contains("1024 by 1024")
        );
        // The server refuses the removal of the last device with `last_device`, and a
        // server from before that code with `conflict`.
        for code in [ErrorCode::LastDevice, ErrorCode::Conflict] {
            assert_eq!(
                refusal(AskKind::RemoveDevice(device), &rejected(code, None)),
                LAST_DEVICE
            );
        }
        assert_eq!(ErrorCode::LastDevice.http_status(), 409);
        // The code of the last device has the same sentence at every other control.
        assert_eq!(
            refusal(AskKind::SaveName, &rejected(ErrorCode::LastDevice, None)),
            LAST_DEVICE
        );
        // The same code at another control keeps the general sentence.
        assert_ne!(
            refusal(AskKind::SaveName, &rejected(ErrorCode::Conflict, None)),
            LAST_DEVICE
        );
        assert_eq!(
            refusal(
                AskKind::RenameDevice(device),
                &rejected(ErrorCode::NotFound, None)
            ),
            "Your account holds this device no longer."
        );
        // A name and a file that are not sent.
        assert_eq!(
            refusal(AskKind::SaveName, &ApiError::Name(InvalidName::Empty)),
            "The name is empty."
        );
    }

    #[test]
    fn the_failures_of_a_link_have_their_sentences() {
        for code in ErrorCode::ALL {
            let sentence = link_error(&FlowError::refused(*code));
            assert!(sentence.ends_with('.'), "{code:?}: {sentence}");
        }
        assert!(link_error(&FlowError::refused(ErrorCode::LinkCodeInvalid)).contains("10 minutes"));
        assert!(
            link_error(&FlowError::refused(ErrorCode::PublicKeyMismatch))
                .contains("Nothing was approved")
        );
        assert_eq!(
            link_error(&FlowError::InvalidBundle("link offer")),
            "The text is not a link offer. Copy the whole offer text of the first device."
        );
        assert_eq!(
            link_error(&FlowError::InvalidBundle("link request")),
            "The text is not a link request. Copy the whole request text of the new device."
        );
        assert_eq!(
            link_error(&FlowError::Incomplete {
                failed: 1,
                total: 2
            }),
            "1 of 2 servers did not succeed. Its line states why."
        );
        assert_eq!(
            link_error(&FlowError::Incomplete {
                failed: 2,
                total: 3
            }),
            "2 of 3 servers did not succeed. Their lines state why."
        );
        assert_eq!(
            link_error(&FlowError::Limited {
                code: ErrorCode::RateLimited,
                retry_after: Some(std::time::Duration::from_secs(40))
            }),
            "The server limits how many link codes are tried or asked for. Try again in 40 \
             seconds."
        );
        // A failure without a sentence of its own keeps the sentence of the flows.
        assert_eq!(
            link_error(&FlowError::Unavailable),
            crate::text::flow_error(&FlowError::Unavailable)
        );
    }

    #[test]
    fn the_question_of_a_removal_names_what_follows() {
        let this = remove_device_question(true);
        assert!(this.contains("Its session ends at once"));
        assert!(this.contains("loses its access"));
        let other = remove_device_question(false);
        assert!(
            other.contains("Its sessions end at once") && other.contains("cannot log in again")
        );
        assert_ne!(this, other);
    }

    #[test]
    fn a_question_holds_no_text_of_a_server() {
        // A sentence that asks or instructs is fixed text. A name of a server has a line
        // of its own, where its text cannot be read as a part of the sentence.
        for sentence in [
            remove_device_question(true),
            remove_device_question(false),
            ACCOUNT_CHANGED,
        ] {
            assert!(
                sentence.ends_with('.') && !sentence.contains('!'),
                "{sentence}"
            );
        }
        assert!(ACCOUNT_CHANGED.contains(&format!("“{USE_THIS_ACCOUNT}”")));
        assert_eq!(account_now("Ada H."), "Account now: Ada H.");
        assert_eq!(
            text_length(120),
            "The text has 120 characters. “Copy” takes all of it."
        );
    }
}
