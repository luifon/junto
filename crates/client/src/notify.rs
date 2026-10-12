//! The notifications of the operating system, behind an interface (ADR 0742).
//!
//! The state of the application decides what is shown (`state::notices::alerts`) and tells
//! it to a [`SystemNotifier`]. The binary gives the notifier of gpui, which shows a
//! notification of the system. Every other start of the application state, such as a test
//! and the screen tool, gets one that only keeps what it was told: no automated run shows
//! a notification or makes the system ask for a permission.

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
