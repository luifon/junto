//! The messages in the state of a server (ADR 0721): the unread summary of each place, the
//! messages of the places that a reader has open, and the messages that wait to be sent.

use std::sync::Arc;

use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{AccountId, AttachmentId, ChannelId, ConversationId, MessageId};
use junto_protocol::message::{
    Conversation, Document, Message, MessagePage, MessagePlace, MessageState,
};
use time::OffsetDateTime;

use super::ServerModel;
use crate::api::{ApiError, ErrorCategory};

/// A place that holds messages: a channel, or a direct conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PlaceId {
    Channel(ChannelId),
    Direct(ConversationId),
}

impl PlaceId {
    pub const fn of(place: &MessagePlace) -> Self {
        match place {
            MessagePlace::Channel { channel_id, .. } => Self::Channel(*channel_id),
            MessagePlace::Direct {
                conversation_id, ..
            } => Self::Direct(*conversation_id),
        }
    }
}

/// How many messages of one place the state holds at most. A page that passes the bound
/// drops messages at the other end, so a server cannot make the state grow without a bound
/// (ADR 0714).
pub const MOST_LOADED: usize = 10_000;

/// Why a read of messages got no messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFailure {
    /// The place or the message that the read started from does not exist for the account.
    NotFound,
    Refused(ErrorCode),
    /// No answer arrived, or one that this client cannot read.
    NoAnswer,
}

impl ReadFailure {
    pub(crate) fn of(error: &ApiError) -> Self {
        match error.rejection() {
            Some(rejection) if rejection.category() == ErrorCategory::NotFound => Self::NotFound,
            Some(rejection) => Self::Refused(rejection.code),
            None => Self::NoAnswer,
        }
    }
}

/// The reads of one place that are on their way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Reading {
    /// The first page, or the page that replaces the messages after a reload.
    pub open: bool,
    pub older: bool,
    pub newer: bool,
}

/// Which messages of a place a read asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Reach {
    /// The page that a place starts with, or starts again with after a reload.
    Open,
    /// The page before the first loaded message.
    Older,
    /// The page after the last loaded message.
    Newer,
    /// The latest page, in place of what is loaded.
    End,
}

/// The loaded messages of one place: one range of its history without a hole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub place: PlaceId,
    /// In the order of the server: by position and then by identifier.
    pub messages: Vec<Arc<Message>>,
    /// The history has messages before the first loaded one.
    pub more_before: bool,
    /// The history has messages after the last loaded one. While it is false, a new
    /// message of the place is added at the end.
    pub more_after: bool,
    pub reading: Reading,
    /// A first page has arrived.
    pub loaded: bool,
    /// Why the last read failed. The next read that succeeds clears it.
    pub failed: Option<ReadFailure>,
    /// The state was replaced by a snapshot, and the page that replaces these messages
    /// has not arrived yet. Changes of the time between can be missing.
    pub stale: bool,
    /// The read position of the account when the place was opened with unread messages.
    /// A reader draws the line of the first unread message after it.
    pub unread_after: Option<u64>,
}

fn key(message: &Message) -> (u64, MessageId) {
    (message.position, message.id)
}

impl Timeline {
    pub(crate) fn new(place: PlaceId) -> Self {
        Self {
            place,
            messages: Vec::new(),
            more_before: false,
            more_after: false,
            reading: Reading::default(),
            loaded: false,
            failed: None,
            stale: false,
            unread_after: None,
        }
    }

    pub fn message(&self, message_id: MessageId) -> Option<&Arc<Message>> {
        self.messages
            .iter()
            .find(|message| message.id == message_id)
    }

    /// True when a change of the message concerns what is loaded: the message is held, or
    /// it lies inside the loaded range, or it lies beyond an end where the history has no
    /// more messages.
    pub(crate) fn wants(&self, message_id: MessageId, position: u64) -> bool {
        if !self.loaded {
            return false;
        }
        if self.message(message_id).is_some() {
            return true;
        }
        let at = (position, message_id);
        match (self.messages.first(), self.messages.last()) {
            (Some(first), Some(last)) => {
                if at > key(last) {
                    !self.more_after
                } else if at < key(first) {
                    !self.more_before
                } else {
                    true
                }
            }
            _ => !self.more_after && !self.more_before,
        }
    }

    /// Puts a message at its place in the order, in place of the copy that is held.
    /// Nothing changes when the message lies outside what is loaded.
    pub(crate) fn put(&mut self, message: Message) -> bool {
        if !self.wants(message.id, message.position) {
            return false;
        }
        match self
            .messages
            .binary_search_by(|held| key(held).cmp(&key(&message)))
        {
            Ok(index) => {
                if *self.messages[index] == message {
                    return false;
                }
                self.messages[index] = Arc::new(message);
            }
            Err(index) => {
                // A message never changes its position; a copy under another position
                // would be a second row of the same message.
                self.messages.retain(|held| held.id != message.id);
                let index = index.min(self.messages.len());
                self.messages.insert(index, Arc::new(message));
                self.bound(index * 2 >= self.messages.len());
            }
        }
        true
    }

