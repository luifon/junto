use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::{ChannelContent, Request, WorkspacePermission, WorkspaceScope};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{CategoryId, ChannelId, WorkspaceId};
use junto_protocol::structure::{
    Category, Channel, ChannelKind, ChannelList, ChannelMove, ChannelPermissions, CreateCategory,
    CreateChannel, MOST_CHANNEL_MOVES, MoveChannels, UpdateCategory, UpdateChannel,
};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::Access;
pub(crate) use crate::access::order::Target;
use crate::access::order::{Held, reordered};
use crate::audit::{self, About};
use crate::calls::Calls;
use crate::db::StoredText;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::events::log::Affected;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::state::AppState;
use crate::validate;
use crate::{read, transact};

async fn require_category(
    connection: &mut PgConnection,
    access: &Access,
    workspace_id: WorkspaceId,
    category_id: CategoryId,
) -> ApiResult<()> {
    let (_, scope) = access
        .visible_category(connection, workspace_id, category_id)
        .await?;
    access.require(Request::Channel {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })
}

/// A channel at the root of the list, outside every category, needs manage channel in the
/// workspace: to create it there and to move it there (ADR 0304).
async fn require_root(
    connection: &mut PgConnection,
    access: &Access,
    workspace_id: WorkspaceId,
) -> ApiResult<()> {
    let scope = access.workspace(connection, workspace_id).await?;
    access.require(Request::Workspace {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })
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

    sqlx::query(
        "insert into categories (id, workspace_id, name, position, created_at)
         select $1, $2, $3, coalesce(max(position), -1) + 1, $4 from categories
         where workspace_id = $2",
    )
    .bind(id)
    .bind(workspace_id)
    .bind(name)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    let position = order_categories(
        connection,
        access,
        now,
        (workspace_id, &scope),
        id,
        position.map(Target::Position),
    )
    .await?;
    let category = Category {
        id,
        workspace_id,
        name: name.to_owned(),
        position,
    };
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
    // A new category is empty, and listed only to the people who manage channels: the others
    // change from the stored positions of the categories to places (ADR 0304).
    if category_numbers_differ(connection, workspace_id).await? {
        events::log::structure_reordered(connection, now, workspace_id).await?;
    }
    Ok(category)
}

/// The ids of a group in the stored order, after `changes` are applied to its positions.
fn order_after<Id: Copy + Ord + std::hash::Hash>(
    group: &[Held<Id>],
    changes: &[(Id, i32)],
) -> Vec<Id> {
    let mut after = group.to_vec();
    for (id, position) in changes {
        if let Some(held) = after.iter_mut().find(|held| held.id == *id) {
            held.position = *position;
        }
    }
    crate::access::order::stored_order(&mut after);
    after.iter().map(|held| held.id).collect()
}

/// Applies a request to the order of the categories of a workspace (ADR 0304,
/// `access::order`): writes the stored positions that change, and, when the order changed,
/// every person of the workspace reads the structure again, because the position that a
/// person who does not see every category reads is a place among those that the person
/// sees. Returns the position that the caller reads of `category_id`.
async fn order_categories(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    (workspace_id, scope): (WorkspaceId, &WorkspaceScope),
    category_id: CategoryId,
    target: Option<Target>,
) -> ApiResult<i32> {
    let group = access
        .category_order(connection, workspace_id, scope)
        .await?;
    let changes = match target {
        Some(target) => reordered(&group, &[(category_id, target)]),
        None => Vec::new(),
    };
    for (id, position) in &changes {
        sqlx::query("update categories set position = $3 where id = $1 and workspace_id = $2")
            .bind(id)
            .bind(workspace_id)
            .bind(position)
            .execute(&mut *connection)
            .await?;
    }
    let before: Vec<CategoryId> = group.iter().map(|held| held.id).collect();
    if order_after(&group, &changes) != before {
        events::log::structure_reordered(connection, now, workspace_id).await?;
    }
    let mut after = group;
    for (id, position) in &changes {
        if let Some(held) = after.iter_mut().find(|held| held.id == *id) {
            held.position = *position;
        }
    }
    crate::access::order::stored_order(&mut after);
    crate::access::order::read_positions(&after)
        .get(&category_id)
        .copied()
        .ok_or_else(ApiError::not_found)
}

