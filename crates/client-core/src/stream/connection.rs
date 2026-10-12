//! The stream client of one server: one task that owns the state of the server, holds the
//! connection, applies what arrives and connects again after a loss (ADR 0718).
//!
//! The task does one thing at a time, so the state has one writer and needs no lock.
//! Requests to the server run as tasks of their own and report back, and the task goes on
//! reading the connection while they run.

mod messages;
mod recordings;

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use junto_protocol::call::{ChannelCall, OwnCallView};
use junto_protocol::discovery::NotificationPage;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{
    ClientMessage, CloseCode, Cursor, EndReason, Event, Level, Received, ServerMessage, decode,
};
use junto_protocol::ids::WorkspaceId;
use junto_protocol::message::{AttachmentLimits, Message, MessageState, ThreadPage, UnreadThreads};
use junto_protocol::recording::RecordingDetail;
use junto_protocol::snapshot::Snapshot;
use junto_protocol::structure::WorkspaceMember;
use junto_protocol::transcript::Transcript;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::AbortHandle;
use tokio::time::Instant;

use super::socket::{Incoming, OpenError, StreamRequest, StreamSocket};
use super::{Desired, Environment, Host, StreamPolicy};
use crate::api::error::read_rejection;
use crate::api::{
    ApiClient, ApiError, ErrorCategory, PageQuery, TransportError, TransportErrorKind,
};
use crate::events::{ConnectionLevel, StreamTarget};
use crate::live::{Command, LiveView, LoadError, Notice, Shared, StreamFault, StreamStatus};
use crate::model::{Follow, LinesRead, Loadable, Part, ServerModel};

const MINUTE: Duration = Duration::from_secs(60);
/// How many frames that wait are applied before the readers get the new state.
const FRAMES_PER_VIEW: usize = 256;
/// How many events, and how many bytes of their messages, are kept for a part that is
/// being read. A read that lasts for more starts again.
const MOST_KEPT_EVENTS: usize = 4096;
const MOST_KEPT_BYTES: usize = 4 * 1024 * 1024;
/// After this many bytes of applied events the state is replaced by a snapshot, whose size
/// is bounded (ADR 0714). A server therefore cannot make the state grow without a bound.
const MOST_APPLIED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Load {
    Snapshot,
    Part(Part),
}

enum Payload {
    Snapshot(Box<Snapshot>),
    Members(Vec<WorkspaceMember>),
    OwnCall(OwnCallView),
    ChannelCall(ChannelCall),
    MessageState(MessageState),
    Message(Box<Message>),
    Pages(messages::Pages),
    Limits(AttachmentLimits),
    Threads(UnreadThreads),
    ThreadHead(Box<ThreadPage>),
    Pins(Vec<Message>),
    Notices(NotificationPage),
    Recording(Box<(RecordingDetail, Transcript)>),
    Lines(LinesRead),
}

struct Loaded {
    ticket: u64,
    result: Result<Payload, ApiError>,
}

/// A read that is on its way, with the events that arrived since it was sent.
struct Pending {
    ticket: u64,
    load: Load,
    events: Vec<Arc<Event>>,
    bytes: usize,
    request: AbortHandle,
}

impl Drop for Pending {
    /// A read that nobody waits for any more ends its request.
    fn drop(&mut self) {
        self.request.abort();
    }
}

/// A connection attempt that is on its way.
struct Opening {
    result: oneshot::Receiver<Result<StreamSocket, OpenError>>,
    attempt: AbortHandle,
}

impl Drop for Opening {
    fn drop(&mut self) {
        self.attempt.abort();
    }
}

enum Subscription {
    None,
    Asked {
        level: Level,
        cursor: Cursor,
        deadline: Instant,
    },
    Live {
        level: Level,
        since: Instant,
    },
}

struct Link {
    incoming: mpsc::Receiver<Incoming>,
    outgoing: mpsc::Sender<String>,
    greeted: bool,
    /// The longest time without a frame.
    silence: Duration,
    /// The time by which the next frame must arrive.
    deadline: Instant,
    subscription: Subscription,
    /// When the messages of the last minute were sent.
    sent: VecDeque<Instant>,
    /// The time at which the next message may be sent, when the minute is used up.
    send_at: Option<Instant>,
}

/// The waits between attempts: each twice the one before, up to a bound, and of each a
/// random part between a half and the whole, so that the clients of a server that
/// restarted do not all return at the same moment.
struct Backoff {
    first: Duration,
    longest: Duration,
    next: Duration,
}

impl Backoff {
    fn new(first: Duration, longest: Duration) -> Self {
        Self {
            first,
            longest,
            next: first,
        }
    }

    fn reset(&mut self) {
        self.next = self.first;
    }

    fn wait(&mut self) -> Duration {
        let whole = self.next;
        self.next = self.next.saturating_mul(2).min(self.longest);
        let half = whole / 2;
        let random = getrandom::u64().unwrap_or(u64::MAX);
        let part = u64::try_from(half.as_millis()).unwrap_or(u64::MAX);
        half + Duration::from_millis(random % part.saturating_add(1))
    }
}

