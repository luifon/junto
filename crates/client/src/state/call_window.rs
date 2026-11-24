use super::*;
mod writes;
use junto_client_core::call_window::CallWindowSettings;
use junto_client_core::window_placement::{WindowPlacement, WindowStore};
pub(super) use writes::Writes;
/// What the panel of the call window shows over its chat, with the server and the channel of
/// the call that it belongs to (ADR 0741).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallWindowPanel {
    pub server_id: ServerId,
    pub channel_id: ChannelId,
    pub shows: CallWindowShows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallWindowShows {
    Thread(MessageId),
    Pins,
    Transcript(RecordingId),
}

impl AppState {
    /// Opens a thread, the pin list or a transcript of the channel of the call in the panel
    /// of the call window, over its chat. The main window keeps what it shows.
    pub fn open_in_call_window(&mut self, shows: CallWindowShows, cx: &mut Context<Self>) {
        let Some(call) = self.media_calls().call else {
            return;
        };
        let CallPlace::Channel { channel_id, .. } = call.target.place else {
            return;
        };
        self.call_window_panel = Some(CallWindowPanel {
            server_id: call.target.server_id,
            channel_id,
            shows,
        });
        if let CallWindowShows::Transcript(_) = shows {
            self.known.transcript_line = None;
        }
        self.changed(cx);
    }

    /// Closes the thread, the pin list or the transcript of the panel of the call window.
    pub fn close_in_call_window(&mut self, cx: &mut Context<Self>) {
        if self.call_window_panel.take().is_some() {
            self.changed(cx);
        }
    }

    /// The panel of the call window, while the call window shows the call of its channel.
    pub(super) fn call_window_panel(&self) -> Option<CallWindowPanel> {
        self.call_window.as_ref()?;
        let call = self.media_calls().call?;
        let panel = self.call_window_panel?;
        match call.target.place {
            CallPlace::Channel { channel_id, .. }
                if call.target.server_id == panel.server_id && channel_id == panel.channel_id =>
            {
                Some(panel)
            }
            _ => None,
        }
    }

    /// Drops the panel of the call window when the call window closed or the call moved to
    /// another place, and holds its pin list open.
    pub(super) fn follow_call_window_panel(&mut self, cx: &mut Context<Self>) {
        if self.call_window_panel.is_some() && self.call_window_panel().is_none() {
            self.call_window_panel = None;
        }
        let wanted = self.call_window_panel().and_then(|panel| {
            (panel.shows == CallWindowShows::Pins).then_some((panel.server_id, panel.channel_id))
        });
        if self.open_call_window_pins.as_ref().map(|(key, _)| *key) == wanted {
            return;
        }
        if let Some(((server_id, channel_id), _)) = self.open_call_window_pins.take()
            && let Some(server) = self.client.server(server_id)
        {
            server.live().release_pins(channel_id);
        }
        let Some(key) = wanted else {
            return;
        };
        let Some(server) = self.client.server(key.0) else {
            return;
        };
        let opening = server.live().open_pins(key.1);
        let task = cx.spawn(async move |this, cx| {
            let result = opening.await;
            let _ = this.update(cx, |this, cx| {
                if matches!(
                    result,
                    Err(junto_client_core::live::LoadError::Off
                        | junto_client_core::live::LoadError::NotInState)
                ) && this
                    .open_call_window_pins
                    .as_ref()
                    .is_some_and(|(held, _)| *held == key)
                {
                    this.open_call_window_pins = None;
                }
                cx.notify();
            });
        });
        self.open_call_window_pins = Some((key, task));
    }

    /// The place of the thread of the panel of the call window, which the window reads.
    pub(super) fn call_window_thread_place(&self) -> Option<(ServerId, PlaceId)> {
        let panel = self.call_window_panel()?;
        match panel.shows {
            CallWindowShows::Thread(root_id) => Some((
                panel.server_id,
                PlaceId::Thread {
                    channel_id: panel.channel_id,
                    root_id,
                },
            )),
            _ => None,
        }
    }

    /// The key of push-to-talk is held in a window or released. A press and a release in
    /// a call with push-to-talk play their sounds (ADR 0707).
    pub fn window_talking(&mut self, window: gpui::EntityId, talking: bool) {
        let talking = talking && !self.recording_keybind();
        if talking {
            self.talking_windows.insert(window);
        } else {
            self.talking_windows.remove(&window);
        }
        self.sync_talking();
    }
    /// The call window, while it is open.
    pub fn call_window(&self) -> Option<gpui::AnyWindowHandle> {
        self.call_window.map(Into::into)
    }

