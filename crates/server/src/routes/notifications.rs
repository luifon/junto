//! Account notification settings and notice read positions.
use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    events,
    extract::{ApiJson, ApiPath, ApiQuery, Bearer},
    messages::{Place, notifications},
    read,
    state::AppState,
    transact,
};
use axum::{Json, extract::State};
use junto_authz::{Request, ServerPermission};
use junto_protocol::{
    discovery::{
        Notification, NotificationDefault, NotificationLevel, NotificationPage,
        NotificationSettings,
    },
    events::Event,
    ids::{ChannelId, MessageId, WorkspaceId},
};
use serde::Deserialize;
use sqlx::PgConnection;
use uuid::Uuid;

async fn scope(connection: &mut PgConnection, access: &Access, id: Uuid) -> ApiResult<bool> {
    if id.is_nil() {
        access.require(Request::OwnAccount)?;
        return Ok(false);
    }
    let workspace: bool = sqlx::query_scalar("select exists(select 1 from workspaces where id=$1)")
        .bind(id)
        .fetch_one(&mut *connection)
        .await?;
    if workspace {
        let scope = access
            .workspace(connection, WorkspaceId::from_uuid(id))
            .await?;
        access.require(Request::ViewWorkspace(&scope))?;
        Ok(false)
    } else {
        Place::channel(connection, access, ChannelId::from_uuid(id))
            .await
            .map(|_| true)
    }
}

#[utoipa::path(get,path="/api/v1/notification-settings/{scope_id}",tag="messages",params(("scope_id"=Uuid,Path,description="Nil UUID for account/server defaults, otherwise workspace or channel")),security(("session"=[])),responses((status=200,body=NotificationSettings),ApiError))]
pub async fn get_settings(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<NotificationSettings>> {
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        super::search::read_rate(&app, &access, &mut counted)?;
        scope(connection, &access, id).await?;
        let value: Option<sqlx::types::Json<NotificationSettings>> = sqlx::query_scalar(
            "select settings from notification_settings where account_id=$1 and scope_id=$2",
        )
        .bind(access.account_id)
        .bind(id)
        .fetch_optional(connection)
        .await?;
        Ok(value.map(|value| value.0).unwrap_or_default())
    })
    .map(Json)
}

#[utoipa::path(put,path="/api/v1/notification-settings/{scope_id}",tag="messages",params(("scope_id"=Uuid,Path)),request_body=NotificationSettings,security(("session"=[])),responses((status=200,body=NotificationSettings),ApiError))]
pub async fn set_settings(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<Uuid>,
    ApiJson(body): ApiJson<NotificationSettings>,
) -> ApiResult<Json<NotificationSettings>> {
    if body.daily_quiet.as_ref().is_some_and(|quiet| {
        quiet.start_minute == quiet.end_minute
            || quiet.start_minute >= 1440
            || quiet.end_minute >= 1440
            || !(-840..=840).contains(&quiet.utc_offset_minutes)
    }) || body.muted_until.is_some_and(|until| until <= app.now())
        || (!body.mute && body.muted_until.is_some())
    {
        return Err(ApiError::invalid("invalid notification settings"));
    }
    if !id.is_nil() && (body.do_not_disturb || body.daily_quiet.is_some()) {
        return Err(ApiError::invalid("do not disturb is an account setting"));
    }
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        let channel = scope(connection, &access, id).await?;
        if !channel
            && (body.mute
                || body.muted_until.is_some()
                || body.role_mentions.is_some()
                || body.everyone_mentions.is_some())
        {
            return Err(ApiError::invalid(
                "mute and mention switches are channel settings",
            ));
        }
        super::messages::rate(&app, &access, &mut counted)?;
        let changed = sqlx::query(
            "insert into notification_settings(account_id,scope_id,settings) values($1,$2,$3) on
             conflict(account_id,scope_id) do update set settings=excluded.settings where
             notification_settings.settings<>excluded.settings",
        )
        .bind(access.account_id)
        .bind(id)
        .bind(sqlx::types::Json(&body))
        .execute(&mut *connection)
        .await?
        .rows_affected();
        if changed > 0 {
            events::tell_account(
                connection,
                app.now(),
                access.account_id,
                &Event::NotificationSettingsChanged {},
            )
            .await?;
        }
        Ok(body.clone())
    })
    .map(Json)
}

