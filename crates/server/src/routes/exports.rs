//! The routes of exports (ADR 0411). An export belongs to the account that asked for it:
//! no other account lists it, reads it, downloads it or removes it.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use junto_protocol::audit::AuditEvent;
use junto_protocol::error::ErrorCode;
use junto_protocol::export::{Export, ExportRemoval, ExportState};
use junto_protocol::ids::{ChannelId, ConversationId, ExportId};
use junto_protocol::message::MessagePlace;
use sqlx::PgConnection;

use crate::access::Access;
use crate::audit::{self, About};
use crate::download::{self, Described};
use crate::error::{ApiError, ApiResult};
use crate::exports::{self, build};
use crate::extract::{ApiPath, Bearer};
use crate::messages::Place;
use crate::recordings::files;
use crate::routes::recordings::FileBytes;
use crate::state::AppState;
use crate::storage;
use crate::{read, transact};

fn forbidden() -> ApiError {
    ApiError::new(
        ErrorCode::Forbidden,
        "an export of this channel needs the permission export_channel",
    )
}

/// Stores a new export of `place` for the caller, who may export it, or returns the export
/// of the place that already waits or is written for the caller.
async fn request(
    connection: &mut PgConnection,
    app: &AppState,
    access: &Access,
    place: &Place,
    id: ExportId,
) -> ApiResult<(Export, bool)> {
    let now = app.now();
    let limits = &app.config.limits.exports;
    app.exports.directory()?;
    let open: Vec<ExportId> = sqlx::query_scalar(
        "select id from exports
         where account_id = $1 and removed_at is null and state in ('waiting', 'running')
           and place_id = $2
         order by id",
    )
    .bind(access.account_id)
    .bind(place.id)
    .fetch_all(&mut *connection)
    .await?;
    if let Some(existing) = open.first() {
        let stored = exports::export(connection, *existing, false)
            .await?
            .ok_or_else(ApiError::not_found)?;
        return Ok((stored.export, false));
    }
    let free = app.exports.free_bytes();
    if free.is_none_or(|free| storage::at_reserve(free, app.storage_reserve_bytes())) {
        return Err(ApiError::new(
            ErrorCode::StorageLow,
            "the free disk space of the server is at its reserve; no export starts",
        ));
    }
    let (stored, waiting): (i64, i64) = sqlx::query_as(
        "select count(*), count(*) filter (where state in ('waiting', 'running'))
         from exports where account_id = $1 and removed_at is null",
    )
    .bind(access.account_id)
    .fetch_one(&mut *connection)
    .await?;
    if waiting >= i64::from(limits.most_open_per_account) {
        return Err(ApiError::limit_reached(
            "the account has the largest number of exports that wait or are written",
        ));
    }
    if stored >= i64::from(limits.most_per_account) {
        return Err(ApiError::limit_reached(
            "the account has the largest number of exports; remove one first",
        ));
    }
    place.ensure(connection).await?;
    sqlx::query(
        "insert into exports (id, account_id, place_id, state, requested_at)
         values ($1, $2, $3, 'waiting', $4)",
    )
    .bind(id)
    .bind(access.account_id)
    .bind(place.id)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    if let MessagePlace::Channel {
        workspace_id,
        channel_id,
    } = place.value
    {
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Channel(workspace_id, channel_id),
            &AuditEvent::ExportRequested { export_id: id },
        )
        .await?;
    }
    let stored = exports::export(connection, id, false)
        .await?
        .ok_or_else(ApiError::not_found)?;
    exports::announce(connection, now, &stored).await?;
    Ok((stored.export, true))
}

fn answered((export, created): (Export, bool)) -> (StatusCode, Json<Export>) {
    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    (status, Json(export))
}

/// Asks for an export of a channel: one ZIP file with its messages, attachments, recordings
/// and transcripts, as far as the caller reads them. The caller holds `export_channel` in
/// the channel. The server writes the file afterwards; the event `export_updated` tells
/// when it is ready. While an export of the channel waits or is written for the caller,
/// the answer carries that one with the status 200.
#[utoipa::path(
    post,
    path = "/api/v1/channels/{channel_id}/exports",
    tag = "exports",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 201, body = Export), (status = 200, body = Export), ApiError)
)]
pub async fn export_channel(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<(StatusCode, Json<Export>)> {
    let id = ExportId::generate();
    let now = app.now();
    let made = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let place = Place::channel(connection, &access, channel_id).await?;
        let value = place.value;
        let place = exports::permitted(connection, &access, value)
            .await?
            .ok_or_else(forbidden)?;
        request(connection, &app, &access, &place, id).await
    })?;
    app.exports.wake();
    Ok(answered(made))
}

/// Asks for an export of a direct-message conversation of the caller: its messages and
/// attachments. Only a participant asks for it.
#[utoipa::path(
    post,
    path = "/api/v1/conversations/{conversation_id}/exports",
    tag = "exports",
    params(("conversation_id" = ConversationId, Path)),
    security(("session" = [])),
    responses((status = 201, body = Export), (status = 200, body = Export), ApiError)
)]
pub async fn export_conversation(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(conversation_id): ApiPath<ConversationId>,
) -> ApiResult<(StatusCode, Json<Export>)> {
    let id = ExportId::generate();
    let now = app.now();
    let made = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let place = Place::direct(connection, &access, conversation_id).await?;
        request(connection, &app, &access, &place, id).await
    })?;
    app.exports.wake();
    Ok(answered(made))
}

