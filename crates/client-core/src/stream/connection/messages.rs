//! The messages in the stream client (ADR 0721): the reads that the events of messages
//! ask for, the pages of the places that a reader has open, the read position, and the
//! messages that wait to be sent.
//!
//! The events of messages carry no content (ADR 0408). The client reads what an event
//! names. An event that arrives while such a read is on its way does not change the
//! answer: after the answer the read starts again.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::{AttachmentId, ChannelId, MessageId};
use junto_protocol::message::{Message, MessagePage, MessageState, SendMessage, ThreadState};
use tokio::sync::oneshot;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use super::{Client, Load, Payload};
use crate::api::{ApiError, Changed, ErrorCategory, PageQuery, ServerApi};
use crate::live::{Command, LoadError};
use crate::model::{
    Followed, Part, PendingMessage, PlaceId, Reach, ReadFailure, SendFailure, SendState,
    ServerModel,
};
use crate::secret::SecretToken;

/// How many reads of single messages are on their way at one time.
const MESSAGE_READS: usize = 4;
/// How many reads of recordings and of pages of transcript lines are on their way at one
/// time. A window that opens a channel asks for every recording that it shows.
const RECORDING_READS: usize = 4;

/// True for a read that belongs to a recording.
fn of_a_recording(part: &Part) -> bool {
    matches!(part, Part::Recording(_) | Part::TranscriptLines(_))
}
/// The messages of the page that a place opens with, and of a page before or after.
const PAGE: u32 = 50;
/// The messages before the read position that a place with unread messages opens with.
const READ_CONTEXT: u32 = 30;
/// The messages of a page that starts at a known position.
const FROM_PAGE: u32 = 100;
/// The time between two requests that move the read position of one place.
const MARK_INTERVAL: Duration = Duration::from_secs(1);
/// The wait before a read position is sent again after a request without an answer.
const MARK_RETRY: Duration = Duration::from_secs(5);
/// The longest wait that the client accepts from the server for a read position.
const LONGEST_MARK_WAIT: Duration = Duration::from_secs(60);
/// How many pages of history the client reads to find a message without an answer.
const SEARCH_PAGES: usize = 5;
/// How often a read of one message that got no answer is sent again, and the wait before
/// the first of them. Each further wait is twice as long. A read of a summary is sent
/// again without an end, with the longest of these waits.
const READ_TRIES: u8 = 5;
const READ_RETRY: Duration = Duration::from_secs(2);
/// The longest wait before a read is tried again: the doubled wait of the last try, which
/// also bounds the time that a refusal names in `Retry-After`.
const LONGEST_READ_WAIT: Duration = Duration::from_secs(2 << READ_TRIES);
/// The entries of one page of the unread threads, and of one page of the notices.
pub(super) const LIST_PAGE: u32 = 200;
pub(super) const NOTICES_PAGE: u32 = 50;
/// The time after the end of a mute at which the summary of its channel is read: the
/// clock of the server decides the end.
const MUTE_MARGIN: Duration = Duration::from_secs(2);
/// The wait before the summary is read again while it says "muted" after the end that
/// the settings state: the clock of this machine is ahead of the one of the server.
const MUTE_RECHECK: Duration = Duration::from_secs(30);

type Reply = oneshot::Sender<Result<(), LoadError>>;

/// Which messages one read of a page asks the server for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PageAsk {
    Latest,
    /// The messages up to a read position, and the messages after it.
    Around(u64),
    /// The messages from a position on.
    From(u64),
    Before(MessageId),
    After(MessageId),
}

/// The messages of the answer to a read of a page.
pub(super) enum PageKind {
    /// The messages that replace what is loaded.
    Opened {
        messages: Vec<Message>,
        more_before: bool,
        more_after: bool,
    },
    Page(MessagePage),
}

/// The answer to a read of a page.
pub(super) struct Pages {
    kind: PageKind,
    /// For a thread: its root and the channels that it was forked into.
    head: Option<(Message, Vec<ChannelId>)>,
}

/// One page of a place. The page of a thread also carries its root and its forks.
async fn page_of(
    api: &ServerApi,
    place: PlaceId,
    query: &PageQuery,
    head: &mut Option<(Message, Vec<ChannelId>)>,
) -> Result<MessagePage, ApiError> {
    match place.thread_root() {
        Some(root_id) => {
            let page = api.thread(root_id, query).await?;
            *head = Some((page.root, page.forks));
            Ok(page.replies)
        }
        None => api.messages(place, query).await,
    }
}

pub(super) async fn read_pages(
    api: &ServerApi,
    place: PlaceId,
    ask: PageAsk,
) -> Result<Pages, ApiError> {
    let limit = |limit: u32| PageQuery {
        limit: Some(limit),
        ..PageQuery::default()
    };
    let mut head = None;
    let kind = match ask {
        PageAsk::Latest => {
            let page = page_of(api, place, &limit(PAGE), &mut head).await?;
            PageKind::Opened {
                messages: page.messages,
                more_before: page.has_more,
                more_after: false,
            }
        }
        PageAsk::Around(position) => {
            let query = PageQuery {
                before: Some(position.saturating_add(1)),
                ..limit(READ_CONTEXT)
            };
            let before = page_of(api, place, &query, &mut head).await?;
            let query = PageQuery {
                after: Some(position),
                ..limit(FROM_PAGE)
            };
            let after = page_of(api, place, &query, &mut head).await?;
            let mut messages = before.messages;
            messages.extend(after.messages);
            PageKind::Opened {
                messages,
                more_before: before.has_more,
                more_after: after.has_more,
            }
        }
        PageAsk::From(position) => {
            let query = PageQuery {
                after: Some(position.saturating_sub(1)),
                ..limit(FROM_PAGE)
            };
            let page = page_of(api, place, &query, &mut head).await?;
            PageKind::Opened {
                messages: page.messages,
                more_before: position > 1,
                more_after: page.has_more,
            }
        }
        PageAsk::Before(message_id) => {
            let query = PageQuery {
                before_message: Some(message_id),
                ..limit(PAGE)
            };
            PageKind::Page(page_of(api, place, &query, &mut head).await?)
        }
        PageAsk::After(message_id) => {
            let query = PageQuery {
                after_message: Some(message_id),
                ..limit(PAGE)
            };
            PageKind::Page(page_of(api, place, &query, &mut head).await?)
        }
    };
    Ok(Pages { kind, head })
}

/// A place whose messages a reader has open.
#[derive(Debug, Default)]
struct Wanted {
    readers: usize,
    /// The position that the reader shows. Absent while the reader follows the end.
    anchor: Option<u64>,
}

/// The read position of one place that the person reached and the server may not have.
#[derive(Debug, Default)]
struct Mark {
    wanted: u64,
    /// The position that the server has confirmed, or refused for good.
    settled: u64,
    flying: Option<AbortHandle>,
    next_at: Option<Instant>,
    /// An event changed the summary of the place while the request was on its way: the
    /// answer can be older than that event. It is not applied, and the summary is read.
    overtaken: bool,
}

/// What the stream client does for one waiting message.
#[derive(Debug, Default)]
struct Sending {
    /// The request that sends the message, or that searches the history for it.
    request: Option<AbortHandle>,
    /// When the last request that may have reached the server was sent.
    attempted_at: Option<Instant>,
    /// The person asked to send the message again: the history is searched at this time,
    /// and the message is sent when the history does not hold it.
    search_at: Option<Instant>,
    /// The place whose history the search that is on its way reads, with the events of the
    /// messages of that place that arrived since it was sent. Its answer can be older than
    /// them (ADR 0721).
    searching: Option<(PlaceId, Vec<Arc<Event>>)>,
    /// More events arrived during the search than are kept: its answer is not used, and
    /// the history is searched again.
    search_overtaken: bool,
}

