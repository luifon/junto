//! Message history, writes, reactions and per-account read positions.

use crate::access::Access;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery, Bearer};
use crate::state::AppState;
use crate::{
    events,
    messages::{self, Place, Purpose, document},
    read, store, transact,
};
use axum::{Json, extract::State, http::StatusCode};
use junto_authz::{Request, ServerPermission, WorkspacePermission};
use junto_protocol::{
    error::ErrorCode,
    events::Event,
    ids::{ChannelId, ConversationId, MessageId},
    message::*,
};
use serde::Deserialize;
use sqlx::types::Json as SqlJson;

#[derive(Clone, Copy)]
pub(super) enum Target {
    Channel(ChannelId),
    Direct(ConversationId),
    Thread(MessageId),
}
impl Target {
    async fn open(
        self,
        connection: &mut sqlx::PgConnection,
        access: &Access,
        purpose: Purpose,
    ) -> ApiResult<Place> {
        match self {
            Self::Channel(id) => Place::channel(connection, access, id).await,
            Self::Direct(id) => Place::direct(connection, access, id, purpose).await,
            Self::Thread(id) => Ok(super::threads::root(connection, access, id, purpose)
                .await?
                .0),
        }
    }
}
#[derive(Debug, Deserialize, utoipa::IntoParams)]
pub struct Page {
    pub before: Option<u64>,
    pub after: Option<u64>,
    pub limit: Option<u32>,
    pub before_message: Option<MessageId>,
    pub after_message: Option<MessageId>,
}
impl Page {
    pub(super) fn validate(&self) -> ApiResult<i64> {
        let limit = self.limit.unwrap_or(50);
        if !(1..=200).contains(&limit)
            || [
                self.before.is_some(),
                self.after.is_some(),
                self.before_message.is_some(),
                self.after_message.is_some(),
            ]
            .into_iter()
            .filter(|present| *present)
            .count()
                > 1
            || self
                .before
                .into_iter()
                .chain(self.after)
                .any(|position| position > i64::MAX as u64)
        {
            return Err(ApiError::invalid(
                "use limit 1 to 200 and one nonnegative signed 64-bit before or after position",
            ));
        }
        Ok(i64::from(limit))
    }
}
pub(super) fn rate(app: &AppState, access: &Access, counted: &mut bool) -> ApiResult<()> {
    if !*counted {
        app.message_writes
            .attempt_of(access.account_id, app.now())
            .map_err(|error| {
                ApiError::new(ErrorCode::RateLimited, "too many message changes")
                    .retry_after(error.retry_after)
            })?;
        *counted = true;
    }
    Ok(())
}
async fn history(
    app: &AppState,
    bearer: &Bearer,
    target: Target,
    page: &Page,
) -> ApiResult<MessagePage> {
    let limit = page.validate()?;
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, bearer, app.now()).await?;
        let place = target.open(connection, &access, Purpose::Read).await?;
        if !counted {
            app.message_history
                .attempt_of(access.account_id, app.now())
                .map_err(|error| {
                    ApiError::new(ErrorCode::RateLimited, "too many history requests")
                        .retry_after(error.retry_after)
                })?;
            counted = true;
        }
        let ascending = page.after.is_some() || page.after_message.is_some();
        let (comparison, order) = if ascending {
            (">", "asc")
        } else {
            ("<", "desc")
        };
        let cursor = page.before_message.or(page.after_message);
        let position = if let Some(id) = cursor {
            let (parent, message) = messages::get(connection, &access, id, Purpose::Read).await?;
            if parent.id != place.id {
                return Err(ApiError::not_found());
            }
            message.position as i64
        } else {
            page.after.or(page.before).unwrap_or(i64::MAX as u64) as i64
        };
        let bound = if cursor.is_some() {
            format!("(m.position,m.id) {comparison} ($3,$5)")
        } else {
            format!("m.position {comparison} $3 and $5::uuid is null")
        };
        let query = format!(
            "select m.id from messages m
             where m.place_id=$1 and m.thread_root is null and {bound} and {}
             order by m.position {order},m.id {order} limit $4",
            messages::VISIBLE,
        );
        let mut ids: Vec<MessageId> = sqlx::query_scalar(sqlx::AssertSqlSafe(query))
            .bind(place.id)
            .bind(access.account_id)
            .bind(position)
            .bind(limit + 1)
            .bind(cursor)
            .fetch_all(&mut *connection)
            .await?;
        let has_more = ids.len() > limit as usize;
        ids.truncate(limit as usize);
        if !ascending {
            ids.reverse();
        }
        let messages = messages::load_messages(connection, &access, &place, &ids).await?;
        Ok(MessagePage { messages, has_more })
    })
}
pub(super) async fn send(
    app: &AppState,
    bearer: &Bearer,
    target: Target,
    body: &SendMessage,
) -> ApiResult<Message> {
    document::validate(&body.document)?;
    if !document::has_content(&body.document) && body.attachments.is_empty() {
        return Err(ApiError::invalid("a message needs text or an attachment"));
    }
    let id = MessageId::generate();
    let now = app.now();
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, bearer, now).await?;
        let place = target.open(connection, &access, Purpose::Change).await?;
        place.require_open()?;
        rate(app, &access, &mut counted)?;
        if let Some(reply) = body.reply_to {
            let (parent, _) = messages::get(connection, &access, reply, Purpose::Read).await?;
            if parent.id != place.id {
                return Err(ApiError::invalid(
                    "a reply stays in its channel or conversation",
                ));
            }
        }
        let thread_root = match target {
            Target::Thread(id) => Some(id),
            _ => None,
        };
        if let (Some(root), Some(reply)) = (thread_root, body.reply_to) {
            let (_, replied) = messages::get(connection, &access, reply, Purpose::Read).await?;
            if reply != root && replied.thread_root != Some(root) {
                return Err(ApiError::invalid("a thread reply stays in its thread"));
            }
        }
        let position = place.next(connection).await?;
        let permanent: bool = match place.value {
            MessagePlace::Channel { channel_id, .. } => {
                sqlx::query_scalar("select permanent_messages from channels where id=$1")
                    .bind(channel_id)
                    .fetch_one(&mut *connection)
                    .await?
            }
            _ => false,
        };
        sqlx::query("insert into messages(id,place_id,position,author_id,created_at,document,reply_to,permanent,thread_root)
             values($1,$2,$3,$4,$5,$6,$7,$8,$9)")

                        .bind(id)
                        .bind(place.id)
                        .bind(position)
                        .bind(access.account_id)
                        .bind(now)
                        .bind(SqlJson(&body.document))
                        .bind(body.reply_to)
                        .bind(permanent)
                        .bind(thread_root)
                        .execute(&mut *connection)
                        .await?;
        if permanent {
            sqlx::query(
                "update channels set permanent_message_count=permanent_message_count+1
                 where id=$1",
            )
            .bind(place.id)
            .execute(&mut *connection)
            .await?;
        }
        messages::mentions(connection, &access, &place, id, &body.document, now).await?;
        messages::uploads::attach(connection, app, &access, &place, id, &body.attachments).await?;
        let message_event = uuid::Uuid::now_v7();
        messages::changed_at(
            connection,
            now,
            place.value,
            id,
            position as u64,
            false,
            message_event,
        )
        .await?;
        messages::notifications::create(connection, now, id, None, message_event).await?;
        if let Some(root) = thread_root {
            messages::threads::advance(connection, now, &access, root, position).await?;
            let (_, root_message) = messages::get(connection, &access, root, Purpose::Read).await?;
            messages::changed(
                connection,
                now,
                place.value,
                root,
                root_message.position,
                false,
            )
            .await?;
        } else {
            advance_read(connection, now, &access, &place, position).await?;
        }
        Ok(messages::get(connection, &access, id, Purpose::Read)
            .await?
            .1)
    })
}

