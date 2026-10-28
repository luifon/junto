//! The icon of the application in the menu bar of macOS, in the notification area of Windows
//! and in the status area of a Linux desktop, with a menu of call actions (ADR 0759).
//!
//! The state of the application derives a [`TrayState`] at each change and tells it to a
//! [`SystemTray`]. Only the start of the binary installs the tray of the system; every other
//! start, such as a test and the screen tool, gets a [`KeptTray`] that only keeps what it was
//! told.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{App, Entity};
use junto_media::calls::{CallsView, Seat};

use crate::state::AppState;
use crate::state::view::MicrophoneAction;
use crate::text::tray as text;

/// What the tray icon shows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrayState {
    /// The call of this device. Absent outside a call.
    pub call: Option<CallMarks>,
    /// The number of the menu: it changes with each change of the call or its marks. The
    /// code of an entry holds it, so a press on a menu that was drawn before a change is
    /// refused (ADR 0788).
    pub generation: u32,
    /// The unread messages for the person on every server, mentions and direct messages, as
    /// the server book counts them; the icon shows its mark while there is one.
    pub mentions: u64,
}

/// The state of the call of this device that the icon and the menu show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallMarks {
    /// The person closed the microphone.
    pub muted: bool,
    /// A moderator or meeting mode closed the microphone; the person cannot open it.
    pub held: bool,
    /// The person turned the sound off.
    pub deafened: bool,
    /// The call and the seat that an entry of the menu acts on (ADR 0788).
    pub seat: Seat,
}

/// The picture of the icon for a state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picture {
    Plain,
    Call,
    Muted,
    /// A moderator or meeting mode holds the microphone closed.
    Held,
    SoundOff,
}

impl Picture {
    /// The name of the picture in the files of the build script.
    pub fn name(self) -> &'static str {
        match self {
            Picture::Plain => "plain",
            Picture::Call => "call",
            Picture::Muted => "muted",
            Picture::Held => "held",
            Picture::SoundOff => "sound-off",
        }
    }
}

/// What the icon shows: the picture of a state, with the mention mark or without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconPicture {
    pub picture: Picture,
    pub mention: bool,
}

impl IconPicture {
    /// The name of the rendering in the files of the build script.
    pub fn name(self) -> String {
        if self.mention {
            format!("{}-mention", self.picture.name())
        } else {
            self.picture.name().to_owned()
        }
    }
}

/// What an entry of the menu does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayAction {
    /// What the microphone control does, with the seat of the menu.
    Microphone(MicrophoneAction),
    /// Turns the sound off (`true`) or on.
    Sound(bool),
    Leave,
    Open,
    Quit,
}

impl TrayAction {
    /// The number of the kind of the action, the same in every menu.
    pub fn code(&self) -> u32 {
        match self {
            TrayAction::Microphone(_) => 1,
            TrayAction::Sound(_) => 2,
            TrayAction::Leave => 3,
            TrayAction::Open => 4,
            TrayAction::Quit => 5,
        }
    }
}

/// One entry of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuEntry {
    /// The identifier of the entry in the menu of one generation: the generation and the kind
    /// of the action (`entry_code`).
    pub code: u32,
    pub label: &'static str,
    pub action: TrayAction,
    /// The entry can be pressed.
    pub enabled: bool,
    /// A line separates the entry from the one before.
    pub separated: bool,
}

/// The tray state of the calls of this device.
pub fn derive(calls: &CallsView) -> TrayState {
    TrayState {
        call: calls.call.as_ref().map(|call| CallMarks {
            muted: calls.muted,
            held: call.microphone_block.is_some(),
            deafened: calls.deafened,
            seat: call.seat(),
        }),
        generation: 0,
        mentions: 0,
    }
}

/// The code of an entry: the generation of its menu and the kind of its action. The codes stay
/// below 2^31, the identifiers of a D-Bus menu.
pub fn entry_code(generation: u32, kind: u32) -> u32 {
    ((generation & 0x00FF_FFFF) << 3) | (kind & 0x7)
}

impl TrayState {
    /// The picture of the call state. The sound off takes precedence, then a microphone that
    /// the server holds closed, then a microphone that the person closed.
    pub fn picture(&self) -> Picture {
        match &self.call {
            None => Picture::Plain,
            Some(call) if call.deafened => Picture::SoundOff,
            Some(call) if call.held => Picture::Held,
            Some(call) if call.muted => Picture::Muted,
            Some(_) => Picture::Call,
        }
    }

    /// The picture with the mention mark while the application counts a mention.
    pub fn icon(&self) -> IconPicture {
        IconPicture {
            picture: self.picture(),
            mention: self.mentions > 0,
        }
    }

    /// The text that the system shows when the pointer rests on the icon.
    pub fn tooltip(&self) -> String {
        let state = match self.picture() {
            Picture::Plain => text::NOT_IN_A_CALL,
            Picture::Call => text::IN_A_CALL,
            Picture::Muted => text::IN_A_CALL_MUTED,
            Picture::Held => text::IN_A_CALL_HELD,
            Picture::SoundOff => text::IN_A_CALL_SOUND_OFF,
        };
        match self.mentions {
            0 => state.to_owned(),
            count => format!("{state}, {}", text::mentions(count)),
        }
    }

