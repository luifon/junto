use junto_protocol::account::RecoveryNotice;
use junto_protocol::account::{Device, DeviceOrigin};
use junto_protocol::auth::SessionCreated;
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{AccountId, DeviceId, RecoveryId, WorkspaceId};
use junto_protocol::keys::PublicKey;
use junto_protocol::secret::SessionToken;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::db::{self, StoredText};
use crate::error::{ApiError, ApiResult, violates};
use crate::secrets::{hash_session_token, new_session_token};
use crate::state::AppState;

pub async fn default_workspace(connection: &mut PgConnection) -> ApiResult<WorkspaceId> {
    Ok(
        sqlx::query_scalar("select default_workspace_id from server_state")
            .fetch_one(connection)
            .await?,
    )
}

/// The workspaces that an invite or an approval grants: the named ones without repeats, or the
/// default workspace when none is named.
pub async fn granted_workspaces(
    connection: &mut PgConnection,
    named: &[WorkspaceId],
) -> ApiResult<Vec<WorkspaceId>> {
    if named.is_empty() {
        return Ok(vec![default_workspace(connection).await?]);
    }
    let mut granted = named.to_vec();
    granted.sort();
    granted.dedup();
    Ok(granted)
}

pub async fn require_workspaces_exist(
    connection: &mut PgConnection,
    workspace_ids: &[WorkspaceId],
) -> ApiResult<()> {
    let existing: i64 = sqlx::query_scalar("select count(*) from workspaces where id = any($1)")
        .bind(workspace_ids)
        .fetch_one(connection)
        .await?;
    if usize::try_from(existing) == Ok(workspace_ids.len()) {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

pub async fn create_account(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    name: &str,
) -> ApiResult<AccountId> {
    let id = AccountId::generate();
    sqlx::query(
        "insert into accounts (id, name, status, created_at) values ($1, $2, 'active', $3)",
    )
    .bind(id)
    .bind(name)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(id)
}

pub async fn create_device(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    public_key: &PublicKey,
    name: &str,
    origin: DeviceOrigin,
) -> ApiResult<Device> {
    let id = DeviceId::generate();
    let inserted = sqlx::query(
        "insert into devices (id, account_id, public_key, name, origin, created_at)
         values ($1, $2, $3, $4, $5, $6)",
    )
    .bind(id)
    .bind(account_id)
    .bind(public_key.as_bytes().as_slice())
    .bind(name)
    .bind(origin.stored())
    .bind(now)
    .execute(connection)
    .await;
    match inserted {
        Ok(_) => Ok(Device {
            id,
            name: name.to_owned(),
            public_key: *public_key,
            origin,
            created_at: now,
            last_login_at: None,
            current: false,
        }),
        Err(error) if violates(&error, "devices_public_key_unique") => {
            Err(key_already_registered())
        }
        Err(error) => Err(error.into()),
    }
}

pub fn key_already_registered() -> ApiError {
    ApiError::new(
        ErrorCode::KeyAlreadyRegistered,
        "the device key already belongs to an account",
    )
}

pub async fn require_key_unregistered(
    connection: &mut PgConnection,
    public_key: &PublicKey,
) -> ApiResult<()> {
    let registered: bool =
        sqlx::query_scalar("select exists (select 1 from devices where public_key = $1)")
            .bind(public_key.as_bytes().as_slice())
            .fetch_one(connection)
            .await?;
    if registered {
        Err(key_already_registered())
    } else {
        Ok(())
    }
}

pub async fn add_members(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    workspace_ids: &[WorkspaceId],
) -> ApiResult<()> {
    sqlx::query(
        "insert into workspace_members (workspace_id, account_id, joined_at)
         select workspace_id, $1, $2 from unnest($3::uuid[]) as granted (workspace_id)",
    )
    .bind(account_id)
    .bind(now)
    .bind(workspace_ids)
    .execute(connection)
    .await?;
    Ok(())
}

/// Starts a session of a device and records the login. The response carries the recoveries of
/// the account that this device has not been told about, and each is told once.
pub async fn start_session(
    app: &AppState,
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    device_id: DeviceId,
) -> ApiResult<SessionCreated> {
    let recoveries = recovery_rows(
        sqlx::query_as(
            "with told as (
                 delete from recovery_notices where device_id = $1 returning recovery_id)
             select r.id, r.device_id, r.performed_by, r.performed_at
             from recoveries r
             join told on told.recovery_id = r.id
             order by r.performed_at, r.id",
        )
        .bind(device_id)
        .fetch_all(&mut *connection)
        .await?,
    );

    sqlx::query("update devices set last_login_at = $2 where id = $1")
        .bind(device_id)
        .bind(now)
        .execute(&mut *connection)
        .await?;

    let token = new_session_token()?;
    let expires_at = now + app.config.limits.session_lifetime;
    sqlx::query(
        "insert into sessions (id, device_id, token_hash, created_at, expires_at)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(Uuid::now_v7())
    .bind(device_id)
    .bind(hash_session_token(&token).as_slice())
    .bind(now)
    .bind(expires_at)
    .execute(&mut *connection)
    .await?;

    Ok(SessionCreated {
        token: SessionToken::new(token),
        expires_at,
        account_id,
        device_id,
        recoveries,
    })
}

type RecoveryRow = (
    RecoveryId,
    Option<DeviceId>,
    Option<AccountId>,
    OffsetDateTime,
);

fn recovery_rows(rows: Vec<RecoveryRow>) -> Vec<RecoveryNotice> {
    rows.into_iter()
        .map(
            |(id, device_id, performed_by, performed_at)| RecoveryNotice {
                id,
                device_id,
                performed_by,
                performed_at,
            },
        )
        .collect()
}

pub async fn recoveries_of(
    connection: &mut PgConnection,
    account_id: AccountId,
) -> ApiResult<Vec<RecoveryNotice>> {
    Ok(recovery_rows(
        sqlx::query_as(
            "select id, device_id, performed_by, performed_at
             from recoveries
             where account_id = $1
             order by performed_at, id",
        )
        .bind(account_id)
        .fetch_all(connection)
        .await?,
    ))
}

pub type DeviceRow = (
    DeviceId,
    String,
    Vec<u8>,
    String,
    OffsetDateTime,
    Option<OffsetDateTime>,
);

pub const DEVICE_COLUMNS: &str = "id, name, public_key, origin, created_at, last_login_at";

pub fn device(row: DeviceRow, current: DeviceId) -> ApiResult<Device> {
    let (id, name, public_key, origin, created_at, last_login_at) = row;
    Ok(Device {
        id,
        name,
        public_key: db::public_key(&public_key)?,
        origin: DeviceOrigin::read(&origin)?,
        created_at,
        last_login_at,
        current: id == current,
    })
}

pub async fn end_sessions(connection: &mut PgConnection, account_id: AccountId) -> ApiResult<()> {
    sqlx::query(
        "delete from sessions where device_id in (select id from devices where account_id = $1)",
    )
    .bind(account_id)
    .execute(connection)
    .await?;
    Ok(())
}

/// Gives `public_key` access to an account as a new device: ends every session of the
/// account, records the recovery, and leaves one notice for each device of the account.
pub async fn recover(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    public_key: &PublicKey,
    device_name: &str,
    origin: DeviceOrigin,
    performed_by: Option<AccountId>,
) -> ApiResult<RecoveryNotice> {
    end_sessions(&mut *connection, account_id).await?;
    let device = create_device(
        &mut *connection,
        now,
        account_id,
        public_key,
        device_name,
        origin,
    )
    .await?;
    let notice = RecoveryNotice {
        id: RecoveryId::generate(),
        device_id: Some(device.id),
        performed_by,
        performed_at: now,
    };
    sqlx::query(
        "insert into recoveries (id, account_id, device_id, performed_by, performed_at)
         values ($1, $2, $3, $4, $5)",
    )
    .bind(notice.id)
    .bind(account_id)
    .bind(device.id)
    .bind(notice.performed_by)
    .bind(notice.performed_at)
    .execute(&mut *connection)
    .await?;
    sqlx::query(
        "insert into recovery_notices (recovery_id, device_id)
         select $1, id from devices where account_id = $2",
    )
    .bind(notice.id)
    .bind(account_id)
    .execute(&mut *connection)
    .await?;
    Ok(notice)
}
