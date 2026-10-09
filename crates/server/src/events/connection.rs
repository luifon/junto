//! One WebSocket connection of the event stream (ADR 0116, ADR 0207).

use std::collections::VecDeque;
use std::sync::Arc;
use std::task::{Poll, ready};

use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use junto_protocol::API_VERSION;
use junto_protocol::events::{
    ClientMessage, CloseCode, Cursor, Level, ResyncReason, ServerMessage,
};
use junto_protocol::ids::StreamEpoch;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::oneshot;
use tokio::time::Instant;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::{self, Message, Utf8Bytes};

use crate::error::ApiResult;
use crate::events::delivery::{self, Batch, Ended, Opened, Start, Subscription};
use crate::events::hub::{Lane, Link, Registration};
use crate::events::transport::{Guarded, Inbound, Violation};
use crate::state::AppState;

/// Why a connection ends.
enum End {
    /// The socket failed.
    Gone,
    /// The client sent a close frame.
    ClosedByClient,
    Close(CloseCode),
    Ended(Ended),
}

/// How far a message got.
enum Put {
    Written,
    /// The message is in the buffer of the socket and the connection was interrupted before
    /// the buffer was written. A later write sends it.
    Buffered,
    /// The connection was interrupted before the socket took the message.
    Interrupted,
}

/// The result of a read of the database for the connection.
enum Read<T> {
    Done(T),
    /// The database did not answer in time or with an error. The connection tries again.
    Failed,
    Interrupted,
}

/// A message that the client must receive, and the subscription that starts when the socket
/// has taken it.
struct Control {
    message: ServerMessage,
    starts: Option<Subscription>,
}

/// How a connection reacts to reads that lead to no delivery (ADR 0011): it asks for
/// smaller batches when the decisions of a batch expire before an event is sent, and it
/// stalls after a number of attempts in a row without progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Pace {
    largest: u32,
    size: u32,
    failures: u32,
    most_failures: u32,
}

impl Pace {
    fn new(largest: u32, most_failures: u32) -> Self {
        Self {
            largest: largest.max(1),
            size: largest.max(1),
            failures: 0,
            most_failures: most_failures.max(1),
        }
    }

    fn progressed(&mut self) {
        self.failures = 0;
        self.size = self.size.saturating_mul(2).min(self.largest);
    }

    /// Returns true when the connection has stalled.
    fn failed(&mut self) -> bool {
        self.failures += 1;
        self.failures >= self.most_failures
    }

    /// Returns true when the connection has stalled.
    fn expired(&mut self) -> bool {
        self.size = (self.size / 2).max(1);
        self.failed()
    }
}

enum Step {
    /// More work waits; the connection goes on without waiting for a signal.
    Again,
    Idle,
    /// The session must be checked before anything else is sent.
    Interrupted,
    Failed,
}

struct Connection<S> {
    app: AppState,
    who: Opened,
    link: Arc<Link>,
    inbound: Arc<Inbound>,
    sink: SplitSink<WebSocketStream<Guarded<S>>, Message>,
    /// The control messages that the socket has not taken yet, oldest first.
    control: VecDeque<Control>,
    /// The newest `subscribe` of the client that is not answered yet.
    asked: Option<(Level, Option<Cursor>)>,
    subscription: Option<Subscription>,
    /// The highest position that the client has been told: by an event or by a cursor.
    told: i64,
    /// A heartbeat has passed since the last checkpoint could be sent.
    checkpoint_due: bool,
    /// Present while the socket holds a message that is not written yet: the time by which
    /// the client must have taken it. It is fixed when the socket takes the message and is
    /// not renewed by an interruption (ADR 0116).
    write_deadline: Option<Instant>,
    pace: Pace,
    failed_session_checks: u32,
    /// The log can hold events that the connection has not read.
    pending: bool,
    unsubscribed_since: Instant,
}

