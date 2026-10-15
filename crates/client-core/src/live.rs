//! What a user interface reads of one server while the event stream runs (ADR 0719).
//!
//! Every read returns at once: a view is a status and a shared, unchangeable copy of the
//! state. A reader waits for the next change with [`LiveWatch`] and never asks at an
//! interval. Both work on any executor, so the thread of an interface does not block.

use std::future::Future;
use std::sync::{Arc, Mutex};

use junto_protocol::account::RecoveryNotice;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::{Cursor, EndReason, Event, Level};
use junto_protocol::ids::{AttachmentId, ChannelId, MessageId, RecordingId, WorkspaceId};
use junto_protocol::message::Document;
use time::OffsetDateTime;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::api::{ApiError, TransportErrorKind};
use crate::flow::machine::lock;
use crate::model::{AdminPart, PendingFile, PendingMessage, PlaceId, Reach, ServerModel};

mod admin;

/// How many notices wait for a reader that is slower than the stream. A reader that falls
/// further behind is told so and reads the state.
const WAITING_NOTICES: usize = 1024;

/// Why the event stream of a server has no connection that delivers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFault {
    /// No connection was made, or it failed.
    Unreachable(TransportErrorKind),
    /// The server refused the connection (ADR 0116): a limit on connections, or another
    /// error. The code is `unknown` for a response that is no error of the API.
    Refused(ErrorCode),
    /// The server closed the connection with this code of the close frame.
    Closed(Option<u16>),
    /// No frame arrived within the time that the heartbeat of the server allows, or no
    /// answer to a `subscribe`.
    Silent,
    /// The server sent what this client cannot read.
    Unreadable,
    /// The server no longer accepts the session. The client logs in again once
    /// (ADR 0712); the access flow shows the result.
    SessionEnded(Option<EndReason>),
}

/// What the event stream of one server does now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StreamStatus {
    /// No connection is wanted: the device is not logged in, the server has the mode
    /// "connect only when selected" and is not selected, or the streams were not started.
    #[default]
    Off,
    /// The client connects, loads the state or waits for the answer to its `subscribe`.
    Connecting,
    /// Events arrive and the state follows the server.
    Live,
    /// The connection is lost. The state is the last one that was known, and the client
    /// tries again by itself.
    Interrupted(StreamFault),
}

/// The status of the stream and the state of the server at one moment.
#[derive(Debug, Clone, Default)]
pub struct LiveView {
    pub status: StreamStatus,
    /// The level of the subscription that keeps the state current. At the background
    /// level the structure of the workspaces is as old as the last snapshot (ADR 0107).
    pub level: Option<Level>,
    /// Absent until the first snapshot has arrived, and while the status is `Off`.
    pub model: Option<Arc<ServerModel>>,
}

/// One step of the event stream, for a reader that shows each change by itself. The state
/// is the truth; a reader that only draws the state does not need the notices.
#[derive(Debug, Clone)]
pub enum Notice {
    /// An event was applied to the state.
    Event {
        id: u64,
        at: OffsetDateTime,
        event: Arc<Event>,
    },
    /// The state was replaced by a snapshot. Changes between the state before and this
    /// one were not told as events.
    Reloaded { cursor: Cursor },
    /// The server ended the session of the connection (ADR 0208).
    Ended {
        reason: EndReason,
        recovery: Option<RecoveryNotice>,
    },
}

/// The reader was slower than the stream and did not get this number of notices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Missed(pub u64);

/// Why a part of the state was not loaded.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("the server has no event stream now")]
    Off,
    #[error("the state holds no such workspace or place")]
    NotInState,
    /// The request for the part failed. Every reader that waited gets the same error.
    #[error(transparent)]
    Request(Arc<ApiError>),
}

