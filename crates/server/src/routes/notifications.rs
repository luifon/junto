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
use axum::http::{HeaderMap, StatusCode};
use axum::{Json, extract::State};
use junto_authz::{Request, ServerPermission};
use junto_protocol::{
    audit::AuditEvent,
    discovery::{
        MarkNotificationsRead, Notification, NotificationDefault, NotificationPage,
        NotificationSettings, NotificationSettingsRequest, NotificationsRead,
    },
    events::Event,
    ids::{ChannelId, MessageId, WorkspaceId},
};
use serde::Deserialize;
use sqlx::PgConnection;
use uuid::Uuid;

/// The longest name of a time zone that a quiet time states. The names of the IANA
/// database are shorter.
const LONGEST_ZONE_NAME: usize = 64;

pub(super) fn extended_preferences(headers: &HeaderMap) -> bool {
    headers
        .get("Junto-Notification-Preferences")
        .is_some_and(|value| value == "1")
}

pub(super) fn compatible_settings(
    mut settings: NotificationSettings,
    extended: bool,
) -> NotificationSettings {
    if !extended {
        settings.hide_text = false;
        settings.hold_while_presenting = None;
    }
    settings
}

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

#[utoipa::path(get,path="/api/v1/notification-settings/{scope_id}",tag="messages",params(("scope_id"=Uuid,Path,description="Nil UUID for account/server defaults, otherwise workspace or channel"),("Junto-Notification-Preferences"=Option<String>,Header,description="1 includes the text privacy and presentation preferences")),security(("session"=[])),responses((status=200,body=NotificationSettings),ApiError))]
pub async fn get_settings(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<Uuid>,
    headers: HeaderMap,
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
        Ok(compatible_settings(
            value.map(|value| value.0).unwrap_or_default(),
            extended_preferences(&headers),
        ))
    })
    .map(Json)
}

#[utoipa::path(put,path="/api/v1/notification-settings/{scope_id}",tag="messages",params(("scope_id"=Uuid,Path),("Junto-Notification-Preferences"=Option<String>,Header,description="1 replaces the text privacy and presentation preferences; otherwise they are preserved")),request_body=NotificationSettingsRequest,security(("session"=[])),responses((status=200,body=NotificationSettings),ApiError))]
pub async fn set_settings(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<Uuid>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<NotificationSettingsRequest>,
) -> ApiResult<Json<NotificationSettings>> {
    let body = request.settings;
    let extended = extended_preferences(&headers);
    if body.daily_quiet.as_ref().is_some_and(|quiet| {
        quiet.start_minute == quiet.end_minute
            || quiet.start_minute >= 1440
            || quiet.end_minute >= 1440
            || quiet.time_zone.is_empty()
            || quiet.time_zone.len() > LONGEST_ZONE_NAME
    }) || body.muted_until.is_some_and(|until| until <= app.now())
        || (!body.mute && body.muted_until.is_some())
    {
        return Err(ApiError::invalid("invalid notification settings"));
    }
    if !id.is_nil()
        && (body.do_not_disturb
            || body.daily_quiet.is_some()
            || body.hide_text
            || body.hold_while_presenting.is_some())
    {
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
        if let Some(quiet) = &body.daily_quiet {
            // The names that the database lists are the zones whose clock it can read
            // (ADR 0412). Another text is refused, also one that PostgreSQL would read as
            // a rule of its own.
            let known: bool =
                sqlx::query_scalar("select exists(select 1 from pg_timezone_names where name=$1)")
                    .bind(&quiet.time_zone)
                    .fetch_one(&mut *connection)
                    .await?;
            if !known {
                return Err(ApiError::invalid(
                    "the server does not know the time zone of the quiet time",
                ));
            }
        }
        let body = compatible_settings(body.clone(), extended);
        let changed = sqlx::query(
            "insert into notification_settings(account_id,scope_id,settings) values($1,$2,$3) on
             conflict(account_id,scope_id) do update set settings=excluded.settings ||
             case when $4 then '{}'::jsonb else jsonb_strip_nulls(jsonb_build_object(
                 'hide_text', notification_settings.settings->'hide_text',
                 'hold_while_presenting', notification_settings.settings->'hold_while_presenting')) end
             where notification_settings.settings is distinct from (excluded.settings ||
             case when $4 then '{}'::jsonb else jsonb_strip_nulls(jsonb_build_object(
                 'hide_text', notification_settings.settings->'hide_text',
                 'hold_while_presenting', notification_settings.settings->'hold_while_presenting')) end)",
        )
        .bind(access.account_id)
        .bind(id)
        .bind(sqlx::types::Json(&body))
        .bind(extended)
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
        Ok(compatible_settings(body, extended))
    })
    .map(Json)
}

/// The notification default of a workspace. `level` is absent when none is set.
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
        default_workspace(connection, &access, id).await?;
        let value: Option<String> = sqlx::query_scalar(
            "select level from workspace_notification_defaults where workspace_id=$1",
        )
        .bind(id)
        .fetch_optional(connection)
        .await?;
        Ok(NotificationDefault {
            level: value
                .as_deref()
                .map(notifications::stored_level)
                .transpose()?,
        })
    })
    .map(Json)
}