/// Serves one connection until it ends. The place of the connection in the registry is held
/// until the closing handshake is over.
pub async fn run<S>(
    app: AppState,
    socket: WebSocketStream<Guarded<S>>,
    who: Opened,
    epoch: StreamEpoch,
    registration: Registration,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let inbound = socket.get_ref().inbound();
    let (sink, mut source) = socket.split();
    let limits = &app.config.limits.stream;
    let seconds = |duration: time::Duration| {
        u32::try_from(duration.whole_seconds().max(1)).unwrap_or(u32::MAX)
    };
    let hello = ServerMessage::Hello {
        server_version: env!("CARGO_PKG_VERSION").to_owned(),
        api_versions: vec![API_VERSION],
        epoch,
        heartbeat_seconds: seconds(limits.heartbeat),
        idle_seconds: seconds(limits.idle_timeout),
    };
    let mut connection = Connection {
        who,
        link: registration.link().clone(),
        inbound,
        sink,
        control: VecDeque::from([Control {
            message: hello,
            starts: None,
        }]),
        asked: None,
        subscription: None,
        told: 0,
        checkpoint_due: false,
        write_deadline: None,
        pace: Pace::new(limits.batch, limits.most_failed_attempts),
        failed_session_checks: 0,
        pending: false,
        unsubscribed_since: Instant::now(),
        app,
    };
    let end = connection.serve(&mut source).await;
    connection.finish(end, &mut source).await;
    drop(registration);
}

fn text(message: &ServerMessage) -> Result<Message, End> {
    match serde_json::to_string(message) {
        Ok(json) => Ok(Message::Text(Utf8Bytes::from(json))),
        Err(error) => {
            tracing::error!(%error, "a message of the event stream could not be written");
            Err(End::Close(CloseCode::Failed))
        }
    }
}

impl<S: AsyncRead + AsyncWrite + Unpin> Connection<S> {
    async fn serve(&mut self, source: &mut SplitStream<WebSocketStream<Guarded<S>>>) -> End {
        let app = self.app.clone();
        let limits = &app.config.limits.stream;
        let link = self.link.clone();
        let heartbeat = limits.heartbeat.unsigned_abs();
        let idle_timeout = limits.idle_timeout.unsigned_abs();
        let subscribe_timeout = limits.subscribe_timeout.unsigned_abs();

        let mut beats = tokio::time::interval_at(Instant::now() + heartbeat, heartbeat);
        beats.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut held = app.streams.held();
        let mut last_heard = Instant::now();
        let mut check_session = true;
        let mut not_before = Instant::now();

        loop {
            if link.is_stopping() {
                return End::Close(CloseCode::Stopping);
            }
            if link.is_replaced() {
                return End::Close(CloseCode::Replaced);
            }
            let mut step = Step::Idle;
            if Instant::now() >= not_before {
                let deliveries_held = *held.borrow();
                let worked = if check_session {
                    self.check_session().await
                } else {
                    self.work(deliveries_held).await
                };
                step = match worked {
                    Ok(step) => step,
                    Err(end) => return end,
                };
                match step {
                    Step::Interrupted => check_session = true,
                    Step::Failed => not_before = Instant::now() + limits.retry_delay.unsigned_abs(),
                    Step::Again | Step::Idle => check_session = false,
                }
            }
            let waits_for_retry = Instant::now() < not_before;
            let continues = !waits_for_retry && matches!(step, Step::Again | Step::Interrupted);

            tokio::select! {
                biased;
                () = link.attention() => check_session = true,
                frame = source.next() => {
                    last_heard = Instant::now();
                    if let Err(end) = self.receive(frame) {
                        return end;
                    }
                }
                _ = beats.tick() => {
                    if last_heard.elapsed() > idle_timeout {
                        return End::Close(CloseCode::Idle);
                    }
                    match self.put(Message::Ping(Vec::new().into())).await {
                        Ok(Put::Written) => {}
                        Ok(Put::Buffered | Put::Interrupted) => check_session = true,
                        Err(end) => return end,
                    }
                    if self.subscription.is_some() {
                        self.pending = true;
                        self.checkpoint_due = true;
                    } else {
                        check_session = true;
                    }
                }
                () = tokio::time::sleep_until(self.unsubscribed_since + subscribe_timeout),
                    if self.subscription.is_none() && self.asked.is_none() =>
                {
                    return End::Close(CloseCode::Idle);
                }
                () = std::future::ready(()), if continues => {}
                () = tokio::time::sleep_until(not_before), if waits_for_retry => {}
                () = link.news() => self.pending = true,
                changed = held.changed() => {
                    if changed.is_err() {
                        return End::Close(CloseCode::Stopping);
                    }
                    self.pending = true;
                }
            }
        }
    }

