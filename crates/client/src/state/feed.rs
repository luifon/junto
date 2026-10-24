//! The rows of a message list, derived from the messages of one place (ADR 0740).
//!
//! [`derive`] is a function without side effects, as the derivation of the window is
//! (ADR 0730). It takes the timeline of a place, the messages that wait to be sent, the
//! names that the client holds and a time zone, and returns rows: day separators, the line
//! of the first unread message, messages with what belongs to them, system lines, and the
//! messages that wait. The drawing code holds no rule about a message.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use junto_client_core::transfers::DownloadProgress;

use super::recorded::{self, RecordingCard};
use junto_client_core::model::{
    PendingMessage, PlaceId, ReadFailure, Reading, SendState, ServerModel, Timeline,
};
use junto_client_core::protocol::account::Departure;
use junto_client_core::protocol::ids::{
    AccountId, ChannelId, MessageId, RecordingId, RecordingSegmentId, ServerId, WorkspaceRoleId,
};
use junto_client_core::protocol::message::{
    AttachmentLimits, Block, Document, Inline, Message, SystemMessage,
};
use time::{Date, OffsetDateTime, UtcOffset};

/// The time zone in which times of messages are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zone {
    /// A zone of the database of this build, with its rule for each date: the zone of the
    /// system as the state read it last (ADR 0798). The rules are in the binary, so a time
    /// is shown without a file of the system.
    Named(chrono_tz::Tz),
    /// One offset for every date. The tests and the screen tool use it.
    Fixed(UtcOffset),
}

impl Default for Zone {
    fn default() -> Self {
        Self::Named(chrono_tz::Tz::UTC)
    }
}

impl Zone {
    /// A moment as the clock of the zone shows it. A moment that this clock cannot state
    /// is shown on the clock that the server wrote it with (ADR 0747).
    pub fn local(self, at: OffsetDateTime) -> OffsetDateTime {
        let offset = match self {
            Self::Fixed(offset) => offset,
            Self::Named(zone) => {
                use chrono::{Offset as _, TimeZone as _};
                let seconds = zone
                    .timestamp_opt(at.unix_timestamp(), 0)
                    .single()
                    .map_or(0, |local| local.offset().fix().local_minus_utc());
                UtcOffset::from_whole_seconds(seconds).unwrap_or(UtcOffset::UTC)
            }
        };
        crate::moment::in_offset(at, offset)
    }

    /// The first moment of a date on the clock of the zone. On a date whose clock skips
    /// midnight, the first hour that the clock shows.
    pub fn start_of(self, date: Date) -> OffsetDateTime {
        match self {
            Self::Fixed(offset) => date.midnight().assume_offset(offset),
            Self::Named(zone) => {
                use chrono::TimeZone as _;
                let day = chrono::NaiveDate::from_ymd_opt(
                    date.year(),
                    u32::from(u8::from(date.month())),
                    u32::from(date.day()),
                );
                (0..24)
                    .find_map(|hour| {
                        let local = day?.and_hms_opt(hour, 0, 0)?;
                        let first = zone.from_local_datetime(&local).earliest()?;
                        OffsetDateTime::from_unix_timestamp(first.timestamp()).ok()
                    })
                    .unwrap_or_else(|| date.midnight().assume_utc())
            }
        }
    }
}

/// Messages of one author within this time form one group under one name.
const GROUP_SECONDS: i64 = 5 * 60;
/// The characters of a quoted message that a reply shows.
const EXCERPT_CHARACTERS: usize = 120;

/// A message that a reply names and that the loaded messages do not hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Quoted {
    /// The message was read from the server.
    Read(Arc<Message>),
    /// The server holds no such message for the account.
    Gone,
    /// The message is not held: the quoted messages of the window weigh as much as they
    /// may, or the answer of the server was a refusal or no answer that the client reads
    /// (ADR 0747). The reply shows no text of it, and the message is not read again until
    /// an event names it or the state is loaded again.
    NotHeld,
}

/// What the window holds for a quoted message whose read failed. `None` when the same
/// read can succeed later: no answer, `busy`, a limit on attempts, a failure of the
/// server, and a request that was not sent because the device has no session at that
/// moment. That read is sent again with a later change of the state. Every other answer
/// of the server is final for the message as it is now.
pub fn quote_without_answer(error: &junto_client_core::api::ApiError) -> Option<Quoted> {
    use junto_client_core::api::{ApiError, ErrorCategory};
    match error {
        _ if error.category() == Some(ErrorCategory::NotFound) => Some(Quoted::Gone),
        _ if error.category() == Some(ErrorCategory::Limited) || error.is_unavailable() => None,
        ApiError::Rejected(_) | ApiError::InvalidResponse { .. } => Some(Quoted::NotHeld),
        _ => None,
    }
}

/// The weight of the quoted messages that the window holds at most, in the measure of
/// [`junto_client_core::model::weight`].
pub const MOST_QUOTED_WEIGHT: usize = 8 * 1024 * 1024;

/// Puts a quoted message that was read into the quoted messages of the window, when
/// they stay inside their weight with it.
pub fn hold_quote(
    quoted: &mut HashMap<(ServerId, MessageId), Quoted>,
    key: (ServerId, MessageId),
    message: Message,
) {
    use junto_client_core::model::weight;
    let held: usize = quoted
        .iter()
        .filter(|(of, _)| **of != key)
        .map(|(_, quoted)| match quoted {
            Quoted::Read(message) => weight(message),
            Quoted::Gone | Quoted::NotHeld => 0,
        })
        .sum();
    let fits = held.saturating_add(weight(&message)) <= MOST_QUOTED_WEIGHT;
    let entry = if fits {
        Quoted::Read(Arc::new(message))
    } else {
        Quoted::NotHeld
    };
    quoted.insert(key, entry);
}

/// What a reply shows of the message that it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuoteText {
    Text(String),
    /// The message has no text: the name of its first file.
    File(String),
    Deleted,
    /// The message is not loaded yet, or the server does not give it to the account.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quote {
    pub message_id: MessageId,
    pub author_id: Option<AccountId>,
    pub author: Option<String>,
    pub text: QuoteText,
}

/// How a system line is marked at its start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemKind {
    /// Something began or was allowed: the green square.
    Start,
    /// A recording: the red dot.
    Recording,
    Plain,
}

/// One part of the text of a system line. A name is drawn bold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Part {
    pub text: String,
    pub strong: bool,
}

/// How many authors of replies the thread line of a message shows.
pub const MOST_THREAD_AUTHORS: usize = 3;

/// What a message of a channel shows of the thread under it (ADR 0403).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadMark {
    pub replies: u64,
    /// The first authors of the replies, at most three, in the order of their first
    /// reply (ADR 0413). Empty when the server stated none.
    pub authors: Vec<AccountId>,
    /// The author and the time of the last reply, once that reply was read.
    pub last: Option<LastReply>,
    /// The side panel shows this thread.
    pub open: bool,
    /// What the person has not read in the thread. 100 means 100 or more.
    pub unread: u64,
    pub mentions: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LastReply {
    pub author_id: Option<AccountId>,
    pub author: String,
    /// The time of day in the zone of the list.
    pub time: String,
}

#[derive(Debug, Clone)]
pub struct MessageRow {
    pub message: Arc<Message>,
    pub author: String,
    /// The time of day in the zone of the list, as `09:02`.
    pub time: String,
    /// The message follows a message of the same author within a short time: it is drawn
    /// without the avatar and the name.
    pub grouped: bool,
    pub own: bool,
    /// The message mentions the account, one of its roles, or everyone.
    pub mentions_own: bool,
    pub reply: Option<Quote>,
    /// The message takes a reaction and an answer: a message of a person in a place that
    /// takes changes (ADR 0410).
    pub can_react: bool,
    pub can_reply: bool,
    /// The roles of the person allow the change. The server decides each request.
    pub can_edit: bool,
    pub can_delete: bool,
    /// The message can be pinned or unpinned, and the roles of the person allow it.
    pub can_pin: bool,
    /// The message can have a thread: an authored message of a channel that is no reply
    /// in a thread itself.
    pub can_thread: bool,
    /// The thread under the message, when it has replies.
    pub thread: Option<ThreadMark>,
    /// The person went to this message from a notice or a search result.
    pub target: bool,
}