fn fault_of(error: &ApiError) -> StreamFault {
    match error {
        ApiError::Unreachable(error) => StreamFault::Unreachable(error.kind),
        ApiError::Rejected(rejection) => StreamFault::Refused(rejection.code),
        _ => StreamFault::Unreadable,
    }
}

fn wire(level: ConnectionLevel) -> Option<Level> {
    match level {
        ConnectionLevel::Off => None,
        ConnectionLevel::Background => Some(Level::Background),
        ConnectionLevel::Selected => Some(Level::Full),
    }
}

async fn opened(waiting: &mut Option<Opening>) -> Result<StreamSocket, OpenError> {
    match waiting {
        Some(opening) => (&mut opening.result).await.unwrap_or_else(|_| {
            Err(OpenError::Failed(TransportError::new(
                TransportErrorKind::Interrupted,
                "the attempt stopped",
            )))
        }),
        None => std::future::pending().await,
    }
}

async fn next_frame(link: &mut Option<Link>) -> Option<Incoming> {
    match link {
        Some(link) => link.incoming.recv().await,
        None => std::future::pending().await,
    }
}

async fn until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

/// Where a reader that asked for the members of a workspace gets its answer.
type MembersReply = oneshot::Sender<Result<(), LoadError>>;

struct Client {
    shared: Arc<Shared>,
    host: Arc<dyn Host>,
    environment: Arc<Environment>,
    loaded: mpsc::UnboundedSender<Loaded>,
    target: Option<StreamTarget>,
    wanted: Level,
    model: Option<ServerModel>,
    /// The state must be replaced by a snapshot before an event is applied again.
    stale: bool,
    pending: Vec<Pending>,
    tickets: u64,
    /// The workspaces whose members a reader asked for.
    wanted_members: BTreeSet<WorkspaceId>,
    waiting_for_members: BTreeMap<WorkspaceId, Vec<MembersReply>>,
    /// The answers to readers that asked for members, until the view that holds the result
    /// is given to the readers: a reader that is answered finds the members in the view.
    answers: Vec<(MembersReply, Result<(), LoadError>)>,
    link: Option<Link>,
    opening: Option<Opening>,
    /// The earliest time of the next connection attempt.
    retry_at: Option<Instant>,
    backoff: Backoff,
    /// When the connection attempts of the last minute started. It outlives the state,
    /// so that changes between `off` and a level stay below the rate of the server.
    opened_at: VecDeque<Instant>,
    /// Nothing is sent to the server before this time: the wait after an ended session.
    not_before: Option<Instant>,
    /// The parts whose read was on its way when the session changed. They are read again
    /// with the next session.
    resume: Vec<Part>,
    /// The level of the subscriptions that moved the cursor since the last snapshot. A
    /// state that the background level kept is not continued at the full level.
    covered: Level,
    snapshot_retry_at: Option<Instant>,
    snapshot_backoff: Backoff,
    /// The session ended. Nothing is tried until the client core gives another session.
    halted: bool,
    fault: Option<StreamFault>,
    shown: Option<(StreamStatus, Option<Level>)>,
    model_changed: bool,
    in_call: bool,
    /// The notices of the changes that the readers do not have yet. A reader gets a
    /// notice after the state that holds its change.
    untold: Vec<Notice>,
    /// The bytes of the messages whose events were applied since the last snapshot.
    applied_bytes: usize,
    /// What the client does for the messages (ADR 0721).
    messages: messages::Work,
    done: mpsc::UnboundedSender<messages::Done>,
    /// What the client does for the recordings that a reader has open.
    recordings: recordings::Work,
}

/// Runs the stream client of one server until `desired` is closed.
pub(crate) async fn run(
    shared: Arc<Shared>,
    host: Arc<dyn Host>,
    environment: Arc<Environment>,
    mut desired: watch::Receiver<Desired>,
    mut commands: mpsc::UnboundedReceiver<Command>,
) {
    let (loaded, mut loads) = mpsc::unbounded_channel();
    let (done, mut dones) = mpsc::unbounded_channel();
    let policy = environment.policy;
    let mut client = Client {
        shared,
        host,
        environment,
        loaded,
        target: None,
        wanted: Level::Background,
        model: None,
        stale: false,
        pending: Vec::new(),
        tickets: 0,
        wanted_members: BTreeSet::new(),
        waiting_for_members: BTreeMap::new(),
        answers: Vec::new(),
        link: None,
        opening: None,
        retry_at: None,
        backoff: Backoff::new(policy.first_delay, policy.longest_delay),
        opened_at: VecDeque::new(),
        not_before: None,
        resume: Vec::new(),
        covered: Level::Full,
        snapshot_retry_at: None,
        snapshot_backoff: Backoff::new(policy.first_delay, policy.longest_delay),
        halted: false,
        fault: None,
        shown: None,
        model_changed: false,
        in_call: false,
        untold: Vec::new(),
        applied_bytes: 0,
        messages: messages::Work::default(),
        done,
        recordings: recordings::Work::default(),
    };
    let first = desired.borrow_and_update().clone();
    client.retarget(first);
    loop {
        client.advance();
        client.publish();
        let deadline = client.deadline();
        tokio::select! {
            changed = desired.changed() => {
                if changed.is_err() {
                    break;
                }
                let now = desired.borrow_and_update().clone();
                client.retarget(now);
            }
            Some(loaded) = loads.recv() => client.loaded(loaded),
            Some(done) = dones.recv() => client.done(done),
            Some(command) = commands.recv() => client.command(command),
            result = opened(&mut client.opening) => {
                client.opening = None;
                client.opened(result);
            }
            frame = next_frame(&mut client.link) => {
                client.frame(frame);
                for _ in 1..FRAMES_PER_VIEW {
                    let Some(Ok(frame)) = client.link.as_mut().map(|link| link.incoming.try_recv())
                    else {
                        break;
                    };
                    client.frame(Some(frame));
                }
            }
            () = until(deadline) => client.timer(),
        }
    }
    client.forget();
    client.shared.detach();
}