#[utoipa::path(get,path="/api/v1/messages/{message_id}",tag="messages",params(("message_id"=MessageId,Path)),security(("session"=[])),responses((status=200,body=Message),ApiError))]
pub async fn get_message(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
) -> ApiResult<Json<Message>> {
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        if !counted {
            app.message_reads
                .attempt_of(access.account_id, app.now())
                .map_err(|error| {
                    ApiError::new(ErrorCode::RateLimited, "too many message reads")
                        .retry_after(error.retry_after)
                })?;
            counted = true;
        }
        Ok(messages::get(connection, &access, id, Purpose::Read)
            .await?
            .1)
    })
    .map(Json)
}
#[utoipa::path(put,path="/api/v1/messages/{message_id}",tag="messages",params(("message_id"=MessageId,Path)),request_body=EditMessage,security(("session"=[])),responses((status=200,body=Message),ApiError))]
pub async fn edit_message(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
    ApiJson(body): ApiJson<EditMessage>,
) -> ApiResult<Json<Message>> {
    document::validate(&body.document)?;
    let now = app.now();
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (place, message) = messages::get(connection, &access, id, Purpose::Change).await?;
        if message.deleted {
            return Err(ApiError::not_found());
        }
        place.require_open()?;
        place.edit(&access, &message)?;
        rate(&app, &access, &mut counted)?;
        if !document::has_content(&body.document) && message.attachments.is_empty() {
            return Err(ApiError::invalid("a message needs text or an attachment"));
        }
        if message.document.as_ref() == Some(&body.document) {
            return Ok(message);
        }
        let recipients =
            messages::mentions(connection, &access, &place, id, &body.document, now).await?;
        sqlx::query("update messages set document=$2,edited_at=$3 where id=$1")
            .bind(id)
            .bind(SqlJson(&body.document))
            .bind(now)
            .execute(&mut *connection)
            .await?;
        let message_event = uuid::Uuid::now_v7();
        messages::changed_at(
            connection,
            now,
            place.value,
            id,
            message.position,
            false,
            message_event,
        )
        .await?;
        messages::notifications::create(connection, now, id, Some(&recipients), message_event)
            .await?;
        Ok(messages::get(connection, &access, id, Purpose::Read)
            .await?
            .1)
    })
    .map(Json)
}
#[utoipa::path(delete,path="/api/v1/messages/{message_id}",tag="messages",params(("message_id"=MessageId,Path)),security(("session"=[])),responses((status=204),ApiError))]
pub async fn delete_message(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (place, message) = messages::get(connection, &access, id, Purpose::Change).await?;
        if message.deleted {
            return Err(ApiError::not_found());
        }
        place.require_open()?;
        if let Some(system) = &message.system {
            if system.action != "recording_ready" {
                return Err(ApiError::new(
                    ErrorCode::Forbidden,
                    "system messages cannot be deleted",
                ));
            }
            let recording_id = system.recording_id.ok_or_else(ApiError::not_found)?;
            let (recording, scope) =
                super::recordings::readable(connection, &access, recording_id, true).await?;
            access.require(Request::Channel {
                scope: &scope,
                permission: WorkspacePermission::ManageRecordings,
            })?;
            if recording.state.is_active()
                || recording.state == junto_protocol::recording::RecordingState::Processing
            {
                return Err(ApiError::conflict("recording is still processing"));
            }
            rate(&app, &access, &mut counted)?;
            crate::recordings::store::remove(connection, now, &recording, Some(access.account_id))
                .await?;
            return Ok(());
        }
        if message.permanent {
            access.require(Request::Server(ServerPermission::Administrator))?;
        } else if message.author_id != Some(access.account_id) {
            let scope = place.scope.as_ref().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::Forbidden,
                    "only the author deletes a direct message",
                )
            })?;
            access.require(Request::Channel {
                scope,
                permission: WorkspacePermission::ManageMessages,
            })?;
        }
        rate(&app, &access, &mut counted)?;
        if message.permanent
            && let MessagePlace::Channel {
                workspace_id,
                channel_id,
            } = place.value
        {
            crate::audit::record(
                connection,
                now,
                Some(access.account_id),
                crate::audit::About::Channel(workspace_id, channel_id),
                &junto_protocol::audit::AuditEvent::PermanentMessageDeleted { message_id: id },
            )
            .await?;
        }
        messages::remove(connection, now, &place, &message).await
    })?;
    app.recordings.process_now();
    Ok(StatusCode::NO_CONTENT)
}
#[utoipa::path(put,path="/api/v1/messages/{message_id}/reaction",tag="messages",params(("message_id"=MessageId,Path)),request_body=SetReaction,security(("session"=[])),responses((status=204),ApiError))]
pub async fn add_reaction(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
    ApiJson(body): ApiJson<SetReaction>,
) -> ApiResult<StatusCode> {
    reaction(&app, &bearer, id, &body.emoji, true).await?;
    Ok(StatusCode::NO_CONTENT)
}
#[utoipa::path(delete,path="/api/v1/messages/{message_id}/reaction/{emoji}",tag="messages",params(("message_id"=MessageId,Path),("emoji"=String,Path)),security(("session"=[])),responses((status=204),ApiError))]
pub async fn remove_reaction(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((id, emoji)): ApiPath<(MessageId, String)>,
) -> ApiResult<StatusCode> {
    reaction(&app, &bearer, id, &emoji, false).await?;
    Ok(StatusCode::NO_CONTENT)
}
async fn reaction(
    app: &AppState,
    bearer: &Bearer,
    id: MessageId,
    emoji: &str,
    add: bool,
) -> ApiResult<()> {
    if emoji.len() > 64 || emojis::get(emoji).is_none() {
        return Err(ApiError::invalid("a reaction is one standard emoji"));
    }
    let emoji = emojis::get(emoji)
        .ok_or_else(|| ApiError::invalid("a reaction is one standard emoji"))?
        .as_str();
    let now = app.now();
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, bearer, now).await?;
        let (place, message) = messages::get(connection, &access, id, Purpose::Change).await?;
        if message.deleted {
            return Err(ApiError::not_found());
        }
        place.require_open()?;
        rate(app, &access, &mut counted)?;
        if add
            && message.reactions.len() >= 20
            && !message
                .reactions
                .iter()
                .any(|reaction| reaction.emoji == emoji)
        {
            return Err(ApiError::limit_reached(
                "a message has at most 20 different reactions",
            ));
        }
        let sql = if add {
            "insert into message_reactions(message_id,account_id,emoji) values($1,$2,$3)
                 on conflict do nothing"
        } else {
            "delete
                 from message_reactions
                 where message_id=$1
                 and account_id=$2
                 and emoji=$3"
        };
        let changed = sqlx::query(sql)
            .bind(id)
            .bind(access.account_id)
            .bind(emoji)
            .execute(&mut *connection)
            .await?
            .rows_affected();
        if changed > 0 {
            messages::changed(connection, now, place.value, id, message.position, false).await?;
        }
        Ok(())
    })
}
#[utoipa::path(post,path="/api/v1/conversations",tag="messages",request_body=OpenConversation,security(("session"=[])),responses((status=200,body=Conversation),ApiError))]
pub async fn open_conversation(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<OpenConversation>,
) -> ApiResult<Json<Conversation>> {
    let id = ConversationId::generate();
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        if body.account_id == access.account_id {
            return Err(ApiError::invalid(
                "a conversation has two different accounts",
            ));
        }
        if !store::may_reach(connection, &access, body.account_id).await? {
            return Err(ApiError::not_found());
        }
        rate(&app, &access, &mut counted)?;
        let low = access.account_id.min(body.account_id);
        let high = access.account_id.max(body.account_id);
        let (conversation, is_new) = Place::conversation_between(connection, low, high, id).await?;
        Place::direct(connection, &access, conversation.id, Purpose::Change)
            .await?
            .ensure(connection)
            .await?;
        if is_new {
            events::publish(
                connection,
                app.now(),
                &Event::ConversationOpened {
                    conversation: conversation.clone(),
                },
            )
            .await?;
        }
        Ok(conversation)
    })
    .map(Json)
}
#[utoipa::path(get,path="/api/v1/conversations",tag="messages",security(("session"=[])),responses((status=200,body=Vec<Conversation>),ApiError))]
pub async fn list_conversations(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<Conversation>>> {
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        Place::conversations(connection, &access).await
    })
    .map(Json)
}
async fn advance_read(
    connection: &mut sqlx::PgConnection,
    now: time::OffsetDateTime,
    access: &Access,
    place: &Place,
    position: i64,
) -> ApiResult<()> {
    let changed = sqlx::query(
        "insert into message_reads(place_id,account_id,position) values($1,$2,$3)
         on conflict(place_id,account_id) do update set position=excluded.position
         where message_reads.position<excluded.position",
    )
    .bind(place.id)
    .bind(access.account_id)
    .bind(position)
    .execute(&mut *connection)
    .await?
    .rows_affected();
    if changed > 0 {
        sqlx::query("update notifications n set read_at=$1 from messages m where n.message_id=m.id and n.account_id=$2 and m.place_id=$3 and m.thread_root is null and m.position<=$4 and n.read_at is null")
            .bind(now).bind(access.account_id).bind(place.id).bind(position).execute(&mut *connection).await?;
        events::tell_account(
            connection,
            now,
            access.account_id,
            &Event::MessageRead {
                place: place.value,
                position: position as u64,
            },
        )
        .await?;
    }
    Ok(())
}

