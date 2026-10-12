use std::collections::HashSet;

use junto_authz::{Request, RoleRank, ServerPermission, WorkspacePermission};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{ServerRoleId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::roles::{ServerRole, WorkspaceRole};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::access::Access;
use crate::audit::{self, About};
use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::events::{self, log::Affected};

const MOST_REORDERED_ROLES: usize = 250;
const HIGHEST_REORDER_POSITION: i32 = 1_000_000;
const POSITIONS_PER_EVENT: usize = 128;

fn table(workspace: Option<WorkspaceId>) -> &'static str {
    if workspace.is_some() {
        "workspace_roles"
    } else {
        "server_roles"
    }
}

fn rank(workspace: Option<WorkspaceId>, position: i32) -> RoleRank {
    if workspace.is_some() {
        RoleRank::Workspace(position)
    } else {
        RoleRank::Server(position)
    }
}

async fn require_manage(
    connection: &mut PgConnection,
    access: &Access,
    workspace: Option<WorkspaceId>,
) -> ApiResult<()> {
    if let Some(workspace) = workspace {
        let scope = access.workspace(connection, workspace).await?;
        access.require(Request::Workspace {
            scope: &scope,
            permission: WorkspacePermission::ManageRoles,
        })
    } else {
        access.require(Request::Server(ServerPermission::Administrator))
    }
}

pub async fn insert_bottom(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    workspace: Option<WorkspaceId>,
) -> ApiResult<()> {
    let actor = access.rank(connection, workspace).await?;
    Access::require_rank(actor, rank(workspace, 0))?;
    let highest: Option<i32> = if let Some(workspace) = workspace {
        sqlx::query_scalar("select max(position) from workspace_roles where workspace_id = $1")
            .bind(workspace)
            .fetch_one(&mut *connection)
            .await?
    } else {
        sqlx::query_scalar("select max(position) from server_roles")
            .fetch_one(&mut *connection)
            .await?
    };
    if highest == Some(i32::MAX) {
        return Err(ApiError::conflict(
            "role positions are exhausted; reorder them first",
        ));
    }
    if let Some(workspace_id) = workspace {
        let rows: Vec<(WorkspaceRoleId, i32)> = sqlx::query_as(
            "update workspace_roles set position = position + 1 where workspace_id = $1 and not is_everyone returning id, position",
        ).bind(workspace_id).fetch_all(&mut *connection).await?;
        for chunk in rows.chunks(POSITIONS_PER_EVENT) {
            let positions = chunk
                .iter()
                .map(
                    |&(role_id, position)| junto_protocol::roles::WorkspaceRolePosition {
                        role_id,
                        position,
                    },
                )
                .collect();
            events::publish(
                connection,
                now,
                &Event::WorkspaceRolePositionsShifted {
                    workspace_id,
                    positions,
                },
            )
            .await?;
        }
    } else {
        let rows: Vec<(ServerRoleId, i32)> = sqlx::query_as(
            "update server_roles set position = position + 1 returning id, position",
        )
        .fetch_all(&mut *connection)
        .await?;
        for chunk in rows.chunks(POSITIONS_PER_EVENT) {
            let positions = chunk
                .iter()
                .map(
                    |&(role_id, position)| junto_protocol::roles::ServerRolePosition {
                        role_id,
                        position,
                    },
                )
                .collect();
            events::publish(
                connection,
                now,
                &Event::ServerRolePositionsShifted { positions },
            )
            .await?;
        }
    }

    Ok(())
}