#[utoipa::path(get,path="/api/v1/workspaces/{workspace_id}/notification-default",tag="messages",params(("workspace_id"=WorkspaceId,Path)),security(("session"=[])),responses((status=200,body=NotificationDefault),ApiError))]
pub async fn get_default(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<WorkspaceId>,
) -> ApiResult<Json<NotificationDefault>> {
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        super::search::read_rate(&app, &access, &mut counted)?;
        scope(connection, &access, id.as_uuid()).await?;
        let value: Option<String> = sqlx::query_scalar(
            "select level from workspace_notification_defaults where workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(connection)
        .await?;
        Ok(NotificationDefault {
            level: match value.as_deref() {
                Some("all") => NotificationLevel::All,
                Some("nothing") => NotificationLevel::Nothing,
                _ => NotificationLevel::Mentions,
            },
        })
    })
    .map(Json)
}

#[utoipa::path(put,path="/api/v1/workspaces/{workspace_id}/notification-default",tag="messages",params(("workspace_id"=WorkspaceId,Path)),request_body=NotificationDefault,security(("session"=[])),responses((status=200,body=NotificationDefault),ApiError))]
pub async fn set_default(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<NotificationDefault>,
) -> ApiResult<Json<NotificationDefault>> {
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        scope(connection, &access, id.as_uuid()).await?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        super::messages::rate(&app, &access, &mut counted)?;
        let level = match body.level {
            NotificationLevel::All => "all",
            NotificationLevel::Mentions => "mentions",
            NotificationLevel::Nothing => "nothing",
        };
        let changed = sqlx::query("insert into workspace_notification_defaults(workspace_id,level) values($1,$2) on conflict(workspace_id) do
             update set level=excluded.level where workspace_notification_defaults.level<>excluded.level")
            .bind(id)
            .bind(level)
            .execute(&mut *connection)
            .await?
            .rows_affected();
        if changed > 0 {
            crate::audit::record(
                connection,
                app.now(),
                Some(access.account_id),
                crate::audit::About::Workspace(id),
                &junto_protocol::audit::AuditEvent::NotificationDefaultChanged {
                    level: body.level,
                },
            )
            .await?;
            // The default is a setting of the workspace: the accounts that see it are told.
            events::log::record(
                connection,
                app.now(),
                &events::audience::Audience::Workspace { workspace_id: id },
                &Event::NotificationSettingsChanged {},
            )
            .await?;
        }
        Ok(body.clone())
    })
    .map(Json)
}

#[derive(Deserialize, utoipa::IntoParams)]
pub struct Page {
    pub before: Option<Uuid>,
    pub limit: Option<u32>,
}

#[utoipa::path(get,path="/api/v1/notifications",tag="messages",params(Page),security(("session"=[])),responses((status=200,body=NotificationPage),ApiError))]
pub async fn list(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiQuery(page): ApiQuery<Page>,
) -> ApiResult<Json<NotificationPage>> {
    let limit = page.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::invalid("limit must be 1 to 200"));
    }
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        super::search::read_rate(&app, &access, &mut counted)?;
        let mut notifications = notifications::visible(
            connection,
            &access,
            app.now(),
            None,
            page.before,
            i64::from(limit) + 1,
        )
        .await?;
        let has_more = notifications.len() > limit as usize;
        notifications.truncate(limit as usize);
        Ok(NotificationPage {
            notifications,
            has_more,
        })
    })
    .map(Json)
}

#[utoipa::path(put,path="/api/v1/notifications/{notification_id}/read",tag="messages",params(("notification_id"=Uuid,Path)),security(("session"=[])),responses((status=200,body=Notification),ApiError))]
pub async fn mark_read(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<Uuid>,
) -> ApiResult<Json<Notification>> {
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        super::messages::rate(&app, &access, &mut counted)?;
        let mut notices = notifications::visible(
            connection,
            &access,
            app.now(),
            Some(&[MessageId::from_uuid(id)]),
            None,
            1,
        )
        .await?;
        let mut notice = notices.pop().ok_or_else(ApiError::not_found)?;
        if notice.read_at.is_none() {
            sqlx::query("update notifications set read_at=$3 where account_id=$1 and id=$2")
                .bind(access.account_id)
                .bind(id)
                .bind(app.now())
                .execute(&mut *connection)
                .await?;
            notice.read_at = Some(app.now());
            notifications::publish(
                connection,
                app.now(),
                notice.message_id,
                &[(access.account_id, true)],
                false,
            )
            .await?;
        }
        Ok(notice)
    })
    .map(Json)
}