async fn mark(
    app: &AppState,
    bearer: &Bearer,
    target: Target,
    body: &MarkRead,
) -> ApiResult<MessageState> {
    let position =
        i64::try_from(body.position).map_err(|_| ApiError::invalid("invalid read position"))?;
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, bearer, app.now()).await?;
        let place = target.open(connection, &access, Purpose::Read).await?;
        rate(app, &access, &mut counted)?;
        place.ensure(connection).await?;
        let head = messages::state(connection, &access, &place, app.now())
            .await?
            .latest as i64;
        if position > head {
            return Err(ApiError::invalid("read position is beyond the history"));
        }
        advance_read(connection, app.now(), &access, &place, position).await?;
        messages::state(connection, &access, &place, app.now()).await
    })
}

#[utoipa::path(get,path="/api/v1/channels/{channel_id}/messages",tag="messages",params(("channel_id"=ChannelId,Path),Page),security(("session"=[])),responses((status=200,body=MessagePage),ApiError))]
pub async fn channel_history(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<ChannelId>,
    ApiQuery(page): ApiQuery<Page>,
) -> ApiResult<Json<MessagePage>> {
    history(&app, &bearer, Target::Channel(id), &page)
        .await
        .map(Json)
}
#[utoipa::path(post,path="/api/v1/channels/{channel_id}/messages",tag="messages",params(("channel_id"=ChannelId,Path)),request_body=SendMessage,security(("session"=[])),responses((status=201,body=Message),ApiError))]
pub async fn send_channel_message(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<ChannelId>,
    ApiJson(body): ApiJson<SendMessage>,
) -> ApiResult<(StatusCode, Json<Message>)> {
    Ok((
        StatusCode::CREATED,
        Json(send(&app, &bearer, Target::Channel(id), &body).await?),
    ))
}
#[utoipa::path(put,path="/api/v1/channels/{channel_id}/messages/read",tag="messages",params(("channel_id"=ChannelId,Path)),request_body=MarkRead,security(("session"=[])),responses((status=200,body=MessageState),ApiError))]
pub async fn read_channel_messages(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<ChannelId>,
    ApiJson(body): ApiJson<MarkRead>,
) -> ApiResult<Json<MessageState>> {
    mark(&app, &bearer, Target::Channel(id), &body)
        .await
        .map(Json)
}

