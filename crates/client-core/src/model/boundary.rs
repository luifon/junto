//! The messages of a channel whose history the account may not read (ADR 0304). The server
//! gives such an account the messages that were created during its current connection to
//! the event stream, and no page of the history. The state notes the connection of each
//! read that returned such a message. When the server tells that older messages of the
//! channel changed, or the account loses the history of a channel, the state keeps only
//! the messages that a read of the current connection returned.

use std::collections::HashMap;
use std::sync::Arc;

use junto_protocol::WorkspacePermission;
use junto_protocol::ids::{ChannelId, MessageId};
use junto_protocol::message::{Message, MessagePlace};

use super::{Named, PlaceId, ServerModel};

/// Above this number of notes, the notes of messages that the state no longer holds are
/// dropped.
const MOST_NOTES: usize = 1024;

fn of_connection(
    received_in: &HashMap<MessageId, u64>,
    connection: Option<u64>,
    message_id: MessageId,
) -> bool {
    connection.is_some() && received_in.get(&message_id).copied() == connection
}

impl ServerModel {
    /// Whether the account may read the history of a channel, as far as the state knows.
    fn reads_history(&self, channel_id: ChannelId) -> Option<bool> {
        self.permissions_in(channel_id)
            .map(|held| held.contains(&WorkspacePermission::ReadMessageHistory))
    }

    /// True for a channel whose history the account may not read, as far as the state
    /// knows. Its timeline holds the messages that the connections brought, and no page.
    pub(crate) fn reads_live_only(&self, place: PlaceId) -> bool {
        matches!(place, PlaceId::Channel(channel_id) if self.reads_history(channel_id) == Some(false))
    }

    /// Notes that a read of one message that was sent during `connection` returned
    /// `message`. Only a message whose history the account may not read is noted.
    pub(crate) fn note_received(&mut self, message: &Message, connection: Option<u64>) {
        let (Some(connection), MessagePlace::Channel { channel_id, .. }) =
            (connection, message.place)
        else {
            return;
        };
        if self.reads_history(channel_id) != Some(false) {
            return;
        }
        self.received_in.insert(message.id, connection);
        if self.received_in.len() > MOST_NOTES {
            self.prune_notes();
        }
    }

    fn holds_message(&self, message_id: MessageId) -> bool {
        self.timelines.iter().any(|timeline| {
            timeline.message(message_id).is_some()
                || timeline
                    .root
                    .as_ref()
                    .is_some_and(|root| root.id == message_id)
        }) || self.pins.iter().any(|list| {
            list.messages
                .loaded()
                .is_some_and(|pins| pins.iter().any(|pin| pin.id == message_id))
        }) || self.named(message_id).is_some()
            || self.withheld.iter().any(|held| held.id == message_id)
    }

    fn prune_notes(&mut self) {
        let notes = std::mem::take(&mut self.received_in);
        self.received_in = notes
            .into_iter()
            .filter(|(message_id, _)| self.holds_message(*message_id))
            .collect();
    }

    /// Before a withdrawal of the state empties the timelines of `channels`, the messages
    /// of the current connection are put aside: the timeline of a channel whose history
    /// the account cannot read takes them again after the reload.
    pub(super) fn withhold(&mut self, channels: &[ChannelId]) {
        let received_in = &self.received_in;
        let connection = self.connection;
        for timeline in &self.timelines {
            if !matches!(timeline.place, PlaceId::Channel(channel_id) if channels.contains(&channel_id))
            {
                continue;
            }
            for message in &timeline.messages {
                if of_connection(received_in, connection, message.id)
                    && !self.withheld.iter().any(|held| held.id == message.id)
                {
                    self.withheld.push(message.clone());
                }
            }
        }
    }

    /// A reader opens a channel whose history the account may not read. Its timeline
    /// keeps the messages that it holds and that a read without that permission returned,
    /// and takes again the messages of the current connection that a withdrawal put
    /// aside.
    pub(crate) fn open_live(&mut self, place: PlaceId) -> bool {
        let (back, other): (Vec<Arc<Message>>, Vec<Arc<Message>>) =
            std::mem::take(&mut self.withheld)
                .into_iter()
                .partition(|held| PlaceId::of_message(held) == place);
        self.withheld = other;
        let received_in = &self.received_in;
        let connection = self.connection;
        let Some(timeline) = self
            .timelines
            .iter_mut()
            .find(|timeline| timeline.place == place)
            .map(Arc::make_mut)
        else {
            return false;
        };
        let mut messages: Vec<Message> = timeline
            .messages
            .iter()
            .filter(|held| received_in.contains_key(&held.id))
            .map(|held| held.as_ref().clone())
            .collect();
        messages.extend(
            back.iter()
                .filter(|held| of_connection(received_in, connection, held.id))
                .map(|held| held.as_ref().clone()),
        );
        timeline.replace(messages, false, false);
        true
    }

    /// A page of the history replaced the messages of a place: what a withdrawal put aside
    /// for it is not needed any more.
    pub(crate) fn drop_withheld(&mut self, place: PlaceId) {
        self.withheld
            .retain(|held| PlaceId::of_message(held) != place);
    }

    /// The server told that messages of the channel that are older than the current
    /// connection changed (ADR 0304). Every holder of messages of the channel keeps only
    /// the messages that a read of this connection returned. Returns the named messages
    /// that are read again, and the timelines that read their history again because the
    /// state says that the account may read it.
    pub(crate) fn withdraw_history(
        &mut self,
        channel_id: ChannelId,
    ) -> (Vec<MessageId>, Vec<PlaceId>) {
        let place = PlaceId::Channel(channel_id);
        let reads_history = self.reads_history(channel_id) == Some(true);
        let received_in = &self.received_in;
        let connection = self.connection;
        let keep = |message: &Arc<Message>| of_connection(received_in, connection, message.id);
        let mut reread = Vec::new();
        for timeline in &mut self.timelines {
            if timeline.place.container() != place {
                continue;
            }
            let older = timeline.messages.iter().any(|held| !keep(held))
                || timeline.root.as_ref().is_some_and(|root| !keep(root));
            if !older {
                continue;
            }
            let timeline = Arc::make_mut(timeline);
            timeline.messages.retain(|held| keep(held));
            if timeline.root.as_ref().is_some_and(|root| !keep(root)) {
                timeline.root = None;
            }
            if reads_history {
                timeline.stale = true;
                reread.push(timeline.place);
            }
        }
        for list in &mut self.pins {
            if list.channel_id == channel_id
                && let Some(pins) = list.messages.loaded_mut()
            {
                pins.retain(|held| keep(held));
            }
        }
        self.withheld
            .retain(|held| PlaceId::of_message(held).container() != place || keep(held));
        let mut named = Vec::new();
        self.named.retain(|(message_id, held)| match held {
            Named::Read(message) if PlaceId::of(&message.place) == place && !keep(message) => {
                named.push(*message_id);
                false
            }
            _ => true,
        });
        self.prune_notes();
        (named, reread)
    }
}