/// What a request that is no read reports back.
pub(super) enum Outcome {
    Sent {
        id: u64,
        result: Result<Box<Changed<Message>>, ApiError>,
    },
    /// The history after the attempt, read to find a message without an answer.
    Searched {
        id: u64,
        result: Result<Searched, ApiError>,
    },
    Marked {
        place: PlaceId,
        position: u64,
        result: Result<Marked, ApiError>,
    },
}

/// The answer to a read position: the summary of a place, or the state of a thread.
pub(super) enum Marked {
    Place(MessageState),
    Thread(ThreadState),
}

/// The history that one search read.
pub(super) struct Searched {
    messages: Vec<Message>,
    /// The search reached the end of the history.
    complete: bool,
}

/// The outcome of a request with the session that carried it. A request of a session
/// before the present one does not end the present session.
pub(super) struct Done {
    token: SecretToken,
    outcome: Outcome,
}

/// A list that is read again after every event of a kind. The limit of the server for
/// reads is shared with the history (ADR 0409), so two reads of the list keep a distance,
/// and the events during that distance make one read at its end.
#[derive(Debug, Default, Clone, Copy)]
struct Spaced {
    /// The earliest time of the next read.
    at: Option<Instant>,
    /// A read waits for that time.
    wait: bool,
}

impl Spaced {
    /// True when the read can be sent now; the next one waits for `interval`. Otherwise
    /// the read waits for the end of the distance.
    fn take(&mut self, now: Instant, interval: Duration) -> bool {
        if self.at.is_some_and(|at| at > now) {
            self.wait = true;
            return false;
        }
        self.at = Some(now + interval);
        self.wait = false;
        true
    }

    /// No read is sent before `until`.
    fn hold(&mut self, until: Instant) {
        self.at = Some(self.at.map_or(until, |at| at.max(until)));
    }

    /// The time of a read that waits.
    fn due(self) -> Option<Instant> {
        self.at.filter(|_| self.wait)
    }

    /// True when a read waited and its time has come.
    fn ready(&mut self, now: Instant) -> bool {
        if self.wait && self.at.is_none_or(|at| at <= now) {
            self.wait = false;
            return true;
        }
        false
    }
}

#[derive(Default)]
pub(super) struct Work {
    wanted: BTreeMap<PlaceId, Wanted>,
    /// The readers that wait for the first page of a place.
    opening: BTreeMap<PlaceId, Vec<Reply>>,
    /// The single messages that wait for a free read.
    queue: VecDeque<Part>,
    /// The reads that got no answer: how often, and when the next try is due.
    retries: Vec<(Part, u8, Instant)>,
    marks: BTreeMap<PlaceId, Mark>,
    sends: BTreeMap<u64, Sending>,
    /// The time of the last step that started what was due.
    advanced_at: Option<Instant>,
    /// The channels whose pin list a reader has open, with the number of their readers.
    pins: BTreeMap<ChannelId, usize>,
    /// The readers that wait for the pin list of a channel.
    opening_pins: BTreeMap<ChannelId, Vec<Reply>>,
    /// The reads of the unread threads and of the first page of the notices: every change
    /// of a thread or of a notice asks for them, and two reads of one keep a distance.
    threads: Spaced,
    notices: Spaced,
    /// The continuation that the read of a further page of unread threads was sent with.
    pub(super) continued_threads: Option<String>,
    /// The times at which the mute of a channel ends (ADR 0409).
    mute_ends: Vec<(Instant, ChannelId)>,
}

impl Drop for Work {
    fn drop(&mut self) {
        self.end_requests();
    }
}

impl Work {
    fn end_requests(&mut self) {
        for mark in self.marks.values_mut() {
            if let Some(request) = mark.flying.take() {
                request.abort();
            }
        }
        for sending in self.sends.values_mut() {
            if let Some(request) = sending.request.take() {
                request.abort();
            }
        }
    }

    pub(super) fn wants(&self, place: PlaceId) -> bool {
        self.wanted.contains_key(&place)
    }

    pub(super) fn wanted_places(&self) -> Vec<PlaceId> {
        self.wanted.keys().copied().collect()
    }

    /// Forgets the reads that wait for another try, for the parts that `of` names.
    pub(super) fn drop_queued(&mut self, of: impl Fn(&Part) -> bool) {
        self.queue.retain(|part| !of(part));
    }

    pub(super) fn drop_retries(&mut self, of: impl Fn(&Part) -> bool) {
        self.retries.retain(|(part, _, _)| !of(part));
    }

    /// The channels whose pin list a reader has open, as places.
    pub(super) fn wanted_pins(&self) -> Vec<PlaceId> {
        self.pins.keys().copied().map(PlaceId::Channel).collect()
    }

    /// The next moment at which a read position, a retry or a search is due, of the
    /// moments after the last step. What was due at that step was started in it, or waits
    /// for the answer to a request, and that answer wakes the task. A moment that has
    /// passed since the step is named: the task wakes at once and starts it.
    pub(super) fn next_due(&self) -> Option<Instant> {
        let marks = self
            .marks
            .values()
            .filter(|mark| mark.flying.is_none() && mark.wanted > mark.settled)
            .filter_map(|mark| mark.next_at);
        let searches = self
            .sends
            .values()
            .filter(|sending| sending.request.is_none())
            .filter_map(|sending| sending.search_at);
        let retries = self.retries.iter().map(|(_, _, due)| *due);
        let spaced = [self.threads.due(), self.notices.due()]
            .into_iter()
            .flatten();
        let mutes = self.mute_ends.iter().map(|(due, _)| *due);
        marks
            .chain(searches)
            .chain(retries)
            .chain(spaced)
            .chain(mutes)
            .filter(|at| self.advanced_at.is_none_or(|step| *at > step))
            .min()
    }

    /// Keeps an event of the messages of a place whose history a search reads now.
    pub(super) fn keep_for_searches(&mut self, event: &Arc<Event>, model: &ServerModel) {
        for sending in self.sends.values_mut() {
            let Some((place, kept)) = &mut sending.searching else {
                continue;
            };
            if !Part::Messages(*place, Reach::Open).concerns(event, model) {
                continue;
            }
            if kept.len() >= super::MOST_KEPT_EVENTS {
                kept.clear();
                sending.search_overtaken = true;
            } else if !sending.search_overtaken {
                kept.push(event.clone());
            }
        }
    }

    /// The state was replaced by a snapshot: the events between the state before and the
    /// new one were not told, so the answer of a search that is on its way is not used.
    pub(super) fn overtake_searches(&mut self) {
        for sending in self.sends.values_mut() {
            if let Some((_, kept)) = &mut sending.searching {
                kept.clear();
                sending.search_overtaken = true;
            }
        }
    }

    /// An event arrived. A read position whose request is on its way gets an answer that
    /// can be older than the event, when the event changes the summary of its place.
    pub(super) fn overtake_marks(
        &mut self,
        event: &junto_protocol::events::Event,
        model: &ServerModel,
    ) {
        for (place, mark) in &mut self.marks {
            if mark.flying.is_some() && Part::MessageState(*place).concerns(event, model) {
                mark.overtaken = true;
            }
        }
    }
}

fn transient(error: &ApiError) -> bool {
    error.is_unavailable() || error.category() == Some(ErrorCategory::Limited)
}

impl Client {
    /// The account and its state are gone: nothing of the messages stays.
    pub(super) fn forget_messages(&mut self) {
        let mut work = std::mem::take(&mut self.messages);
        work.end_requests();
        let waiting = std::mem::take(&mut work.opening)
            .into_values()
            .chain(std::mem::take(&mut work.opening_pins).into_values());
        for reply in waiting.flatten() {
            let _ = reply.send(Err(LoadError::Off));
        }
    }

