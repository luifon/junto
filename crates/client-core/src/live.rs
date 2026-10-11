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
use junto_protocol::ids::WorkspaceId;
use time::OffsetDateTime;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::api::{ApiError, TransportErrorKind};
use crate::flow::machine::lock;
use crate::model::ServerModel;

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
    #[error("the state holds no such workspace")]
    NotInState,
    /// The request for the part failed. Every reader that waited gets the same error.
    #[error(transparent)]
    Request(Arc<ApiError>),
}

pub(crate) enum Command {
    LoadMembers {
        workspace_id: WorkspaceId,
        reply: oneshot::Sender<Result<(), LoadError>>,
    },
    ReleaseMembers(WorkspaceId),
}

/// What the stream client of one server and its readers share.
pub(crate) struct Shared {
    view: watch::Sender<LiveView>,
    notices: broadcast::Sender<Notice>,
    commands: Mutex<Option<mpsc::UnboundedSender<Command>>>,
}

impl Shared {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self {
            view: watch::Sender::new(LiveView::default()),
            notices: broadcast::Sender::new(WAITING_NOTICES),
            commands: Mutex::new(None),
        })
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

    fn send(&self, command: Command) -> bool {
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
