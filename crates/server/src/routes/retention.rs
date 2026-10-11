//! Administrator-owned retention policies; absent days means never.
use crate::{
    access::Access,
    audit::{self, About},
    error::{ApiError, ApiResult},
    events,
    extract::{ApiJson, ApiPath, Bearer},
    read,
    state::AppState,
    transact,
};
use axum::{Json, extract::State};
use junto_authz::{Request, ServerPermission};
use junto_protocol::{audit::AuditEvent, discovery::Retention, events::Event, ids::WorkspaceId};

async fn get(
    app: &AppState,
    bearer: &Bearer,
    id: Option<WorkspaceId>,
    recordings: bool,
) -> ApiResult<Retention> {
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, bearer, app.now()).await?;
        if let Some(id) = id {
            let scope = access.workspace(connection, id).await?;
            access.require(Request::ViewWorkspace(&scope))?;
        }
        access.require(Request::Server(ServerPermission::Administrator))?;
        super::search::read_rate(app, &access, &mut counted)?;
        let days: Option<i32> = if let Some(id) = id {
            sqlx::query_scalar(if recordings {
                "select days from workspace_recording_retention where workspace_id=$1"
            } else {
                "select days from workspace_file_retention where workspace_id=$1"
            })
            .bind(id)
            .fetch_optional(connection)
            .await?
            .flatten()
        } else {
            sqlx::query_scalar("select days from direct_message_retention")
                .fetch_one(connection)
                .await?
        };
        Ok(Retention {
            days: days.map(|days| days as u32),
        })
    })
}

async fn set(
    app: &AppState,
    bearer: &Bearer,
    id: Option<WorkspaceId>,
    body: &Retention,
    recordings: bool,
) -> ApiResult<Retention> {
    if body.days.is_some_and(|days| !(1..=36500).contains(&days)) {
        return Err(ApiError::invalid(
            "retention days must be 1 to 36500 or null",
        ));
    }
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, bearer, app.now()).await?;
        if let Some(id) = id {
            let scope = access.workspace(connection, id).await?;
            access.require(Request::ViewWorkspace(&scope))?;
        }
        access.require(Request::Server(ServerPermission::Administrator))?;
        super::messages::rate(app, &access, &mut counted)?;
        let changed = if let Some(id) = id {
            sqlx::query(if recordings {
                "insert into workspace_recording_retention(workspace_id,days) values($1,$2) on conflict(workspace_id) do
             update set days=excluded.days where workspace_recording_retention.days is distinct from excluded.days"
            } else {
                "insert into workspace_file_retention(workspace_id,days) values($1,$2) on conflict(workspace_id) do update
             set days=excluded.days where workspace_file_retention.days is distinct from excluded.days"
            })
            .bind(id)
            .bind(body.days.map(|days| days as i32))
            .execute(&mut *connection)
            .await?
            .rows_affected()
        } else {
            sqlx::query(
                "update direct_message_retention set days=$1 where days is distinct from $1",
            )
            .bind(body.days.map(|days| days as i32))
            .execute(&mut *connection)
            .await?
            .rows_affected()
        };
        if changed > 0 {
            audit::record(
                connection,
                app.now(),
                Some(access.account_id),
                id.map_or(About::Server, About::Workspace),
                &if recordings {
                    AuditEvent::RecordingRetentionChanged { days: body.days }
                } else {
                    AuditEvent::RetentionChanged { days: body.days }
                },
            )
            .await?;
            events::publish(
                connection,
                app.now(),
                &Event::RetentionChanged { workspace_id: id },
            )
            .await?;
        }
        Ok(body.clone())
    })
}

#[utoipa::path(get,path="/api/v1/workspaces/{workspace_id}/file-retention",tag="messages",params(("workspace_id"=WorkspaceId,Path)),security(("session"=[])),responses((status=200,body=Retention),ApiError))]
pub async fn get_workspace(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<WorkspaceId>,
) -> ApiResult<Json<Retention>> {
    get(&app, &bearer, Some(id), false).await.map(Json)
}

#[utoipa::path(put,path="/api/v1/workspaces/{workspace_id}/file-retention",tag="messages",params(("workspace_id"=WorkspaceId,Path)),request_body=Retention,security(("session"=[])),responses((status=200,body=Retention),ApiError))]
pub async fn set_workspace(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<Retention>,
) -> ApiResult<Json<Retention>> {
    set(&app, &bearer, Some(id), &body, false).await.map(Json)
}

#[utoipa::path(get,path="/api/v1/direct-message-retention",tag="messages",security(("session"=[])),responses((status=200,body=Retention),ApiError))]
pub async fn get_direct(State(app): State<AppState>, bearer: Bearer) -> ApiResult<Json<Retention>> {
    get(&app, &bearer, None, false).await.map(Json)
}

#[utoipa::path(put,path="/api/v1/direct-message-retention",tag="messages",request_body=Retention,security(("session"=[])),responses((status=200,body=Retention),ApiError))]
pub async fn set_direct(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<Retention>,
) -> ApiResult<Json<Retention>> {
    set(&app, &bearer, None, &body, false).await.map(Json)
}

#[utoipa::path(get,path="/api/v1/workspaces/{workspace_id}/recording-retention",tag="messages",params(("workspace_id"=WorkspaceId,Path)),security(("session"=[])),responses((status=200,body=Retention),ApiError))]
pub async fn get_recording(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<WorkspaceId>,
) -> ApiResult<Json<Retention>> {
    get(&app, &bearer, Some(id), true).await.map(Json)
}

#[utoipa::path(put,path="/api/v1/workspaces/{workspace_id}/recording-retention",tag="messages",params(("workspace_id"=WorkspaceId,Path)),request_body=Retention,security(("session"=[])),responses((status=200,body=Retention),ApiError))]
pub async fn set_recording(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<Retention>,
) -> ApiResult<Json<Retention>> {
    set(&app, &bearer, Some(id), &body, true).await.map(Json)
}
