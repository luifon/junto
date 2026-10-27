//! The key bindings of the person (ADR 0777): each action of the window that a key can do,
//! its default key, and the keys that the person chose. Every binding works while a window
//! of Junto is in front; a binding of a call action can also work while another application
//! is in front, through the keys of the system (ADR 0776).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::global_keys::{Key, KeyCombo};

/// An action that a key can do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    ToggleMute,
    ToggleSoundOff,
    PushToTalk,
    PushToMute,
    ToggleCamera,
    ShareScreen,
    LeaveCall,
    AnswerCall,
    DeclineCall,
    ShowCall,
    PreviousServer,
    NextServer,
    PreviousWorkspace,
    NextWorkspace,
    PreviousChannel,
    NextChannel,
    PreviousUnreadChannel,
    NextUnreadChannel,
    ToggleServerBook,
    Search,
    Notices,
    JumpToNewest,
    MembersList,
    PinnedMessages,
    MarkChannelRead,
    MarkServerRead,
    ReplyToNewest,
    UploadFile,
    OpenSettings,
    OpenAdministration,
    OpenKeybinds,
}

/// The groups of the actions, in the order of the settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionGroup {
    Call,
    Navigation,
    Messages,
    App,
}

impl KeyAction {
    pub const ALL: [KeyAction; 31] = [
        KeyAction::ToggleMute,
        KeyAction::ToggleSoundOff,
        KeyAction::PushToTalk,
        KeyAction::PushToMute,
        KeyAction::ToggleCamera,
        KeyAction::ShareScreen,
        KeyAction::LeaveCall,
        KeyAction::AnswerCall,
        KeyAction::DeclineCall,
        KeyAction::ShowCall,
        KeyAction::PreviousServer,
        KeyAction::NextServer,
        KeyAction::PreviousWorkspace,
        KeyAction::NextWorkspace,
        KeyAction::PreviousChannel,
        KeyAction::NextChannel,
        KeyAction::PreviousUnreadChannel,
        KeyAction::NextUnreadChannel,
        KeyAction::ToggleServerBook,
        KeyAction::Search,
        KeyAction::Notices,
        KeyAction::JumpToNewest,
        KeyAction::MembersList,
        KeyAction::PinnedMessages,
        KeyAction::MarkChannelRead,
        KeyAction::MarkServerRead,
        KeyAction::ReplyToNewest,
        KeyAction::UploadFile,
        KeyAction::OpenSettings,
        KeyAction::OpenAdministration,
        KeyAction::OpenKeybinds,
    ];

    pub fn group(self) -> ActionGroup {
        use KeyAction::*;
        match self {
            ToggleMute | ToggleSoundOff | PushToTalk | PushToMute | ToggleCamera | ShareScreen
            | LeaveCall | AnswerCall | DeclineCall | ShowCall => ActionGroup::Call,
            PreviousServer
            | NextServer
            | PreviousWorkspace
            | NextWorkspace
            | PreviousChannel
            | NextChannel
            | PreviousUnreadChannel
            | NextUnreadChannel
            | ToggleServerBook
            | Search
            | Notices
            | JumpToNewest
            | MembersList
            | PinnedMessages => ActionGroup::Navigation,
            MarkChannelRead | MarkServerRead | ReplyToNewest | UploadFile => ActionGroup::Messages,
            OpenSettings | OpenAdministration | OpenKeybinds => ActionGroup::App,
        }
    }

