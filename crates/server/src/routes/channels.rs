use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::{
    ChannelContent, Override, Request, WorkspacePermission, WorkspacePermissions, WorkspaceScope,
};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{CategoryId, ChannelId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::structure::{
    Category, Channel, ChannelList, ChannelOverride, CreateCategory, CreateChannel,
    SetChannelOverride, UpdateCategory, UpdateChannel,
};
use sqlx::PgConnection;

use crate::access::{Access, stored_override};
use crate::audit::{self, About};
use crate::db::{self, StoredText};
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::events::log::Affected;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::state::AppState;
use crate::validate;
use crate::{read, transact};

async fn require_category(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    category_id: CategoryId,
) -> ApiResult<()> {
    let exists: bool = sqlx::query_scalar(
        "select exists (select 1 from categories where id = $1 and workspace_id = $2)",
    )
    .bind(category_id)
    .bind(workspace_id)
    .fetch_one(connection)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

/// Creates a category in a workspace.
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/categories",
    tag = "channels",
    params(("workspace_id" = WorkspaceId, Path)),
    request_body = CreateCategory,
    security(("session" = [])),
    responses((status = 201, body = Category), ApiError)
)]
pub async fn create_category(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<CreateCategory>,
) -> ApiResult<(StatusCode, Json<Category>)> {
    let name = validate::name("name", &body.name)?;
    let id = CategoryId::generate();
    let now = app.now();
    let category = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::Workspace {
            scope: &scope,
            permission: WorkspacePermission::ManageChannel,
        })?;

        let position = match body.position {
            Some(position) => position,
            None => {
                sqlx::query_scalar(
                    "select coalesce(max(position), -1) + 1 from categories
                         where workspace_id = $1",
                )
                .bind(workspace_id)
                .fetch_one(&mut *connection)
                .await?
            }
        };
        let category = Category {
            id,
            workspace_id,
            name: name.clone(),
            position,
        };
        sqlx::query(
            "insert into categories (id, workspace_id, name, position, created_at)
                 values ($1, $2, $3, $4, $5)",
        )
        .bind(category.id)
        .bind(workspace_id)
        .bind(&category.name)
        .bind(category.position)
        .bind(now)
        .execute(&mut *connection)
        .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Workspace(workspace_id),
            &AuditEvent::CategoryCreated {
                category_id: category.id,
                name: category.name.clone(),
            },
        )
        .await?;
        let created = Event::CategoryCreated {
            category: category.clone(),
        };
        events::publish(connection, now, &created).await?;
        Ok(category)
    })?;
    Ok((StatusCode::CREATED, Json(category)))
}

/// Changes the name or the position of a category.
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/categories/{category_id}",
    tag = "channels",
    params(("workspace_id" = WorkspaceId, Path), ("category_id" = CategoryId, Path)),
    request_body = UpdateCategory,
    security(("session" = [])),
    responses((status = 200, body = Category), ApiError)
)]
pub async fn update_category(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, category_id)): ApiPath<(WorkspaceId, CategoryId)>,
    ApiJson(body): ApiJson<UpdateCategory>,
) -> ApiResult<Json<Category>> {
    let new_name = body
        .name
        .as_deref()
        .map(|name| validate::name("name", name))
        .transpose()?;
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::Workspace {
            scope: &scope,
            permission: WorkspacePermission::ManageChannel,
        })?;

        let updated: Option<(String, i32)> = sqlx::query_as(
            "update categories
             set name = coalesce($3, name), position = coalesce($4, position)
             where id = $1 and workspace_id = $2
             returning name, position",
        )
        .bind(category_id)
        .bind(workspace_id)
        .bind(new_name.as_deref())
        .bind(body.position)
        .fetch_optional(&mut *connection)
        .await?;
        let (name, position) = updated.ok_or_else(ApiError::not_found)?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Workspace(workspace_id),
            &AuditEvent::CategoryUpdated {
                category_id,
                name: name.clone(),
                position,
            },
        )
        .await?;
        let category = Category {
            id: category_id,
            workspace_id,
            name,
            position,
        };
        let updated = Event::CategoryUpdated {
            category: category.clone(),
        };
        events::publish(connection, now, &updated).await?;
        Ok(category)
    })
    .map(Json)
}

