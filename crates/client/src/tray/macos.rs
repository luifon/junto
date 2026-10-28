//! The tray icon of macOS: a status item of the menu bar (ADR 0759).

use std::cell::RefCell;
use std::rc::Rc;

use junto_client_core::menu_bar::{Entry, StatusItem};

use super::{SystemTray, TrayAction, TrayState, renderings};

/// The side of the picture in the menu bar, in points: the pictures of the build script are
/// 18 and 36 pixels.
const POINTS: f64 = 18.0;

pub(super) struct MenuBarTray {
    item: Option<StatusItem>,
    /// The state of the shown menu.
    shown: Rc<RefCell<TrayState>>,
}

impl MenuBarTray {
    pub(super) fn new(presses: futures_channel::mpsc::UnboundedSender<TrayAction>) -> Self {
        let shown: Rc<RefCell<TrayState>> = Rc::default();
        let held = shown.clone();
        let item = StatusItem::new(Box::new(move |code| {
            if let Some(action) = held.borrow().action(code) {
                let _ = presses.unbounded_send(action);
            }
        }));
        if item.is_none() {
            tracing::warn!("the menu bar item was not created: not on the main thread");
        }
        Self { item, shown }
    }
}

impl SystemTray for MenuBarTray {
    fn set(&self, state: &TrayState) {
        let Some(item) = &self.item else {
            return;
        };
        // The template pictures at 1x and at 2x (ADR 0759).
        let pictures: Vec<&[u8]> = renderings(state.icon()).map(|(_, png)| png).collect();
        if pictures.is_empty() {
            return;
        }
        let menu = state.menu();
        let entries: Vec<Entry<'_>> = menu
            .iter()
            .map(|entry| Entry {
                label: entry.label,
                code: entry.code,
                enabled: entry.enabled,
                separated: entry.separated,
            })
            .collect();
        *self.shown.borrow_mut() = state.clone();
        item.set(&pictures, POINTS, &state.tooltip(), &entries);
    }
}