/// The exports of the caller, the newest first.
#[utoipa::path(
    get,
    path = "/api/v1/exports",
    tag = "exports",
    security(("session" = [])),
    responses((status = 200, body = [Export]), ApiError)
)]
pub async fn list_exports(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<Export>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(junto_authz::Request::OwnAccount)?;
        let stored = exports::of_account(connection, access.account_id).await?;
        Ok(stored.into_iter().map(|stored| stored.export).collect())
    })
    .map(Json)
}

/// An export of the caller. An export of another account is not found.
async fn own(
    connection: &mut PgConnection,
    access: &Access,
    id: ExportId,
    for_change: bool,
) -> ApiResult<exports::ExportRow> {
    exports::export(connection, id, for_change)
        .await?
        .filter(|stored| stored.account_id == access.account_id)
        .ok_or_else(ApiError::not_found)
}

/// One export of the caller.
#[utoipa::path(
    get,
    path = "/api/v1/exports/{export_id}",
    tag = "exports",
    params(("export_id" = ExportId, Path)),
    security(("session" = [])),
    responses((status = 200, body = Export), ApiError)
)]
pub async fn get_export(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(export_id): ApiPath<ExportId>,
) -> ApiResult<Json<Export>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        Ok(own(connection, &access, export_id, false).await?.export)
    })
    .map(Json)
}

/// Removes an export of the caller with its file. An export that is written is stopped.
#[utoipa::path(
    delete,
    path = "/api/v1/exports/{export_id}",
    tag = "exports",
    params(("export_id" = ExportId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn remove_export(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(export_id): ApiPath<ExportId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        own(connection, &access, export_id, true).await?;
        exports::remove(connection, now, export_id, ExportRemoval::Removed).await?;
        Ok(())
    })?;
    app.exports.wake();
    Ok(StatusCode::NO_CONTENT)
}

/// The file of an export of the caller, whole or as one byte range. The permission is
/// decided again: an account that can no longer export the channel, or is no longer a
/// participant who reaches the other account, gets `not_found`, as for an export that is
/// not ready or has expired.
#[utoipa::path(
    get,
    path = "/api/v1/exports/{export_id}/file",
    tag = "exports",
    params(
        ("export_id" = ExportId, Path),
        ("Range" = Option<String>, Header, description = "One range of bytes")
    ),
    security(("session" = [])),
    responses(
        (status = 200, description = "The whole file.", content(
            (inline(FileBytes) = "application/zip")
        )),
        (status = 206, description = "The range of the file that `Content-Range` states.", content(
            (inline(FileBytes) = "application/zip")
        )),
        ApiError
    )
)]
pub async fn get_export_file(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(export_id): ApiPath<ExportId>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let now = app.now();
    let (account_id, attempt): (_, i32) = read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let attempt = downloadable(connection, &access, export_id, now).await?;
        Ok((access.account_id, attempt))
    })?;
    let root = app.exports.directory()?.to_path_buf();
    let place = app.recordings.download(account_id).ok_or_else(|| {
        ApiError::new(
            ErrorCode::Busy,
            "the server sends as many files as it sends at one time; send the request again",
        )
        .retry_after(time::Duration::seconds(1))
    })?;
    let file_place = files::Place::export(export_id).with(&build::finished_name(attempt));
    let opened = tokio::task::spawn_blocking(move || files::open(&root, &file_place))
        .await
        .map_err(|error| ApiError::internal(&error))?;
    let file = opened.map_err(|error| {
        if !files::is_absent(&error) {
            tracing::warn!(%error, export = %export_id, "the file of an export cannot be opened");
        }
        ApiError::not_found()
    })?;
    let file_name = format!("junto-export-{export_id}.zip");
    let described = Described {
        media_type: "application/zip",
        file_name: &file_name,
    };
    let stall = app.config.limits.recordings.download_stall.unsigned_abs();
    // A download that runs ends when the export is gone or its account may no longer
    // read it.
    let still_allowed = download::decided_again(&app, now, move |app, at| async move {
        still_downloadable(&app, bearer, export_id, at).await.ok() == Some(attempt)
    });
    download::send_while(file, &headers, described, place, stall, still_allowed).await
}

/// The attempt whose file the caller may download now: the export is the caller's, is
/// ready and has not expired, the caller may still export its place, and the file holds no
/// recording that the caller may not read now.
async fn downloadable(
    connection: &mut PgConnection,
    access: &Access,
    export_id: ExportId,
    now: time::OffsetDateTime,
) -> ApiResult<i32> {
    let stored = own(connection, access, export_id, false).await?;
    let ready = stored.export.state == ExportState::Ready
        && stored.export.expires_at.is_some_and(|at| at > now);
    if !ready {
        return Err(ApiError::not_found());
    }
    exports::permitted(connection, access, stored.export.place)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if exports::holds_hidden_recording(connection, access, export_id).await? {
        return Err(ApiError::not_found());
    }
    Ok(
        sqlx::query_scalar("select attempts from exports where id = $1")
            .bind(export_id)
            .fetch_one(connection)
            .await?,
    )
}

async fn still_downloadable(
    app: &AppState,
    bearer: Bearer,
    export_id: ExportId,
    now: time::OffsetDateTime,
) -> ApiResult<i32> {
    read!(app.background(), |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        downloadable(connection, &access, export_id, now).await
    })
}