    /// The session changed. A read position is sent again with the next session. A
    /// message that is on its way keeps its request: the server may have taken it.
    pub(super) fn park_messages(&mut self) {
        for mark in self.messages.marks.values_mut() {
            if let Some(request) = mark.flying.take() {
                request.abort();
            }
        }
    }

    /// True when the state still needs a read that a change of the session interrupted.
    pub(super) fn message_read_needed(&self, part: Part, model: &ServerModel) -> bool {
        match part {
            Part::MessageState(place) => model.place(place).is_some(),
            Part::Message(place, message_id) => model.wants_message(place, message_id, None),
            Part::Messages(place, _) | Part::ThreadHead(place) => self.messages.wants(place),
            Part::AttachmentLimits => !self.messages.wanted.is_empty(),
            Part::UnreadThreads | Part::Notices => true,
            // A reader asks for a further page again.
            Part::MoreThreads | Part::MoreNotices(_) => false,
            Part::Pins(channel_id) => self.messages.pins.contains_key(&channel_id),
            Part::Named(message_id) => model.names(message_id),
            Part::Recording(recording_id) | Part::TranscriptLines(recording_id) => {
                self.recordings.wants(recording_id)
            }
            Part::Members(_) | Part::OwnCall | Part::Admin(_) => false,
        }
    }

    /// Starts a read whose content no event carries, unless the same read is on its way:
    /// that one starts again after its answer when an event asked for it meanwhile.
    pub(super) fn read(&mut self, part: Part) {
        let load = Load::Part(part);
        if self.pending.iter().any(|pending| pending.load == load) {
            return;
        }
        if let Part::Message(..) | Part::Named(_) | Part::Recording(_) | Part::TranscriptLines(_) =
            part
        {
            if !self.messages.queue.contains(&part) {
                self.messages.queue.push_back(part);
            }
            self.pump_message_reads();
            return;
        }
        let interval = self.environment.policy.list_interval;
        let spaced = match part {
            Part::UnreadThreads => Some(&mut self.messages.threads),
            Part::Notices => Some(&mut self.messages.notices),
            _ => None,
        };
        if let Some(spaced) = spaced
            && !spaced.take(Instant::now(), interval)
        {
            return;
        }
        self.start(load);
    }

    /// Reads the messages that the lists of the state name and that the state does not
    /// hold yet.
    pub(super) fn read_missing_named(&mut self) {
        let missing = self
            .model
            .as_ref()
            .map(ServerModel::missing_named)
            .unwrap_or_default();
        for message_id in missing {
            self.read(Part::Named(message_id));
        }
    }

    /// Starts the reads that wait, up to the bound of their kind: the reads of single
    /// messages, and the reads of recordings.
    pub(super) fn pump_message_reads(&mut self) {
        loop {
            if self.halted || self.not_before.is_some() {
                return;
            }
            let flying = |recording: bool| {
                self.pending
                    .iter()
                    .filter(|pending| match &pending.load {
                        Load::Part(part @ (Part::Message(..) | Part::Named(_))) => {
                            !recording && !of_a_recording(part)
                        }
                        Load::Part(part) => recording && of_a_recording(part),
                        _ => false,
                    })
                    .count()
            };
            let room = [
                flying(false) < MESSAGE_READS,
                flying(true) < RECORDING_READS,
            ];
            // The first read that waits and whose kind has room.
            let next = self
                .messages
                .queue
                .iter()
                .position(|part| room[usize::from(of_a_recording(part))]);
            let Some(part) = next.and_then(|index| self.messages.queue.remove(index)) else {
                return;
            };
            let wanted = self.model.as_ref().is_some_and(|model| match part {
                Part::Message(place, message_id) => model.wants_message(place, message_id, None),
                Part::Named(message_id) => model.needs_named(message_id),
                Part::Recording(recording_id) | Part::TranscriptLines(recording_id) => {
                    self.recordings.wants(recording_id)
                }
                _ => false,
            });
            if wanted {
                self.start(Load::Part(part));
            }
        }
    }

    /// What a read of a page asks for now. `None` when there is nothing to read: no reader
    /// holds the place, or the history has no more messages in that direction.
    pub(super) fn page_ask(&self, place: PlaceId, reach: Reach) -> Option<PageAsk> {
        let wanted = self.messages.wanted.get(&place)?;
        let model = self.model.as_ref()?;
        let timeline = model.timeline(place)?;
        match reach {
            Reach::End => Some(PageAsk::Latest),
            Reach::At(position) => Some(PageAsk::Around(position)),
            Reach::Open => Some(match wanted.anchor {
                Some(position) => PageAsk::From(position),
                None if timeline.loaded => PageAsk::Latest,
                None => match unread_from(model, place) {
                    Some(last_read) if last_read > 0 => PageAsk::Around(last_read),
                    _ => PageAsk::Latest,
                },
            }),
            Reach::Older => timeline
                .messages
                .first()
                .filter(|_| timeline.more_before)
                .map(|first| PageAsk::Before(first.id)),
            Reach::Newer => timeline
                .messages
                .last()
                .filter(|_| timeline.more_after)
                .map(|last| PageAsk::After(last.id)),
        }
    }

    /// Gives the timeline of a place the position that its reader shows, so that a bound
    /// keeps the messages at that position (ADR 0747).
    fn anchor_timeline(&mut self, place: PlaceId) {
        let anchor = self
            .messages
            .wanted
            .get(&place)
            .and_then(|held| held.anchor);
        if let Some(model) = &mut self.model {
            self.model_changed |= model.anchor_timeline(place, anchor);
        }
    }

    /// Marks in the state that a page of a place is being read.
    pub(super) fn mark_reading(&mut self, place: PlaceId, reach: Reach, reading: bool) {
        let Some(timeline) = self
            .model
            .as_mut()
            .and_then(|model| model.timeline_mut(place))
        else {
            return;
        };
        match reach {
            Reach::Open | Reach::End | Reach::At(_) => timeline.reading.open = reading,
            Reach::Older => timeline.reading.older = reading,
            Reach::Newer => timeline.reading.newer = reading,
        }
        self.model_changed = true;
    }

