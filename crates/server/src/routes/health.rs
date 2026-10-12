use std::time::Duration;

use axum::Json;
use axum::extract::State;
use junto_protocol::error::ErrorCode;
use junto_protocol::server::Health;

use crate::error::{ApiError, ApiResult};
use crate::read;
use crate::state::AppState;

const DATABASE_TIME_LIMIT: Duration = Duration::from_secs(2);

async fn database_answers(app: &AppState) -> ApiResult<()> {
    read!(app, |connection| {
        sqlx::query("select 1").execute(connection).await?;
        Ok(())
    })
}

/// Whether the server reaches its database, whether its last admission check reached the
/// media server, and whether the free disk space of the recordings is at the reserve
/// (ADR 0012, ADR 0605). The status follows the database only.
#[utoipa::path(
    get,
    path = "/api/v1/health",
    tag = "server",
    responses((status = 200, body = Health), ApiError)
)]
pub async fn get_health(State(app): State<AppState>) -> ApiResult<Json<Health>> {
    match tokio::time::timeout(DATABASE_TIME_LIMIT, database_answers(&app)).await {
        Ok(Ok(())) => Ok(Json(Health {
            database: true,
            media_server: app.calls.reachable(),
            failing_call_rooms: app.calls.failing_rooms(),
            storage_low: crate::maintenance::storage_low(&app).await,
        })),
        Ok(Err(_)) | Err(_) => Err(ApiError::new(
            ErrorCode::Unavailable,
            "the server does not reach its database",
        )
        .retry_after(time::Duration::seconds(1))),
    }
}
