use axum::Json;
use axum::extract::State;
use junto_authz::{Request, WorkspacePermission};
use junto_protocol::call::{CallMove, CallPlace, ReportCallIdle};
use junto_protocol::ids::{CallMoveId, WorkspaceId};
use junto_protocol::structure::{AfkSettings, ChannelKind};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::Access;
use crate::calls::entry::{self, Fresh};
use crate::calls::store::{self, MoveRow};
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::state::AppState;
use crate::{proof, read, transact};

pub(crate) async fn settings(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
) -> ApiResult<AfkSettings> {
    let (channel_id, enabled, minutes): (_, _, i32) = sqlx::query_as(
        "select channel_id, enabled, idle_minutes from workspace_afk where workspace_id = $1",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(ApiError::not_found)?;
    Ok(AfkSettings {
        channel_id,
        enabled,
        idle_minutes: minutes as u32,
    })
}

#[utoipa::path(
    get, path = "/api/v1/workspaces/{workspace_id}/afk", tag = "calls",
    params(("workspace_id" = WorkspaceId, Path)), security(("session" = [])),
    responses((status = 200, body = AfkSettings), ApiError)
)]
pub async fn get_afk(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
) -> ApiResult<Json<AfkSettings>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ViewWorkspace(&scope))?;
        let settings = settings(connection, workspace_id).await?;
        let (_, channel_scope) = access.channel(connection, settings.channel_id).await?;
        access.require(Request::Channel {
            scope: &channel_scope,
            permission: WorkspacePermission::ViewChannel,
        })?;
        Ok(settings)
    })
    .map(Json)
}

#[utoipa::path(
    put, path = "/api/v1/workspaces/{workspace_id}/afk", tag = "calls",
    params(("workspace_id" = WorkspaceId, Path)), request_body = AfkSettings,
    security(("session" = [])), responses((status = 200, body = AfkSettings), ApiError)
)]
pub async fn set_afk(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<AfkSettings>,
) -> ApiResult<Json<AfkSettings>> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        set_in(connection, &app, &access, now, workspace_id, &body).await
    })
    .map(Json)
}

pub(crate) async fn set_in(
    connection: &mut PgConnection,
    app: &AppState,
    access: &Access,
    now: OffsetDateTime,
    workspace_id: WorkspaceId,
    body: &AfkSettings,
) -> ApiResult<AfkSettings> {
    let scope = access.workspace(connection, workspace_id).await?;
    access.require(Request::Workspace {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })?;
    if ![5, 15, 30, 60].contains(&body.idle_minutes) {
        return Err(ApiError::invalid(
            "AFK idle time must be 5, 15, 30 or 60 minutes",
        ));
    }
    let (channel, channel_scope) = access.channel(connection, body.channel_id).await?;
    access.require(Request::Channel {
        scope: &channel_scope,
        permission: WorkspacePermission::ViewChannel,
    })?;
    channel.require_open()?;
    if channel.channel.workspace_id != workspace_id || channel.channel.kind != ChannelKind::Voice {
        return Err(ApiError::invalid(
            "the AFK channel must be a voice channel of this workspace",
        ));
    }
    let meeting: bool = sqlx::query_scalar("select exists(select 1 from calls where channel_id = $1 and meeting_presenter is not null)")
        .bind(body.channel_id).fetch_one(&mut *connection).await?;
    if body.enabled && meeting {
        return Err(ApiError::conflict(
            "end meeting mode before choosing this AFK channel",
        ));
    }
    let before = settings(connection, workspace_id).await?;
    sqlx::query("update workspace_afk set channel_id = $2, enabled = $3, idle_minutes = $4 where workspace_id = $1")
        .bind(workspace_id).bind(body.channel_id).bind(body.enabled).bind(body.idle_minutes as i32)
        .execute(&mut *connection).await?;
    for channel_id in [before.channel_id, body.channel_id] {
        if let Some(call) = store::call(
            connection,
            store::By::Place(CallPlace::Channel {
                workspace_id,
                channel_id,
            }),
            false,
        )
        .await?
        {
            app.calls.mark(call.room.as_str());
        }
    }
    crate::audit::record(
        connection,
        now,
        Some(access.account_id),
        crate::audit::About::Channel(workspace_id, body.channel_id),
        &junto_protocol::audit::AuditEvent::WorkspaceAfkSet {
            workspace_id,
            settings: body.clone(),
        },
    )
    .await?;
    if before != *body {
        events::publish(
            connection,
            now,
            &junto_protocol::events::Event::WorkspaceAfkChanged { workspace_id },
        )
        .await?;
    }
    Ok(body.clone())
}

pub(crate) async fn is_afk(connection: &mut PgConnection, place: &CallPlace) -> ApiResult<bool> {
    let CallPlace::Channel {
        workspace_id,
        channel_id,
    } = place
    else {
        return Ok(false);
    };
    Ok(sqlx::query_scalar("select exists(select 1 from workspace_afk where workspace_id = $1 and channel_id = $2 and enabled)")
        .bind(workspace_id).bind(channel_id).fetch_one(&mut *connection).await?)
}