/// Applies the targets of a request to the order of the channels of one group, its category
/// or the root (ADR 0304, `access::order`), as for the categories. Every channel of
/// `targets` is in the group already; a channel that entered it is at its end.
pub(crate) async fn order_channels(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    (workspace_id, category_id): (WorkspaceId, Option<CategoryId>),
    targets: &[(ChannelId, Target)],
) -> ApiResult<()> {
    let scope = access.workspace(connection, workspace_id).await?;
    let order = access
        .channel_order(connection, workspace_id, &scope)
        .await?;
    let group = crate::access::channel_order::group_of(&order, category_id);
    let changes = reordered(&group, targets);
    for (id, position) in &changes {
        sqlx::query("update channels set position = $3 where id = $1 and workspace_id = $2")
            .bind(id)
            .bind(workspace_id)
            .bind(position)
            .execute(&mut *connection)
            .await?;
    }
    let before: Vec<ChannelId> = group.iter().map(|held| held.id).collect();
    if order_after(&group, &changes) != before {
        events::log::structure_reordered(connection, now, workspace_id).await?;
    }
    Ok(())
}

/// True when an open channel follows `channel` in its group, whose places then change for
/// a person who does not see every channel of the group when `channel` leaves or enters the
/// lists (ADR 0304).
pub(crate) async fn followed_in_group(
    connection: &mut PgConnection,
    channel: &Channel,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "select exists (select 1 from channels
         where workspace_id = $1 and category_id is not distinct from $2 and id <> $4
           and deleted_at is null and archived_at is null and (position, id) > ($3, $4))",
    )
    .bind(channel.workspace_id)
    .bind(channel.category_id)
    .bind(channel.position)
    .bind(channel.id)
    .fetch_one(connection)
    .await?)
}

/// The stored position after the last channel of a group that is not deleted.
async fn end_of_group(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    category_id: Option<CategoryId>,
) -> ApiResult<i32> {
    Ok(sqlx::query_scalar(
        "select coalesce(max(position), -1) + 1 from channels
         where workspace_id = $1 and category_id is not distinct from $2 and deleted_at is null",
    )
    .bind(workspace_id)
    .bind(category_id)
    .fetch_one(connection)
    .await?)
}

/// Numbers the channels of a group, that are not deleted, from 0 in their stored order, after
/// the channel `left` left it, when a rule may have hidden that channel from a person: a
/// person who sees every remaining channel reads the stored positions, and a gap would show
/// where a channel hidden from that person was (ADR 0304). A gap of a channel that every
/// person saw shows nothing hidden. `None` numbers the group in every case. Every person of
/// the workspace reads the structure again when a position changed, or when the numbers of
/// a person who saw fewer channels change from places to the stored positions.
pub(crate) async fn close_channel_gap(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    (workspace_id, category_id): (WorkspaceId, Option<CategoryId>),
    left: Option<ChannelId>,
) -> ApiResult<()> {
    if let Some(left) = left
        && !events::log::may_be_hidden(connection, left).await?
    {
        return Ok(());
    }
    let changed = sqlx::query(
        "update channels c set position = o.place
         from (select id, (row_number() over (order by position, id) - 1)::integer as place
               from channels
               where workspace_id = $1 and category_id is not distinct from $2
                 and deleted_at is null) o
         where c.id = o.id and c.position <> o.place",
    )
    .bind(workspace_id)
    .bind(category_id)
    .execute(&mut *connection)
    .await?;
    if changed.rows_affected() > 0
        || channel_numbers_differ(connection, workspace_id, category_id).await?
    {
        events::log::structure_reordered(connection, now, workspace_id).await?;
    }
    Ok(())
}

