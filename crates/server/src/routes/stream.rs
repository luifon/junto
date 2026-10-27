use axum::extract::{Request, State};
use axum::response::Response;
use junto_authz::Request as Permission;

use crate::access::Access;
use crate::error::{ApiError, ApiResult};
use crate::events::connection;
use crate::events::delivery::Opened;
use crate::events::log;
use crate::events::transport::Upgrade;
use crate::extract::Bearer;
use crate::proof;
use crate::read;
use crate::state::AppState;

/// Opens the event stream of the caller's device as a WebSocket. The messages are described
/// in `docs/api/events.json`.
#[utoipa::path(
    get,
    path = "/api/v1/events",
    tag = "events",
    security(("session" = [])),
    responses(
        (status = 101, description = "The connection is now a WebSocket."),
        ApiError
    )
)]
pub async fn open_event_stream(
    State(app): State<AppState>,
    bearer: Bearer,
    mut request: Request,
) -> ApiResult<Response> {
    let now = app.now();
    let (who, epoch) = read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Permission::OwnAccount)?;
        let who = Opened {
            bearer,
            account_id: access.account_id,
            device_id: access.device_id,
            session_id: access.session_id,
        };
        Ok((who, log::state(connection).await?.epoch))
    })?;
    let upgrade = Upgrade::of(&mut request).ok_or_else(|| {
        ApiError::invalid("the event stream is opened with a WebSocket upgrade request")
    })?;
    proof::limit(&app.stream_opens, who.account_id, now)?;
    let registration = app
        .streams
        .open(who.account_id, who.device_id, who.session_id, now)?;

    let link = registration.link().clone();
    let response = upgrade.response();
    let task = tokio::spawn(async move {
        let limits = app.streams.limits();
        let upgraded =
            tokio::time::timeout(limits.send_timeout.unsigned_abs(), upgrade.socket(&limits));
        match upgraded.await {
            Ok(Ok(socket)) => connection::run(app, socket, who, epoch, registration).await,
            Ok(Err(error)) => tracing::debug!(%error, "a WebSocket upgrade did not complete"),
            Err(_) => tracing::debug!("a WebSocket upgrade did not complete in time"),
        }
    });
    link.served_by(task.abort_handle());
    Ok(response)
}
