use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    events,
    extract::{ApiJson, ApiPath, Bearer},
    messages::{Place, Purpose, groups},
    read,
    state::AppState,
    store, transact,
};
use axum::{Json, extract::State, http::StatusCode};
use junto_protocol::{
    events::Event,
    ids::ConversationId,
    message::{
        CreateGroupConversation, GroupConversation, GroupMembership, OpenConversation,
        RenameGroupConversation,
    },
};

async fn changed(
    connection: &mut sqlx::PgConnection,
    app: &AppState,
    id: ConversationId,
) -> ApiResult<GroupConversation> {
    let group = groups::load(connection, id).await?;
    events::publish(
        connection,
        app.now(),
        &Event::GroupConversationChanged {
            group: group.clone(),
        },
    )
    .await?;
    Ok(group)
}

#[utoipa::path(post,path="/api/v1/group-conversations",tag="messages",request_body=CreateGroupConversation,security(("session"=[])),responses((status=201,body=GroupConversation),ApiError))]
pub async fn create_group(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<CreateGroupConversation>,
) -> ApiResult<(StatusCode, Json<GroupConversation>)> {
    let name = groups::name(&body.name)?;
    let id = ConversationId::generate();
    let mut counted = false;
    let group = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        super::messages::rate(&app, &access, &mut counted)?;
        let mut accounts = body.account_ids.clone();
        accounts.push(access.account_id);
        accounts.sort_unstable();
        accounts.dedup();
        if !(3..=10).contains(&accounts.len()) {
            return Err(ApiError::invalid(
                "a group starts with 3 to 10 different people",
            ));
        }
        for other in accounts.iter().filter(|id| **id != access.account_id) {
            if !store::may_reach(connection, &access, *other).await? {
                return Err(ApiError::not_found());
            }
        }
        if !groups::eligible(connection, &accounts).await? {
            return Err(ApiError::not_found());
        }
        groups::reserve_memberships(connection, &accounts).await?;
        sqlx::query("insert into conversations(id,name) values($1,$2)")
            .bind(id)
            .bind(&name)
            .execute(&mut *connection)
            .await?;
        sqlx::query("insert into conversation_members(conversation_id,account_id) select $1,unnest($2::uuid[])").bind(id).bind(&accounts).execute(&mut *connection).await?;
        Place::direct(connection, &access, id, Purpose::Change)
            .await?
            .ensure(connection)
            .await?;
        changed(connection, &app, id).await
    })?;
    Ok((StatusCode::CREATED, Json(group)))
}

#[utoipa::path(get,path="/api/v1/group-conversations",tag="messages",security(("session"=[])),responses((status=200,body=Vec<GroupConversation>),ApiError))]
pub async fn list_groups(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<GroupConversation>>> {
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        groups::list(connection, &access).await
    })
    .map(Json)
}

#[utoipa::path(get,path="/api/v1/group-conversations/memberships",tag="messages",security(("session"=[])),responses((status=200,body=Vec<GroupMembership>),ApiError))]
pub async fn list_group_memberships(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<GroupMembership>>> {
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        if !groups::supported() || !access.allows(junto_authz::Request::DirectMessage(
            junto_authz::Participation::Participant,
        )) {
            return Err(ApiError::not_found());
        }
        let rows: Vec<(ConversationId, String)> = sqlx::query_as(
            "select c.id,c.name from conversations c join conversation_members m on m.conversation_id=c.id
             where m.account_id=$1 and c.name is not null order by lower(c.name),c.id",
        ).bind(access.account_id).fetch_all(connection).await?;
        Ok(rows.into_iter().map(|(id,name)| GroupMembership { id,name }).collect())
    }).map(Json)
}

#[utoipa::path(patch,path="/api/v1/group-conversations/{conversation_id}",tag="messages",params(("conversation_id"=ConversationId,Path)),request_body=RenameGroupConversation,security(("session"=[])),responses((status=200,body=GroupConversation),ApiError))]
pub async fn rename_group(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<ConversationId>,
    ApiJson(body): ApiJson<RenameGroupConversation>,
) -> ApiResult<Json<GroupConversation>> {
    let name = groups::name(&body.name)?;
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        groups::readable(connection, &access, id).await?;
        super::messages::rate(&app, &access, &mut counted)?;
        sqlx::query("update conversations set name=$2 where id=$1")
            .bind(id)
            .bind(&name)
            .execute(&mut *connection)
            .await?;
        changed(connection, &app, id).await
    })
    .map(Json)
}

#[utoipa::path(post,path="/api/v1/group-conversations/{conversation_id}/members",tag="messages",params(("conversation_id"=ConversationId,Path)),request_body=OpenConversation,security(("session"=[])),responses((status=200,body=GroupConversation),ApiError))]
pub async fn add_group_member(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<ConversationId>,
    ApiJson(body): ApiJson<OpenConversation>,
) -> ApiResult<Json<GroupConversation>> {
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        let mut group = groups::readable(connection, &access, id).await?;
        super::messages::rate(&app, &access, &mut counted)?;
        sqlx::query("select id from conversations where id=$1 for update")
            .bind(id)
            .execute(&mut *connection)
            .await?;
        if !group.account_ids.contains(&body.account_id) {
            if group.account_ids.len() >= 10 {
                return Err(ApiError::invalid("a group has at most 10 people"));
            }
            if !store::may_reach(connection, &access, body.account_id).await? {
                return Err(ApiError::not_found());
            }
            group.account_ids.push(body.account_id);
            if !groups::eligible(connection, &group.account_ids).await? {
                return Err(ApiError::not_found());
            }
            groups::reserve_memberships(connection, &[body.account_id]).await?;
            sqlx::query(
                "insert into conversation_members(conversation_id,account_id) values($1,$2)",
            )
            .bind(id)
            .bind(body.account_id)
            .execute(&mut *connection)
            .await?;
        }
        changed(connection, &app, id).await
    })
    .map(Json)
}

#[utoipa::path(delete,path="/api/v1/group-conversations/{conversation_id}/membership",tag="messages",params(("conversation_id"=ConversationId,Path)),security(("session"=[])),responses((status=204),ApiError))]
pub async fn leave_group(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<ConversationId>,
) -> ApiResult<StatusCode> {
    let mut counted = false;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        groups::participant(connection, &access, id).await?;
        let room: Option<String> =
            sqlx::query_scalar("select room_name from calls where conversation_id=$1")
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?;
        if let Some(room) = room {
            app.calls.mark(&room);
        }
        super::messages::rate(&app, &access, &mut counted)?;
        sqlx::query("insert into conversation_former_members(conversation_id,account_id) values($1,$2) on conflict do nothing")
            .bind(id).bind(access.account_id).execute(&mut *connection).await?;
        sqlx::query("delete from hidden_conversations where conversation_id=$1 and account_id=$2")
            .bind(id)
            .bind(access.account_id)
            .execute(&mut *connection)
            .await?;
        sqlx::query("delete from conversation_members where conversation_id=$1 and account_id=$2")
            .bind(id)
            .bind(access.account_id)
            .execute(&mut *connection)
            .await?;
        events::publish(
            connection,
            app.now(),
            &Event::GroupConversationLeft {
                conversation_id: id,
                account_id: access.account_id,
            },
        )
        .await?;
        changed(connection, &app, id).await?;
        Ok(())
    })?;
    app.calls.poke();
    Ok(StatusCode::NO_CONTENT)
}