/// True when the stored positions of the listed channels of a group differ from their places
/// among the listed channels, because an archived channel stands before one or a client of
/// API v1 stored a position as given. A person whose view of the group changes between every
/// channel and fewer then reads other numbers, and reads the structure again (ADR 0304).
async fn channel_numbers_differ(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    category_id: Option<CategoryId>,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "select exists (select 1 from (
             select position, (row_number() over (order by position, id) - 1)::integer as place
             from channels
             where workspace_id = $1 and category_id is not distinct from $2
               and deleted_at is null and archived_at is null) o
         where o.position <> o.place)",
    )
    .bind(workspace_id)
    .bind(category_id)
    .fetch_one(connection)
    .await?)
}

/// The same for the categories of a workspace.
async fn category_numbers_differ(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "select exists (select 1 from (
             select position, (row_number() over (order by position, id) - 1)::integer as place
             from categories where workspace_id = $1) o
         where o.position <> o.place)",
    )
    .bind(workspace_id)
    .fetch_one(connection)
    .await?)
}

/// Numbers the categories of a workspace from 0 in their stored order, after one left it, as
/// for the channels of a group.
async fn close_category_gap(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    workspace_id: WorkspaceId,
) -> ApiResult<()> {
    let changed = sqlx::query(
        "update categories c set position = o.place
         from (select id, (row_number() over (order by position, id) - 1)::integer as place
               from categories where workspace_id = $1) o
         where c.id = o.id and c.position <> o.place",
    )
    .bind(workspace_id)
    .execute(&mut *connection)
    .await?;
    if changed.rows_affected() > 0 {
        events::log::structure_reordered(connection, now, workspace_id).await?;
    }
    Ok(())
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
        let target = match (body.place, body.position) {
            (Some(place), _) => Some(Target::Place(place)),
            (None, Some(position)) => Some(Target::Position(position)),
            (None, None) => None,
        };
        update_category_in(connection, &access, now, ids, new_name.as_deref(), target).await
    })
    .map(Json)
}

pub(crate) async fn update_category_in(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    (workspace_id, category_id): (WorkspaceId, CategoryId),
    new_name: Option<&str>,
    target: Option<Target>,
) -> ApiResult<Category> {
    let (_, scope) = access
        .visible_category(connection, workspace_id, category_id)
        .await?;
    access.require(Request::Channel {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })?;

    let name: Option<String> = sqlx::query_scalar(
        "update categories set name = coalesce($3, name)
         where id = $1 and workspace_id = $2
         returning name",
    )
    .bind(category_id)
    .bind(workspace_id)
    .bind(new_name)
    .fetch_optional(&mut *connection)
    .await?;
    let name = name.ok_or_else(ApiError::not_found)?;
    let workspace = access.workspace(connection, workspace_id).await?;
    let position = order_categories(
        connection,
        access,
        now,
        (workspace_id, &workspace),
        category_id,
        target,
    )
    .await?;
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
    let categories = access
        .categories(connection, workspace_id, scope, None)
        .await?;
    let permissions = access.workspace_permissions(scope);
    let mut channels = Vec::new();
    let mut channel_permissions = Vec::new();
    let records: Vec<_> = access
        .workspace_channels(connection, workspace_id, scope)
        .await?
        .into_iter()
        .filter(|(record, _)| record.deleted_at.is_none())
        .collect();
    // The position that the caller reads of each channel of the lists (ADR 0304).
    let order: Vec<crate::access::channel_order::Placed> = records
        .iter()
        .map(
            |(record, channel_scope)| crate::access::channel_order::Placed {
                id: record.channel.id,
                category_id: record.channel.category_id,
                position: record.channel.position,
                listed: access.listed(record, channel_scope),
                viewable: access.allows(Request::Channel {
                    scope: channel_scope,
                    permission: WorkspacePermission::ViewChannel,
                }),
            },
        )
        .collect();
    let places = crate::access::channel_order::channel_ranks(&order);
    for (record, channel_scope) in records {
        let Some(&position) = places.get(&record.channel.id) else {
            continue;
        };
        let held = access.channel_permissions(&channel_scope);
        if held != permissions {
            channel_permissions.push(ChannelPermissions {
                channel_id: record.channel.id,
                permissions: held,
            });
        }
        channels.push(Channel {
            position,
            ..record.channel
        });
    }
    let categories: Vec<(Category, bool)> = categories
        .into_iter()
        .map(|(category, scope)| {
            let visible = access.category_visible(
                &scope,
                channels
                    .iter()
                    .any(|channel| channel.category_id == Some(category.id)),
            );
            (category, visible)
        })
        .collect();
    let category_places = crate::access::order::read_positions(
        &categories
            .iter()
            .map(|(category, visible)| crate::access::order::Held {
                id: category.id,
                position: category.position,
                listed: *visible,
                viewable: *visible,
            })
            .collect::<Vec<_>>(),
    );
    Ok(Visible {
        list: ChannelList {
            categories: categories
                .into_iter()
                .filter_map(|(category, _)| {
                    let position = *category_places.get(&category.id)?;
                    Some(Category {
                        position,
                        ..category
                    })
                })
                .collect(),
            channels,
        },
        permissions,
        channel_permissions,
    })
}

