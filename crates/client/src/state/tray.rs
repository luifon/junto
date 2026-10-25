//! The state of the application tells the tray icon the state of the call of this device
//! (ADR 0757).

use std::rc::Rc;

use gpui::Context;

use super::AppState;
use crate::tray::{KeptTray, SystemTray, derive};

impl AppState {
    /// Uses this tray for the tray icon, and tells it the state of now.
    pub fn set_tray(&mut self, tray: Rc<dyn SystemTray>, cx: &mut Context<Self>) {
        self.tray = tray;
        self.tray_shown = None;
        self.follow_tray();
        cx.notify();
    }

    /// The tray of a start without the tray icon of the system.
    pub fn kept_tray(&self) -> KeptTray {
        self.kept_tray.clone()
    }

    /// Tells the tray the state of the calls as the state of the application has taken them,
    /// when it differs from the state that the tray was told last.
    pub(super) fn follow_tray(&mut self) {
        #[cfg(feature = "test-support")]
        let calls = match &self.media_fixture {
            Some(media) => &media.calls,
            None => &self.known.media.calls,
        };
        #[cfg(not(feature = "test-support"))]
        let calls = &self.known.media.calls;
        let shown = derive(calls);
        if self.tray_shown.as_ref() != Some(&shown) {
            self.tray.set(&shown);
            self.tray_shown = Some(shown);
        }
    }
}