impl PartialEq for MessageRow {
    /// Two rows are the same when they draw the same. A message is compared by the copy
    /// that the state holds, which is replaced when the message changes.
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.message, &other.message)
            && self.grouped == other.grouped
            && self.author == other.author
            && self.time == other.time
            && self.reply == other.reply
            && self.can_react == other.can_react
            && self.can_reply == other.can_reply
            && self.can_edit == other.can_edit
            && self.can_delete == other.can_delete
            && self.can_pin == other.can_pin
            && self.can_thread == other.can_thread
            && self.thread == other.thread
            && self.target == other.target
    }
}

impl Eq for MessageRow {}

impl MessageRow {
    /// The message offers at least one action: its bar and its menu have an entry.
    pub fn has_actions(&self) -> bool {
        self.can_react
            || self.can_reply
            || self.can_thread
            || self.can_pin
            || self.can_edit
            || self.can_delete
    }
}

#[derive(Debug, Clone)]
pub struct SystemRow {
    pub message: Arc<Message>,
    pub time: String,
    pub kind: SystemKind,
    pub parts: Vec<Part>,
}

/// A finished recording in the chat of its voice channel (ADR 0744).
#[derive(Debug, Clone)]
pub struct RecordingRow {
    /// The system message that posted the recording.
    pub message: Arc<Message>,
    pub time: String,
    pub card: RecordingCard,
}

/// What the cards of the recordings of a list read beside the state of the server.
pub struct Cards<'a> {
    /// The name of the voice channel of the list.
    pub channel: &'a str,
    pub channel_id: ChannelId,
    /// The names of the accounts of the server that the client holds.
    pub accounts: &'a HashMap<AccountId, String>,
    /// The roles of the person hold "manage recordings", and the channel is not
    /// archived: the removal of a recording and a new request for its transcript.
    pub may_remove: bool,
    pub downloads: &'a HashMap<(ServerId, RecordingSegmentId), DownloadProgress>,
    pub texts: &'a HashMap<(ServerId, RecordingId), DownloadProgress>,
    pub removing: &'a HashSet<(ServerId, RecordingId)>,
    /// The recordings whose transcript the person asked for again.
    pub requesting: &'a HashSet<(ServerId, RecordingId)>,
    pub refused: &'a HashMap<(ServerId, RecordingId), String>,
}

#[derive(Debug, Clone)]
pub struct WaitingRow {
    pub pending: Arc<PendingMessage>,
    pub author: String,
    pub time: String,
    pub grouped: bool,
    pub reply: Option<Quote>,
}

#[derive(Debug, Clone)]
pub enum FeedRow {
    /// The history has no message before the first loaded one.
    Start,
    /// The history has messages before the first loaded one.
    Earlier {
        reading: bool,
    },
    Day(Date),
    /// The line before the first unread message.
    Unread,
    Message(MessageRow),
    System(SystemRow),
    /// The card of a finished recording, in place of the system line that posted it.
    Recording(RecordingRow),
    Waiting(WaitingRow),
    /// The history has messages after the last loaded one.
    Later {
        reading: bool,
    },
}

impl PartialEq for FeedRow {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Start, Self::Start) | (Self::Unread, Self::Unread) => true,
            (Self::Earlier { reading: one }, Self::Earlier { reading: other })
            | (Self::Later { reading: one }, Self::Later { reading: other }) => one == other,
            (Self::Day(one), Self::Day(other)) => one == other,
            (Self::Message(one), Self::Message(other)) => one == other,
            (Self::System(one), Self::System(other)) => {
                Arc::ptr_eq(&one.message, &other.message)
                    && one.parts == other.parts
                    && one.time == other.time
            }
            (Self::Recording(one), Self::Recording(other)) => {
                Arc::ptr_eq(&one.message, &other.message)
                    && one.time == other.time
                    && one.card == other.card
            }
            (Self::Waiting(one), Self::Waiting(other)) => {
                Arc::ptr_eq(&one.pending, &other.pending)
                    && one.grouped == other.grouped
                    && one.reply == other.reply
            }
            _ => false,
        }
    }
}

impl Eq for FeedRow {}

impl FeedRow {
    /// The position of the message of the row in the history of its place.
    pub fn position(&self) -> Option<u64> {
        match self {
            Self::Message(row) => Some(row.message.position),
            Self::System(row) => Some(row.message.position),
            Self::Recording(row) => Some(row.message.position),
            _ => None,
        }
    }
}

/// The message list of one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Feed {
    pub server_id: ServerId,
    pub place: PlaceId,
    pub rows: Arc<Vec<FeedRow>>,
    /// A first page has arrived.
    pub loaded: bool,
    pub failed: Option<ReadFailure>,
    /// The messages can miss changes: the state was replaced, and their page is on its
    /// way, or a read of a change failed.
    pub stale: bool,
    pub reading: Reading,
    pub more_before: bool,
    pub more_after: bool,
    /// The position of the last loaded message, when the list reaches the end of the
    /// history. The read position moves to it when the person has seen the end.
    pub end_position: Option<u64>,
    /// The unread messages of the place, as the server counts them. 100 means 100 or more.
    pub unread: u64,
    pub limits: Option<AttachmentLimits>,
    /// The names for the mentions of the messages.
    pub names: Arc<Names>,
    /// The date of the derivation in the zone of the list. A day separator of another
    /// year names its year.
    pub today: Option<Date>,
    /// The message that the person went to, with the number of that jump. A second jump
    /// to the same message has another number, and the list goes there again.
    pub target: Option<(MessageId, u64)>,
}

impl Feed {
    /// True when the list holds a message that waits to be sent.
    pub fn has_waiting(&self) -> bool {
        self.rows
            .iter()
            .any(|row| matches!(row, FeedRow::Waiting(_)))
    }
}

/// The names that the messages of a place mention.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Names {
    /// The names of departed accounts carry their mark here (ADR 0121).
    pub accounts: HashMap<AccountId, String>,
    pub roles: HashMap<WorkspaceRoleId, String>,
    /// The accounts that left the server or were deleted.
    pub departed: HashMap<AccountId, Departure>,
}

/// What the derivation of one list reads.
pub struct Source<'a> {
    pub server_id: ServerId,
    pub model: &'a ServerModel,
    pub timeline: &'a Timeline,
    pub names: Arc<Names>,
    /// The roles of the account in the workspace of the channel. Empty for a direct
    /// conversation.
    pub own_roles: &'a [WorkspaceRoleId],
    /// The roles of the person allow the deletion of messages of other people, and pins.
    pub may_manage: bool,
    /// The account is an administrator of the server: it deletes a permanent message of a
    /// channel (ADR 0402).
    pub administrator: bool,
    /// The thread that the side panel shows.
    pub open_thread: Option<MessageId>,
    /// The message that the person went to from a notice or a search result, with the
    /// number of that jump.
    pub target: Option<(MessageId, u64)>,
    /// The list offers the thread of a message. The chat of the call of this device does
    /// not: its side panel is the panel of the call.
    pub threads: bool,
    pub quoted: &'a HashMap<(ServerId, MessageId), Quoted>,
    pub zone: Zone,
    pub today: Option<Date>,
    /// The list shows a finished recording as a card. Absent: as a system line.
    pub cards: Option<&'a Cards<'a>>,
}

fn clock(at: OffsetDateTime) -> String {
    format!("{:02}:{:02}", at.hour(), at.minute())
}

