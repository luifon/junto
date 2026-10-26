//! The notifications of the operating system, behind an interface (ADR 0742).
//!
//! The state of the application decides what is shown (`state::notices::alerts`) and tells
//! it to a [`SystemNotifier`]. The binary gives the notifier of the operating system, which shows a
//! notification of the system. Every other start of the application state, such as a test
//! and the screen tool, gets one that only keeps what it was told: no automated run shows
//! a notification or makes the system ask for a permission.

#[cfg(target_os = "linux")]
mod linux;

use std::sync::{Arc, Mutex, PoisonError};

use gpui::{App, SystemNotification};

use crate::state::notices::Alert;

/// Shows and takes away notifications of the operating system.
pub trait SystemNotifier: 'static {
    fn unavailable_reason(&self) -> Option<&'static str> {
        None
    }

    fn show(&self, alert: &Alert, cx: &mut App);

    /// Takes away the notification with this tag, when the system still shows it.
    fn dismiss(&self, tag: &str, cx: &mut App);
}

/// The notifications of the system that gpui runs on. On macOS they are shown only when
/// the application runs from an application bundle, and the system asks the person for
/// the permission at the first one (ADR 0701).
pub struct PlatformNotifier;

impl PlatformNotifier {
    pub fn available() -> bool {
        #[cfg(target_os = "macos")]
        {
            let bundle = objc2_foundation::NSBundle::mainBundle();
            bundle.bundleIdentifier().is_some()
                && std::path::Path::new(&bundle.bundlePath().to_string())
                    .extension()
                    .is_some_and(|extension| extension == "app")
        }
        #[cfg(not(target_os = "macos"))]
        {
            true
        }
    }
}

impl SystemNotifier for PlatformNotifier {
    fn unavailable_reason(&self) -> Option<&'static str> {
        (!Self::available()).then_some(crate::text::NOTIFICATIONS_NEED_BUNDLE)
    }
    fn show(&self, alert: &Alert, cx: &mut App) {
        if !Self::available() {
            return;
        }
        cx.show_system_notification(SystemNotification {
            tag: alert.tag.clone().into(),
            title: alert.title.clone().into(),
            body: alert.body.clone().into(),
            actions: Vec::new(),
        });
    }

    fn dismiss(&self, tag: &str, cx: &mut App) {
        if !Self::available() {
            return;
        }
        cx.dismiss_system_notification(tag);
    }
}

/// Brings the application to the front after a press on one of its notifications or on the
/// tray. `token`: the activation token that the system gave for the press.
pub fn bring_to_front(token: Option<&str>, cx: &mut App) {
    cx.activate(true);
    // gpui activates no application on Linux and on Windows. The main window, which is the
    // first one, is asked instead: it is restored when it is minimized, and X11 and Windows
    // give it the focus. A Wayland compositor gives the focus only for a token that it
    // issued, such as the token of the press (ADR 0742).
    #[cfg(any(target_os = "linux", windows))]
    if let Some(window) = cx.windows().first() {
        let _ = window.update(cx, |_, window, _| match token {
            Some(token) => window.activate_with_token(token),
            None => window.activate_window(),
        });
    }
    #[cfg(not(any(target_os = "linux", windows)))]
    let _ = token;
}

/// Installs the system backend and its activation route only for the real application.
/// `data_dir` holds the picture of the notifications on Windows.
pub fn platform(
    cx: &mut App,
    data_dir: &std::path::Path,
    pressed: impl Fn(&str, Option<&str>, &mut App) + 'static,
) -> std::rc::Rc<dyn SystemNotifier> {
    #[cfg(windows)]
    windows_identity(cx, data_dir);
    #[cfg(not(windows))]
    let _ = data_dir;
    #[cfg(target_os = "linux")]
    {
        std::rc::Rc::new(linux::DesktopNotifier::new(cx, pressed))
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Without an application bundle macOS has no notifications of the application,
        // and nothing is asked of the system.
        if PlatformNotifier::available() {
            cx.on_system_notification_response(move |response, cx| {
                pressed(&response.tag, None, cx)
            });
        }
        std::rc::Rc::new(PlatformNotifier)
    }
}

/// A program without a package identity shows notifications on Windows only under an
/// application user model identifier that the account registered with a name and a picture
/// (ADR 0742). gpui writes the name at the first notification; the program writes the key and
/// the picture before, at the start, and names the identifier of the process.
#[cfg(windows)]
fn windows_identity(cx: &mut App, data_dir: &std::path::Path) {
    use junto_client_core::app_identity::{self, Registration};
    const PICTURE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/notification-icon.png"));
    let picture = data_dir.join(app_identity::NOTIFICATION_PICTURE);
    let registration = Registration {
        identifier: app_identity::APP_ID,
        name: crate::text::APP_NAME,
        picture: &picture,
    };
    if let Err(error) = app_identity::register(&registration, PICTURE) {
        tracing::warn!(%error, "the identity of the notifications was not registered");
    }
    cx.set_app_identity(app_identity::APP_ID, crate::text::APP_NAME);
}

/// What a [`KeptNotifier`] was told.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Kept {
    /// The notifications that would be on screen, the oldest first.
    pub shown: Vec<Alert>,
    /// Every tag that was taken away, in the order of the calls.
    pub dismissed: Vec<String>,
    /// Every notification that was shown, in the order of the calls.
    pub told: Vec<Alert>,
}

/// A notifier that shows nothing and keeps what it was told.
#[derive(Debug, Clone, Default)]
pub struct KeptNotifier {
    kept: Arc<Mutex<Kept>>,
}

impl KeptNotifier {
    pub fn kept(&self) -> Kept {
        self.kept
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl SystemNotifier for KeptNotifier {
    fn show(&self, alert: &Alert, _: &mut App) {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        // A notification with the tag of an earlier one replaces it.
        kept.shown.retain(|held| held.tag != alert.tag);
        kept.shown.push(alert.clone());
        kept.told.push(alert.clone());
    }

    fn dismiss(&self, tag: &str, _: &mut App) {
        let mut kept = self.kept.lock().unwrap_or_else(PoisonError::into_inner);
        kept.shown.retain(|held| held.tag != tag);
        kept.dismissed.push(tag.to_owned());
    }
}