impl Client {
    fn policy(&self) -> &StreamPolicy {
        &self.environment.policy
    }

    /// Takes what the client core wants now: no connection, another level, another
    /// session, or another account.
    fn retarget(&mut self, desired: Desired) {
        let (Some(target), Some(level)) = (desired.target, wire(desired.level)) else {
            // The level `off` is an act of the user or the end of the access: the next
            // level starts without the waits of before.
            self.forget();
            self.target = None;
            self.retry_at = None;
            self.not_before = None;
            self.backoff.reset();
            return;
        };
        let (same_account, same_session) = match &self.target {
            Some(held) => (
                held.account_id == target.account_id && held.address == target.address,
                held.token == target.token,
            ),
            None => (false, false),
        };
        if !same_account {
            self.forget();
        }
        self.wanted = level;
        self.target = Some(target);
        if same_account && !same_session {
            // The state stays: it belongs to the account. The reads of the session
            // before would fail, so they start again with this one.
            self.disconnect();
            self.park();
            self.park_messages();
        }
        if !same_session {
            // The wait that an ended session set stays: a server that ends every
            // session at once gets its logins at the waits of lost connections.
            self.halted = false;
            self.fault = None;
            self.snapshot_retry_at = None;
        }
    }

    /// Ends the reads that are on their way and keeps their parts for the next session.
    fn park(&mut self) {
        let parts: Vec<Part> = self
            .pending
            .drain(..)
            .filter_map(|pending| match pending.load {
                Load::Part(part) => Some(part),
                Load::Snapshot => None,
            })
            .collect();
        for part in parts {
            self.keep(part);
        }
    }

    fn keep(&mut self, part: Part) {
        if !self.resume.contains(&part) {
            self.resume.push(part);
        }
    }

    /// Reads again the parts that a change of the session interrupted, as far as the
    /// state still needs them.
    fn read_again(&mut self) {
        for part in std::mem::take(&mut self.resume) {
            let needed = self.model.as_ref().is_some_and(|model| match part {
                Part::Members(workspace_id) => {
                    self.wanted_members.contains(&workspace_id)
                        && model
                            .workspace(workspace_id)
                            .is_some_and(|workspace| workspace.members.loaded().is_none())
                }
                Part::OwnCall => model.own_call.is_some(),
                Part::ChannelCall(channel_id) => model.channel(channel_id).is_some(),
                part => self.message_read_needed(part, model),
            });
            if needed && part.is_read_again() {
                self.read(part);
            } else if needed {
                self.start(Load::Part(part));
            }
        }
    }

    /// Drops the connection, the state of the account and everything that waits for them.
    /// The waits stay: they belong to the server, and a server that gives another account
    /// at every login must not get its requests sooner for that.
    fn forget(&mut self) {
        self.disconnect();
        self.model = None;
        self.model_changed = true;
        self.stale = false;
        self.pending.clear();
        self.resume.clear();
        self.wanted_members.clear();
        self.waiting_for_members.clear();
        self.forget_messages();
        self.forget_recordings();
        self.halted = false;
        self.fault = None;
        self.snapshot_retry_at = None;
        self.covered = Level::Full;
        self.snapshot_backoff.reset();
    }

    fn disconnect(&mut self) {
        self.link = None;
        self.opening = None;
    }

