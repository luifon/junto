use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::{Request, ServerPermission};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::structure::{CreateWorkspace, UpdateWorkspace, Workspace, WorkspaceMember};

use crate::access::Access;
use crate::audit::{self, About};
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::routes::server_roles::require_active_account;
use crate::state::AppState;
use crate::{bootstrap, store, validate};
use crate::{read, transact};

/// The workspaces that the caller can see.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces",
    tag = "workspaces",
    security(("session" = [])),
    responses((status = 200, body = [Workspace]), ApiError)
)]
pub async fn list_workspaces(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<Workspace>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let default_workspace = store::default_workspace(connection).await?;
        let rows: Vec<(WorkspaceId, String)> =
            sqlx::query_as("select id, name from workspaces order by id")
                .fetch_all(&mut *connection)
                .await?;
        let mut workspaces = Vec::new();
        for (id, name) in rows {
            let scope = access.workspace(connection, id).await?;
            if access.allows(Request::ViewWorkspace(&scope)) {
                workspaces.push(Workspace {
                    id,
                    name,
                    is_default: id == default_workspace,
                });
            }
        }
        Ok(workspaces)
    })
    .map(Json)
}

/// Creates a workspace. The caller becomes its first member.
#[utoipa::path(
    post,
    path = "/api/v1/workspaces",
    tag = "workspaces",
    request_body = CreateWorkspace,
    security(("session" = [])),
    responses((status = 201, body = Workspace), ApiError)
)]
pub async fn create_workspace(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<CreateWorkspace>,
) -> ApiResult<(StatusCode, Json<Workspace>)> {
    let name = validate::name("name", &body.name)?;
    let now = app.now();
    let workspace_id = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::CreateWorkspace))?;
        let (workspace_id, everyone) = bootstrap::create_workspace(connection, &name, now).await?;
        let created = Event::WorkspaceCreated {
            workspace: Workspace {
                id: workspace_id,
                name: name.clone(),
                is_default: false,
            },
        };
        events::publish(connection, now, &created).await?;
        let role = Event::WorkspaceRoleCreated { role: everyone };
        events::publish(connection, now, &role).await?;
        store::add_members(connection, now, access.account_id, &[workspace_id]).await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::WorkspaceCreated {
                workspace_id,
                name: name.clone(),
            },
        )
        .await?;
        Ok(workspace_id)
    })?;
    Ok((
        StatusCode::CREATED,
        Json(Workspace {
            id: workspace_id,
            name,
            is_default: false,
        }),
    ))
}

/// Renames a workspace.
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path)),
    request_body = UpdateWorkspace,
    security(("session" = [])),
    responses((status = 200, body = Workspace), ApiError)
)]
pub async fn update_workspace(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<UpdateWorkspace>,
) -> ApiResult<Json<Workspace>> {
    let name = validate::name("name", &body.name)?;
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        let renamed = sqlx::query("update workspaces set name = $2 where id = $1")
            .bind(workspace_id)
            .bind(&name)
            .execute(&mut *connection)
            .await?;
        if renamed.rows_affected() == 0 {
            return Err(ApiError::not_found());
        }
        let default_workspace = store::default_workspace(connection).await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Server,
            &AuditEvent::WorkspaceRenamed {
                workspace_id,
                name: name.clone(),
            },
        )
        .await?;
        let workspace = Workspace {
            id: workspace_id,
            name: name.clone(),
            is_default: workspace_id == default_workspace,
        };
        let updated = Event::WorkspaceUpdated {
            workspace: workspace.clone(),
        };
        events::publish(connection, now, &updated).await?;
        Ok(workspace)
    })
    .map(Json)
}

/// The members of a workspace with their roles.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/members",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path)),
    security(("session" = [])),
    responses((status = 200, body = [WorkspaceMember]), ApiError)
)]
pub async fn list_members(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
) -> ApiResult<Json<Vec<WorkspaceMember>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ViewWorkspace(&scope))?;
        let rows: Vec<(AccountId, String, Vec<WorkspaceRoleId>)> = sqlx::query_as(
            "select m.account_id, a.name,
                    array(select r.role_id from member_roles r
                          where r.workspace_id = m.workspace_id and r.account_id = m.account_id
                          order by r.role_id)
             from workspace_members m
             join accounts a on a.id = m.account_id
             where m.workspace_id = $1
             order by m.account_id",
        )
        .bind(workspace_id)
        .fetch_all(&mut *connection)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(account_id, name, role_ids)| WorkspaceMember {
                account_id,
                name,
                role_ids,
            })
            .collect())
    })
    .map(Json)
}

/// Adds an existing account to a workspace. Only a server administrator can do this.
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/members/{account_id}",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path), ("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn add_member(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, account_id)): ApiPath<(WorkspaceId, AccountId)>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        store::require_workspaces_exist(connection, &[workspace_id]).await?;
        require_active_account(connection, account_id).await?;
        let added = sqlx::query(
            "insert into workspace_members (workspace_id, account_id, joined_at)
             values ($1, $2, $3)
             on conflict do nothing",
        )
        .bind(workspace_id)
        .bind(account_id)
        .bind(now)
        .execute(&mut *connection)
        .await?;
        if added.rows_affected() > 0 {
            audit::record(
                connection,
                now,
                Some(access.account_id),
                About::Workspace(workspace_id),
                &AuditEvent::WorkspaceMemberAdded { account_id },
            )
            .await?;
            store::announce_members(connection, now, account_id, &[workspace_id]).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}
