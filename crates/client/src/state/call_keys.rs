//! The key bindings of the person (ADR 0777) and the keys of a call in every application
//! (ADR 0776): the state keeps the bindings, gives the source of the keys of the system the
//! bindings that work while another application is in front, and does the call actions of
//! the keys. A window that is in front gives every binding too, so that each works where the
//! system has no source.

use futures_util::StreamExt as _;
use gpui::Context;
use junto_client_core::global_keys::{CallKeys, GlobalKeys, KeyCombo, KeyEvent, KeysState};
use junto_client_core::keybinds::{KeyAction, KeybindError, Keybinds};
use junto_client_core::voice::InputMode;

use super::AppState;
use super::view::ServerSnapshot;
use crate::text;
use junto_client_core::flow::FlowError;

impl AppState {
    /// Uses the keys of the system: the reader of its keyboard, or the global shortcuts
    /// portal of a Wayland desktop. Only the start of the binary does it; every other start
    /// has no source, and only its windows give the keys.
    pub fn install_global_keys(&mut self, cx: &mut Context<Self>) {
        let (sender, events) = futures_channel::mpsc::unbounded();
        let runtime = self.client.runtime().handle().clone();
        let keys = GlobalKeys::of_system(&runtime, move |event| {
            let _ = sender.unbounded_send(event);
        });
        self.use_global_keys(keys, events, cx);
    }

    /// Reads the keys with the keys that a test holds.
    #[cfg(feature = "test-support")]
    pub fn install_held_keys(
        &mut self,
        held: junto_client_core::global_keys::HeldKeys,
        cx: &mut Context<Self>,
    ) {
        let (sender, events) = futures_channel::mpsc::unbounded();
        let keys = GlobalKeys::with_reader(Box::new(held), move |event| {
            let _ = sender.unbounded_send(event);
        });
        self.use_global_keys(keys, events, cx);
    }