    /// Starts what is due: a snapshot, a connection, a `subscribe`.
    fn advance(&mut self) {
        if self.target.is_none() || self.halted {
            return;
        }
        let now = Instant::now();
        let due = |at: Option<Instant>| at.is_none_or(|at| at <= now);
        if !due(self.not_before) {
            return;
        }
        self.not_before = None;
        self.read_again();

        if let Some(link) = &mut self.link
            && let Subscription::Live { level, .. } = link.subscription
            && level != self.wanted
        {
            link.subscription = Subscription::None;
        }
        if self.wanted == Level::Full && self.covered == Level::Background && !self.stale {
            // The background level did not deliver the changes of the structure
            // (ADR 0207), with or without a connection at this moment.
            self.reload();
        }

        let needs_snapshot = self.model.is_none() || self.stale;
        let loading = self
            .pending
            .iter()
            .any(|pending| pending.load == Load::Snapshot);
        if needs_snapshot && !loading && due(self.snapshot_retry_at) {
            self.snapshot_retry_at = None;
            self.start(Load::Snapshot);
        }

        if self.link.is_none() && self.opening.is_none() && due(self.retry_at) {
            self.retry_at = None;
            while self
                .opened_at
                .front()
                .is_some_and(|opened| now.duration_since(*opened) >= MINUTE)
            {
                self.opened_at.pop_front();
            }
            if self.opened_at.len() >= self.policy().connections_per_minute {
                self.retry_at = self.opened_at.front().map(|opened| *opened + MINUTE);
            } else {
                self.opened_at.push_back(now);
                self.open();
            }
        }

        let unsubscribed = self.link.as_ref().is_some_and(|link| {
            link.greeted
                && matches!(link.subscription, Subscription::None)
                && link.send_at.is_none_or(|at| at <= now)
        });
        if !needs_snapshot
            && unsubscribed
            && let Some(cursor) = self.model.as_ref().map(|model| model.cursor)
        {
            self.subscribe(cursor, now);
        }
        if !needs_snapshot {
            self.advance_messages();
        }
    }

    fn subscribe(&mut self, cursor: Cursor, now: Instant) {
        let level = self.wanted;
        let limit = self.policy().messages_per_minute;
        let deadline = now + self.policy().subscribe_timeout;
        let Some(link) = &mut self.link else {
            return;
        };
        while link
            .sent
            .front()
            .is_some_and(|sent| now.duration_since(*sent) >= MINUTE)
        {
            link.sent.pop_front();
        }
        if link.sent.len() >= limit {
            link.send_at = link.sent.front().map(|sent| *sent + MINUTE);
            return;
        }
        link.send_at = None;
        let message = ClientMessage::Subscribe {
            level,
            cursor: Some(cursor),
        };
        let sent = serde_json::to_string(&message)
            .ok()
            .is_some_and(|text| link.outgoing.try_send(text).is_ok());
        if !sent {
            self.lost(StreamFault::Unreachable(TransportErrorKind::Interrupted));
            return;
        }
        link.sent.push_back(now);
        link.subscription = Subscription::Asked {
            level,
            cursor,
            deadline,
        };
    }

    fn open(&mut self) {
        let Some(target) = &self.target else {
            return;
        };
        let path = format!("{}/events", target.api_version.prefix());
        let request = StreamRequest {
            url: target.address.endpoint(&path),
            bearer: target.token.clone(),
        };
        let opening = self.environment.sockets.open(request);
        let limit = self.policy().open_timeout;
        let (sender, result) = oneshot::channel();
        let attempt = tokio::spawn(async move {
            let result = match tokio::time::timeout(limit, opening).await {
                Ok(result) => result,
                Err(_) => Err(OpenError::Failed(TransportError::new(
                    TransportErrorKind::Timeout,
                    "the event stream did not open",
                ))),
            };
            let _ = sender.send(result);
        });
        self.opening = Some(Opening {
            result,
            attempt: attempt.abort_handle(),
        });
    }

    fn opened(&mut self, result: Result<StreamSocket, OpenError>) {
        let now = Instant::now();
        match result {
            Ok(socket) => {
                let silence = self.policy().hello_timeout;
                self.link = Some(Link {
                    incoming: socket.incoming,
                    outgoing: socket.outgoing,
                    greeted: false,
                    silence,
                    deadline: now + silence,
                    subscription: Subscription::None,
                    sent: VecDeque::new(),
                    send_at: None,
                });
            }
            Err(OpenError::Failed(error)) => self.lost(StreamFault::Unreachable(error.kind)),
            Err(OpenError::Refused(response)) => {
                let error = read_rejection(&response);
                if error.category() == Some(ErrorCategory::SessionEnded) {
                    self.ended(None);
                    return;
                }
                let code = error.code().unwrap_or(ErrorCode::Unknown);
                self.lost(StreamFault::Refused(code));
                // A limit on connections names the time after which the server accepts
                // one again (ADR 0116).
                if let Some(stated) = error.retry_after() {
                    let stated = now + stated.min(self.policy().longest_delay);
                    self.retry_at = self.retry_at.max(Some(stated));
                }
            }
        }
    }

    /// The connection is gone. The state stays, and a new attempt follows.
    fn lost(&mut self, fault: StreamFault) {
        let settled = self.link.as_ref().is_some_and(|link| {
            matches!(link.subscription, Subscription::Live { since, .. }
                if since.elapsed() >= self.environment.policy.settled_after)
        });
        if settled {
            self.backoff.reset();
        }
        self.disconnect();
        self.fault = Some(fault);
        self.retry_at = Some(Instant::now() + self.backoff.wait());
    }

    /// The server no longer accepts the session. The client core logs in again once
    /// (ADR 0712) and then gives another session, or none.
    fn ended(&mut self, reason: Option<EndReason>) {
        self.lost(StreamFault::SessionEnded(reason));
        // The login and everything after it wait as a new connection attempt does.
        self.not_before = self.retry_at.take();
        self.park();
        self.halted = true;
        if let Some(target) = &self.target {
            self.host.session_ended(&target.token);
        }
    }