/// One channel the caller can see, including an archived channel.
#[utoipa::path(
    get,
    path = "/api/v1/channels/{channel_id}",
    tag = "channels",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 200, body = Channel), ApiError)
)]
pub async fn get_channel(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<Json<Channel>> {
    let now = app.now();
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (record, scope) = access.channel(connection, channel_id).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ViewChannel,
        })?;
        super::search::read_rate(&app, &access, &mut counted)?;
        let workspace_id = record.channel.workspace_id;
        let position = access
            .channel_place(connection, workspace_id, channel_id)
            .await?;
        Ok(Channel {
            position,
            ..record.channel
        })
    })
    .map(Json)
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
        topic: &body.topic,
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
    pub topic: &'a str,
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
    if let Some(category_id) = new.category_id {
        require_category(connection, access, workspace_id, category_id).await?;
    } else {
        require_root(connection, access, workspace_id).await?;
    }

    let position = end_of_group(connection, workspace_id, new.category_id).await?;
    let mut channel = Channel {
        id: new.id,
        workspace_id,
        category_id: new.category_id,
        kind: new.kind,
        name: new.name.to_owned(),
        voice_status: String::new(),
        topic: validate::channel_topic(new.topic)?,
        position,
        permanent_messages: false,
        user_limit: None,
        hides_occupants: false,
    };
    sqlx::query(
        "insert into channels
             (id, workspace_id, category_id, kind, name, topic, position, created_at)
         values ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(channel.id)
    .bind(workspace_id)
    .bind(channel.category_id)
    .bind(channel.kind.stored())
    .bind(&channel.name)
    .bind(&channel.topic)
    .bind(channel.position)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    if let Some(position) = new.position {
        let group = (workspace_id, channel.category_id);
        order_channels(
            connection,
            access,
            now,
            group,
            &[(channel.id, Target::Position(position))],
        )
        .await?;
    }
    channel.position = access
        .channel_place(connection, workspace_id, channel.id)
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
    events::permissions_changed_by_overrides_of(connection, now, workspace_id, channel.id).await?;
    // A person who does not see the new channel can change from the stored positions of its
    // group to places (ADR 0304).
    if events::log::may_be_hidden(connection, channel.id).await?
        && channel_numbers_differ(connection, workspace_id, channel.category_id).await?
    {
        events::log::structure_reordered(connection, now, workspace_id).await?;
    }
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
            Order::Now,
        )
        .await
    })
    .map(Json)
}

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
    if body.moves.is_empty() || body.moves.len() > MOST_CHANNEL_MOVES {
        return Err(ApiError::invalid(format!(
            "moves must name 1 to {MOST_CHANNEL_MOVES} channels"
        )));
    }
    let mut named = std::collections::HashSet::new();
    if !body.moves.iter().all(|held| named.insert(held.channel_id)) {
        return Err(ApiError::invalid("a channel is named twice"));
    }
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let workspace = access.workspace(connection, workspace_id).await?;
        let places = crate::access::channel_order::channel_ranks(
            &access
                .channel_order(connection, workspace_id, &workspace)
                .await?,
        );
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
            // The position of a channel that the client read is the position that the
            // caller reads (ADR 0304).
            if record.channel.category_id != held.from.category_id
                || places.get(&held.channel_id) != Some(&held.from.position)
            {
                return Err(ApiError::conflict(
                    "a channel is no longer where the request read it",
                ));
            }
        }
        let moved: Vec<&ChannelMove> = body
            .moves
            .iter()
            .filter(|held| held.to != held.from)
            .collect();
        // The channels change their category first, each to the end of its new group; then
        // the order of each group is set once from the positions of the whole request, which
        // a client of API v1 computed together (ADR 0304).
        for held in &moved {
            let change = UpdateChannel {
                category_id: Some(held.to.category_id),
                ..UpdateChannel::default()
            };
            update_channel_in(
                connection,
                &access,
                now,
                held.channel_id,
                None,
                &change,
                Order::Later,
            )
            .await?;
        }
        let mut groups: Vec<Option<CategoryId>> =
            moved.iter().map(|held| held.to.category_id).collect();
        groups.sort();
        groups.dedup();
        for group in groups {
            let targets: Vec<(ChannelId, Target)> = moved
                .iter()
                .filter(|held| held.to.category_id == group)
                .map(|held| {
                    let target = held
                        .to
                        .place
                        .map_or(Target::Position(held.to.position), Target::Place);
                    (held.channel_id, target)
                })
                .collect();
            order_channels(connection, &access, now, (workspace_id, group), &targets).await?;
        }
        let places = crate::access::channel_order::channel_ranks(
            &access
                .channel_order(connection, workspace_id, &workspace)
                .await?,
        );
        for held in &moved {
            let (record, _) = access.channel(connection, held.channel_id).await?;
            let position = places.get(&held.channel_id).copied().unwrap_or(0);
            let updated = AuditEvent::ChannelUpdated {
                name: record.channel.name.clone(),
                topic: record.channel.topic.clone(),
                category_id: record.channel.category_id,
                position,
                permanent_messages: record.channel.permanent_messages,
            };
            let about = About::Channel(workspace_id, held.channel_id);
            audit::record(connection, now, Some(access.account_id), about, &updated).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// When a change of a channel sets its place among the channels of its group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Order {
    /// With the change, and its audit entry with it.
    Now,
    /// After the other changes of the request, which records the audit entries then.
    Later,
}

