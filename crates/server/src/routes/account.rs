use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, X_CONTENT_TYPE_OPTIONS};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use junto_authz::Request;
use junto_protocol::account::{
    AccountProfile, ApproveDeviceLink, Avatar, Device, DeviceLinkCreated, DeviceOrigin,
    LookUpDeviceLink, OwnAccount, PendingDeviceLink, RecoveryNotice, RenameDevice,
    RequestDeviceLink, UpdateProfile,
};
use junto_protocol::auth::ChallengeOperation;
use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, DeviceId, RecoveryId};
use junto_protocol::secret::LinkCode;
use sha2::{Digest, Sha256};
use sqlx::AssertSqlSafe;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::access::Access;
use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::extract::{ApiJson, ApiPath, Bearer, ClientAddress};
use crate::image;
use crate::proof;
use crate::secrets::{CodePurpose, LINK_CODE_SYMBOLS, hash_code, random_code};
use crate::state::AppState;
use crate::store::{self, DEVICE_COLUMNS, DeviceRow, SessionEnd, Sessions};
use crate::validate;
use crate::{read, transact};

/// The caller's account, its server roles and its workspaces.
#[utoipa::path(
    get,
    path = "/api/v1/account",
    tag = "account",
    security(("session" = [])),
    responses((status = 200, body = OwnAccount), ApiError)
)]
pub async fn get_own_account(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<OwnAccount>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        store::own_account(connection, &access).await
    })
    .map(Json)
}

/// The bytes of an image as the whole body of a request or of a response.
pub struct ImageBytes;

impl utoipa::PartialSchema for ImageBytes {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{KnownFormat, ObjectBuilder, SchemaFormat, Type};
        ObjectBuilder::new()
            .schema_type(Type::String)
            .format(Some(SchemaFormat::KnownFormat(KnownFormat::Binary)))
            .into()
    }
}

impl utoipa::ToSchema for ImageBytes {}

/// Changes the name that the caller's account shows.
#[utoipa::path(
    patch,
    path = "/api/v1/account",
    tag = "account",
    request_body = UpdateProfile,
    security(("session" = [])),
    responses((status = 200, body = OwnAccount), ApiError)
)]
pub async fn update_own_account(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<UpdateProfile>,
) -> ApiResult<Json<OwnAccount>> {
    let name = validate::name("name", &body.name)?;
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let changed = sqlx::query("update accounts set name = $2 where id = $1 and name <> $2")
            .bind(access.account_id)
            .bind(&name)
            .execute(&mut *connection)
            .await?;
        if changed.rows_affected() > 0 {
            store::announce_profile(connection, now, access.account_id).await?;
        }
        store::own_account(connection, &access).await
    })
    .map(Json)
}

/// Stores the avatar of the caller's account. The body is the image itself: a still PNG,
/// JPEG or WebP image of at most 262144 bytes and at most 1024 by 1024 pixels. The server
/// reads the format from the bytes, not from `Content-Type`.
#[utoipa::path(
    put,
    path = "/api/v1/account/avatar",
    tag = "account",
    request_body(content(
        (inline(ImageBytes) = "image/png"),
        (inline(ImageBytes) = "image/jpeg"),
        (inline(ImageBytes) = "image/webp")
    )),
    security(("session" = [])),
    responses((status = 200, body = Avatar), ApiError)
)]
pub async fn set_avatar(
    State(app): State<AppState>,
    bearer: Bearer,
    body: Bytes,
) -> ApiResult<Json<Avatar>> {
    let limits = &app.config.limits;
    let inspected = || {
        if body.len() > limits.avatar_bytes {
            return Err(ApiError::new(
                ErrorCode::ImageTooLarge,
                format!("an avatar has at most {} bytes", limits.avatar_bytes),
            ));
        }
        let longest = limits.avatar_longest_side;
        let stated = image::inspect(&body)
            .filter(|image| image.width <= longest && image.height <= longest)
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::ImageUnsupported,
                    format!(
                        "an avatar is a still PNG, JPEG or WebP image of at most {longest} by \
                         {longest} pixels"
                    ),
                )
            })?;
        let width = i32::try_from(stated.width).map_err(|error| ApiError::internal(&error))?;
        let height = i32::try_from(stated.height).map_err(|error| ApiError::internal(&error))?;
        let hash: [u8; 32] = Sha256::digest(&body).into();
        Ok((stated.kind, width, height, hash))
    };
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let (kind, width, height, hash) = inspected()?;
        let stored: Option<Vec<u8>> =
            sqlx::query_scalar("select hash from account_avatars where account_id = $1")
                .bind(access.account_id)
                .fetch_optional(&mut *connection)
                .await?;
        if stored.as_deref() != Some(hash.as_slice()) {
            sqlx::query(
                "insert into account_avatars
                     (account_id, media_type, content, hash, width, height, updated_at)
                 values ($1, $2, $3, $4, $5, $6, $7)
                 on conflict (account_id) do update set
                     media_type = excluded.media_type,
                     content = excluded.content,
                     hash = excluded.hash,
                     width = excluded.width,
                     height = excluded.height,
                     updated_at = excluded.updated_at",
            )
            .bind(access.account_id)
            .bind(kind.media_type())
            .bind(body.as_ref())
            .bind(hash.as_slice())
            .bind(width)
            .bind(height)
            .bind(now)
            .execute(&mut *connection)
            .await?;
            store::announce_profile(connection, now, access.account_id).await?;
        }
        store::avatar_of(connection, access.account_id)
            .await?
            .ok_or_else(ApiError::not_found)
    })
    .map(Json)
}

