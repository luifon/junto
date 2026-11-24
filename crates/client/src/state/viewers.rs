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
    fn viewers_generation(&self, server_id: ServerId) -> u64 {
        self.known
            .viewers_generation
            .get(&server_id)
            .copied()
            .unwrap_or_default()
    }

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
        let generation = self.viewers_generation(server_id);
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
                // A change that arrived while the read ran keeps the list marked old, so
                // that it is read again.
                if this.viewers_generation(server_id) == generation {
                    this.known.viewers_stale.remove(&key);
                } else {
                    this.known.viewers_stale.insert(key);
                }
                this.known.viewers.insert(key, viewers);
                this.changed(cx);
            });
        })
        .detach();
    }

    /// The member list of the shown channel opens: its list stays shown and is read again,
    /// because the server tells a member of a rule of a channel only when the member may
    /// manage the channel (ADR 0790).
    pub fn channel_viewers_shown(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown.server.zip(self.shown.channel);
        if let Some(key) = shown
            && self.known.viewers.contains_key(&key)
            && self.known.viewers_stale.insert(key)
        {
            self.changed(cx);
        }
    }

    /// Something changed on a server after which other people can see other channels: what
    /// was read stays shown and is read again when the member list of its channel is shown.
    pub(super) fn forget_channel_viewers(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        *self.known.viewers_generation.entry(server_id).or_default() += 1;
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
