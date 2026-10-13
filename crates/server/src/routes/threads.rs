//! Thread history and transactional copies into channels.
use axum::{Json, extract::State, http::StatusCode};
use junto_authz::{Request, WorkspacePermission};
use junto_protocol::{
    audit::AuditEvent,
    events::Event,
    ids::{ChannelId, MessageId},
    message::{
        FollowedThreads, ForkThread, MarkRead, Message, MessagePage, MessagePlace, SendMessage,
        SetThreadFollow, ThreadPage, ThreadState,
    },
    structure::Channel,
};
use sqlx::PgConnection;

use super::messages::{Page, Target, rate, send};
use crate::{
    access::Access,
    audit::{self, About},
    db::StoredText,
    error::{ApiError, ApiResult},
    events,
    extract::{ApiJson, ApiPath, ApiQuery, Bearer},
    messages::{self, Place},
    read,
    state::AppState,
    transact, validate,
};

pub(super) async fn root(
    connection: &mut PgConnection,
    access: &Access,
    id: MessageId,
) -> ApiResult<(Place, Message)> {
    let (place, message) = messages::get(connection, access, id).await?;
    let authored_root: bool =
        sqlx::query_scalar("select call_id is null and readers is null from messages where id=$1")
            .bind(id)
            .fetch_one(&mut *connection)
            .await?;
    if !authored_root
        || !matches!(place.value, MessagePlace::Channel { .. })
        || message.thread_root.is_some()
        || message.system.is_some()
    {
        return Err(ApiError::invalid("a thread needs an authored channel root"));
    }
    Ok((place, message))
}

#[utoipa::path(get,path="/api/v1/messages/{message_id}/thread",tag="messages",params(("message_id"=MessageId,Path),Page),security(("session"=[])),responses((status=200,body=ThreadPage),ApiError))]
pub async fn history(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
    ApiQuery(page): ApiQuery<Page>,
) -> ApiResult<Json<ThreadPage>> {
    let limit = page.validate()?;
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        let (place, root) = root(connection, &access, id).await?;
        super::search::read_rate(&app, &access, &mut counted)?;
        let ascending = page.after.is_some() || page.after_message.is_some();
        let cursor = page.after_message.or(page.before_message);
        let position = if let Some(cursor) = cursor {
            let (_, message) = messages::get(connection, &access, cursor).await?;
            if message.thread_root != Some(id) {
                return Err(ApiError::not_found());
            }
            message.position as i64
        } else {
            page.after.or(page.before).unwrap_or(i64::MAX as u64) as i64
        };
        let (cmp, order) = if ascending {
            (">", "asc")
        } else {
            ("<", "desc")
        };
        let query = format!("select id from messages where thread_root=$1 and position {cmp} $2 order by position {order},id {order} limit $3");
        let mut ids: Vec<MessageId> = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
            .bind(id)
            .bind(position)
            .bind(limit + 1)
            .fetch_all(&mut *connection)
            .await?;
        let has_more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        if !ascending {
            ids.reverse();
        }
        let replies = MessagePage {
            messages: messages::load_messages(connection, &access, &place, &ids).await?,
            has_more,
        };
        let allowed = super::search::places(connection, &access).await?;
        let forks = sqlx::query_scalar("select channel_id from thread_forks where root_id=$1 and channel_id=any($2) order by channel_id")
            .bind(id)
            .bind(&allowed.channels)
            .fetch_all(connection)
            .await?;
        Ok(ThreadPage {
            root,
            replies,
            forks,
        })
    })
    .map(Json)
}

#[utoipa::path(post,path="/api/v1/messages/{message_id}/thread",tag="messages",params(("message_id"=MessageId,Path)),request_body=SendMessage,security(("session"=[])),responses((status=201,body=Message),ApiError))]
pub async fn reply(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
    ApiJson(body): ApiJson<SendMessage>,
) -> ApiResult<(StatusCode, Json<Message>)> {
    Ok((
        StatusCode::CREATED,
        Json(send(&app, &bearer, Target::Thread(id), &body).await?),
    ))
}

