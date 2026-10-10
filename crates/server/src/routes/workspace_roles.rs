use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use std::collections::HashMap;

use junto_authz::{ChannelGrant, Request, WorkspacePermission, WorkspacePermissions};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, ChannelId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::roles::{CreateWorkspaceRole, UpdateWorkspaceRole, WorkspaceRole};
use sqlx::PgConnection;

use crate::access::Access;
use crate::audit::{self, About};
use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::events::log::Affected;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::state::AppState;
use crate::validate;
use crate::{read, transact};

struct StoredRole {
    name: String,
    permissions: WorkspacePermissions,
    is_everyone: bool,
}

async fn locked_role(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    role_id: WorkspaceRoleId,
) -> ApiResult<StoredRole> {
    let row: Option<(String, i64, bool)> = sqlx::query_as(
        "select name, permissions, is_everyone from workspace_roles
         where id = $1 and workspace_id = $2
         for update",
    )
    .bind(role_id)
    .bind(workspace_id)
    .fetch_optional(connection)
    .await?;
    let (name, bits, is_everyone) = row.ok_or_else(ApiError::not_found)?;
    Ok(StoredRole {
        name,
        permissions: db::workspace_permissions(bits)?,
        is_everyone,
    })
}

/// The roles of a workspace, the "everyone" role included.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/roles",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path)),
    security(("session" = [])),
    responses((status = 200, body = [WorkspaceRole]), ApiError)
)]
pub async fn list_workspace_roles(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
) -> ApiResult<Json<Vec<WorkspaceRole>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ViewWorkspace(&scope))?;
        roles_of(connection, workspace_id).await
    })
    .map(Json)
}

/// The roles of a workspace, the "everyone" role first.
pub async fn roles_of(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
) -> ApiResult<Vec<WorkspaceRole>> {
    let rows: Vec<(WorkspaceRoleId, String, i64, bool)> = sqlx::query_as(
        "select id, name, permissions, is_everyone from workspace_roles
         where workspace_id = $1
         order by is_everyone desc, id",
    )
    .bind(workspace_id)
    .fetch_all(connection)
    .await?;
    rows.into_iter()
        .map(|(id, name, bits, is_everyone)| {
            Ok(WorkspaceRole {
                id,
                workspace_id,
                name,
                permissions: db::workspace_list(db::workspace_permissions(bits)?),
                is_everyone,
            })
        })
        .collect()
}

pub async fn role_ids_of(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    account_id: AccountId,
) -> ApiResult<Vec<WorkspaceRoleId>> {
    Ok(sqlx::query_scalar(
        "select role_id from member_roles
         where workspace_id = $1 and account_id = $2
         order by role_id",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_all(connection)
    .await?)
}

/// Records a change of the roles of a member: for the members of the workspace, and for the
/// member, whose permissions changed.
async fn announce_roles(
    connection: &mut PgConnection,
    now: time::OffsetDateTime,
    workspace_id: WorkspaceId,
    account_id: AccountId,
) -> ApiResult<()> {
    let changed = Event::WorkspaceMemberRolesChanged {
        workspace_id,
        account_id,
        role_ids: role_ids_of(connection, workspace_id, account_id).await?,
    };
    events::publish(connection, now, &changed).await?;
    let affected = Affected::Account(account_id);
    events::permissions_changed(connection, now, affected, Some(workspace_id)).await
}

/// Creates a workspace role.
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/roles",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path)),
    request_body = CreateWorkspaceRole,
    security(("session" = [])),
    responses((status = 201, body = WorkspaceRole), ApiError)
)]
pub async fn create_workspace_role(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<CreateWorkspaceRole>,
) -> ApiResult<(StatusCode, Json<WorkspaceRole>)> {
    let permissions: WorkspacePermissions = body.permissions.iter().copied().collect();
    let role = WorkspaceRole {
        id: WorkspaceRoleId::generate(),
        workspace_id,
        name: validate::name("name", &body.name)?,
        permissions: db::workspace_list(permissions),
        is_everyone: false,
    };
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ManageWorkspaceRoles {
            scope: &scope,
            granting: permissions,
            in_channels: &[],
        })?;
        let existing: i64 =
            sqlx::query_scalar("select count(*) from workspace_roles where workspace_id = $1")
                .bind(workspace_id)
                .fetch_one(&mut *connection)
                .await?;
        if existing >= i64::from(app.config.limits.most_roles_per_workspace) {
            return Err(ApiError::limit_reached(
                "the workspace has the largest number of roles",
            ));
        }
        sqlx::query(
            "insert into workspace_roles (id, workspace_id, name, permissions, created_at)
             values ($1, $2, $3, $4, $5)",
        )
        .bind(role.id)
        .bind(workspace_id)
        .bind(&role.name)
        .bind(db::workspace_bits(permissions))
        .bind(now)
        .execute(&mut *connection)
        .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Workspace(workspace_id),
            &AuditEvent::WorkspaceRoleCreated {
                role_id: role.id,
                name: role.name.clone(),
                permissions: role.permissions.clone(),
            },
        )
        .await?;
        let created = Event::WorkspaceRoleCreated { role: role.clone() };
        events::publish(connection, now, &created).await
    })?;
    Ok((StatusCode::CREATED, Json(role)))
}

