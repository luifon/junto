//! The text of the order of the server book, of the update notice and of the version of a
//! server (ADR 0780).

use junto_client_core::arrangement::GroupColour;
use junto_client_core::updates::Version;
use junto_client_core::version::ServerVersion;

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

// The arrangement of the server book (ADR 0775).
pub const FIRST_IN_GROUP: &str = "It is the first server of its group.";
pub const LAST_IN_GROUP: &str = "It is the last server of its group.";
pub const GROUP_GONE: &str = "The group is no longer in the server book. Nothing was changed.";
pub const WORKSPACE_GONE: &str = "The workspace is no longer listed. It was not moved.";

pub fn too_many_groups(most: usize) -> String {
    format!("The server book holds {most} groups, the most. Dissolve one to make another.")
}

/// The second line of a move item of a server: what the item does, or why it does nothing.
/// A server in a group moves among the servers of its group.
pub const fn server_move_help(step: Step, at_end: bool, in_group: bool) -> &'static str {
    match (step, at_end, in_group) {
        (Step::Up, true, true) => FIRST_IN_GROUP,
        (Step::Down, true, true) => LAST_IN_GROUP,
        (step, at_end, _) => move_help(step, at_end),
    }
}

pub const COLLAPSE_EVERYTHING: &str = "Collapse every server and group";
pub const EXPAND_EVERYTHING: &str = "Expand every server and group";
pub const COLLAPSE_WORKSPACES: &str = "Collapse workspaces";
pub const EXPAND_WORKSPACES: &str = "Expand workspaces";
pub const COLLAPSE_WORKSPACES_HELP: &str = "Show the server without its workspaces.";
pub const EXPAND_WORKSPACES_HELP: &str = "Show the workspaces under the server.";
pub const MOVE_TO_GROUP: &str = "Group…";
pub const MOVE_TO_GROUP_HELP: &str = "Put the server into a group, or into a new group.";
pub const GROUP_CHOICE: &str = "Group of the server";
pub const NEW_GROUP: &str = "New group";
pub const NEW_GROUP_HELP: &str = "A new group that holds this server.";
pub const REMOVE_FROM_GROUP: &str = "Remove from the group";
pub const REMOVE_FROM_GROUP_HELP: &str = "The server stands directly after the group.";
pub const IN_THIS_GROUP: &str = "The server is in this group.";
pub const MARK_SERVER_READ: &str = "Mark server as read";
pub const MARK_SERVER_READ_HELP: &str =
    "Every channel and direct conversation of the server, up to its latest message.";
pub const MARK_GROUP_READ: &str = "Mark group as read";
pub const MARK_GROUP_READ_HELP: &str = "Every server of the group, up to its latest message.";
pub const MARK_WORKSPACE_READ: &str = "Mark workspace as read";
pub const MARK_WORKSPACE_READ_HELP: &str =
    "Every channel of the workspace, up to its latest message.";
pub const NOTHING_UNREAD: &str = "Nothing is unread.";

pub const GROUP_MENU: &str = "Menu of the group";
pub const RENAME_GROUP: &str = "Rename…";
pub const RENAME_GROUP_HELP: &str = "Give the group a name, or leave it without one.";
pub const COLLAPSE_GROUP: &str = "Collapse group";
pub const EXPAND_GROUP: &str = "Expand group";
pub const DISSOLVE_GROUP: &str = "Dissolve group";
pub const DISSOLVE_GROUP_HELP: &str = "The servers stay in their place, outside a group.";
pub const COLOUR: &str = "Colour";
pub const CHOSEN: &str = "Chosen";
pub const GROUP_MOVE_UP_HELP: &str = "One place up in the server book.";
pub const GROUP_MOVE_DOWN_HELP: &str = "One place down in the server book.";
pub const FIRST_PART: &str = "It is the first entry of the server book.";
pub const LAST_PART: &str = "It is the last entry of the server book.";

/// The second line of a move item of a group.
pub const fn group_move_help(step: Step, at_end: bool) -> &'static str {
    match (step, at_end) {
        (Step::Up, false) => GROUP_MOVE_UP_HELP,
        (Step::Up, true) => FIRST_PART,
        (Step::Down, false) => GROUP_MOVE_DOWN_HELP,
        (Step::Down, true) => LAST_PART,
    }
}

pub const RENAME_GROUP_TITLE: &str = "Name of the group";
pub const GROUP_NAME_FIELD: &str = "Name";
pub const GROUP_NAME_HELP: &str =
    "At most 64 characters. Without a name the server book names the group by its servers.";

pub const WORKSPACE_MENU: &str = "Menu of the workspace";
pub const WORKSPACE_MOVE_UP_HELP: &str = "One place up among the workspaces of the server.";
pub const WORKSPACE_MOVE_DOWN_HELP: &str = "One place down among the workspaces of the server.";
pub const FIRST_WORKSPACE: &str = "It is the first workspace of the server.";
pub const LAST_WORKSPACE: &str = "It is the last workspace of the server.";

/// The second line of a move item of a workspace.
pub const fn workspace_move_help(step: Step, at_end: bool) -> &'static str {
    match (step, at_end) {
        (Step::Up, false) => WORKSPACE_MOVE_UP_HELP,
        (Step::Up, true) => FIRST_WORKSPACE,
        (Step::Down, false) => WORKSPACE_MOVE_DOWN_HELP,
        (Step::Down, true) => LAST_WORKSPACE,
    }
}

