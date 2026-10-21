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
    Category, ChangeChannelOverride, Channel, ChannelKind, ChannelList, ChannelOverride,
    ChannelPermissions, CreateCategory, CreateChannel, MoveChannels, OverrideState,
    SetChannelOverride, UpdateCategory, UpdateChannel,
};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::{Access, stored_override};
use crate::audit::{self, About};
use crate::calls::Calls;
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
        create_category_in(
            connection,
            &access,
            now,
            (workspace_id, id),
            &name,
            body.position,
        )
        .await
    })?;
    Ok((StatusCode::CREATED, Json(category)))
}

pub(crate) async fn create_category_in(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    (workspace_id, id): (WorkspaceId, CategoryId),
    name: &str,
    position: Option<i32>,
) -> ApiResult<Category> {
    let scope = access.workspace(connection, workspace_id).await?;
    access.require(Request::Workspace {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })?;

    let position = match position {
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
        name: name.to_owned(),
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
        let ids = (workspace_id, category_id);
        update_category_in(
            connection,
            &access,
            now,
            ids,
            new_name.as_deref(),
            body.position,
        )
        .await
    })
    .map(Json)
}

pub(crate) async fn update_category_in(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    (workspace_id, category_id): (WorkspaceId, CategoryId),
    new_name: Option<&str>,
    position: Option<i32>,
) -> ApiResult<Category> {
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
    .bind(new_name)
    .bind(position)
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
        Ok(visible_structure(connection, &access, workspace_id, &scope)
            .await?
            .list)
    })
    .map(Json)
}

/// What the caller sees of the structure of a workspace, with what the caller may do in
/// it (ADR 0310).
pub struct Visible {
    pub list: ChannelList,
    /// The permissions that the caller holds in the workspace.
    pub permissions: Vec<WorkspacePermission>,
    /// The channels of the list in which the caller holds another set.
    pub channel_permissions: Vec<ChannelPermissions>,
}

/// The categories of a workspace and the channels of it that the caller can see, and the
/// permissions of the caller in the workspace and in each of those channels.
pub async fn visible_structure(
    connection: &mut PgConnection,
    access: &Access,
    workspace_id: WorkspaceId,
    scope: &WorkspaceScope,
) -> ApiResult<Visible> {
    let categories: Vec<(CategoryId, String, i32)> = sqlx::query_as(
        "select id, name, position from categories
         where workspace_id = $1
         order by position, id",
    )
    .bind(workspace_id)
    .fetch_all(&mut *connection)
    .await?;
    let permissions = access.workspace_permissions(scope);
    let mut channels = Vec::new();
    let mut channel_permissions = Vec::new();
    let seen = access
        .workspace_channels(connection, workspace_id, scope)
        .await?
        .into_iter()
        .filter(|(record, channel_scope)| {
            record.deleted_at.is_none()
                && record.archived_at.is_none()
                && access.allows(Request::Channel {
                    scope: channel_scope,
                    permission: WorkspacePermission::ViewChannel,
                })
        });
    for (record, channel_scope) in seen {
        let held = access.channel_permissions(&channel_scope);
        if held != permissions {
            channel_permissions.push(ChannelPermissions {
                channel_id: record.channel.id,
                permissions: held,
            });
        }
        channels.push(record.channel);
    }
    Ok(Visible {
        list: ChannelList {
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
        },
        permissions,
        channel_permissions,
    })
}

/// The workspace permissions that the caller holds in one channel that it can see, an
/// archived channel included (ADR 0310).
#[utoipa::path(
    get,
    path = "/api/v1/channels/{channel_id}/permissions",
    tag = "channels",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 200, body = ChannelPermissions), ApiError)
)]
pub async fn get_channel_permissions(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<Json<ChannelPermissions>> {
    let now = app.now();
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (_, scope) = access.channel(connection, channel_id).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ViewChannel,
        })?;
        super::search::read_rate(&app, &access, &mut counted)?;
        Ok(ChannelPermissions {
            channel_id,
            permissions: access.channel_permissions(&scope),
        })
    })
    .map(Json)
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
    let new = NewChannel {
        id: ChannelId::generate(),
        workspace_id,
        name: &name,
        kind: body.kind,
        category_id: body.category_id,
        position: body.position,
    };
    let now = app.now();
    let channel = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        create_channel_in(connection, &access, now, &new).await
    })?;
    Ok((StatusCode::CREATED, Json(channel)))
}