    /// Does the next thing that waits, in this order: a control message, the answer to a
    /// `subscribe`, the rest of an interrupted write, a batch of events.
    async fn work(&mut self, held: bool) -> Result<Step, End> {
        if let Some(control) = self.control.front() {
            let frame = text(&control.message)?;
            return Ok(match self.put(frame).await? {
                Put::Interrupted => Step::Interrupted,
                put => {
                    self.sent_control();
                    match put {
                        Put::Written => Step::Again,
                        _ => Step::Interrupted,
                    }
                }
            });
        }
        if let Some((level, cursor)) = self.asked {
            return self.answer(level, cursor).await;
        }
        if self.write_deadline.is_some() {
            return Ok(match self.flush().await? {
                true => Step::Again,
                false => Step::Interrupted,
            });
        }
        if self.pending && self.subscription.is_some() && !held {
            return self.deliver().await;
        }
        Ok(Step::Idle)
    }

    /// A control message has entered the socket: the transition that it announces takes
    /// place now.
    fn sent_control(&mut self) {
        let Some(control) = self.control.pop_front() else {
            return;
        };
        if let Some(subscription) = control.starts {
            self.subscription = Some(subscription);
            self.told = subscription.position;
            self.link.set_level(Some(subscription.level));
            self.pending = true;
        }
    }

    /// Waits for a read of the database, within the deadline of a read; a signal to the
    /// connection ends the wait. The read itself runs apart from the connection: it takes a
    /// place of its lane and keeps it until its work in the database has ended, also when
    /// the connection no longer waits. A read that has no place yet when the connection
    /// gives up does not start.
    async fn read<T, F>(&self, lane: Lane, work: F) -> Read<T>
    where
        T: Send + 'static,
        F: Future<Output = ApiResult<T>> + Send + 'static,
    {
        let deadline = self.app.config.limits.stream.read_deadline.unsigned_abs();
        let places = self.app.streams.places(lane);
        let (waiting, mut given_up) = oneshot::channel::<()>();
        let mut reading = tokio::spawn(async move {
            let _place = tokio::select! {
                biased;
                place = places.acquire_owned() => place.ok()?,
                _ = &mut given_up => return None,
            };
            Some(work.await)
        });
        let outcome = tokio::select! {
            biased;
            () = self.link.attention() => Read::Interrupted,
            done = tokio::time::timeout(deadline, &mut reading) => match done {
                Ok(Ok(Some(Ok(value)))) => Read::Done(value),
                Ok(Ok(Some(Err(error)))) => {
                    tracing::warn!(code = ?error.code(), "a read of the event stream failed");
                    Read::Failed
                }
                Ok(Ok(None) | Err(_)) => Read::Failed,
                Err(_) => {
                    tracing::warn!("a read of the event stream took too long");
                    Read::Failed
                }
            },
        };
        drop(waiting);
        outcome
    }

    /// A connection whose session cannot be checked for several attempts in a row is closed;
    /// the client connects again with its cursor.
    async fn check_session(&mut self) -> Result<Step, End> {
        let (app, who) = (self.app.clone(), self.who);
        let checked = self
            .read(Lane::Session, async move {
                delivery::validate(&app, &who).await
            })
            .await;
        match checked {
            Read::Done(None) => {
                self.failed_session_checks = 0;
                Ok(Step::Again)
            }
            Read::Done(Some(ended)) => Err(End::Ended(ended)),
            Read::Failed => {
                self.failed_session_checks += 1;
                let most = self.app.config.limits.stream.most_failed_attempts.max(1);
                if self.failed_session_checks >= most {
                    return Err(End::Close(CloseCode::Failed));
                }
                Ok(Step::Failed)
            }
            Read::Interrupted => Ok(Step::Interrupted),
        }
    }

    /// Answers the `subscribe` of the client. The answer is a control message; the
    /// subscription starts when the socket has taken it.
    async fn answer(&mut self, level: Level, cursor: Option<Cursor>) -> Result<Step, End> {
        let (app, who) = (self.app.clone(), self.who);
        let start = self
            .read(Lane::Events, async move {
                delivery::subscribe(&app, &who, level, cursor).await
            })
            .await;
        let message = match start {
            Read::Interrupted => return Ok(Step::Interrupted),
            Read::Failed => return Err(End::Close(CloseCode::Failed)),
            Read::Done(Start::Ended(ended)) => return Err(End::Ended(ended)),
            Read::Done(Start::Resync(reason)) => Control {
                message: ServerMessage::Resync { reason },
                starts: None,
            },
            Read::Done(Start::Subscribed(cursor)) => Control {
                message: ServerMessage::Subscribed { level, cursor },
                starts: Some(Subscription {
                    level,
                    position: i64::try_from(cursor.position).unwrap_or(i64::MAX),
                    epoch: cursor.epoch,
                }),
            },
        };
        self.asked = None;
        self.unsubscribed_since = Instant::now();
        self.control.push_back(message);
        Ok(Step::Again)
    }