    fn use_global_keys(
        &mut self,
        keys: GlobalKeys,
        mut events: futures_channel::mpsc::UnboundedReceiver<KeyEvent>,
        cx: &mut Context<Self>,
    ) {
        self.global_keys = keys;
        self.call_keys_told = None;
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.next().await {
                if this
                    .update(cx, |this, cx| this.global_key(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let snapshots = self.snapshots();
        self.follow_call_keys(&snapshots);
        cx.notify();
    }

    fn global_key(&mut self, event: KeyEvent, cx: &mut Context<Self>) {
        if let KeyEvent::Ready(boundary) = event {
            if boundary == self.keybind_boundary {
                self.keybind_global_ready = true;
            }
            return;
        }
        if self.recording_keybind() || !self.keybind_global_ready {
            return;
        }
        match event {
            KeyEvent::Ready(_) => {}
            KeyEvent::Held(KeyAction::PushToTalk, held) => {
                self.talking_globally = held;
                self.sync_talking();
            }
            KeyEvent::Held(action, held) => self.hold_action(None, action, held),
            KeyEvent::Pressed(action) => {
                if !self.perform_call_action(action, cx) && action.may_work_outside() {
                    crate::notify::bring_to_front(cx);
                    cx.emit(super::WindowKeyAction(action));
                }
            }
        }
        cx.notify();
    }

    pub fn recording_keybind(&self) -> bool {
        !self.keybind_recordings.is_empty()
    }

    pub fn begin_keybind_recording(&mut self, window: gpui::EntityId) {
        self.keybind_recordings.insert(window);
        self.keybind_boundary += 1;
        self.keybind_global_ready = false;
        self.global_keys.pause_for_recording();
        self.talking_windows.clear();
        self.talking_globally = false;
        self.sync_talking();
        self.mute_holds.clear();
        self.hold_action(None, KeyAction::PushToMute, false);
    }

    pub fn end_keybind_recording(&mut self, window: gpui::EntityId) {
        if self.keybind_recordings.remove(&window) && self.keybind_recordings.is_empty() {
            self.keybind_boundary += 1;
            self.keybind_global_ready = !self
                .global_keys
                .resume_after_release(self.call_keys_told.clone(), self.keybind_boundary);
        }
    }

    /// What the key that mutes does: it closes an open microphone of the call, and opens a
    /// closed one at the seat of the call, as the button of the microphone does. Outside a
    /// call it does nothing.
    pub fn toggle_mute(&mut self) {
        let calls = self.calls.view();
        let Some(call) = &calls.call else {
            return;
        };
        if calls.muted {
            self.calls.unmute_in(Some(&call.seat()));
        } else {
            self.calls.mute();
        }
    }

    /// Does a call action of a key: the actions that need no window. Returns false for
    /// another action, which a window does.
    pub fn perform_call_action(&mut self, action: KeyAction, cx: &mut Context<Self>) -> bool {
        match action {
            KeyAction::ToggleMute => self.toggle_mute(),
            KeyAction::ToggleSoundOff => {
                let off = self.calls.view().deafened;
                self.set_sound_off(!off);
            }
            KeyAction::ToggleCamera => {
                if let Some(camera) = self.camera_action() {
                    self.use_camera(&camera, cx);
                }
            }
            KeyAction::LeaveCall => {
                if self.calls.view().call.is_some() {
                    self.leave_call(cx);
                }
            }
            KeyAction::AnswerCall | KeyAction::DeclineCall => {
                if let Some(invited) = self.view().invitations.first().cloned() {
                    if action == KeyAction::AnswerCall {
                        self.accept_invitation(&invited, cx);
                    } else {
                        self.decline_invitation(&invited, cx);
                    }
                }
            }
            _ => return false,
        }
        true
    }

    /// The keys of push to mute are held or let go in a window (`Some`) or on the keyboard
    /// of the system (`None`): while any holds them, an open microphone of the call is
    /// closed, and it opens again when the last one lets them go. Push to talk is
    /// [`AppState::window_talking`].
    pub fn hold_action(&mut self, window: Option<gpui::EntityId>, action: KeyAction, held: bool) {
        if action != KeyAction::PushToMute {
            return;
        }
        let held = held && !self.recording_keybind();
        if held {
            self.mute_holds.insert(window);
        } else {
            self.mute_holds.remove(&window);
        }
        let calls = self.calls.view();
        let Some(call) = &calls.call else {
            self.mute_holds.clear();
            self.muted_by_hold = false;
            return;
        };
        if !self.mute_holds.is_empty() && !calls.muted {
            self.calls.mute();
            self.muted_by_hold = true;
        } else if self.mute_holds.is_empty() && std::mem::take(&mut self.muted_by_hold) {
            self.calls.unmute_in(Some(&call.seat()));
        }
    }

    /// True while a window or the keyboard of the system holds the keys of push to mute.
    pub fn holds_mute(&self) -> bool {
        !self.mute_holds.is_empty()
    }

    pub fn release_window_keys(&mut self, window: gpui::EntityId) {
        self.window_talking(window, false);
        self.hold_action(Some(window), KeyAction::PushToMute, false);
    }

    /// True when the keyboard of the system gives the keys of `action` now: a window then
    /// does not do the action a second time at the same press.
    pub fn system_gives(&self, action: KeyAction) -> bool {
        let source = matches!(
            self.global_keys.state(),
            KeysState::Reading | KeysState::Portal
        );
        source
            && self.keybind_global_ready
            && self
                .call_keys_told
                .as_ref()
                .is_some_and(|keys| keys.bindings.iter().any(|(bound, _)| *bound == action))
    }

    /// The keys of push to talk: the binding of the person, or the key of the sound settings
    /// while the person did not change the binding (ADR 0507, ADR 0777).
    pub fn talk_keys(&self) -> Option<KeyCombo> {
        let binding = self.keybinds.get(KeyAction::PushToTalk);
        if binding.inherited {
            Some(self.voice().talk_key.combo())
        } else {
            binding.keys
        }
    }

    /// The keys of `action` as they work now.
    pub fn keys_of(&self, action: KeyAction) -> Option<KeyCombo> {
        if action == KeyAction::PushToTalk {
            self.talk_keys()
        } else {
            self.keybinds.get(action).keys
        }
    }

    /// The bindings that the keyboard of the system reads now: those that work while
    /// another application is in front, while this device is in a call or a call rings for
    /// it. Push to talk only while push to talk is on. None while nothing needs them.
    fn call_keys(&self, snapshots: &[ServerSnapshot]) -> Option<CallKeys> {
        let in_call = self.known.media.calls.call.is_some();
        let ringing = snapshots.iter().any(|server| {
            super::view::shown_model(server)
                .is_some_and(|model| model.incoming_invitations().next().is_some())
        });
        if !in_call && !ringing {
            return None;
        }
        let voice = self.voice();
        let bindings: Vec<(KeyAction, KeyCombo)> = KeyAction::ALL
            .into_iter()
            .filter(|action| {
                let binding = self.keybinds.get(*action);
                action.may_work_outside() && binding.outside
            })
            .filter(|action| match action {
                KeyAction::AnswerCall | KeyAction::DeclineCall => ringing,
                KeyAction::PushToTalk => in_call && voice.input_mode == InputMode::PushToTalk,
                _ => in_call,
            })
            .filter_map(|action| self.keys_of(action).map(|keys| (action, keys)))
            .collect();
        let keys = CallKeys { bindings }.resolved();
        (!keys.is_empty()).then_some(keys)
    }

    /// Tells the source the keys that the calls need now, when they changed.
    pub(super) fn follow_call_keys(&mut self, snapshots: &[ServerSnapshot]) {
        let wanted = self.call_keys(snapshots);
        if self.call_keys_told != wanted {
            let talks = wanted.as_ref().is_some_and(|keys| {
                keys.bindings
                    .iter()
                    .any(|(action, _)| *action == KeyAction::PushToTalk)
            });
            if !talks && self.talking_globally {
                self.talking_globally = false;
                self.sync_talking();
            }
            self.global_keys.set(wanted.clone());
            self.call_keys_told = wanted;
        }
    }

    /// Where the keys of a call come from now.
    pub fn global_keys_state(&self) -> KeysState {
        self.global_keys.state()
    }

    /// The calls send the microphone with push to talk while a window or the source of the
    /// system holds the key. A press and a release in a call with push to talk play their
    /// sounds (ADR 0707).
    pub(super) fn sync_talking(&mut self) {
        let before = self.calls.talking();
        let now = !self.talking_windows.is_empty() || self.talking_globally;
        self.calls.set_talking(now);
        if before != now
            && self.known.media.calls.call.is_some()
            && self.known.media.voice.input_mode == InputMode::PushToTalk
        {
            let output = self.heard.output.clone();
            self.play_sound(
                if now {
                    junto_media::audio::sounds::Sound::TalkStarts
                } else {
                    junto_media::audio::sounds::Sound::TalkEnds
                },
                output.as_ref(),
            );
        }
    }

    pub fn keybinds(&self) -> &Keybinds {
        &self.keybinds
    }

    /// Changes the key bindings, uses them at once and writes the change to the settings
    /// file on a background thread. Only this change is written, so that a second program
    /// that changed another binding keeps it.
    pub fn change_keybinds(
        &mut self,
        change: KeybindChange,
        cx: &mut Context<Self>,
    ) -> Result<(), KeybindError> {
        change.apply(&mut self.keybinds)?;
        let store = self.client.settings();
        Self::queue_write(
            cx,
            move || store.update_keybinds(|stored| drop(change.apply(stored))),
            |this, result, cx| {
                this.keybinds_error = result
                    .err()
                    .map(|error| text::flow_error(&FlowError::Storage(error.to_string())));
                cx.notify();
            },
        );
        self.changed(cx);
        Ok(())
    }

    /// Why the last change of the key bindings was not written.
    pub fn keybinds_error(&self) -> Option<&str> {
        self.keybinds_error.as_deref()
    }
}

/// A change of the key bindings (ADR 0777).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeybindChange {
    Set(KeyAction, Option<KeyCombo>),
    Outside(KeyAction, bool),
    Reset(KeyAction),
    ResetAll,
}

impl KeybindChange {
    fn apply(&self, keybinds: &mut Keybinds) -> Result<(), KeybindError> {
        match self {
            KeybindChange::Set(action, keys) => keybinds.set(*action, keys.clone()),
            KeybindChange::Outside(action, outside) => keybinds.set_outside(*action, *outside),
            KeybindChange::Reset(action) => {
                keybinds.reset(*action);
                Ok(())
            }
            KeybindChange::ResetAll => {
                keybinds.reset_all();
                Ok(())
            }
        }
    }
}
