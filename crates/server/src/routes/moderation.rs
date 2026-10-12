use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::{OwnerAction, Request, ServerPermission};
use junto_protocol::account::{
    AccountStatus, AccountSummary, DeviceOrigin, RecoverAccount, RecoveryNotice,
};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, InviteId, ServerRoleId, WorkspaceId};
use junto_protocol::invite::InviteRevocation;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::Access;
use crate::audit::{self, About};
use crate::db::StoredText;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::state::AppState;
use crate::store::{SessionEnd, Sessions};
use crate::{read, transact};
use crate::{store, validate};

type AccountRow = (
    AccountId,
    String,
    String,
    bool,
    Vec<ServerRoleId>,
    OffsetDateTime,
);

/// Every account of the server, with its status and server roles.
#[utoipa::path(
    get,
    path = "/api/v1/accounts",
    tag = "moderation",
    security(("session" = [])),
    responses((status = 200, body = [AccountSummary]), ApiError)
)]
pub async fn list_accounts(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<AccountSummary>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::BanMembers))?;
        let rows: Vec<AccountRow> = sqlx::query_as(
            "select a.id, a.name, a.status,
                    a.id is not distinct from (select owner_account_id from server_state),
                    array(select r.role_id from account_server_roles r
                          where r.account_id = a.id order by r.role_id),
                    a.created_at
             from accounts a
             order by a.id",
        )
        .fetch_all(&mut *connection)
        .await?;
        rows.into_iter()
            .map(
                |(id, name, status, is_owner, server_role_ids, created_at)| {
                    Ok(AccountSummary {
                        id,
                        name,
                        status: AccountStatus::read(&status)?,
                        is_owner,
                        server_role_ids,
                        created_at,
                    })
                },
            )
            .collect()
    })
    .map(Json)
}

pub async fn locked_status(
    connection: &mut PgConnection,
    account_id: AccountId,
) -> ApiResult<AccountStatus> {
    let status: Option<String> =
        sqlx::query_scalar("select status from accounts where id = $1 for update")
            .bind(account_id)
            .fetch_optional(connection)
            .await?;
    AccountStatus::read(&status.ok_or_else(ApiError::not_found)?)
}

async fn set_status(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    status: AccountStatus,
) -> ApiResult<()> {
    sqlx::query("update accounts set status = $2 where id = $1")
        .bind(account_id)
        .bind(status.stored())
        .execute(&mut *connection)
        .await?;
    let changed = Event::AccountStatusChanged { account_id, status };
    events::publish(connection, now, &changed).await
}

/// Removes an account from the server. Its devices, sessions, memberships and roles end.
#[utoipa::path(
    post,
    path = "/api/v1/accounts/{account_id}/removal",
    tag = "moderation",
    params(("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn remove_account(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Moderate {
            target: access.target(account_id),
        })?;
        if locked_status(connection, account_id).await? == AccountStatus::Removed {
            return Err(ApiError::conflict("the account is already removed"));
        }

        set_status(connection, now, account_id, AccountStatus::Removed).await?;
        let sessions = Sessions::OfAccount(account_id);
        store::record_ended(connection, &app.calls, now, sessions, SessionEnd::Removed).await?;
        sqlx::query("delete from devices where account_id = $1")
            .bind(account_id)
            .execute(&mut *connection)
            .await?;
        crate::tokens::end_all_of(connection, now, account_id).await?;
        crate::exports::account_ended(connection, now, account_id).await?;
        let left: Vec<WorkspaceId> = sqlx::query_scalar(
            "delete from workspace_members where account_id = $1 returning workspace_id",
        )
        .bind(account_id)
        .fetch_all(&mut *connection)
        .await?;
        for workspace_id in left {
            let removed = Event::WorkspaceMemberRemoved {
                workspace_id,
                account_id,
            };
            events::publish(connection, now, &removed).await?;
        }
        let roles = sqlx::query("delete from account_server_roles where account_id = $1")
            .bind(account_id)
            .execute(&mut *connection)
            .await?;
        if roles.rows_affected() > 0 {
            let changed = Event::AccountServerRolesChanged {
                account_id,
                server_role_ids: Vec::new(),
            };
            events::publish(connection, now, &changed).await?;
        }
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::AccountRemoved { account_id },
        )
        .await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Bans an account. Its device keys cannot log in and the invites it created are cancelled.
#[utoipa::path(
    put,
    path = "/api/v1/accounts/{account_id}/ban",
    tag = "moderation",
    params(("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn ban_account(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Moderate {
            target: access.target(account_id),
        })?;
        match locked_status(connection, account_id).await? {
            AccountStatus::Active => {}
            AccountStatus::Banned => {
                return Err(ApiError::conflict("the account is already banned"));
            }
            AccountStatus::Removed => return Err(ApiError::conflict("the account is removed")),
        }

        set_status(connection, now, account_id, AccountStatus::Banned).await?;
        store::end_sessions(connection, &app.calls, now, account_id, SessionEnd::Banned).await?;
        crate::exports::account_ended(connection, now, account_id).await?;
        let cancelled_invite_ids: Vec<InviteId> = sqlx::query_scalar(
            "update invites set revoked_at = $2, revocation = 'creator_banned'
             where created_by = $1 and revoked_at is null
             returning id",
        )
        .bind(account_id)
        .bind(now)
        .fetch_all(&mut *connection)
        .await?;
        if !cancelled_invite_ids.is_empty() {
            let revoked = Event::InvitesOfCreatorRevoked {
                created_by: account_id,
                revocation: InviteRevocation::CreatorBanned,
            };
            events::publish(connection, now, &revoked).await?;
        }
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::AccountBanned {
                account_id,
                cancelled_invite_ids,
            },
        )
        .await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Lifts the ban of an account. The invites that the ban cancelled stay cancelled.
#[utoipa::path(
    delete,
    path = "/api/v1/accounts/{account_id}/ban",
    tag = "moderation",
    params(("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn unban_account(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Moderate {
            target: access.target(account_id),
        })?;
        if locked_status(connection, account_id).await? != AccountStatus::Banned {
            return Err(ApiError::conflict("the account is not banned"));
        }
        set_status(connection, now, account_id, AccountStatus::Active).await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::AccountUnbanned { account_id },
        )
        .await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Gives a new device key access to an existing account. Only the server owner can do this.
#[utoipa::path(
    post,
    path = "/api/v1/accounts/{account_id}/recovery",
    tag = "moderation",
    params(("account_id" = AccountId, Path)),
    request_body = RecoverAccount,
    security(("session" = [])),
    responses((status = 201, body = RecoveryNotice), ApiError)
)]
pub async fn recover_account(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
    ApiJson(body): ApiJson<RecoverAccount>,
) -> ApiResult<(StatusCode, Json<RecoveryNotice>)> {
    let device_name = validate::optional_name("device_name", &body.device_name)?;
    let now = app.now();
    let notice = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnerOnly(OwnerAction::RecoverAccount))?;
        if locked_status(connection, account_id).await? != AccountStatus::Active {
            return Err(ApiError::conflict("the account is not active"));
        }

        let notice = store::recover(
            connection,
            &app.calls,
            now,
            store::Recovery {
                account_id,
                public_key: &body.public_key,
                device_name: &device_name,
                origin: DeviceOrigin::Recovery,
                performed_by: Some(access.account_id),
            },
        )
        .await?;
        let device_id = notice.device_id.ok_or_else(ApiError::not_found)?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::AccountRecovered {
                account_id,
                device_id,
            },
        )
        .await?;
        Ok(notice)
    })?;
    Ok((StatusCode::CREATED, Json(notice)))
}