/// The name of a colour of a group.
pub const fn colour_name(colour: GroupColour) -> &'static str {
    match colour {
        GroupColour::Bone => "Bone",
        GroupColour::Terracotta => "Terracotta",
        GroupColour::Ochre => "Ochre",
        GroupColour::Olive => "Olive",
        GroupColour::Cypress => "Cypress",
        GroupColour::Bronze => "Bronze",
        GroupColour::Cinnabar => "Cinnabar",
    }
}

/// The name that a group shows: its own, or the names of its servers.
pub fn group_shown_name(name: &str, servers: &[&str]) -> String {
    if name.is_empty() {
        servers.join(", ")
    } else {
        name.to_owned()
    }
}

/// How many servers a group holds, as its head states it.
pub fn server_count(count: usize) -> String {
    match count {
        1 => "1 server".to_owned(),
        count => format!("{count} servers"),
    }
}

/// The name of the head of a group for the accessibility tree and its tooltip: the name, the
/// number of servers, whether it is collapsed, and what it holds unread and waiting.
pub fn group_label(shown: &str, count: usize, collapsed: bool, extra: &[String]) -> String {
    let mut label = format!(
        "Group {shown}, {}, {}",
        server_count(count),
        if collapsed { "collapsed" } else { "expanded" }
    );
    for part in extra {
        label.push_str(", ");
        label.push_str(part);
    }
    label
}

/// The item of the choice of a group that moves the server into it.
pub fn move_into(group: &str) -> String {
    format!("Move into {group}")
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

// The version of a server, under its name in the menu of the server.
/// The version is a semantic version of at most 64 bytes, so the server cannot put other
/// text into the line.
pub fn server_version(version: &ServerVersion) -> String {
    format!("Server version {version}")
}

pub fn server_release_available(version: Version) -> String {
    format!("Version {version} is available")
}

// The version of this copy of Junto, below "Add server" in the server book.
/// The version line: the name and the version of this build.
pub fn client_version(build: Version) -> String {
    format!(
        "Junto {}",
        junto_client_core::release_configuration::label(
            &build.to_string(),
            junto_client_core::releases::TEST_CONFIGURATION
        )
    )
}
/// The version number alone, in the collapsed server book.
pub fn client_version_number(build: Version) -> String {
    build.to_string()
}
pub const UP_TO_DATE: &str = "Up to date.";
pub const LOOKING: &str = "Looking for a newer version…";
pub const NO_SOURCE: &str = "This build does not look for newer versions.";
pub const NOT_LOOKED: &str = "Not checked for a newer version yet.";
pub const NOT_FOUND_OUT: &str = "Could not check for a newer version.";
pub const RESTART: &str = "Restart";
pub const RESTART_JUNTO: &str = "Restart Junto";
pub const COPY_COMMAND: &str = "Copy command";
pub const DOWNLOAD_PAGE: &str = "Address of the download page";

/// The name of the Debian package of a version, as the release site offers it (ADR 0727).
pub fn deb_name(version: Version) -> String {
    format!("junto_{version}_amd64.deb")
}

/// The command that installs the Debian package of a version from the folder of the
/// download.
pub fn deb_command(version: Version) -> String {
    format!("sudo apt install ./{}", deb_name(version))
}

/// The short sentence of an available version in the server book, by the way this copy
/// gets it.
pub fn available_way(version: Version, way: &crate::state::update_install::UpdateWay) -> String {
    use crate::state::update_install::UpdateWay;
    match way {
        UpdateWay::InApp | UpdateWay::NoSource => update_available(version),
        UpdateWay::MoveFirst => format!(
            "Junto {version} is available. Move Junto to a folder of your account that it can write, then update."
        ),
        UpdateWay::SystemPackage => format!(
            "Junto {version} is available. Install {} from the download page.",
            deb_name(version)
        ),
        UpdateWay::NotAPackage => format!(
            "Junto {version} is available. This copy was not installed from a package: install a package from the download page."
        ),
    }
}

/// The steps of a newer version for a copy that does not install it itself, in the
/// section "Version" of the settings.
pub fn update_steps(
    version: Version,
    way: &crate::state::update_install::UpdateWay,
) -> Option<String> {
    use crate::state::update_install::UpdateWay;
    match way {
        UpdateWay::SystemPackage => Some(format!(
            "Junto installed from a .deb package is updated by the package manager of the system. Download {} from the download page, run this command in the folder of the download, then restart Junto:",
            deb_name(version)
        )),
        UpdateWay::MoveFirst => Some(
            "Junto cannot write the folder that holds it, or macOS runs it from a temporary copy. Move Junto into a folder of your account (on macOS: Applications, by dragging it there in the Finder), open it from there, then choose Update.".to_owned(),
        ),
        UpdateWay::NotAPackage => Some(
            "This copy of Junto was not installed from a package, so it does not update itself. Install the package of your system from the download page; it keeps your servers and settings.".to_owned(),
        ),
        UpdateWay::InApp | UpdateWay::NoSource => None,
    }
}

/// The sentence after an installed update, below "Add server".
pub fn restart_to_use(version: Version) -> String {
    format!("Junto {version} is installed. Restart Junto to use it.")
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
pub const INSTALLS_ITSELF: &str = "When a newer version is available, Junto downloads and installs it on Update, and starts it on Restart.";

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
            "Update installed. Restart Junto to use the new version. Version {version} is kept for a rollback."
        ),
        Previous::Missing(reason) => format!(
            "Update installed. Restart Junto to use the new version. No previous version is kept for a rollback: {reason}."
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