/// The categories of a workspace and the channels that the caller can see.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/channels",
    tag = "channels",
    params(("workspace_id" = WorkspaceId, Path)),
    security(("session" = [])),
    responses((status = 200, body = ChannelList), ApiError)
)]
pub async fn list_channels(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
) -> ApiResult<Json<ChannelList>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ViewWorkspace(&scope))?;
        visible_structure(connection, &access, workspace_id, &scope).await
    })
    .map(Json)
}

/// The categories of a workspace and the channels of it that the caller can see.
pub async fn visible_structure(
    connection: &mut PgConnection,
    access: &Access,
    workspace_id: WorkspaceId,
    scope: &WorkspaceScope,
) -> ApiResult<ChannelList> {
    let categories: Vec<(CategoryId, String, i32)> = sqlx::query_as(
        "select id, name, position from categories
         where workspace_id = $1
         order by position, id",
    )
    .bind(workspace_id)
    .fetch_all(&mut *connection)
    .await?;
    let channels = access
        .workspace_channels(connection, workspace_id, scope)
        .await?
        .into_iter()
        .filter(|(record, channel_scope)| {
            record.deleted_at.is_none()
                && access.allows(Request::Channel {
                    scope: channel_scope,
                    permission: WorkspacePermission::ViewChannel,
                })
        })
        .map(|(record, _)| record.channel)
        .collect();
    Ok(ChannelList {
        categories: categories
            .into_iter()
            .map(|(id, name, position)| Category {
                id,
                workspace_id,
                name,
                position,
            })
            .collect(),
        channels,
    })
}

/// Creates a text channel or a voice channel in a workspace.
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/channels",
    tag = "channels",
    params(("workspace_id" = WorkspaceId, Path)),
    request_body = CreateChannel,
    security(("session" = [])),
    responses((status = 201, body = Channel), ApiError)
)]
pub async fn create_channel(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<CreateChannel>,
) -> ApiResult<(StatusCode, Json<Channel>)> {
    let name = validate::name("name", &body.name)?;
    let id = ChannelId::generate();
    let now = app.now();
    let channel = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::Workspace {
            scope: &scope,
            permission: WorkspacePermission::ManageChannel,
        })?;
        if let Some(category_id) = body.category_id {
            require_category(connection, workspace_id, category_id).await?;
        }

        let position = match body.position {
            Some(position) => position,
            None => {
                sqlx::query_scalar(
                    "select coalesce(max(position), -1) + 1 from channels
                         where workspace_id = $1 and deleted_at is null",
                )
                .bind(workspace_id)
                .fetch_one(&mut *connection)
                .await?
            }
        };
        let channel = Channel {
            id,
            workspace_id,
            category_id: body.category_id,
            kind: body.kind,
            name: name.clone(),
            position,
            permanent_messages: false,
        };
        sqlx::query(
            "insert into channels
                     (id, workspace_id, category_id, kind, name, position, created_at)
                 values ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(channel.id)
        .bind(workspace_id)
        .bind(channel.category_id)
        .bind(channel.kind.stored())
        .bind(&channel.name)
        .bind(channel.position)
        .bind(now)
        .execute(&mut *connection)
        .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Channel(workspace_id, channel.id),
            &AuditEvent::ChannelCreated {
                name: channel.name.clone(),
                kind: channel.kind,
            },
        )
        .await?;
        let created = Event::ChannelCreated {
            channel: channel.clone(),
        };
        events::publish(connection, now, &created).await?;
        Ok(channel)
    })?;
    Ok((StatusCode::CREATED, Json(channel)))
}

