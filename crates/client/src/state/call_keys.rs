//! The keys of a call in every application (ADR 0776): the state gives the source of the
//! keys the keys that the call needs, and does what each key does. A window that is in front
//! gives the key of push to talk and the key that mutes too, so that they work where the
//! system has no source.

use futures_util::StreamExt as _;
use gpui::Context;
use junto_client_core::global_keys::{CallKeys, GlobalKeys, KeyCombo, KeyEvent, KeysState};
use junto_client_core::voice::InputMode;

use super::AppState;

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
        self.follow_call_keys();
        cx.notify();
    }

    fn global_key(&mut self, event: KeyEvent, cx: &mut Context<Self>) {
        match event {
            KeyEvent::Talk(held) => {
                self.talking_globally = held;
                self.sync_talking();
            }
            KeyEvent::ToggleMute => self.toggle_mute(),
        }
        cx.notify();
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

    /// The window that is in front gives the key that mutes while no source of the system
    /// gives it: a window would toggle a second time what the source toggled.
    pub fn window_mute_key(&mut self, pressed: &KeyCombo) {
        let source = matches!(
            self.global_keys.state(),
            KeysState::Reading | KeysState::Portal
        );
        if !source && self.voice().mute_key.as_ref() == Some(pressed) {
            self.toggle_mute();
        }
    }

    /// The keys that the call needs: the key of push to talk while push to talk is on, and
    /// the key that mutes. None outside a call.
    fn call_keys(&self) -> Option<CallKeys> {
        self.known.media.calls.call.as_ref()?;
        let voice = self.voice();
        let keys = CallKeys {
            talk: (voice.input_mode == InputMode::PushToTalk).then(|| voice.talk_key.combo()),
            mute: voice.mute_key.clone(),
        };
        (keys.talk.is_some() || keys.mute.is_some()).then_some(keys)
    }

    /// Tells the source the keys that the call needs now, when they changed.
    pub(super) fn follow_call_keys(&mut self) {
        let wanted = self.call_keys();
        if self.call_keys_told != wanted {
            if wanted.is_none() {
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

    /// Changes the key that mutes. Absent: no key.
    pub fn set_mute_key(&mut self, key: Option<KeyCombo>, cx: &mut Context<Self>) {
        self.change_voice(cx, move |voice| voice.mute_key = key.clone());
        self.follow_call_keys();
    }

    /// The calls send the microphone with push to talk while a window or the source of the
    /// system holds the key.
    pub(super) fn sync_talking(&mut self) {
        self.calls
            .set_talking(!self.talking_windows.is_empty() || self.talking_globally);
    }
}