pub async fn reorder(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    workspace: Option<WorkspaceId>,
    positions: &[(Uuid, i32)],
) -> ApiResult<()> {
    require_manage(connection, access, workspace).await?;
    if positions.len() > MOST_REORDERED_ROLES {
        return Err(ApiError::invalid("at most 250 role positions may change"));
    }
    let mut ids = HashSet::new();
    let mut values = HashSet::new();
    for (id, position) in positions {
        if !(1..=HIGHEST_REORDER_POSITION).contains(position)
            || !ids.insert(*id)
            || !values.insert(*position)
        {
            return Err(ApiError::invalid(
                "role identifiers and positions from 1 to 1000000 must be distinct",
            ));
        }
    }
    let actor = access.rank(connection, workspace).await?;
    let mut changes = Vec::new();
    for (id, position) in positions {
        let old: Option<i32> = if let Some(workspace) = workspace {
            sqlx::query_scalar(
                "select position from workspace_roles where id = $1 and workspace_id = $2",
            )
            .bind(id)
            .bind(workspace)
            .fetch_optional(&mut *connection)
            .await?
        } else {
            sqlx::query_scalar("select position from server_roles where id = $1")
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?
        };
        let old = old.ok_or_else(ApiError::not_found)?;
        if old == 0 {
            return Err(ApiError::invalid("the everyone role cannot move"));
        }
        Access::require_rank(actor, rank(workspace, old))?;
        Access::require_rank(actor, rank(workspace, *position))?;
        let occupant: Option<Uuid> = if let Some(workspace) = workspace {
            sqlx::query_scalar(
                "select id from workspace_roles where workspace_id = $1 and position = $2",
            )
            .bind(workspace)
            .bind(position)
            .fetch_optional(&mut *connection)
            .await?
        } else {
            sqlx::query_scalar("select id from server_roles where position = $1")
                .bind(position)
                .fetch_optional(&mut *connection)
                .await?
        };
        if occupant.is_some_and(|id| !ids.contains(&id)) {
            return Err(ApiError::invalid(
                "a position is held by a role outside the reorder",
            ));
        }
        if old != *position {
            changes.push((*id, *position));
        }
    }
    for (id, position) in changes {
        write_position(connection, workspace, id, position).await?;
        announce(connection, access, now, workspace, id, position).await?;
    }
    Ok(())
}

async fn write_position(
    connection: &mut PgConnection,
    workspace: Option<WorkspaceId>,
    id: Uuid,
    position: i32,
) -> ApiResult<()> {
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "update {} set position = $2 where id = $1",
        table(workspace)
    )))
    .bind(id)
    .bind(position)
    .execute(connection)
    .await?;
    Ok(())
}

async fn announce(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    workspace: Option<WorkspaceId>,
    id: Uuid,
    position: i32,
) -> ApiResult<()> {
    let (event, audit, affected, about) = if let Some(workspace_id) = workspace {
        let (name, bits, is_everyone): (String, i64, bool) = sqlx::query_as(
            "select name, permissions, is_everyone from workspace_roles where id = $1",
        )
        .bind(id)
        .fetch_one(&mut *connection)
        .await?;
        let role_id = WorkspaceRoleId::from_uuid(id);
        (
            Event::WorkspaceRoleUpdated {
                role: WorkspaceRole {
                    id: role_id,
                    workspace_id,
                    position,
                    name,
                    is_everyone,
                    permissions: db::workspace_list(db::workspace_permissions(bits)?),
                },
            },
            AuditEvent::WorkspaceRolePositionChanged { role_id, position },
            Affected::WorkspaceRoleHolders(workspace_id, role_id),
            About::Workspace(workspace_id),
        )
    } else {
        let (name, bits): (String, i64) =
            sqlx::query_as("select name, permissions from server_roles where id = $1")
                .bind(id)
                .fetch_one(&mut *connection)
                .await?;
        let role_id = ServerRoleId::from_uuid(id);
        (
            Event::ServerRoleUpdated {
                role: ServerRole {
                    id: role_id,
                    position,
                    name,
                    permissions: db::server_list(db::server_permissions(bits)?),
                },
            },
            AuditEvent::ServerRolePositionChanged { role_id, position },
            Affected::ServerRoleHolders(role_id),
            About::Server,
        )
    };
    audit::record(connection, now, Some(access.account_id), about, &audit).await?;
    events::publish(connection, now, &event).await?;
    events::permissions_changed(connection, now, affected, workspace).await
}
