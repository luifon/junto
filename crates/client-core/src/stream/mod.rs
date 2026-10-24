//! The event stream client: one connection per saved server (ADR 0107, ADR 0718).

mod connection;
mod socket;

#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use junto_protocol::events::EndReason;
use tokio::sync::{mpsc, watch};

use crate::api::{RetryPolicy, Transport};
use crate::events::{ConnectionLevel, StreamTarget};
use crate::flow::machine::lock;
use crate::live::Shared;
use crate::runtime::Runtime;
use crate::secret::SecretToken;

pub use socket::{
    Incoming, MAX_MESSAGE_BYTES, OpenError, OpenFuture, ReqwestStreamTransport, StreamRequest,
    StreamSocket, StreamTransport,
};

/// The times and bounds of the stream client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamPolicy {
    /// The wait before the first new attempt after a lost connection. Each further
    /// attempt waits twice as long, up to `longest_delay`. The actual wait is a random
    /// time between half of that and all of it.
    pub first_delay: Duration,
    pub longest_delay: Duration,
    /// A connection that delivered for this long starts again at `first_delay`.
    pub settled_after: Duration,
    /// The time for the upgrade request and its answer.
    pub open_timeout: Duration,
    /// The time from the upgrade to the `hello` of the server.
    pub hello_timeout: Duration,
    /// The time for the answer to a `subscribe`.
    pub subscribe_timeout: Duration,
    /// The server sends a frame at each heartbeat (ADR 0116). A connection without a
    /// frame for two heartbeats and this time is taken as lost.
    pub silence_grace: Duration,
    /// How many messages the client sends in one minute at most. The server closes a
    /// connection after 30 frames in one minute, the answers to its pings included.
    pub messages_per_minute: usize,
    /// How many connection attempts the client makes for one server in one minute at
    /// most. The server accepts 30 upgrade requests of an account in one minute.
    pub connections_per_minute: usize,
    /// A message whose answer was lost is not sent again before this time after the
    /// request: the server ends a request after 30 seconds (ADR 0008), and until then the
    /// first request can still take effect (ADR 0721).
    pub send_again_after: Duration,
    /// The shortest time between two reads of the list of unread threads, and between two
    /// reads of the first page of the notices (ADR 0722). The limit of the server for reads
    /// (120 in a minute) is shared with the history.
    pub list_interval: Duration,
}

impl Default for StreamPolicy {
    fn default() -> Self {
        Self {
            first_delay: Duration::from_secs(1),
            longest_delay: Duration::from_secs(60),
            settled_after: Duration::from_secs(30),
            open_timeout: Duration::from_secs(20),
            hello_timeout: Duration::from_secs(10),
            subscribe_timeout: Duration::from_secs(30),
            silence_grace: Duration::from_secs(10),
            messages_per_minute: 20,
            connections_per_minute: 20,
            send_again_after: Duration::from_secs(30),
            list_interval: Duration::from_secs(2),
        }
    }
}

/// What the stream client of one server asks of the client core.
pub(crate) trait Host: Send + Sync + 'static {
    /// The server no longer accepts the session of `token`, for `reason` when the server
    /// stated one in `ended`.
    fn session_ended(&self, token: &SecretToken, reason: Option<EndReason>);
    /// Whether the account is in a call with this device now.
    fn in_call(&self, in_call: bool);
}

/// What a stream client should hold now.
#[derive(Clone)]
pub(crate) struct Desired {
    pub(crate) level: ConnectionLevel,
    pub(crate) target: Option<StreamTarget>,
}

pub(crate) struct Environment {
    pub(crate) transport: Arc<dyn Transport>,
    pub(crate) sockets: Arc<dyn StreamTransport>,
    pub(crate) retry: RetryPolicy,
    pub(crate) policy: StreamPolicy,
}

struct Entry {
    shared: Arc<Shared>,
    desired: watch::Sender<Desired>,
}

/// The stream clients of all servers. Each runs as one task that ends when this value
/// is dropped or its server is removed.
pub(crate) struct Streams {
    environment: Arc<Environment>,
    runtime: Runtime,
    entries: Mutex<Vec<Entry>>,
}

impl Streams {
    pub(crate) fn new(environment: Environment, runtime: Runtime) -> Self {
        Self {
            environment: Arc::new(environment),
            runtime,
            entries: Mutex::new(Vec::new()),
        }
    }

    /// Sets what the stream client of one server should hold. `gone` states that the
    /// server was removed from the client: its task ends.
    pub(crate) fn set(
        &self,
        shared: &Arc<Shared>,
        host: impl FnOnce() -> Arc<dyn Host>,
        desired: Desired,
        gone: bool,
    ) {
        let mut entries = lock(&self.entries);
        let position = entries
            .iter()
            .position(|entry| Arc::ptr_eq(&entry.shared, shared));
        match position {
            Some(position) if gone => {
                entries.swap_remove(position);
            }
            Some(position) => {
                entries[position].desired.send_replace(desired);
            }
            None if gone || desired.level == ConnectionLevel::Off => {}
            None => {
                let (sender, receiver) = watch::channel(desired);
                let (commands, received) = mpsc::unbounded_channel();
                shared.attach(commands);
                let task = connection::run(
                    shared.clone(),
                    host(),
                    self.environment.clone(),
                    receiver,
                    received,
                );
                drop(self.runtime.run(task));
                entries.push(Entry {
                    shared: shared.clone(),
                    desired: sender,
                });
            }
        }
    }
}