/// Removes the avatar of the caller's account.
#[utoipa::path(
    delete,
    path = "/api/v1/account/avatar",
    tag = "account",
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn remove_avatar(State(app): State<AppState>, bearer: Bearer) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let removed = sqlx::query("delete from account_avatars where account_id = $1")
            .bind(access.account_id)
            .execute(&mut *connection)
            .await?;
        if removed.rows_affected() > 0 {
            store::announce_profile(connection, now, access.account_id).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// The name and the avatar of an account that the caller may see: its own, an account in a
/// workspace that the caller is a member of, or any account for a caller with `ban_members`.
#[utoipa::path(
    get,
    path = "/api/v1/accounts/{account_id}/profile",
    tag = "account",
    params(("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 200, body = AccountProfile), ApiError)
)]
pub async fn get_profile(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
) -> ApiResult<Json<AccountProfile>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        require_sight(connection, &access, account_id).await?;
        store::profile_of(connection, account_id)
            .await?
            .ok_or_else(ApiError::not_found)
    })
    .map(Json)
}

/// An account that the caller may not see is answered like an account that does not exist.
async fn require_sight(
    connection: &mut sqlx::PgConnection,
    access: &Access,
    account_id: AccountId,
) -> ApiResult<()> {
    let acquaintance = store::acquaintance(connection, access, account_id).await?;
    if access.allows(Request::ViewAccount(acquaintance)) {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

/// The avatar of an account that the caller may see, as the stored image.
#[utoipa::path(
    get,
    path = "/api/v1/accounts/{account_id}/avatar",
    tag = "account",
    params(("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses(
        (status = 200, description = "The image. `ETag` carries its hash.", content(
            (inline(ImageBytes) = "image/png"),
            (inline(ImageBytes) = "image/jpeg"),
            (inline(ImageBytes) = "image/webp")
        )),
        ApiError
    )
)]
pub async fn get_avatar(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
) -> ApiResult<Response> {
    let now = app.now();
    let (media_type, content, hash): (String, Vec<u8>, Vec<u8>) = read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        require_sight(connection, &access, account_id).await?;
        let row = sqlx::query_as(
            "select media_type, content, hash from account_avatars where account_id = $1",
        )
        .bind(account_id)
        .fetch_optional(&mut *connection)
        .await?;
        row.ok_or_else(ApiError::not_found)
    })?;
    let tag: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    let header =
        |text: &str| HeaderValue::from_str(text).map_err(|error| ApiError::internal(&error));
    Ok((
        [
            (CONTENT_TYPE, header(&media_type)?),
            (ETAG, header(&format!("\"{tag}\""))?),
            (CACHE_CONTROL, HeaderValue::from_static("private, no-cache")),
            (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
        ],
        content,
    )
        .into_response())
}

/// The devices of the caller's account.
#[utoipa::path(
    get,
    path = "/api/v1/account/devices",
    tag = "account",
    security(("session" = [])),
    responses((status = 200, body = [Device]), ApiError)
)]
pub async fn list_devices(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<Device>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let rows: Vec<DeviceRow> = sqlx::query_as(AssertSqlSafe(format!(
            "select {DEVICE_COLUMNS} from devices where account_id = $1 order by created_at, id"
        )))
        .bind(access.account_id)
        .fetch_all(&mut *connection)
        .await?;
        rows.into_iter()
            .map(|row| store::device(row, access.device_id))
            .collect()
    })
    .map(Json)
}