    pub(crate) fn remove(&mut self, message_id: MessageId) -> bool {
        let before = self.messages.len();
        self.messages.retain(|held| held.id != message_id);
        self.messages.len() != before
    }

    /// Replaces what is loaded by the messages of an open read.
    pub(crate) fn replace(&mut self, messages: Vec<Message>, more_before: bool, more_after: bool) {
        self.messages = sorted(messages);
        self.more_before = more_before;
        self.more_after = more_after;
        self.loaded = true;
        self.stale = false;
        self.failed = None;
        self.bound(true);
    }

    /// Adds the page before the first loaded message.
    pub(crate) fn add_older(&mut self, page: MessagePage) {
        self.more_before = page.has_more;
        self.failed = None;
        let first = self.messages.first().map(|first| key(first));
        let mut older: Vec<Arc<Message>> = sorted(page.messages)
            .into_iter()
            .filter(|message| first.is_none_or(|first| key(message) < first))
            .collect();
        older.append(&mut self.messages);
        self.messages = older;
        self.bound(false);
    }

    /// Adds the page after the last loaded message.
    pub(crate) fn add_newer(&mut self, page: MessagePage) {
        self.more_after = page.has_more;
        self.failed = None;
        let last = self.messages.last().map(|last| key(last));
        self.messages.extend(
            sorted(page.messages)
                .into_iter()
                .filter(|message| last.is_none_or(|last| key(message) > last)),
        );
        self.bound(true);
    }

    /// Keeps at most [`MOST_LOADED`] messages: the end that was not added to gives way.
    fn bound(&mut self, added_at_end: bool) {
        let Some(over) = self.messages.len().checked_sub(MOST_LOADED) else {
            return;
        };
        if over == 0 {
            return;
        }
        if added_at_end {
            self.messages.drain(..over);
            self.more_before = true;
        } else {
            self.messages.truncate(MOST_LOADED);
            self.more_after = true;
        }
    }
}

fn sorted(messages: Vec<Message>) -> Vec<Arc<Message>> {
    let mut messages: Vec<Arc<Message>> = messages.into_iter().map(Arc::new).collect();
    messages.sort_by_key(|message| key(message));
    messages.dedup_by_key(|message| message.id);
    messages
}

/// A file of a message that waits to be sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingFile {
    /// The upload of this client that carries the file.
    pub upload: u64,
    pub name: String,
    pub bytes: u64,
    /// Present when the upload is complete on the server.
    pub attachment_id: Option<AttachmentId>,
}

/// Why a message was not sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendFailure {
    /// The server refused the message.
    Refused { code: ErrorCode, status: u16 },
    /// No connection was made: the server did not receive the message.
    NotSent,
    /// A file of the message was not uploaded.
    File,
}

/// Where a message that the person sent stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendState {
    /// The files of the message are being uploaded.
    WaitingForFiles,
    Sending,
    /// The request was sent and no answer arrived: the server may hold the message. The
    /// message leaves this state when it arrives from the server, or when the person
    /// sends it again or discards it.
    Unconfirmed,
    Failed(SendFailure),
}

/// A message that the person sent and that the server has not confirmed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMessage {
    /// The number of the message on this client.
    pub id: u64,
    pub place: PlaceId,
    pub document: Document,
    pub reply_to: Option<MessageId>,
    pub files: Vec<PendingFile>,
    pub state: SendState,
    /// The time of this device at which the person sent the message.
    pub created_at: OffsetDateTime,
    /// The latest position of the place that the state held before the first attempt. A
    /// message of the server confirms this one only after that position.
    pub(crate) after: u64,
}

impl PendingMessage {
    /// A message that waits. It waits for its files when one of them has no attachment
    /// on the server yet.
    pub fn new(
        id: u64,
        place: PlaceId,
        document: Document,
        reply_to: Option<MessageId>,
        files: Vec<PendingFile>,
        created_at: OffsetDateTime,
    ) -> Self {
        let state = if files.iter().all(|file| file.attachment_id.is_some()) {
            SendState::Sending
        } else {
            SendState::WaitingForFiles
        };
        Self {
            id,
            place,
            document,
            reply_to,
            files,
            state,
            created_at,
            after: 0,
        }
    }

    /// The attachments in the order of the message, when every file is uploaded.
    pub(crate) fn attachment_ids(&self) -> Option<Vec<AttachmentId>> {
        self.files.iter().map(|file| file.attachment_id).collect()
    }

