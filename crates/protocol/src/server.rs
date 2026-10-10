use serde::{Deserialize, Serialize};

use crate::ids::ServerId;

/// What a server tells any caller before the caller has an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct ServerInfo {
    pub server_id: ServerId,
    pub name: String,
    /// The path of the logo on this server, when the server has one.
    pub logo_path: Option<String>,
    pub api_versions: Vec<u32>,
}

/// What the health route reports when the server can serve requests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
pub struct Health {
    /// True when the server reached its database for this response.
    pub database: bool,
    /// Whether the last admission check reached the media server. Absent on a server that
    /// has no media server configured. While it is false, no call can be joined.
    pub media_server: Option<bool>,
    /// The number of rooms of calls that the server could not check several times in a row,
    /// while the media server answers otherwise. In such a room the server cannot remove a
    /// person, and it lets nobody in.
    #[serde(default)]
    pub failing_call_rooms: u32,
}