/// Changes a channel. `new_name` is the validated name of `change`.
pub(crate) async fn update_channel_in(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    channel_id: ChannelId,
    new_name: Option<&str>,
    change: &UpdateChannel,
    order: Order,
) -> ApiResult<Channel> {
    let (record, scope) = access.channel(connection, channel_id).await?;
    access.require(Request::Channel {
        scope: &scope,
        permission: WorkspacePermission::ManageChannel,
    })?;
    record.require_open()?;

    let mut channel = record.channel;
    if let Some(topic) = &change.topic {
        channel.topic = validate::channel_topic(topic)?;
    }
    let permanent_before = channel.permanent_messages;
    let category_before = channel.category_id;
    let name_before = channel.name.clone();
    if let Some(name) = new_name {
        name.clone_into(&mut channel.name);
    }
    let mut moved_rules = super::rules::MovedRules::Unchanged;
    if let Some(category_id) = change.category_id {
        match category_id {
            Some(category_id) if Some(category_id) != channel.category_id => {
                require_category(connection, access, channel.workspace_id, category_id).await?;
            }
            // A channel enters the root of the list with what a channel created there needs
            // (ADR 0304).
            None if channel.category_id.is_some() => {
                require_root(connection, access, channel.workspace_id).await?;
            }
            _ => {}
        }
        moved_rules =
            super::rules::check_move(connection, access, &scope, &channel, category_id).await?;
        if let super::rules::MovedRules::Detach = moved_rules {
            crate::access::inheritance::detach_channels(connection, &[channel_id]).await?;
        }
        channel.category_id = category_id;
    }
    // A channel that enters another group starts at its end; a position is then a place
    // among the channels of the group that the caller sees (ADR 0304).
    let moved_group = channel.category_id != category_before;
    if moved_group {
        channel.position =
            end_of_group(connection, channel.workspace_id, channel.category_id).await?;
    }
    if let Some(permanent_messages) = change.permanent_messages {
        channel.permanent_messages = permanent_messages;
    }
    let updated = sqlx::query(
        "update channels
         set name = $2, category_id = $3, position = $4, permanent_messages = $5, topic = $6
         where id = $1 and deleted_at is null",
    )
    .bind(channel_id)
    .bind(&channel.name)
    .bind(channel.category_id)
    .bind(channel.position)
    .bind(channel.permanent_messages)
    .bind(&channel.topic)
    .execute(&mut *connection)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    if moved_group {
        let group = (channel.workspace_id, category_before);
        close_channel_gap(connection, now, group, Some(channel_id)).await?;
    }
    if let (Order::Now, Some(position)) = (order, change.position) {
        let group = (channel.workspace_id, channel.category_id);
        order_channels(
            connection,
            access,
            now,
            group,
            &[(channel_id, Target::Position(position))],
        )
        .await?;
    }
    // A request of several moves reads the places once, after its last move.
    if order == Order::Now {
        channel.position = access
            .channel_place(connection, channel.workspace_id, channel_id)
            .await?;
    }
    // The setting of permanent messages has an entry and a system line of its own. A
    // request that names only that setting records nothing else.
    let about = About::Channel(channel.workspace_id, channel_id);
    let only_permanence = change.permanent_messages.is_some()
        && new_name.is_none()
        && change.topic.is_none()
        && change.category_id.is_none()
        && change.position.is_none();
    if !only_permanence && order == Order::Now {
        let updated = AuditEvent::ChannelUpdated {
            name: channel.name.clone(),
            topic: channel.topic.clone(),
            category_id: channel.category_id,
            position: channel.position,
            permanent_messages: channel.permanent_messages,
        };
        audit::record(connection, now, Some(access.account_id), about, &updated).await?;
    }
    // A rename has a line with both names; a move in the list or to another category has
    // none: the audit log keeps it (ADR 0408).
    if channel.name != name_before {
        let actor = access.account_id;
        crate::messages::rename_line(
            connection,
            now,
            channel_id,
            actor,
            &name_before,
            &channel.name,
        )
        .await?;
    }
    if channel.permanent_messages != permanent_before {
        let turned = if channel.permanent_messages {
            AuditEvent::PermanentMessagesTurnedOn
        } else {
            AuditEvent::PermanentMessagesTurnedOff
        };
        audit::record(connection, now, Some(access.account_id), about, &turned).await?;
    }
    let updated = Event::ChannelUpdated {
        channel: channel.clone(),
    };
    events::publish(connection, now, &updated).await?;
    let ids = (channel.workspace_id, channel_id);
    let actor = Some(access.account_id);
    match moved_rules {
        super::rules::MovedRules::Unchanged => {}
        super::rules::MovedRules::Detach => {
            super::rules::announce_sync(connection, now, actor, ids, None, false).await?;
        }
        super::rules::MovedRules::Take(category_id) => {
            crate::access::rules::sync_channels(connection, &[channel_id]).await?;
            super::rules::announce_sync(connection, now, actor, ids, Some(category_id), true)
                .await?;
        }
    }
    if channel.category_id != category_before {
        events::permissions_changed(
            connection,
            now,
            Affected::Workspace(channel.workspace_id),
            Some(channel.workspace_id),
        )
        .await?;
    }
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

    super::afk::require_replace_before_removal(connection, channel_id).await?;
    let removed = Removed {
        id: channel_id,
        workspace_id: record.channel.workspace_id,
        name: &record.channel.name,
        permanent_message_count: record.permanent_message_count,
    };
    remove_channel(connection, calls, now, &removed, Some(access.account_id)).await?;
    if record.archived_at.is_some() {
        return Ok(());
    }
    left_lists(connection, now, &record.channel).await
}

