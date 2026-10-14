//! What the window holds about the threads that the account follows (ADR 0412, ADR 0413):
//! the follow state of the thread of the side panel with the request that changes it, the
//! followed threads of the shown channel, and the roots that were read for their titles.
//!
//! The types are plain data: each one names the requests to send and takes their answers.
//! [`AppState`] sends the requests. A request has a number, so that an answer that arrives
//! late, for an earlier request or for a thread or a channel that is not shown any more,
//! changes nothing.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::Context;
use junto_client_core::ServerHandle;
use junto_client_core::api::{ApiError, ErrorCategory};
use junto_client_core::flow::FlowError;
use junto_client_core::flow::access::AccessState;
use junto_client_core::live::Notice;
use junto_client_core::model::{Named, PlaceId, ServerModel};
use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::ids::{AccountId, ChannelId, MessageId, ServerId};
use junto_client_core::protocol::message::{FollowedThreads, Message, ThreadState};

use super::AppState;
use super::view::{FollowControl, MoreFollowed, ShellView};
use crate::text;

/// The followed threads without something unread that a channel lists before the person
/// asks for the further ones.
pub const FOLLOWED_SHOWN: usize = 8;
/// The threads of one page of the followed threads of a channel.
const PAGE: u32 = 50;
/// The most threads that the server gives in one page (ADR 0413).
const LARGEST_PAGE: u32 = 200;
/// The roots that are read at one time.
const ROOT_READS: usize = 4;
/// The roots that are held for their titles.
const MOST_ROOTS: usize = 200;

/// The follow state of a thread as the server told it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FollowRead {
    #[default]
    NotRead,
    Reading,
    Read {
        following: bool,
        /// The choice of the account. Absent: the rule of the server decides.
        setting: Option<bool>,
    },
    Failed,
}

/// A request about the thread of the side panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThreadAsk {
    pub number: u64,
    pub server_id: ServerId,
    pub root_id: MessageId,
}

/// Whether the account follows the thread that the side panel shows, the change that the
/// person asked for, and why the server did not make the last change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThreadFollow {
    thread: Option<(ServerId, MessageId)>,
    read: FollowRead,
    /// The number of the read that is on its way.
    reading: Option<u64>,
    /// The state is read again: events were missed, or the last read failed.
    again: bool,
    /// The number of the change that is on its way.
    asking: Option<u64>,
    /// An event told the state while the change was on its way.
    overtaken: bool,
    refused: Option<String>,
    /// The number of the last request. It goes on from one thread to the next, so that no
    /// two requests have one number.
    asks: u64,
}

impl ThreadFollow {
    /// The thread that the side panel shows now, or none. With another thread, what was
    /// held of the one before ends: its state, its requests and its refusal.
    pub fn show(&mut self, thread: Option<(ServerId, MessageId)>) {
        if self.thread != thread {
            self.start(thread);
        }
    }

    fn start(&mut self, thread: Option<(ServerId, MessageId)>) {
        *self = Self {
            thread,
            asks: self.asks,
            ..Self::default()
        };
    }

    /// The server has another account: what was read belongs to the account before.
    fn forget(&mut self, server_id: ServerId) {
        if self.server() == Some(server_id) {
            self.start(self.thread);
        }
    }

    /// The server of the thread.
    pub fn server(&self) -> Option<ServerId> {
        self.thread.map(|(server_id, _)| server_id)
    }

    /// The read to send now: the first one of the thread, or one more when the state is
    /// read again. One read is on its way at a time, and none beside a change, whose
    /// answer tells the state.
    pub fn next_read(&mut self) -> Option<ThreadAsk> {
        let (server_id, root_id) = self.thread?;
        if self.reading.is_some() || self.asking.is_some() {
            return None;
        }
        if self.read != FollowRead::NotRead && !self.again {
            return None;
        }
        self.again = false;
        // What was read before stays shown while it is read again.
        if !matches!(self.read, FollowRead::Read { .. }) {
            self.read = FollowRead::Reading;
        }
        self.asks += 1;
        self.reading = Some(self.asks);
        Some(ThreadAsk {
            number: self.asks,
            server_id,
            root_id,
        })
    }

    /// The answer to a read: whether the account follows the thread and its choice, or
    /// nothing when the read failed. True when the answer is the one that was waited for.
    pub fn read_answered(&mut self, number: u64, state: Option<(bool, Option<bool>)>) -> bool {
        if self.reading != Some(number) {
            return false;
        }
        self.reading = None;
        match state {
            Some((following, setting)) => self.read = FollowRead::Read { following, setting },
            None if self.read == FollowRead::Reading => self.read = FollowRead::Failed,
            // What was read before stays shown.
            None => {}
        }
        true
    }

    /// An event told the follow state of a thread. A read that is on its way was sent
    /// before the event: its answer is not used.
    pub fn told(
        &mut self,
        server_id: ServerId,
        root_id: MessageId,
        following: bool,
        setting: Option<bool>,
    ) -> bool {
        if self.thread != Some((server_id, root_id)) {
            return false;
        }
        self.read = FollowRead::Read { following, setting };
        self.reading = None;
        self.again = false;
        self.overtaken = self.asking.is_some();
        true
    }

    /// The person asks for a change. Nothing while the state is not read, and while
    /// another change is on its way. What was told about the change before is put away.
    pub fn ask(&mut self) -> Option<ThreadAsk> {
        let (server_id, root_id) = self.thread?;
        if self.asking.is_some() || !matches!(self.read, FollowRead::Read { .. }) {
            return None;
        }
        self.asks += 1;
        self.asking = Some(self.asks);
        self.overtaken = false;
        self.refused = None;
        Some(ThreadAsk {
            number: self.asks,
            server_id,
            root_id,
        })
    }

    /// The answer to a change: the state that holds now, nothing when the server made the
    /// change and did not tell the state, or the sentence of the refusal. True when the
    /// answer is the one that was waited for.
    pub fn ask_answered(
        &mut self,
        number: u64,
        answer: Result<Option<(bool, Option<bool>)>, String>,
    ) -> bool {
        if self.asking != Some(number) {
            return false;
        }
        self.asking = None;
        let overtaken = std::mem::take(&mut self.overtaken);
        match answer {
            // An event told the state during the request, and the answer can be older
            // than the event: the state of the event stays, and the server is asked
            // which one holds.
            Ok(_) if overtaken => self.again = true,
            Ok(state) => {
                self.read = match state {
                    Some((following, setting)) => FollowRead::Read { following, setting },
                    None => FollowRead::NotRead,
                };
                // A read that was sent before the change is older than its answer.
                self.reading = None;
            }
            Err(refusal) => self.refused = Some(refusal),
        }
        true
    }

    /// Events of the server were not told: the state is read again.
    fn stale(&mut self, server_id: ServerId) -> bool {
        if self.server() != Some(server_id) {
            return false;
        }
        self.again = true;
        true
    }

    /// An event of the server arrived: a read that failed is sent again.
    fn retry(&mut self, server_id: ServerId) -> bool {
        if self.server() != Some(server_id) || self.read != FollowRead::Failed || self.again {
            return false;
        }
        self.again = true;
        true
    }

    /// The control of the thread panel. Absent for another thread, and until the server
    /// has told the state.
    pub fn control(
        &self,
        server_id: ServerId,
        root_id: MessageId,
        read_only: bool,
    ) -> Option<FollowControl> {
        if self.thread != Some((server_id, root_id)) {
            return None;
        }
        let FollowRead::Read { following, setting } = self.read else {
            return None;
        };
        Some(FollowControl {
            following,
            setting,
            busy: self.asking.is_some(),
            read_only,
            refused: self.refused.clone(),
        })
    }
}

/// What a read of the followed threads depends on in the state of a server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Seen {
    /// The unread threads of the state: the root, the newest reply, the count of the
    /// unread replies, and whether the account follows the thread.
    pub unread: Vec<(MessageId, u64, u64, bool)>,
    /// The root of the open thread with the count of its replies.
    pub open: Option<(MessageId, u64)>,
}

impl Seen {
    pub fn of(model: &ServerModel, open: Option<(ChannelId, MessageId)>) -> Self {
        Self {
            unread: model
                .threads
                .iter()
                .map(|state| (state.root_id, state.latest, state.unread, state.following))
                .collect(),
            open: open.and_then(|(channel_id, root_id)| {
                let place = PlaceId::Thread {
                    channel_id,
                    root_id,
                };
                let root = model.timeline(place)?.root.as_ref()?;
                Some((root_id, root.reply_count))
            }),
        }
    }
}

/// A read of the followed threads of a channel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListRead {
    pub number: u64,
    pub server_id: ServerId,
    pub channel_id: ChannelId,
    /// The continuation of the page before. Absent: the list is read from its start.
    pub before: Option<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ListReading {
    number: u64,
    /// The read asks for the page after the held ones.
    further: bool,
    /// The read was sent for another channel or another account: its answer is not used.
    dropped: bool,
}

