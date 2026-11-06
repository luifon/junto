//! The logo of a workspace (ADR 0122): a still PNG, JPEG or WebP within the limits of an
//! avatar (ADR 0119). A server administrator sets and removes it; a person who sees the
//! workspace reads it.

use axum::http::{
    HeaderMap, HeaderValue, StatusCode,
    header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH, X_CONTENT_TYPE_OPTIONS},
};
use axum::response::{IntoResponse, Response};
use axum::{Json, body::Bytes, extract::State};
use junto_authz::{Request, ServerPermission};
use junto_protocol::{account::Avatar, audit::AuditEvent, error::ErrorCode, ids::WorkspaceId};
use sha2::{Digest, Sha256};
use sqlx::PgConnection;
use std::collections::HashMap;
use time::OffsetDateTime;

use super::account::{ImageBytes, limit_change};
use crate::{
    access::Access,
    audit::{self, About},
    error::{ApiError, ApiResult},
    extract::{ApiPath, Bearer},
    image, read,
    state::AppState,
    store, transact,
};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The logos of workspaces, by workspace.
pub(crate) async fn logos(
    connection: &mut PgConnection,
    workspace_ids: &[WorkspaceId],
) -> ApiResult<HashMap<WorkspaceId, Avatar>> {
    let rows: Vec<(WorkspaceId, Vec<u8>, String, i32, i32, i32)> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select workspace_id, {} from workspace_logos where workspace_id = any($1)",
            store::AVATAR_COLUMNS
        )))
        .bind(workspace_ids)
        .fetch_all(&mut *connection)
        .await?;
    rows.into_iter()
        .map(|(workspace_id, hash, media_type, bytes, width, height)| {
            Ok((
                workspace_id,
                store::avatar((hash, media_type, bytes, width, height))?,
            ))
        })
        .collect()
}

/// The logo of one workspace.
pub(crate) async fn logo(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
) -> ApiResult<Option<Avatar>> {
    Ok(logos(connection, &[workspace_id])
        .await?
        .remove(&workspace_id))
}

/// The image of the logo of a workspace that the caller sees.
#[utoipa::path(get, path = "/api/v1/workspaces/{workspace_id}/logo", tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path)), security(("session" = [])),
    responses((status = 304, description = "The current image matches If-None-Match"), (status = 200, content((inline(ImageBytes) = "image/png"), (inline(ImageBytes) = "image/jpeg"), (inline(ImageBytes) = "image/webp"))), ApiError))]
pub async fn get_logo(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let now = app.now();
    let hashes: Vec<String> = headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter_map(|value| {
            let value = value.strip_prefix("W/").unwrap_or(value);
            value
                .strip_prefix('"')?
                .strip_suffix('"')
                .map(str::to_owned)
        })
        .collect();
    let (media_type, content, hash): (String, Option<Vec<u8>>, Vec<u8>) = read!(
        app,
        |connection| {
            let access = Access::open(connection, &bearer, now).await?;
            let scope = access.workspace(connection, workspace_id).await?;
            if !access.allows(Request::ViewWorkspace(&scope)) {
                return Err(ApiError::not_found());
            }
            sqlx::query_as(
            "select l.media_type, case when encode(l.hash,'hex')=any($2) then null else l.content end, l.hash
             from workspace_logos l join workspaces w on w.id=l.workspace_id
             where l.workspace_id=$1 and w.deleted_at is null",
        )
        .bind(workspace_id)
        .bind(&hashes)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or_else(ApiError::not_found)
        }
    )?;
    let header =
        |value: &str| HeaderValue::from_str(value).map_err(|error| ApiError::internal(&error));
    let headers = [
        (CONTENT_TYPE, header(&media_type)?),
        (ETAG, header(&format!("\"{}\"", hex(&hash)))?),
        (CACHE_CONTROL, HeaderValue::from_static("private, no-cache")),
        (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
    ];
    Ok(match content {
        Some(content) => (headers, content).into_response(),
        None => (StatusCode::NOT_MODIFIED, headers).into_response(),
    })
}

/// Tells the members of the workspace and the audit log of a changed logo.
async fn changed(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    workspace_id: WorkspaceId,
    logo: Option<Avatar>,
) -> ApiResult<()> {
    audit::record(
        connection,
        now,
        Some(access.account_id),
        About::Server,
        &AuditEvent::WorkspaceLogoChanged { workspace_id, logo },
    )
    .await?;
    super::workspaces::publish_update(connection, now, workspace_id).await?;
    Ok(())
}

/// The workspace exists and is not deleted.
async fn require_workspace(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
) -> ApiResult<()> {
    let found: Option<(WorkspaceId,)> =
        sqlx::query_as("select id from workspaces where id=$1 and deleted_at is null")
            .bind(workspace_id)
            .fetch_optional(&mut *connection)
            .await?;
    found.map(|_| ()).ok_or_else(ApiError::not_found)
}

/// Sets the logo of a workspace. The caller is a server administrator with a session.
#[utoipa::path(put, path = "/api/v1/workspaces/{workspace_id}/logo", tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path)), security(("session" = [])),
    request_body(content((inline(ImageBytes) = "image/png"), (inline(ImageBytes) = "image/jpeg"), (inline(ImageBytes) = "image/webp"))),
    responses((status = 200, body = Avatar), ApiError))]