    /// Reads one batch of the log and writes what the account may receive. A batch whose read
    /// started longer ago than an authorization is valid is not written; the next read is
    /// smaller and decides its events again. A connection that reads several times in a row
    /// without a delivery is closed, and the client connects again with its cursor
    /// (ADR 0011).
    async fn deliver(&mut self) -> Result<Step, End> {
        let Some(subscription) = self.subscription else {
            return Ok(Step::Idle);
        };
        let validity = self
            .app
            .config
            .limits
            .stream
            .authorization_validity
            .unsigned_abs();
        let (app, who, most) = (self.app.clone(), self.who, self.pace.size);
        let batch = self
            .read(Lane::Events, async move {
                delivery::next_batch(&app, &who, subscription, most).await
            })
            .await;
        let stalled = || Err(End::Close(CloseCode::Failed));
        let (deliveries, position, more, cursor, read_at, skipped) = match batch {
            Read::Interrupted => return Ok(Step::Interrupted),
            Read::Failed => {
                return if self.pace.failed() {
                    stalled()
                } else {
                    Ok(Step::Failed)
                };
            }
            Read::Done(Batch::Ended(ended)) => return Err(End::Ended(ended)),
            Read::Done(Batch::Resync(reason)) => {
                self.pace.progressed();
                self.resync(reason);
                return Ok(Step::Again);
            }
            Read::Done(Batch::Events {
                deliveries,
                position,
                more,
                cursor,
                read_at,
                skipped,
            }) => (deliveries, position, more, cursor, read_at, skipped),
        };
        let mut written = false;
        for delivery in deliveries {
            if read_at.elapsed() > validity {
                return match written {
                    true => Ok(Step::Again),
                    false if self.pace.expired() => stalled(),
                    false => Ok(Step::Failed),
                };
            }
            written = true;
            let put = self.put(text(&delivery.message)?).await?;
            if matches!(put, Put::Interrupted) {
                return Ok(Step::Interrupted);
            }
            self.served(subscription, delivery.position);
            self.told = delivery.position;
            if matches!(put, Put::Buffered) {
                return Ok(Step::Interrupted);
            }
        }
        self.served(subscription, position);
        self.pace.progressed();
        if skipped {
            self.resync(ResyncReason::SkippedEvent);
            return Ok(Step::Again);
        }
        if more {
            return Ok(Step::Again);
        }
        self.pending = false;
        if self.checkpoint_due {
            self.checkpoint_due = false;
            if position > self.told {
                self.told = position;
                self.control.push_back(Control {
                    message: ServerMessage::Checkpoint { cursor },
                    starts: None,
                });
                return Ok(Step::Again);
            }
        }
        Ok(Step::Idle)
    }

    fn served(&mut self, subscription: Subscription, position: i64) {
        self.subscription = Some(Subscription {
            position,
            ..subscription
        });
    }

    fn unsubscribe(&mut self) {
        if self.subscription.take().is_some() {
            self.unsubscribed_since = Instant::now();
        }
        self.link.set_level(None);
    }

    /// Ends the subscription and tells the client to load a snapshot.
    fn resync(&mut self, reason: ResyncReason) {
        self.unsubscribe();
        self.control.push_back(Control {
            message: ServerMessage::Resync { reason },
            starts: None,
        });
    }

    /// Hands one message to the socket and writes it. The client must take a message within
    /// the send time limit, counted from the moment the socket holds it; otherwise the
    /// client is disconnected, keeps its cursor and connects again (ADR 0116). A signal to
    /// the connection ends the wait without a change of that time, and the result states
    /// whether the socket has the message.
    async fn put(&mut self, message: Message) -> Result<Put, End> {
        let link = self.link.clone();
        let limit = self.app.config.limits.stream.send_timeout.unsigned_abs();
        let taken_by = self
            .write_deadline
            .unwrap_or_else(|| Instant::now() + limit);

        let mut waiting = Some(message);
        let taken = std::future::poll_fn(|context| {
            ready!(self.sink.poll_ready_unpin(context))?;
            match waiting.take() {
                Some(message) => Poll::Ready(self.sink.start_send_unpin(message)),
                None => Poll::Ready(Ok(())),
            }
        });
        tokio::select! {
            biased;
            () = link.attention() => return Ok(Put::Interrupted),
            taken = taken => {
                if taken.is_err() {
                    return Err(End::Gone);
                }
            }
            () = tokio::time::sleep_until(taken_by) => return Err(End::Close(CloseCode::Slow)),
        }
        if self.write_deadline.is_none() {
            self.write_deadline = Some(Instant::now() + limit);
        }
        Ok(match self.flush().await? {
            true => Put::Written,
            false => Put::Buffered,
        })
    }