/// The threads of the shown channel that the account follows, as far as their pages were
/// read (ADR 0413).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FollowedList {
    channel: Option<(ServerId, ChannelId)>,
    /// In the order of the server: the thread with the newest reply first.
    threads: Vec<ThreadState>,
    /// The server lists more followed threads after the held ones.
    more: bool,
    next: Option<String>,
    /// The person asked for the further threads. It ends when another channel is shown.
    expanded: bool,
    reading: Option<ListReading>,
    /// The list is read from its start: a trigger arrived.
    wanted: bool,
    /// The page after the held ones is read.
    wanted_further: bool,
    seen: Seen,
    /// The number of the last read.
    reads: u64,
}

/// The threads that a read of the list from its start asks for: as many pages as are
/// held, so that a list that the person read further stays as long as it was.
fn refresh_limit(held: usize) -> u32 {
    let pages = u32::try_from(held).unwrap_or(LARGEST_PAGE).div_ceil(PAGE);
    pages.saturating_mul(PAGE).clamp(PAGE, LARGEST_PAGE)
}

impl FollowedList {
    /// The channel that is shown now, or none. With another channel the list starts
    /// empty and is read, and the answer of a read that is on its way is not used.
    pub fn show(&mut self, channel: Option<(ServerId, ChannelId)>) {
        if self.channel != channel {
            self.start(channel);
        }
    }

    fn start(&mut self, channel: Option<(ServerId, ChannelId)>) {
        *self = Self {
            channel,
            reading: self.reading.map(|reading| ListReading {
                dropped: true,
                ..reading
            }),
            wanted: channel.is_some(),
            reads: self.reads,
            ..Self::default()
        };
    }

    /// The server has another account: the list belongs to the account before.
    fn forget(&mut self, server_id: ServerId) {
        if self.server() == Some(server_id) {
            self.start(self.channel);
        }
    }

    fn server(&self) -> Option<ServerId> {
        self.channel.map(|(server_id, _)| server_id)
    }

    /// Something happened on the server after which the list can differ: it is read from
    /// its start. True when the list is of this server.
    pub fn trigger(&mut self, server_id: ServerId) -> bool {
        if self.server() != Some(server_id) {
            return false;
        }
        self.wanted = true;
        true
    }

    /// An event told whether the account follows a thread. A thread that is not followed
    /// any more leaves the list at once; the server states the rest.
    fn told(&mut self, server_id: ServerId, root_id: MessageId, following: bool) -> bool {
        if !self.trigger(server_id) {
            return false;
        }
        // A read that is on its way was sent before the event: its answer is not used.
        // The further page that the person asked for is read after the list.
        if let Some(reading) = &mut self.reading {
            self.wanted_further |= reading.further && !reading.dropped;
            reading.dropped = true;
        }
        if !following {
            self.threads.retain(|held| held.root_id != root_id);
        }
        true
    }

    /// Takes what the state of the server holds now. The list is read again when the
    /// unread threads differ from the ones of the call before, and when the open thread
    /// has another count of replies than before.
    pub fn observe(&mut self, seen: Seen) {
        if self.channel.is_none() {
            return;
        }
        let replied = match (self.seen.open, seen.open) {
            (Some((before, was)), Some((now, is))) => before == now && was != is,
            _ => false,
        };
        if replied || self.seen.unread != seen.unread {
            self.wanted = true;
        }
        self.seen = seen;
    }

    /// The person asks for the further followed threads: the held ones are all listed,
    /// and the next page is read when the server has one.
    pub fn expand(&mut self) -> bool {
        if self.channel.is_none() {
            return false;
        }
        self.expanded = true;
        self.wanted_further = self.more;
        true
    }

    /// The read to send now. One read is on its way at a time; the list from its start
    /// comes before a further page.
    pub fn next_read(&mut self) -> Option<ListRead> {
        let (server_id, channel_id) = self.channel?;
        if self.reading.is_some() {
            return None;
        }
        let (further, before, limit) = if self.wanted {
            self.wanted = false;
            (false, None, refresh_limit(self.threads.len()))
        } else if self.wanted_further {
            self.wanted_further = false;
            let before = self.next.clone().filter(|_| self.more)?;
            (true, Some(before), PAGE)
        } else {
            return None;
        };
        self.reads += 1;
        self.reading = Some(ListReading {
            number: self.reads,
            further,
            dropped: false,
        });
        Some(ListRead {
            number: self.reads,
            server_id,
            channel_id,
            before,
            limit,
        })
    }

    /// The answer to a read: its page, or nothing when the read failed. A failed read
    /// keeps what is listed, and the next trigger reads again. True when the answer is
    /// the one that was waited for: the next read can be sent.
    pub fn answered(&mut self, number: u64, page: Option<FollowedThreads>) -> bool {
        let Some(reading) = self.reading.filter(|reading| reading.number == number) else {
            return false;
        };
        self.reading = None;
        let Some(page) = page.filter(|_| !reading.dropped) else {
            return true;
        };
        if !reading.further {
            self.threads.clear();
        }
        // A thread is listed once, whatever a page repeats.
        for thread in page.threads {
            if self
                .threads
                .iter()
                .all(|held| held.root_id != thread.root_id)
            {
                self.threads.push(thread);
            }
        }
        self.more = page.more && page.next.is_some();
        self.next = page.next;
        true
    }

    /// The followed threads of a channel that its rows list after the threads in
    /// `listed`, which the rows before them show as unread: the first
    /// [`FOLLOWED_SHOWN`] of them, or all of them after the person asked for the further
    /// ones. With them, the row that offers the further ones. `open`: the thread of the
    /// side panel, which has a row of its own when it is not among the listed ones.
    pub fn listed(
        &self,
        channel: (ServerId, ChannelId),
        listed: &[MessageId],
        open: Option<MessageId>,
    ) -> (Vec<&ThreadState>, Option<MoreFollowed>) {
        if self.channel != Some(channel) {
            return (Vec::new(), None);
        }
        let mut further: Vec<&ThreadState> = self
            .threads
            .iter()
            .filter(|state| !listed.contains(&state.root_id))
            .collect();
        let shown = if self.expanded {
            further.len()
        } else {
            further.len().min(FOLLOWED_SHOWN)
        };
        let hidden = further
            .split_off(shown)
            .into_iter()
            .filter(|state| Some(state.root_id) != open)
            .count();
        let more = if self.more {
            Some(MoreFollowed { count: None })
        } else {
            (hidden > 0).then_some(MoreFollowed {
                count: Some(hidden),
            })
        };
        (further, more)
    }
}

/// What the server answered to the read of a root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootAnswer {
    Read(Arc<Message>),
    /// The server has no such message for the account.
    Refused,
    /// The server did not answer.
    Failed,
}

impl RootAnswer {
    fn of(read: Result<Message, ApiError>) -> Self {
        match read {
            Ok(message) => Self::Read(Arc::new(message)),
            Err(error) if error.code().is_some() => Self::Refused,
            Err(_) => Self::Failed,
        }
    }
}

/// A read of the root of a followed thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootRead {
    pub number: u64,
    pub server_id: ServerId,
    pub root_id: MessageId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Root {
    /// Absent: the server did not give the message. The thread has the neutral title.
    message: Option<Arc<Message>>,
    /// False after events were missed: the root is read again when a row needs it, and
    /// what was read before is shown until then.
    current: bool,
    /// When a row needed the root last, for the bound of the cache.
    used: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RootAsked {
    number: u64,
    /// The message changed, or events were missed, after the read was sent: its answer
    /// is not kept. The read keeps its place among the reads until it ends.
    outdated: bool,
}

/// The roots of followed threads that were read for their titles, because the state of
/// the server did not hold them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Roots {
    held: HashMap<(ServerId, MessageId), Root>,
    asked: HashMap<(ServerId, MessageId), RootAsked>,
    clock: u64,
    /// The number of the last read.
    reads: u64,
}

impl Roots {
    /// The root of a thread as it was read.
    pub fn get(&self, server_id: ServerId, root_id: MessageId) -> Option<&Message> {
        self.held.get(&(server_id, root_id))?.message.as_deref()
    }

    /// Takes the roots that the rows of a server need now, in the order of the rows, and
    /// returns the reads to send: a root is read once, and at most [`ROOT_READS`] reads
    /// are on their way. Only the first [`MOST_ROOTS`] rows get a root: with more rows
    /// than the cache holds, each answer would drop a root that another row needs, and
    /// the reads would not end.
    pub fn next_reads(&mut self, server_id: ServerId, wanted: &[MessageId]) -> Vec<RootRead> {
        self.clock += 1;
        let mut reads = Vec::new();
        for root_id in wanted.iter().take(MOST_ROOTS) {
            let key = (server_id, *root_id);
            let current = self.held.get_mut(&key).is_some_and(|root| {
                root.used = self.clock;
                root.current
            });
            if current || self.asked.contains_key(&key) || self.asked.len() >= ROOT_READS {
                continue;
            }
            self.reads += 1;
            self.asked.insert(
                key,
                RootAsked {
                    number: self.reads,
                    outdated: false,
                },
            );
            reads.push(RootRead {
                number: self.reads,
                server_id,
                root_id: *root_id,
            });
        }
        reads
    }