    /// The state must be replaced: the client no longer knows which changes it missed, or
    /// it was told that what it may see has changed (ADR 0207, ADR 0208).
    fn reload(&mut self) {
        self.stale = true;
        if let Some(link) = &mut self.link {
            link.subscription = Subscription::None;
        }
    }

    fn frame(&mut self, frame: Option<Incoming>) {
        let Some(link) = &mut self.link else {
            return;
        };
        if link.greeted {
            // The time for the `hello` is one fixed moment: a ping does not move it.
            link.deadline = Instant::now() + link.silence;
        }
        let text = match frame {
            Some(Incoming::Text(text)) => text,
            Some(Incoming::Alive) => return,
            Some(Incoming::Closed { code }) => {
                match code.and_then(CloseCode::from_code) {
                    Some(CloseCode::Ended) => self.ended(None),
                    _ => self.lost(StreamFault::Closed(code)),
                }
                return;
            }
            Some(Incoming::Failed(detail)) => {
                tracing::debug!(%detail, "the event stream failed");
                self.lost(StreamFault::Unreachable(TransportErrorKind::Interrupted));
                return;
            }
            Some(Incoming::TooLarge) => {
                // As for a message that cannot be read: the cursor of a snapshot lies
                // after it.
                self.reload();
                self.lost(StreamFault::Unreadable);
                return;
            }
            None => {
                self.lost(StreamFault::Closed(None));
                return;
            }
        };
        match decode(&text) {
            Ok(Received::Message(message)) => self.message(message, text.len()),
            Ok(Received::UnknownEvent { id }) => self.move_cursor(id),
            Ok(Received::UnknownMessage) => {}
            Err(error) => {
                // A new connection with the same cursor would get the same frame again.
                // The cursor of a snapshot lies after it.
                tracing::debug!(%error, "a message of the event stream cannot be read");
                self.reload();
                self.lost(StreamFault::Unreadable);
            }
        }
    }

    fn message(&mut self, message: ServerMessage, bytes: usize) {
        let Some(link) = &mut self.link else {
            return;
        };
        if !link.greeted {
            let ServerMessage::Hello {
                epoch,
                heartbeat_seconds,
                ..
            } = message
            else {
                self.lost(StreamFault::Unreadable);
                return;
            };
            link.greeted = true;
            link.silence = Duration::from_secs(u64::from(heartbeat_seconds)).saturating_mul(2)
                + self.environment.policy.silence_grace;
            link.deadline = Instant::now() + link.silence;
            if self
                .model
                .as_ref()
                .is_some_and(|model| model.cursor.epoch != epoch)
            {
                self.reload();
            }
            return;
        }
        match message {
            ServerMessage::Hello { .. } => {}
            ServerMessage::Subscribed { level, cursor } => {
                let Subscription::Asked {
                    level: asked,
                    cursor: from,
                    ..
                } = link.subscription
                else {
                    return;
                };
                if level != asked {
                    return;
                }
                if cursor != from {
                    self.reload();
                    return;
                }
                link.subscription = Subscription::Live {
                    level,
                    since: Instant::now(),
                };
                self.fault = None;
                if level == Level::Background {
                    self.covered = Level::Background;
                }
            }
            ServerMessage::Event { id, at, event } => {
                if !matches!(link.subscription, Subscription::Live { .. }) {
                    return;
                }
                self.event(id, at, Arc::new(event), bytes);
            }
            ServerMessage::Checkpoint { cursor } => {
                if !matches!(link.subscription, Subscription::Live { .. }) {
                    return;
                }
                let epoch = self.model.as_ref().map(|model| model.cursor.epoch);
                if epoch == Some(cursor.epoch) {
                    self.move_cursor(cursor.position);
                } else {
                    self.reload();
                }
            }
            ServerMessage::Resync { reason } => {
                tracing::debug!(?reason, "the server does not continue from the cursor");
                self.reload();
            }
            ServerMessage::Ended { reason, recovery } => {
                self.untold.push(Notice::Ended { reason, recovery });
                self.ended(Some(reason));
            }
        }
    }

    fn live(&self) -> bool {
        !self.stale
            && self
                .link
                .as_ref()
                .is_some_and(|link| matches!(link.subscription, Subscription::Live { .. }))
    }

    fn move_cursor(&mut self, position: u64) {
        if !self.live() {
            return;
        }
        if let Some(model) = &mut self.model
            && position > model.cursor.position
        {
            model.cursor.position = position;
            self.model_changed = true;
        }
    }