    /// Writes what the socket holds. Returns false when a signal to the connection ended
    /// the wait.
    async fn flush(&mut self) -> Result<bool, End> {
        let Some(written_by) = self.write_deadline else {
            return Ok(true);
        };
        let link = self.link.clone();
        tokio::select! {
            biased;
            () = link.attention() => Ok(false),
            written = self.sink.flush() => match written {
                Ok(()) => {
                    self.write_deadline = None;
                    Ok(true)
                }
                Err(_) => Err(End::Gone),
            },
            () = tokio::time::sleep_until(written_by) => Err(End::Close(CloseCode::Slow)),
        }
    }

    /// Handles one frame of the client. A `subscribe` ends the subscription at once and is
    /// answered by [`Connection::answer`].
    fn receive(&mut self, frame: Option<Result<Message, tungstenite::Error>>) -> Result<(), End> {
        let message = match frame {
            None => return Err(End::Gone),
            Some(Ok(Message::Close(_))) => return Err(End::ClosedByClient),
            Some(Err(error)) => {
                return Err(match (self.inbound.violation(), error) {
                    (Some(Violation::Fragmented), _) => End::Close(CloseCode::InvalidMessage),
                    (Some(Violation::TooManyFrames), _) => End::Close(CloseCode::TooManyMessages),
                    (None, tungstenite::Error::Capacity(_)) => End::Close(CloseCode::MessageTooBig),
                    (None, _) => End::Gone,
                });
            }
            Some(Ok(Message::Ping(_) | Message::Pong(_))) => return Ok(()),
            Some(Ok(Message::Binary(_) | Message::Frame(_))) => {
                return Err(End::Close(CloseCode::InvalidMessage));
            }
            Some(Ok(Message::Text(text))) => text,
        };
        let Ok(ClientMessage::Subscribe { level, cursor }) = serde_json::from_str(&message) else {
            return Err(End::Close(CloseCode::InvalidMessage));
        };
        self.unsubscribe();
        for control in &mut self.control {
            control.starts = None;
        }
        self.asked = Some((level, cursor));
        Ok(())
    }

    /// Tells the client why the connection ends, as far as the client still reads, and waits
    /// for its close frame so that the reason is not lost with the socket.
    async fn finish(mut self, end: End, source: &mut SplitStream<WebSocketStream<Guarded<S>>>) {
        let limit = self.app.config.limits.stream.close_timeout.unsigned_abs();
        let code = match end {
            End::Gone => return,
            End::ClosedByClient => {
                let _ = tokio::time::timeout(limit, self.sink.flush()).await;
                return;
            }
            End::Close(code) => code,
            End::Ended(ended) => {
                if let Ok(frame) = text(&ended.message()) {
                    let sent = tokio::time::timeout(limit, self.sink.send(frame)).await;
                    if !matches!(sent, Ok(Ok(()))) {
                        return;
                    }
                }
                CloseCode::Ended
            }
        };
        let frame = Message::Close(Some(CloseFrame {
            code: code.code().into(),
            reason: Utf8Bytes::from_static(code.name()),
        }));
        let closing = async {
            if self.sink.send(frame).await.is_ok() {
                while let Some(Ok(_)) = source.next().await {}
            }
        };
        let _ = tokio::time::timeout(limit, closing).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_whose_decisions_expire_is_followed_by_a_smaller_one() {
        let mut pace = Pace::new(64, 4);
        assert_eq!(pace.size, 64);
        assert!(!pace.expired());
        assert_eq!(pace.size, 32);
        assert!(!pace.expired());
        assert!(!pace.expired());
        assert_eq!(pace.size, 8);
        pace.progressed();
        assert_eq!((pace.size, pace.failures), (16, 0));
        for _ in 0..10 {
            pace.progressed();
        }
        assert_eq!(pace.size, 64);

        let mut smallest = Pace::new(1, 100);
        smallest.expired();
        assert_eq!(smallest.size, 1);
    }

    #[test]
    fn a_connection_stalls_after_a_number_of_attempts_in_a_row_without_progress() {
        let mut pace = Pace::new(64, 3);
        assert!(!pace.failed());
        assert!(!pace.expired());
        pace.progressed();
        assert!(!pace.failed());
        assert!(!pace.failed());
        assert!(pace.expired());
    }
}
