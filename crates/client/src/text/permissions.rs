//! The text of the permissions of the operating system (ADR 0708).

use crate::permissions::{Access, Action, Permission, System};

pub const PERMISSIONS: &str = "Permissions";
pub const PERMISSIONS_HELP: &str = "What the system allows Junto to use. Junto asks the system for each when it first needs it. A permission that the system does not allow is changed in the settings of the system.";
pub const CHECK_AGAIN: &str = "Check again";
pub const ALLOW: &str = "Allow";

/// The name of a permission on a system.
pub fn name(system: System, permission: Permission) -> &'static str {
    match permission {
        Permission::Microphone => "Microphone",
        Permission::Camera => "Camera",
        Permission::Screen if system == System::MacOs => "Screen & System Audio Recording",
        Permission::Screen => "Screen sharing",
        Permission::Notifications => "Notifications",
        Permission::Downloads => "Downloads folder",
        Permission::LocalNetwork => "Local network",
        Permission::InputMonitoring => "Input Monitoring",
    }
}

/// What Junto uses a permission for.
pub fn purpose(permission: Permission) -> &'static str {
    match permission {
        Permission::Microphone => "Your voice in calls.",
        Permission::Camera => "Your video in calls.",
        Permission::Screen => "Sharing a screen or a window, and their sound, in calls.",
        Permission::Notifications => {
            "Messages, mentions, calls and requests while Junto is not in front."
        }
        Permission::Downloads => "Saving the files of messages and recordings.",
        Permission::LocalNetwork => "Servers on your local network.",
        Permission::InputMonitoring => {
            "The key of push to talk and the mute key while another application is in front."
        }
    }
}

/// The label of the button of an action.
pub fn action_label(system: System, action: Action) -> &'static str {
    match (action, system) {
        (Action::Ask, _) => ALLOW,
        (Action::OpenSettings, System::Windows) => "Open Settings",
        (Action::OpenSettings, _) => crate::text::OPEN_SYSTEM_SETTINGS,
    }
}

/// The place in the settings of the system where a permission is changed.
fn where_to_change(system: System, permission: Permission) -> &'static str {
    match (system, permission) {
        (System::MacOs, Permission::Microphone) => {
            "Turn on Junto in System Settings, Privacy & Security, Microphone."
        }
        (System::MacOs, Permission::Camera) => {
            "Turn on Junto in System Settings, Privacy & Security, Camera."
        }
        (System::MacOs, Permission::Screen) => {
            "Turn on Junto in System Settings, Privacy & Security, Screen & System Audio Recording. macOS can then ask you to quit and open Junto again."
        }
        (System::MacOs, Permission::Notifications) => {
            "Turn on Allow notifications for Junto in System Settings, Notifications."
        }
        (System::MacOs, Permission::Downloads) => {
            "Turn on Downloads Folder for Junto in System Settings, Privacy & Security, Files & Folders."
        }
        (System::MacOs, Permission::LocalNetwork) => {
            "Turn on Junto in System Settings, Privacy & Security, Local Network."
        }
        (System::MacOs, Permission::InputMonitoring) => {
            "Turn on Junto in System Settings, Privacy & Security, Input Monitoring. macOS can then ask you to quit and open Junto again."
        }
        (System::Windows, Permission::Microphone) => {
            "Turn on Microphone access and Let desktop apps access your microphone in Settings, Privacy & security, Microphone."
        }
        (System::Windows, Permission::Camera) => {
            "Turn on Camera access and Let desktop apps access your camera in Settings, Privacy & security, Camera."
        }
        (System::Windows, Permission::Notifications) => {
            "Turn on Notifications, and the notifications of Junto, in Settings, System, Notifications."
        }
        (System::Linux, Permission::Camera) => {
            "Your account cannot open a camera device. The administrator of this computer gives the account access to the video devices, often through the group video."
        }
        _ => "Allow Junto in the settings of the system.",
    }
}

/// What macOS shows for a permission of an earlier copy of Junto that it does not apply to
/// this copy: Junto is in the list and turned on, but not allowed.
const LISTED_BUT_NOT_ALLOWED: &str = "If Junto is already turned on there, select it, remove it with the minus button, and allow it again.";

/// The state of a permission, and what the person does about it.
pub fn state(system: System, permission: Permission, access: Access) -> String {
    match access {
        Access::Allowed => "Allowed.".to_owned(),
        Access::NotAsked => "Not asked yet. Junto asks when it first needs it.".to_owned(),
        Access::AskedAtEachUse => "The desktop asks at each share.".to_owned(),
        Access::Unread => match permission {
            Permission::Downloads => {
                "macOS asks when Junto first saves a file into Downloads.".to_owned()
            }
            Permission::LocalNetwork => {
                "macOS asks when Junto first connects to a server on your local network.".to_owned()
            }
            _ => "Not read yet.".to_owned(),
        },
        Access::Restricted if system == System::Linux => {
            where_to_change(system, permission).to_owned()
        }
        Access::Restricted => {
            "Not allowed by a rule of this computer. The administrator of the computer changes it."
                .to_owned()
        }
        Access::Refused => {
            let what = match permission {
                Permission::Downloads => "Not allowed: a file was not saved.",
                Permission::LocalNetwork => {
                    "Not allowed: macOS did not let Junto connect to a server on your local network."
                }
                _ if system == System::Windows => "Off.",
                _ => "Not allowed.",
            };
            let change = where_to_change(system, permission);
            match (system, permission) {
                (System::MacOs, Permission::Notifications | Permission::Downloads) => {
                    format!("{what} {change}")
                }
                (System::MacOs, _) => format!("{what} {change} {LISTED_BUT_NOT_ALLOWED}"),
                _ => format!("{what} {change}"),
            }
        }
    }
}

/// The sentence of a failed download that the system did not let Junto write.
pub fn download_refused(system: System) -> String {
    match system {
        System::MacOs => format!(
            "macOS did not let Junto save into Downloads. {}",
            where_to_change(system, Permission::Downloads)
        ),
        _ => "The system did not let Junto write into the download directory.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_permission_of_each_system_has_a_name_a_purpose_and_a_sentence_for_each_state() {
        for system in [System::MacOs, System::Windows, System::Linux] {
            for permission in system.permissions() {
                assert!(!name(system, *permission).is_empty());
                assert!(purpose(*permission).ends_with('.'));
                for access in [
                    Access::Allowed,
                    Access::NotAsked,
                    Access::Refused,
                    Access::Restricted,
                    Access::AskedAtEachUse,
                    Access::Unread,
                ] {
                    let sentence = state(system, *permission, access);
                    assert!(
                        sentence.ends_with('.'),
                        "{system:?} {permission:?} {access:?}"
                    );
                }
            }
        }
        // A refusal on macOS names the page and the step for a copy whose entry macOS
        // keeps from an earlier signature.
        let refused = state(System::MacOs, Permission::Screen, Access::Refused);
        assert!(
            refused.contains("Screen & System Audio Recording"),
            "{refused}"
        );
        assert!(refused.contains("minus button"), "{refused}");
        let windows = state(System::Windows, Permission::Microphone, Access::Refused);
        assert!(
            windows.contains("Let desktop apps access your microphone"),
            "{windows}"
        );
        assert_eq!(
            action_label(System::Windows, Action::OpenSettings),
            "Open Settings"
        );
        assert_eq!(
            action_label(System::MacOs, Action::OpenSettings),
            "Open System Settings"
        );
        assert_eq!(action_label(System::MacOs, Action::Ask), "Allow");
    }
}