pub(crate) enum Command {
    Refresh {
        reply: oneshot::Sender<Result<Cursor, LoadError>>,
    },
    /// The login after an ended session got no answer of the server, and the client core
    /// keeps the session (ADR 0787).
    LoginUnanswered,
    LoadMembers {
        workspace_id: WorkspaceId,
        reply: oneshot::Sender<Result<(), LoadError>>,
    },
    ReleaseMembers(WorkspaceId),
    OpenMessages {
        place: PlaceId,
        reply: oneshot::Sender<Result<(), LoadError>>,
    },
    ReleaseMessages(PlaceId),
    ReadMessages {
        place: PlaceId,
        reach: Reach,
    },
    /// The position that the reader shows: absent while the reader follows the end.
    MessageAnchor {
        place: PlaceId,
        position: Option<u64>,
    },
    MarkRead {
        place: PlaceId,
        position: u64,
    },
    Enqueue(Box<PendingMessage>),
    /// The uploads of a waiting message are complete: the upload of this client and the
    /// attachment that the server holds for it.
    FilesReady {
        id: u64,
        files: Vec<(u64, AttachmentId)>,
    },
    FilesFailed(u64),
    SendAgain(u64),
    Discard(u64),
    OpenPins {
        channel_id: ChannelId,
        reply: oneshot::Sender<Result<(), LoadError>>,
    },
    ReleasePins(ChannelId),
    /// The page of unread threads after the loaded ones.
    MoreThreads,
    /// The page of notices after the loaded ones.
    MoreNotices,
    OpenRecording {
        recording_id: RecordingId,
        reply: oneshot::Sender<Result<(), LoadError>>,
    },
    ReleaseRecording(RecordingId),
    /// The lines of the transcript of an open recording after the loaded ones.
    MoreTranscriptLines(RecordingId),
    EarlierTranscriptLines(RecordingId),
    TranscriptLinesAround {
        recording_id: RecordingId,
        line: u32,
    },
    OpenAdmin {
        part: AdminPart,
        reply: oneshot::Sender<Result<(), LoadError>>,
    },
    ReleaseAdmin(AdminPart),
    /// Reads again a collection whose last read failed.
    ReadAdminAgain(AdminPart),
}

/// What the stream client of one server and its readers share.
pub(crate) struct Shared {
    view: watch::Sender<LiveView>,
    notices: broadcast::Sender<Notice>,
    commands: Mutex<Option<mpsc::UnboundedSender<Command>>>,
    /// The numbers that this client gives to its waiting messages and to its uploads.
    numbers: std::sync::atomic::AtomicU64,
}

impl Shared {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            view: watch::Sender::new(LiveView::default()),
            notices: broadcast::Sender::new(WAITING_NOTICES),
            commands: Mutex::new(None),
            numbers: std::sync::atomic::AtomicU64::new(1),
        })
    }

    pub(crate) fn next_number(&self) -> u64 {
        self.numbers
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    pub(crate) fn attach(&self, commands: mpsc::UnboundedSender<Command>) {
        *lock(&self.commands) = Some(commands);
    }

    /// The stream client of the server has ended.
    pub(crate) fn detach(&self) {
        *lock(&self.commands) = None;
        self.show(LiveView::default());
    }

    pub(crate) fn show(&self, view: LiveView) {
        self.view.send_replace(view);
    }

    pub(crate) fn model(&self) -> Option<Arc<ServerModel>> {
        self.view.borrow().model.clone()
    }

    pub(crate) fn tell(&self, notice: Notice) {
        let _ = self.notices.send(notice);
    }

    pub(crate) fn send(&self, command: Command) -> bool {
        lock(&self.commands)
            .as_ref()
            .is_some_and(|commands| commands.send(command).is_ok())
    }
}

/// The state of one saved server and its changes.
#[derive(Clone)]
pub struct LiveServer {
    pub(crate) shared: Arc<Shared>,
}

impl LiveServer {
    /// The status and the state now. The call copies two words and a pointer.
    pub fn view(&self) -> LiveView {
        self.shared.view.borrow().clone()
    }