    /// The name of the action in the settings file and in the shortcuts of a desktop portal.
    pub fn name(self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    /// What the action does, in the dialog of a desktop that confirms the shortcuts that
    /// Junto binds through its portal (ADR 0776).
    pub fn description(self) -> &'static str {
        use KeyAction::*;
        match self {
            ToggleMute => "Mute or unmute the microphone in a call of Junto",
            ToggleSoundOff => "Turn the sound of a call of Junto off or on",
            PushToTalk => "Push to talk in a call of Junto",
            PushToMute => "Push to mute in a call of Junto",
            ToggleCamera => "Turn the camera on or off in a call of Junto",
            ShareScreen => "Share a screen or a window in a call of Junto",
            LeaveCall => "Leave the call of Junto",
            AnswerCall => "Answer a call of Junto",
            DeclineCall => "Decline a call of Junto",
            ShowCall => "Show the call of Junto",
            PreviousServer
            | NextServer
            | PreviousWorkspace
            | NextWorkspace
            | PreviousChannel
            | NextChannel
            | PreviousUnreadChannel
            | NextUnreadChannel
            | ToggleServerBook
            | Search
            | Notices
            | JumpToNewest
            | MembersList
            | PinnedMessages
            | MarkChannelRead
            | MarkServerRead
            | ReplyToNewest
            | UploadFile
            | OpenSettings
            | OpenAdministration
            | OpenKeybinds => "An action of the window of Junto",
        }
    }

    /// True for an action that lasts while its key is held: push to talk and push to mute.
    /// Every other action happens once at each press.
    pub fn holds(self) -> bool {
        matches!(self, KeyAction::PushToTalk | KeyAction::PushToMute)
    }

    /// True for an action whose key may also work while another application is in front:
    /// the actions of a call that need no window of Junto.
    pub fn may_work_outside(self) -> bool {
        use KeyAction::*;
        matches!(
            self,
            ToggleMute
                | ToggleSoundOff
                | PushToTalk
                | PushToMute
                | ToggleCamera
                | LeaveCall
                | AnswerCall
                | DeclineCall
        )
    }

    /// True when a new binding of the action works while another application is in front:
    /// the microphone and the sound, as in Discord. The others are switched on by the person.
    fn outside_by_default(self) -> bool {
        use KeyAction::*;
        matches!(self, ToggleMute | ToggleSoundOff | PushToTalk | PushToMute)
    }

    /// The default keys, as Discord's where Discord has the action. The actions of the
    /// window use Command on macOS where they use Control elsewhere; the actions of a call
    /// use Control and Shift on every system, as Discord. Push to talk has the key of the
    /// sound settings (ADR 0507) and is not here.
    pub fn default_keys(self, macos: bool) -> Option<KeyCombo> {
        use Key::*;
        use KeyAction::*;
        let main = if macos { Meta } else { Control };
        let keys: &[Key] = match self {
            ToggleMute => &[Control, Shift, Letter('M')],
            ToggleSoundOff => &[Control, Shift, Letter('D')],
            PushToTalk | PushToMute => return None,
            ToggleCamera => &[Control, Shift, Letter('E')],
            ShareScreen => &[Control, Shift, Letter('S')],
            LeaveCall => &[Control, Shift, Letter('H')],
            AnswerCall => &[Control, Enter],
            DeclineCall => &[Control, Shift, Backspace],
            ShowCall => &[Control, Alt, Letter('A')],
            PreviousServer => &[main, Alt, Up],
            NextServer => &[main, Alt, Down],
            PreviousWorkspace => &[main, Shift, Up],
            NextWorkspace => &[main, Shift, Down],
            PreviousChannel => &[Alt, Up],
            NextChannel => &[Alt, Down],
            PreviousUnreadChannel => &[Alt, Shift, Up],
            NextUnreadChannel => &[Alt, Shift, Down],
            ToggleServerBook => &[main, Shift, Letter('B')],
            Search => &[main, Letter('F')],
            Notices => &[main, Letter('I')],
            JumpToNewest => &[Shift, PageDown],
            MembersList => &[main, Letter('U')],
            PinnedMessages => &[main, Letter('P')],
            MarkChannelRead => &[Shift, Escape],
            MarkServerRead => &[main, Shift, Escape],
            ReplyToNewest => &[main, Shift, Letter('R')],
            UploadFile => &[main, Shift, Letter('U')],
            OpenSettings => &[main, Comma],
            OpenAdministration => &[main, Shift, Comma],
            OpenKeybinds => &[main, Slash],
        };
        KeyCombo::new(keys.iter().copied()).ok()
    }
}

/// The keys of one action and whether they also work while another application is in front.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Binding {
    /// Absent: the action has no key.
    pub keys: Option<KeyCombo>,
    #[serde(default)]
    pub outside: bool,
}