/// The caller of a change of the notification default of a workspace: an administrator
/// who sees the workspace.
/// The workspace whose notification default a request names: one that the caller sees.
/// Another identifier, the one of a channel included, gets `not_found`.
async fn default_workspace(
    connection: &mut PgConnection,
    access: &Access,
    id: WorkspaceId,
) -> ApiResult<()> {
    let scope = access.workspace(connection, id).await?;
    access.require(Request::ViewWorkspace(&scope))
}

async fn default_changer(
    app: &AppState,
    connection: &mut PgConnection,
    bearer: &Bearer,
    id: WorkspaceId,
) -> ApiResult<Access> {
    let access = Access::open(connection, bearer, app.now()).await?;
    default_workspace(connection, &access, id).await?;
    access.require(Request::Server(ServerPermission::Administrator))?;
    Ok(access)
}

/// Records a change of the notification default of a workspace for the accounts that see
/// the workspace. The event carries the level, so no client reads it again.
async fn default_changed(
    connection: &mut PgConnection,
    now: time::OffsetDateTime,
    access: &Access,
    id: WorkspaceId,
    body: &NotificationDefault,
) -> ApiResult<()> {
    let entry = match body.level {
        Some(level) => AuditEvent::NotificationDefaultChanged { level },
        None => AuditEvent::NotificationDefaultRemoved,
    };
    let about = crate::audit::About::Workspace(id);
    crate::audit::record(connection, now, Some(access.account_id), about, &entry).await?;
    let changed = Event::NotificationDefaultChanged {
        workspace_id: id,
        level: body.level,
    };
    events::publish(connection, now, &changed).await
}

/// Sets the notification default of a workspace. The body states a level; `DELETE` on the
/// same path removes the default.
#[utoipa::path(put,path="/api/v1/workspaces/{workspace_id}/notification-default",tag="messages",params(("workspace_id"=WorkspaceId,Path)),request_body=NotificationDefault,security(("session"=[])),responses((status=200,body=NotificationDefault),ApiError))]
pub async fn set_default(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<NotificationDefault>,
) -> ApiResult<Json<NotificationDefault>> {
    let Some(level) = body.level else {
        return Err(ApiError::invalid(
            "state a level; DELETE removes the default of the workspace",
        ));
    };
    let mut counted = false;
    transact!(app, |connection| {
        let access = default_changer(&app, connection, &bearer, id).await?;
        super::messages::rate(&app, &access, &mut counted)?;
        let changed = sqlx::query("insert into workspace_notification_defaults(workspace_id,level) values($1,$2) on conflict(workspace_id) do
             update set level=excluded.level where workspace_notification_defaults.level<>excluded.level")
            .bind(id)
            .bind(notifications::level_name(level))
            .execute(&mut *connection)
            .await?
            .rows_affected();
        if changed > 0 {
            default_changed(connection, app.now(), &access, id, &body).await?;
        }
        Ok(body.clone())
    })
    .map(Json)
}

/// Removes the notification default of a workspace. The workspace then has none: the
/// default of each account, or `mentions`, applies (ADR 0412).
#[utoipa::path(delete,path="/api/v1/workspaces/{workspace_id}/notification-default",tag="messages",params(("workspace_id"=WorkspaceId,Path)),security(("session"=[])),responses((status=204),ApiError))]
pub async fn remove_default(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<WorkspaceId>,
) -> ApiResult<StatusCode> {
    let mut counted = false;
    transact!(app, |connection| {
        let access = default_changer(&app, connection, &bearer, id).await?;
        super::messages::rate(&app, &access, &mut counted)?;
        let removed =
            sqlx::query("delete from workspace_notification_defaults where workspace_id=$1")
                .bind(id)
                .execute(&mut *connection)
                .await?
                .rows_affected();
        if removed > 0 {
            let none = NotificationDefault { level: None };
            default_changed(connection, app.now(), &access, id, &none).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
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

/// Marks every notice that the caller reads now and that was created at or before `until`,
/// in one request (ADR 0412). The devices of the account get one event.
#[utoipa::path(put,path="/api/v1/notifications/read",tag="messages",request_body=MarkNotificationsRead,security(("session"=[])),responses((status=200,body=NotificationsRead),ApiError))]
pub async fn mark_all_read(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<MarkNotificationsRead>,
) -> ApiResult<Json<NotificationsRead>> {
    let now = app.now();
    let until = body.until.map_or(now, |until| until.min(now));
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::OwnAccount)?;
        super::messages::rate(&app, &access, &mut counted)?;
        let places = crate::access::visibility::places(connection, &access).await?;
        let marked = notifications::mark_all_read(connection, &access, now, until, &places).await?;
        if marked > 0 {
            let read = Event::NotificationsRead {
                until,
                read_at: now,
            };
            events::tell_account(connection, now, access.account_id, &read).await?;
        }
        Ok(NotificationsRead {
            until,
            read_at: now,
            marked: u32::try_from(marked).unwrap_or(u32::MAX),
        })
    })
    .map(Json)
}