    /// A value that gives the newest view after each change.
    pub fn watch(&self) -> LiveWatch {
        LiveWatch {
            receiver: self.shared.view.subscribe(),
        }
    }

    /// Every step of the stream from now on.
    pub fn notices(&self) -> Notices {
        Notices {
            receiver: self.shared.notices.subscribe(),
        }
    }

    /// Replaces the state with a snapshot requested after this call, and returns its
    /// cursor only after the replacement is visible to readers.
    pub fn refresh(
        &self,
    ) -> impl Future<Output = Result<Cursor, LoadError>> + Send + 'static + use<> {
        let (reply, answer) = oneshot::channel();
        let sent = self.shared.send(Command::Refresh { reply });
        async move {
            if !sent {
                return Err(LoadError::Off);
            }
            answer.await.unwrap_or(Err(LoadError::Off))
        }
    }

    /// Asks for the members of a workspace. They are read from their route, appear in the
    /// state, and are kept current by the events from then on; after a reload of the state
    /// the client reads them again by itself (ADR 0207). The future ends when the members
    /// are in the state or the read failed.
    pub fn load_members(
        &self,
        workspace_id: WorkspaceId,
    ) -> impl Future<Output = Result<(), LoadError>> + Send + 'static + use<> {
        let (reply, answer) = oneshot::channel();
        let sent = self.shared.send(Command::LoadMembers {
            workspace_id,
            reply,
        });
        async move {
            if !sent {
                return Err(LoadError::Off);
            }
            answer.await.unwrap_or(Err(LoadError::Off))
        }
    }

    /// States that no reader needs the members of a workspace any more. The state drops
    /// them and does not read them again.
    pub fn release_members(&self, workspace_id: WorkspaceId) {
        self.shared.send(Command::ReleaseMembers(workspace_id));
    }

    /// Asks for the messages of a place (ADR 0721). The first page is read, appears in
    /// the state as a timeline, and follows the events from then on. A place with unread
    /// messages opens at the read position of the account, and every other place at its
    /// end. After a reload of the state the client reads the page again by itself. The
    /// future ends when the page is in the state or the read failed. Each call needs one
    /// [`LiveServer::release_messages`].
    pub fn open_messages(
        &self,
        place: PlaceId,
    ) -> impl Future<Output = Result<(), LoadError>> + Send + 'static + use<> {
        let (reply, answer) = oneshot::channel();
        let sent = self.shared.send(Command::OpenMessages { place, reply });
        async move {
            if !sent {
                return Err(LoadError::Off);
            }
            answer.await.unwrap_or(Err(LoadError::Off))
        }
    }

    /// States that a reader needs the messages of a place no longer. When no reader is
    /// left, the state drops them.
    pub fn release_messages(&self, place: PlaceId) {
        self.shared.send(Command::ReleaseMessages(place));
    }

    /// Reads the page before the first loaded message of an open place.
    pub fn read_older_messages(&self, place: PlaceId) {
        let reach = Reach::Older;
        self.shared.send(Command::ReadMessages { place, reach });
    }

    /// Reads the page after the last loaded message of an open place.
    pub fn read_newer_messages(&self, place: PlaceId) {
        let reach = Reach::Newer;
        self.shared.send(Command::ReadMessages { place, reach });
    }

    /// Replaces the loaded messages of an open place by its latest page.
    pub fn read_latest_messages(&self, place: PlaceId) {
        let reach = Reach::End;
        self.shared.send(Command::ReadMessages { place, reach });
    }

    /// Replaces the loaded messages of an open place by the messages around a position:
    /// the ones up to it and the ones after it. A reader uses it to show one message of
    /// the history, such as a search result. The position becomes the anchor of the place.
    pub fn read_messages_around(&self, place: PlaceId, position: u64) {
        let reach = Reach::At(position);
        self.shared.send(Command::ReadMessages { place, reach });
    }

    /// Tells where the reader of an open place is: at the position of a message, or at the
    /// end (`None`). After a reload of the state the client reads the page from there.
    pub fn set_message_anchor(&self, place: PlaceId, position: Option<u64>) {
        self.shared.send(Command::MessageAnchor { place, position });
    }

    /// Moves the read position of the account in a place forward. The client sends one
    /// request for a place at a time, at most one in a second, with the newest position.
    pub fn mark_read(&self, place: PlaceId, position: u64) {
        self.shared.send(Command::MarkRead { place, position });
    }

    /// Puts a message into the state as one that waits to be sent, and sends it when all
    /// its files are uploaded. Returns the number of the message on this client. `None`
    /// when the server has no event stream now.
    pub(crate) fn enqueue_message(
        &self,
        place: PlaceId,
        document: Document,
        reply_to: Option<MessageId>,
        files: Vec<PendingFile>,
    ) -> Option<u64> {
        let id = self.shared.next_number();
        let created_at = OffsetDateTime::now_utc();
        let pending = PendingMessage::new(id, place, document, reply_to, files, created_at);
        self.shared
            .send(Command::Enqueue(Box::new(pending)))
            .then_some(id)
    }

    /// Sends a message without files.
    pub fn send_message(
        &self,
        place: PlaceId,
        document: Document,
        reply_to: Option<MessageId>,
    ) -> Option<u64> {
        self.enqueue_message(place, document, reply_to, Vec::new())
    }

    /// Sends a waiting message again: one that failed, or one whose answer was lost. The
    /// second kind is sent only after the history shows that the server does not hold it
    /// (ADR 0721).
    pub fn send_again(&self, id: u64) {
        self.shared.send(Command::SendAgain(id));
    }

    /// Takes a waiting message out of the state. A request that is on its way is ended;
    /// the server may hold the message all the same.
    pub fn discard_message(&self, id: u64) {
        self.shared.send(Command::Discard(id));
    }
}

