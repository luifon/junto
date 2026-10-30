use std::collections::{HashMap, HashSet};

use junto_authz::WorkspaceScope;
use junto_protocol::events::Event;
use junto_protocol::ids::{ServerRoleId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::roles::{ServerRole, WorkspaceRole};
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::access::Access;
use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::routes::{server_roles, workspace_roles};

pub(crate) enum Creation {
    Server(ServerRole),
    Workspace(WorkspaceRole),
}

impl Creation {
    pub(crate) fn id(&self) -> Uuid {
        match self {
            Self::Server(role) => role.id.as_uuid(),
            Self::Workspace(role) => role.id.as_uuid(),
        }
    }

    fn name(&self) -> &str {
        match self {
            Self::Server(role) => &role.name,
            Self::Workspace(role) => &role.name,
        }
    }

    fn bits(&self) -> i64 {
        match self {
            Self::Server(role) => db::server_bits(role.permissions.iter().copied().collect()),
            Self::Workspace(role) => db::workspace_bits(role.permissions.iter().copied().collect()),
        }
    }
}

pub(crate) struct Insertions {
    workspace: Option<WorkspaceId>,
    ceiling: u32,
    scope: Option<WorkspaceScope>,
    existing: Option<Vec<(Uuid, i32)>>,
    pending: Vec<Creation>,
}

impl Insertions {
    pub(crate) fn new(workspace: Option<WorkspaceId>, ceiling: u32) -> Self {
        Self {
            workspace,
            ceiling,
            scope: None,
            existing: None,
            pending: Vec::new(),
        }
    }

    pub(crate) async fn push(
        &mut self,
        connection: &mut PgConnection,
        access: &Access,
        role: Creation,
    ) -> ApiResult<()> {
        let workspace = match &role {
            Creation::Server(_) => None,
            Creation::Workspace(role) => Some(role.workspace_id),
        };
        if self.workspace != workspace {
            return Err(ApiError::internal(&"a role batch changes its scope"));
        }
        match &role {
            Creation::Server(role) => server_roles::require_creation(access, role)?,
            Creation::Workspace(role) => {
                if self.scope.is_none() {
                    self.scope = Some(access.workspace(connection, role.workspace_id).await?);
                }
                if let Some(scope) = &self.scope {
                    workspace_roles::require_creation(access, scope, role)?;
                } else {
                    return Err(ApiError::internal(&"a workspace role batch has no scope"));
                }
            }
        }
        if self.existing.is_none() {
            self.existing = Some(if let Some(workspace) = self.workspace {
                sqlx::query_as(
                    "select id,position from workspace_roles where workspace_id=$1 limit $2",
                )
                .bind(workspace)
                .bind(i64::from(self.ceiling))
                .fetch_all(&mut *connection)
                .await?
            } else {
                sqlx::query_as("select id,position from server_roles limit $1")
                    .bind(i64::from(self.ceiling))
                    .fetch_all(&mut *connection)
                    .await?
            });
        }
        let count = self.existing.as_ref().map_or(0, Vec::len) + self.pending.len();
        if count >= self.ceiling as usize {
            return Err(ApiError::limit_reached(if self.workspace.is_some() {
                "the workspace has the largest number of roles"
            } else {
                "the server has the largest number of roles"
            }));
        }
        if self.pending.is_empty() {
            let actor = access.rank(connection, self.workspace).await?;
            Access::require_rank(actor, super::rank(self.workspace, 0))?;
        }
        self.pending.push(role);
        Ok(())
    }

    pub(crate) async fn finish(
        self,
        connection: &mut PgConnection,
        access: &Access,
        now: OffsetDateTime,
    ) -> ApiResult<()> {
        let mut positions: Vec<_> = self
            .existing
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, position)| *position > 0)
            .collect();
        let previous: HashSet<_> = positions.iter().map(|(id, _)| *id).collect();
        let mut shifts = Vec::new();
        for role in &self.pending {
            if positions.iter().any(|(_, position)| *position == i32::MAX) {
                positions.sort_by_key(|(_, position)| *position);
                for (index, (_, position)) in positions.iter_mut().enumerate() {
                    *position =
                        i32::try_from(index + 1).map_err(|error| ApiError::internal(&error))?;
                }
            }
            for (_, position) in &mut positions {
                *position += 1;
            }
            shifts.push(positions.clone());
            positions.push((role.id(), 1));
        }
        let old: Vec<_> = positions
            .iter()
            .filter(|(id, _)| previous.contains(id))
            .collect();
        if !old.is_empty() {
            let ids: Vec<_> = old.iter().map(|(id, _)| *id).collect();
            let values: Vec<_> = old.iter().map(|(_, position)| *position).collect();
            sqlx::query(if self.workspace.is_some() {
                "update workspace_roles r set position=p.position from unnest($1::uuid[],$2::integer[]) p(id,position) where r.id=p.id"
            } else {
                "update server_roles r set position=p.position from unnest($1::uuid[],$2::integer[]) p(id,position) where r.id=p.id"
            }).bind(ids).bind(values).execute(&mut *connection).await?;
        }
        let final_positions: HashMap<_, _> = positions.into_iter().collect();
        let ids: Vec<_> = self.pending.iter().map(Creation::id).collect();
        let names: Vec<_> = self
            .pending
            .iter()
            .map(|role| role.name().to_owned())
            .collect();
        let bits: Vec<_> = self.pending.iter().map(Creation::bits).collect();
        let values: Vec<_> = ids
            .iter()
            .map(|id| {
                final_positions
                    .get(id)
                    .copied()
                    .ok_or_else(|| ApiError::internal(&"a created role has no position"))
            })
            .collect::<ApiResult<_>>()?;
        if let Some(workspace) = self.workspace {
            sqlx::query("insert into workspace_roles (id,name,permissions,position,workspace_id,created_at) select id,name,permissions,position,$5,$6 from unnest($1::uuid[],$2::text[],$3::bigint[],$4::integer[]) p(id,name,permissions,position)")
                .bind(ids).bind(names).bind(bits).bind(values).bind(workspace).bind(now)
                .execute(&mut *connection).await?;
        } else {
            sqlx::query("insert into server_roles (id,name,permissions,position,created_at) select id,name,permissions,position,$5 from unnest($1::uuid[],$2::text[],$3::bigint[],$4::integer[]) p(id,name,permissions,position)")
                .bind(ids).bind(names).bind(bits).bind(values).bind(now)
                .execute(&mut *connection).await?;
        }
        for (role, shifted) in self.pending.iter().zip(shifts) {
            for chunk in shifted.chunks(super::POSITIONS_PER_EVENT) {
                let event = if let Some(workspace_id) = self.workspace {
                    Event::WorkspaceRolePositionsShifted {
                        workspace_id,
                        positions: chunk
                            .iter()
                            .map(
                                |&(id, position)| junto_protocol::roles::WorkspaceRolePosition {
                                    role_id: WorkspaceRoleId::from_uuid(id),
                                    position,
                                },
                            )
                            .collect(),
                    }
                } else {
                    Event::ServerRolePositionsShifted {
                        positions: chunk
                            .iter()
                            .map(
                                |&(id, position)| junto_protocol::roles::ServerRolePosition {
                                    role_id: ServerRoleId::from_uuid(id),
                                    position,
                                },
                            )
                            .collect(),
                    }
                };
                events::publish(connection, now, &event).await?;
            }
            match role {
                Creation::Server(role) => {
                    server_roles::announce_creation(connection, access, now, role).await?
                }
                Creation::Workspace(role) => {
                    workspace_roles::announce_creation(connection, access, now, role).await?
                }
            }
        }
        Ok(())
    }
}
