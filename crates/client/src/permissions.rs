//! The permissions of the operating system that the client uses, what each system keeps of
//! them, and the page of the settings of the system that changes each (ADR 0708).

use junto_client_core::notification_access::NotificationAccess;
use junto_media::camera::CameraAccess;
use junto_media::permission::MicrophoneAccess;
use junto_media::share::ScreenAccess;

/// A permission that the client needs from the operating system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    Microphone,
    Camera,
    /// The recording of the screen, of windows and of the sound of other applications.
    Screen,
    Notifications,
    /// The download directory of the account, into which files are saved.
    Downloads,
    /// The connection to a server on the local network.
    LocalNetwork,
    /// The keys of push to talk and of the mute key while another application is in front.
    InputMonitoring,
}

/// An operating system, so that the rules of each can be tested on every system.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    MacOs,
    Windows,
    Linux,
}

impl System {
    pub const fn this() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }

    /// The permissions that this system keeps for an application, in the order of the
    /// settings of the client.
    pub const fn permissions(self) -> &'static [Permission] {
        match self {
            Self::MacOs => &[
                Permission::Microphone,
                Permission::Camera,
                Permission::Screen,
                Permission::Notifications,
                Permission::Downloads,
                Permission::LocalNetwork,
                Permission::InputMonitoring,
            ],
            Self::Windows => &[
                Permission::Microphone,
                Permission::Camera,
                Permission::Notifications,
            ],
            Self::Linux => &[Permission::Camera, Permission::Screen],
        }
    }

    /// The page of the settings of the system that holds a permission.
    pub const fn settings_page(self, permission: Permission) -> Option<&'static str> {
        match (self, permission) {
            (Self::MacOs, Permission::Microphone) => {
                Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
            }
            (Self::MacOs, Permission::Camera) => {
                Some("x-apple.systempreferences:com.apple.preference.security?Privacy_Camera")
            }
            (Self::MacOs, Permission::Screen) => Some(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
            ),
            (Self::MacOs, Permission::Notifications) => Some(
                "x-apple.systempreferences:com.apple.Notifications-Settings.extension?id=org.junto.app",
            ),
            (Self::MacOs, Permission::Downloads) => Some(
                "x-apple.systempreferences:com.apple.preference.security?Privacy_FilesAndFolders",
            ),
            (Self::MacOs, Permission::LocalNetwork) => {
                Some("x-apple.systempreferences:com.apple.preference.security?Privacy_LocalNetwork")
            }
            (Self::MacOs, Permission::InputMonitoring) => {
                Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent")
            }
            (Self::Windows, Permission::Microphone) => Some("ms-settings:privacy-microphone"),
            (Self::Windows, Permission::Camera) => Some("ms-settings:privacy-webcam"),
            (Self::Windows, Permission::Notifications) => Some("ms-settings:notifications"),
            _ => None,
        }
    }
}

/// What the client knows of a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    Allowed,
    /// The system has not asked the person yet, and asks when the client makes it.
    NotAsked,
    /// The person refused or turned it off. Only the settings of the system change it.
    Refused,
    /// A rule of the machine forbids it, or no device can be opened by the account.
    Restricted,
    /// The system asks the person at every use and keeps no answer.
    AskedAtEachUse,
    /// The system keeps an answer that it does not let a program read. It asks at the first
    /// use; a use that it refused tells it.
    Unread,
}

impl From<MicrophoneAccess> for Access {
    fn from(access: MicrophoneAccess) -> Self {
        match access {
            MicrophoneAccess::Allowed => Self::Allowed,
            MicrophoneAccess::NotAsked => Self::NotAsked,
            MicrophoneAccess::Refused => Self::Refused,
        }
    }
}

impl From<CameraAccess> for Access {
    fn from(access: CameraAccess) -> Self {
        match access {
            CameraAccess::Allowed => Self::Allowed,
            CameraAccess::NotAsked => Self::NotAsked,
            CameraAccess::Refused => Self::Refused,
            CameraAccess::Restricted => Self::Restricted,
        }
    }
}

impl From<NotificationAccess> for Access {
    fn from(access: NotificationAccess) -> Self {
        match access {
            NotificationAccess::Allowed => Self::Allowed,
            NotificationAccess::NotAsked => Self::NotAsked,
            NotificationAccess::Refused => Self::Refused,
        }
    }
}

