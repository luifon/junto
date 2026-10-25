use super::*;
mod writes;
use junto_client_core::call_window::{CallWindowSettings, CallWindowStore};
pub(super) use writes::Writes;
impl AppState {
    pub fn window_talking(&mut self, window: gpui::EntityId, talking: bool) {
        if talking {
            self.talking_windows.insert(window);
        } else {
            self.talking_windows.remove(&window);
        }
        self.calls.set_talking(!self.talking_windows.is_empty());
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
    pub fn save_call_window(&mut self, settings: CallWindowSettings, cx: &mut Context<Self>) {
        self.call_window_settings = settings;
        let store = CallWindowStore::new(self.client.data_dir().clone());
        let writes = self.call_window_writes.clone();
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
            |_, result, _| {
                if let Err(error) = result {
                    tracing::warn!(%error, "the call window settings were not stored");
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
        let snapshots = self.snapshots();
        if let Some(call) = &self.media_calls().call {
            let server_id = call.target.server_id;
            selection.server = Some(server_id);
            if self.call_window_notices {
                selection.notices = Some(server_id);
            }
            match call.target.place {
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