/// The bindings that the person changed. An action that is not here has its default.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Keybinds {
    changed: BTreeMap<KeyAction, Binding>,
}

/// The most bindings that a settings file holds: one per action.
pub const MOST_BINDINGS: usize = KeyAction::ALL.len();

impl Keybinds {
    /// The default binding of an action on this system.
    pub fn default_binding(action: KeyAction) -> Binding {
        Binding {
            keys: action.default_keys(cfg!(target_os = "macos")),
            outside: action.outside_by_default(),
        }
    }

    /// The binding of an action now.
    pub fn get(&self, action: KeyAction) -> Binding {
        self.changed
            .get(&action)
            .cloned()
            .unwrap_or_else(|| Self::default_binding(action))
    }

    /// True when the person changed the binding of the action.
    pub fn is_changed(&self, action: KeyAction) -> bool {
        self.changed.contains_key(&action)
    }

    fn store(&mut self, action: KeyAction, binding: Binding) {
        if binding == Self::default_binding(action) {
            self.changed.remove(&action);
        } else {
            self.changed.insert(action, binding);
        }
    }

    /// Gives the action these keys; absent: no key. A key without a modifier is refused,
    /// except for an action that is held, so that typing does not fire an action.
    pub fn set(&mut self, action: KeyAction, keys: Option<KeyCombo>) -> Result<(), KeybindError> {
        if let Some(keys) = &keys
            && !action.holds()
            && !keys.is_shortcut()
        {
            return Err(KeybindError::NeedsModifier);
        }
        let outside = self.get(action).outside;
        self.store(action, Binding { keys, outside });
        Ok(())
    }

    /// Lets the keys of an action work while another application is in front, or only while
    /// a window of Junto is.
    pub fn set_outside(&mut self, action: KeyAction, outside: bool) -> Result<(), KeybindError> {
        if outside && !action.may_work_outside() {
            return Err(KeybindError::NotOutside);
        }
        let keys = self.get(action).keys;
        self.store(action, Binding { keys, outside });
        Ok(())
    }

    pub fn reset(&mut self, action: KeyAction) {
        self.changed.remove(&action);
    }

    pub fn reset_all(&mut self) {
        self.changed.clear();
    }

    /// The other actions with the same keys as `action`.
    pub fn conflicts(&self, action: KeyAction) -> Vec<KeyAction> {
        let Some(keys) = self.get(action).keys else {
            return Vec::new();
        };
        KeyAction::ALL
            .into_iter()
            .filter(|other| *other != action && self.get(*other).keys.as_ref() == Some(&keys))
            .collect()
    }

    /// The action of a key press while a window of Junto is in front: the first action in
    /// the order of the settings whose keys are these.
    pub fn action_for(&self, pressed: &KeyCombo) -> Option<KeyAction> {
        KeyAction::ALL
            .into_iter()
            .find(|action| self.get(*action).keys.as_ref() == Some(pressed))
    }

    /// The bindings that work while another application is in front, with keys.
    pub fn outside(&self) -> Vec<(KeyAction, KeyCombo)> {
        KeyAction::ALL
            .into_iter()
            .filter_map(|action| {
                let binding = self.get(action);
                (binding.outside && action.may_work_outside())
                    .then_some(binding.keys)
                    .flatten()
                    .map(|keys| (action, keys))
            })
            .collect()
    }

    /// Drops what a settings file may not hold: an action that may not work outside marked
    /// so, and a key without a modifier for an action that is not held.
    pub fn checked(mut self) -> Self {
        self.changed.retain(|action, binding| {
            binding
                .keys
                .as_ref()
                .is_none_or(|keys| action.holds() || keys.is_shortcut())
        });
        for (action, binding) in &mut self.changed {
            binding.outside &= action.may_work_outside();
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KeybindError {
    #[error("a key of this action needs Control, Alt, Shift or the system key")]
    NeedsModifier,
    #[error("this action works only while a window of Junto is in front")]
    NotOutside,
}

#[cfg(test)]
mod tests;
