use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::{Request, RoleRank, ServerPermission, ServerPermissions};
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
) -> ApiResult<(String, ServerPermissions, i32)> {
    let row: Option<(String, i64, i32)> = sqlx::query_as(
        "select name, permissions, position from server_roles where id = $1 for update",
    )
    .bind(role_id)
    .fetch_optional(connection)
    .await?;
    let (name, bits, position) = row.ok_or_else(ApiError::not_found)?;
    Ok((name, db::server_permissions(bits)?, position))
}

/// The server roles, visible to administrators and holders of ban_members.
/// Tokens may use either read_structure or manage_members for this list.
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
        access.require(Request::Server(ServerPermission::BanMembers))?;
        let rows: Vec<(ServerRoleId, String, i64, i32)> = sqlx::query_as(
            "select id, name, permissions, position from server_roles order by position",
        )
        .fetch_all(&mut *connection)
        .await?;
        rows.into_iter()
            .map(|(id, name, bits, position)| {
                Ok(ServerRole {
                    position,
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
        position: 1,
        id: ServerRoleId::generate(),
        name: validate::name("name", &body.name)?,
        permissions: db::server_list(permissions),
    };
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        create(
            connection,
            &access,
            now,
            &role,
            app.config.limits.most_server_roles,
        )
        .await
    })?;
    Ok((StatusCode::CREATED, Json(role)))
}

pub(crate) async fn create(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    role: &ServerRole,
    ceiling: u32,
) -> ApiResult<()> {
    let permissions: ServerPermissions = role.permissions.iter().copied().collect();
    access.require(Request::ManageServerRoles {
        granting: permissions,
    })?;
    let existing: i64 = sqlx::query_scalar(
        "select count(*) from (select position from server_roles order by position limit $1) roles",
    )
    .bind(i64::from(ceiling))
    .fetch_one(&mut *connection)
    .await?;
    if existing >= i64::from(ceiling) {
        return Err(ApiError::limit_reached(
            "the server has the largest number of roles",
        ));
    }
    crate::role_order::insert_bottom(connection, access, now, None).await?;
    sqlx::query(
        "insert into server_roles (id, name, permissions, created_at, position)
         values ($1, $2, $3, $4, 1)",
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
    let (old_name, old_permissions, position) = locked_role(connection, role_id).await?;
    Access::require_rank(
        access.rank(connection, None).await?,
        RoleRank::Server(position),
    )?;
    let permissions = new_permissions.unwrap_or(old_permissions);
    access.require(Request::ManageServerRoles {
        granting: permissions.difference(old_permissions),
    })?;

    let role = ServerRole {
        position,
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
    if administers(permissions) != administers(old_permissions) {
        store::contact_changed_for_holders(connection, now, role_id).await?;
    }
    Ok(role)
}

/// Deletes a server role. No account holds it afterwards.
#[utoipa::path(
    delete,
    path = "/api/v1/server-roles/{role_id}",
    tag = "server roles",
    params(("role_id" = ServerRoleId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn delete_server_role(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(role_id): ApiPath<ServerRoleId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        delete(connection, &access, now, role_id).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn delete(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    role_id: ServerRoleId,
) -> ApiResult<()> {
    access.require(Request::ManageServerRoles {
        granting: ServerPermissions::EMPTY,
    })?;
    let (name, permissions, position) = locked_role(connection, role_id).await?;
    Access::require_rank(
        access.rank(connection, None).await?,
        RoleRank::Server(position),
    )?;
    if administers(permissions) {
        store::contact_changed_for_holders(connection, now, role_id).await?;
    }
    let holders = events::Receivers::ServerRoleHolders(role_id);
    let holders = events::permissions_changed_for(connection, now, holders, None).await?;
    sqlx::query("delete from structure_identifiers where kind = 'server_role' and object_id = $1")
        .bind(role_id)
        .execute(&mut *connection)
        .await?;
    sqlx::query("delete from server_roles where id = $1")
        .bind(role_id)
        .execute(&mut *connection)
        .await?;
    audit::record(
        connection,
        now,
        Some(access.account_id),
        About::Server,
        &AuditEvent::ServerRoleDeleted {
            role_id,
            name,
            permissions: db::server_list(permissions),
            holders: u32::try_from(holders).map_err(|error| ApiError::internal(&error))?,
        },
    )
    .await?;
    events::publish(connection, now, &Event::ServerRoleDeleted { role_id }).await
}

/// Whether a role with these permissions makes its holders reach every account, and every
/// account reach them, for direct messages and direct calls (ADR 0307).
fn administers(permissions: ServerPermissions) -> bool {
    permissions.contains(ServerPermission::Administrator)
}

/// Records a change of the server roles of an account: for the people who list accounts, and
/// for the account, whose permissions changed. A role with the administrator permission
/// changes whom the account reaches, so the accounts that have a direct-message
/// conversation with it are told too.
async fn announce_roles(
    connection: &mut PgConnection,
    now: time::OffsetDateTime,
    account_id: AccountId,
    role: ServerPermissions,
) -> ApiResult<()> {
    let changed = Event::AccountServerRolesChanged {
        account_id,
        server_role_ids: store::server_role_ids_of(connection, account_id).await?,
    };
    events::publish(connection, now, &changed).await?;
    events::permissions_changed(connection, now, Affected::Account(account_id), None).await?;
    if administers(role) {
        store::contact_changed(connection, now, account_id).await?;
    }
    Ok(())
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
        let (_, permissions, position) = locked_role(connection, role_id).await?;
        Access::require_rank(
            access.rank(connection, None).await?,
            RoleRank::Server(position),
        )?;
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
            announce_roles(connection, now, account_id, permissions).await?;
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
        let (_, _, position) = locked_role(connection, role_id).await?;
        Access::require_rank(
            access.rank(connection, None).await?,
            RoleRank::Server(position),
        )?;
        let deleted =
            sqlx::query("delete from account_server_roles where account_id = $1 and role_id = $2")
                .bind(account_id)
                .bind(role_id)
                .execute(&mut *connection)
                .await?;
        if deleted.rows_affected() > 0 {
            let (_, permissions, _) = locked_role(connection, role_id).await?;
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
            announce_roles(connection, now, account_id, permissions).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Reorders up to 250 roles in one transaction.
#[utoipa::path(
    patch,
    path = "/api/v1/server-roles/positions",
    tag = "server roles",

    request_body = junto_protocol::roles::ReorderServerRoles,
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn reorder_server_roles(
    State(app): State<AppState>,
    bearer: Bearer,

    ApiJson(body): ApiJson<junto_protocol::roles::ReorderServerRoles>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    let positions: Vec<_> = body
        .positions
        .iter()
        .map(|p| (p.role_id.as_uuid(), p.position))
        .collect();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        crate::role_order::reorder(connection, &access, now, None, &positions).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}
