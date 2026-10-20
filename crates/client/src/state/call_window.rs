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
    pub fn call_window_settings(&self) -> CallWindowSettings {
        self.call_window_settings
    }
    pub fn opened_call_window(
        &mut self,
        window: gpui::WindowHandle<gpui_base::Root>,
        cx: &mut Context<Self>,
    ) {
        self.call_window = Some(window);
        cx.notify();
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
        self.talking_windows.clear();
        self.calls.set_talking(false);
        cx.notify();
    }
    pub fn return_call(&mut self, cx: &mut Context<Self>) {
        self.call_window_closes(cx);
        if let Some(window) = self.call_window.take() {
            let _ = window.update(cx, |_, window, _| window.remove_window());
        }
        self.talking_windows.clear();
        self.calls.set_talking(false);
        cx.notify();
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
    /// The view of the call window: the place of the call, whatever the main window shows.
    /// The chat of a direct-message call is the conversation of its two accounts.
    pub fn call_view(&self) -> ShellView {
        let mut selection = self.selection.clone();
        let snapshots = self.snapshots();
        if let Some(call) = &self.media_calls().call {
            let server_id = call.target.server_id;
            selection.server = Some(server_id);
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
                    let conversation = snapshots
                        .iter()
                        .find(|snapshot| snapshot.id == server_id)
                        .and_then(view::shown_model)
                        .and_then(|model| {
                            model.conversations.iter().find(|conversation| {
                                accounts.contains(&conversation.low_account_id)
                                    && accounts.contains(&conversation.high_account_id)
                            })
                        })
                        .map(|conversation| conversation.id);
                    match conversation {
                        Some(id) => selection.conversations.insert(server_id, id),
                        None => selection.conversations.remove(&server_id),
                    };
                }
            }
        }
        let known = self.known.clone();
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
