use axum::http::{
    HeaderMap, HeaderValue, StatusCode,
    header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH, X_CONTENT_TYPE_OPTIONS},
};
use axum::response::{IntoResponse, Response};
use axum::{Json, body::Bytes, extract::State};
use junto_authz::{Request, ServerPermission};
use junto_protocol::{account::Avatar, audit::AuditEvent, error::ErrorCode, events::Event};
use sha2::{Digest, Sha256};

use super::account::{ImageBytes, limit_change};
use crate::{
    access::Access,
    audit::{self, About},
    error::{ApiError, ApiResult},
    events,
    extract::{Bearer, ClientAddress},
    image, read,
    state::AppState,
    transact,
};

/// The public image advertised by the server information route.
#[utoipa::path(get, path = "/api/v1/server/logo", tag = "server",
    responses((status = 304, description = "The current image matches If-None-Match"), (status = 200, content((inline(ImageBytes) = "image/png"), (inline(ImageBytes) = "image/jpeg"), (inline(ImageBytes) = "image/webp"))), ApiError))]
pub async fn get_logo(
    State(app): State<AppState>,
    ClientAddress(address): ClientAddress,
    headers: HeaderMap,
) -> ApiResult<Response> {
    crate::proof::limit_address(&app.logo_reads, address, app.now())?;
    let conditions: Vec<&str> = headers
        .get_all(IF_NONE_MATCH)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .collect();
    let any = conditions.contains(&"*");
    let hashes: Vec<String> = conditions
        .into_iter()
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
            sqlx::query_as("select media_type, case when $1 or encode(hash,'hex')=any($2) then null else content end, hash from server_logo where singleton")
            .bind(any).bind(&hashes)
            .fetch_optional(connection)
            .await?
            .ok_or_else(ApiError::not_found)
        }
    )?;
    let hash: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    let header =
        |value: &str| HeaderValue::from_str(value).map_err(|error| ApiError::internal(&error));
    let headers = [
        (CONTENT_TYPE, header(&media_type)?),
        (ETAG, header(&format!("\"{hash}\""))?),
        (CACHE_CONTROL, HeaderValue::from_static("public, no-cache")),
        (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
    ];
    Ok(match content {
        Some(content) => (headers, content).into_response(),
        None => (StatusCode::NOT_MODIFIED, headers).into_response(),
    })
}

/// Sets the public logo. The caller is a server administrator with a session.
#[utoipa::path(put, path = "/api/v1/server/logo", tag = "server", security(("session" = [])),
    request_body(content((inline(ImageBytes) = "image/png"), (inline(ImageBytes) = "image/jpeg"), (inline(ImageBytes) = "image/webp"))),
    responses((status = 200, body = Avatar), ApiError))]
pub async fn set_logo(
    State(app): State<AppState>,
    bearer: Bearer,
    body: Bytes,
) -> ApiResult<Json<Avatar>> {
    let now = app.now();
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::Administrator))?;
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
            hash: hash.iter().map(|byte| format!("{byte:02x}")).collect(),
            media_type: image.kind,
            bytes: u32::try_from(body.len()).map_err(|error| ApiError::internal(&error))?,
            width: image.width,
            height: image.height,
        };
        let width = i32::try_from(image.width).map_err(|error| ApiError::internal(&error))?;
        let height = i32::try_from(image.height).map_err(|error| ApiError::internal(&error))?;
        let changed = sqlx::query(
            "insert into server_logo(singleton, media_type, content, hash, width, height)
            values(true, $1, $2, $3, $4, $5) on conflict(singleton) do update set
            media_type=excluded.media_type, content=excluded.content, hash=excluded.hash,
            width=excluded.width, height=excluded.height where server_logo.hash <> excluded.hash",
        )
        .bind(image.kind.media_type())
        .bind(body.as_ref())
        .bind(hash.as_slice())
        .bind(width)
        .bind(height)
        .execute(&mut *connection)
        .await?
        .rows_affected()
            != 0;
        if changed {
            sqlx::query("update server_state set logo_path='/api/v1/server/logo'")
                .execute(&mut *connection)
                .await?;
            audit::record(
                connection,
                now,
                Some(access.account_id),
                About::Server,
                &AuditEvent::ServerLogoChanged {
                    logo: Some(logo.clone()),
                },
            )
            .await?;
            events::publish(
                connection,
                now,
                &Event::ServerLogoChanged {
                    logo: Some(logo.clone()),
                },
            )
            .await?;
        }
        Ok(logo)
    })
    .map(Json)
}

/// Removes the public logo. An absent logo is unchanged.
#[utoipa::path(delete, path = "/api/v1/server/logo", tag = "server", security(("session" = [])), responses((status = 204), ApiError))]
pub async fn remove_logo(State(app): State<AppState>, bearer: Bearer) -> ApiResult<StatusCode> {
    let now = app.now();
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        limit_change(&app, &access, &mut counted)?;
        if sqlx::query("delete from server_logo where singleton")
            .execute(&mut *connection)
            .await?
            .rows_affected()
            != 0
        {
            sqlx::query("update server_state set logo_path=null")
                .execute(&mut *connection)
                .await?;
            audit::record(
                connection,
                now,
                Some(access.account_id),
                About::Server,
                &AuditEvent::ServerLogoChanged { logo: None },
            )
            .await?;
            events::publish(connection, now, &Event::ServerLogoChanged { logo: None }).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}