    fn event(&mut self, id: u64, at: time::OffsetDateTime, event: Arc<Event>, bytes: usize) {
        if !self.live() {
            return;
        }
        let Some(model) = &mut self.model else {
            return;
        };
        if id <= model.cursor.position {
            // A repeat: deliveries are at least once (ADR 0011).
            return;
        }
        let follow = model.apply(&event);
        model.cursor.position = id;
        self.model_changed = true;
        self.messages.overtake_marks(&event, model);
        let mut again = Vec::new();
        for pending in &mut self.pending {
            if let Load::Part(part) = pending.load {
                // A read whose content no event carries keeps only the events that start
                // it again.
                if part.is_read_again() && !part.concerns(&event, model) {
                    continue;
                }
                pending.events.push(event.clone());
                pending.bytes += bytes;
                if pending.events.len() > MOST_KEPT_EVENTS || pending.bytes > MOST_KEPT_BYTES {
                    again.push(part);
                }
            }
        }
        self.untold.push(Notice::Event { id, at, event });
        self.follow(follow);
        for part in again {
            self.start(Load::Part(part));
        }
        self.applied_bytes += bytes;
        if self.applied_bytes > MOST_APPLIED_BYTES {
            self.reload();
        }
    }

    fn follow(&mut self, follow: Follow) {
        if follow.reload {
            self.reload();
        }
        for part in follow.load {
            if part.is_read_again() {
                self.read(part);
            } else {
                self.start(Load::Part(part));
            }
        }
    }

    /// Sends a read to the server. A read of the same thing that is on its way is dropped.
    fn start(&mut self, load: Load) {
        if self.target.is_none() {
            return;
        }
        self.pending.retain(|pending| pending.load != load);
        if let (Load::Part(Part::Members(workspace_id)), Some(model)) = (load, &mut self.model) {
            model.set_members(workspace_id, Loadable::Loading);
            self.model_changed = true;
        }
        let page = match load {
            Load::Part(Part::Messages(place, reach)) => {
                let Some(ask) = self.page_ask(place, reach) else {
                    return;
                };
                self.mark_reading(place, reach, true);
                Some(ask)
            }
            _ => None,
        };
        let continuation = match load {
            Load::Part(Part::MoreThreads) => {
                let Some(token) = self
                    .model
                    .as_ref()
                    .and_then(|model| model.threads_continuation())
                    .map(str::to_owned)
                else {
                    return;
                };
                self.messages.continued_threads = Some(token.clone());
                Some(token)
            }
            _ => None,
        };
        if let (Load::Part(Part::Pins(channel_id)), Some(model)) = (load, &mut self.model) {
            model.set_pins_reading(channel_id, None);
            self.model_changed = true;
        }
        if let (Load::Part(Part::Recording(recording_id)), Some(model)) = (load, &mut self.model) {
            model.set_recording_reading(recording_id);
            self.model_changed = true;
        }
        let lines = match load {
            Load::Part(Part::TranscriptLines(recording_id)) => {
                let Some(ask) = self.lines_ask(recording_id) else {
                    return;
                };
                Some(ask)
            }
            _ => None,
        };
        let Some(target) = &self.target else {
            return;
        };
        if self.halted || self.not_before.is_some() {
            // Nothing is sent with a session that ended. The part is read with the next.
            if let Load::Part(part) = load {
                self.keep(part);
            }
            return;
        }
        self.tickets += 1;
        let ticket = self.tickets;
        let api = ApiClient::new(
            self.environment.transport.clone(),
            target.address.clone(),
            target.server_id,
            target.api_version,
            self.environment.retry,
        )
        .with_token(target.token.clone());
        let loaded = self.loaded.clone();
        let request = tokio::spawn(async move {
            let result = match load {
                Load::Snapshot => api
                    .snapshot()
                    .await
                    .map(|snapshot| Payload::Snapshot(Box::new(snapshot))),
                Load::Part(Part::Members(workspace_id)) => api
                    .workspace_members(workspace_id)
                    .await
                    .map(Payload::Members),
                Load::Part(Part::OwnCall) => api.own_call().await.map(Payload::OwnCall),
                Load::Part(Part::ChannelCall(channel_id)) => {
                    api.channel_call(channel_id).await.map(Payload::ChannelCall)
                }
                Load::Part(Part::MessageState(place)) => {
                    api.message_state(place).await.map(Payload::MessageState)
                }
                Load::Part(Part::Message(_, message_id)) => api
                    .message(message_id)
                    .await
                    .map(|message| Payload::Message(Box::new(message))),
                Load::Part(Part::Messages(place, _)) => match page {
                    Some(ask) => messages::read_pages(&api, place, ask)
                        .await
                        .map(Payload::Pages),
                    None => Err(ApiError::Stopped),
                },
                Load::Part(Part::AttachmentLimits) => {
                    api.attachment_limits().await.map(Payload::Limits)
                }
                Load::Part(Part::UnreadThreads) => api
                    .unread_threads(None, Some(messages::LIST_PAGE))
                    .await
                    .map(Payload::Threads),
                Load::Part(Part::MoreThreads) => api
                    .unread_threads(continuation.as_deref(), Some(messages::LIST_PAGE))
                    .await
                    .map(Payload::Threads),
                Load::Part(Part::ThreadHead(place)) => match place.thread_root() {
                    Some(root_id) => {
                        let one = PageQuery {
                            limit: Some(1),
                            ..PageQuery::default()
                        };
                        api.thread(root_id, &one)
                            .await
                            .map(|page| Payload::ThreadHead(Box::new(page)))
                    }
                    None => Err(ApiError::Stopped),
                },
                Load::Part(Part::Pins(channel_id)) => api.pins(channel_id).await.map(Payload::Pins),
                Load::Part(Part::Notices) => api
                    .notifications(None, Some(messages::NOTICES_PAGE))
                    .await
                    .map(Payload::Notices),
                Load::Part(Part::MoreNotices(before)) => api
                    .notifications(Some(before), Some(messages::NOTICES_PAGE))
                    .await
                    .map(Payload::Notices),
                Load::Part(Part::Named(message_id)) => api
                    .message(message_id)
                    .await
                    .map(|message| Payload::Message(Box::new(message))),
                Load::Part(Part::Recording(recording_id)) => {
                    recordings::read_recording(&api, recording_id)
                        .await
                        .map(|read| Payload::Recording(Box::new(read)))
                }
                Load::Part(Part::TranscriptLines(recording_id)) => match lines {
                    Some(ask) => recordings::read_lines(&api, recording_id, ask)
                        .await
                        .map(Payload::Lines),
                    None => Err(ApiError::Stopped),
                },
            };
            let _ = loaded.send(Loaded { ticket, result });
        });
        self.pending.push(Pending {
            ticket,
            load,
            events: Vec::new(),
            bytes: 0,
            request: request.abort_handle(),
        });
    }