/// The text of a document on one line, without its formatting, up to a bound.
pub fn plain_text(document: &Document, names: &Names, most: usize) -> String {
    fn inline(content: &[Inline], names: &Names, out: &mut String) {
        for node in content {
            match node {
                Inline::Text { text, .. } | Inline::Code { text } | Inline::Link { text, .. } => {
                    out.push_str(text);
                }
                Inline::Account { account_id } => {
                    out.push('@');
                    out.push_str(
                        names
                            .accounts
                            .get(account_id)
                            .map_or(crate::text::UNKNOWN_PERSON, String::as_str),
                    );
                }
                Inline::Role { role_id } => {
                    out.push('@');
                    out.push_str(
                        names
                            .roles
                            .get(role_id)
                            .map_or(crate::text::UNKNOWN_ROLE, String::as_str),
                    );
                }
                Inline::Everyone => out.push_str(crate::text::MENTION_EVERYONE),
            }
        }
    }
    let mut text = String::new();
    for block in &document.blocks {
        if !text.is_empty() {
            text.push(' ');
        }
        match block {
            Block::Paragraph { content } | Block::Quote { content } => {
                inline(content, names, &mut text);
            }
            Block::Code { text: code, .. } => text.push_str(code),
            Block::List { items, .. } => {
                for item in items {
                    inline(item, names, &mut text);
                    text.push(' ');
                }
            }
            Block::Checklist { items } => {
                for item in items {
                    inline(&item.content, names, &mut text);
                    text.push(' ');
                }
            }
        }
        if text.chars().count() > most {
            break;
        }
    }
    let line: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > most {
        let cut: String = line.chars().take(most).collect();
        format!("{}…", cut.trim_end())
    } else {
        line
    }
}

fn mentions(document: &Document, own: AccountId, own_roles: &[WorkspaceRoleId]) -> bool {
    let named = |node: &Inline| match node {
        Inline::Account { account_id } => *account_id == own,
        Inline::Role { role_id } => own_roles.contains(role_id),
        Inline::Everyone => true,
        _ => false,
    };
    document.blocks.iter().any(|block| match block {
        Block::Paragraph { content } | Block::Quote { content } => content.iter().any(named),
        Block::Code { .. } => false,
        Block::List { items, .. } => items.iter().flatten().any(named),
        Block::Checklist { items } => items.iter().flat_map(|item| &item.content).any(named),
    })
}

fn author_name(message: &Message, names: &Names) -> String {
    let departure = message
        .author_id
        .and_then(|id| names.departed.get(&id).copied());
    message
        .author_name
        .as_deref()
        .map(crate::text::label)
        .filter(|name| !name.trim().is_empty())
        .map(|name| match departure {
            Some(departure) => crate::text::departed_name(&name, departure),
            None => name,
        })
        .or_else(|| {
            message
                .author_id
                .and_then(|id| names.accounts.get(&id).cloned())
        })
        .unwrap_or_else(|| crate::text::UNKNOWN_PERSON.to_owned())
}

fn quote_of(message: &Message, names: &Names) -> Quote {
    let text = if message.deleted {
        QuoteText::Deleted
    } else {
        match &message.document {
            Some(document) => {
                let text = plain_text(document, names, EXCERPT_CHARACTERS);
                match (text.is_empty(), message.attachments.first()) {
                    (true, Some(file)) => QuoteText::File(crate::text::label(&file.name)),
                    _ => QuoteText::Text(text),
                }
            }
            None => match &message.system {
                Some(system) => QuoteText::Text(
                    system_parts(system, names)
                        .1
                        .iter()
                        .map(|part| part.text.as_str())
                        .collect(),
                ),
                None => QuoteText::Unknown,
            },
        }
    };
    Quote {
        message_id: message.id,
        author_id: message.author_id,
        author: message
            .author_id
            .is_some()
            .then(|| author_name(message, names)),
        text,
    }
}

/// The loaded messages of a timeline by their identifiers. It is built for the first
/// reply of a derivation: a list without a reply does not pay for it.
#[derive(Default)]
pub struct Loaded<'a>(Option<HashMap<MessageId, &'a Arc<Message>>>);

impl<'a> Loaded<'a> {
    fn get(&mut self, timeline: &'a Timeline, message_id: MessageId) -> Option<&'a Arc<Message>> {
        self.0
            .get_or_insert_with(|| {
                timeline
                    .messages
                    .iter()
                    .map(|message| (message.id, message))
                    .collect()
            })
            .get(&message_id)
            .copied()
    }
}

fn quote<'a>(source: &Source<'a>, loaded: &mut Loaded<'a>, message_id: MessageId) -> Quote {
    let unknown = |text| Quote {
        message_id,
        author_id: None,
        author: None,
        text,
    };
    if let Some(held) = loaded.get(source.timeline, message_id) {
        return quote_of(held, &source.names);
    }
    match source.quoted.get(&(source.server_id, message_id)) {
        Some(Quoted::Read(message)) => quote_of(message, &source.names),
        Some(Quoted::Gone) => unknown(QuoteText::Deleted),
        Some(Quoted::NotHeld) | None => unknown(QuoteText::Unknown),
    }
}

/// The text of a system line and the mark at its start. A line of an action that this
/// build does not know states that something happened.
pub fn system_parts(system: &SystemMessage, names: &Names) -> (SystemKind, Vec<Part>) {
    let name = |account_id: Option<AccountId>| {
        account_id.map(|id| {
            names
                .accounts
                .get(&id)
                .cloned()
                .unwrap_or_else(|| crate::text::UNKNOWN_PERSON.to_owned())
        })
    };
    let actor = name(system.actor_id);
    let subject = name(system.subject_id);
    let (kind, sentence) = crate::text::system_line(&system.action);
    let mut parts = Vec::new();
    let mut push = |text: &str, strong: bool| {
        if !text.is_empty() {
            parts.push(Part {
                text: text.to_owned(),
                strong,
            });
        }
    };
    // The sentence names the actor as `{actor}` and the subject as `{subject}`.
    let mut rest = sentence;
    while let Some(at) = rest.find('{') {
        push(&rest[..at], false);
        let end = rest[at..].find('}').map_or(rest.len(), |end| at + end + 1);
        match &rest[at..end] {
            "{actor}" => push(
                actor.as_deref().unwrap_or(crate::text::SOMEBODY),
                actor.is_some(),
            ),
            "{subject}" => push(
                subject.as_deref().unwrap_or(crate::text::SOMEBODY),
                subject.is_some(),
            ),
            "{duration}" => push(&crate::text::call_length(system.duration_seconds), false),
            other => push(other, false),
        }
        rest = &rest[end..];
    }
    push(rest, false);
    (kind, parts)
}

/// What a message of a channel shows of its thread. `None` for a message without replies.
fn thread_mark<'a>(
    source: &Source<'a>,
    message: &Message,
    loaded: &mut Loaded<'a>,
) -> Option<ThreadMark> {
    if message.reply_count == 0 {
        return None;
    }
    let last = message.last_reply.and_then(|reply_id| {
        // The last reply lies in the thread, not in this list: the open thread holds it,
        // or the window read it.
        let held = loaded.get(source.timeline, reply_id).cloned().or_else(|| {
            source
                .model
                .timelines
                .iter()
                .filter(|timeline| timeline.place.thread_root() == Some(message.id))
                .find_map(|timeline| timeline.message(reply_id).cloned())
        });
        let read = match source.quoted.get(&(source.server_id, reply_id)) {
            Some(Quoted::Read(read)) => Some(read.clone()),
            _ => None,
        };
        held.or(read).map(|reply| LastReply {
            author_id: reply.author_id,
            author: author_name(&reply, &source.names),
            time: clock(source.zone.local(reply.created_at)),
        })
    });
    let state = source.model.thread_state(message.id);
    Some(ThreadMark {
        replies: message.reply_count,
        // The server states at most three; a longer list is cut (ADR 0714).
        authors: message
            .reply_authors
            .iter()
            .copied()
            .take(MOST_THREAD_AUTHORS)
            .collect(),
        last,
        open: source.open_thread == Some(message.id),
        unread: state.map_or(0, |state| state.unread),
        mentions: state.map_or(0, |state| state.mentions),
    })
}