#[utoipa::path(get,path="/api/v1/conversations/{conversation_id}/messages",tag="messages",params(("conversation_id"=ConversationId,Path),Page),security(("session"=[])),responses((status=200,body=MessagePage),ApiError))]
pub async fn conversation_history(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<ConversationId>,
    ApiQuery(page): ApiQuery<Page>,
) -> ApiResult<Json<MessagePage>> {
    history(&app, &bearer, Target::Direct(id), &page)
        .await
        .map(Json)
}
#[utoipa::path(post,path="/api/v1/conversations/{conversation_id}/messages",tag="messages",params(("conversation_id"=ConversationId,Path)),request_body=SendMessage,security(("session"=[])),responses((status=201,body=Message),ApiError))]
pub async fn send_conversation_message(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<ConversationId>,
    ApiJson(body): ApiJson<SendMessage>,
) -> ApiResult<(StatusCode, Json<Message>)> {
    Ok((
        StatusCode::CREATED,
        Json(send(&app, &bearer, Target::Direct(id), &body).await?),
    ))
}
#[utoipa::path(put,path="/api/v1/conversations/{conversation_id}/messages/read",tag="messages",params(("conversation_id"=ConversationId,Path)),request_body=MarkRead,security(("session"=[])),responses((status=200,body=MessageState),ApiError))]
pub async fn read_conversation_messages(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<ConversationId>,
    ApiJson(body): ApiJson<MarkRead>,
) -> ApiResult<Json<MessageState>> {
    mark(&app, &bearer, Target::Direct(id), &body)
        .await
        .map(Json)
}

#[utoipa::path(get,path="/api/v1/channels/{channel_id}/messages/read",tag="messages",params(("channel_id"=ChannelId,Path)),security(("session"=[])),responses((status=200,body=MessageState),ApiError))]
pub async fn channel_message_state(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<ChannelId>,
) -> ApiResult<Json<MessageState>> {
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        let place = Target::Channel(id)
            .open(connection, &access, Purpose::Read)
            .await?;
        messages::state(connection, &access, &place, app.now()).await
    })
    .map(Json)
}

#[utoipa::path(get,path="/api/v1/conversations/{conversation_id}/messages/read",tag="messages",params(("conversation_id"=ConversationId,Path)),security(("session"=[])),responses((status=200,body=MessageState),ApiError))]
pub async fn conversation_message_state(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<ConversationId>,
) -> ApiResult<Json<MessageState>> {
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        let place = Target::Direct(id)
            .open(connection, &access, Purpose::Read)
            .await?;
        messages::state(connection, &access, &place, app.now()).await
    })
    .map(Json)
}