    /// The answer to a read. True when the answer is the one that was waited for: a
    /// further read can be sent.
    pub fn answered(&mut self, read: &RootRead, answer: RootAnswer) -> bool {
        let key = (read.server_id, read.root_id);
        let Some(asked) = self
            .asked
            .get(&key)
            .copied()
            .filter(|asked| asked.number == read.number)
        else {
            return false;
        };
        self.asked.remove(&key);
        if asked.outdated {
            return true;
        }
        let message = match answer {
            RootAnswer::Read(message) => Some(message),
            RootAnswer::Refused => None,
            // What was read before the events were missed stays.
            RootAnswer::Failed => self.held.get(&key).and_then(|root| root.message.clone()),
        };
        self.clock += 1;
        self.held.insert(
            key,
            Root {
                message,
                current: true,
                used: self.clock,
            },
        );
        while self.held.len() > MOST_ROOTS {
            let oldest = self
                .held
                .iter()
                .min_by_key(|(key, root)| (root.used, key.1))
                .map(|(key, _)| *key);
            let Some(oldest) = oldest else {
                break;
            };
            self.held.remove(&oldest);
        }
        true
    }

    /// An event named a message: what was read of it is dropped, and it is read again
    /// when a row needs it.
    fn changed(&mut self, server_id: ServerId, message_id: MessageId) -> bool {
        let key = (server_id, message_id);
        let held = self.held.remove(&key).is_some();
        let asked = self.asked.get_mut(&key).is_some_and(|asked| {
            asked.outdated = true;
            true
        });
        held || asked
    }

    /// Events of the server were not told: every root can have changed. A root that the
    /// server did not give is dropped; the others are shown until they are read again.
    fn stale(&mut self, server_id: ServerId) -> bool {
        let before = self.held.len();
        self.held
            .retain(|key, root| key.0 != server_id || root.message.is_some());
        let mut changed = self.held.len() != before;
        for (key, root) in &mut self.held {
            if key.0 == server_id && root.current {
                root.current = false;
                changed = true;
            }
        }
        self.outdate(server_id) || changed
    }

    /// The server has another account: a message is readable for one account and not
    /// for another.
    fn forget(&mut self, server_id: ServerId) {
        self.held.retain(|key, _| key.0 != server_id);
        self.outdate(server_id);
    }

    fn outdate(&mut self, server_id: ServerId) -> bool {
        let mut changed = false;
        for (key, asked) in &mut self.asked {
            if key.0 == server_id && !asked.outdated {
                asked.outdated = true;
                changed = true;
            }
        }
        changed
    }

    /// Drops what is held of every server that is not one of these.
    fn retain(&mut self, servers: &HashSet<ServerId>) {
        self.held.retain(|key, _| servers.contains(&key.0));
        self.asked.retain(|key, _| servers.contains(&key.0));
    }
}

/// What the window holds about followed threads.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Followed {
    pub thread: ThreadFollow,
    pub list: FollowedList,
    pub roots: Roots,
    /// The account that the entries of a server were read for.
    owners: HashMap<ServerId, AccountId>,
}

impl Followed {
    /// Drops what is held of every server that is not one of these.
    pub fn retain(&mut self, servers: &HashSet<ServerId>) {
        self.owners
            .retain(|server_id, _| servers.contains(server_id));
        self.roots.retain(servers);
    }

    /// Tells the account of the session of a server. With another account than the one
    /// that the entries were read for, everything of the server is read anew.
    pub fn belongs(&mut self, server_id: ServerId, account_id: AccountId) {
        let before = self.owners.insert(server_id, account_id);
        if before.is_some_and(|before| before != account_id) {
            self.thread.forget(server_id);
            self.list.forget(server_id);
            self.roots.forget(server_id);
        }
    }

    /// An event told whether the account follows a thread.
    pub fn told(
        &mut self,
        server_id: ServerId,
        root_id: MessageId,
        following: bool,
        setting: Option<bool>,
    ) -> bool {
        let thread = self.thread.told(server_id, root_id, following, setting);
        let list = self.list.told(server_id, root_id, following);
        thread || list
    }

    /// An event of a server that tells nothing about a followed thread. `changed`: the
    /// message that it names.
    pub fn event(&mut self, server_id: ServerId, changed: Option<MessageId>) -> bool {
        let retried = self.thread.retry(server_id);
        let dropped = changed.is_some_and(|message_id| self.roots.changed(server_id, message_id));
        retried || dropped
    }

    /// The state of a server was replaced, or events were not told: everything of the
    /// server is read again.
    pub fn missed(&mut self, server_id: ServerId) -> bool {
        let thread = self.thread.stale(server_id);
        let list = self.list.trigger(server_id);
        let roots = self.roots.stale(server_id);
        thread || list || roots
    }

    /// The answer to a change of the thread of the side panel. The list of the channel
    /// follows a change that the server made.
    pub fn change_answered(
        &mut self,
        ask: &ThreadAsk,
        answer: Result<Option<(bool, Option<bool>)>, String>,
    ) -> bool {
        let made = answer.as_ref().ok().copied();
        let overtaken = self.thread.asking == Some(ask.number) && self.thread.overtaken;
        if !self.thread.ask_answered(ask.number, answer) {
            return false;
        }
        match made {
            // An event told the state during the request, and the answer can be older
            // than the event: it does not state what the list holds. The list is read.
            Some(_) if overtaken => {
                self.list.trigger(ask.server_id);
            }
            Some(Some((following, _))) => {
                self.list.told(ask.server_id, ask.root_id, following);
            }
            Some(None) => {
                self.list.trigger(ask.server_id);
            }
            None => {}
        }
        true
    }

    /// The roots of the followed threads that the rows of a channel list now and that
    /// the state of the server does not hold, in the order of the rows.
    pub fn missing_roots(
        &self,
        model: &ServerModel,
        channel: (ServerId, ChannelId),
        open: Option<MessageId>,
    ) -> Vec<MessageId> {
        let unread: Vec<MessageId> = model
            .threads
            .iter()
            .map(|state| state.root_id)
            .filter(|root_id| listed_as_unread(model, channel.1, *root_id))
            .collect();
        let (further, _) = self.list.listed(channel, &unread, open);
        further
            .into_iter()
            .map(|state| state.root_id)
            .filter(|root_id| held_root(model, channel.1, *root_id).is_none())
            .collect()
    }
}

/// True when the channel column lists a thread as an unread thread of a channel: the
/// state of the server holds its root, and the root is a message of the channel.
fn listed_as_unread(model: &ServerModel, channel_id: ChannelId, root_id: MessageId) -> bool {
    matches!(
        model.named(root_id),
        Some(Named::Read(root)) if PlaceId::of(&root.place) == PlaceId::Channel(channel_id)
    )
}

/// The root of a thread of a channel as the state of the server holds it: among the
/// messages that its lists name, in the loaded messages of the channel, or as the root
/// of the thread that is open.
pub(super) fn held_root(
    model: &ServerModel,
    channel_id: ChannelId,
    root_id: MessageId,
) -> Option<&Message> {
    if let Some(Named::Read(root)) = model.named(root_id) {
        return Some(root.as_ref());
    }
    let thread = PlaceId::Thread {
        channel_id,
        root_id,
    };
    model
        .timeline(PlaceId::Channel(channel_id))
        .and_then(|timeline| timeline.message(root_id))
        .or_else(|| {
            model
                .timeline(thread)
                .and_then(|timeline| timeline.root.as_ref())
        })
        .map(Arc::as_ref)
}

/// What the answer to a read of the state of a thread tells: whether the account follows
/// it, and its choice. The server knows no state of a thread for an account that never
/// wrote in it, was never named in it and made no choice: that account does not follow
/// it. Nothing when the read failed.
fn follow_state(read: Result<ThreadState, ApiError>) -> Option<(bool, Option<bool>)> {
    match read {
        Ok(state) => Some((state.following, state.follow_setting)),
        Err(error) if error.category() == Some(ErrorCategory::NotFound) => Some((false, None)),
        Err(_) => None,
    }
}

/// Why the server did not change whether the account follows a thread.
fn change_refusal(error: &ApiError) -> String {
    match error.category() {
        Some(ErrorCategory::NotFound) => text::followed::THREAD_GONE.to_owned(),
        _ => text::flow_error(&FlowError::from(error)),
    }
}

impl AppState {
    /// After the view is derived: the thread of the side panel and the channel that the
    /// window shows now are taken, and what they need is read.
    pub(super) fn follow_followed(&mut self, view: &ShellView, cx: &mut Context<Self>) {
        let shown = self.shown;
        let followed = &mut self.known.followed;
        followed.retain(&view.servers.iter().map(|server| server.id).collect());
        followed.thread.show(
            shown
                .server
                .zip(shown.thread)
                .map(|(server_id, (_, root_id))| (server_id, root_id)),
        );
        followed.list.show(shown.server.zip(shown.channel));
        self.read_followed(cx);
    }

