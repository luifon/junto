use axum::Json;
use axum::extract::State;
use junto_authz::Request;
use junto_protocol::ids::WorkspaceId;
use junto_protocol::snapshot::{Snapshot, WorkspaceState};
use junto_protocol::structure::Workspace;

use crate::access::Access;
use crate::error::{ApiError, ApiResult};
use crate::events::log;
use crate::extract::Bearer;
use crate::read;
use crate::routes::{channels, workspace_roles};
use crate::state::AppState;
use crate::store;

/// The state that a client starts from, with the cursor from which the event stream
/// continues it.
#[utoipa::path(
    get,
    path = "/api/v1/snapshot",
    tag = "events",
    security(("session" = [])),
    responses((status = 200, body = Snapshot), ApiError)
)]
pub async fn get_snapshot(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Snapshot>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let log = log::state(connection).await?;
        let account = store::own_account(connection, &access).await?;
        let default_workspace = store::default_workspace(connection).await?;
        let rows: Vec<(WorkspaceId, String)> =
            sqlx::query_as("select id, name from workspaces order by id")
                .fetch_all(&mut *connection)
                .await?;
        let mut workspaces = Vec::new();
        for (id, name) in rows {
            let scope = access.workspace(connection, id).await?;
            if !access.allows(Request::ViewWorkspace(&scope)) {
                continue;
            }
            let structure = channels::visible_structure(connection, &access, id, &scope).await?;
            workspaces.push(WorkspaceState {
                workspace: Workspace {
                    id,
                    name,
                    is_default: id == default_workspace,
                },
                own_role_ids: workspace_roles::role_ids_of(connection, id, access.account_id)
                    .await?,
                roles: workspace_roles::roles_of(connection, id).await?,
                categories: structure.categories,
                channels: structure.channels,
            });
        }
        Ok(Snapshot {
            cursor: log.cursor(log.head)?,
            account,
            workspaces,
        })
    })
    .map(Json)
}
