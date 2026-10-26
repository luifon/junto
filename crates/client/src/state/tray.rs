//! The state of the application tells the tray icon the state of the call of this device
//! (ADR 0759).

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

    /// Tells the tray icon the unread mentions that the application counts: the icon shows
    /// its mention mark while there is one, and the tooltip states the number (ADR 0759).
    pub fn set_tray_mentions(&mut self, mentions: u64, cx: &mut Context<Self>) {
        if self.tray_mentions != mentions {
            self.tray_mentions = mentions;
            self.follow_tray();
            cx.notify();
        }
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
        let mut shown = derive(calls);
        shown.mentions = self.tray_mentions;
        let told = self.tray_shown.as_ref();
        let same_call = told.map(|told| &told.call) == Some(&shown.call);
        if same_call && told.map(|told| told.mentions) == Some(shown.mentions) {
            return;
        }
        // A new menu: a press on a menu of an earlier generation is refused (ADR 0788). A
        // change of the mentions alone keeps the menu.
        shown.generation = match told {
            Some(told) if same_call => told.generation,
            Some(told) => told.generation.wrapping_add(1),
            None => 0,
        };
        self.tray.set(&shown);
        self.tray_shown = Some(shown);
    }
}