impl LiveServer {
    /// Asks for the pinned messages of a channel (ADR 0722). The list is read, appears in
    /// the state, and follows the events from then on; after a reload of the state the
    /// client reads it again by itself. The future ends when the list is in the state or
    /// the read failed. Each call needs one [`LiveServer::release_pins`].
    pub fn open_pins(
        &self,
        channel_id: ChannelId,
    ) -> impl Future<Output = Result<(), LoadError>> + Send + 'static + use<> {
        let (reply, answer) = oneshot::channel();
        let sent = self.shared.send(Command::OpenPins { channel_id, reply });
        async move {
            if !sent {
                return Err(LoadError::Off);
            }
            answer.await.unwrap_or(Err(LoadError::Off))
        }
    }

    /// States that a reader needs the pinned messages of a channel no longer.
    pub fn release_pins(&self, channel_id: ChannelId) {
        self.shared.send(Command::ReleasePins(channel_id));
    }

    /// Reads the page of unread threads after the loaded ones, while the state says that
    /// there are more. A later change of the threads replaces the list by its first page.
    pub fn read_more_threads(&self) {
        self.shared.send(Command::MoreThreads);
    }

    /// Reads the page of notices after the loaded ones.
    pub fn read_more_notices(&self) {
        self.shared.send(Command::MoreNotices);
    }
}

