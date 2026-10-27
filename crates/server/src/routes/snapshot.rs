use axum::extract::State;
use axum::{Json, http::HeaderMap};
use junto_authz::Request;
use junto_protocol::call::{CallMove, ChannelCall, EntryRequest};
use junto_protocol::ids::{CallId, ChannelId, WorkspaceId};
use junto_protocol::snapshot::{Snapshot, SnapshotResponse, WorkspaceState};
use junto_protocol::structure::Workspace;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::Access;
use crate::calls::admission::may_be_at;
use crate::calls::store::{
    self as calls, By, ENTRY_COLUMNS, INVITATION_COLUMNS, MOVE_COLUMNS, MoveRow,
};
use crate::error::{ApiError, ApiResult};
use crate::events::log;
use crate::extract::Bearer;
use crate::read;
use crate::routes::{channels, workspace_roles};
use crate::state::AppState;
use crate::store;

/// The most invitations of one side, moves and requests that a snapshot carries. The rules
/// of calls keep an account far below it: an account is in one call, and a call has at most
/// 50 invitations that wait.
const MOST_WAITING: i64 = 200;

/// The moves that wait and that name the account as the moved person or as the person who
/// moved, while the account may see both voice channels (ADR 0511).
async fn waiting_moves(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
) -> ApiResult<Vec<CallMove>> {
    let rows: Vec<MoveRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {MOVE_COLUMNS} from (
             select * from call_moves where account_id = $1
             union
             select * from call_moves where moved_by = $1
         ) as moves
         where expires_at > $2
         order by created_at, id
         limit $3"
    )))
    .bind(access.account_id)
    .bind(now)
    .bind(MOST_WAITING)
    .fetch_all(&mut *connection)
    .await?;
    let mut moves = Vec::new();
    for pending in rows {
        let from = calls::call(connection, By::Id(pending.from_call_id), false).await?;
        let to = calls::call(connection, By::Id(pending.to_call_id), false).await?;
        if let (Some(from), Some(to)) = (from, to)
            && may_be_at(connection, access, &from.place).await?
            && may_be_at(connection, access, &to.place).await?
        {
            moves.push(calls::call_move(&pending, from.place, to.place));
        }
    }
    Ok(moves)
}

/// The requests of the account to enter a voice channel that wait for an answer, while the
/// account may see the channel (ADR 0514).
async fn own_entry_requests(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
) -> ApiResult<Vec<EntryRequest>> {
    let rows: Vec<(CallId, _, _, _, _)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select call_id, {ENTRY_COLUMNS} from call_entry_requests
         where account_id = $1 and accepted_by is null and expires_at > $2
         order by created_at, id
         limit $3"
    )))
    .bind(access.account_id)
    .bind(now)
    .bind(MOST_WAITING)
    .fetch_all(&mut *connection)
    .await?;
    let mut requests = Vec::new();
    for (call_id, id, account_id, created_at, expires_at) in rows {
        if let Some(call) = calls::call(connection, By::Id(call_id), false).await?
            && may_be_at(connection, access, &call.place).await?
        {
            let row = (id, account_id, created_at, expires_at);
            requests.push(calls::entry_request(&call, row));
        }
    }
    Ok(requests)
}

/// The state that a client starts from, with the cursor from which the event stream
/// continues it.
#[utoipa::path(
    get,
    path = "/api/v1/snapshot",
    tag = "events",
    security(("session" = [])),
    params(("Junto-Notification-Preferences" = Option<String>, Header, description = "1 includes the text privacy and presentation preferences")),
    responses((status = 200, body = SnapshotResponse), ApiError)
)]
pub async fn get_snapshot(
    State(app): State<AppState>,
    bearer: Bearer,
    headers: HeaderMap,
) -> ApiResult<Json<SnapshotResponse>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let log = log::state(connection).await?;
        let account = store::own_account(connection, &access).await?;
        let default_workspace = store::default_workspace(connection).await?;
        let rows: Vec<(WorkspaceId, String)> =
            sqlx::query_as("select id, name from workspaces where deleted_at is null order by id")
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
            for channel in &structure.list.channels {
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
                categories: structure.list.categories,
                channels: structure.list.channels,
                calls: channel_calls,
                permissions: structure.permissions,
                channel_permissions: structure.channel_permissions,
                notification_default: None,
            });
        }
        let seen: Vec<WorkspaceId> = workspaces.iter().map(|state| state.workspace.id).collect();
        let defaults: Vec<(WorkspaceId, String)> = sqlx::query_as(
            "select workspace_id, level from workspace_notification_defaults
             where workspace_id = any($1)",
        )
        .bind(&seen)
        .fetch_all(&mut *connection)
        .await?;
        for (workspace_id, level) in defaults {
            if let Some(state) = workspaces
                .iter_mut()
                .find(|state| state.workspace.id == workspace_id)
            {
                state.notification_default =
                    Some(crate::messages::notifications::stored_level(&level)?);
            }
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
        let mut call_invitations = Vec::new();
        let mut sent_call_invitations = Vec::new();
        for (side, list) in [
            ("account_id = $1 and not silent", &mut call_invitations),
            ("invited_by = $1", &mut sent_call_invitations),
        ] {
            let waiting: Vec<(CallId, _, _, _, _, _, _)> =
                sqlx::query_as(sqlx::AssertSqlSafe(format!(
                    "select call_id, {INVITATION_COLUMNS} from call_invitations
                     where {side} and expires_at > $2
                     order by created_at, id
                     limit $3"
                )))
                .bind(access.account_id)
                .bind(now)
                .bind(MOST_WAITING)
                .fetch_all(&mut *connection)
                .await?;
            for (call_id, id, account_id, invited_by, created_at, expires_at, silent) in waiting {
                if let Some(call) = calls::call(connection, By::Id(call_id), false).await?
                    && may_be_at(connection, &access, &call.place).await?
                {
                    let row = (id, account_id, invited_by, created_at, expires_at, silent);
                    list.push(calls::invitation(&call, row).invitation);
                }
            }
        }
        let call_moves = waiting_moves(connection, &access, now).await?;
        let entry_requests = own_entry_requests(connection, &access, now).await?;
        let direct_mutes = calls::direct_mutes(connection, now, access.account_id).await?;
        let messages = crate::messages::snapshot(connection, &access, &workspaces, now).await?;
        let allowed = crate::access::visibility::places(connection, &access).await?;
        let threads = crate::messages::threads::unread(
            connection,
            access.account_id,
            &allowed.channels,
            None,
            200,
        )
        .await?;
        let notices =
            super::moderation_notices::page(connection, now, Some(access.account_id), None, None)
                .await?;
        let snapshot = Snapshot {
            cursor: log.cursor(log.head)?,
            account,
            workspaces,
            own_call,
            direct_calls,
            call_invitations,
            sent_call_invitations,
            call_moves,
            entry_requests,
            direct_mutes,
            messages,
            threads: threads.threads,
            threads_more: threads.more,
            threads_next: threads.next,
            notifications: crate::messages::notifications::visible_in(
                connection, &access, now, None, None, 50, &allowed,
            )
            .await?,
            notification_settings: crate::messages::notifications::settings(
                connection, &access, &allowed,
            )
            .await?,
        };
        Ok(SnapshotResponse {
            snapshot,
            moderation_notices: notices.notices,
            moderation_notices_next: notices.next_before,
        })
    })
    .map(|mut response| {
        let extended = super::notifications::extended_preferences(&headers);
        for scope in &mut response.snapshot.notification_settings {
            scope.settings =
                super::notifications::compatible_settings(scope.settings.clone(), extended);
        }
        Json(response)
    })
}