pub(crate) async fn still_allowed(
    connection: &mut PgConnection,
    from: &CallPlace,
    to: &CallPlace,
) -> ApiResult<bool> {
    let (
        CallPlace::Channel {
            workspace_id,
            channel_id,
        },
        CallPlace::Channel {
            workspace_id: target_workspace,
            ..
        },
    ) = (from, to)
    else {
        return Ok(false);
    };
    if workspace_id != target_workspace || from == to || !is_afk(connection, to).await? {
        return Ok(false);
    }
    let meeting: bool = sqlx::query_scalar("select exists(select 1 from calls where channel_id = $1 and meeting_presenter is not null)")
        .bind(channel_id).fetch_one(&mut *connection).await?;
    Ok(!meeting)
}

#[utoipa::path(
    post, path = "/api/v1/call/idle", tag = "calls", request_body = ReportCallIdle,
    security(("session" = [])), responses((status = 200, body = CallMove), ApiError)
)]
pub async fn report_idle(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<ReportCallIdle>,
) -> ApiResult<Json<CallMove>> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let idle_seconds = body
        .system_idle_seconds
        .min(body.microphone_silent_seconds)
        .min(3600) as i32;
    let fresh = Fresh::new(&app)?;
    let id = CallMoveId::generate();
    let expires_at = now + app.config.limits.calls.move_lifetime;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        let (from, member) = super::calls::own(connection, &access).await?;
        if from.id != body.call_id || member.identity != body.identity || member.connected_at.is_none() { return Err(entry::not_in_call()) }
        let CallPlace::Channel { workspace_id, .. } = from.place else { return Err(ApiError::conflict("this call has no AFK channel")) };
        let afk = settings(connection, workspace_id).await?;
        let to_place = CallPlace::Channel { workspace_id, channel_id: afk.channel_id };
        if !still_allowed(connection, &from.place, &to_place).await?
            || body.system_idle_seconds < afk.idle_minutes * 60
            || body.microphone_silent_seconds < afk.idle_minutes * 60 {
            return Err(ApiError::conflict("the call is not eligible for an AFK move"));
        }
        let destination = super::calls::voice(connection, &access, afk.channel_id).await?;
        if !crate::calls::admission::may_be_at(connection, &access, &destination.place).await? {
            return Err(ApiError::new(junto_protocol::error::ErrorCode::Forbidden, "the AFK channel cannot be joined"));
        }
        let to = store::call_for_entry(connection, now, to_place, fresh.call_id, &fresh.room).await?;
        app.calls.admitting_into(&to.room)?;
        let identity = fresh.identity(access.account_id);
        sqlx::query("insert into call_moves (id, account_id, from_call_id, to_call_id, moved_by, identity, created_at, expires_at, automatic_afk, afk_idle_seconds) values ($1,$2,$3,$4,$2,$5,$6,$7,true,$8)")
            .bind(id).bind(access.account_id).bind(from.id).bind(to.id).bind(&identity).bind(now).bind(expires_at).bind(idle_seconds)
            .execute(&mut *connection).await.map_err(ApiError::concurrent)?;
        let pending = MoveRow { afk_idle_seconds: Some(idle_seconds), id, account_id: access.account_id, from_call_id: from.id, to_call_id: to.id, moved_by: access.account_id, identity, expires_at, automatic_afk: true };
        let call_move = store::call_move(&pending, from.place, to.place);
        events::tell_account(connection, now, access.account_id, &junto_protocol::events::Event::CallMoveRequested { call_move: call_move.clone() }).await?;
        Ok(call_move)
    }).map(Json)
}

pub(crate) async fn require_replace_before_removal(
    connection: &mut PgConnection,
    channel_id: junto_protocol::ids::ChannelId,
) -> ApiResult<()> {
    let selected: bool =
        sqlx::query_scalar("select exists(select 1 from workspace_afk where channel_id = $1)")
            .bind(channel_id)
            .fetch_one(connection)
            .await?;
    if selected {
        return Err(ApiError::conflict(
            "choose another AFK channel before removing this channel",
        ));
    }
    Ok(())
}

pub(crate) async fn pending_allowed(
    connection: &mut PgConnection,
    pending: &MoveRow,
    from: &CallPlace,
    to: &CallPlace,
) -> ApiResult<bool> {
    if !still_allowed(connection, from, to).await? {
        return Ok(false);
    }
    let CallPlace::Channel { workspace_id, .. } = to else {
        return Ok(false);
    };
    let current = settings(connection, *workspace_id).await?;
    Ok(pending.afk_idle_seconds.unwrap_or(0) >= current.idle_minutes as i32 * 60)
}
