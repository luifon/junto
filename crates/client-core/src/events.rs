//! Where the event stream of a server attaches (ADR 0107).
//!
//! The stream client is not part of this crate yet. This module fixes what it gets from
//! the client core: the level that each server should have, what it needs to connect, and
//! the cursor that it stores through `ServerHandle::store_cursor`.

use junto_protocol::ids::ServerId;

use crate::address::ServerAddress;
use crate::book::EventCursor;
use crate::secret::SecretToken;
use crate::version::ApiVersion;

/// How much of the event stream of one server the client wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ConnectionLevel {
    /// No connection: the device is not logged in, or the server has the mode "connect
    /// only when selected" and is not selected.
    #[default]
    Off,
    /// Direct messages, mentions, call invites and control events.
    Background,
    /// Every event.
    Selected,
}

/// What a stream needs to connect to one server.
#[derive(Debug, Clone)]
pub struct StreamTarget {
    pub address: ServerAddress,
    pub api_version: ApiVersion,
    pub token: SecretToken,
    /// The cursor that was stored last, to continue after it.
    pub cursor: Option<EventCursor>,
}

/// The event streams of all servers. The client core calls it when the level of a server
/// changes: after a login, a logout, a ban, a removal, a selection or a change of the
/// connection mode. `target` is present when the level is not [`ConnectionLevel::Off`].
pub trait EventStreams: Send + Sync + 'static {
    fn set_level(&self, server_id: ServerId, level: ConnectionLevel, target: Option<StreamTarget>);
}