    /// The server of the requests with its state, when the state is the one of the
    /// account of the session (the rule of [`super::view::shown_model`]).
    fn followed_source(&self, server_id: ServerId) -> Option<(ServerHandle, Arc<ServerModel>)> {
        let server = self.client.server(server_id)?;
        let AccessState::LoggedIn { account_id, .. } = server.access().state else {
            return None;
        };
        let model = server.live().view().model?;
        (model.account.id == account_id).then_some((server, model))
    }

    /// Sends the reads that the follow state, the list and the titles name now. Each
    /// answer starts the next ones.
    fn read_followed(&mut self, cx: &mut Context<Self>) {
        let shown = self.shown;
        let Some(server_id) = shown.server else {
            return;
        };
        let Some((server, model)) = self.followed_source(server_id) else {
            return;
        };
        let followed = &mut self.known.followed;
        followed.belongs(server_id, model.account.id);
        followed.list.observe(Seen::of(&model, shown.thread));
        if let Some(ask) = followed.thread.next_read() {
            let api = server.api();
            cx.spawn(async move |this, cx| {
                let state = follow_state(api.thread_state(ask.root_id).await);
                let _ = this.update(cx, |this, cx| {
                    if this.known.followed.thread.read_answered(ask.number, state) {
                        this.followed_changed(cx);
                    }
                });
            })
            .detach();
        }
        if let Some(read) = followed.list.next_read() {
            let api = server.api();
            cx.spawn(async move |this, cx| {
                let page = api
                    .followed_threads(read.channel_id, read.before.as_deref(), Some(read.limit))
                    .await
                    .ok();
                let _ = this.update(cx, |this, cx| {
                    if this.known.followed.list.answered(read.number, page) {
                        this.followed_changed(cx);
                    }
                });
            })
            .detach();
        }
        let open = shown.thread.map(|(_, root_id)| root_id);
        let missing = shown
            .channel
            .map(|channel_id| followed.missing_roots(&model, (server_id, channel_id), open))
            .unwrap_or_default();
        for read in followed.roots.next_reads(server_id, &missing) {
            let api = server.api();
            cx.spawn(async move |this, cx| {
                let answer = RootAnswer::of(api.message(read.root_id).await);
                let _ = this.update(cx, |this, cx| {
                    if this.known.followed.roots.answered(&read, answer) {
                        this.followed_changed(cx);
                    }
                });
            })
            .detach();
        }
    }

    /// What is held of followed threads changed: the next reads are sent, and the window
    /// draws again.
    fn followed_changed(&mut self, cx: &mut Context<Self>) {
        self.read_followed(cx);
        cx.notify();
    }

    /// One change of the state of a server, told by itself.
    pub(super) fn followed_noticed(
        &mut self,
        server_id: ServerId,
        notice: &Notice,
        cx: &mut Context<Self>,
    ) {
        let followed = &mut self.known.followed;
        let changed = match notice {
            Notice::Event { event, .. } => match event.as_ref() {
                Event::ThreadFollowChanged {
                    root_id,
                    following,
                    follow_setting,
                } => followed.told(server_id, *root_id, *following, *follow_setting),
                Event::MessageChanged { message_id, .. } => {
                    followed.event(server_id, Some(*message_id))
                }
                _ => followed.event(server_id, None),
            },
            Notice::Reloaded { .. } => followed.missed(server_id),
            Notice::Ended { .. } => false,
        };
        if changed {
            self.followed_changed(cx);
        }
    }

    /// Events of a server were not told.
    pub(super) fn followed_missed(&mut self, server_id: ServerId, cx: &mut Context<Self>) {
        if self.known.followed.missed(server_id) {
            self.followed_changed(cx);
        }
    }

    /// Stores the choice of the person for the thread of the side panel: follow it, or do
    /// not follow it (ADR 0412). The state of the request and a refusal are in the view
    /// of the thread.
    pub fn follow_thread(&mut self, following: bool, cx: &mut Context<Self>) {
        self.change_follow(Some(following), cx);
    }

    /// Removes the choice of the person for the thread of the side panel: the rule of the
    /// server decides again.
    pub fn clear_thread_follow(&mut self, cx: &mut Context<Self>) {
        self.change_follow(None, cx);
    }

