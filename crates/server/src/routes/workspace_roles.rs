use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use std::collections::HashMap;

use junto_authz::{
    ChannelGrant, Override, Request, WorkspacePermission, WorkspacePermissions, WorkspaceScope,
};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, ChannelId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::roles::{
    ChangeWorkspaceRolePermission, CreateWorkspaceRole, ListedWorkspaceRole, RoleActions,
    UpdateWorkspaceRole, WorkspaceRole,
};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::{Access, stored_override};
use crate::audit::{self, About};
use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::events::log::Affected;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::routes::server_roles::require_active_account;
use crate::state::AppState;
use crate::validate;
use crate::{read, transact};

struct StoredRole {
    position: i32,
    name: String,
    permissions: WorkspacePermissions,
    is_everyone: bool,
}

async fn locked_role(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    role_id: WorkspaceRoleId,
) -> ApiResult<StoredRole> {
    let row: Option<(String, i64, bool, i32)> = sqlx::query_as(
        "select name, permissions, is_everyone, position from workspace_roles
         where id = $1 and workspace_id = $2
         for update",
    )
    .bind(role_id)
    .bind(workspace_id)
    .fetch_optional(connection)
    .await?;
    let (name, bits, is_everyone, position) = row.ok_or_else(ApiError::not_found)?;
    Ok(StoredRole {
        position,
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
    responses((status = 200, body = [ListedWorkspaceRole]), ApiError)
)]
pub async fn list_workspace_roles(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
) -> ApiResult<Json<Vec<ListedWorkspaceRole>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ViewWorkspace(&scope))?;
        let checks = if access.allows(Request::Workspace {
            scope: &scope,
            permission: WorkspacePermission::ManageRoles,
        }) {
            Some(RoleChecks::load(connection, &access, workspace_id, &scope).await?)
        } else {
            None
        };
        let mut roles = Vec::new();
        for role in roles_of(connection, workspace_id).await? {
            let (assign, remove) = match checks.as_ref().filter(|_| !role.is_everyone) {
                Some(checks) => (
                    crate::role_order::permitted(checks.require(&access, &scope, &role, true))?,
                    crate::role_order::permitted(checks.require(&access, &scope, &role, false))?,
                ),
                None => (false, false),
            };
            let actions = RoleActions {
                may_assign: access.token_id.is_none() && assign,
                may_remove: access.token_id.is_none() && remove,
                may_delete: access.permits_token_action(
                    junto_protocol::access_token::TokenAction::ManageStructure,
                ) && remove,
            };
            roles.push(ListedWorkspaceRole { role, actions });
        }
        Ok(roles)
    })
    .map(Json)
}

