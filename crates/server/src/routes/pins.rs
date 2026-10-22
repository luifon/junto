//! Pins keep a reference to current channel content.
use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    extract::{ApiPath, Bearer},
    messages::{self, Place},
    read,
    state::AppState,
    transact,
};
use axum::{Json, extract::State, http::StatusCode};
use junto_authz::{Request, WorkspacePermission};
use junto_protocol::{
    ids::{ChannelId, MessageId},
    message::Message,
};

#[utoipa::path(get,path="/api/v1/channels/{channel_id}/pins",tag="messages",params(("channel_id"=ChannelId,Path)),security(("session"=[])),responses((status=200,body=Vec<Message>),ApiError))]
pub async fn list(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<ChannelId>,
) -> ApiResult<Json<Vec<Message>>> {
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        let place = Place::channel(connection, &access, id).await?;
        super::search::read_rate(&app, &access, &mut counted)?;
        let ids = sqlx::query_scalar(
            "select message_id from message_pins where channel_id=$1 order by pinned_at,message_id",
        )
        .bind(id)
        .fetch_all(&mut *connection)
        .await?;
        messages::load_messages(connection, &access, &place, &ids).await
    })
    .map(Json)
}

async fn set(app: &AppState, bearer: &Bearer, id: MessageId, pin: bool) -> ApiResult<()> {
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, bearer, app.now()).await?;
        let (place, message) =
            messages::get(connection, &access, id, messages::Purpose::Change).await?;
        let scope = place.scope.as_ref().ok_or_else(ApiError::not_found)?;
        access.require(Request::Channel {
            scope,
            permission: WorkspacePermission::ManageMessages,
        })?;
        if message.deleted || message.system.is_some() {
            return Err(ApiError::invalid("only authored messages can be pinned"));
        }
        place.require_open()?;
        super::messages::rate(app, &access, &mut counted)?;
        let changed = if pin {
            let count: i64 = sqlx::query_scalar(
                "select count(*) from message_pins where channel_id=$1 and message_id<>$2",
            )
            .bind(place.id)
            .bind(id)
            .fetch_one(&mut *connection)
            .await?;
            if count >= 100 {
                return Err(ApiError::limit_reached("a channel has at most 100 pins"));
            }
            sqlx::query("insert into message_pins(message_id,channel_id,pinned_by,pinned_at) values($1,$2,$3,$4) on conflict do nothing")
                .bind(id)
                .bind(place.id)
                .bind(access.account_id)
                .bind(app.now())
                .execute(&mut *connection)
                .await?
                .rows_affected()
        } else {
            sqlx::query("delete from message_pins where message_id=$1")
                .bind(id)
                .execute(&mut *connection)
                .await?
                .rows_affected()
        };
        if changed > 0 {
            messages::changed(
                connection,
                app.now(),
                place.value,
                id,
                message.position,
                false,
            )
            .await?;
        }
        Ok(())
    })
}

#[utoipa::path(put,path="/api/v1/messages/{message_id}/pin",tag="messages",params(("message_id"=MessageId,Path)),security(("session"=[])),responses((status=204),ApiError))]
pub async fn pin(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
) -> ApiResult<StatusCode> {
    set(&app, &bearer, id, true).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(delete,path="/api/v1/messages/{message_id}/pin",tag="messages",params(("message_id"=MessageId,Path)),security(("session"=[])),responses((status=204),ApiError))]
pub async fn unpin(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<MessageId>,
) -> ApiResult<StatusCode> {
    set(&app, &bearer, id, false).await?;
    Ok(StatusCode::NO_CONTENT)
}
