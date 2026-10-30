use gpui::{Context, Window};
use junto_client_core::links::{self, AppLink};

use super::{Layer, Shell};

impl Shell {
    pub fn open_url(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(link) = links::parse(url) else {
            return false;
        };
        self.open_app_link(link, window, cx);
        true
    }

    pub fn open_app_link(&mut self, link: AppLink, window: &mut Window, cx: &mut Context<Self>) {
        self.adding_number = self.adding_number.wrapping_add(1);
        self.adding = None;
        self.pending_link = None;
        if self.state.read(cx).preset_name().is_empty() {
            self.pending_link = Some(link);
            return;
        }
        if !matches!(self.layer, Layer::None) {
            self.close_layer(window, cx);
        }
        match link {
            AppLink::Message { server, id } => {
                let opened = self
                    .state
                    .update(cx, |state, cx| state.open_message_link(&server, id, cx));
                if !opened {
                    self.prefill_link_server(server.as_str(), window, cx);
                }
            }
            AppLink::Thread { server, id } => {
                let opened = self
                    .state
                    .update(cx, |state, cx| state.open_thread_link(&server, id, cx));
                if !opened {
                    self.prefill_link_server(server.as_str(), window, cx);
                }
            }
            AppLink::Channel { server, id } => {
                let opened = self
                    .state
                    .update(cx, |state, cx| state.open_channel_link(&server, id, cx));
                if !opened {
                    self.prefill_link_server(server.as_str(), window, cx);
                }
            }
            AppLink::Conversation { server, id } => {
                let opened = self.state.update(cx, |state, cx| {
                    state.open_conversation_link(&server, id, cx)
                });
                if !opened {
                    self.prefill_link_server(server.as_str(), window, cx);
                }
            }
            AppLink::File { server, id } => {
                let opened = self
                    .state
                    .update(cx, |state, cx| state.open_file_link(&server, id, cx));
                if !opened {
                    self.prefill_link_server(server.as_str(), window, cx);
                }
            }
            AppLink::Recording { server, id } => {
                let opened = self
                    .state
                    .update(cx, |state, cx| state.open_recording_link(&server, id, cx));
                if !opened {
                    self.prefill_link_server(server.as_str(), window, cx);
                }
            }
            AppLink::Invite { server, code } => {
                let saved = self
                    .state
                    .read(cx)
                    .client()
                    .servers()
                    .into_iter()
                    .find(|saved| saved.entry().address == server);
                if let Some(saved) = saved {
                    self.state
                        .update(cx, |state, cx| state.select_server(saved.id(), cx));
                    self.open_join_for(saved.id(), window, cx);
                    self.type_code(code.expose(), window, cx);
                } else {
                    self.prefill_link_server(server.as_str(), window, cx);
                    self.pending_link = Some(AppLink::Invite { server, code });
                    self.submit_address(window, cx);
                }
            }
        }
        cx.notify();
    }

    fn prefill_link_server(&mut self, address: &str, window: &mut Window, cx: &mut Context<Self>) {
        let narrow = super::logical_width(window) < crate::theme::theme(cx).metrics.narrow_below;
        self.show_address_field(narrow, window, cx);
        self.type_address(address, window, cx);
        self.focus_address(window, cx);
    }
}
