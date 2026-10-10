use axum::Json;
use axum::extract::State;
use junto_authz::Request;
use junto_protocol::call::ChannelCall;
use junto_protocol::ids::{CallId, ChannelId, WorkspaceId};
use junto_protocol::snapshot::{Snapshot, WorkspaceState};
use junto_protocol::structure::Workspace;

use crate::access::Access;
use crate::calls::admission::may_be_at;
use crate::calls::store::{self as calls, By, INVITATION_COLUMNS};
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
            let in_use: Vec<ChannelId> = sqlx::query_scalar(
                "select channel_id from calls
                 where workspace_id = $1 and started_at is not null",
            )
            .bind(id)
            .fetch_all(&mut *connection)
            .await?;
            let mut channel_calls: Vec<ChannelCall> = Vec::new();
            for channel in &structure.channels {
                if in_use.contains(&channel.id) {
                    let call = super::calls::visible_call(connection, &access, channel).await?;
                    channel_calls.push(call);
                }
            }
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
                calls: channel_calls,
            });
        }

        let member = calls::member_of(connection, access.account_id, false).await?;
        let mut own_call = None;
        if let Some(member) = member
            && let Some(call) = calls::call(connection, By::Id(member.call_id), false).await?
            && may_be_at(connection, &access, &call.place).await?
        {
            own_call = Some(member.own_call(&call));
        }
        let direct_ids: Vec<CallId> = sqlx::query_scalar(
            "select id from calls
             where (direct_low = $1 or direct_high = $1) and started_at is not null
             order by id",
        )
        .bind(access.account_id)
        .fetch_all(&mut *connection)
        .await?;
        let mut direct_calls = Vec::new();
        for call_id in direct_ids {
            if let Some(call) = calls::call(connection, By::Id(call_id), false).await?
                && may_be_at(connection, &access, &call.place).await?
            {
                direct_calls.push(calls::view(connection, &call).await?);
            }
        }
        let invited: Vec<(CallId, _, _, _, _, _)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "select call_id, {INVITATION_COLUMNS} from call_invitations
             where account_id = $1 and expires_at > $2
             order by created_at, id"
        )))
        .bind(access.account_id)
        .bind(now)
        .fetch_all(&mut *connection)
        .await?;
        let mut call_invitations = Vec::new();
        for (call_id, id, account_id, invited_by, created_at, expires_at) in invited {
            if let Some(call) = calls::call(connection, By::Id(call_id), false).await?
                && may_be_at(connection, &access, &call.place).await?
            {
                let row = (id, account_id, invited_by, created_at, expires_at);
                call_invitations.push(calls::invitation(&call, row));
            }
        }
        let direct_mutes = calls::direct_mutes(connection, now, access.account_id).await?;
        Ok(Snapshot {
            cursor: log.cursor(log.head)?,
            account,
            workspaces,
            own_call,
            direct_calls,
            call_invitations,
            direct_mutes,
        })
    })
    .map(Json)
}