    /// Takes the answer to a read whose content no event carries. `kept` holds the events
    /// that arrived while the read was on its way, and `connection` is the connection of
    /// the stream during which the read was sent.
    pub(super) fn read_answered(
        &mut self,
        part: Part,
        result: Result<Payload, ApiError>,
        kept: &[std::sync::Arc<junto_protocol::events::Event>],
        connection: Option<u64>,
    ) {
        if let Part::Messages(place, reach) = part {
            self.mark_reading(place, reach, false);
        }
        let payload = match result {
            Ok(payload) => payload,
            Err(error) => {
                self.read_failed(part, error);
                return;
            }
        };
        let Some(model) = &mut self.model else {
            return;
        };
        let mut lines_answered = None;
        match (part, payload) {
            (Part::MessageState(asked), Payload::MessageState(state)) => {
                if model.place(PlaceId::of(&state.place)).is_some() {
                    model.set_message_state(state);
                }
                if let PlaceId::Channel(channel_id) = asked {
                    self.messages.mute_ends.retain(|(_, of)| *of != channel_id);
                    if let Some(until) = model.stated_mute_end(channel_id) {
                        // Before the stated end "muted" is the expected answer, and the
                        // summary is read after that end. After it the two clocks
                        // differ, and the summary is read again.
                        let left = until - time::OffsetDateTime::now_utc();
                        let wait = match Duration::try_from(left) {
                            Ok(left) => left.checked_add(MUTE_MARGIN),
                            Err(_) => Some(MUTE_RECHECK),
                        };
                        let due = wait.and_then(|wait| Instant::now().checked_add(wait));
                        if let Some(due) = due {
                            self.messages.mute_ends.push((due, channel_id));
                        }
                    }
                }
            }
            (Part::Message(_, asked) | Part::Named(asked), Payload::Message(message)) => {
                if message.id == asked {
                    let place = PlaceId::of_message(&message);
                    model.note_received(&message, connection);
                    model.put_message(*message);
                    model.confirm_arrived(place);
                } else {
                    // Not the message that was asked for (ADR 0714): the answer is not
                    // used, and the message is not asked for again. A named message is
                    // settled as failed. The change of another message is not in the
                    // state, so its place is not current.
                    match part {
                        Part::Message(place, _) => {
                            model.mark_stale(place, ReadFailure::NoAnswer);
                        }
                        _ => {
                            model.settle_named(asked, false);
                        }
                    }
                }
            }
            (Part::Messages(place, reach), Payload::Pages(pages)) => {
                let first = model.timeline(place).is_some_and(|held| !held.loaded);
                let unread_after = unread_from(model, place).filter(|_| first);
                if let Some((root, forks)) = pages.head {
                    model.set_thread_head(place, root, forks);
                }
                let mut opened = false;
                if let Some(timeline) = model.timeline_mut(place) {
                    // A message of another place is not taken (ADR 0714), and the
                    // timeline says that its read failed.
                    let mut foreign = false;
                    let mut of_place = |messages: &mut Vec<Message>| {
                        let all = messages.len();
                        messages.retain(|message| PlaceId::of_message(message) == place);
                        foreign |= messages.len() != all;
                    };
                    match (pages.kind, reach) {
                        (
                            PageKind::Opened {
                                mut messages,
                                more_before,
                                more_after,
                            },
                            _,
                        ) => {
                            of_place(&mut messages);
                            timeline.replace(messages, more_before, more_after);
                            if first {
                                timeline.unread_after = unread_after;
                            }
                            opened = true;
                        }
                        (PageKind::Page(mut page), Reach::Older) => {
                            of_place(&mut page.messages);
                            timeline.add_older(page);
                        }
                        (PageKind::Page(mut page), _) => {
                            of_place(&mut page.messages);
                            timeline.add_newer(page);
                        }
                    }
                    if foreign {
                        timeline.failed = Some(ReadFailure::NoAnswer);
                    }
                }
                if opened {
                    model.drop_withheld(place);
                }
                model.confirm_arrived(place);
            }
            (Part::AttachmentLimits, Payload::Limits(limits)) => {
                model.attachment_limits = Some(limits);
            }
            (Part::UnreadThreads, Payload::Threads(page)) => model.set_threads(page),
            (Part::MoreThreads, Payload::Threads(page)) => {
                // The page continues the list that it was asked for. A first page that
                // arrived meanwhile replaced that list.
                let continued = self.messages.continued_threads.take();
                if continued.is_some() && continued.as_deref() == model.threads_continuation() {
                    model.add_threads(page);
                }
            }
            (Part::ThreadHead(place), Payload::ThreadHead(page)) => {
                model.set_thread_forks(place, page.root, page.forks);
            }
            (Part::Pins(channel_id), Payload::Pins(messages)) => {
                model.set_pins(channel_id, messages);
            }
            (Part::Notices, Payload::Notices(page)) => model.set_notices(page),
            (Part::MoreNotices(before), Payload::Notices(page)) => {
                model.add_notices(before, page);
            }
            (Part::Recording(recording_id), Payload::Recording(read)) => {
                let (detail, transcript) = *read;
                if !model.set_recording(recording_id, detail, transcript) {
                    self.messages.retries.retain(|(held, _, _)| *held != part);
                    self.recording_answered_wrongly(recording_id);
                    return;
                }
            }
            (Part::TranscriptLines(recording_id), Payload::Lines(read)) => {
                lines_answered = Some((recording_id, read.ask));
                model.add_transcript_lines(recording_id, read);
            }
            _ => return,
        }
        let reads = model.replay_reads(part, kept.iter().map(std::sync::Arc::as_ref));
        self.model_changed = true;
        self.messages.retries.retain(|(held, _, _)| *held != part);
        match part {
            Part::Messages(place, _) => self.answer_opening(place, || Ok(())),
            Part::Pins(channel_id) => self.answer_pins(channel_id, || Ok(())),
            Part::Recording(recording_id) => self.answer_recording(recording_id, || Ok(())),
            _ => {}
        }
        for read in reads {
            self.read(read);
        }
        if let Some((recording_id, asked)) = lines_answered {
            self.lines_due_after(recording_id, asked);
        }
        self.read_missing_named();
    }

    fn read_failed(&mut self, part: Part, error: ApiError) {
        tracing::debug!(%error, ?part, "a read of messages failed");
        let gone = error.category() == Some(ErrorCategory::NotFound);
        let tries = self
            .messages
            .retries
            .iter()
            .find(|(held, _, _)| *held == part)
            .map_or(0, |(_, tries, _)| *tries);
        self.messages.retries.retain(|(held, _, _)| *held != part);
        // The first page of a place is read again when the reader asks again. A page before
        // or after the loaded messages is tried again as other reads are: the list asks for
        // it once for each number of its rows (ADR 0740). Every other read follows an event
        // that is not delivered a second time, so the client tries again by itself. A
        // reader can ask for the messages of a place again and not for its summary: the
        // read of a summary has no last try.
        let again = transient(&error)
            && match part {
                Part::Messages(_, Reach::Older | Reach::Newer) => tries < READ_TRIES,
                Part::Messages(..)
                | Part::Pins(_)
                | Part::MoreThreads
                | Part::MoreNotices(_)
                | Part::TranscriptLines(_) => false,
                // No reader can ask for these lists, and no event tells a change a
                // second time.
                Part::MessageState(_) | Part::UnreadThreads | Part::Notices => true,
                _ => tries < READ_TRIES,
            };
        if again {
            // A server that limits the reads names the time after which it takes the next
            // one; an earlier try is refused again. The named time is bounded as the
            // longest doubled wait: a proxy can name hours.
            let named = error
                .retry_after()
                .unwrap_or_default()
                .min(LONGEST_READ_WAIT);
            let wait = READ_RETRY
                .saturating_mul(1 << tries.min(READ_TRIES))
                .max(named);
            let due = Instant::now() + wait;
            self.messages
                .retries
                .push((part, tries.saturating_add(1).min(READ_TRIES), due));
            // An event during the named time asks for the same list: it waits for the try
            // at the end of that time.
            let spaced = match part {
                Part::UnreadThreads => Some(&mut self.messages.threads),
                Part::Notices => Some(&mut self.messages.notices),
                _ => None,
            };
            if let Some(spaced) = spaced
                && !named.is_zero()
            {
                spaced.hold(due);
            }
            if let Part::Recording(recording_id) = part {
                // The reader of a recording is told of the first failure, and the state
                // tells it until a read succeeds.
                self.recording_unread(recording_id, error);
            }
            return;
        }
        let Some(model) = &mut self.model else {
            return;
        };
        match part {
            Part::Message(place, message_id) if gone => {
                self.model_changed |= model.remove_message(place, message_id);
            }
            Part::Message(place, _) => {
                // The change of this message is not in the state, and no event tells it
                // again: the loaded messages of its place and of the threads of its
                // channel are not current any more.
                self.model_changed |= model.mark_stale(place, ReadFailure::of(&error));
            }
            // The server answers no page of a channel whose history the account may not
            // read (ADR 0304). Its timeline holds the messages that the connections
            // brought.
            Part::Messages(place, Reach::Open | Reach::End | Reach::At(_))
                if error.code() == Some(ErrorCode::Forbidden) && model.reads_live_only(place) =>
            {
                self.model_changed |= model.open_live(place);
                self.answer_opening(place, || Ok(()));
            }
            Part::Messages(place, reach) => {
                // A page before or after the loaded messages starts at a message. The
                // server answers `not_found` when that message is gone, also for a place
                // that it still gives. The read around the position of that message
                // names no message, and its answer decides.
                let around = model
                    .timeline(place)
                    .filter(|_| gone)
                    .and_then(|timeline| match reach {
                        Reach::Older => timeline.messages.first(),
                        Reach::Newer => timeline.messages.last(),
                        Reach::Open | Reach::End | Reach::At(_) => None,
                    })
                    .map(|held| held.position);
                if let Some(position) = around {
                    self.read(Part::Messages(place, Reach::At(position)));
                    return;
                }
                if gone {
                    model.empty_timeline(place);
                    self.model_changed = true;
                } else if let Some(timeline) = model.timeline_mut(place) {
                    timeline.failed = Some(ReadFailure::of(&error));
                    self.model_changed = true;
                }
                let error = std::sync::Arc::new(error);
                self.answer_opening(place, || Err(LoadError::Request(error.clone())));
            }
            Part::MessageState(place) if gone => {
                let before = model.message_states.len();
                model
                    .message_states
                    .retain(|held| PlaceId::of(&held.place) != place);
                self.model_changed |= model.message_states.len() != before;
            }
            Part::Named(message_id) => {
                self.model_changed |= model.settle_named(message_id, gone);
            }
            Part::Pins(channel_id) => {
                model.set_pins_reading(channel_id, Some(ReadFailure::of(&error)));
                self.model_changed = true;
                let error = std::sync::Arc::new(error);
                self.answer_pins(channel_id, || Err(LoadError::Request(error.clone())));
            }
            // The server takes only a notice that the account reads now as the cursor of a
            // page. A refused cursor, such as a notice of a place that was deleted, would
            // refuse every further page: the notices are read again from their first page.
            Part::MoreNotices(_) if error.category() == Some(ErrorCategory::ClientFault) => {
                self.read(Part::Notices);
            }
            Part::Recording(recording_id) => self.recording_unread(recording_id, error),
            Part::TranscriptLines(recording_id) => {
                model.fail_transcript_lines(recording_id, ReadFailure::of(&error));
                self.model_changed = true;
            }
            _ => {}
        }
    }