    fn change_follow(&mut self, choice: Option<bool>, cx: &mut Context<Self>) {
        let Some(server) = self
            .known
            .followed
            .thread
            .server()
            .and_then(|server_id| self.client.server(server_id))
        else {
            return;
        };
        let Some(ask) = self.known.followed.thread.ask() else {
            return;
        };
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let answer = match choice {
                Some(following) => api
                    .set_thread_follow(ask.root_id, following)
                    .await
                    .map(|state| Some((state.following, state.follow_setting))),
                None => match api.clear_thread_follow(ask.root_id).await {
                    // Only the server knows what its rule gives for the account.
                    Ok(()) => Ok(follow_state(api.thread_state(ask.root_id).await)),
                    Err(error) => Err(error),
                },
            };
            let answer = answer.map_err(|error| change_refusal(&error));
            let _ = this.update(cx, |this, cx| {
                if this.known.followed.change_answered(&ask, answer) {
                    this.followed_changed(cx);
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Lists every followed thread of the shown channel that was read, and reads the next
    /// page of them.
    pub fn show_more_followed(&mut self, cx: &mut Context<Self>) {
        if self.known.followed.list.expand() {
            self.followed_changed(cx);
        }
    }

    /// Draws this follow state of the thread of the side panel in place of what the
    /// server told. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_follow_fixture(
        &mut self,
        thread: (ServerId, MessageId),
        state: (bool, Option<bool>),
        busy: bool,
        refused: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let mut shown = ThreadFollow::default();
        shown.show(Some(thread));
        if let Some(read) = shown.next_read() {
            shown.read_answered(read.number, Some(state));
        }
        if let Some(ask) = shown.ask()
            && !busy
        {
            shown.ask_answered(ask.number, refused.map_or(Ok(Some(state)), Err));
        }
        self.known.followed.thread = shown;
        self.changed(cx);
    }

    /// Draws these followed threads under a channel in place of what the server told,
    /// with these roots as the ones that were read. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show_followed_fixture(
        &mut self,
        channel: (ServerId, ChannelId),
        threads: Vec<ThreadState>,
        more: bool,
        expanded: bool,
        roots: Vec<Message>,
        cx: &mut Context<Self>,
    ) {
        let mut list = FollowedList::default();
        list.show(Some(channel));
        if let Some(read) = list.next_read() {
            let page = FollowedThreads {
                threads,
                more,
                next: more.then(|| "next".to_owned()),
            };
            list.answered(read.number, Some(page));
        }
        if expanded {
            list.expand();
        }
        self.known.followed.list = list;
        let ids: Vec<MessageId> = roots.iter().map(|root| root.id).collect();
        let mut given: HashMap<MessageId, Message> =
            roots.into_iter().map(|root| (root.id, root)).collect();
        let held = &mut self.known.followed.roots;
        // A few roots are read at one time: each pass answers the reads of the pass.
        loop {
            let reads = held.next_reads(channel.0, &ids);
            if reads.is_empty() {
                break;
            }
            for read in reads {
                let answer = given
                    .remove(&read.root_id)
                    .map_or(RootAnswer::Refused, |root| RootAnswer::Read(Arc::new(root)));
                held.answered(&read, answer);
            }
        }
        self.changed(cx);
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::protocol::ids::WorkspaceId;
    use junto_client_core::protocol::message::MessagePlace;
    use time::OffsetDateTime;

    use super::*;

    fn thread() -> (ServerId, MessageId) {
        (ServerId::generate(), MessageId::generate())
    }

    fn channel() -> (ServerId, ChannelId) {
        (ServerId::generate(), ChannelId::generate())
    }

    fn state(root_id: MessageId, unread: u64) -> ThreadState {
        ThreadState {
            root_id,
            last_read: 4,
            latest: 4 + unread,
            unread,
            mentions: 0,
            following: true,
            follow_setting: None,
        }
    }

    fn page(threads: Vec<ThreadState>, next: Option<&str>) -> FollowedThreads {
        FollowedThreads {
            threads,
            more: next.is_some(),
            next: next.map(str::to_owned),
        }
    }

    fn states(count: usize) -> Vec<ThreadState> {
        (0..count)
            .map(|_| state(MessageId::generate(), 0))
            .collect()
    }

    fn message(text: &str) -> Message {
        Message {
            id: MessageId::generate(),
            place: MessagePlace::Channel {
                workspace_id: WorkspaceId::generate(),
                channel_id: ChannelId::generate(),
            },
            position: 1,
            author_id: Some(AccountId::generate()),
            author_name: Some("Ana".to_owned()),
            created_at: OffsetDateTime::UNIX_EPOCH,
            edited_at: None,
            document: Some(crate::compose::parse(text, &[])),
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

    /// A follow state that was read for a thread.
    fn read(following: bool, setting: Option<bool>) -> (ThreadFollow, (ServerId, MessageId)) {
        let shown = thread();
        let mut follow = ThreadFollow::default();
        follow.show(Some(shown));
        let ask = follow.next_read().unwrap();
        assert!(follow.read_answered(ask.number, Some((following, setting))));
        (follow, shown)
    }

    fn control(follow: &ThreadFollow, shown: (ServerId, MessageId)) -> Option<FollowControl> {
        follow.control(shown.0, shown.1, false)
    }

    #[test]
    fn the_state_of_a_thread_is_read_once_when_the_panel_shows_it() {
        let mut follow = ThreadFollow::default();
        // Without a thread nothing is read.
        assert_eq!(follow.next_read(), None);
        let shown = thread();
        follow.show(Some(shown));
        assert_eq!(follow.read, FollowRead::NotRead);
        let ask = follow.next_read().unwrap();
        assert_eq!((ask.server_id, ask.root_id), shown);
        assert_eq!(follow.read, FollowRead::Reading);
        // The control is not drawn while the state is read, and no second read starts.
        assert_eq!(control(&follow, shown), None);
        assert_eq!(follow.next_read(), None);
        assert!(follow.read_answered(ask.number, Some((true, None))));
        assert_eq!(
            control(&follow, shown),
            Some(FollowControl {
                following: true,
                setting: None,
                busy: false,
                read_only: false,
                refused: None,
            })
        );
        // The state was read: showing the same thread again reads nothing.
        follow.show(Some(shown));
        assert_eq!(follow.next_read(), None);
        // The control belongs to its thread, and states an archived channel.
        assert_eq!(follow.control(shown.0, MessageId::generate(), false), None);
        assert!(follow.control(shown.0, shown.1, true).unwrap().read_only);
    }

    #[test]
    fn an_answer_for_a_thread_that_is_not_shown_any_more_changes_nothing() {
        let (first, second) = (thread(), thread());
        let mut follow = ThreadFollow::default();
        follow.show(Some(first));
        let late = follow.next_read().unwrap();
        follow.show(Some(second));
        assert_eq!(follow.read, FollowRead::NotRead);
        let ask = follow.next_read().unwrap();
        assert_ne!(ask.number, late.number);
        assert!(!follow.read_answered(late.number, Some((true, Some(true)))));
        assert_eq!(follow.read, FollowRead::Reading);
        assert!(follow.read_answered(ask.number, Some((false, None))));
        assert!(!control(&follow, second).unwrap().following);
        // The panel is closed: nothing is held, and nothing is read.
        follow.show(None);
        assert_eq!(control(&follow, second), None);
        assert_eq!(follow.next_read(), None);
    }

    #[test]
    fn a_change_is_asked_for_once_at_a_time_and_its_answer_tells_the_state() {
        let mut unread = ThreadFollow::default();
        let shown = thread();
        unread.show(Some(shown));
        // The state is not read yet: there is nothing to change.
        assert_eq!(unread.ask(), None);

        let (mut follow, shown) = read(false, None);
        let ask = follow.ask().unwrap();
        assert_eq!((ask.server_id, ask.root_id), shown);
        // The actions take no click while the request is on its way.
        assert!(control(&follow, shown).unwrap().busy);
        assert_eq!(follow.ask(), None);
        assert_eq!(follow.next_read(), None);
        assert!(follow.ask_answered(ask.number, Ok(Some((true, Some(true))))));
        let drawn = control(&follow, shown).unwrap();
        assert!(drawn.following && !drawn.busy);
        assert_eq!(drawn.setting, Some(true));
        // The answer was taken once.
        assert!(!follow.ask_answered(ask.number, Ok(Some((false, Some(false))))));
        assert!(control(&follow, shown).unwrap().following);
    }

    #[test]
    fn a_late_answer_of_a_change_for_another_thread_changes_nothing() {
        let (mut follow, _) = read(false, None);
        let late = follow.ask().unwrap();
        let second = thread();
        follow.show(Some(second));
        let ask = follow.next_read().unwrap();
        assert!(follow.read_answered(ask.number, Some((false, None))));
        assert!(!follow.ask_answered(late.number, Ok(Some((true, Some(true))))));
        assert!(!follow.ask_answered(late.number, Err("refused".to_owned())));
        let drawn = control(&follow, second).unwrap();
        assert!(!drawn.following && !drawn.busy);
        assert_eq!(drawn.refused, None);
    }

    #[test]
    fn a_refusal_stays_until_the_next_request_or_until_another_thread_is_shown() {
        let (mut follow, shown) = read(true, None);
        let ask = follow.ask().unwrap();
        assert!(follow.ask_answered(ask.number, Err("Not allowed.".to_owned())));
        let drawn = control(&follow, shown).unwrap();
        assert_eq!(drawn.refused.as_deref(), Some("Not allowed."));
        // What was read before the refusal still holds, and the actions take a click.
        assert!(drawn.following && !drawn.busy);
        // The next request puts the refusal away.
        follow.ask().unwrap();
        assert_eq!(control(&follow, shown).unwrap().refused, None);

        let (mut follow, shown) = read(true, None);
        let ask = follow.ask().unwrap();
        follow.ask_answered(ask.number, Err("Not allowed.".to_owned()));
        follow.show(Some(thread()));
        follow.show(Some(shown));
        let again = follow.next_read().unwrap();
        follow.read_answered(again.number, Some((true, None)));
        assert_eq!(control(&follow, shown).unwrap().refused, None);
    }

    #[test]
    fn an_event_tells_the_state_and_an_older_read_does_not_replace_it() {
        let shown = thread();
        let mut follow = ThreadFollow::default();
        follow.show(Some(shown));
        let ask = follow.next_read().unwrap();
        // The event of another thread, and of another server, tells nothing here.
        assert!(!follow.told(shown.0, MessageId::generate(), true, Some(true)));
        assert!(!follow.told(ServerId::generate(), shown.1, true, Some(true)));
        assert!(follow.told(shown.0, shown.1, false, Some(false)));
        let drawn = control(&follow, shown).unwrap();
        assert_eq!((drawn.following, drawn.setting), (false, Some(false)));
        // The read was sent before the event.
        assert!(!follow.read_answered(ask.number, Some((true, None))));
        assert!(!control(&follow, shown).unwrap().following);
        assert_eq!(follow.next_read(), None);
    }

    #[test]
    fn a_read_that_failed_is_sent_again_when_an_event_of_the_server_arrives() {
        let shown = thread();
        let mut follow = ThreadFollow::default();
        follow.show(Some(shown));
        let ask = follow.next_read().unwrap();
        assert!(follow.read_answered(ask.number, None));
        assert_eq!(follow.read, FollowRead::Failed);
        assert_eq!(control(&follow, shown), None);
        // Nothing reads again by itself.
        assert_eq!(follow.next_read(), None);
        assert!(!follow.retry(ServerId::generate()));
        assert!(follow.retry(shown.0));
        // One event asks for one read.
        assert!(!follow.retry(shown.0));
        let again = follow.next_read().unwrap();
        assert_ne!(again.number, ask.number);
        assert!(follow.read_answered(again.number, Some((true, None))));
        assert!(control(&follow, shown).unwrap().following);
        // A state that was read is not read again by an event.
        assert!(!follow.retry(shown.0));
    }

    #[test]
    fn missed_events_read_the_state_again_and_keep_what_is_shown_meanwhile() {
        let (mut follow, shown) = read(true, Some(true));
        assert!(!follow.stale(ServerId::generate()));
        assert!(follow.stale(shown.0));
        let again = follow.next_read().unwrap();
        // The control stays while the state is read again.
        assert!(control(&follow, shown).unwrap().following);
        // A read that fails keeps what was read before.
        assert!(follow.read_answered(again.number, None));
        assert!(control(&follow, shown).unwrap().following);
        assert!(follow.stale(shown.0));
        let again = follow.next_read().unwrap();
        assert!(follow.read_answered(again.number, Some((false, None))));
        assert!(!control(&follow, shown).unwrap().following);
    }

    #[test]
    fn a_removed_choice_whose_state_the_server_did_not_tell_is_read_again() {
        let (mut follow, shown) = read(false, Some(false));
        let ask = follow.ask().unwrap();
        assert!(follow.ask_answered(ask.number, Ok(None)));
        assert_eq!(control(&follow, shown), None);
        let again = follow.next_read().unwrap();
        assert!(follow.read_answered(again.number, Some((true, None))));
        let drawn = control(&follow, shown).unwrap();
        assert_eq!((drawn.following, drawn.setting), (true, None));
    }

    #[test]
    fn the_state_of_another_account_of_the_server_is_not_kept() {
        let (follow, shown) = read(true, Some(true));
        let (first, second) = (AccountId::generate(), AccountId::generate());
        let mut followed = Followed {
            thread: follow,
            ..Followed::default()
        };
        followed.belongs(shown.0, first);
        followed.belongs(shown.0, first);
        assert!(followed.thread.control(shown.0, shown.1, false).is_some());
        followed.belongs(shown.0, second);
        assert_eq!(followed.thread.control(shown.0, shown.1, false), None);
        assert!(followed.thread.next_read().is_some());
    }

    #[test]
    fn the_list_is_read_when_a_channel_is_shown_and_one_read_runs_at_a_time() {
        let mut list = FollowedList::default();
        assert_eq!(list.next_read(), None);
        let shown = channel();
        list.show(Some(shown));
        let read = list.next_read().unwrap();
        assert_eq!((read.server_id, read.channel_id), shown);
        assert_eq!((read.before.as_deref(), read.limit), (None, 50));
        // A second read does not start while the first one is on its way.
        assert!(list.trigger(shown.0));
        assert_eq!(list.next_read(), None);
        let threads = states(3);
        assert!(list.answered(read.number, Some(page(threads.clone(), None))));
        assert_eq!(list.threads, threads);
        // The trigger during the read is followed by one more read, and by no third one.
        let again = list.next_read().unwrap();
        assert_ne!(again.number, read.number);
        assert!(list.answered(again.number, Some(page(threads, None))));
        assert_eq!(list.next_read(), None);
        // The same channel again reads nothing.
        list.show(Some(shown));
        assert_eq!(list.next_read(), None);
        // A trigger of another server is none.
        assert!(!list.trigger(ServerId::generate()));
        assert_eq!(list.next_read(), None);
    }

    #[test]
    fn an_answer_for_a_channel_that_is_not_shown_any_more_is_dropped() {
        let (first, second) = (channel(), channel());
        let mut list = FollowedList::default();
        list.show(Some(first));
        let late = list.next_read().unwrap();
        list.show(Some(second));
        // The read of the channel before is still on its way: the next one waits.
        assert_eq!(list.next_read(), None);
        assert!(list.answered(late.number, Some(page(states(2), None))));
        assert!(list.threads.is_empty());
        let read = list.next_read().unwrap();
        assert_eq!((read.server_id, read.channel_id), second);
        let threads = states(1);
        assert!(list.answered(read.number, Some(page(threads.clone(), None))));
        assert_eq!(list.threads, threads);
        // An answer with a number that is not waited for changes nothing.
        assert!(!list.answered(late.number, Some(page(states(4), None))));
        assert_eq!(list.threads, threads);
        // No channel: nothing is listed, and nothing is read.
        list.show(None);
        assert!(list.threads.is_empty());
        assert!(!list.trigger(second.0));
        assert_eq!(list.next_read(), None);
    }

    #[test]
    fn a_failed_read_keeps_the_list_and_the_next_trigger_reads_again() {
        let shown = channel();
        let mut list = FollowedList::default();
        list.show(Some(shown));
        let read = list.next_read().unwrap();
        let threads = states(2);
        list.answered(read.number, Some(page(threads.clone(), None)));
        list.trigger(shown.0);
        let failed = list.next_read().unwrap();
        assert!(list.answered(failed.number, None));
        assert_eq!(list.threads, threads);
        // Nothing reads again by itself.
        assert_eq!(list.next_read(), None);
        list.trigger(shown.0);
        assert!(list.next_read().is_some());
    }

    #[test]
    fn a_change_of_the_unread_threads_or_of_the_replies_of_the_open_thread_is_a_trigger() {
        let shown = channel();
        let (root, other) = (MessageId::generate(), MessageId::generate());
        let seen = |unread: u64, replies: Option<(MessageId, u64)>| Seen {
            unread: vec![(root, 9, unread, true)],
            open: replies,
        };
        let mut list = FollowedList::default();
        // Without a channel nothing is observed.
        list.observe(seen(1, None));
        assert_eq!(list.next_read(), None);
        list.show(Some(shown));
        list.observe(seen(1, None));
        let read = list.next_read().unwrap();
        list.answered(read.number, Some(page(Vec::new(), None)));
        // The same state is no trigger.
        list.observe(seen(1, None));
        assert_eq!(list.next_read(), None);
        // Another count of unread replies is one.
        list.observe(seen(2, None));
        let read = list.next_read().unwrap();
        list.answered(read.number, Some(page(Vec::new(), None)));
        // A thread that is opened is none, and its first reply count is none.
        list.observe(seen(2, Some((other, 3))));
        assert_eq!(list.next_read(), None);
        // The person wrote a reply in the open thread.
        list.observe(seen(2, Some((other, 4))));
        let read = list.next_read().unwrap();
        list.answered(read.number, Some(page(Vec::new(), None)));
        // Another open thread with another count is none.
        list.observe(seen(2, Some((root, 7))));
        assert_eq!(list.next_read(), None);
    }

    #[test]
    fn the_first_eight_followed_threads_are_listed_and_a_row_offers_the_others() {
        let shown = channel();
        let threads = states(11);
        let roots: Vec<MessageId> = threads.iter().map(|state| state.root_id).collect();
        let mut list = FollowedList::default();
        list.show(Some(shown));
        let read = list.next_read().unwrap();
        list.answered(read.number, Some(page(threads, None)));
        let ids = |listed: &[&ThreadState]| -> Vec<MessageId> {
            listed.iter().map(|state| state.root_id).collect()
        };

        let (listed, more) = list.listed(shown, &[], None);
        assert_eq!(ids(&listed), roots[..8]);
        assert_eq!(more, Some(MoreFollowed { count: Some(3) }));
        // A thread that a row before shows as unread is not listed a second time.
        let (listed, more) = list.listed(shown, &roots[..2], None);
        assert_eq!(ids(&listed), roots[2..10]);
        assert_eq!(more, Some(MoreFollowed { count: Some(1) }));
        // The open thread has a row of its own: the row of the others does not count it.
        let (_, more) = list.listed(shown, &[], Some(roots[9]));
        assert_eq!(more, Some(MoreFollowed { count: Some(2) }));
        // Another channel lists nothing of this one.
        assert_eq!(list.listed(channel(), &[], None), (Vec::new(), None));

        // The person asks for the others: all are listed, and no row is left.
        assert!(list.expand());
        let (listed, more) = list.listed(shown, &[], None);
        assert_eq!(ids(&listed), roots);
        assert_eq!(more, None);
        // The server has no further page: nothing is read.
        assert_eq!(list.next_read(), None);
        // The expansion ends when another channel is shown.
        list.show(Some(channel()));
        list.show(Some(shown));
        assert!(!list.expanded);
    }

    #[test]
    fn the_further_pages_are_read_after_the_person_asks_and_the_list_keeps_its_length() {
        let shown = channel();
        let first = states(50);
        let mut list = FollowedList::default();
        list.show(Some(shown));
        let read = list.next_read().unwrap();
        list.answered(read.number, Some(page(first.clone(), Some("after-50"))));
        // The server lists more: the row does not state a count.
        let (listed, more) = list.listed(shown, &[], None);
        assert_eq!(listed.len(), 8);
        assert_eq!(more, Some(MoreFollowed { count: None }));
        assert_eq!(list.next_read(), None);

        assert!(list.expand());
        let further = list.next_read().unwrap();
        assert_eq!(
            (further.before.as_deref(), further.limit),
            (Some("after-50"), 50)
        );
        // A thread that both pages state is held once.
        let mut second = states(20);
        second.insert(0, first[49].clone());
        assert!(list.answered(further.number, Some(page(second, None))));
        assert_eq!(list.threads.len(), 70);
        let (listed, more) = list.listed(shown, &[], None);
        assert_eq!((listed.len(), more), (70, None));
        assert_eq!(list.next_read(), None);

        // A trigger reads the list again as far as it was read: two pages in one read.
        list.trigger(shown.0);
        let again = list.next_read().unwrap();
        assert_eq!((again.before.as_deref(), again.limit), (None, 100));
        assert_eq!(refresh_limit(0), 50);
        assert_eq!(refresh_limit(50), 50);
        assert_eq!(refresh_limit(51), 100);
        assert_eq!(refresh_limit(1000), 200);
    }

    #[test]
    fn a_trigger_and_a_further_page_are_read_one_after_the_other() {
        let shown = channel();
        let mut list = FollowedList::default();
        list.show(Some(shown));
        let read = list.next_read().unwrap();
        list.answered(read.number, Some(page(states(50), Some("after-50"))));
        list.trigger(shown.0);
        list.expand();
        // The list from its start comes first, and gives the continuation of the page.
        let again = list.next_read().unwrap();
        assert_eq!(again.before, None);
        assert_eq!(list.next_read(), None);
        list.answered(again.number, Some(page(states(50), Some("after-new"))));
        let further = list.next_read().unwrap();
        assert_eq!(further.before.as_deref(), Some("after-new"));
        // A further page that fails is not asked for again by itself.
        assert!(list.answered(further.number, None));
        assert_eq!(list.next_read(), None);
        assert_eq!(list.threads.len(), 50);
    }

    #[test]
    fn a_thread_that_the_account_follows_no_longer_leaves_the_list_at_once() {
        let shown = channel();
        let threads = states(3);
        let mut list = FollowedList::default();
        list.show(Some(shown));
        let read = list.next_read().unwrap();
        list.answered(read.number, Some(page(threads.clone(), None)));
        // An event of another server tells nothing here.
        assert!(!list.told(ServerId::generate(), threads[1].root_id, false));
        assert_eq!(list.threads.len(), 3);
        assert!(list.told(shown.0, threads[1].root_id, false));
        assert_eq!(list.threads, [threads[0].clone(), threads[2].clone()]);
        // The server states the list again.
        assert!(list.next_read().is_some());

        // A thread that the account follows from now on is stated by the server.
        let mut list = FollowedList::default();
        list.show(Some(shown));
        let read = list.next_read().unwrap();
        list.answered(read.number, Some(page(Vec::new(), None)));
        assert!(list.told(shown.0, MessageId::generate(), true));
        assert!(list.threads.is_empty());
        assert!(list.next_read().is_some());
    }

    #[test]
    fn the_answer_to_a_change_is_followed_by_the_list_of_its_channel() {
        let (follow, shown) = read(true, None);
        let channel = (shown.0, ChannelId::generate());
        let mut followed = Followed {
            thread: follow,
            ..Followed::default()
        };
        followed.list.show(Some(channel));
        let first = followed.list.next_read().unwrap();
        followed
            .list
            .answered(first.number, Some(page(vec![state(shown.1, 0)], None)));
        // A refusal changes nothing of the list.
        let ask = followed.thread.ask().unwrap();
        assert!(followed.change_answered(&ask, Err("Not allowed.".to_owned())));
        assert_eq!(followed.list.threads.len(), 1);
        assert_eq!(followed.list.next_read(), None);
        // The server stored "do not follow": the thread leaves the list.
        let ask = followed.thread.ask().unwrap();
        assert!(followed.change_answered(&ask, Ok(Some((false, Some(false))))));
        assert!(followed.list.threads.is_empty());
        assert!(followed.list.next_read().is_some());
        // An answer that was taken changes nothing a second time.
        assert!(!followed.change_answered(&ask, Ok(Some((true, Some(true))))));
        assert_eq!(
            followed
                .thread
                .control(shown.0, shown.1, false)
                .unwrap()
                .setting,
            Some(false)
        );
    }

    #[test]
    fn at_most_four_roots_are_read_at_one_time_and_each_root_once() {
        let server_id = ServerId::generate();
        let wanted: Vec<MessageId> = (0..6).map(|_| MessageId::generate()).collect();
        let mut roots = Roots::default();
        let reads = roots.next_reads(server_id, &wanted);
        let asked: Vec<MessageId> = reads.iter().map(|read| read.root_id).collect();
        assert_eq!(asked, wanted[..4]);
        // The same rows again start no read.
        assert!(roots.next_reads(server_id, &wanted).is_empty());
        let root = Arc::new(message("Tracks or mixdown?"));
        assert!(roots.answered(&reads[0], RootAnswer::Read(root.clone())));
        assert_eq!(roots.get(server_id, wanted[0]), Some(root.as_ref()));
        // The answer was taken once.
        assert!(!roots.answered(&reads[0], RootAnswer::Refused));
        assert_eq!(roots.get(server_id, wanted[0]), Some(root.as_ref()));
        // One place is free: the next root is read, and the read one is not.
        let more = roots.next_reads(server_id, &wanted);
        assert_eq!(more.len(), 1);
        assert_eq!(more[0].root_id, wanted[4]);
        // The same root on another server is another message.
        assert_eq!(roots.get(ServerId::generate(), wanted[0]), None);
    }

    #[test]
    fn a_root_that_the_server_does_not_give_has_no_title_and_is_not_read_again() {
        let server_id = ServerId::generate();
        let (refused, unanswered) = (MessageId::generate(), MessageId::generate());
        let mut roots = Roots::default();
        let reads = roots.next_reads(server_id, &[refused, unanswered]);
        assert!(roots.answered(&reads[0], RootAnswer::Refused));
        assert!(roots.answered(&reads[1], RootAnswer::Failed));
        assert_eq!(roots.get(server_id, refused), None);
        assert_eq!(roots.get(server_id, unanswered), None);
        assert!(
            roots
                .next_reads(server_id, &[refused, unanswered])
                .is_empty()
        );
    }

    #[test]
    fn the_title_of_a_root_is_dropped_and_read_again_after_an_event_named_it() {
        let server_id = ServerId::generate();
        let root = Arc::new(message("Tracks or mixdown?"));
        let mut roots = Roots::default();
        let reads = roots.next_reads(server_id, &[root.id]);
        roots.answered(&reads[0], RootAnswer::Read(root.clone()));
        // A message that is not held, and the message on another server, change nothing.
        assert!(!roots.changed(server_id, MessageId::generate()));
        assert!(!roots.changed(ServerId::generate(), root.id));
        assert!(roots.changed(server_id, root.id));
        assert_eq!(roots.get(server_id, root.id), None);
        let again = roots.next_reads(server_id, &[root.id]);
        assert_eq!(again.len(), 1);
        assert_ne!(again[0].number, reads[0].number);

        // The message changes again while that read is on its way: its answer is older
        // than the change and is not kept, and the root is read once more.
        assert!(roots.changed(server_id, root.id));
        assert!(roots.next_reads(server_id, &[root.id]).is_empty());
        assert!(roots.answered(&again[0], RootAnswer::Read(root.clone())));
        assert_eq!(roots.get(server_id, root.id), None);
        let third = roots.next_reads(server_id, &[root.id]);
        assert_eq!(third.len(), 1);
        let edited = Arc::new(message("Tracks, one per speaker"));
        assert!(roots.answered(&third[0], RootAnswer::Read(edited.clone())));
        assert_eq!(roots.get(server_id, root.id), Some(edited.as_ref()));
    }

    #[test]
    fn missed_events_read_a_root_again_and_show_what_was_read_until_the_answer() {
        let server_id = ServerId::generate();
        let root = Arc::new(message("Tracks or mixdown?"));
        let refused = MessageId::generate();
        let mut roots = Roots::default();
        let reads = roots.next_reads(server_id, &[root.id, refused]);
        roots.answered(&reads[0], RootAnswer::Read(root.clone()));
        roots.answered(&reads[1], RootAnswer::Refused);
        assert!(!roots.stale(ServerId::generate()));
        assert!(roots.stale(server_id));
        assert_eq!(roots.get(server_id, root.id), Some(root.as_ref()));
        // Both are read again: the server can give now what it refused before.
        let again = roots.next_reads(server_id, &[root.id, refused]);
        assert_eq!(again.len(), 2);
        // A read without an answer keeps what was read before.
        assert!(roots.answered(&again[0], RootAnswer::Failed));
        assert_eq!(roots.get(server_id, root.id), Some(root.as_ref()));
        assert!(roots.next_reads(server_id, &[root.id]).is_empty());
    }

    #[test]
    fn the_cache_holds_two_hundred_roots_and_drops_the_one_that_was_needed_longest_ago() {
        let server_id = ServerId::generate();
        let mut roots = Roots::default();
        let mut ids = Vec::new();
        for _ in 0..MOST_ROOTS {
            let root = Arc::new(message("A root"));
            ids.push(root.id);
            let reads = roots.next_reads(server_id, &[root.id]);
            assert!(roots.answered(&reads[0], RootAnswer::Read(root)));
        }
        assert_eq!(roots.held.len(), MOST_ROOTS);
        // A row needs the first root again: it is not the one that leaves.
        assert!(roots.next_reads(server_id, &[ids[0]]).is_empty());
        let root = Arc::new(message("One more root"));
        let reads = roots.next_reads(server_id, &[root.id]);
        assert!(roots.answered(&reads[0], RootAnswer::Read(root.clone())));
        assert_eq!(roots.held.len(), MOST_ROOTS);
        assert!(roots.get(server_id, ids[0]).is_some());
        assert_eq!(roots.get(server_id, ids[1]), None);
        assert!(roots.get(server_id, ids[2]).is_some());
        assert!(roots.get(server_id, root.id).is_some());
    }

    #[test]
    fn the_roots_of_a_server_that_left_and_of_another_account_are_dropped() {
        let (stays, leaves) = (ServerId::generate(), ServerId::generate());
        let (first, second) = (Arc::new(message("First")), Arc::new(message("Second")));
        let waiting = MessageId::generate();
        let mut followed = Followed::default();
        followed.belongs(stays, AccountId::generate());
        followed.belongs(leaves, AccountId::generate());
        let reads = followed.roots.next_reads(stays, &[first.id]);
        followed
            .roots
            .answered(&reads[0], RootAnswer::Read(first.clone()));
        let reads = followed.roots.next_reads(leaves, &[second.id, waiting]);
        followed
            .roots
            .answered(&reads[0], RootAnswer::Read(second.clone()));

        followed.retain(&[stays].into_iter().collect());
        assert!(followed.roots.get(stays, first.id).is_some());
        assert_eq!(followed.roots.get(leaves, second.id), None);
        // The answer of a read for the server that left is not kept.
        assert!(!followed.roots.answered(&reads[1], RootAnswer::Read(second)));
        assert_eq!(followed.roots.get(leaves, waiting), None);

        // Another account on the server that stays: its roots are read anew, and the
        // answer of a read that was sent for the account before is not kept.
        let late = followed.roots.next_reads(stays, &[waiting]);
        followed.belongs(stays, AccountId::generate());
        assert_eq!(followed.roots.get(stays, first.id), None);
        assert!(
            followed
                .roots
                .answered(&late[0], RootAnswer::Read(first.clone()))
        );
        assert_eq!(followed.roots.get(stays, waiting), None);
        assert_eq!(
            followed.roots.next_reads(stays, &[first.id, waiting]).len(),
            2
        );
    }

    #[test]
    fn missed_events_read_everything_of_their_server_again() {
        let (follow, shown) = read(true, None);
        let channel = (shown.0, ChannelId::generate());
        let mut followed = Followed {
            thread: follow,
            ..Followed::default()
        };
        followed.list.show(Some(channel));
        let first = followed.list.next_read().unwrap();
        followed
            .list
            .answered(first.number, Some(page(Vec::new(), None)));
        assert!(!followed.missed(ServerId::generate()));
        assert_eq!(followed.thread.next_read(), None);
        assert_eq!(followed.list.next_read(), None);
        assert!(followed.missed(shown.0));
        assert!(followed.thread.next_read().is_some());
        assert!(followed.list.next_read().is_some());
        // An event that names a message tells nothing while no root of it is held.
        assert!(!followed.event(shown.0, Some(MessageId::generate())));
        assert!(!followed.event(shown.0, None));
    }

    #[test]
    fn more_wanted_roots_than_the_cache_holds_are_read_once_each_and_the_reads_end() {
        let server_id = ServerId::generate();
        let wanted: Vec<MessageId> = (0..MOST_ROOTS + 40)
            .map(|_| MessageId::generate())
            .collect();
        let mut roots = Roots::default();
        let mut read = 0;
        // Each pass asks for what the rows still need and answers it: a server that gives
        // every root, and one that refuses every root.
        for refuses in [false, true] {
            loop {
                let reads = roots.next_reads(server_id, &wanted);
                if reads.is_empty() {
                    break;
                }
                for asked in reads {
                    let answer = if refuses {
                        RootAnswer::Refused
                    } else {
                        RootAnswer::Read(Arc::new(message("A root")))
                    };
                    roots.answered(&asked, answer);
                    read += 1;
                    assert!(
                        read <= 2 * wanted.len(),
                        "the reads of the roots do not end"
                    );
                }
            }
            // The first rows have their titles; the rows beyond the bound have none.
            assert!(read <= MOST_ROOTS);
            assert!(roots.held.len() <= MOST_ROOTS);
            assert!(roots.held.contains_key(&(server_id, wanted[0])));
            roots = Roots::default();
            read = 0;
        }
    }

    #[test]
    fn an_event_during_a_change_is_not_replaced_by_the_answer_of_the_change() {
        let (mut follow, shown) = read(false, None);
        // The person asks to follow. Another device of the account then stops following,
        // and its event arrives before the answer of the request of this device.
        let ask = follow.ask().unwrap();
        assert!(follow.told(shown.0, shown.1, false, Some(false)));
        assert!(follow.ask_answered(ask.number, Ok(Some((true, Some(true))))));
        // The answer can be older than the event: the state of the event stays, and the
        // server is asked which one holds.
        let control = control(&follow, shown).unwrap();
        assert_eq!((control.following, control.setting), (false, Some(false)));
        assert!(!control.busy);
        let again = follow.next_read().unwrap();
        assert!(follow.read_answered(again.number, Some((false, Some(false)))));
        assert!(follow.next_read().is_none());

        // Without an event during the change, the answer tells the state and no read
        // follows.
        let ask = follow.ask().unwrap();
        assert!(follow.ask_answered(ask.number, Ok(Some((true, Some(true))))));
        let control = control_of(&follow, shown);
        assert_eq!(control, (true, Some(true)));
        assert!(follow.next_read().is_none());
    }

    fn control_of(follow: &ThreadFollow, shown: (ServerId, MessageId)) -> (bool, Option<bool>) {
        let control = control(follow, shown).unwrap();
        (control.following, control.setting)
    }

    #[test]
    fn a_page_that_was_asked_for_before_an_event_does_not_list_the_thread_again() {
        let shown = channel();
        let threads = states(3);
        let gone = threads[1].root_id;
        let mut list = FollowedList::default();
        list.show(Some(shown));
        let first = list.next_read().unwrap();
        assert!(list.answered(first.number, Some(page(threads.clone(), None))));

        // A read is on its way when the event tells that the thread is not followed.
        assert!(list.trigger(shown.0));
        let older = list.next_read().unwrap();
        assert!(list.told(shown.0, gone, false));
        let listed = |list: &FollowedList| -> Vec<MessageId> {
            list.listed(shown, &[], None)
                .0
                .iter()
                .map(|state| state.root_id)
                .collect()
        };
        assert!(!listed(&list).contains(&gone));
        // Its answer was made before the event and names the thread: it is not used, and
        // the list is read again.
        assert!(list.answered(older.number, Some(page(threads.clone(), None))));
        assert!(!listed(&list).contains(&gone));
        let newer = list.next_read().unwrap();
        let after: Vec<ThreadState> = threads
            .iter()
            .filter(|state| state.root_id != gone)
            .cloned()
            .collect();
        assert!(list.answered(newer.number, Some(page(after, None))));
        assert_eq!(listed(&list).len(), 2);
    }

    #[test]
    fn a_page_that_names_a_thread_twice_lists_it_once() {
        let shown = channel();
        let threads = states(2);
        let mut twice = threads.clone();
        twice.push(threads[0].clone());
        twice.push(threads[1].clone());
        let mut list = FollowedList::default();
        list.show(Some(shown));
        let first = list.next_read().unwrap();
        assert!(list.answered(first.number, Some(page(twice.clone(), Some("next")))));
        let listed = |list: &FollowedList| -> Vec<MessageId> {
            list.listed(shown, &[], None)
                .0
                .iter()
                .map(|state| state.root_id)
                .collect()
        };
        assert_eq!(listed(&list), [threads[0].root_id, threads[1].root_id]);
        // A further page that repeats them adds nothing.
        assert!(list.expand());
        let further = list.next_read().unwrap();
        assert!(list.answered(further.number, Some(page(twice, None))));
        assert_eq!(listed(&list), [threads[0].root_id, threads[1].root_id]);
    }

    #[test]
    fn a_further_page_that_an_event_overtook_is_asked_for_again_after_the_list() {
        let shown = channel();
        let mut list = FollowedList::default();
        list.show(Some(shown));
        let read = list.next_read().unwrap();
        list.answered(read.number, Some(page(states(50), Some("after-50"))));
        // The person asks for the further threads, and an event arrives before the page.
        assert!(list.expand());
        let further = list.next_read().unwrap();
        assert!(list.told(shown.0, MessageId::generate(), true));
        // The page was asked for before the event: it is not used.
        assert!(list.answered(further.number, Some(page(states(20), None))));
        assert_eq!(list.threads.len(), 50);
        // The list is read from its start, and then the further page with the
        // continuation that this read gave.
        let again = list.next_read().unwrap();
        assert_eq!(again.before, None);
        assert!(list.answered(again.number, Some(page(states(50), Some("after-new")))));
        let further = list.next_read().unwrap();
        assert_eq!(further.before.as_deref(), Some("after-new"));
        assert!(list.answered(further.number, Some(page(states(20), None))));
        assert_eq!(list.threads.len(), 70);
        assert_eq!(list.next_read(), None);

        // Another channel ends what the person asked for: the page that an event
        // overtook is not asked for there.
        let mut list = FollowedList::default();
        list.show(Some(shown));
        let read = list.next_read().unwrap();
        list.answered(read.number, Some(page(states(50), Some("after-50"))));
        assert!(list.expand());
        let further = list.next_read().unwrap();
        assert!(list.told(shown.0, MessageId::generate(), true));
        let other = (shown.0, ChannelId::generate());
        list.show(Some(other));
        assert!(list.answered(further.number, Some(page(states(20), None))));
        let first = list.next_read().unwrap();
        assert_eq!((first.channel_id, first.before.clone()), (other.1, None));
        assert!(list.answered(first.number, Some(page(states(50), Some("other-50")))));
        assert_eq!(list.next_read(), None);
    }

    #[test]
    fn an_answer_of_a_change_that_an_event_overtook_does_not_take_the_thread_from_the_list() {
        let (follow, shown) = read(true, None);
        let channel = (shown.0, ChannelId::generate());
        let mut followed = Followed {
            thread: follow,
            ..Followed::default()
        };
        followed.list.show(Some(channel));
        let first = followed.list.next_read().unwrap();
        let listed = || Some(page(vec![state(shown.1, 0)], None));
        followed.list.answered(first.number, listed());
        // The person stops following. Another device of the account then follows the
        // thread again: its event arrives before the answer of this device, and the list
        // that is read after the event names the thread.
        let ask = followed.thread.ask().unwrap();
        assert!(followed.told(shown.0, shown.1, true, Some(true)));
        let after = followed.list.next_read().unwrap();
        followed.list.answered(after.number, listed());
        assert!(followed.change_answered(&ask, Ok(Some((false, Some(false))))));
        // The answer can be older than the event: the thread stays listed, and the
        // server is asked for the list and for the state.
        assert_eq!(followed.list.threads.len(), 1);
        assert!(followed.list.next_read().is_some());
        assert!(followed.thread.next_read().is_some());
    }
}
