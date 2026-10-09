use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::ids::ServerId;

/// What a server tells any caller before the caller has an account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ServerInfo {
    pub server_id: ServerId,
    pub name: String,
    /// The path of the logo on this server, when the server has one.
    pub logo_path: Option<String>,
    pub api_versions: Vec<u32>,
}