    fn answer_pins(&mut self, channel_id: ChannelId, answer: impl Fn() -> Result<(), LoadError>) {
        let waiting = self.messages.opening_pins.remove(&channel_id);
        for waiting in waiting.unwrap_or_default() {
            self.answers.push((waiting, answer()));
        }
    }

    fn answer_opening(&mut self, place: PlaceId, answer: impl Fn() -> Result<(), LoadError>) {
        for waiting in self.messages.opening.remove(&place).unwrap_or_default() {
            self.answers.push((waiting, answer()));
        }
    }

    /// After a snapshot: the timelines that readers hold stay until their page arrives,
    /// and each is read again from where its reader is.
    pub(super) fn carry_messages(&mut self, before: Option<&ServerModel>) {
        self.messages.queue.clear();
        self.messages.retries.clear();
        self.messages.threads.wait = false;
        self.messages.notices.wait = false;
        self.messages.continued_threads = None;
        let wanted = self.messages.wanted_places();
        let Some(model) = &mut self.model else {
            return;
        };
        if let Some(before) = before {
            model.carry_messages(before, &wanted);
            model.carry_named(before);
        }
        let mut unseen = Vec::new();
        for place in &wanted {
            if model.place(*place).is_some() {
                model.open_timeline(*place);
                let anchor = self.messages.wanted.get(place).and_then(|held| held.anchor);
                model.anchor_timeline(*place, anchor);
            } else {
                model.close_timeline(*place);
                unseen.push(*place);
            }
        }
        let mut pins: Vec<ChannelId> = self.messages.pins.keys().copied().collect();
        let mut unpinned = Vec::new();
        pins.retain(|channel_id| {
            let held = model.readable_channel(*channel_id).is_some();
            if held {
                model.open_pins(*channel_id);
            } else {
                unpinned.push(*channel_id);
            }
            held
        });
        for place in unseen {
            self.answer_opening(place, || Err(LoadError::NotInState));
        }
        for channel_id in unpinned {
            self.messages.pins.remove(&channel_id);
            self.answer_pins(channel_id, || Err(LoadError::NotInState));
        }
        for place in &wanted {
            self.read(Part::Messages(*place, Reach::Open));
        }
        if !wanted.is_empty() {
            self.read(Part::AttachmentLimits);
        }
        for channel_id in pins {
            self.read(Part::Pins(channel_id));
        }
        self.read_missing_named();
        self.schedule_mute_ends();
    }

    /// Takes from the settings of the state the times at which the mute of a channel
    /// ends. No event tells that end: the server decides it by its clock when it is asked
    /// (ADR 0409), so the client reads the summary of the channel after that time.
    fn schedule_mute_ends(&mut self) {
        let Some(model) = &self.model else {
            return;
        };
        let (clock, now) = (time::OffsetDateTime::now_utc(), Instant::now());
        self.messages.mute_ends = model
            .mute_ends(clock)
            .into_iter()
            .filter_map(|(channel_id, until)| {
                // An end that has passed here while the server still says "muted" is
                // asked for at once.
                let wait = Duration::try_from(until - clock).unwrap_or_default();
                let due = now.checked_add(wait)?.checked_add(MUTE_MARGIN)?;
                Some((due, channel_id))
            })
            .collect();
    }

    /// Takes a command of a reader about messages.
    pub(super) fn message_command(&mut self, command: Command) {
        match command {
            Command::OpenMessages { place, reply } => self.open_messages(place, reply),
            Command::ReleaseMessages(place) => self.release_messages(place),
            Command::ReadMessages { place, reach } => {
                if !self.messages.wants(place) || self.stale {
                    return;
                }
                if let Reach::End | Reach::At(_) = reach {
                    if let Some(wanted) = self.messages.wanted.get_mut(&place) {
                        wanted.anchor = match reach {
                            Reach::At(position) => Some(position),
                            _ => None,
                        };
                    }
                    self.anchor_timeline(place);
                    // The pages before and after belong to what is replaced: their reads
                    // end, and the timeline does not wait for them any more.
                    self.pending.retain(|pending| {
                        !matches!(pending.load, Load::Part(Part::Messages(of, _)) if of == place)
                    });
                    self.messages.retries.retain(
                        |(part, _, _)| !matches!(part, Part::Messages(of, _) if *of == place),
                    );
                    if let Some(timeline) = self
                        .model
                        .as_mut()
                        .and_then(|model| model.timeline_mut(place))
                    {
                        timeline.reading = crate::model::Reading::default();
                        self.model_changed = true;
                    }
                }
                // A page that failed for a while waits for its next try, which the client
                // makes by itself.
                let part = Part::Messages(place, reach);
                if self
                    .messages
                    .retries
                    .iter()
                    .any(|(held, _, _)| *held == part)
                {
                    return;
                }
                self.read(part);
            }
            Command::MessageAnchor { place, position } => {
                if let Some(wanted) = self.messages.wanted.get_mut(&place) {
                    wanted.anchor = position;
                }
                self.anchor_timeline(place);
            }
            Command::MarkRead { place, position } => {
                let read = self.model.as_ref().map_or(0, |model| match place {
                    PlaceId::Thread { root_id, .. } => model
                        .thread_state(root_id)
                        .map_or(0, |state| state.last_read),
                    PlaceId::Channel(_) | PlaceId::Direct(_) => model
                        .message_state(place)
                        .map_or(0, |state| state.last_read),
                });
                let mark = self.messages.marks.entry(place).or_default();
                mark.settled = mark.settled.max(read);
                mark.wanted = mark.wanted.max(position);
            }
            Command::Enqueue(pending) => self.enqueue(*pending),
            Command::FilesReady { id, files } => self.files_ready(id, &files),
            Command::FilesFailed(id) => {
                self.set_send_state(id, SendState::Failed(SendFailure::File));
            }
            Command::SendAgain(id) => self.send_again(id),
            Command::Discard(id) => {
                if let Some(request) = self
                    .messages
                    .sends
                    .remove(&id)
                    .and_then(|sending| sending.request)
                {
                    request.abort();
                }
                if let Some(model) = &mut self.model {
                    let before = model.outbox.len();
                    model.outbox.retain(|pending| pending.id != id);
                    self.model_changed |= model.outbox.len() != before;
                }
            }
            Command::OpenPins { channel_id, reply } => self.open_pins(channel_id, reply),
            Command::ReleasePins(channel_id) => self.release_pins(channel_id),
            Command::MoreThreads => {
                if !self.stale {
                    self.read(Part::MoreThreads);
                }
            }
            Command::MoreNotices => {
                let last = self
                    .model
                    .as_ref()
                    .filter(|model| model.notices_more && !self.stale)
                    .and_then(|model| model.notices.last())
                    .map(|last| last.id);
                if let Some(last) = last {
                    self.read(Part::MoreNotices(last));
                }
            }
            Command::Refresh { .. }
            | Command::LoginUnanswered
            | Command::LoadMembers { .. }
            | Command::ReleaseMembers(_)
            | Command::OpenRecording { .. }
            | Command::ReleaseRecording(_)
            | Command::MoreTranscriptLines(_)
            | Command::EarlierTranscriptLines(_)
            | Command::TranscriptLinesAround { .. }
            | Command::OpenAdmin { .. }
            | Command::ReleaseAdmin(_)
            | Command::ReadAdminAgain(_)
            | Command::ReadAdminNow(_) => {}
        }
    }