/// A channel to create. The name is validated.
pub(crate) struct NewChannel<'a> {
    pub id: ChannelId,
    pub workspace_id: WorkspaceId,
    pub name: &'a str,
    pub kind: ChannelKind,
    pub category_id: Option<CategoryId>,
    /// Absent: after the last channel.
    pub position: Option<i32>,
}

pub(crate) async fn create_channel_in(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    new: &NewChannel<'_>,
) -> ApiResult<Channel> {
    let workspace_id = new.workspace_id;
    let scope = access.workspace(connection, workspace_id).await?;
    access.require(Request::Workspace {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })?;
    if let Some(category_id) = new.category_id {
        require_category(connection, workspace_id, category_id).await?;
    }

    let position = match new.position {
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
        id: new.id,
        workspace_id,
        category_id: new.category_id,
        kind: new.kind,
        name: new.name.to_owned(),
        position,
        permanent_messages: false,
        user_limit: None,
        hides_occupants: false,
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
        update_channel_in(
            connection,
            &access,
            now,
            channel_id,
            new_name.as_deref(),
            &body,
        )
        .await
    })
    .map(Json)
}

/// The most channels that one change of the order names.
const MOST_MOVES: usize = 200;

/// Changes the category and the position of channels of one workspace in one transaction:
/// all of them or none. Each move states where the client read the channel; when one of
/// them is no longer there, the answer is `conflict` and nothing changes. A move whose `to`
/// equals its `from` is a check only: it changes nothing and needs "view channel", so that a
/// client names the channels next to the destination that it read (ADR 0304). Each other
/// channel needs "manage channel", as on the route of one channel.
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/channels/positions",
    tag = "channels",
    params(("workspace_id" = WorkspaceId, Path)),
    request_body = MoveChannels,
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn move_channels(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<MoveChannels>,
) -> ApiResult<StatusCode> {
    if body.moves.is_empty() || body.moves.len() > MOST_MOVES {
        return Err(ApiError::invalid(format!(
            "moves must name 1 to {MOST_MOVES} channels"
        )));
    }
    let mut named = std::collections::HashSet::new();
    if !body.moves.iter().all(|held| named.insert(held.channel_id)) {
        return Err(ApiError::invalid("a channel is named twice"));
    }
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        // Every channel is checked against the state before the first change, so that a
        // later move is not compared with a position that an earlier move wrote.
        // A channel that the caller does not see is `not_found` before any other check, so
        // that the answer does not tell whether it exists or in which workspace it is.
        for held in &body.moves {
            let (record, scope) = access.channel(connection, held.channel_id).await?;
            access.require(Request::Channel {
                scope: &scope,
                permission: WorkspacePermission::ViewChannel,
            })?;
            if record.channel.workspace_id != workspace_id {
                return Err(ApiError::invalid("a channel is not in this workspace"));
            }
            if held.to != held.from {
                access.require(Request::Channel {
                    scope: &scope,
                    permission: WorkspacePermission::ManageChannel,
                })?;
            }
            record.require_open()?;
            if record.channel.category_id != held.from.category_id
                || record.channel.position != held.from.position
            {
                return Err(ApiError::conflict(
                    "a channel is no longer where the request read it",
                ));
            }
        }
        for held in body.moves.iter().filter(|held| held.to != held.from) {
            let change = UpdateChannel {
                category_id: Some(held.to.category_id),
                position: Some(held.to.position),
                ..UpdateChannel::default()
            };
            update_channel_in(connection, &access, now, held.channel_id, None, &change).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Changes a channel. `new_name` is the validated name of `change`.
pub(crate) async fn update_channel_in(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    channel_id: ChannelId,
    new_name: Option<&str>,
    change: &UpdateChannel,
) -> ApiResult<Channel> {
    let (record, scope) = access.channel(connection, channel_id).await?;
    access.require(Request::Channel {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })?;
    record.require_open()?;

    let mut channel = record.channel;
    if let Some(name) = new_name {
        name.clone_into(&mut channel.name);
    }
    if let Some(category_id) = change.category_id {
        if let Some(category_id) = category_id {
            require_category(connection, channel.workspace_id, category_id).await?;
        }
        channel.category_id = category_id;
    }
    if let Some(position) = change.position {
        channel.position = position;
    }
    if let Some(permanent_messages) = change.permanent_messages {
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
        delete_channel_in(connection, &app.calls, &access, now, channel_id).await
    })?;
    app.purges.wake();
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn delete_channel_in(
    connection: &mut PgConnection,
    calls: &Calls,
    access: &Access,
    now: OffsetDateTime,
    channel_id: ChannelId,
) -> ApiResult<()> {
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

    let removed = Removed {
        id: channel_id,
        workspace_id: record.channel.workspace_id,
        name: &record.channel.name,
        permanent_message_count: record.permanent_message_count,
    };
    remove_channel(connection, calls, now, &removed, Some(access.account_id)).await
}

/// A channel that is deleted, as the deletion read it.
pub(crate) struct Removed<'a> {
    pub id: ChannelId,
    pub workspace_id: WorkspaceId,
    pub name: &'a str,
    pub permanent_message_count: i64,
}

/// Deletes a channel: for a person, who passed the checks of the route, and for the server
/// when the archive period of the channel has ended (ADR 0410). The row stays as a marker
/// (ADR 0407). The content of the channel is removed afterwards, in bounded parts
/// (ADR 0017).
pub(crate) async fn remove_channel(
    connection: &mut PgConnection,
    calls: &Calls,
    now: OffsetDateTime,
    channel: &Removed<'_>,
    by: Option<junto_protocol::ids::AccountId>,
) -> ApiResult<()> {
    let deleted = sqlx::query(
        "update channels set deleted_at = $2
         where id = $1 and deleted_at is null and permanent_message_count = $3",
    )
    .bind(channel.id)
    .bind(now)
    .bind(channel.permanent_message_count)
    .execute(&mut *connection)
    .await?;
    if deleted.rows_affected() == 0 {
        return Err(ApiError::conflict("the channel changed during the request"));
    }
    sqlx::query("delete from thread_forks where channel_id=$1")
        .bind(channel.id)
        .execute(&mut *connection)
        .await?;
    crate::calls::store::mark_channel(connection, calls, channel.id).await?;
    events::channel_deleted(connection, now, channel.workspace_id, channel.id).await?;
    sqlx::query("delete from channel_overrides where channel_id = $1")
        .bind(channel.id)
        .execute(&mut *connection)
        .await?;
    crate::purge::queue(connection, now, crate::purge::Subject::Channel(channel.id)).await?;
    audit::record(
        connection,
        now,
        by,
        About::Channel(channel.workspace_id, channel.id),
        &AuditEvent::ChannelDeleted {
            name: channel.name.to_owned(),
        },
    )
    .await
}

/// Deletes a category. Its channels stay and have no category.
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}/categories/{category_id}",
    tag = "channels",
    params(("workspace_id" = WorkspaceId, Path), ("category_id" = CategoryId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn delete_category(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, category_id)): ApiPath<(WorkspaceId, CategoryId)>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        delete_category_in(connection, &access, now, (workspace_id, category_id)).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn delete_category_in(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    (workspace_id, category_id): (WorkspaceId, CategoryId),
) -> ApiResult<()> {
    let scope = access.workspace(connection, workspace_id).await?;
    access.require(Request::Workspace {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })?;
    let name: Option<String> = sqlx::query_scalar(
        "select name from categories where id = $1 and workspace_id = $2 for update",
    )
    .bind(category_id)
    .bind(workspace_id)
    .fetch_optional(&mut *connection)
    .await?;
    let name = name.ok_or_else(ApiError::not_found)?;
    // The rows of deleted channels refer to the category as well.
    sqlx::query("update channels set category_id = null where category_id = $1")
        .bind(category_id)
        .execute(&mut *connection)
        .await?;
    sqlx::query("delete from structure_identifiers where kind = 'category' and object_id = $1")
        .bind(category_id)
        .execute(&mut *connection)
        .await?;
    sqlx::query("delete from categories where id = $1")
        .bind(category_id)
        .execute(&mut *connection)
        .await?;
    audit::record(
        connection,
        now,
        Some(access.account_id),
        About::Workspace(workspace_id),
        &AuditEvent::CategoryDeleted { category_id, name },
    )
    .await?;
    let deleted = Event::CategoryDeleted {
        workspace_id,
        category_id,
    };
    events::publish(connection, now, &deleted).await
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
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        set_override_in(
            connection,
            &app.calls,
            &access,
            now,
            (channel_id, role_id),
            next,
            false,
        )
        .await
    })
    .map(Json)
}

