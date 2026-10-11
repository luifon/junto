//! The messages in the stream client (ADR 0721): the reads that the events of messages
//! ask for, the pages of the places that a reader has open, the read position, and the
//! messages that wait to be sent.
//!
//! The events of messages carry no content (ADR 0408). The client reads what an event
//! names. An event that arrives while such a read is on its way does not change the
//! answer: after the answer the read starts again.

use std::collections::{BTreeMap, VecDeque};
use std::time::Duration;

use junto_protocol::ids::{AttachmentId, MessageId};
use junto_protocol::message::{Message, MessagePage, MessageState, SendMessage};
use tokio::sync::oneshot;
use tokio::task::AbortHandle;
use tokio::time::Instant;

use super::{Client, Load, Payload};
use crate::api::{ApiError, ErrorCategory, PageQuery, ServerApi};
use crate::live::{Command, LoadError};
use crate::model::{
    Part, PendingMessage, PlaceId, Reach, ReadFailure, SendFailure, SendState, ServerModel,
};
use crate::secret::SecretToken;

/// How many reads of single messages are on their way at one time.
const MESSAGE_READS: usize = 4;
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

/// The answer to a read of a page.
pub(super) enum Pages {
    /// The messages that replace what is loaded.
    Opened {
        messages: Vec<Message>,
        more_before: bool,
        more_after: bool,
    },
    Page(MessagePage),
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
    match ask {
        PageAsk::Latest => {
            let page = api.messages(place, &limit(PAGE)).await?;
            Ok(Pages::Opened {
                messages: page.messages,
                more_before: page.has_more,
                more_after: false,
            })
        }
        PageAsk::Around(position) => {
            let query = PageQuery {
                before: Some(position.saturating_add(1)),
                ..limit(READ_CONTEXT)
            };
            let before = api.messages(place, &query).await?;
            let query = PageQuery {
                after: Some(position),
                ..limit(FROM_PAGE)
            };
            let after = api.messages(place, &query).await?;
            let mut messages = before.messages;
            messages.extend(after.messages);
            Ok(Pages::Opened {
                messages,
                more_before: before.has_more,
                more_after: after.has_more,
            })
        }
        PageAsk::From(position) => {
            let query = PageQuery {
                after: Some(position.saturating_sub(1)),
                ..limit(FROM_PAGE)
            };
            let page = api.messages(place, &query).await?;
            Ok(Pages::Opened {
                messages: page.messages,
                more_before: position > 1,
                more_after: page.has_more,
            })
        }
        PageAsk::Before(message_id) => {
            let query = PageQuery {
                before_message: Some(message_id),
                ..limit(PAGE)
            };
            api.messages(place, &query).await.map(Pages::Page)
        }
        PageAsk::After(message_id) => {
            let query = PageQuery {
                after_message: Some(message_id),
                ..limit(PAGE)
            };
            api.messages(place, &query).await.map(Pages::Page)
        }
    }
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
}