/// Changes the name of one device of the caller's account.
#[utoipa::path(
    patch,
    path = "/api/v1/account/devices/{device_id}",
    tag = "account",
    params(("device_id" = DeviceId, Path)),
    request_body = RenameDevice,
    security(("session" = [])),
    responses((status = 200, body = Device), ApiError)
)]
pub async fn rename_device(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(device_id): ApiPath<DeviceId>,
    ApiJson(body): ApiJson<RenameDevice>,
) -> ApiResult<Json<Device>> {
    let name = validate::optional_name("name", &body.name)?;
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let row: Option<DeviceRow> = sqlx::query_as(AssertSqlSafe(format!(
            "update devices set name = $3
             where id = $1 and account_id = $2
             returning {DEVICE_COLUMNS}"
        )))
        .bind(device_id)
        .bind(access.account_id)
        .bind(&name)
        .fetch_optional(&mut *connection)
        .await?;
        let row = row.ok_or_else(ApiError::not_found)?;
        let renamed = Event::DeviceRenamed {
            device_id,
            name: name.clone(),
        };
        events::tell_account(connection, now, access.account_id, &renamed).await?;
        store::device(row, access.device_id)
    })
    .map(Json)
}

/// Removes one device of the caller's account and ends its sessions.
#[utoipa::path(
    delete,
    path = "/api/v1/account/devices/{device_id}",
    tag = "account",
    params(("device_id" = DeviceId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn remove_device(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(device_id): ApiPath<DeviceId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let device_ids: Vec<DeviceId> =
            sqlx::query_scalar("select id from devices where account_id = $1 for update")
                .bind(access.account_id)
                .fetch_all(&mut *connection)
                .await?;
        if !device_ids.contains(&device_id) {
            return Err(ApiError::not_found());
        }
        if device_ids.len() == 1 {
            return Err(ApiError::conflict(
                "the last device of an account cannot be removed",
            ));
        }
        let sessions = Sessions::OfDevice(device_id);
        store::record_ended(
            connection,
            &app.calls,
            now,
            sessions,
            SessionEnd::DeviceRemoved,
        )
        .await?;
        sqlx::query("delete from devices where id = $1")
            .bind(device_id)
            .execute(&mut *connection)
            .await?;
        let removed = Event::DeviceRemoved { device_id };
        events::tell_account(connection, now, access.account_id, &removed).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// The recoveries of the caller's account.
#[utoipa::path(
    get,
    path = "/api/v1/account/recoveries",
    tag = "account",
    security(("session" = [])),
    responses((status = 200, body = [RecoveryNotice]), ApiError)
)]
pub async fn list_recoveries(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<RecoveryNotice>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        store::recoveries_of(connection, access.account_id).await
    })
    .map(Json)
}

/// The recoveries of the account that the calling device has not acknowledged. A login does
/// not remove them.
#[utoipa::path(
    get,
    path = "/api/v1/account/recovery-notices",
    tag = "account",
    security(("session" = [])),
    responses((status = 200, body = [RecoveryNotice]), ApiError)
)]
pub async fn list_recovery_notices(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<RecoveryNotice>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        store::recovery_notices_of(connection, access.device_id).await
    })
    .map(Json)
}