    fn open_pins(&mut self, channel_id: ChannelId, reply: Reply) {
        if self.target.is_none() {
            let _ = reply.send(Err(LoadError::Off));
            return;
        }
        let ready = match &mut self.model {
            Some(model) if !self.stale => {
                if model.readable_channel(channel_id).is_none() {
                    let _ = reply.send(Err(LoadError::NotInState));
                    return;
                }
                model.open_pins(channel_id);
                self.model_changed = true;
                Some(
                    model
                        .pin_list(channel_id)
                        .is_some_and(|list| list.messages.loaded().is_some()),
                )
            }
            // The list is read when the state has arrived.
            _ => None,
        };
        *self.messages.pins.entry(channel_id).or_default() += 1;
        match ready {
            Some(true) => self.answers.push((reply, Ok(()))),
            Some(false) => {
                self.messages
                    .opening_pins
                    .entry(channel_id)
                    .or_default()
                    .push(reply);
                self.read(Part::Pins(channel_id));
            }
            None => self
                .messages
                .opening_pins
                .entry(channel_id)
                .or_default()
                .push(reply),
        }
    }

    fn release_pins(&mut self, channel_id: ChannelId) {
        let Some(readers) = self.messages.pins.get_mut(&channel_id) else {
            return;
        };
        *readers = readers.saturating_sub(1);
        if *readers > 0 {
            return;
        }
        self.messages.pins.remove(&channel_id);
        self.messages.opening_pins.remove(&channel_id);
        let part = Part::Pins(channel_id);
        self.resume.retain(|held| *held != part);
        self.pending
            .retain(|pending| pending.load != Load::Part(part));
        if let Some(model) = &mut self.model {
            model.close_pins(channel_id);
            self.model_changed = true;
        }
    }

    fn open_messages(&mut self, place: PlaceId, reply: Reply) {
        if self.target.is_none() {
            let _ = reply.send(Err(LoadError::Off));
            return;
        }
        let ready = match &mut self.model {
            Some(model) if !self.stale => {
                if model.place(place).is_none() {
                    let _ = reply.send(Err(LoadError::NotInState));
                    return;
                }
                model.open_timeline(place);
                let anchor = self
                    .messages
                    .wanted
                    .get(&place)
                    .and_then(|held| held.anchor);
                model.anchor_timeline(place, anchor);
                self.model_changed = true;
                Some(model.timeline(place).is_some_and(|held| held.loaded))
            }
            // The page is read when the state has arrived.
            _ => None,
        };
        self.messages.wanted.entry(place).or_default().readers += 1;
        match ready {
            Some(true) => self.answers.push((reply, Ok(()))),
            Some(false) => {
                self.messages.opening.entry(place).or_default().push(reply);
                self.read(Part::Messages(place, Reach::Open));
                let limits = self
                    .model
                    .as_ref()
                    .is_some_and(|model| model.attachment_limits.is_none());
                if limits {
                    self.read(Part::AttachmentLimits);
                }
            }
            None => self.messages.opening.entry(place).or_default().push(reply),
        }
    }

    fn release_messages(&mut self, place: PlaceId) {
        let Some(wanted) = self.messages.wanted.get_mut(&place) else {
            return;
        };
        wanted.readers = wanted.readers.saturating_sub(1);
        if wanted.readers > 0 {
            return;
        }
        self.messages.wanted.remove(&place);
        self.messages.opening.remove(&place);
        // A read of one message stays: an open thread of the channel, or its pin list,
        // can want the message still. The read is dropped at its start when nothing does.
        let of_place = |part: &Part| matches!(part, Part::Messages(of, _) | Part::ThreadHead(of) if *of == place);
        self.messages.queue.retain(|part| !of_place(part));
        self.messages.retries.retain(|(part, _, _)| !of_place(part));
        self.resume.retain(|part| !of_place(part));
        self.pending
            .retain(|pending| !matches!(&pending.load, Load::Part(part) if of_place(part)));
        if let Some(model) = &mut self.model {
            model.close_timeline(place);
            self.model_changed = true;
        }
    }

    /// Does what is due for the messages: the reads that wait for a retry, the read
    /// positions, and the messages that wait to be sent.
    pub(super) fn advance_messages(&mut self) {
        let now = Instant::now();
        self.messages.advanced_at = Some(now);
        let mut due = Vec::new();
        self.messages.retries.retain(|(part, tries, at)| {
            if *at <= now {
                due.push((*part, *tries));
                false
            } else {
                true
            }
        });
        for (part, tries) in due {
            // The count stays with the part while its read is on its way again.
            let far = now + Duration::from_secs(24 * 60 * 60);
            self.messages.retries.push((part, tries, far));
            self.read(part);
        }
        if self.messages.threads.ready(now) {
            self.read(Part::UnreadThreads);
        }
        if self.messages.notices.ready(now) {
            self.read(Part::Notices);
        }
        let mut ended = Vec::new();
        self.messages.mute_ends.retain(|(due, channel_id)| {
            if *due <= now {
                ended.push(*channel_id);
                false
            } else {
                true
            }
        });
        for channel_id in ended {
            self.read(Part::MessageState(PlaceId::Channel(channel_id)));
        }
        self.pump_message_reads();
        self.send_marks(now);
        self.pump_sends(now);
    }

    fn token(&self) -> Option<SecretToken> {
        self.target.as_ref().map(|target| target.token.clone())
    }

    fn api(&self) -> Option<ServerApi> {
        let target = self.target.as_ref()?;
        Some(
            crate::api::ApiClient::new(
                self.environment.transport.clone(),
                target.address.clone(),
                target.server_id,
                target.api_version,
                self.environment.retry,
            )
            .with_token(target.token.clone()),
        )
    }

