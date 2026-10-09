use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::Request;
use junto_protocol::account::{
    ApproveDeviceLink, Device, DeviceLinkCreated, DeviceOrigin, LookUpDeviceLink, OwnAccount,
    PendingDeviceLink, RecoveryNotice, RenameDevice, RequestDeviceLink,
};
use junto_protocol::auth::ChallengeOperation;
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{DeviceId, WorkspaceId};
use junto_protocol::secret::LinkCode;
use sqlx::AssertSqlSafe;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::access::Access;
use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath, Bearer, ClientAddress};
use crate::proof;
use crate::secrets::{CodePurpose, LINK_CODE_SYMBOLS, hash_code, random_code};
use crate::state::AppState;
use crate::store::{self, DEVICE_COLUMNS, DeviceRow};
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
        let name: String = sqlx::query_scalar("select name from accounts where id = $1")
            .bind(access.account_id)
            .fetch_one(&mut *connection)
            .await?;
        let workspace_ids: Vec<WorkspaceId> = sqlx::query_scalar(
            "select workspace_id from workspace_members
             where account_id = $1
             order by workspace_id",
        )
        .bind(access.account_id)
        .fetch_all(&mut *connection)
        .await?;
        Ok(OwnAccount {
            id: access.account_id,
            name,
            is_owner: access.is_owner(),
            server_permissions: db::server_list(access.actor().server_permissions()),
            server_role_ids: access.server_role_ids,
            workspace_ids,
        })
    })
    .map(Json)
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
        sqlx::query("delete from devices where id = $1")
            .bind(device_id)
            .execute(&mut *connection)
            .await?;
        Ok(())
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