/// One message of a person as a row. The caller decides whether it continues a group.
pub fn message_row<'a>(
    source: &Source<'a>,
    message: &'a Arc<Message>,
    loaded: &mut Loaded<'a>,
) -> MessageRow {
    let own = source.model.account.id;
    let names = &source.names;
    let local = source.zone.local(message.created_at);
    let is_own = message.author_id == Some(own);
    let changeable = !message.deleted && message.system.is_none();
    let in_channel = matches!(
        message.place,
        junto_client_core::protocol::message::MessagePlace::Channel { .. }
    );
    let has_thread = changeable && in_channel && message.thread_root.is_none();
    let can_thread = has_thread && source.threads;
    MessageRow {
        author: author_name(message, names),
        time: clock(local),
        grouped: false,
        own: is_own,
        mentions_own: message
            .document
            .as_ref()
            .is_some_and(|document| mentions(document, own, source.own_roles)),
        reply: message.reply_to.map(|id| quote(source, loaded, id)),
        can_react: changeable,
        can_reply: changeable,
        can_edit: changeable && is_own && !message.permanent,
        // A permanent message is deleted by an administrator only, also its author's
        // (ADR 0402).
        can_delete: changeable
            && if message.permanent {
                in_channel && source.administrator
            } else {
                is_own || source.may_manage
            },
        can_pin: changeable && in_channel && source.may_manage,
        can_thread,
        thread: has_thread
            .then(|| thread_mark(source, message, loaded))
            .flatten(),
        target: source
            .target
            .is_some_and(|(message_id, _)| message_id == message.id),
        message: message.clone(),
    }
}

/// The rows of the messages of one place.
pub fn derive(source: &Source<'_>) -> Feed {
    let timeline = source.timeline;
    let own = source.model.account.id;
    let names = &source.names;
    let mut rows = Vec::with_capacity(timeline.messages.len() + 8);
    // The replies of a thread stand under its root: the root is the start of their
    // history, and its day is named with it.
    let in_thread = timeline.place.thread_root().is_some();
    if timeline.loaded && timeline.more_before {
        rows.push(FeedRow::Earlier {
            reading: timeline.reading.older,
        });
    } else if timeline.loaded && !in_thread {
        rows.push(FeedRow::Start);
    }
    let mut loaded = Loaded::default();
    let mut day: Option<Date> = timeline
        .root
        .as_ref()
        .filter(|_| in_thread && !timeline.more_before)
        .map(|root| source.zone.local(root.created_at).date());
    // The author and the time of the message row before, while a group can go on.
    let mut group: Option<(AccountId, OffsetDateTime)> = None;
    // The line needs a loaded message before it that the person has read. When every
    // loaded message lies after the read position, no line is drawn above them all.
    let mut unread_drawn = timeline.unread_after.is_none_or(|read| {
        timeline
            .messages
            .first()
            .is_some_and(|first| first.position > read)
    });
    for message in &timeline.messages {
        let local = source.zone.local(message.created_at);
        if day != Some(local.date()) {
            day = Some(local.date());
            rows.push(FeedRow::Day(local.date()));
            group = None;
        }
        let unread = !unread_drawn
            && timeline
                .unread_after
                .is_some_and(|read| message.position > read)
            && message.author_id.is_some_and(|author| author != own)
            && !message.deleted;
        if unread {
            unread_drawn = true;
            rows.push(FeedRow::Unread);
            group = None;
        }
        if let (None, Some(system)) = (message.author_id, &message.system) {
            // The post of a finished recording is its card (view G of the design).
            let recording = system
                .recording_id
                .filter(|_| system.action == "recording_ready")
                .zip(source.cards);
            if let Some((recording_id, cards)) = recording {
                rows.push(FeedRow::Recording(RecordingRow {
                    message: message.clone(),
                    time: clock(local),
                    card: recorded::card(&recorded::CardSource {
                        server_id: source.server_id,
                        recording_id,
                        open: source.model.recording(recording_id).map(Arc::as_ref),
                        channel: cards.channel,
                        channel_id: cards.channel_id,
                        names: cards.accounts,
                        zone: source.zone,
                        today: source.today,
                        may_remove: cards.may_remove,
                        may_request: cards.may_remove,
                        downloads: cards.downloads,
                        texts: cards.texts,
                        removing: cards.removing.contains(&(source.server_id, recording_id)),
                        requesting: cards.requesting.contains(&(source.server_id, recording_id)),
                        refused: cards
                            .refused
                            .get(&(source.server_id, recording_id))
                            .map(String::as_str),
                    }),
                }));
                group = None;
                continue;
            }
            let (kind, parts) = system_parts(system, names);
            rows.push(FeedRow::System(SystemRow {
                message: message.clone(),
                time: clock(local),
                kind,
                parts,
            }));
            group = None;
            continue;
        }
        let grouped = !message.deleted
            && message.reply_to.is_none()
            && message.author_id.is_some_and(|author| {
                group.is_some_and(|(before, at)| {
                    before == author
                        && (message.created_at - at).whole_seconds() <= GROUP_SECONDS
                        && message.created_at >= at
                })
            });
        group = match (message.author_id, message.deleted) {
            (Some(author), false) => Some((author, message.created_at)),
            _ => None,
        };
        let mut row = message_row(source, message, &mut loaded);
        row.grouped = grouped;
        rows.push(FeedRow::Message(row));
    }
    if timeline.more_after {
        rows.push(FeedRow::Later {
            reading: timeline.reading.newer,
        });
    } else if timeline.loaded || timeline.failed.is_some() {
        // The messages that wait are drawn after the end of the history, also when the
        // history was not read: the person can send them again or discard them.
        for pending in source.model.shown_waiting(timeline.place) {
            let local = source.zone.local(pending.created_at);
            if day != Some(local.date()) {
                day = Some(local.date());
                rows.push(FeedRow::Day(local.date()));
                group = None;
            }
            let grouped = pending.reply_to.is_none()
                && group.is_some_and(|(before, at)| {
                    before == own && (pending.created_at - at).whole_seconds() <= GROUP_SECONDS
                });
            group = Some((own, pending.created_at));
            rows.push(FeedRow::Waiting(WaitingRow {
                pending: pending.clone(),
                author: source.model.account.name.clone(),
                time: clock(local),
                grouped,
                reply: pending.reply_to.map(|id| quote(source, &mut loaded, id)),
            }));
        }
    }
    let unread = match timeline.place {
        PlaceId::Thread { root_id, .. } => source
            .model
            .thread_state(root_id)
            .map_or(0, |state| state.unread.max(state.mentions)),
        PlaceId::Channel(_) | PlaceId::Direct(_) => source
            .model
            .message_state(timeline.place)
            .map_or(0, |state| state.unread),
    };
    Feed {
        server_id: source.server_id,
        place: timeline.place,
        rows: Arc::new(rows),
        loaded: timeline.loaded,
        failed: timeline.failed,
        stale: timeline.stale,
        reading: timeline.reading,
        more_before: timeline.more_before,
        more_after: timeline.more_after,
        end_position: (!timeline.more_after)
            .then(|| timeline.messages.last().map(|last| last.position))
            .flatten(),
        unread,
        limits: source.model.attachment_limits.clone(),
        names: source.names.clone(),
        today: source.today,
        target: source.target,
    }
}

/// The messages that the rows of a timeline name and that the timeline does not hold: the
/// message that a reply answers, and the last reply of a thread.
pub fn named_quotes(timeline: &Timeline) -> std::collections::HashSet<MessageId> {
    let mut loaded = Loaded::default();
    timeline
        .messages
        .iter()
        .flat_map(|message| {
            let last_reply = message.last_reply.filter(|_| message.reply_count > 0);
            message.reply_to.into_iter().chain(last_reply)
        })
        .filter(|id| loaded.get(timeline, *id).is_none())
        .collect()
}

/// The channel of a place of messages, when it has one.
pub fn channel_of(place: PlaceId) -> Option<ChannelId> {
    match place.container() {
        PlaceId::Channel(channel_id) => Some(channel_id),
        _ => None,
    }
}

/// The replies of a timeline whose message is not loaded: the window reads them once.
pub fn missing_quotes(
    server_id: ServerId,
    timeline: &Timeline,
    quoted: &HashMap<(ServerId, MessageId), Quoted>,
) -> Vec<MessageId> {
    let mut loaded = Loaded::default();
    let mut missing: Vec<MessageId> = timeline
        .messages
        .iter()
        .filter_map(|message| message.reply_to)
        .filter(|id| loaded.get(timeline, *id).is_none() && !quoted.contains_key(&(server_id, *id)))
        .collect();
    missing.sort_unstable();
    missing.dedup();
    missing
}

