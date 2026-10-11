use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::{Request, ServerPermission, ServerPermissions};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, ServerRoleId};
use junto_protocol::roles::{CreateServerRole, ServerRole, UpdateServerRole};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::Access;
use crate::audit::{self, About};
use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::events::log::Affected;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::state::AppState;
use crate::{read, transact};
use crate::{store, validate};

async fn locked_role(
    connection: &mut PgConnection,
    role_id: ServerRoleId,
) -> ApiResult<(String, ServerPermissions)> {
    let row: Option<(String, i64)> =
        sqlx::query_as("select name, permissions from server_roles where id = $1 for update")
            .bind(role_id)
            .fetch_optional(connection)
            .await?;
    let (name, bits) = row.ok_or_else(ApiError::not_found)?;
    Ok((name, db::server_permissions(bits)?))
}

/// The server roles.
#[utoipa::path(
    get,
    path = "/api/v1/server-roles",
    tag = "server roles",
    security(("session" = [])),
    responses((status = 200, body = [ServerRole]), ApiError)
)]
pub async fn list_server_roles(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<ServerRole>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        let rows: Vec<(ServerRoleId, String, i64)> =
            sqlx::query_as("select id, name, permissions from server_roles order by id")
                .fetch_all(&mut *connection)
                .await?;
        rows.into_iter()
            .map(|(id, name, bits)| {
                Ok(ServerRole {
                    id,
                    name,
                    permissions: db::server_list(db::server_permissions(bits)?),
                })
            })
            .collect()
    })
    .map(Json)
}

/// Creates a server role.
#[utoipa::path(
    post,
    path = "/api/v1/server-roles",
    tag = "server roles",
    request_body = CreateServerRole,
    security(("session" = [])),
    responses((status = 201, body = ServerRole), ApiError)
)]
pub async fn create_server_role(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<CreateServerRole>,
) -> ApiResult<(StatusCode, Json<ServerRole>)> {
    let permissions: ServerPermissions = body.permissions.iter().copied().collect();
    let role = ServerRole {
        id: ServerRoleId::generate(),
        name: validate::name("name", &body.name)?,
        permissions: db::server_list(permissions),
    };
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        create(connection, &access, now, &role).await
    })?;
    Ok((StatusCode::CREATED, Json(role)))
}

pub(crate) async fn create(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    role: &ServerRole,
) -> ApiResult<()> {
    let permissions: ServerPermissions = role.permissions.iter().copied().collect();
    access.require(Request::ManageServerRoles {
        granting: permissions,
    })?;
    sqlx::query(
        "insert into server_roles (id, name, permissions, created_at)
         values ($1, $2, $3, $4)",
    )
    .bind(role.id)
    .bind(&role.name)
    .bind(db::server_bits(permissions))
    .bind(now)
    .execute(&mut *connection)
    .await?;
    audit::record(
        connection,
        now,
        Some(access.account_id),
        About::Server,
        &AuditEvent::ServerRoleCreated {
            role_id: role.id,
            name: role.name.clone(),
            permissions: role.permissions.clone(),
        },
    )
    .await?;
    let created = Event::ServerRoleCreated { role: role.clone() };
    events::publish(connection, now, &created).await
}

/// Changes the name or the permissions of a server role.
#[utoipa::path(
    patch,
    path = "/api/v1/server-roles/{role_id}",
    tag = "server roles",
    params(("role_id" = ServerRoleId, Path)),
    request_body = UpdateServerRole,
    security(("session" = [])),
    responses((status = 200, body = ServerRole), ApiError)
)]
pub async fn update_server_role(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(role_id): ApiPath<ServerRoleId>,
    ApiJson(body): ApiJson<UpdateServerRole>,
) -> ApiResult<Json<ServerRole>> {
    let new_name = body
        .name
        .as_deref()
        .map(|name| validate::name("name", name))
        .transpose()?;
    let new_permissions: Option<ServerPermissions> = body
        .permissions
        .as_ref()
        .map(|permissions| permissions.iter().copied().collect());
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        update(
            connection,
            &access,
            now,
            role_id,
            new_name.clone(),
            new_permissions,
        )
        .await
    })
    .map(Json)
}