/// A category is listed to a person while it holds a channel of the lists that the person
/// sees, or while the person manages it (ADR 0304). A channel that leaves the lists by a
/// deletion or an archive can be the last such channel, so each person of the workspace
/// reads the lists again, as after a move.
pub(crate) async fn left_lists(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    channel: &Channel,
) -> ApiResult<()> {
    // A channel of the root leaves no category; the open channels after it in the root then
    // have other places for a person who does not see every channel of the root (ADR 0304).
    if channel.category_id.is_none() {
        if followed_in_group(connection, channel).await? {
            events::log::structure_reordered(connection, now, channel.workspace_id).await?;
        }
        return Ok(());
    }
    let workspace_id = channel.workspace_id;
    events::permissions_changed(
        connection,
        now,
        Affected::Workspace(workspace_id),
        Some(workspace_id),
    )
    .await
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
    let deleted: Option<Option<CategoryId>> = sqlx::query_scalar(
        "update channels set deleted_at = $2
         where id = $1 and deleted_at is null and permanent_message_count = $3
         returning category_id",
    )
    .bind(channel.id)
    .bind(now)
    .bind(channel.permanent_message_count)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(category_id) = deleted else {
        return Err(ApiError::conflict("the channel changed during the request"));
    };
    let group = (channel.workspace_id, category_id);
    close_channel_gap(connection, now, group, Some(channel.id)).await?;
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
    sqlx::query("delete from channel_person_overrides where channel_id = $1")
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
    let (_, scope) = access
        .visible_category(connection, workspace_id, category_id)
        .await?;
    access.require(Request::Channel {
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
    // The channels of the category go to the root, which needs manage channel in the
    // workspace, as a move there does (ADR 0304).
    let holds_channels: bool = sqlx::query_scalar(
        "select exists (select 1 from channels where category_id = $1 and deleted_at is null)",
    )
    .bind(category_id)
    .fetch_one(&mut *connection)
    .await?;
    if holds_channels {
        require_root(connection, access, workspace_id).await?;
    }
    let children: Vec<ChannelId> =
        sqlx::query_scalar("select id from channels where category_id=$1")
            .bind(category_id)
            .fetch_all(&mut *connection)
            .await?;
    let synced = crate::access::rules::synced_children(connection, category_id).await?;
    crate::access::inheritance::detach_channels(connection, &children).await?;
    for channel_id in synced {
        let detached = Event::ChannelRulesSynced {
            workspace_id,
            channel_id,
            synced: false,
        };
        events::publish(connection, now, &detached).await?;
    }
    // The rows of deleted channels refer to the category as well. The channels go after the
    // channels of the root, in their order, numbered on from the end of the root.
    let end = end_of_group(connection, workspace_id, None).await?;
    sqlx::query(
        "update channels c set category_id = null, position = $2 + o.place
         from (select id, (row_number() over (order by position, id) - 1)::integer as place
               from channels where category_id = $1) o
         where c.id = o.id",
    )
    .bind(category_id)
    .bind(end)
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
    close_category_gap(connection, now, workspace_id).await?;
    close_channel_gap(connection, now, (workspace_id, None), None).await?;
    audit::record(
        connection,
        now,
        Some(access.account_id),
        About::Workspace(workspace_id),
        &AuditEvent::CategoryDeleted { category_id, name },
    )
    .await?;
    events::permissions_changed(
        connection,
        now,
        Affected::Workspace(workspace_id),
        Some(workspace_id),
    )
    .await
}