/// Changes the name or the permissions of a workspace role.
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/roles/{role_id}",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path), ("role_id" = WorkspaceRoleId, Path)),
    request_body = UpdateWorkspaceRole,
    security(("session" = [])),
    responses((status = 200, body = WorkspaceRole), ApiError)
)]
pub async fn update_workspace_role(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, role_id)): ApiPath<(WorkspaceId, WorkspaceRoleId)>,
    ApiJson(body): ApiJson<UpdateWorkspaceRole>,
) -> ApiResult<Json<WorkspaceRole>> {
    let new_name = body
        .name
        .as_deref()
        .map(|name| validate::name("name", name))
        .transpose()?;
    let new_permissions: Option<WorkspacePermissions> = body
        .permissions
        .as_ref()
        .map(|permissions| permissions.iter().copied().collect());
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::Workspace {
            scope: &scope,
            permission: WorkspacePermission::ManageRoles,
        })?;
        let stored = locked_role(connection, workspace_id, role_id).await?;
        let permissions = new_permissions.unwrap_or(stored.permissions);
        access.require(Request::ManageWorkspaceRoles {
            scope: &scope,
            granting: permissions.difference(stored.permissions),
            in_channels: &[],
        })?;

        let role = WorkspaceRole {
            id: role_id,
            workspace_id,
            name: new_name.clone().unwrap_or(stored.name),
            permissions: db::workspace_list(permissions),
            is_everyone: stored.is_everyone,
        };
        sqlx::query("update workspace_roles set name = $2, permissions = $3 where id = $1")
            .bind(role_id)
            .bind(&role.name)
            .bind(db::workspace_bits(permissions))
            .execute(&mut *connection)
            .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Workspace(workspace_id),
            &AuditEvent::WorkspaceRoleUpdated {
                role_id,
                name: role.name.clone(),
                permissions: role.permissions.clone(),
            },
        )
        .await?;
        let updated = Event::WorkspaceRoleUpdated { role: role.clone() };
        events::publish(connection, now, &updated).await?;
        if permissions != stored.permissions {
            let holders = Affected::WorkspaceRoleHolders(workspace_id, role_id);
            events::permissions_changed(connection, now, holders, Some(workspace_id)).await?;
        }
        Ok(role)
    })
    .map(Json)
}

/// Gives a workspace role to a member of the workspace.
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/members/{account_id}/roles/{role_id}",
    tag = "workspaces",
    params(
        ("workspace_id" = WorkspaceId, Path),
        ("account_id" = AccountId, Path),
        ("role_id" = WorkspaceRoleId, Path)
    ),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn assign_workspace_role(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, account_id, role_id)): ApiPath<(
        WorkspaceId,
        AccountId,
        WorkspaceRoleId,
    )>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::Workspace {
            scope: &scope,
            permission: WorkspacePermission::ManageRoles,
        })?;
        let stored = locked_role(connection, workspace_id, role_id).await?;
        let allowed_in: HashMap<ChannelId, i64> = sqlx::query_as(
            "select channel_id, allow from channel_overrides where role_id = $1 and allow <> 0",
        )
        .bind(role_id)
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .collect();
        let channels = access
            .workspace_channels(connection, workspace_id, &scope)
            .await?;
        let mut in_channels = Vec::new();
        for (record, channel_scope) in &channels {
            if let Some(allow) = allowed_in.get(&record.channel.id) {
                in_channels.push(ChannelGrant {
                    scope: channel_scope,
                    granting: db::workspace_permissions(*allow)?,
                });
            }
        }
        access.require(Request::ManageWorkspaceRoles {
            scope: &scope,
            granting: stored.permissions,
            in_channels: &in_channels,
        })?;
        if stored.is_everyone {
            return Err(ApiError::conflict(
                "every member holds the everyone role; it is not assigned",
            ));
        }
        let is_member: bool = sqlx::query_scalar(
            "select exists (
                 select 1 from workspace_members where workspace_id = $1 and account_id = $2)",
        )
        .bind(workspace_id)
        .bind(account_id)
        .fetch_one(&mut *connection)
        .await?;
        if !is_member {
            return Err(ApiError::not_found());
        }

        let inserted = sqlx::query(
            "insert into member_roles (workspace_id, account_id, role_id) values ($1, $2, $3)
             on conflict do nothing",
        )
        .bind(workspace_id)
        .bind(account_id)
        .bind(role_id)
        .execute(&mut *connection)
        .await?;
        if inserted.rows_affected() > 0 {
            let held = role_ids_of(connection, workspace_id, account_id)
                .await?
                .len();
            if held > app.config.limits.most_roles_per_member as usize {
                return Err(ApiError::limit_reached(
                    "the member holds the largest number of roles of the workspace",
                ));
            }
            audit::record(
                connection,
                now,
                Some(access.account_id),
                About::Workspace(workspace_id),
                &AuditEvent::WorkspaceRoleAssigned {
                    role_id,
                    account_id,
                },
            )
            .await?;
            announce_roles(connection, now, workspace_id, account_id).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Takes a workspace role from a member.
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/members/{account_id}/roles/{role_id}",
    tag = "workspaces",
    params(
        ("workspace_id" = WorkspaceId, Path),
        ("account_id" = AccountId, Path),
        ("role_id" = WorkspaceRoleId, Path)
    ),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn unassign_workspace_role(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, account_id, role_id)): ApiPath<(
        WorkspaceId,
        AccountId,
        WorkspaceRoleId,
    )>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ManageWorkspaceRoles {
            scope: &scope,
            granting: WorkspacePermissions::EMPTY,
            in_channels: &[],
        })?;
        let deleted = sqlx::query(
            "delete from member_roles
             where workspace_id = $1 and account_id = $2 and role_id = $3",
        )
        .bind(workspace_id)
        .bind(account_id)
        .bind(role_id)
        .execute(&mut *connection)
        .await?;
        if deleted.rows_affected() > 0 {
            audit::record(
                connection,
                now,
                Some(access.account_id),
                About::Workspace(workspace_id),
                &AuditEvent::WorkspaceRoleUnassigned {
                    role_id,
                    account_id,
                },
            )
            .await?;
            announce_roles(connection, now, workspace_id, account_id).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}
