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
                    Ok(accounts) => {
                        Viewers::Read(Arc::new(accounts.into_iter().collect::<HashSet<_>>()))
                    }
                    // A refused read after an earlier answer keeps that answer shown.
                    Err(_) => match this.known.viewers.get(&key) {
                        Some(Viewers::Read(held)) => Viewers::Read(held.clone()),
                        _ => Viewers::Failed,
                    },
                };
                this.known.viewers_stale.remove(&key);
                this.known.viewers.insert(key, viewers);
                this.changed(cx);
            });
        })
        .detach();
    }

    /// Something changed on a server after which other people can see other channels: what
    /// was read stays shown and is read again when the member list of its channel is shown.
    pub(super) fn forget_channel_viewers(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        let held: Vec<_> = self
            .known
            .viewers
            .keys()
            .filter(|(server, _)| *server == server_id)
            .copied()
            .collect();
        let before = self.known.viewers_stale.len();
        self.known.viewers_stale.extend(held);
        if self.known.viewers_stale.len() != before {
            self.changed(cx);
        }
    }
}