    /// The entries of the menu: the call actions in a call, then the application.
    pub fn menu(&self) -> Vec<MenuEntry> {
        let mut entries = Vec::new();
        if let Some(call) = &self.call {
            entries.push(MenuEntry {
                code: 0,
                label: if call.muted {
                    text::UNMUTE_MICROPHONE
                } else {
                    text::MUTE_MICROPHONE
                },
                action: TrayAction::Microphone(if call.muted {
                    MicrophoneAction::Open(Some(call.seat.clone()))
                } else {
                    MicrophoneAction::Close
                }),
                // The person cannot open a microphone that the server holds closed.
                enabled: !(call.muted && call.held),
                separated: false,
            });
            entries.push(MenuEntry {
                code: 0,
                label: if call.deafened {
                    text::TURN_SOUND_ON
                } else {
                    text::TURN_SOUND_OFF
                },
                action: TrayAction::Sound(!call.deafened),
                enabled: true,
                separated: false,
            });
            entries.push(MenuEntry {
                code: 0,
                label: text::LEAVE_THE_CALL,
                action: TrayAction::Leave,
                enabled: true,
                separated: false,
            });
        }
        entries.push(MenuEntry {
            code: 0,
            label: text::OPEN_JUNTO,
            action: TrayAction::Open,
            enabled: true,
            separated: self.call.is_some(),
        });
        entries.push(MenuEntry {
            code: 0,
            label: text::QUIT,
            action: TrayAction::Quit,
            enabled: true,
            separated: false,
        });
        for entry in &mut entries {
            entry.code = entry_code(self.generation, entry.action.code());
        }
        entries
    }

    /// The action of the enabled entry with this code (`MenuEntry::code`). A code of a menu of
    /// another generation names no entry: the press of a menu that was drawn before a change
    /// of the call does nothing, so it acts neither on another call nor in the other direction
    /// (ADR 0788).
    pub fn action(&self, code: u32) -> Option<TrayAction> {
        self.menu()
            .into_iter()
            .find(|entry| entry.enabled && entry.code == code)
            .map(|entry| entry.action)
    }
}

/// Shows the tray icon of a state.
pub trait SystemTray: 'static {
    fn set(&self, state: &TrayState);
}

/// Does what a pressed entry of the menu does. The platform trays call it on the thread of the
/// windows.
pub fn press(state: &Entity<AppState>, action: &TrayAction, cx: &mut App) {
    match action {
        TrayAction::Microphone(action) => state.update(cx, |state, _| state.use_microphone(action)),
        TrayAction::Sound(off) => state.update(cx, |state, _| state.set_sound_off(*off)),
        TrayAction::Leave => state.update(cx, |state, cx| state.leave_call(cx)),
        TrayAction::Open => crate::notify::bring_to_front(None, cx),
        TrayAction::Quit => crate::app::quit(cx),
    }
}

/// The tray of the system, for the start of the binary. `pressed` runs on the thread of the
/// windows for each pressed entry. Without a tray of the system the start gets a kept tray.
pub fn platform(
    cx: &mut App,
    pressed: impl Fn(TrayAction, &mut App) + 'static,
) -> Rc<dyn SystemTray> {
    let (sender, mut presses) = futures_channel::mpsc::unbounded::<TrayAction>();
    cx.spawn(async move |cx| {
        use futures_util::StreamExt as _;
        while let Some(action) = presses.next().await {
            cx.update(|cx| pressed(action, cx));
        }
    })
    .detach();
    #[cfg(target_os = "linux")]
    {
        Rc::new(linux::StatusNotifierTray::new(sender))
    }
    #[cfg(target_os = "macos")]
    {
        Rc::new(macos::MenuBarTray::new(sender))
    }
    #[cfg(windows)]
    {
        match windows::NotificationAreaTray::new(sender) {
            Ok(tray) => {
                let tray = Rc::new(tray);
                let held = tray.clone();
                cx.on_app_quit(move |_| {
                    held.remove();
                    async {}
                })
                .detach();
                tray
            }
            Err(error) => {
                tracing::warn!(%error, "the icon of the notification area was not created");
                Rc::new(KeptTray::default())
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    {
        drop(sender);
        Rc::new(KeptTray::default())
    }
}

/// The pictures that the build script rendered, as `(picture name, side, bytes)`: on macOS PNG
/// pictures, elsewhere straight RGBA pixels.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
const PICTURES: &[(&str, u32, &[u8])] = include!(concat!(env!("OUT_DIR"), "/tray-pictures.rs"));

/// The renderings of a picture, as `(side, bytes)`, the smallest first.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn renderings(shown: IconPicture) -> impl Iterator<Item = (u32, &'static [u8])> {
    let name = shown.name();
    PICTURES
        .iter()
        .filter(move |(held, _, _)| *held == name)
        .map(|(_, side, bytes)| (*side, *bytes))
}

/// A tray that shows nothing and keeps each state that it was told.
#[derive(Debug, Clone, Default)]
pub struct KeptTray {
    told: Rc<RefCell<Vec<TrayState>>>,
}

impl KeptTray {
    /// Every state that was told, in the order of the calls.
    pub fn told(&self) -> Vec<TrayState> {
        self.told.borrow().clone()
    }

    /// The state that the icon would show now.
    pub fn shown(&self) -> TrayState {
        self.told.borrow().last().cloned().unwrap_or_default()
    }

    /// Presses the entry of the shown menu with this label, as a person does, and returns
    /// whether the menu has an enabled entry with it.
    pub fn press(&self, label: &str, state: &Entity<AppState>, cx: &mut App) -> bool {
        let entry = self
            .shown()
            .menu()
            .into_iter()
            .find(|entry| entry.label == label && entry.enabled);
        match entry {
            Some(entry) => {
                press(state, &entry.action, cx);
                true
            }
            None => false,
        }
    }
}

impl SystemTray for KeptTray {
    fn set(&self, state: &TrayState) {
        self.told.borrow_mut().push(state.clone());
    }
}

#[cfg(test)]
mod tests;
