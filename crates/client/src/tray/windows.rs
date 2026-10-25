//! The tray icon of Windows: an icon of the notification area (ADR 0759).

use std::cell::RefCell;
use std::rc::Rc;

use junto_client_core::notification_area::{Entry, NotificationIcon, Press, small_icon_side};

use super::{Picture, SystemTray, TrayAction, TrayState, renderings};

pub(super) struct NotificationAreaTray {
    icon: NotificationIcon,
    /// The state of the shown menu.
    shown: Rc<RefCell<TrayState>>,
}

impl NotificationAreaTray {
    pub(super) fn new(
        presses: futures_channel::mpsc::UnboundedSender<TrayAction>,
    ) -> std::io::Result<Self> {
        let shown: Rc<RefCell<TrayState>> = Rc::default();
        let held = shown.clone();
        let icon = NotificationIcon::new(Box::new(move |press| {
            let action = match press {
                Press::Entry(code) => held.borrow().action(code),
                Press::Icon => Some(TrayAction::Open),
            };
            if let Some(action) = action {
                let _ = presses.unbounded_send(action);
            }
        }))?;
        Ok(Self { icon, shown })
    }

    /// Takes the icon away before the application ends.
    pub(super) fn remove(&self) {
        self.icon.remove();
    }
}

/// The smallest rendering at least as large as the small icons of the system, or the largest.
fn rendering(picture: Picture, wanted: u32) -> Option<(u32, &'static [u8])> {
    let all: Vec<_> = renderings(picture).collect();
    all.iter()
        .find(|(side, _)| *side >= wanted)
        .or(all.last())
        .copied()
}

impl SystemTray for NotificationAreaTray {
    fn set(&self, state: &TrayState) {
        let Some((side, rgba)) = rendering(state.picture(), small_icon_side()) else {
            return;
        };
        let entries = state
            .menu()
            .into_iter()
            .map(|entry| Entry {
                label: entry.label.to_owned(),
                code: entry.action.code(),
                enabled: entry.enabled,
                separated: entry.separated,
            })
            .collect();
        *self.shown.borrow_mut() = state.clone();
        if let Err(error) = self.icon.set(rgba, side, state.tooltip(), entries) {
            tracing::warn!(%error, "the icon of the notification area was not shown");
        }
    }
}