#[utoipa::path(post,path="/api/v1/messages/{message_id}/fork",tag="messages",params(("message_id"=MessageId,Path)),request_body=ForkThread,security(("session"=[])),responses((status=201,body=Channel),ApiError))]
pub async fn fork(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
    ApiJson(body): ApiJson<ForkThread>,
) -> ApiResult<(StatusCode, Json<Channel>)> {
    let name = validate::name("name", &body.name)?;
    let channel_id = ChannelId::generate();
    let now = app.now();
    let mut counted = false;
    let channel = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (place, _) = root(connection, &access, id).await?;
        place.require_open()?;
        let MessagePlace::Channel {
            workspace_id,
            channel_id: source_id,
        } = place.value
        else {
            return Err(ApiError::not_found());
        };
        access.require(Request::Channel {
            scope: place.scope.as_ref().ok_or_else(ApiError::not_found)?,
            permission: WorkspacePermission::ManageChannel,
        })?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::Workspace {
            scope: &scope,
            permission: WorkspacePermission::ManageChannel,
        })?;
        rate(&app, &access, &mut counted)?;
        let (source, _) = access.channel(connection, source_id).await?;
        let exists: bool =
            sqlx::query_scalar("select exists(select 1 from thread_forks where root_id=$1)")
                .bind(id)
                .fetch_one(&mut *connection)
                .await?;
        if exists {
            return Err(ApiError::conflict("the thread has already been forked"));
        }
        let position: i32 = sqlx::query_scalar("select coalesce(max(position),-1)+1 from channels where workspace_id=$1 and deleted_at is null")
            .bind(workspace_id)
            .fetch_one(&mut *connection)
            .await?;
        let channel = Channel {
            id: channel_id,
            workspace_id,
            category_id: source.channel.category_id,
            kind: body.kind,
            name: name.clone(),
            position,
            permanent_messages: source.channel.permanent_messages,
            user_limit: None,
            hides_occupants: body.kind == junto_protocol::structure::ChannelKind::Voice
                && source.channel.hides_occupants,
        };
        sqlx::query("insert into
             channels(id,workspace_id,category_id,kind,name,position,created_at,permanent_messages,hides_occupants)
             values($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(channel.id)
            .bind(workspace_id)
            .bind(channel.category_id)
            .bind(channel.kind.stored())
            .bind(&name)
            .bind(channel.position)
            .bind(now)
            .bind(channel.permanent_messages)
            .bind(channel.hides_occupants)
            .execute(&mut *connection)
            .await?;
        sqlx::query(
            "insert into channel_overrides(channel_id,role_id,workspace_id,allow,deny) select
             $1,role_id,workspace_id,allow,deny from channel_overrides where channel_id=$2",
        )
        .bind(channel.id)
        .bind(source_id)
        .execute(&mut *connection)
        .await?;
        let target = Place::channel(connection, &access, channel.id).await?;
        target.ensure(connection).await?;
        let linked = sqlx::query("insert into thread_forks(root_id,channel_id) values($1,$2) on conflict(root_id) do nothing")
            .bind(id)
            .bind(channel.id)
            .execute(&mut *connection)
            .await?
            .rows_affected();
        if linked == 0 {
            return Err(ApiError::conflict("the thread has already been forked"));
        }
        let head = copy_thread(connection, now, id, channel.id).await?;
        sqlx::query("update message_places set head=$2,read_floor=$2 where id=$1")
            .bind(channel.id)
            .bind(head)
            .execute(&mut *connection)
            .await?;
        sqlx::query("update channels set permanent_message_count=(select count(*) from messages where place_id=$1 and permanent) where id=$1")
            .bind(channel.id)
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
        events::publish(
            connection,
            now,
            &Event::ChannelCreated {
                channel: channel.clone(),
            },
        )
        .await?;
        events::permissions_changed_by_overrides_of(connection, now, workspace_id, channel.id)
            .await?;
        let (_, source_root) = messages::get(connection, &access, id).await?;
        messages::changed(
            connection,
            now,
            place.value,
            id,
            source_root.position,
            false,
        )
        .await?;
        Ok(channel)
    })?;
    Ok((StatusCode::CREATED, Json(channel)))
}