    fn loaded(&mut self, loaded: Loaded) {
        let Some(position) = self
            .pending
            .iter()
            .position(|pending| pending.ticket == loaded.ticket)
        else {
            return;
        };
        let mut pending = self.pending.swap_remove(position);
        let kept = std::mem::take(&mut pending.events);
        let ended = matches!(&loaded.result, Err(error)
            if error.category() == Some(ErrorCategory::SessionEnded));
        if ended && let Load::Part(part) = pending.load {
            // The read is sent again with the next session.
            self.keep(part);
        }
        if let Load::Part(part) = pending.load
            && part.is_read_again()
        {
            if ended {
                self.ended(None);
            } else {
                self.read_answered(part, loaded.result, &kept);
            }
            self.pump_message_reads();
            return;
        }
        match (pending.load, loaded.result) {
            (Load::Snapshot, Ok(Payload::Snapshot(snapshot))) => self.install(*snapshot),
            (Load::Snapshot, Err(error)) if !ended => {
                tracing::debug!(%error, "the snapshot was not loaded");
                self.fault = Some(fault_of(&error));
                self.snapshot_retry_at = Some(Instant::now() + self.snapshot_backoff.wait());
            }
            (Load::Part(part), Ok(payload)) => {
                let Some(model) = &mut self.model else {
                    return;
                };
                match (part, payload) {
                    (Part::Members(workspace_id), Payload::Members(members)) => {
                        model.set_members(workspace_id, Loadable::Loaded(members));
                    }
                    (Part::OwnCall, Payload::OwnCall(view)) => model.set_own_call(view.call),
                    (Part::ChannelCall(_), Payload::ChannelCall(shown)) => {
                        model.set_channel_call(shown);
                    }
                    _ => return,
                }
                model.replay(part, kept.iter().map(Arc::as_ref));
                self.model_changed = true;
                if let Part::Members(workspace_id) = part {
                    self.answer_members(workspace_id, || Ok(()));
                }
            }
            (Load::Part(part), Err(error)) if !ended => {
                tracing::debug!(%error, ?part, "a part of the state was not loaded");
                if let Part::Members(workspace_id) = part {
                    if let Some(model) = &mut self.model {
                        model.set_members(workspace_id, Loadable::NotLoaded);
                        self.model_changed = true;
                    }
                    let error = Arc::new(error);
                    self.answer_members(workspace_id, || Err(LoadError::Request(error.clone())));
                }
            }
            _ => {}
        }
        if ended {
            self.ended(None);
        }
    }

    fn answer_members(
        &mut self,
        workspace_id: WorkspaceId,
        answer: impl Fn() -> Result<(), LoadError>,
    ) {
        for waiting in self
            .waiting_for_members
            .remove(&workspace_id)
            .unwrap_or_default()
        {
            self.answers.push((waiting, answer()));
        }
    }