/// The roles of a workspace, the "everyone" role first.
pub async fn roles_of(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
) -> ApiResult<Vec<WorkspaceRole>> {
    let rows: Vec<(WorkspaceRoleId, String, i64, bool, i32)> = sqlx::query_as(
        "select id, name, permissions, is_everyone, position from workspace_roles
         where workspace_id = $1
         order by position",
    )
    .bind(workspace_id)
    .fetch_all(connection)
    .await?;
    rows.into_iter()
        .map(|(id, name, bits, is_everyone, position)| {
            Ok(WorkspaceRole {
                position,
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
    now: OffsetDateTime,
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
        position: 1,
        id: WorkspaceRoleId::generate(),
        workspace_id,
        name: validate::name("name", &body.name)?,
        permissions: db::workspace_list(permissions),
        is_everyone: false,
    };
    let now = app.now();
    let ceiling = app.config.limits.most_roles_per_workspace;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        create(connection, &access, now, &role, ceiling).await
    })?;
    Ok((StatusCode::CREATED, Json(role)))
}

/// Creates `role` in its workspace, which holds at most `ceiling` roles.
pub(crate) async fn create(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    role: &WorkspaceRole,
    ceiling: u32,
) -> ApiResult<()> {
    let workspace_id = role.workspace_id;
    let permissions: WorkspacePermissions = role.permissions.iter().copied().collect();
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
    if existing >= i64::from(ceiling) {
        return Err(ApiError::limit_reached(
            "the workspace has the largest number of roles",
        ));
    }
    crate::role_order::insert_bottom(connection, access, now, Some(workspace_id)).await?;
    sqlx::query(
        "insert into workspace_roles (id, workspace_id, name, permissions, created_at, position)
         values ($1, $2, $3, $4, $5, 1)",
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
        update(
            connection,
            &access,
            now,
            (workspace_id, role_id),
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
    (workspace_id, role_id): (WorkspaceId, WorkspaceRoleId),
    new_name: Option<String>,
    new_permissions: Option<WorkspacePermissions>,
) -> ApiResult<WorkspaceRole> {
    let scope = access.workspace(connection, workspace_id).await?;
    access.require(Request::Workspace {
        scope: &scope,
        permission: WorkspacePermission::ManageRoles,
    })?;
    let stored = locked_role(connection, workspace_id, role_id).await?;
    Access::require_rank(
        access.rank(connection, Some(workspace_id)).await?,
        junto_authz::RoleRank::Workspace(stored.position),
    )?;
    let permissions = new_permissions.unwrap_or(stored.permissions);
    access.require(Request::ManageWorkspaceRoles {
        scope: &scope,
        granting: permissions.difference(stored.permissions),
        in_channels: &[],
    })?;

    let role = WorkspaceRole {
        position: stored.position,
        id: role_id,
        workspace_id,
        name: new_name.unwrap_or_else(|| stored.name.clone()),
        permissions: db::workspace_list(permissions),
        is_everyone: stored.is_everyone,
    };
    if role.name == stored.name && permissions == stored.permissions {
        return Ok(role);
    }
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
}

/// Deletes a workspace role. No member holds it afterwards, and no channel has an override
/// of it. The "everyone" role is not deleted.
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/roles/{role_id}",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path), ("role_id" = WorkspaceRoleId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn delete_workspace_role(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, role_id)): ApiPath<(WorkspaceId, WorkspaceRoleId)>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        delete(connection, &access, now, (workspace_id, role_id)).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Requires what the grant rule asks of a caller who takes a role from its holders, by the
/// deletion of the role or by its removal from one member: the managing permission and, in
/// each channel, the permissions that an override of the role denies there. A holder gets
/// those permissions back, so the change adds them (ADR 0304).
/// Deletes a workspace role with its assignments and its overrides. The removal of an
/// override that denies a permission can add that permission to a holder of the role, so
/// the caller must hold, in each channel, what the role is denied there (ADR 0304).
pub(crate) async fn delete(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    (workspace_id, role_id): (WorkspaceId, WorkspaceRoleId),
) -> ApiResult<()> {
    let scope = access.workspace(connection, workspace_id).await?;
    access.require(Request::Workspace {
        scope: &scope,
        permission: WorkspacePermission::ManageRoles,
    })?;
    let stored = locked_role(connection, workspace_id, role_id).await?;
    require_role_action(
        connection,
        access,
        &scope,
        &stored.description(workspace_id, role_id),
        false,
    )
    .await?;
    if stored.is_everyone {
        return Err(ApiError::conflict(
            "every member holds the everyone role; it is not deleted",
        ));
    }

    let holders = events::Receivers::WorkspaceRoleHolders(role_id);
    let holders =
        events::permissions_changed_for(connection, now, holders, Some(workspace_id)).await?;
    sqlx::query(
        "delete from structure_identifiers where kind = 'workspace_role' and object_id = $1",
    )
    .bind(role_id)
    .execute(&mut *connection)
    .await?;
    sqlx::query("delete from workspace_roles where id = $1")
        .bind(role_id)
        .execute(&mut *connection)
        .await?;
    audit::record(
        connection,
        now,
        Some(access.account_id),
        About::Workspace(workspace_id),
        &AuditEvent::WorkspaceRoleDeleted {
            role_id,
            name: stored.name,
            permissions: db::workspace_list(stored.permissions),
            holders: u32::try_from(holders).map_err(|error| ApiError::internal(&error))?,
        },
    )
    .await?;
    let deleted = Event::WorkspaceRoleDeleted {
        workspace_id,
        role_id,
    };
    events::publish(connection, now, &deleted).await
}

async fn require_role_action(
    connection: &mut PgConnection,
    access: &Access,
    scope: &WorkspaceScope,
    role: &WorkspaceRole,
    assigning: bool,
) -> ApiResult<()> {
    RoleChecks::load(connection, access, role.workspace_id, scope)
        .await?
        .require(access, scope, role, assigning)
}

struct RoleChecks {
    rank: junto_authz::RoleRank,
    channels: Vec<(crate::access::ChannelRecord, junto_authz::ChannelScope)>,
    overrides: HashMap<WorkspaceRoleId, HashMap<ChannelId, Override>>,
}

impl RoleChecks {
    async fn load(
        connection: &mut PgConnection,
        access: &Access,
        workspace: WorkspaceId,
        scope: &WorkspaceScope,
    ) -> ApiResult<Self> {
        let rank = access.rank(connection, Some(workspace)).await?;
        let (channels, rows) = access
            .workspace_channels_and_overrides(connection, workspace, scope)
            .await?;
        let mut overrides: HashMap<WorkspaceRoleId, HashMap<ChannelId, Override>> = HashMap::new();
        for (channel, role, _, allow, deny) in rows {
            overrides
                .entry(role)
                .or_default()
                .insert(channel, stored_override(allow, deny)?);
        }
        Ok(Self {
            rank,
            channels,
            overrides,
        })
    }

    fn require(
        &self,
        access: &Access,
        scope: &WorkspaceScope,
        role: &WorkspaceRole,
        assigning: bool,
    ) -> ApiResult<()> {
        access.require(Request::Workspace {
            scope,
            permission: WorkspacePermission::ManageRoles,
        })?;
        Access::require_rank(self.rank, junto_authz::RoleRank::Workspace(role.position))?;
        let mut in_channels = Vec::new();
        for (record, channel_scope) in &self.channels {
            if let Some(overrides) = self
                .overrides
                .get(&role.id)
                .and_then(|rows| rows.get(&record.channel.id))
            {
                let granting = if assigning {
                    overrides.allow()
                } else {
                    overrides.granted_by_change_to(Override::NONE)
                };
                if granting != WorkspacePermissions::EMPTY {
                    in_channels.push(ChannelGrant {
                        scope: channel_scope,
                        granting,
                    });
                }
            }
        }
        access.require(Request::ManageWorkspaceRoles {
            scope,
            granting: if assigning {
                role.permissions.iter().copied().collect()
            } else {
                WorkspacePermissions::EMPTY
            },
            in_channels: &in_channels,
        })
    }
}

impl StoredRole {
    fn description(&self, workspace_id: WorkspaceId, id: WorkspaceRoleId) -> WorkspaceRole {
        WorkspaceRole {
            position: self.position,
            id,
            workspace_id,
            name: self.name.clone(),
            permissions: db::workspace_list(self.permissions),
            is_everyone: self.is_everyone,
        }
    }
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
        require_role_action(
            connection,
            &access,
            &scope,
            &stored.description(workspace_id, role_id),
            true,
        )
        .await?;
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
        // A banned account keeps its memberships (ADR 0113) and gets no role.
        require_active_account(connection, account_id).await?;

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

/// Takes a workspace role from a member. The member gets back what the overrides of the
/// role deny, so the caller must hold those permissions in the channels of the overrides.
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
        access.require(Request::Workspace {
            scope: &scope,
            permission: WorkspacePermission::ManageRoles,
        })?;
        let stored = locked_role(connection, workspace_id, role_id).await?;
        require_role_action(
            connection,
            &access,
            &scope,
            &stored.description(workspace_id, role_id),
            false,
        )
        .await?;
        if stored.is_everyone {
            return Err(ApiError::conflict("the everyone role is not assigned"));
        }

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

/// Reorders up to 250 roles in one transaction.
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/roles/positions",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path)),
    request_body = junto_protocol::roles::ReorderWorkspaceRoles,
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn reorder_workspace_roles(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<junto_protocol::roles::ReorderWorkspaceRoles>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    let positions: Vec<_> = body
        .positions
        .iter()
        .map(|p| (p.role_id.as_uuid(), p.position))
        .collect();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        crate::role_order::reorder(connection, &access, now, Some(workspace_id), &positions).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Changes one permission while preserving concurrent changes to other permissions.
#[utoipa::path(patch,path="/api/v1/workspaces/{workspace_id}/roles/{role_id}/permissions",tag="workspaces",
    params(("workspace_id"=WorkspaceId,Path),("role_id"=WorkspaceRoleId,Path)),request_body=ChangeWorkspaceRolePermission,
    security(("session"=[])),responses((status=200,body=WorkspaceRole),ApiError))]
pub async fn change_permission(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, role_id)): ApiPath<(WorkspaceId, WorkspaceRoleId)>,
    ApiJson(body): ApiJson<ChangeWorkspaceRolePermission>,
) -> ApiResult<Json<WorkspaceRole>> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::Workspace {
            scope: &scope,
            permission: WorkspacePermission::ManageRoles,
        })?;
        let stored = locked_role(connection, workspace_id, role_id).await?;
        let bit: WorkspacePermissions = [body.permission].into_iter().collect();
        let permissions = if body.enabled {
            stored.permissions.union(bit)
        } else {
            stored.permissions.difference(bit)
        };
        update(
            connection,
            &access,
            now,
            (workspace_id, role_id),
            None,
            Some(permissions),
        )
        .await
    })
    .map(Json)
}