    /// Shows the notices of the server of the call in the panel of the call window, or
    /// closes them there.
    pub fn show_call_window_notices(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.call_window_notices != shown {
            self.call_window_notices = shown;
            self.changed(cx);
        }
    }

    pub fn call_window_settings(&self) -> CallWindowSettings {
        self.call_window_settings
    }
    pub fn opened_call_window(
        &mut self,
        window: gpui::WindowHandle<gpui_base::Root>,
        cx: &mut Context<Self>,
    ) {
        self.call_window_active = window
            .update(cx, |_, window, _| window.is_window_active())
            .unwrap_or(false);
        self.call_window = Some(window);
        self.known.call_in_own_window = true;
        // The call window reads the messages of the place of the call.
        self.changed(cx);
    }
    /// Sets what the call window does before it closes, whatever closes it.
    pub fn on_call_window_closing(&mut self, closing: impl FnOnce(&mut gpui::App) + 'static) {
        self.call_window_closing = Some(Box::new(closing));
    }

    fn call_window_closes(&mut self, cx: &mut Context<Self>) {
        if let Some(closing) = self.call_window_closing.take() {
            closing(cx);
        }
    }

    /// The system closed the call window.
    pub fn closed_call_window(&mut self, cx: &mut Context<Self>) {
        self.call_window_closes(cx);
        self.call_window_closed(cx);
    }

    /// The call window closed itself, after it gave away the text of its composer.
    pub fn call_window_returned(&mut self, cx: &mut Context<Self>) {
        self.call_window_closing = None;
        self.call_window_closed(cx);
    }

    fn call_window_closed(&mut self, cx: &mut Context<Self>) {
        self.call_window = None;
        self.call_window_active = false;
        self.call_window_notices = false;
        self.call_window_panel = None;
        self.known.call_in_own_window = false;
        self.talking_windows.clear();
        self.calls.set_talking(false);
        // The messages of the place of the call are released when the main window does
        // not show that place.
        self.changed(cx);
    }
    pub fn return_call(&mut self, cx: &mut Context<Self>) {
        self.call_window_closes(cx);
        if let Some(window) = self.call_window.take() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
        self.call_window_closed(cx);
    }
    pub fn main_window_settings(&self) -> WindowPlacement {
        self.main_window_settings
    }
    pub fn save_main_window(&mut self, settings: WindowPlacement, cx: &mut Context<Self>) {
        if self.main_window_settings == settings {
            return;
        }
        self.main_window_settings = settings;
        self.save_window_placement(true, settings, cx);
    }
    pub fn save_call_window(&mut self, settings: CallWindowSettings, cx: &mut Context<Self>) {
        if self.call_window_settings == settings {
            return;
        }
        self.call_window_settings = settings;
        self.save_window_placement(false, settings, cx);
    }
    fn save_window_placement(
        &mut self,
        main: bool,
        settings: CallWindowSettings,
        cx: &mut Context<Self>,
    ) {
        let directory = self.client.data_dir().clone();
        let store = if main {
            WindowStore::main_window(directory)
        } else {
            WindowStore::new(directory)
        };
        let writes = if main {
            self.main_window_writes.clone()
        } else {
            self.call_window_writes.clone()
        };
        if !writes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(settings)
        {
            return;
        }
        Self::queue_write(
            cx,
            move || {
                let mut result = Ok(());
                loop {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    let next = writes
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take();
                    let Some(next) = next else {
                        return result;
                    };
                    if let Err(error) = store.save(next) {
                        result = Err(error);
                    }
                }
            },
            move |_, result, _| {
                if let Err(error) = result {
                    tracing::warn!(%error, main_window = main, "the window placement was not stored");
                }
            },
        );
    }
    pub fn set_call_on_top(&mut self, top: bool, cx: &mut Context<Self>) {
        let mut settings = self.call_window_settings;
        settings.keep_on_top = top;
        self.save_call_window(settings, cx);
    }
    /// True while the window reads the messages of a place: for the main window, for its
    /// side panel or for the call window.
    #[cfg(feature = "test-support")]
    pub fn reads_messages(&self, server_id: ServerId, place: PlaceId) -> bool {
        self.open_places.contains_key(&(server_id, place))
    }