    fn send_marks(&mut self, now: Instant) {
        let (Some(api), Some(token)) = (self.api(), self.token()) else {
            return;
        };
        for (place, mark) in &mut self.messages.marks {
            let due = mark.flying.is_none()
                && mark.wanted > mark.settled
                && mark.next_at.is_none_or(|at| at <= now);
            if !due {
                continue;
            }
            let (place, position, api, done) =
                (*place, mark.wanted, api.clone(), self.done.clone());
            let token = token.clone();
            let request = tokio::spawn(async move {
                let result = match place {
                    PlaceId::Thread { root_id, .. } => api
                        .mark_thread_read(root_id, position)
                        .await
                        .map(Marked::Thread),
                    PlaceId::Channel(_) | PlaceId::Direct(_) => {
                        api.mark_read(place, position).await.map(Marked::Place)
                    }
                };
                let outcome = Outcome::Marked {
                    place,
                    position,
                    result,
                };
                let _ = done.send(Done { token, outcome });
            });
            mark.flying = Some(request.abort_handle());
            mark.overtaken = false;
            mark.next_at = Some(now + MARK_INTERVAL);
        }
    }

    pub(super) fn done(&mut self, done: Done) {
        // An ended session of a request that an earlier session carried is known already.
        let current = self.token().is_some_and(|token| token == done.token);
        match done.outcome {
            Outcome::Marked {
                place,
                position,
                result,
            } => self.marked(place, position, result, current),
            Outcome::Sent { id, result } => self.sent(id, result, current),
            Outcome::Searched { id, result } => self.searched(id, result, current),
        }
    }

    fn marked(
        &mut self,
        place: PlaceId,
        position: u64,
        result: Result<Marked, ApiError>,
        current: bool,
    ) {
        let Some(mark) = self.messages.marks.get_mut(&place) else {
            return;
        };
        mark.flying = None;
        let overtaken = std::mem::take(&mut mark.overtaken);
        match result {
            Ok(Marked::Place(state)) => {
                mark.settled = mark.settled.max(position);
                if overtaken {
                    self.read(Part::MessageState(place));
                } else if let Some(model) = &mut self.model
                    && model.place(place).is_some()
                {
                    model.set_message_state(state);
                    self.model_changed = true;
                }
            }
            Ok(Marked::Thread(state)) => {
                mark.settled = mark.settled.max(position);
                // The list can hold a reply or a read position of another device that
                // arrived after the server made this answer. Then the answer is not
                // applied, and the list is read.
                let older = self.model.as_ref().is_some_and(|model| {
                    model.thread_state(state.root_id).is_some_and(|held| {
                        held.latest > state.latest || held.last_read > state.last_read
                    })
                });
                if older {
                    self.read(Part::UnreadThreads);
                } else if let Some(model) = &mut self.model {
                    self.model_changed |= model.set_thread_state(state);
                }
                // An unread mention of a thread counts in the summary of its channel.
                self.read(Part::MessageState(place.container()));
            }
            Err(error) if error.category() == Some(ErrorCategory::SessionEnded) => {
                if current {
                    self.ended(None);
                }
            }
            Err(error) if transient(&error) => {
                let wait = error
                    .retry_after()
                    .unwrap_or(MARK_RETRY)
                    .min(LONGEST_MARK_WAIT);
                mark.next_at = Some(Instant::now() + wait);
            }
            Err(error) => {
                // The server does not take this position: it is not sent again.
                tracing::debug!(%error, "a read position was refused");
                mark.settled = mark.settled.max(position);
            }
        }
    }

    fn set_send_state(&mut self, id: u64, state: SendState) {
        if let Some(model) = &mut self.model
            && let Some(pending) = model.pending_mut(id)
            && pending.state != state
        {
            pending.state = state;
            self.model_changed = true;
        }
    }

    fn enqueue(&mut self, mut pending: PendingMessage) {
        let Some(model) = &mut self.model else {
            return;
        };
        // A reply in a thread gets a position of its channel.
        pending.after = model
            .latest_known(pending.place)
            .max(model.latest_known(pending.place.container()));
        self.messages.sends.insert(pending.id, Sending::default());
        model.outbox.push(std::sync::Arc::new(pending));
        self.model_changed = true;
    }

    fn files_ready(&mut self, id: u64, files: &[(u64, AttachmentId)]) {
        let Some(pending) = self.model.as_mut().and_then(|model| model.pending_mut(id)) else {
            return;
        };
        for file in &mut pending.files {
            if let Some((_, attachment_id)) =
                files.iter().find(|(upload, _)| *upload == file.upload)
            {
                file.attachment_id = Some(*attachment_id);
            }
        }
        if pending.state == SendState::WaitingForFiles && pending.attachment_ids().is_some() {
            pending.state = SendState::Sending;
        }
        self.model_changed = true;
    }

    fn send_again(&mut self, id: u64) {
        let Some(state) = self
            .model
            .as_ref()
            .and_then(|model| model.outbox.iter().find(|pending| pending.id == id))
            .map(|pending| pending.state)
        else {
            return;
        };
        let sending = self.messages.sends.entry(id).or_default();
        match state {
            SendState::Failed(SendFailure::File)
            | SendState::WaitingForFiles
            | SendState::Sending => return,
            SendState::Failed(SendFailure::NotSent | SendFailure::Refused { .. }) => {
                sending.attempted_at = None;
                sending.search_at = None;
            }
            SendState::Unconfirmed => {
                let attempted = sending.attempted_at.unwrap_or_else(Instant::now);
                sending.search_at = Some(attempted + self.environment.policy.send_again_after);
            }
        }
        self.set_send_state(id, SendState::Sending);
    }

    /// Sends the waiting messages that are due. One request per place is on its way at a
    /// time, so that the messages of a place arrive in the order in which the person sent
    /// them.
    fn pump_sends(&mut self, now: Instant) {
        let (Some(api), Some(token)) = (self.api(), self.token()) else {
            return;
        };
        let Some(model) = &self.model else {
            return;
        };
        let mut busy: Vec<PlaceId> = Vec::new();
        let mut starts = Vec::new();
        for pending in &model.outbox {
            let Some(sending) = self.messages.sends.get(&pending.id) else {
                continue;
            };
            if busy.contains(&pending.place) {
                continue;
            }
            if sending.request.is_some() {
                busy.push(pending.place);
                continue;
            }
            if pending.state != SendState::Sending {
                continue;
            }
            // A message that waits for its search holds the place: a later message of
            // the place is not sent before it.
            busy.push(pending.place);
            match sending.search_at {
                Some(at) if at > now => {}
                search => starts.push((pending.clone(), search.is_some())),
            }
        }
        for (pending, search) in starts {
            let (id, place, done, api) =
                (pending.id, pending.place, self.done.clone(), api.clone());
            let Some(sending) = self.messages.sends.get_mut(&id) else {
                continue;
            };
            let token = token.clone();
            let request = if search {
                let after = pending.after;
                sending.searching = Some((place, Vec::new()));
                sending.search_overtaken = false;
                tokio::spawn(async move {
                    let result = history_after(&api, place, after).await;
                    let outcome = Outcome::Searched { id, result };
                    let _ = done.send(Done { token, outcome });
                })
            } else {
                let Some(attachments) = pending.attachment_ids() else {
                    continue;
                };
                sending.attempted_at = Some(now);
                let message = SendMessage {
                    document: pending.document.clone(),
                    reply_to: pending.reply_to,
                    attachments,
                };
                tokio::spawn(async move {
                    let result = api
                        .send_message_with_receipt(place, &message)
                        .await
                        .map(Box::new);
                    let outcome = Outcome::Sent { id, result };
                    let _ = done.send(Done { token, outcome });
                })
            };
            sending.request = Some(request.abort_handle());
        }
    }

