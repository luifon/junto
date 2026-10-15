//! The text of the order of the server book and of the update notice (ADR 0780).

use junto_client_core::updates::Version;

use crate::state::book::{CheckShown, Step};

// The order of the server book, in the menu of a server.
pub const MOVE_UP: &str = "Move up";
pub const MOVE_DOWN: &str = "Move down";
pub const MOVE_UP_HELP: &str = "One place up in the server book.";
pub const MOVE_DOWN_HELP: &str = "One place down in the server book.";
pub const FIRST_SERVER: &str = "It is the first server of the server book.";
pub const LAST_SERVER: &str = "It is the last server of the server book.";
pub const MOVE_SERVER_GONE: &str = "The server is no longer in the server book. It was not moved.";

pub const fn move_title(step: Step) -> &'static str {
    match step {
        Step::Up => MOVE_UP,
        Step::Down => MOVE_DOWN,
    }
}

/// The second line of a move item: what the item does, or why it does nothing.
pub const fn move_help(step: Step, at_end: bool) -> &'static str {
    match (step, at_end) {
        (Step::Up, false) => MOVE_UP_HELP,
        (Step::Up, true) => FIRST_SERVER,
        (Step::Down, false) => MOVE_DOWN_HELP,
        (Step::Down, true) => LAST_SERVER,
    }
}

// The update notice: one line of the channel column.
/// The sentence of the line. It names no version.
pub const NEW_VERSION: &str = "New version available";
/// The action of the line: it opens the section "Version" of the settings. This version
/// of Junto installs nothing.
pub const DETAILS: &str = "Details";

/// The name and the tooltip of the update notice, and the sentence of the settings. A
/// version is three numbers, so it holds no text of the source of releases.
pub fn update_available(version: Version) -> String {
    format!("Junto {version} is available.")
}

// The section "Version" of the settings.
pub const VERSION: &str = "Version";
pub const NOT_CONFIGURED: &str = "This version of Junto does not look for newer versions.";
pub const NOT_CHECKED: &str = "Junto has not looked for a newer version yet.";
pub const CURRENT: &str = "No newer version is available.";
pub const CHECK_FAILED: &str =
    "Junto could not find out whether a newer version is available. It tries again by itself.";
pub const RELEASE_PAGE: &str = "Address of the release page";
pub const RELEASE_PAGE_HELP: &str =
    "The address comes from the source of releases. Junto does not open it.";
pub const COPY_ADDRESS: &str = "Copy address";
pub const ADDRESS_COPIED: &str = "Copied";
pub const CHECK_NOW: &str = "Check now";
pub const CHECKING: &str = "Checking…";
pub const NO_INSTALL: &str = "This version of Junto does not install updates by itself.";

pub fn this_build(build: Version) -> String {
    format!(
        "This is Junto {}.",
        junto_client_core::release_configuration::label(
            &build.to_string(),
            junto_client_core::releases::TEST_CONFIGURATION
        )
    )
}

/// The state of the update check as the settings state it.
pub fn check_shown(check: &CheckShown) -> String {
    match check {
        CheckShown::NotConfigured => NOT_CONFIGURED.to_owned(),
        CheckShown::NotChecked => NOT_CHECKED.to_owned(),
        CheckShown::Current => CURRENT.to_owned(),
        CheckShown::Available { version, .. } => update_available(*version),
        CheckShown::Failed => CHECK_FAILED.to_owned(),
        CheckShown::Expired => EXPIRED.to_owned(),
        CheckShown::UntrustedKeys => UNTRUSTED_KEYS.to_owned(),
    }
}

/// The sentence of the state "needs a newer Junto" of a server, with the newer version
/// when one is known.
pub fn needs_update(detail: &str, available: Option<Version>) -> String {
    match available {
        Some(version) => format!("{detail} {}", update_available(version)),
        None => detail.to_owned(),
    }
}

pub const EXPIRED: &str = "The update information has no valid date. Check the date and time of this computer, then check again later.";
pub const UNTRUSTED_KEYS: &str = "The update information is not signed by a key this version trusts. This version does not update itself. Take a newer version only from a source you have verified with whoever runs your server.";
pub const MOVE_TO_UPDATE: &str =
    "Move Junto to a writable directory of your account before updating.";
pub const UPDATE: &str = "Update";
pub const INSTALL_UPDATE: &str = "Install update";
pub const INSTALLING: &str = "Downloading and verifying the update…";
pub const NOT_A_PACKAGE: &str =
    "This copy of Junto was not installed from a package. It does not install updates.";
/// The result of an installed update, with what happened to the version that it replaced.
pub fn installed(previous: &junto_client_core::releases::install::Previous) -> String {
    use junto_client_core::releases::install::Previous;
    match previous {
        Previous::Kept(version) => format!(
            "Update installed. Quit Junto and reopen it to use the new version. Version {version} is kept for a rollback."
        ),
        Previous::Missing(reason) => format!(
            "Update installed. Quit Junto and reopen it to use the new version. No previous version is kept for a rollback: {reason}."
        ),
    }
}
pub const INSTALL_STOPPED: &str =
    "The update task stopped. Check the installed version before trying again.";
pub fn install_failed(error: &str) -> String {
    format!("The update failed: {error}.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_notice_names_the_version_and_every_state_of_the_check_has_a_sentence() {
        let version = Version::new(0, 4, 0);
        assert_eq!(update_available(version), "Junto 0.4.0 is available.");
        // The line of the channel column: the sentence of the design, and the action of
        // a version that installs nothing.
        assert_eq!(NEW_VERSION, "New version available");
        assert_eq!(DETAILS, "Details");
        assert!(!NEW_VERSION.contains(|sign: char| sign.is_ascii_digit()));
        assert_eq!(this_build(Version::new(0, 3, 1)), "This is Junto 0.3.1.");
        for check in [
            CheckShown::NotConfigured,
            CheckShown::NotChecked,
            CheckShown::Current,
            CheckShown::Failed,
            CheckShown::Available {
                version,
                page: Some("https://junto.example.test/releases/0.4.0".to_owned()),
            },
        ] {
            let sentence = check_shown(&check);
            assert!(sentence.ends_with('.'), "{check:?}: {sentence}");
            // The address of the page is never a part of a sentence.
            assert!(!sentence.contains("https"), "{check:?}: {sentence}");
        }
        assert_eq!(
            check_shown(&CheckShown::Available {
                version,
                page: None
            }),
            "Junto 0.4.0 is available."
        );
    }

    #[test]
    fn a_server_that_needs_a_newer_client_names_the_version_when_one_is_known() {
        let detail = "The oldest API version of the server is 3.";
        assert_eq!(needs_update(detail, None), detail);
        assert_eq!(
            needs_update(detail, Some(Version::new(1, 0, 0))),
            "The oldest API version of the server is 3. Junto 1.0.0 is available."
        );
    }

    #[test]
    fn a_move_item_says_what_it_does_or_why_it_does_nothing() {
        assert_eq!(move_title(Step::Up), "Move up");
        assert_eq!(move_title(Step::Down), "Move down");
        assert_eq!(move_help(Step::Up, true), FIRST_SERVER);
        assert_eq!(move_help(Step::Down, true), LAST_SERVER);
        assert_eq!(move_help(Step::Up, false), MOVE_UP_HELP);
        assert_eq!(move_help(Step::Down, false), MOVE_DOWN_HELP);
    }
}