async fn copy_thread(
    connection: &mut PgConnection,
    now: time::OffsetDateTime,
    root: MessageId,
    target: ChannelId,
) -> ApiResult<i64> {
    sqlx::query("create temporary table fork_copy(old_id uuid primary key,new_id uuid not null,ordinal bigint not null
             unique) on commit drop")
        .execute(&mut *connection)
        .await?;
    let mut cursor = (0_i64, uuid::Uuid::nil());
    let mut head = 0_i64;
    loop {
        let rows: Vec<(MessageId, i64)> = sqlx::query_as("select id,position from messages where (id=$1 or thread_root=$1) and (position,id)>($2,$3) order by
             position,id limit 200")
            .bind(root)
            .bind(cursor.0)
            .bind(cursor.1)
            .fetch_all(&mut *connection)
            .await?;
        if rows.is_empty() {
            break;
        }
        let old: Vec<_> = rows.iter().map(|(id, _)| *id).collect();
        let new: Vec<_> = rows.iter().map(|_| MessageId::generate()).collect();
        sqlx::query("insert into fork_copy select old_id,new_id,$3+ordinal from unnest($1::uuid[],$2::uuid[]) with ordinality
             mapping(old_id,new_id,ordinal)")
            .bind(&old)
            .bind(&new)
            .bind(head)
            .execute(&mut *connection)
            .await?;
        head += rows.len() as i64;
        if let Some((id, position)) = rows.last() {
            cursor = (*position, id.as_uuid());
        }
    }
    let mut copied = 0_i64;
    while copied < head {
        let rows: Vec<(MessageId, MessageId)> = sqlx::query_as(
            "select old_id,new_id from fork_copy where ordinal>$1 order by ordinal limit 200",
        )
        .bind(copied)
        .fetch_all(&mut *connection)
        .await?;
        let old: Vec<_> = rows.iter().map(|(id, _)| *id).collect();
        let new: Vec<_> = rows.iter().map(|(_, id)| *id).collect();
        sqlx::query("insert into
             messages(id,place_id,position,author_id,created_at,edited_at,document,reply_to,permanent,deleted,files_removed_by_retention)
             select
             mapping.new_id,$1,mapping.ordinal,m.author_id,m.created_at,m.edited_at,m.document,parent.new_id,m.permanent,m.deleted,m.files_removed_by_retention
             from fork_copy mapping join messages m on m.id=mapping.old_id left join fork_copy parent on
             parent.old_id=m.reply_to where mapping.old_id=any($2)")
            .bind(target)
            .bind(&old)
            .execute(&mut *connection)
            .await?;
        sqlx::query("insert into message_mentions(place_id,position,message_id,account_id,role_id,everyone,recipients) select
             $1,mapping.ordinal,mapping.new_id,mm.account_id,mm.role_id,mm.everyone,mm.recipients from fork_copy
             mapping join message_mentions mm on mm.message_id=mapping.old_id where mapping.old_id=any($2)")
            .bind(target)
            .bind(&old)
            .execute(&mut *connection)
            .await?;
        sqlx::query("insert into message_mentions_notified(message_id,account_id) select mapping.new_id,n.account_id from
             fork_copy mapping join message_mentions_notified n on n.message_id=mapping.old_id where
             mapping.old_id=any($1)")
            .bind(&old)
            .execute(&mut *connection)
            .await?;
        messages::uploads::copy_attachments(connection, now, target, &old, &new).await?;
        copied += rows.len() as i64;
    }
    Ok(head)
}

#[utoipa::path(get,path="/api/v1/messages/{message_id}/thread/read",tag="messages",params(("message_id"=MessageId,Path)),security(("session"=[])),responses((status=200,body=ThreadState),ApiError))]
pub async fn read_state(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
) -> ApiResult<Json<ThreadState>> {
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        root(connection, &access, id).await?;
        super::search::read_rate(&app, &access, &mut counted)?;
        messages::threads::state(connection, access.account_id, id).await
    })
    .map(Json)
}

#[utoipa::path(put,path="/api/v1/messages/{message_id}/thread/read",tag="messages",params(("message_id"=MessageId,Path)),request_body=MarkRead,security(("session"=[])),responses((status=200,body=ThreadState),ApiError))]
pub async fn mark_read(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
    ApiJson(body): ApiJson<MarkRead>,
) -> ApiResult<Json<ThreadState>> {
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        root(connection, &access, id).await?;
        rate(&app, &access, &mut counted)?;
        let state = messages::threads::state(connection, access.account_id, id).await?;
        if body.position > state.latest {
            return Err(ApiError::invalid("read position is beyond the thread"));
        }
        messages::threads::advance(connection, app.now(), &access, id, body.position as i64)
            .await?;
        messages::threads::state(connection, access.account_id, id).await
    })
    .map(Json)
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
pub struct UnreadQuery {
    pub before: Option<String>,
    pub limit: Option<u32>,
}

#[utoipa::path(get,path="/api/v1/threads/unread",tag="messages",params(UnreadQuery),security(("session"=[])),responses((status=200,body=junto_protocol::message::UnreadThreads),ApiError))]
pub async fn unread(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiQuery(query): ApiQuery<UnreadQuery>,
) -> ApiResult<Json<junto_protocol::message::UnreadThreads>> {
    let limit = query.limit.unwrap_or(200);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::invalid("limit must be 1 to 200"));
    }
    let before = thread_cursor(query.before.as_deref())?;
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        super::search::read_rate(&app, &access, &mut counted)?;
        let allowed = crate::access::visibility::places(connection, &access).await?;
        messages::threads::unread(
            connection,
            access.account_id,
            &allowed.channels,
            before,
            limit,
        )
        .await
    })
    .map(Json)
}