    /// True when `message` is this message as the server stored it: the same author,
    /// document, reply and files, after the position of the attempt. The server has no
    /// key that names one attempt (ADR 0408), so the content decides.
    pub(crate) fn is_confirmed_by(&self, message: &Message, own: AccountId) -> bool {
        if !matches!(self.state, SendState::Sending | SendState::Unconfirmed) {
            return false;
        }
        let Some(mut wanted) = self.attachment_ids() else {
            return false;
        };
        let mut held: Vec<AttachmentId> = message.attachments.iter().map(|file| file.id).collect();
        wanted.sort_unstable();
        held.sort_unstable();
        message.author_id == Some(own)
            && PlaceId::of(&message.place) == self.place
            && message.position > self.after
            && message.reply_to == self.reply_to
            && message.document.as_ref() == Some(&self.document)
            && held == wanted
    }
}

fn empty_state(place: MessagePlace) -> MessageState {
    MessageState {
        place,
        last_read: 0,
        latest: 0,
        unread: 0,
        mentions: 0,
    }
}

impl ServerModel {
    /// The unread summary of a place. Absent for a place without a message and for a place
    /// that the account cannot read.
    pub fn message_state(&self, place: PlaceId) -> Option<&MessageState> {
        self.message_states
            .iter()
            .find(|state| PlaceId::of(&state.place) == place)
    }

    /// The messages of a place that a reader has open.
    pub fn timeline(&self, place: PlaceId) -> Option<&Arc<Timeline>> {
        self.timelines
            .iter()
            .find(|timeline| timeline.place == place)
    }

    /// The messages of a place that wait to be sent, in the order in which the person sent
    /// them.
    pub fn waiting(&self, place: PlaceId) -> impl Iterator<Item = &Arc<PendingMessage>> {
        self.outbox
            .iter()
            .filter(move |pending| pending.place == place)
    }

    /// The place as the server names it, when the state holds it.
    pub fn place(&self, place: PlaceId) -> Option<MessagePlace> {
        match place {
            PlaceId::Channel(channel_id) => {
                self.channel(channel_id).map(|held| MessagePlace::Channel {
                    workspace_id: held.channel.workspace_id,
                    channel_id,
                })
            }
            PlaceId::Direct(conversation_id) => self
                .conversations
                .iter()
                .find(|held| held.id == conversation_id)
                .map(|held| MessagePlace::Direct {
                    conversation_id,
                    low_account_id: held.low_account_id,
                    high_account_id: held.high_account_id,
                }),
        }
    }

    /// The conversation with an account, when one is open.
    pub fn conversation_with(&self, account_id: AccountId) -> Option<&Conversation> {
        let own = self.account.id;
        self.conversations.iter().find(|held| {
            (held.low_account_id == own && held.high_account_id == account_id)
                || (held.high_account_id == own && held.low_account_id == account_id)
        })
    }

    pub(crate) fn put_conversation(&mut self, conversation: &Conversation) {
        if self
            .conversations
            .iter()
            .all(|held| held.id != conversation.id)
        {
            self.conversations.push(conversation.clone());
            self.conversations.sort_by_key(|held| held.id);
        }
    }

    pub(crate) fn set_message_state(&mut self, state: MessageState) {
        let place = PlaceId::of(&state.place);
        match self
            .message_states
            .iter_mut()
            .find(|held| PlaceId::of(&held.place) == place)
        {
            Some(held) => *held = state,
            None => self.message_states.push(state),
        }
    }

    /// Moves the read position of a place forward. True when the unread counts are known
    /// after the move: everything is read. Otherwise the caller reads the summary.
    pub(crate) fn move_read_position(&mut self, place: MessagePlace, position: u64) -> bool {
        let id = PlaceId::of(&place);
        let index = match self
            .message_states
            .iter()
            .position(|held| PlaceId::of(&held.place) == id)
        {
            Some(index) => index,
            None => {
                self.message_states.push(empty_state(place));
                self.message_states.len() - 1
            }
        };
        let held = &mut self.message_states[index];
        held.last_read = held.last_read.max(position);
        if held.last_read >= held.latest {
            held.unread = 0;
            held.mentions = 0;
            true
        } else {
            false
        }
    }

    pub(crate) fn forget_place(&mut self, place: PlaceId) {
        self.message_states
            .retain(|held| PlaceId::of(&held.place) != place);
        self.timelines.retain(|held| held.place != place);
        self.outbox.retain(|held| held.place != place);
    }

    pub(crate) fn timeline_mut(&mut self, place: PlaceId) -> Option<&mut Timeline> {
        self.timelines
            .iter_mut()
            .find(|timeline| timeline.place == place)
            .map(Arc::make_mut)
    }

