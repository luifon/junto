use gpui::Context;
use junto_client_core::address::ServerAddress;
use junto_client_core::links::{self, AppLink};
use junto_client_core::protocol::ids::{MessageId, ServerId};

use super::{AppState, notices::Jump};
use crate::text;

impl AppState {
    pub fn message_link(&self, server_id: ServerId, id: MessageId) -> Option<String> {
        let server = self.client.server(server_id)?;
        Some(links::format(&AppLink::Message {
            server: server.entry().address,
            id,
        }))
    }

    pub fn message_link_refusal(&self) -> Option<&'static str> {
        (self.message_link_failed == Some(self.navigations))
            .then_some(text::MESSAGE_LINK_UNAVAILABLE)
    }

    pub fn dismiss_message_link_refusal(&mut self, cx: &mut Context<Self>) {
        self.message_link_failed = None;
        self.changed(cx);
    }

    /// False asks the URL router to open the add-server form with this origin.
    pub fn open_message_link(
        &mut self,
        address: &ServerAddress,
        id: MessageId,
        cx: &mut Context<Self>,
    ) -> bool {
        self.message_link_asks = self.message_link_asks.wrapping_add(1);
        let number = self.message_link_asks;
        self.message_link_failed = None;
        let Some(server) = self
            .client
            .servers()
            .into_iter()
            .find(|server| server.entry().address == *address)
        else {
            return false;
        };
        let Some(owner) = server.owner() else {
            self.message_link_failed = Some(self.navigations);
            self.changed(cx);
            return true;
        };
        let server_id = server.id();
        let navigation = self.navigations;
        let api = server.api_for(owner.account_id);
        cx.spawn(async move |this, cx| {
            let read = api.message(id).await;
            let _ = this.update(cx, |this, cx| {
                if this.message_link_asks != number
                    || this.navigations != navigation
                    || this
                        .client
                        .server(server_id)
                        .and_then(|server| server.owner())
                        != Some(owner)
                {
                    return;
                }
                match read {
                    Ok(message) if message.id == id && !message.deleted => {
                        this.go_to(Jump::to(server_id, &message), cx);
                    }
                    _ => {
                        this.message_link_failed = Some(this.navigations);
                        this.changed(cx);
                    }
                }
            });
        })
        .detach();
        self.changed(cx);
        true
    }
}