impl LiveServer {
    /// Asks for a recording with its segments and its transcript. Both are read, appear
    /// in the state, and follow the events from then on: `recording_updated` reads them
    /// again, a rename of a speaker refreshes the labels through the transcript route, and `recording_removed` marks
    /// the recording as gone. After a reload of the state the client reads them again by
    /// itself. The future ends when the recording is in the state or the read failed;
    /// the state then tells why, or that the server has no such recording for the
    /// account. Each call needs one [`LiveServer::release_recording`]. A call for a
    /// recording whose last read failed reads it again.
    pub fn open_recording(
        &self,
        recording_id: RecordingId,
    ) -> impl Future<Output = Result<(), LoadError>> + Send + 'static + use<> {
        let (reply, answer) = oneshot::channel();
        let sent = self.shared.send(Command::OpenRecording {
            recording_id,
            reply,
        });
        async move {
            if !sent {
                return Err(LoadError::Off);
            }
            answer.await.unwrap_or(Err(LoadError::Off))
        }
    }

    /// Retries a failed read for an existing reader without changing its ownership.
    pub fn reload_recording(
        &self,
        recording_id: RecordingId,
    ) -> impl Future<Output = Result<(), LoadError>> + Send + 'static + use<> {
        let opening = self.open_recording(recording_id);
        self.release_recording(recording_id);
        opening
    }

    /// States that a reader needs a recording no longer. When no reader is left, the
    /// state drops it.
    pub fn release_recording(&self, recording_id: RecordingId) {
        self.shared.send(Command::ReleaseRecording(recording_id));
    }

    /// Reads the next page of the lines of the transcript of an open recording: the first
    /// page when none is loaded, and else the page after the last loaded line, while the
    /// state says that there are more. Lines that a reload of the state marked as not
    /// current are read again from the first one. A correction of a loaded line
    /// reads the held lines again through the authorized route.
    pub fn read_transcript_lines(&self, recording_id: RecordingId) {
        self.shared.send(Command::MoreTranscriptLines(recording_id));
    }

    /// Reads the page of lines before the first loaded line, while the state says that the
    /// transcript has lines before it.
    pub fn read_earlier_transcript_lines(&self, recording_id: RecordingId) {
        self.shared
            .send(Command::EarlierTranscriptLines(recording_id));
    }

    /// Reads the lines around one line of the transcript of an open recording, in place of
    /// the loaded ones, when the state does not hold that line. After a reload the lines
    /// are read around it again.
    pub fn read_transcript_lines_around(&self, recording_id: RecordingId, line: u32) {
        self.shared
            .send(Command::TranscriptLinesAround { recording_id, line });
    }
}

#[cfg(feature = "test-support")]
impl LiveServer {
    /// Shows a status and a state to the readers, as the stream client does.
    pub fn show_for_test(&self, status: StreamStatus, model: Option<ServerModel>) {
        self.shared.show(LiveView {
            status,
            level: None,
            model: model.map(Arc::new),
        });
    }

    /// Gives a notice to the readers, as the stream client does.
    pub fn tell_for_test(&self, notice: Notice) {
        self.shared.tell(notice);
    }
}

/// Wakes a reader when the view of one server has changed.
pub struct LiveWatch {
    receiver: watch::Receiver<LiveView>,
}

impl LiveWatch {
    /// Waits for a change and returns the newest view. Several changes can arrive as one.
    /// `None` when the client does not exist any more.
    pub async fn changed(&mut self) -> Option<LiveView> {
        self.receiver.changed().await.ok()?;
        Some(self.receiver.borrow_and_update().clone())
    }

    /// Waits until the view fulfils a condition and returns it.
    pub async fn wait_for(&mut self, holds: impl FnMut(&LiveView) -> bool) -> Option<LiveView> {
        self.receiver
            .wait_for(holds)
            .await
            .ok()
            .map(|view| view.clone())
    }
}

/// The notices of one server in their order.
pub struct Notices {
    receiver: broadcast::Receiver<Notice>,
}

impl Notices {
    /// The next notice. `Err` when this reader was too slow: it continues with the oldest
    /// notice that is still held. `None` when the client does not exist any more.
    pub async fn next(&mut self) -> Option<Result<Notice, Missed>> {
        match self.receiver.recv().await {
            Ok(notice) => Some(Ok(notice)),
            Err(broadcast::error::RecvError::Lagged(missed)) => Some(Err(Missed(missed))),
            Err(broadcast::error::RecvError::Closed) => None,
        }
    }
}