/// Changes the settings of a channel.
#[utoipa::path(
    patch,
    path = "/api/v1/channels/{channel_id}",
    tag = "channels",
    params(("channel_id" = ChannelId, Path)),
    request_body = UpdateChannel,
    security(("session" = [])),
    responses((status = 200, body = Channel), ApiError)
)]
pub async fn update_channel(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
    ApiJson(body): ApiJson<UpdateChannel>,
) -> ApiResult<Json<Channel>> {
    let new_name = body
        .name
        .as_deref()
        .map(|name| validate::name("name", name))
        .transpose()?;
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (record, scope) = access.channel(connection, channel_id).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ManageChannel,
        })?;

        let mut channel = record.channel;
        if let Some(name) = &new_name {
            channel.name.clone_from(name);
        }
        if let Some(category_id) = body.category_id {
            if let Some(category_id) = category_id {
                require_category(connection, channel.workspace_id, category_id).await?;
            }
            channel.category_id = category_id;
        }
        if let Some(position) = body.position {
            channel.position = position;
        }
        if let Some(permanent_messages) = body.permanent_messages {
            channel.permanent_messages = permanent_messages;
        }
        let updated = sqlx::query(
            "update channels
             set name = $2, category_id = $3, position = $4, permanent_messages = $5
             where id = $1 and deleted_at is null",
        )
        .bind(channel_id)
        .bind(&channel.name)
        .bind(channel.category_id)
        .bind(channel.position)
        .bind(channel.permanent_messages)
        .execute(&mut *connection)
        .await?;
        if updated.rows_affected() == 0 {
            return Err(ApiError::not_found());
        }
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Channel(channel.workspace_id, channel_id),
            &AuditEvent::ChannelUpdated {
                name: channel.name.clone(),
                category_id: channel.category_id,
                position: channel.position,
                permanent_messages: channel.permanent_messages,
            },
        )
        .await?;
        let updated = Event::ChannelUpdated {
            channel: channel.clone(),
        };
        events::publish(connection, now, &updated).await?;
        Ok(channel)
    })
    .map(Json)
}

/// Deletes a channel. Only the server owner deletes a channel that contains permanent
/// messages.
#[utoipa::path(
    delete,
    path = "/api/v1/channels/{channel_id}",
    tag = "channels",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn delete_channel(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (record, scope) = access.channel(connection, channel_id).await?;
        let content = if record.permanent_message_count > 0 {
            ChannelContent::PermanentMessages
        } else {
            ChannelContent::NoPermanentMessages
        };
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ViewChannel,
        })?;
        access.require(Request::DeleteChannel {
            scope: &scope,
            content,
        })?;

        let deleted = sqlx::query(
            "update channels set deleted_at = $2
             where id = $1 and deleted_at is null and permanent_message_count = $3",
        )
        .bind(channel_id)
        .bind(now)
        .bind(record.permanent_message_count)
        .execute(&mut *connection)
        .await?;
        if deleted.rows_affected() == 0 {
            return Err(ApiError::conflict("the channel changed during the request"));
        }
        let workspace_id = record.channel.workspace_id;
        events::channel_deleted(connection, now, workspace_id, channel_id).await?;
        sqlx::query("delete from channel_overrides where channel_id = $1")
            .bind(channel_id)
            .execute(&mut *connection)
            .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Channel(record.channel.workspace_id, channel_id),
            &AuditEvent::ChannelDeleted {
                name: record.channel.name,
            },
        )
        .await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// The role overrides of a channel.
#[utoipa::path(
    get,
    path = "/api/v1/channels/{channel_id}/overrides",
    tag = "channels",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 200, body = [ChannelOverride]), ApiError)
)]
pub async fn list_channel_overrides(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<Json<Vec<ChannelOverride>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (_, scope) = access.channel(connection, channel_id).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ManageChannel,
        })?;
        let rows: Vec<(WorkspaceRoleId, i64, i64)> = sqlx::query_as(
            "select role_id, allow, deny from channel_overrides
             where channel_id = $1
             order by role_id",
        )
        .bind(channel_id)
        .fetch_all(&mut *connection)
        .await?;
        rows.into_iter()
            .map(|(role_id, allow, deny)| {
                Ok(ChannelOverride {
                    role_id,
                    allow: db::workspace_list(db::workspace_permissions(allow)?),
                    deny: db::workspace_list(db::workspace_permissions(deny)?),
                })
            })
            .collect()
    })
    .map(Json)
}

