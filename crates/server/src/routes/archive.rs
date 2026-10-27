//! Archiving and restoring a channel, the archived channels of a workspace and its archive
//! period (ADR 0410).

use axum::Json;
use axum::extract::State;
use junto_authz::{Request, ServerPermission, WorkspacePermission};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{ChannelId, WorkspaceId};
use junto_protocol::structure::{ArchivePeriod, ArchivedChannel, Channel};

use crate::access::Access;
use crate::archive::{self, MOST_DAYS, NOTICE_DAYS};
use crate::audit::{self, About};
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::state::AppState;
use crate::{read, transact};

/// Archives a channel: it becomes read-only and leaves the channel list, and the call of a
/// voice channel ends. The caller holds `manage_channel` in the channel. A channel that is
/// archived already stays as it is.
#[utoipa::path(
    put,
    path = "/api/v1/channels/{channel_id}/archive",
    tag = "channels",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 200, body = ArchivedChannel), ApiError)
)]
pub async fn archive_channel(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<Json<ArchivedChannel>> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (mut record, scope) = access.channel(connection, channel_id).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ManageChannel,
        })?;
        let workspace_id = record.channel.workspace_id;
        let days = archive::period(connection, workspace_id).await?;
        if record.archived_at.is_some() {
            return archive::view(connection, record, days, now).await;
        }
        sqlx::query(
            "update channels set archived_at = $2
             where id = $1 and deleted_at is null and archived_at is null",
        )
        .bind(channel_id)
        .bind(now)
        .execute(&mut *connection)
        .await?;
        record.archived_at = Some(now);
        crate::calls::store::mark_channel(connection, &app.calls, channel_id).await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Channel(workspace_id, channel_id),
            &AuditEvent::ChannelArchived,
        )
        .await?;
        let channel = archive::view(connection, record, days, now).await?;
        let archived = Event::ChannelArchived {
            channel: channel.clone(),
        };
        events::publish(connection, now, &archived).await?;
        super::channels::left_lists(connection, now, &channel.channel).await?;
        Ok(channel)
    })
    .map(Json)
}

/// Restores an archived channel: it is a channel of the list again and takes changes. The
/// caller holds `manage_channel` in the channel. A channel that is not archived stays as
/// it is.
#[utoipa::path(
    delete,
    path = "/api/v1/channels/{channel_id}/archive",
    tag = "channels",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 200, body = Channel), ApiError)
)]
pub async fn restore_channel(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<Json<Channel>> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (record, scope) = access.channel(connection, channel_id).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ManageChannel,
        })?;
        if record.archived_at.is_none() {
            return Ok(record.channel);
        }
        sqlx::query(
            "update channels set archived_at = null, deletion_notice_at = null
             where id = $1 and deleted_at is null",
        )
        .bind(channel_id)
        .execute(&mut *connection)
        .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Channel(record.channel.workspace_id, channel_id),
            &AuditEvent::ChannelRestored,
        )
        .await?;
        let restored = Event::ChannelRestored {
            channel: record.channel.clone(),
        };
        events::publish(connection, now, &restored).await?;
        let workspace_id = record.channel.workspace_id;
        events::permissions_changed_by_overrides_of(connection, now, workspace_id, channel_id)
            .await?;
        Ok(record.channel)
    })
    .map(Json)
}

/// The archived channels of a workspace that the caller can see, the one archived last
/// first.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/archived-channels",
    tag = "channels",
    params(("workspace_id" = WorkspaceId, Path)),
    security(("session" = [])),
    responses((status = 200, body = [ArchivedChannel]), ApiError)
)]
pub async fn list_archived_channels(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
) -> ApiResult<Json<Vec<ArchivedChannel>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ViewWorkspace(&scope))?;
        let days = archive::period(connection, workspace_id).await?;
        let seen: Vec<_> = access
            .workspace_channels(connection, workspace_id, &scope)
            .await?
            .into_iter()
            .filter(|(record, channel_scope)| {
                record.deleted_at.is_none()
                    && record.archived_at.is_some()
                    && access.allows(Request::Channel {
                        scope: channel_scope,
                        permission: WorkspacePermission::ViewChannel,
                    })
            })
            .map(|(record, _)| record)
            .collect();
        let mut archived = Vec::with_capacity(seen.len());
        for record in seen {
            archived.push(archive::view(connection, record, days, now).await?);
        }
        archived.sort_by_key(|found| std::cmp::Reverse((found.archived_at, found.channel.id)));
        Ok(archived)
    })
    .map(Json)
}

/// The time after which the server deletes an archived channel of the workspace. The
/// caller holds the administrator permission.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/archive-period",
    tag = "channels",
    params(("workspace_id" = WorkspaceId, Path)),
    security(("session" = [])),
    responses((status = 200, body = ArchivePeriod), ApiError)
)]
pub async fn get_archive_period(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
) -> ApiResult<Json<ArchivePeriod>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ViewWorkspace(&scope))?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        Ok(ArchivePeriod {
            days: archive::period(connection, workspace_id).await?,
        })
    })
    .map(Json)
}

/// Sets the archive period of the workspace: 7 to 36500 days, or `null` for never. Every
/// archived channel of the workspace gets a new notice 7 days before its new time. The
/// caller holds the administrator permission.
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/archive-period",
    tag = "channels",
    params(("workspace_id" = WorkspaceId, Path)),
    request_body = ArchivePeriod,
    security(("session" = [])),
    responses((status = 200, body = ArchivePeriod), ApiError)
)]
pub async fn set_archive_period(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<ArchivePeriod>,
) -> ApiResult<Json<ArchivePeriod>> {
    if body
        .days
        .is_some_and(|days| !(NOTICE_DAYS..=MOST_DAYS).contains(&days))
    {
        return Err(ApiError::invalid(format!(
            "days is from {NOTICE_DAYS} to {MOST_DAYS}, or null for never"
        )));
    }
    let days = body
        .days
        .map(i32::try_from)
        .transpose()
        .map_err(|error| ApiError::internal(&error))?;
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ViewWorkspace(&scope))?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        if archive::period(connection, workspace_id).await? == body.days {
            return Ok(body);
        }
        sqlx::query(
            "insert into workspace_archive_periods (workspace_id, days) values ($1, $2)
             on conflict (workspace_id) do update set days = excluded.days",
        )
        .bind(workspace_id)
        .bind(days)
        .execute(&mut *connection)
        .await?;
        // The time of each deletion changed, so each archived channel gets a new notice.
        sqlx::query(
            "update channels set deletion_notice_at = null
             where workspace_id = $1 and deleted_at is null and deletion_notice_at is not null",
        )
        .bind(workspace_id)
        .execute(&mut *connection)
        .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Workspace(workspace_id),
            &AuditEvent::ArchivePeriodChanged { days: body.days },
        )
        .await?;
        let changed = Event::RetentionChanged {
            workspace_id: Some(workspace_id),
        };
        events::publish(connection, now, &changed).await?;
        Ok(body)
    })
    .map(Json)
}