/// What a request that is no read reports back.
pub(super) enum Outcome {
    Sent {
        id: u64,
        result: Result<Box<Message>, ApiError>,
    },
    /// The history after the attempt, read to find a message without an answer.
    Searched {
        id: u64,
        result: Result<Searched, ApiError>,
    },
    Marked {
        place: PlaceId,
        position: u64,
        result: Result<MessageState, ApiError>,
    },
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
        marks
            .chain(searches)
            .chain(retries)
            .filter(|at| self.advanced_at.is_none_or(|step| *at > step))
            .min()
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
        for (_, waiting) in std::mem::take(&mut work.opening) {
            for reply in waiting {
                let _ = reply.send(Err(LoadError::Off));
            }
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
            Part::Message(place, _) | Part::Messages(place, _) => self.messages.wants(place),
            Part::AttachmentLimits => !self.messages.wanted.is_empty(),
            Part::Members(_) | Part::OwnCall | Part::ChannelCall(_) => false,
        }
    }

    /// Starts a read whose content no event carries, unless the same read is on its way:
    /// that one starts again after its answer when an event asked for it meanwhile.
    pub(super) fn read(&mut self, part: Part) {
        let load = Load::Part(part);
        if self.pending.iter().any(|pending| pending.load == load) {
            return;
        }
        if let Part::Message(..) = part {
            if !self.messages.queue.contains(&part) {
                self.messages.queue.push_back(part);
            }
            self.pump_message_reads();
            return;
        }
        self.start(load);
    }

    /// Starts the reads of single messages that wait, up to the bound.
    pub(super) fn pump_message_reads(&mut self) {
        loop {
            let flying = self
                .pending
                .iter()
                .filter(|pending| matches!(pending.load, Load::Part(Part::Message(..))))
                .count();
            if flying >= MESSAGE_READS || self.halted || self.not_before.is_some() {
                return;
            }
            let Some(part) = self.messages.queue.pop_front() else {
                return;
            };
            let wanted = matches!(part, Part::Message(place, _) if self.messages.wants(place));
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
            Reach::Open => Some(match wanted.anchor {
                Some(position) => PageAsk::From(position),
                None if timeline.loaded => PageAsk::Latest,
                None => match model.message_state(place) {
                    Some(state) if state.unread > 0 && state.last_read > 0 => {
                        PageAsk::Around(state.last_read)
                    }
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
            Reach::Open | Reach::End => timeline.reading.open = reading,
            Reach::Older => timeline.reading.older = reading,
            Reach::Newer => timeline.reading.newer = reading,
        }
        self.model_changed = true;
    }

    /// Takes the answer to a read whose content no event carries. `kept` holds the events
    /// that arrived while the read was on its way.
    pub(super) fn read_answered(
        &mut self,
        part: Part,
        result: Result<Payload, ApiError>,
        kept: &[std::sync::Arc<junto_protocol::events::Event>],
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
        match (part, payload) {
            (Part::MessageState(_), Payload::MessageState(state)) => {
                if model.place(PlaceId::of(&state.place)).is_some() {
                    model.set_message_state(state);
                }
            }
            (Part::Message(place, _), Payload::Message(message)) => {
                model.put_message(*message);
                model.confirm_arrived(place);
            }
            (Part::Messages(place, reach), Payload::Pages(pages)) => {
                let first = model.timeline(place).is_some_and(|held| !held.loaded);
                let unread_after = model
                    .message_state(place)
                    .filter(|state| first && state.unread > 0)
                    .map(|state| state.last_read);
                if let Some(timeline) = model.timeline_mut(place) {
                    match (pages, reach) {
                        (
                            Pages::Opened {
                                messages,
                                more_before,
                                more_after,
                            },
                            _,
                        ) => {
                            timeline.replace(messages, more_before, more_after);
                            if first {
                                timeline.unread_after = unread_after;
                            }
                        }
                        (Pages::Page(page), Reach::Older) => timeline.add_older(page),
                        (Pages::Page(page), _) => timeline.add_newer(page),
                    }
                }
                model.confirm_arrived(place);
            }
            (Part::AttachmentLimits, Payload::Limits(limits)) => {
                model.attachment_limits = Some(limits);
            }
            _ => return,
        }
        let reads = model.replay_reads(part, kept.iter().map(std::sync::Arc::as_ref));
        self.model_changed = true;
        self.messages.retries.retain(|(held, _, _)| *held != part);
        if let Part::Messages(place, _) = part {
            self.answer_opening(place, || Ok(()));
        }
        for read in reads {
            self.read(read);
        }
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
        // A page is read again when the reader asks again. Every other read follows an
        // event that is not delivered a second time, so the client tries again by itself.
        // A reader can ask for the messages of a place again and not for its summary: the
        // read of a summary has no last try.
        let again = transient(&error)
            && match part {
                Part::Messages(..) => false,
                Part::MessageState(_) => true,
                _ => tries < READ_TRIES,
            };
        if again {
            let wait = READ_RETRY.saturating_mul(1 << tries.min(READ_TRIES));
            self.messages.retries.push((
                part,
                tries.saturating_add(1).min(READ_TRIES),
                Instant::now() + wait,
            ));
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
                // again: the loaded messages are not current any more.
                if let Some(timeline) = model.timeline_mut(place) {
                    timeline.stale = true;
                    timeline.failed = Some(ReadFailure::of(&error));
                    self.model_changed = true;
                }
            }
            Part::Messages(place, _) => {
                if let Some(timeline) = model.timeline_mut(place) {
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
            _ => {}
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
        let wanted = self.messages.wanted_places();
        let Some(model) = &mut self.model else {
            return;
        };
        if let Some(before) = before {
            model.carry_messages(before, &wanted);
        }
        let mut unseen = Vec::new();
        for place in &wanted {
            if model.place(*place).is_some() {
                model.open_timeline(*place);
            } else {
                model.close_timeline(*place);
                unseen.push(*place);
            }
        }
        for place in unseen {
            self.answer_opening(place, || Err(LoadError::NotInState));
        }
        for place in &wanted {
            self.read(Part::Messages(*place, Reach::Open));
        }
        if !wanted.is_empty() {
            self.read(Part::AttachmentLimits);
        }
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
                if reach == Reach::End {
                    if let Some(wanted) = self.messages.wanted.get_mut(&place) {
                        wanted.anchor = None;
                    }
                    // The pages before and after belong to what is replaced.
                    self.pending.retain(|pending| {
                        !matches!(pending.load, Load::Part(Part::Messages(of, _)) if of == place)
                    });
                }
                self.read(Part::Messages(place, reach));
            }
            Command::MessageAnchor { place, position } => {
                if let Some(wanted) = self.messages.wanted.get_mut(&place) {
                    wanted.anchor = position;
                }
            }
            Command::PutMessage(message) => {
                if let Some(model) = &mut self.model {
                    let place = PlaceId::of(&message.place);
                    self.model_changed |= model.put_message(*message);
                    self.model_changed |= model.confirm_arrived(place);
                }
            }
            Command::MarkRead { place, position } => {
                let read = self
                    .model
                    .as_ref()
                    .and_then(|model| model.message_state(place))
                    .map_or(0, |state| state.last_read);
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
            Command::LoadMembers { .. } | Command::ReleaseMembers(_) => {}
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
        let of_place = |part: &Part| matches!(part, Part::Message(of, _) | Part::Messages(of, _) if *of == place);
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
                let result = api.mark_read(place, position).await;
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
        result: Result<MessageState, ApiError>,
        current: bool,
    ) {
        let Some(mark) = self.messages.marks.get_mut(&place) else {
            return;
        };
        mark.flying = None;
        let overtaken = std::mem::take(&mut mark.overtaken);
        match result {
            Ok(state) => {
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
        pending.after = model
            .message_state(pending.place)
            .map_or(0, |state| state.latest);
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
                    let result = api.send_message(place, &message).await.map(Box::new);
                    let outcome = Outcome::Sent { id, result };
                    let _ = done.send(Done { token, outcome });
                })
            };
            sending.request = Some(request.abort_handle());
        }
    }

    fn sent(&mut self, id: u64, result: Result<Box<Message>, ApiError>, current: bool) {
        let Some(sending) = self.messages.sends.get_mut(&id) else {
            return;
        };
        sending.request = None;
        match result {
            Ok(message) => {
                self.messages.sends.remove(&id);
                if let Some(model) = &mut self.model {
                    model.outbox.retain(|pending| pending.id != id);
                    model.note_sent(message.id);
                    model.put_message(*message);
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
        let Some(model) = &mut self.model else {
            return;
        };
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
                if let Some(message) = found {
                    self.messages.sends.remove(&id);
                    model.outbox.retain(|held| held.id != id);
                    model.note_sent(message.id);
                    model.put_message(message);
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