    /// Starts to hold the messages of a place for a reader.
    pub(crate) fn open_timeline(&mut self, place: PlaceId) {
        if self.timeline(place).is_none() {
            self.timelines.push(Arc::new(Timeline::new(place)));
        }
    }

    pub(crate) fn close_timeline(&mut self, place: PlaceId) {
        self.timelines.retain(|held| held.place != place);
    }

    /// Puts a message that the server returned into the timeline of its place.
    pub(crate) fn put_message(&mut self, message: Message) -> bool {
        let place = PlaceId::of(&message.place);
        let wanted = self
            .timeline(place)
            .is_some_and(|timeline| timeline.wants(message.id, message.position));
        wanted
            && self
                .timeline_mut(place)
                .is_some_and(|timeline| timeline.put(message))
    }

    /// Records a message that the server returned as the answer to a send of this client.
    /// Such a message confirms no other waiting message. The list holds the newest ones.
    pub(crate) fn note_sent(&mut self, message_id: MessageId) {
        const MOST_NOTED: usize = 256;
        if self.sent_ids.len() >= MOST_NOTED {
            self.sent_ids.remove(0);
        }
        self.sent_ids.push(message_id);
    }

    /// The loaded message of the server that is `pending` as the server stored it, when
    /// no send of this client and no entry of `taken` accounts for it already.
    fn arrival_of(&self, pending: &PendingMessage, taken: &[MessageId]) -> Option<MessageId> {
        let own = self.account.id;
        self.timeline(pending.place)?
            .messages
            .iter()
            .rev()
            .take_while(|message| message.position > pending.after)
            .filter(|message| !self.sent_ids.contains(&message.id) && !taken.contains(&message.id))
            .find(|message| pending.is_confirmed_by(message, own))
            .map(|message| message.id)
    }

    /// Ends the wait of each message without an answer that the loaded messages of its
    /// place show as stored. One message of the server confirms one waiting message.
    pub(crate) fn confirm_arrived(&mut self, place: PlaceId) -> bool {
        let mut confirmed = Vec::new();
        let mut taken = Vec::new();
        for pending in self
            .outbox
            .iter()
            .filter(|pending| pending.place == place && pending.state == SendState::Unconfirmed)
        {
            if let Some(message_id) = self.arrival_of(pending, &taken) {
                confirmed.push(pending.id);
                taken.push(message_id);
            }
        }
        for message_id in taken {
            self.note_sent(message_id);
        }
        self.outbox
            .retain(|pending| !confirmed.contains(&pending.id));
        !confirmed.is_empty()
    }

    /// The waiting messages of a place that a reader draws. A message whose request is on
    /// its way is left out while the loaded messages hold its copy already: the event of
    /// the message can be followed before the answer to the request arrives.
    pub fn shown_waiting(&self, place: PlaceId) -> Vec<&Arc<PendingMessage>> {
        let mut taken = Vec::new();
        self.waiting(place)
            .filter(|pending| {
                if pending.state != SendState::Sending {
                    return true;
                }
                match self.arrival_of(pending, &taken) {
                    Some(message_id) => {
                        taken.push(message_id);
                        false
                    }
                    None => true,
                }
            })
            .collect()
    }

    /// The server holds no such message for the account: a message that was deleted
    /// without a reply to it leaves no marker (ADR 0408).
    pub(crate) fn remove_message(&mut self, place: PlaceId, message_id: MessageId) -> bool {
        let held = self
            .timeline(place)
            .is_some_and(|timeline| timeline.message(message_id).is_some());
        held && self
            .timeline_mut(place)
            .is_some_and(|timeline| timeline.remove(message_id))
    }

    pub(crate) fn pending_mut(&mut self, id: u64) -> Option<&mut PendingMessage> {
        self.outbox
            .iter_mut()
            .find(|pending| pending.id == id)
            .map(Arc::make_mut)
    }

    /// Takes the timelines that readers hold and the waiting messages from the state
    /// before a reload. The timelines are marked: the page that replaces their messages
    /// is on its way.
    pub(crate) fn carry_messages(&mut self, before: &ServerModel, wanted: &[PlaceId]) {
        if before.account.id != self.account.id {
            return;
        }
        self.outbox.clone_from(&before.outbox);
        self.sent_ids.clone_from(&before.sent_ids);
        self.attachment_limits.clone_from(&before.attachment_limits);
        for timeline in &before.timelines {
            if wanted.contains(&timeline.place) {
                let mut carried = timeline.as_ref().clone();
                carried.stale = true;
                carried.reading = Reading::default();
                self.timelines.push(Arc::new(carried));
            }
        }
    }
}
