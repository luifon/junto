use axum::Json;
use axum::extract::State;
use junto_protocol::API_VERSION;
use junto_protocol::ids::ServerId;
use junto_protocol::server::ServerInfo;

use crate::error::{ApiError, ApiResult};
use crate::read;
use crate::state::AppState;

/// The name, the logo and the supported API versions of this server.
#[utoipa::path(
    get,
    path = "/api/server",
    tag = "server",
    responses((status = 200, body = ServerInfo), ApiError)
)]
pub async fn get_server_info(State(app): State<AppState>) -> ApiResult<Json<ServerInfo>> {
    let (server_id, name, logo_path): (ServerId, String, Option<String>) =
        read!(app, |connection| {
            Ok(
                sqlx::query_as("select server_id, name, logo_path from server_state")
                    .fetch_one(connection)
                    .await?,
            )
        })?;
    Ok(Json(ServerInfo {
        server_id,
        name,
        logo_path,
        api_versions: vec![API_VERSION],
    }))
}