/// Stores the choice of the caller to follow a thread or not (ADR 0412). The caller sees
/// the channel of the thread.
#[utoipa::path(put,path="/api/v1/messages/{message_id}/thread/follow",tag="messages",params(("message_id"=MessageId,Path)),request_body=SetThreadFollow,security(("session"=[])),responses((status=200,body=ThreadState),ApiError))]
pub async fn set_follow(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
    ApiJson(body): ApiJson<SetThreadFollow>,
) -> ApiResult<Json<ThreadState>> {
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        root(connection, &access, id).await?;
        rate(&app, &access, &mut counted)?;
        let choice = Some(body.following);
        messages::threads::set_follow(connection, app.now(), &access, id, choice).await?;
        messages::threads::state(connection, access.account_id, id).await
    })
    .map(Json)
}

/// Removes the choice of the caller for a thread. The caller then follows the thread when
/// it wrote in it or was named in it.
#[utoipa::path(delete,path="/api/v1/messages/{message_id}/thread/follow",tag="messages",params(("message_id"=MessageId,Path)),security(("session"=[])),responses((status=204),ApiError))]
pub async fn remove_follow(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
) -> ApiResult<StatusCode> {
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        root(connection, &access, id).await?;
        rate(&app, &access, &mut counted)?;
        messages::threads::set_follow(connection, app.now(), &access, id, None).await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(serde::Deserialize, utoipa::IntoParams)]
pub struct FollowedQuery {
    /// The value `next` of the page before.
    pub before: Option<String>,
    /// From 1 to 200. Absent: 50.
    pub limit: Option<u32>,
}

fn thread_cursor(value: Option<&str>) -> ApiResult<Option<(uuid::Uuid, uuid::Uuid)>> {
    value
        .map(|value| {
            let invalid = || ApiError::invalid("invalid thread cursor");
            let (newest, root) = value.split_once(':').ok_or_else(invalid)?;
            Ok::<_, ApiError>((
                newest.parse::<uuid::Uuid>().map_err(|_| invalid())?,
                root.parse::<uuid::Uuid>().map_err(|_| invalid())?,
            ))
        })
        .transpose()
}

/// The threads of a channel that the caller follows, the one with the newest reply first,
/// in pages. Threads without unread replies are included (ADR 0413). The caller sees the
/// channel.
#[utoipa::path(get,path="/api/v1/channels/{channel_id}/threads",tag="messages",params(("channel_id"=ChannelId,Path),FollowedQuery),security(("session"=[])),responses((status=200,body=FollowedThreads),ApiError))]
pub async fn followed(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
    ApiQuery(query): ApiQuery<FollowedQuery>,
) -> ApiResult<Json<FollowedThreads>> {
    let limit = query.limit.unwrap_or(50);
    if !(1..=200).contains(&limit) {
        return Err(ApiError::invalid("limit must be 1 to 200"));
    }
    let before = thread_cursor(query.before.as_deref())?;
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        Place::channel(connection, &access, channel_id).await?;
        super::search::read_rate(&app, &access, &mut counted)?;
        messages::threads::followed(connection, access.account_id, channel_id, before, limit).await
    })
    .map(Json)
}