/// Sets the allow set and the deny set of one workspace role in a channel.
#[utoipa::path(
    put,
    path = "/api/v1/channels/{channel_id}/overrides/{role_id}",
    tag = "channels",
    params(("channel_id" = ChannelId, Path), ("role_id" = WorkspaceRoleId, Path)),
    request_body = SetChannelOverride,
    security(("session" = [])),
    responses((status = 200, body = ChannelOverride), ApiError)
)]
pub async fn set_channel_override(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((channel_id, role_id)): ApiPath<(ChannelId, WorkspaceRoleId)>,
    ApiJson(body): ApiJson<SetChannelOverride>,
) -> ApiResult<Json<ChannelOverride>> {
    let allow: WorkspacePermissions = body.allow.iter().copied().collect();
    let deny: WorkspacePermissions = body.deny.iter().copied().collect();
    let next = Override::new(allow, deny)
        .map_err(|_| ApiError::invalid("a permission is in both allow and deny"))?;
    let result = ChannelOverride {
        role_id,
        allow: db::workspace_list(allow),
        deny: db::workspace_list(deny),
    };
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (record, scope) = access.channel(connection, channel_id).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ManageChannel,
        })?;
        let workspace_id = record.channel.workspace_id;

        let role_exists: bool = sqlx::query_scalar(
            "select exists (select 1 from workspace_roles where id = $1 and workspace_id = $2)",
        )
        .bind(role_id)
        .bind(workspace_id)
        .fetch_one(&mut *connection)
        .await?;
        if !role_exists {
            return Err(ApiError::not_found());
        }
        let stored: Option<(i64, i64)> = sqlx::query_as(
            "select allow, deny from channel_overrides
             where channel_id = $1 and role_id = $2
             for update",
        )
        .bind(channel_id)
        .bind(role_id)
        .fetch_optional(&mut *connection)
        .await?;
        let previous = match stored {
            Some((allow, deny)) => stored_override(allow, deny)?,
            None => Override::NONE,
        };
        access.require(Request::ManageChannelOverrides {
            scope: &scope,
            granting: previous.granted_by_change_to(next),
        })?;

        if next.is_none() {
            sqlx::query("delete from channel_overrides where channel_id = $1 and role_id = $2")
                .bind(channel_id)
                .bind(role_id)
                .execute(&mut *connection)
                .await?;
        } else {
            sqlx::query(
                "insert into channel_overrides (channel_id, role_id, workspace_id, allow, deny)
                 values ($1, $2, $3, $4, $5)
                 on conflict (channel_id, role_id) do update set
                     allow = excluded.allow, deny = excluded.deny",
            )
            .bind(channel_id)
            .bind(role_id)
            .bind(workspace_id)
            .bind(db::workspace_bits(allow))
            .bind(db::workspace_bits(deny))
            .execute(&mut *connection)
            .await?;
        }
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Channel(workspace_id, channel_id),
            &AuditEvent::ChannelOverrideSet {
                role_id,
                allow: result.allow.clone(),
                deny: result.deny.clone(),
            },
        )
        .await?;
        let set = Event::ChannelOverrideSet {
            workspace_id,
            channel_id,
            role_id,
            allow: result.allow.clone(),
            deny: result.deny.clone(),
        };
        events::publish(connection, now, &set).await?;
        if previous != next {
            let holders = Affected::WorkspaceRoleHolders(workspace_id, role_id);
            events::permissions_changed(connection, now, holders, Some(workspace_id)).await?;
        }
        Ok(())
    })?;
    Ok(Json(result))
}