/// True when a waiting message is drawn as one that the person can send again.
pub fn can_send_again(state: SendState) -> bool {
    matches!(
        state,
        SendState::Unconfirmed
            | SendState::Failed(
                junto_client_core::model::SendFailure::NotSent
                    | junto_client_core::model::SendFailure::Refused { .. }
            )
    )
}

#[cfg(test)]
mod tests {
    use junto_client_core::model::{PendingMessage, SendFailure};
    use junto_client_core::protocol::account::OwnAccount;
    use junto_client_core::protocol::events::Cursor;
    use junto_client_core::protocol::ids::{ChannelId, StreamEpoch, WorkspaceId};
    use junto_client_core::protocol::message::{Attachment, MessagePlace, MessageState};
    use junto_client_core::protocol::snapshot::{Snapshot, WorkspaceState};
    use junto_client_core::protocol::structure::{Channel, ChannelKind, Workspace};
    use time::Duration;

    use super::*;
    use crate::compose;

    struct Scene {
        model: ServerModel,
        own: AccountId,
        ana: AccountId,
        place: MessagePlace,
        names: Arc<Names>,
        quoted: HashMap<(ServerId, MessageId), Quoted>,
        server_id: ServerId,
        role: WorkspaceRoleId,
    }

    /// Thursday, 8 October 2026 at 09:00 UTC.
    fn morning() -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_791_450_000).unwrap()
    }

    fn scene() -> Scene {
        let (own, ana) = (AccountId::generate(), AccountId::generate());
        let workspace_id = WorkspaceId::generate();
        let channel_id = ChannelId::generate();
        let role = WorkspaceRoleId::generate();
        let snapshot = Snapshot {
            cursor: Cursor {
                epoch: StreamEpoch::generate(),
                position: 1,
            },
            account: OwnAccount {
                id: own,
                name: "Luca".to_owned(),
                avatar: None,
                is_owner: false,
                server_role_ids: Vec::new(),
                server_permissions: Vec::new(),
                workspace_ids: vec![workspace_id],
            },
            workspaces: vec![WorkspaceState {
                workspace: Workspace {
                    id: workspace_id,
                    name: "Studio".to_owned(),
                    is_default: true,
                },
                own_role_ids: vec![role],
                roles: Vec::new(),
                categories: Vec::new(),
                channels: vec![Channel {
                    id: channel_id,
                    workspace_id,
                    category_id: None,
                    kind: ChannelKind::Text,
                    name: "general".to_owned(),
                    position: 0,
                    permanent_messages: false,
                    user_limit: None,
                    hides_occupants: false,
                }],
                calls: Vec::new(),
                permissions: Vec::new(),
                channel_permissions: Vec::new(),
                notification_default: None,
            }],
            own_call: None,
            direct_calls: Vec::new(),
            call_invitations: Vec::new(),
            sent_call_invitations: Vec::new(),
            call_moves: Vec::new(),
            entry_requests: Vec::new(),
            direct_mutes: Vec::new(),
            messages: Vec::new(),
            threads: Vec::new(),
            threads_more: false,
            threads_next: None,
            notifications: Vec::new(),
            notification_settings: Vec::new(),
        };
        let names = Names {
            accounts: HashMap::from([(own, "Luca".to_owned()), (ana, "Ana".to_owned())]),
            roles: HashMap::from([(role, "Leads".to_owned())]),
            departed: HashMap::new(),
        };
        Scene {
            model: ServerModel::from_snapshot(crate::state::fixtures::from_roles(snapshot)),
            own,
            ana,
            place: MessagePlace::Channel {
                workspace_id,
                channel_id,
            },
            names: Arc::new(names),
            quoted: HashMap::new(),
            server_id: ServerId::generate(),
            role,
        }
    }

    impl Scene {
        fn message(&self, position: u64, author: AccountId, minutes: i64, text: &str) -> Message {
            Message {
                id: MessageId::generate(),
                place: self.place,
                position,
                author_id: Some(author),
                author_name: self.names.accounts.get(&author).cloned(),
                created_at: morning() + Duration::minutes(minutes),
                edited_at: None,
                document: Some(compose::parse(text, &[])),
                system: None,
                reply_to: None,
                permanent: false,
                deleted: false,
                attachments: Vec::new(),
                reactions: Vec::new(),
                thread_root: None,
                reply_count: 0,
                last_reply: None,
                reply_authors: Vec::new(),
                pinned: false,
                files_removed_by_retention: false,
            }
        }

        fn timeline(&self, messages: Vec<Message>) -> Timeline {
            Timeline {
                place: PlaceId::of(&self.place),
                messages: messages.into_iter().map(Arc::new).collect(),
                more_before: false,
                more_after: false,
                reading: Reading::default(),
                loaded: true,
                failed: None,
                stale: false,
                unread_after: None,
                root: None,
                forks: Vec::new(),
                anchor: None,
            }
        }

        fn feed(&self, timeline: &Timeline, may_manage: bool) -> Feed {
            derive(&Source {
                server_id: self.server_id,
                model: &self.model,
                timeline,
                names: self.names.clone(),
                own_roles: &[self.role],
                may_manage,
                administrator: false,
                open_thread: None,
                target: None,
                threads: true,
                quoted: &self.quoted,
                zone: Zone::Fixed(UtcOffset::UTC),
                today: Some(morning().date()),
                cards: None,
            })
        }
    }

    #[test]
    fn the_quoted_messages_of_the_window_hold_a_bounded_weight() {
        use junto_client_core::model::{MOST_MESSAGE_WEIGHT, weight};

        // Each quoted message has a text close to the most that one message weighs.
        let scene = scene();
        let server_id = scene.server_id;
        let heavy = |position: u64| {
            let mut message = scene.message(position, scene.ana, 0, "");
            message.document = Some(Document {
                blocks: vec![Block::Code {
                    text: "x".repeat(MOST_MESSAGE_WEIGHT - 4096),
                    language: None,
                }],
            });
            message
        };
        let each = weight(&heavy(1));
        assert!(each <= MOST_MESSAGE_WEIGHT);
        let fit = MOST_QUOTED_WEIGHT / each;
        let mut quoted = HashMap::new();
        let mut keys = Vec::new();
        for position in 0..fit as u64 + 5 {
            let message = heavy(position);
            keys.push((server_id, message.id));
            hold_quote(&mut quoted, (server_id, message.id), message);
        }
        let held = |quoted: &HashMap<(ServerId, MessageId), Quoted>| {
            quoted
                .values()
                .filter(|held| matches!(held, Quoted::Read(_)))
                .count()
        };
        assert_eq!(held(&quoted), fit);
        // Each of the others has an entry, so it is not read again, and shows no text.
        assert_eq!(quoted.len(), fit + 5);
        assert_eq!(quoted[&keys[fit]], Quoted::NotHeld);

        // A message that is read again takes the place that it had.
        let again = heavy(0);
        hold_quote(&mut quoted, keys[0], again);
        assert_eq!(held(&quoted), fit);
        assert!(matches!(quoted[&keys[0]], Quoted::Read(_)));
    }

    #[test]
    fn a_quoted_message_that_the_client_cannot_read_is_not_asked_for_again() {
        use junto_client_core::api::ApiError;

        // An answer with a message above the bounds, and an answer in another form.
        let unreadable = ApiError::InvalidResponse {
            status: 200,
            reason: "a message above the bounds of a message",
        };
        assert_eq!(quote_without_answer(&unreadable), Some(Quoted::NotHeld));
        // A request that was not sent, as while a session is renewed, is sent again.
        assert_eq!(quote_without_answer(&ApiError::NotLoggedIn), None);
        assert_eq!(quote_without_answer(&ApiError::Stopped), None);
        // A failure of the server passes: the read is sent again.
        let failing = ApiError::InvalidResponse {
            status: 502,
            reason: "unexpected body",
        };
        assert_eq!(quote_without_answer(&failing), None);
    }

    #[test]
    fn remote_boundary_dates_do_not_panic_view_derivation() {
        // The last moment that the form of dates of the protocol can state, read from the
        // text of a server. One hour east of UTC it lies in the year 10000.
        let scene = scene();
        let mut text =
            serde_json::to_value(scene.message(1, scene.ana, 0, "the last one")).unwrap();
        text["created_at"] = "9999-12-31T23:59:59Z".into();
        text["edited_at"] = "9999-12-31T23:59:59Z".into();
        let last: Message = serde_json::from_value(text).unwrap();
        let timeline = scene.timeline(vec![scene.message(1, scene.ana, 0, "first"), last]);
        for hours in [1, 14, -3] {
            let feed = derive(&Source {
                server_id: scene.server_id,
                model: &scene.model,
                timeline: &timeline,
                names: scene.names.clone(),
                own_roles: &[scene.role],
                may_manage: false,
                administrator: false,
                open_thread: None,
                target: None,
                threads: true,
                quoted: &scene.quoted,
                zone: Zone::Fixed(UtcOffset::from_hms(hours, 0, 0).unwrap()),
                today: Some(morning().date()),
                cards: None,
            });
            let Some(FeedRow::Message(row)) = feed.rows.last() else {
                panic!("no message");
            };
            // East of UTC the moment is shown as the server wrote it, and west of UTC on
            // the clock of the zone.
            let shown = if hours > 0 { "23:59" } else { "20:59" };
            assert_eq!(row.time, shown, "{hours} hours from UTC");
        }
    }

    #[test]
    fn a_message_with_replies_shows_its_thread_and_offers_it_where_the_list_has_threads() {
        let mut scene = scene();
        let mut root = scene.message(1, scene.ana, 0, "root");
        let mut reply = scene.message(2, scene.own, 3, "a reply");
        reply.thread_root = Some(root.id);
        root.reply_count = 1;
        root.last_reply = Some(reply.id);
        scene.quoted.insert(
            (scene.server_id, reply.id),
            Quoted::Read(Arc::new(reply.clone())),
        );
        scene.model.threads = vec![junto_client_core::protocol::message::ThreadState {
            root_id: root.id,
            last_read: 1,
            latest: 2,
            unread: 1,
            mentions: 0,
            following: true,
            follow_setting: None,
        }];
        let timeline = scene.timeline(vec![root.clone()]);
        assert_eq!(named_quotes(&timeline), [reply.id].into_iter().collect());
        let row_of = |threads: bool, open_thread| {
            let feed = derive(&Source {
                server_id: scene.server_id,
                model: &scene.model,
                timeline: &timeline,
                names: scene.names.clone(),
                own_roles: &[],
                may_manage: false,
                administrator: false,
                open_thread,
                target: None,
                threads,
                quoted: &scene.quoted,
                zone: Zone::Fixed(UtcOffset::UTC),
                today: None,
                cards: None,
            });
            feed.rows
                .iter()
                .find_map(|row| match row {
                    FeedRow::Message(row) => Some(row.clone()),
                    _ => None,
                })
                .unwrap()
        };
        let row = row_of(true, Some(root.id));
        assert!(row.can_thread && !row.can_pin);
        let mark = row.thread.unwrap();
        assert_eq!((mark.replies, mark.unread, mark.open), (1, 1, true));
        let last = mark.last.unwrap();
        assert_eq!(
            (last.author.as_str(), last.time.as_str()),
            ("Luca", "09:03")
        );
        // The chat of the call of this device shows the count and offers no thread.
        let row = row_of(false, None);
        assert!(!row.can_thread);
        assert!(
            row.thread
                .is_some_and(|mark| mark.replies == 1 && !mark.open)
        );
    }

    /// One letter for each row: the kind of the row, and for a message whether it is
    /// grouped.
    fn shape(feed: &Feed) -> String {
        feed.rows
            .iter()
            .map(|row| match row {
                FeedRow::Start => 'S',
                FeedRow::Earlier { .. } => 'E',
                FeedRow::Later { .. } => 'L',
                FeedRow::Day(_) => 'D',
                FeedRow::Unread => 'U',
                FeedRow::Message(row) if row.grouped => 'g',
                FeedRow::Message(_) => 'M',
                FeedRow::System(_) => 's',
                FeedRow::Recording(_) => 'R',
                FeedRow::Waiting(row) if row.grouped => 'w',
                FeedRow::Waiting(_) => 'W',
            })
            .collect()
    }

    #[test]
    fn messages_are_grouped_by_author_and_time_under_day_separators() {
        let scene = scene();
        let mut reply = scene.message(6, scene.ana, 12, "a reply");
        let first = scene.message(1, scene.ana, 0, "one");
        reply.reply_to = Some(first.id);
        let timeline = scene.timeline(vec![
            first,
            scene.message(2, scene.ana, 4, "two, within five minutes"),
            scene.message(3, scene.ana, 10, "three, six minutes later"),
            scene.message(4, scene.own, 11, "another author"),
            scene.message(5, scene.ana, 11, "back"),
            reply,
            // The next day in the zone of the list.
            scene.message(7, scene.ana, 16 * 60, "tomorrow"),
            scene.message(8, scene.ana, 16 * 60 + 1, "tomorrow, grouped"),
        ]);
        let feed = scene.feed(&timeline, false);
        assert_eq!(shape(&feed), "SDMgMMMMDMg");
        let FeedRow::Day(day) = &feed.rows[1] else {
            panic!("no day");
        };
        assert_eq!(crate::text::day(*day, feed.today), "Thursday, 8 October");
        let FeedRow::Message(row) = &feed.rows[2] else {
            panic!("no message");
        };
        assert_eq!((row.author.as_str(), row.time.as_str()), ("Ana", "09:00"));
        assert!(!row.own && !row.can_edit && !row.can_delete);
        let FeedRow::Message(own) = &feed.rows[5] else {
            panic!("no message");
        };
        assert!(own.own && own.can_edit && own.can_delete);
        assert_eq!(feed.end_position, Some(8));

        // The zone decides the day: nine hours east, the last two messages of the first
        // day are on the next one.
        let east = derive(&Source {
            server_id: scene.server_id,
            model: &scene.model,
            timeline: &timeline,
            names: scene.names.clone(),
            own_roles: &[],
            may_manage: true,
            administrator: false,
            open_thread: None,
            target: None,
            threads: true,
            quoted: &scene.quoted,
            zone: Zone::Fixed(UtcOffset::from_hms(14, 55, 0).unwrap()),
            today: None,
            cards: None,
        });
        assert_eq!(shape(&east), "SDMgDMMMMMg");
        let FeedRow::Message(row) = &east.rows[2] else {
            panic!("no message");
        };
        assert_eq!(row.time, "23:55");
        // With "manage messages" a message of another person can be deleted, not edited.
        assert!(row.can_delete && !row.can_edit);
        assert_eq!(
            crate::text::day(
                morning().date(),
                Some(morning().date().replace_year(2027).unwrap())
            ),
            "Thursday, 8 October 2026"
        );
    }

    #[test]
    fn only_an_administrator_deletes_a_permanent_message() {
        let scene = scene();
        let permanent = |author| {
            let mut message = scene.message(1, author, 0, "approved");
            message.permanent = true;
            message
        };
        let timeline = scene.timeline(vec![permanent(scene.own), permanent(scene.ana)]);
        let rows = |feed: &Feed| -> Vec<(bool, bool)> {
            feed.rows
                .iter()
                .filter_map(|row| match row {
                    FeedRow::Message(row) => Some((row.can_edit, row.can_delete)),
                    _ => None,
                })
                .collect()
        };
        // Neither its author nor a person with "manage messages" deletes it.
        assert_eq!(rows(&scene.feed(&timeline, true)), [(false, false); 2]);
        let source = |administrator| Source {
            server_id: scene.server_id,
            model: &scene.model,
            timeline: &timeline,
            names: scene.names.clone(),
            own_roles: &[],
            may_manage: false,
            administrator,
            open_thread: None,
            target: None,
            threads: true,
            quoted: &scene.quoted,
            zone: Zone::Fixed(UtcOffset::UTC),
            today: None,
            cards: None,
        };
        // An administrator deletes every permanent message of a channel and edits none.
        assert_eq!(rows(&derive(&source(true))), [(false, true); 2]);
        assert_eq!(rows(&derive(&source(false))), [(false, false); 2]);
    }

    #[test]
    fn the_unread_line_stands_before_the_first_unread_message_of_another_person() {
        let scene = scene();
        let messages = vec![
            scene.message(1, scene.ana, 0, "read"),
            scene.message(2, scene.ana, 1, "read too"),
            scene.message(3, scene.own, 2, "own, after the read position"),
            scene.message(4, scene.ana, 3, "unread"),
            scene.message(5, scene.ana, 4, "unread too"),
        ];
        let mut timeline = scene.timeline(messages);
        timeline.unread_after = Some(2);
        assert_eq!(shape(&scene.feed(&timeline, false)), "SDMgMUMg");
        // Without a read position there is no line.
        timeline.unread_after = None;
        assert_eq!(shape(&scene.feed(&timeline, false)), "SDMgMMg");
        // When every loaded message is unread and the history has more before, no line
        // is drawn above them all.
        timeline.unread_after = Some(0);
        timeline.more_before = true;
        assert_eq!(shape(&scene.feed(&timeline, false)), "EDMgMMg");
        // The history has more after the loaded messages: the list does not reach its end.
        timeline.more_after = true;
        timeline.reading.newer = true;
        let feed = scene.feed(&timeline, false);
        assert_eq!(shape(&feed), "EDMgMMgL");
        assert_eq!(feed.end_position, None);
        assert_eq!(feed.rows.last(), Some(&FeedRow::Later { reading: true }));
    }

    #[test]
    fn replies_system_lines_deleted_messages_and_mentions_are_derived() {
        let mut scene = scene();
        let first = scene.message(1, scene.ana, 0, "The **export** worker ran out of memory.");
        let mut with_file = scene.message(2, scene.ana, 1, "");
        with_file.attachments = vec![Attachment {
            id: junto_client_core::protocol::ids::AttachmentId::generate(),
            name: "notes.pdf".to_owned(),
            bytes: 10,
            sha256: String::new(),
            media_type: "application/octet-stream".to_owned(),
            width: None,
            height: None,
        }];
        let mut marker = scene.message(3, scene.ana, 2, "");
        marker.deleted = true;
        marker.document = None;
        marker.author_id = None;
        marker.author_name = None;
        let reply = |position, to: &Message| {
            let mut reply = scene.message(position, scene.own, 3, "answer");
            reply.reply_to = Some(to.id);
            reply
        };
        let (to_first, to_file, to_marker) =
            (reply(4, &first), reply(5, &with_file), reply(6, &marker));
        let outside = MessageId::generate();
        let gone = MessageId::generate();
        let read = MessageId::generate();
        let mut to_outside = scene.message(7, scene.own, 3, "answer");
        to_outside.reply_to = Some(outside);
        let mut to_gone = scene.message(8, scene.own, 3, "answer");
        to_gone.reply_to = Some(gone);
        let mut to_read = scene.message(9, scene.own, 3, "answer");
        to_read.reply_to = Some(read);
        let mut system = scene.message(10, scene.ana, 4, "");
        system.author_id = None;
        system.author_name = None;
        system.document = None;
        system.system = Some(SystemMessage {
            action: "call_microphone_blocked".to_owned(),
            subject_id: Some(scene.own),
            actor_id: Some(scene.ana),
            recording_id: None,
            duration_seconds: None,
        });
        let mut unknown = system.clone();
        unknown.id = MessageId::generate();
        unknown.system = Some(SystemMessage {
            action: "made_up_later".to_owned(),
            subject_id: None,
            actor_id: None,
            recording_id: None,
            duration_seconds: None,
        });
        let mention = |position, text: &str, target| {
            let at = text.find('@').unwrap();
            let mut message = scene.message(position, scene.ana, 5, "");
            message.document = Some(compose::parse(
                text,
                &[compose::Mention {
                    range: at..at + 2,
                    target,
                }],
            ));
            message
        };
        let timeline = scene.timeline(vec![
            first.clone(),
            with_file,
            marker,
            to_first,
            to_file,
            to_marker,
            to_outside,
            to_gone,
            to_read,
            system,
            unknown,
            mention(11, "hello @x", compose::MentionTarget::Account(scene.own)),
            mention(12, "hello @x", compose::MentionTarget::Account(scene.ana)),
            mention(13, "hello @x", compose::MentionTarget::Role(scene.role)),
            mention(14, "hello @x", compose::MentionTarget::Everyone),
        ]);
        assert_eq!(
            missing_quotes(scene.server_id, &timeline, &scene.quoted).len(),
            3
        );
        scene.quoted.insert((scene.server_id, gone), Quoted::Gone);
        let mut earlier = first.clone();
        earlier.id = read;
        earlier.document = Some(compose::parse("an earlier message", &[]));
        scene
            .quoted
            .insert((scene.server_id, read), Quoted::Read(Arc::new(earlier)));
        assert_eq!(
            missing_quotes(scene.server_id, &timeline, &scene.quoted),
            [outside]
        );
        let feed = scene.feed(&timeline, false);
        let quotes: Vec<QuoteText> = feed
            .rows
            .iter()
            .filter_map(|row| match row {
                FeedRow::Message(row) => row.reply.as_ref().map(|quote| quote.text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            quotes,
            [
                QuoteText::Text("The export worker ran out of memory.".to_owned()),
                QuoteText::File("notes.pdf".to_owned()),
                QuoteText::Deleted,
                QuoteText::Unknown,
                QuoteText::Deleted,
                QuoteText::Text("an earlier message".to_owned()),
            ]
        );
        let systems: Vec<(SystemKind, String)> = feed
            .rows
            .iter()
            .filter_map(|row| match row {
                FeedRow::System(row) => Some((
                    row.kind,
                    row.parts
                        .iter()
                        .map(|part| {
                            if part.strong {
                                format!("[{}]", part.text)
                            } else {
                                part.text.clone()
                            }
                        })
                        .collect(),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            systems,
            [
                (
                    SystemKind::Plain,
                    "[Ana] closed the microphone of [Luca]".to_owned()
                ),
                (
                    SystemKind::Plain,
                    "An event that this version of Junto does not know".to_owned()
                ),
            ]
        );
        let mentioned: Vec<bool> = feed
            .rows
            .iter()
            .rev()
            .take(4)
            .rev()
            .filter_map(|row| match row {
                FeedRow::Message(row) => Some(row.mentions_own),
                _ => None,
            })
            .collect();
        assert_eq!(mentioned, [true, false, true, true]);
        // A deleted message can be neither edited nor deleted, and ends a group.
        let FeedRow::Message(marker) = &feed.rows[4] else {
            panic!("no marker");
        };
        assert!(marker.message.deleted && !marker.can_delete && !marker.grouped);
    }

    #[test]
    fn the_post_of_a_finished_recording_is_a_card_where_the_list_shows_cards() {
        let scene = scene();
        let recording_id = RecordingId::generate();
        let mut post = scene.message(1, scene.ana, 0, "");
        post.author_id = None;
        post.author_name = None;
        post.document = None;
        post.system = Some(SystemMessage {
            action: "recording_ready".to_owned(),
            subject_id: None,
            actor_id: None,
            recording_id: Some(recording_id),
            duration_seconds: None,
        });
        let mut started = post.clone();
        started.id = MessageId::generate();
        started.system.as_mut().unwrap().action = "recording_started".to_owned();
        let timeline = scene.timeline(vec![started, post]);
        // A list without cards, such as the chat of the call of this device, shows the
        // line.
        assert_eq!(shape(&scene.feed(&timeline, false)), "SDss");

        let (downloads, texts) = (HashMap::new(), HashMap::new());
        let (removing, refused) = (HashSet::new(), HashMap::new());
        let cards = Cards {
            channel: "Daily",
            channel_id: ChannelId::generate(),
            accounts: &scene.names.accounts,
            may_remove: true,
            downloads: &downloads,
            texts: &texts,
            removing: &removing,
            requesting: &removing,
            refused: &refused,
        };
        let feed = derive(&Source {
            server_id: scene.server_id,
            model: &scene.model,
            timeline: &timeline,
            names: scene.names.clone(),
            own_roles: &[],
            may_manage: false,
            administrator: false,
            open_thread: None,
            target: None,
            threads: true,
            quoted: &scene.quoted,
            zone: Zone::Fixed(UtcOffset::UTC),
            today: None,
            cards: Some(&cards),
        });
        // The line of the start stays a line, and the post of the files is the card.
        assert_eq!(shape(&feed), "SDsR");
        let FeedRow::Recording(row) = &feed.rows[3] else {
            panic!("the post is a card");
        };
        assert_eq!(row.card.recording_id, recording_id);
        assert!(row.card.may_remove);
        // The client core holds no such recording yet.
        assert_eq!(row.card.state, recorded::CardState::Reading);
        assert_eq!(feed.rows[3].position(), Some(1));
    }

    #[test]
    fn waiting_messages_follow_the_end_of_the_history() {
        let mut scene = scene();
        let waiting = |id, text: &str, state| {
            let mut pending = PendingMessage::new(
                id,
                PlaceId::of(&scene.place),
                compose::parse(text, &[]),
                None,
                Vec::new(),
                morning() + Duration::minutes(3),
            );
            pending.state = state;
            Arc::new(pending)
        };
        scene.model.outbox = vec![
            waiting(1, "on its way", SendState::Sending),
            waiting(2, "failed", SendState::Failed(SendFailure::NotSent)),
        ];
        scene.model.message_states = vec![MessageState {
            place: scene.place,
            last_read: 1,
            latest: 2,
            unread: 1,
            mentions: 0,
            unread_sign: true,
            notification_mentions: 0,
            muted: false,
        }];
        let mut timeline = scene.timeline(vec![
            scene.message(1, scene.ana, 0, "one"),
            scene.message(2, scene.own, 1, "two"),
        ]);
        let feed = scene.feed(&timeline, false);
        assert_eq!(shape(&feed), "SDMMww");
        assert_eq!(feed.unread, 1);
        assert!(can_send_again(SendState::Unconfirmed));
        assert!(can_send_again(SendState::Failed(SendFailure::NotSent)));
        assert!(!can_send_again(SendState::Failed(SendFailure::File)));
        assert!(!can_send_again(SendState::Sending));
        // The list does not reach the end: the waiting messages are not drawn in it.
        timeline.more_after = true;
        assert_eq!(shape(&scene.feed(&timeline, false)), "SDMML");
        // A list whose first page has not arrived has no rows.
        let mut unloaded = scene.timeline(Vec::new());
        unloaded.loaded = false;
        assert_eq!(shape(&scene.feed(&unloaded, false)), "");
        // A place that the server does not give any more has no history. Its waiting
        // messages stay, so that the person can send them again or discard them.
        unloaded.failed = Some(ReadFailure::NotFound);
        assert_eq!(shape(&scene.feed(&unloaded, false)), "DWw");
    }

    #[test]
    fn rows_that_did_not_change_are_the_same_rows() {
        let scene = scene();
        let mut timeline = scene.timeline(vec![
            scene.message(1, scene.ana, 0, "one"),
            scene.message(2, scene.own, 1, "two"),
        ]);
        let before = scene.feed(&timeline, false);
        assert_eq!(before.rows, scene.feed(&timeline, false).rows);
        // A message that changed is another row; the rows around it are the same.
        let mut edited = timeline.messages[1].as_ref().clone();
        edited.edited_at = Some(morning());
        timeline.messages[1] = Arc::new(edited);
        let after = scene.feed(&timeline, false);
        let same: Vec<bool> = before
            .rows
            .iter()
            .zip(after.rows.iter())
            .map(|(one, other)| one == other)
            .collect();
        assert_eq!(same, [true, true, true, false]);
    }

    /// The derivation of a list with 10,000 loaded messages. The bound is generous: the
    /// test fails when the derivation stops being linear, not when a machine is slow.
    #[test]
    fn ten_thousand_messages_are_derived_in_a_few_milliseconds() {
        let scene = scene();
        let messages: Vec<Message> = (0..10_000u64)
            .map(|number| {
                let author = if number % 3 == 0 {
                    scene.own
                } else {
                    scene.ana
                };
                let mut message = scene.message(
                    number + 1,
                    author,
                    number as i64,
                    "A message with **some** formatting and `code` in it.",
                );
                if number % 7 == 0 && number > 0 {
                    message.reply_to = Some(MessageId::generate());
                }
                message
            })
            .collect();
        let mut timeline = scene.timeline(messages);
        let first = timeline.messages[0].id;
        for index in (1..10_000).step_by(11) {
            let mut reply = timeline.messages[index].as_ref().clone();
            reply.reply_to = Some(first);
            timeline.messages[index] = Arc::new(reply);
        }
        let started = std::time::Instant::now();
        let rounds = 5;
        let mut rows = 0;
        for _ in 0..rounds {
            rows = scene.feed(&timeline, false).rows.len();
        }
        let each = started.elapsed() / rounds;
        println!("derivation of 10,000 messages: {each:?} for {rows} rows");
        assert!(rows > 10_000);
        assert!(each < std::time::Duration::from_millis(500), "{each:?}");
    }
    #[test]
    fn the_thread_line_names_the_first_authors_of_the_replies_that_the_server_stated() {
        let scene = scene();
        let mut root = scene.message(1, scene.ana, 0, "root");
        root.reply_count = 5;
        let others: Vec<AccountId> = (0..3).map(|_| AccountId::generate()).collect();
        let authors = |root: &Message| {
            let timeline = scene.timeline(vec![root.clone()]);
            scene
                .feed(&timeline, false)
                .rows
                .iter()
                .find_map(|row| match row {
                    FeedRow::Message(row) => row.thread.as_ref().map(|mark| mark.authors.clone()),
                    _ => None,
                })
                .unwrap()
        };
        // A server of before the field states none.
        assert!(authors(&root).is_empty());
        root.reply_authors = vec![scene.own, others[0]];
        assert_eq!(authors(&root), [scene.own, others[0]]);
        // The server states at most three; a longer list is cut.
        root.reply_authors = vec![others[2], scene.own, others[0], others[1]];
        assert_eq!(authors(&root), [others[2], scene.own, others[0]]);
    }

    #[test]
    fn the_lines_of_a_direct_call_state_who_called_who_did_not_answer_and_the_length() {
        let scene = scene();
        let line = |action: &str, actor, subject, duration_seconds| {
            let system = SystemMessage {
                action: action.to_owned(),
                subject_id: Some(subject),
                actor_id: Some(actor),
                recording_id: None,
                duration_seconds,
            };
            let (kind, parts) = system_parts(&system, &scene.names);
            let text: String = parts
                .iter()
                .map(|part| {
                    if part.strong {
                        format!("[{}]", part.text)
                    } else {
                        part.text.clone()
                    }
                })
                .collect();
            (kind, text)
        };
        let (ana, own) = (scene.ana, scene.own);
        assert_eq!(
            line("direct_call_started", ana, own, None),
            (SystemKind::Start, "[Ana] started a call".to_owned())
        );
        // The actor of the line is the caller, and the subject the account that was
        // called (ADR 0518).
        assert_eq!(
            line("direct_call_not_answered", ana, own, None),
            (
                SystemKind::Plain,
                "[Luca] did not answer the call of [Ana]".to_owned()
            )
        );
        assert_eq!(
            line("direct_call_declined", own, ana, None),
            (SystemKind::Plain, "[Luca] declined the call".to_owned())
        );
        let ended = |seconds| line("direct_call_ended", ana, own, seconds).1;
        assert_eq!(
            ended(Some(252)),
            "The call ended after 4 minutes 12 seconds"
        );
        assert_eq!(ended(Some(1)), "The call ended after 1 second");
        assert_eq!(ended(Some(60)), "The call ended after 1 minute 0 seconds");
        assert_eq!(ended(Some(3_725)), "The call ended after 1 hour 2 minutes");
        // A server that states no length.
        assert_eq!(ended(None), "The call ended");
    }
}