    /// The place whose messages the call window shows while it is open: the voice channel of
    /// the call, or the conversation of a direct-message call.
    pub(super) fn call_window_place(
        &self,
        snapshots: &[ServerSnapshot],
    ) -> Option<(ServerId, PlaceId)> {
        self.call_window.as_ref()?;
        let call = self.media_calls().call?;
        let server_id = call.target.server_id;
        let place = match call.target.place {
            CallPlace::Group { conversation_id } => PlaceId::Direct(conversation_id),
            CallPlace::Channel { channel_id, .. } => PlaceId::Channel(channel_id),
            CallPlace::Direct {
                low_account_id,
                high_account_id,
            } => PlaceId::Direct(direct_conversation(
                snapshots,
                server_id,
                [low_account_id, high_account_id],
            )?),
        };
        Some((server_id, place))
    }

    /// The view of the call window: the place of the call, whatever the main window shows.
    /// The chat of a direct-message call is the conversation of its two accounts.
    pub fn call_view(&self) -> ShellView {
        let mut selection = self.selection.clone();
        // The panel of the call window shows the notices of the server of the call when
        // the person opened them there, whatever the main window shows (ADR 0742).
        selection.notices = None;
        // Its thread, pin list and transcript are its own (ADR 0741).
        selection.thread = None;
        selection.pins = None;
        selection.transcript = None;
        if let Some(panel) = self.call_window_panel() {
            let (server_id, channel_id) = (panel.server_id, panel.channel_id);
            match panel.shows {
                CallWindowShows::Thread(root_id) => {
                    selection.thread = Some((server_id, channel_id, root_id));
                }
                CallWindowShows::Pins => selection.pins = Some((server_id, channel_id)),
                CallWindowShows::Transcript(recording_id) => {
                    selection.transcript = Some((server_id, channel_id, recording_id));
                }
            }
        }
        let snapshots = self.snapshots();
        if let Some(call) = &self.media_calls().call {
            let server_id = call.target.server_id;
            selection.server = Some(server_id);
            if self.call_window_notices {
                selection.notices = Some(server_id);
            }
            match call.target.place {
                CallPlace::Group { conversation_id } => {
                    selection.conversations.insert(server_id, conversation_id);
                }
                CallPlace::Channel {
                    workspace_id,
                    channel_id,
                } => {
                    selection.workspaces.insert(server_id, workspace_id);
                    selection
                        .channels
                        .insert((server_id, workspace_id), channel_id);
                    selection.conversations.remove(&server_id);
                }
                CallPlace::Direct {
                    low_account_id,
                    high_account_id,
                } => {
                    let accounts = [low_account_id, high_account_id];
                    match direct_conversation(&snapshots, server_id, accounts) {
                        Some(id) => selection.conversations.insert(server_id, id),
                        None => selection.conversations.remove(&server_id),
                    };
                }
            }
        }
        // The call window draws the call in every kind of place.
        let known = Known {
            call_in_own_window: false,
            ..self.known.clone()
        };
        #[cfg(feature = "test-support")]
        let known = if let Some(media) = &self.media_fixture {
            Known {
                media: media.clone(),
                ..known
            }
        } else {
            known
        };
        self.direct
            .filter_view(view::derive(&snapshots, &selection, &known))
    }
}

/// The conversation of the two accounts of a direct-message call, when the state of its
/// server holds it.
fn direct_conversation(
    snapshots: &[ServerSnapshot],
    server_id: ServerId,
    accounts: [AccountId; 2],
) -> Option<junto_client_core::protocol::ids::ConversationId> {
    snapshots
        .iter()
        .find(|snapshot| snapshot.id == server_id)
        .and_then(view::shown_model)
        .and_then(|model| {
            model.conversations.iter().find(|conversation| {
                accounts.contains(&conversation.low_account_id)
                    && accounts.contains(&conversation.high_account_id)
            })
        })
        .map(|conversation| conversation.id)
}

pub(super) fn call_transition(
    before: Option<&junto_media::calls::ActiveCall>,
    after: Option<&junto_media::calls::ActiveCall>,
) -> (bool, bool) {
    let before = before.map(|call| call.target);
    let after = after.map(|call| call.target);
    (
        before.is_some() && after.is_none(),
        after.is_some() && before != after,
    )
}