pub async fn set_logo(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    body: Bytes,
) -> ApiResult<Json<Avatar>> {
    let now = app.now();
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        require_workspace(connection, workspace_id).await?;
        if body.len() > app.config.limits.avatar_bytes {
            return Err(ApiError::new(
                ErrorCode::ImageTooLarge,
                "the logo exceeds the avatar byte limit",
            ));
        }
        let longest = app.config.limits.avatar_longest_side;
        let image = image::inspect(&body)
            .filter(|image| image.width <= longest && image.height <= longest)
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::ImageUnsupported,
                    "the logo must be a still PNG, JPEG or WebP within the avatar dimensions",
                )
            })?;
        limit_change(&app, &access, &mut counted)?;
        let hash: [u8; 32] = Sha256::digest(&body).into();
        let logo = Avatar {
            hash: hex(&hash),
            media_type: image.kind,
            bytes: u32::try_from(body.len()).map_err(|error| ApiError::internal(&error))?,
            width: image.width,
            height: image.height,
        };
        let width = i32::try_from(image.width).map_err(|error| ApiError::internal(&error))?;
        let height = i32::try_from(image.height).map_err(|error| ApiError::internal(&error))?;
        let changed_rows = sqlx::query(
            "insert into workspace_logos(workspace_id, media_type, content, hash, width, height)
            values($1, $2, $3, $4, $5, $6) on conflict(workspace_id) do update set
            media_type=excluded.media_type, content=excluded.content, hash=excluded.hash,
            width=excluded.width, height=excluded.height where workspace_logos.hash <> excluded.hash",
        )
        .bind(workspace_id)
        .bind(image.kind.media_type())
        .bind(body.as_ref())
        .bind(hash.as_slice())
        .bind(width)
        .bind(height)
        .execute(&mut *connection)
        .await?
        .rows_affected();
        if changed_rows != 0 {
            changed(connection, &access, now, workspace_id, Some(logo.clone())).await?;
        }
        Ok(logo)
    })
    .map(Json)
}

/// Removes the logo of a workspace. An absent logo is unchanged.
#[utoipa::path(delete, path = "/api/v1/workspaces/{workspace_id}/logo", tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path)), security(("session" = [])),
    responses((status = 204), ApiError))]
pub async fn remove_logo(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        require_workspace(connection, workspace_id).await?;
        limit_change(&app, &access, &mut counted)?;
        let removed = sqlx::query("delete from workspace_logos where workspace_id=$1")
            .bind(workspace_id)
            .execute(&mut *connection)
            .await?
            .rows_affected();
        if removed != 0 {
            changed(connection, &access, now, workspace_id, None).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}