pub(crate) async fn update(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    role_id: ServerRoleId,
    new_name: Option<String>,
    new_permissions: Option<ServerPermissions>,
) -> ApiResult<ServerRole> {
    access.require(Request::Server(ServerPermission::Administrator))?;
    let (old_name, old_permissions) = locked_role(connection, role_id).await?;
    let permissions = new_permissions.unwrap_or(old_permissions);
    access.require(Request::ManageServerRoles {
        granting: permissions.difference(old_permissions),
    })?;

    let role = ServerRole {
        id: role_id,
        name: new_name.unwrap_or(old_name),
        permissions: db::server_list(permissions),
    };
    sqlx::query("update server_roles set name = $2, permissions = $3 where id = $1")
        .bind(role_id)
        .bind(&role.name)
        .bind(db::server_bits(permissions))
        .execute(&mut *connection)
        .await?;
    audit::record(
        connection,
        now,
        Some(access.account_id),
        About::Server,
        &AuditEvent::ServerRoleUpdated {
            role_id,
            name: role.name.clone(),
            permissions: role.permissions.clone(),
        },
    )
    .await?;
    let updated = Event::ServerRoleUpdated { role: role.clone() };
    events::publish(connection, now, &updated).await?;
    if permissions != old_permissions {
        let holders = Affected::ServerRoleHolders(role_id);
        events::permissions_changed(connection, now, holders, None).await?;
    }
    Ok(role)
}

/// Records a change of the server roles of an account: for the people who list accounts, and
/// for the account, whose permissions changed.
async fn announce_roles(
    connection: &mut PgConnection,
    now: time::OffsetDateTime,
    account_id: AccountId,
) -> ApiResult<()> {
    let changed = Event::AccountServerRolesChanged {
        account_id,
        server_role_ids: store::server_role_ids_of(connection, account_id).await?,
    };
    events::publish(connection, now, &changed).await?;
    events::permissions_changed(connection, now, Affected::Account(account_id), None).await
}

pub async fn require_active_account(
    connection: &mut PgConnection,
    account_id: AccountId,
) -> ApiResult<()> {
    let active: Option<bool> =
        sqlx::query_scalar("select status = 'active' from accounts where id = $1 for share")
            .bind(account_id)
            .fetch_optional(connection)
            .await?;
    match active {
        None => Err(ApiError::not_found()),
        Some(false) => Err(ApiError::conflict("the account is not active")),
        Some(true) => Ok(()),
    }
}

/// Gives a server role to an account.
#[utoipa::path(
    put,
    path = "/api/v1/accounts/{account_id}/server-roles/{role_id}",
    tag = "server roles",
    params(("account_id" = AccountId, Path), ("role_id" = ServerRoleId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn assign_server_role(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((account_id, role_id)): ApiPath<(AccountId, ServerRoleId)>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        let (_, permissions) = locked_role(connection, role_id).await?;
        access.require(Request::ManageServerRoles {
            granting: permissions,
        })?;
        require_active_account(connection, account_id).await?;

        let inserted = sqlx::query(
            "insert into account_server_roles (account_id, role_id) values ($1, $2)
             on conflict do nothing",
        )
        .bind(account_id)
        .bind(role_id)
        .execute(&mut *connection)
        .await?;
        if inserted.rows_affected() > 0 {
            let held = store::server_role_ids_of(connection, account_id)
                .await?
                .len();
            if held > app.config.limits.most_server_roles_per_account as usize {
                return Err(ApiError::limit_reached(
                    "the account holds the largest number of server roles",
                ));
            }
            audit::record(
                connection,
                now,
                Some(access.account_id),
                About::Server,
                &AuditEvent::ServerRoleAssigned {
                    role_id,
                    account_id,
                },
            )
            .await?;
            announce_roles(connection, now, account_id).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Takes a server role from an account.
#[utoipa::path(
    delete,
    path = "/api/v1/accounts/{account_id}/server-roles/{role_id}",
    tag = "server roles",
    params(("account_id" = AccountId, Path), ("role_id" = ServerRoleId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn unassign_server_role(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((account_id, role_id)): ApiPath<(AccountId, ServerRoleId)>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::ManageServerRoles {
            granting: ServerPermissions::EMPTY,
        })?;
        let deleted =
            sqlx::query("delete from account_server_roles where account_id = $1 and role_id = $2")
                .bind(account_id)
                .bind(role_id)
                .execute(&mut *connection)
                .await?;
        if deleted.rows_affected() > 0 {
            audit::record(
                connection,
                now,
                Some(access.account_id),
                About::Server,
                &AuditEvent::ServerRoleUnassigned {
                    role_id,
                    account_id,
                },
            )
            .await?;
            announce_roles(connection, now, account_id).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}
