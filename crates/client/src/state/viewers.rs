//! The members of a workspace who can see a channel, for the member list of the channel
//! (ADR 0790). The client cannot compute them: it holds only the permissions of its own
//! account, so the server lists them.

use std::collections::HashSet;
use std::sync::Arc;

use gpui::Context;
use junto_client_core::protocol::ids::{ChannelId, ServerId};

use super::AppState;
use super::view::Viewers;

impl AppState {
    /// Reads the members who can see a channel. What was read before stays shown until the
    /// answer; one read of a channel is on its way at a time.
    pub fn read_channel_viewers(
        &mut self,
        server_id: ServerId,
        channel_id: ChannelId,
        cx: &mut Context<Self>,
    ) {
        let key = (server_id, channel_id);
        if !self.viewers_asked.insert(key) {
            return;
        }
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let Some(owner) = server.owner() else {
            return;
        };
        self.known.viewers.entry(key).or_insert(Viewers::Reading);
        let api = server.api_for(owner.account_id);
        cx.spawn(async move |this, cx| {
            let read = api.channel_viewers(channel_id).await;
            let _ = this.update(cx, |this, cx| {
                this.viewers_asked.remove(&key);
                if this.owner(server_id) != Some(owner) {
                    return;
                }
                let viewers = match read {
                    Ok(accounts) => Viewers::Read(Arc::new(accounts.into_iter().collect::<HashSet<_>>())),
                    Err(_) => Viewers::Failed,
                };
                this.known.viewers.insert(key, viewers);
                this.changed(cx);
            });
        })
        .detach();
    }

    /// Something changed on a server after which other people can see other channels: what
    /// was read is read again when the member list of a channel asks for it.
    pub(super) fn forget_channel_viewers(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        let before = self.known.viewers.len();
        self.known.viewers.retain(|(server, _), _| *server != server_id);
        if self.known.viewers.len() != before {
            self.changed(cx);
        }
    }
}
