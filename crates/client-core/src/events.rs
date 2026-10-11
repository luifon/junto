//! The connection level of each server, and what the client core gives the event stream
//! client of a server (ADR 0107, ADR 0715).

use junto_protocol::ids::{AccountId, DeviceId, ServerId};

use crate::address::ServerAddress;
use crate::manager::Server;
use crate::secret::SecretToken;
use crate::version::ApiVersion;

/// How much of the event stream of one server the client wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ConnectionLevel {
    /// No connection: the device is not logged in, or the server has the mode "connect
    /// only when selected", is not selected, and this device is in no call on it.
    #[default]
    Off,
    /// Direct messages, mentions, call invites and control events.
    Background,
    /// Every event.
    Selected,
}

/// What a stream needs to connect to one server. The state that the stream keeps belongs
/// to `account_id`; `device_id` tells whether a call of the account is on this device.
#[derive(Debug, Clone)]
pub(crate) struct StreamTarget {
    pub(crate) server_id: ServerId,
    pub(crate) address: ServerAddress,
    pub(crate) api_version: ApiVersion,
    pub(crate) token: SecretToken,
    pub(crate) account_id: AccountId,
    pub(crate) device_id: DeviceId,
}

/// The event streams of all servers. The client core calls it when the level of a server
/// or its session changes: after a login, a logout, a renewal of the session, a ban, a
/// removal, a selection, a change of the connection mode, and when this device enters or
/// leaves a call. `target` is present when the level is not [`ConnectionLevel::Off`].
pub(crate) trait EventStreams: Send + Sync + 'static {
    fn set_level(&self, server: &Server, level: ConnectionLevel, target: Option<StreamTarget>);
}