/// States that the calling device has shown a recovery to its user. The notice of that
/// recovery is removed for this device only.
#[utoipa::path(
    delete,
    path = "/api/v1/account/recovery-notices/{recovery_id}",
    tag = "account",
    params(("recovery_id" = RecoveryId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn acknowledge_recovery_notice(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(recovery_id): ApiPath<RecoveryId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let acknowledged =
            sqlx::query("delete from recovery_notices where recovery_id = $1 and device_id = $2")
                .bind(recovery_id)
                .bind(access.device_id)
                .execute(&mut *connection)
                .await?;
        if acknowledged.rows_affected() == 0 {
            return Err(ApiError::not_found());
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Sent by a new device. Returns the one-time code that a device of the account approves.
#[utoipa::path(
    post,
    path = "/api/v1/device-links",
    tag = "account",
    request_body = RequestDeviceLink,
    responses((status = 201, body = DeviceLinkCreated), ApiError)
)]
pub async fn request_device_link(
    State(app): State<AppState>,
    ClientAddress(address): ClientAddress,
    ApiJson(body): ApiJson<RequestDeviceLink>,
) -> ApiResult<(StatusCode, Json<DeviceLinkCreated>)> {
    proof::limit_address(&app.join_attempts, address, app.now())?;
    let device_name = validate::optional_name("device_name", &body.device_name)?;
    let now = app.now();
    let code = random_code(LINK_CODE_SYMBOLS)?;
    let expires_at = now + app.config.limits.device_link_lifetime;

    transact!(app, |connection| {
        let proven = proof::consume(
            &app,
            connection,
            address,
            &body.proof,
            ChallengeOperation::LinkDevice,
            now,
        )
        .await?;
        store::require_key_unregistered(connection, &proven.public_key).await?;
        let waiting: i64 = sqlx::query_scalar(
            "select count(*) from device_links where expires_at > $1 and public_key <> $2",
        )
        .bind(now)
        .bind(proven.public_key.as_bytes().as_slice())
        .fetch_one(&mut *connection)
        .await?;
        if waiting >= i64::from(app.config.limits.most_pending_device_links) {
            return Err(ApiError::new(
                ErrorCode::DeviceLinksFull,
                "too many device links wait for an approval",
            ));
        }

        sqlx::query(
            "insert into device_links
                 (id, code_hash, public_key, device_name, created_at, expires_at)
             values ($1, $2, $3, $4, $5, $6)
             on conflict on constraint device_links_one_per_key do update set
                 id = excluded.id,
                 code_hash = excluded.code_hash,
                 device_name = excluded.device_name,
                 created_at = excluded.created_at,
                 expires_at = excluded.expires_at",
        )
        .bind(Uuid::now_v7())
        .bind(hash_code(CodePurpose::DeviceLink, &code).as_slice())
        .bind(proven.public_key.as_bytes().as_slice())
        .bind(&device_name)
        .bind(now)
        .bind(expires_at)
        .execute(&mut *connection)
        .await?;
        Ok(())
    })?;

    Ok((
        StatusCode::CREATED,
        Json(DeviceLinkCreated {
            code: LinkCode::new(code),
            expires_at,
        }),
    ))
}

/// Shows the device key behind a link code, so that the approving device can compare it.
#[utoipa::path(
    post,
    path = "/api/v1/account/device-links/lookup",
    tag = "account",
    request_body = LookUpDeviceLink,
    security(("session" = [])),
    responses((status = 200, body = PendingDeviceLink), ApiError)
)]
pub async fn look_up_device_link(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<LookUpDeviceLink>,
) -> ApiResult<Json<PendingDeviceLink>> {
    let now = app.now();
    let code_hash = hash_code(CodePurpose::DeviceLink, body.code.expose());
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let row: Option<(Vec<u8>, String, OffsetDateTime)> = sqlx::query_as(
            "select public_key, device_name, expires_at from device_links
             where code_hash = $1 and expires_at > $2",
        )
        .bind(code_hash.as_slice())
        .bind(now)
        .fetch_optional(&mut *connection)
        .await?;
        let (public_key, device_name, expires_at) = row.ok_or_else(link_code_invalid)?;
        Ok(PendingDeviceLink {
            public_key: db::public_key(&public_key)?,
            device_name,
            expires_at,
        })
    })
    .map(Json)
}

/// Adds the device behind a link code to the caller's account. The approval names the public
/// key of the new device.
#[utoipa::path(
    post,
    path = "/api/v1/account/device-links/approve",
    tag = "account",
    request_body = ApproveDeviceLink,
    security(("session" = [])),
    responses((status = 201, body = Device), ApiError)
)]
pub async fn approve_device_link(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<ApproveDeviceLink>,
) -> ApiResult<(StatusCode, Json<Device>)> {
    let now = app.now();
    let code_hash = hash_code(CodePurpose::DeviceLink, body.code.expose());
    let device = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let link: Option<(Vec<u8>, String)> = sqlx::query_as(
            "delete from device_links where code_hash = $1 and expires_at > $2
                 returning public_key, device_name",
        )
        .bind(code_hash.as_slice())
        .bind(now)
        .fetch_optional(&mut *connection)
        .await?;
        let (public_key, device_name) = link.ok_or_else(link_code_invalid)?;
        let public_key = db::public_key(&public_key)?;
        if public_key != body.public_key {
            return Err(ApiError::new(
                ErrorCode::PublicKeyMismatch,
                "the link code belongs to another device key",
            ));
        }
        store::create_device(
            connection,
            now,
            access.account_id,
            &public_key,
            &device_name,
            DeviceOrigin::Link,
        )
        .await
    })?;
    Ok((StatusCode::CREATED, Json(device)))
}

fn link_code_invalid() -> ApiError {
    ApiError::new(
        ErrorCode::LinkCodeInvalid,
        "the link code is unknown, used or expired",
    )
}
