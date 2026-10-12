use super::*;
use junto_client_core::call_window::{CallWindowSettings, CallWindowStore};
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
    pub fn closed_call_window(&mut self, cx: &mut Context<Self>) {
        self.call_window = None;
        self.talking_windows.clear();
        self.calls.set_talking(false);
        cx.notify();
    }
    pub fn return_call(&mut self, cx: &mut Context<Self>) {
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
        Self::queue_write(
            cx,
            move || store.save(settings),
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
    pub fn call_view(&self) -> ShellView {
        let mut selection = self.selection.clone();
        if let Some(call) = &self.media_calls().call {
            selection.server = Some(call.target.server_id);
            if let CallPlace::Channel {
                workspace_id,
                channel_id,
            } = call.target.place
            {
                selection
                    .workspaces
                    .insert(call.target.server_id, workspace_id);
                selection.channels.insert(workspace_id, channel_id);
                selection.conversations.remove(&call.target.server_id);
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
        view::derive(&self.snapshots(), &selection, &known)
    }
}