    /// Replaces the state by a snapshot. What the snapshot does not carry is discarded
    /// (ADR 0207); the parts that a reader asked for and the call of the account are read
    /// again.
    fn install(&mut self, snapshot: Snapshot) {
        let model = ServerModel::from_snapshot(snapshot);
        let cursor = model.cursor;
        let in_call = model.own_call.is_some();
        let before = self.model.take();
        let unseen: Vec<WorkspaceId> = self
            .wanted_members
            .iter()
            .copied()
            .filter(|workspace_id| model.workspace(*workspace_id).is_none())
            .collect();
        self.model = Some(model);
        self.model_changed = true;
        self.stale = false;
        self.applied_bytes = 0;
        self.covered = Level::Full;
        self.resume.clear();
        self.snapshot_backoff.reset();
        self.pending.clear();
        if let Some(link) = &mut self.link {
            link.subscription = Subscription::None;
        }
        for workspace_id in unseen {
            self.wanted_members.remove(&workspace_id);
            self.answer_members(workspace_id, || Err(LoadError::NotInState));
        }
        for workspace_id in self.wanted_members.clone() {
            self.start(Load::Part(Part::Members(workspace_id)));
        }
        if in_call {
            self.start(Load::Part(Part::OwnCall));
        }
        self.carry_messages(before.as_ref());
        self.carry_recordings(before.as_ref());
        self.untold.push(Notice::Reloaded { cursor });
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::LoadMembers {
                workspace_id,
                reply,
            } => {
                if self.target.is_none() {
                    let _ = reply.send(Err(LoadError::Off));
                    return;
                }
                let held = match &self.model {
                    Some(model) if !self.stale => match model.workspace(workspace_id) {
                        Some(workspace) => Some(workspace.members.clone()),
                        None => {
                            let _ = reply.send(Err(LoadError::NotInState));
                            return;
                        }
                    },
                    // The members are read when the state has arrived.
                    _ => None,
                };
                self.wanted_members.insert(workspace_id);
                match held {
                    Some(Loadable::Loaded(_)) => self.answers.push((reply, Ok(()))),
                    Some(Loadable::NotLoaded) => {
                        self.waiting_for_members
                            .entry(workspace_id)
                            .or_default()
                            .push(reply);
                        self.start(Load::Part(Part::Members(workspace_id)));
                    }
                    Some(Loadable::Loading) | None => self
                        .waiting_for_members
                        .entry(workspace_id)
                        .or_default()
                        .push(reply),
                }
            }
            command @ (Command::OpenRecording { .. }
            | Command::ReleaseRecording(_)
            | Command::MoreTranscriptLines(_)) => self.recording_command(command),
            Command::ReleaseMembers(workspace_id) => {
                self.wanted_members.remove(&workspace_id);
                self.waiting_for_members.remove(&workspace_id);
                self.pending
                    .retain(|pending| pending.load != Load::Part(Part::Members(workspace_id)));
                if let Some(model) = &mut self.model {
                    model.set_members(workspace_id, Loadable::NotLoaded);
                    self.model_changed = true;
                }
            }
            command => self.message_command(command),
        }
    }

    fn timer(&mut self) {
        let now = Instant::now();
        let silent = self.link.as_ref().is_some_and(|link| {
            link.deadline <= now
                || matches!(link.subscription, Subscription::Asked { deadline, .. }
                    if deadline <= now)
        });
        if silent {
            self.lost(StreamFault::Silent);
        }
        // A time that has passed is no time to wait for. The next `subscribe` counts the
        // messages of the last minute again.
        if let Some(link) = &mut self.link
            && link.send_at.is_some_and(|at| at <= now)
        {
            link.send_at = None;
        }
    }

    /// The next moment at which something is due without a message from outside.
    fn deadline(&self) -> Option<Instant> {
        if self.target.is_none() || self.halted {
            return None;
        }
        if self.not_before.is_some() {
            return self.not_before;
        }
        let mut times = Vec::new();
        match &self.link {
            Some(link) => {
                times.push(link.deadline);
                times.extend(link.send_at);
                if let Subscription::Asked { deadline, .. } = link.subscription {
                    times.push(deadline);
                }
            }
            None if self.opening.is_none() => times.extend(self.retry_at),
            None => {}
        }
        if self.model.is_none() || self.stale {
            times.extend(self.snapshot_retry_at);
        } else {
            times.extend(self.messages.next_due());
        }
        times.into_iter().min()
    }

    /// Gives the readers the status and the state when one of them changed, and tells
    /// the client core whether this device is in a call.
    fn publish(&mut self) {
        let (status, level) = if self.target.is_none() {
            (StreamStatus::Off, None)
        } else if let Some(fault) = self.fault {
            (StreamStatus::Interrupted(fault), None)
        } else {
            match self.link.as_ref().map(|link| &link.subscription) {
                Some(Subscription::Live { level, .. }) if !self.stale => {
                    (StreamStatus::Live, Some(*level))
                }
                _ => (StreamStatus::Connecting, None),
            }
        };
        if self.model_changed || self.shown != Some((status, level)) {
            let model = match (&self.model, self.model_changed) {
                (Some(model), true) => Some(Arc::new(model.clone())),
                (Some(_), false) => self.shared.model(),
                (None, _) => None,
            };
            self.shared.show(LiveView {
                status,
                level,
                model,
            });
            self.shown = Some((status, level));
            self.model_changed = false;
        }
        for (waiting, answer) in self.answers.drain(..) {
            let _ = waiting.send(answer);
        }
        for notice in self.untold.drain(..) {
            self.shared.tell(notice);
        }
        let in_call = match (&self.model, &self.target) {
            (Some(model), Some(target)) => model.in_call_with(target.device_id),
            _ => false,
        };
        if in_call != self.in_call {
            self.in_call = in_call;
            self.host.in_call(in_call);
        }
    }
}