    fn sent(&mut self, id: u64, result: Result<Box<Changed<Message>>, ApiError>, current: bool) {
        let Some(sending) = self.messages.sends.get_mut(&id) else {
            return;
        };
        sending.request = None;
        match result {
            Ok(sent) => {
                self.messages.sends.remove(&id);
                if let Some(model) = &mut self.model {
                    model.outbox.retain(|pending| pending.id != id);
                    model.note_sent(sent.value.id);
                    // The events of the message decide when the state has followed them
                    // already: a later edit or deletion is not undone by the answer
                    // (ADR 0214). An answer without a stated position is used as before.
                    if model.followed(sent.receipt) != Followed::Yes {
                        model.put_message(sent.value);
                    }
                    self.model_changed = true;
                }
            }
            Err(error) if error.category() == Some(ErrorCategory::SessionEnded) => {
                // The server did not take the request. It is sent with the next session.
                sending.attempted_at = None;
                if current {
                    self.ended(None);
                }
            }
            Err(ApiError::Rejected(rejection)) => {
                sending.attempted_at = None;
                self.set_send_state(
                    id,
                    SendState::Failed(SendFailure::Refused {
                        code: rejection.code,
                        status: rejection.status,
                    }),
                );
            }
            Err(ApiError::Unreachable(error)) if !error.request_may_have_arrived() => {
                sending.attempted_at = None;
                self.set_send_state(id, SendState::Failed(SendFailure::NotSent));
            }
            Err(error) => {
                tracing::debug!(%error, "a message was sent and no answer arrived");
                self.set_send_state(id, SendState::Unconfirmed);
                let place = self
                    .model
                    .as_ref()
                    .and_then(|model| model.outbox.iter().find(|pending| pending.id == id))
                    .map(|pending| pending.place);
                if let (Some(place), Some(model)) = (place, &mut self.model) {
                    self.model_changed |= model.confirm_arrived(place);
                }
            }
        }
    }

    /// The history after the attempt arrived: the message is confirmed when the history
    /// holds it, and sent again otherwise.
    fn searched(&mut self, id: u64, result: Result<Searched, ApiError>, current: bool) {
        let Some(sending) = self.messages.sends.get_mut(&id) else {
            return;
        };
        sending.request = None;
        let kept = sending
            .searching
            .take()
            .map(|(_, kept)| kept)
            .unwrap_or_default();
        let overtaken = std::mem::take(&mut sending.search_overtaken);
        let Some(model) = &mut self.model else {
            return;
        };
        let mut reads = Vec::new();
        let Some(pending) = model
            .outbox
            .iter()
            .find(|pending| pending.id == id)
            .cloned()
        else {
            return;
        };
        match result {
            Ok(history) => {
                sending.search_at = None;
                let own = model.account.id;
                let mut probe = pending.as_ref().clone();
                probe.state = SendState::Unconfirmed;
                let found = history
                    .messages
                    .iter()
                    .find(|message| {
                        !model.sent_ids.contains(&message.id) && probe.is_confirmed_by(message, own)
                    })
                    .cloned();
                if overtaken {
                    // The answer can be older than events that were not kept: the history
                    // is searched again.
                    sending.search_at = Some(Instant::now());
                } else if let Some(message) = found {
                    self.messages.sends.remove(&id);
                    model.outbox.retain(|held| held.id != id);
                    model.note_sent(message.id);
                    // The events of the message that arrived during the search are applied
                    // after the answer: a later edit or deletion is not undone by it.
                    let part = Part::Message(PlaceId::of_message(&message), message.id);
                    model.put_message(message);
                    reads = model.replay_reads(part, kept.iter().map(Arc::as_ref));
                    self.model_changed = true;
                } else if history.complete {
                    // The server does not hold the message: the next step sends it.
                    sending.attempted_at = None;
                } else {
                    // The history goes on after what was read, and the server may hold
                    // the message there. Nothing is sent. The next search starts after
                    // the last message of this one.
                    let reached = history.messages.last().map(|message| message.position);
                    if let Some(held) = model.pending_mut(id) {
                        held.after = reached.map_or(held.after, |reached| reached.max(held.after));
                        held.state = SendState::Unconfirmed;
                    }
                    self.model_changed = true;
                }
            }
            Err(error) if error.category() == Some(ErrorCategory::SessionEnded) => {
                // The search runs again with the next session.
                if current {
                    self.ended(None);
                }
            }
            Err(error) => {
                tracing::debug!(%error, "the history was not read; the message stays unconfirmed");
                sending.search_at = None;
                self.set_send_state(id, SendState::Unconfirmed);
            }
        }
        for read in reads {
            self.read(read);
        }
    }
}

/// The messages of a place after a position, up to a bound of pages.
async fn history_after(api: &ServerApi, place: PlaceId, after: u64) -> Result<Searched, ApiError> {
    let mut messages = Vec::new();
    let mut query = PageQuery {
        after: Some(after),
        limit: Some(200),
        ..PageQuery::default()
    };
    for _ in 0..SEARCH_PAGES {
        let page = api.messages(place, &query).await?;
        let last = page.messages.last().map(|message| message.id);
        messages.extend(page.messages);
        match last {
            Some(last) if page.has_more => {
                query = PageQuery {
                    after_message: Some(last),
                    limit: Some(200),
                    ..PageQuery::default()
                };
            }
            _ => {
                return Ok(Searched {
                    messages,
                    complete: true,
                });
            }
        }
    }
    Ok(Searched {
        messages,
        complete: false,
    })
}

/// The read position that a place opens at: the one of the summary of a place, or of the
/// state of a thread, while something after it is unread.
fn unread_from(model: &ServerModel, place: PlaceId) -> Option<u64> {
    match place {
        PlaceId::Thread { root_id, .. } => model
            .thread_state(root_id)
            .filter(|state| state.unread > 0 || state.mentions > 0)
            .map(|state| state.last_read),
        PlaceId::Channel(_) | PlaceId::Direct(_) => model
            .message_state(place)
            .filter(|state| state.unread > 0)
            .map(|state| state.last_read),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn nothing_is_due_for_a_search_that_is_on_its_way() {
        let started = Instant::now();
        tokio::time::advance(Duration::from_secs(60)).await;
        let now = Instant::now();
        let request = tokio::spawn(std::future::pending::<()>());
        let mut work = Work::default();
        work.sends.insert(
            1,
            Sending {
                request: Some(request.abort_handle()),
                attempted_at: Some(started),
                search_at: Some(started + Duration::from_secs(30)),
                ..Sending::default()
            },
        );
        // The time of the search has passed and its request runs: the task has nothing
        // to wake for.
        work.advanced_at = Some(now);
        assert_eq!(work.next_due(), None);
        // A search that is due later is a reason to wake.
        let later = now + Duration::from_secs(5);
        work.sends.insert(
            2,
            Sending {
                request: None,
                attempted_at: Some(now),
                search_at: Some(later),
                ..Sending::default()
            },
        );
        assert_eq!(work.next_due(), Some(later));
    }

    #[tokio::test(start_paused = true)]
    async fn what_became_due_after_the_last_step_wakes_the_task() {
        let step = Instant::now();
        let due = step + Duration::from_millis(1);
        let mut work = Work::default();
        work.advanced_at = Some(step);
        work.sends.insert(
            1,
            Sending {
                request: None,
                attempted_at: Some(step),
                search_at: Some(due),
                ..Sending::default()
            },
        );
        work.marks.insert(
            PlaceId::Channel(junto_protocol::ids::ChannelId::generate()),
            Mark {
                wanted: 2,
                settled: 1,
                next_at: Some(due),
                ..Mark::default()
            },
        );
        // The step did not start them: they were not due. The time has passed when the
        // task asks what is due next, and they are still named.
        tokio::time::advance(Duration::from_millis(5)).await;
        assert_eq!(work.next_due(), Some(due));
    }
}