/// The screen permission of a system. macOS does not tell a permission that it never asked
/// for from one that the person refused: both are "not allowed", and the request makes
/// macOS ask when it has not asked before.
pub fn screen_access(access: ScreenAccess) -> Access {
    match access {
        ScreenAccess::Allowed => Access::Allowed,
        ScreenAccess::NotAllowed => Access::Refused,
        ScreenAccess::AskOnStart => Access::AskedAtEachUse,
    }
}

/// What the person can do about a permission in the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Make the system ask the person.
    Ask,
    /// Open the page of the settings of the system.
    OpenSettings,
}

/// The action of a permission in a state on a system. The screen permission of macOS has
/// both: its request makes macOS list the client and ask once, and its page is where the
/// person turns it on.
pub fn action(system: System, permission: Permission, access: Access) -> Option<Action> {
    let page = system.settings_page(permission).is_some();
    match access {
        Access::Allowed | Access::AskedAtEachUse => None,
        Access::NotAsked => Some(Action::Ask),
        Access::Refused | Access::Restricted | Access::Unread => {
            page.then_some(Action::OpenSettings)
        }
    }
}

/// The Input Monitoring permission of macOS, behind an interface so that a test gives a
/// fixed one.
pub trait InputMonitoring: 'static {
    /// Whether the system allows it now. It shows no question.
    fn allowed(&self) -> bool;
    /// Makes the system list the client and ask once. Returns whether it is allowed now.
    fn request(&self) -> bool;
}

/// The permission as macOS holds it.
pub struct SystemInputMonitoring;

impl InputMonitoring for SystemInputMonitoring {
    fn allowed(&self) -> bool {
        junto_client_core::input_monitoring::allowed()
    }

    fn request(&self) -> bool {
        junto_client_core::input_monitoring::request()
    }
}

/// A permission with a fixed state, which a test changes as a person does in the settings
/// of the system. It counts the requests.
#[derive(Debug, Clone, Default)]
pub struct FixedInputMonitoring {
    held: std::sync::Arc<std::sync::Mutex<(bool, u32)>>,
}

impl FixedInputMonitoring {
    pub fn set(&self, allowed: bool) {
        self.lock().0 = allowed;
    }

    pub fn asked(&self) -> u32 {
        self.lock().1
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (bool, u32)> {
        self.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl InputMonitoring for FixedInputMonitoring {
    fn allowed(&self) -> bool {
        self.lock().0
    }

    fn request(&self) -> bool {
        let mut held = self.lock();
        held.1 += 1;
        held.0
    }
}

/// One permission as the settings of the client show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionRow {
    pub permission: Permission,
    pub access: Access,
    pub action: Option<Action>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_system_lists_what_it_keeps_and_each_refusal_has_a_page_where_one_exists() {
        assert_eq!(System::MacOs.permissions().len(), 7);
        assert_eq!(
            System::Windows.permissions(),
            [
                Permission::Microphone,
                Permission::Camera,
                Permission::Notifications
            ]
        );
        assert_eq!(
            System::Linux.permissions(),
            [Permission::Camera, Permission::Screen]
        );
        for system in [System::MacOs, System::Windows] {
            for permission in system.permissions() {
                let page = system.settings_page(*permission);
                assert!(page.is_some(), "{system:?} {permission:?}");
                assert_eq!(
                    action(system, *permission, Access::Refused),
                    Some(Action::OpenSettings)
                );
            }
        }
        // Linux has no settings page for a permission of a program outside a sandbox.
        for permission in System::Linux.permissions() {
            assert_eq!(System::Linux.settings_page(*permission), None);
            assert_eq!(action(System::Linux, *permission, Access::Restricted), None);
        }
        assert_eq!(
            System::MacOs.settings_page(Permission::Screen),
            Some("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture")
        );
        assert_eq!(
            System::Windows.settings_page(Permission::Microphone),
            Some("ms-settings:privacy-microphone")
        );
    }

    #[test]
    fn a_permission_that_was_not_asked_for_is_asked_for_and_an_allowed_one_has_no_action() {
        assert_eq!(
            action(System::MacOs, Permission::Notifications, Access::NotAsked),
            Some(Action::Ask)
        );
        assert_eq!(
            action(System::MacOs, Permission::Camera, Access::Allowed),
            None
        );
        assert_eq!(
            action(System::Linux, Permission::Screen, Access::AskedAtEachUse),
            None
        );
        assert_eq!(
            action(System::MacOs, Permission::Downloads, Access::Unread),
            Some(Action::OpenSettings)
        );
        assert_eq!(screen_access(ScreenAccess::NotAllowed), Access::Refused);
        assert_eq!(Access::from(CameraAccess::Restricted), Access::Restricted);
    }
}