/// Changes one permission of a channel override with the grant ceiling on its delta.
#[utoipa::path(
    patch,
    path = "/api/v1/channels/{channel_id}/overrides/{role_id}",
    tag = "channels",
    params(("channel_id" = ChannelId, Path), ("role_id" = WorkspaceRoleId, Path)),
    request_body = ChangeChannelOverride,
    security(("session" = [])),
    responses((status = 200, body = ChannelOverride), ApiError)
)]
pub async fn change_channel_override(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((channel_id, role_id)): ApiPath<(ChannelId, WorkspaceRoleId)>,
    ApiJson(body): ApiJson<ChangeChannelOverride>,
) -> ApiResult<Json<ChannelOverride>> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (_, scope) = access.channel(connection, channel_id).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ManageChannel,
        })?;
        let stored: Option<(i64, i64)> = sqlx::query_as(
            "select allow, deny from channel_overrides
             where channel_id=$1 and role_id=$2 for update",
        )
        .bind(channel_id)
        .bind(role_id)
        .fetch_optional(&mut *connection)
        .await?;
        let previous = match stored {
            Some((allow, deny)) => stored_override(allow, deny)?,
            None => Override::NONE,
        };
        let changed = WorkspacePermissions::from(body.permission);
        let mut allow = previous.allow().difference(changed);
        let mut deny = previous.deny().difference(changed);
        match body.state {
            OverrideState::Allow => allow = allow.union(changed),
            OverrideState::Deny => deny = deny.union(changed),
            OverrideState::Inherit => {}
        }
        let next = Override::new(allow, deny).map_err(|error| ApiError::internal(&error))?;
        set_override_in(
            connection,
            &app.calls,
            &access,
            now,
            (channel_id, role_id),
            next,
            true,
        )
        .await
    })
    .map(Json)
}

pub(crate) async fn set_override_in(
    connection: &mut PgConnection,
    calls: &Calls,
    access: &Access,
    now: OffsetDateTime,
    (channel_id, role_id): (ChannelId, WorkspaceRoleId),
    next: Override,
    only_if_changed: bool,
) -> ApiResult<ChannelOverride> {
    let (allow, deny) = (next.allow(), next.deny());
    let result = ChannelOverride {
        role_id,
        allow: db::workspace_list(allow),
        deny: db::workspace_list(deny),
    };
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

    if only_if_changed && previous == next {
        return Ok(result);
    }
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
        crate::calls::store::mark_channel(connection, calls, channel_id).await?;
        let holders = Affected::WorkspaceRoleHolders(workspace_id, role_id);
        events::permissions_changed(connection, now, holders, Some(workspace_id)).await?;
    }
    Ok(result)
}
