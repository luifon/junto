//! The route that the media server calls. It is not part of the public API (ADR 0201).

use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::AUTHORIZATION;
use axum::http::{HeaderMap, StatusCode};

use junto_protocol::error::ErrorCode;

use crate::calls::Notified;
use crate::error::{ApiError, ApiResult};
use crate::proof;
use crate::state::AppState;

pub const MEDIA_NOTIFICATIONS: &str = "/internal/media/notifications";

/// Takes a notification of the media server. A notification whose signature the media server
/// made for exactly this body makes the admission check look at the room that it names at
/// once, and the controller of recordings look at the recordings; it changes nothing by
/// itself, and a lost one is made up for by the full check and by the next pass of the
/// controller (ADR 0509, ADR 0604).
///
/// The same notification is processed once while its signature is valid, and the number of
/// notifications that start a check in a time is limited: a holder of one valid notification
/// cannot make the server check again and again.
pub async fn media_notification(
    State(app): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let authorization = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(ApiError::unauthenticated)?;
    let media = app.calls.media().ok_or_else(ApiError::not_found)?;
    let now = app.now();
    let notification = media
        .verify_notification(authorization, &body, now)
        .ok_or_else(ApiError::unauthenticated)?;
    tracing::debug!(event = %notification.event, "the media server sent a notification");
    let key = notification.key;
    let most = app.config.limits.calls.most_remembered_notifications;
    match app.calls.notified(key, notification.valid_until, now, most) {
        Notified::Repeated => return Ok(StatusCode::NO_CONTENT),
        Notified::Full => {
            return Err(ApiError::new(
                ErrorCode::RateLimited,
                "the server holds as many notifications as it remembers",
            )
            .retry_after(time::Duration::seconds(2)));
        }
        Notified::First => {}
    }
    if let Err(limited) = proof::limit(&app.calls.notifications, (), now) {
        app.calls.not_notified(key);
        return Err(limited);
    }
    match &notification.room {
        Some(room) => app.calls.mark(room),
        None => app.calls.mark_everything(),
    }
    app.recordings.poke();
    Ok(StatusCode::NO_CONTENT)
}
