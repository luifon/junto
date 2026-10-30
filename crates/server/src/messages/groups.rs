use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    store,
};
use junto_authz::{Participation, Request};
use junto_protocol::{
    ids::{AccountId, ConversationId},
    message::GroupConversation,
};
use sqlx::PgConnection;

tokio::task_local! { static SUPPORTED: bool; }

pub fn supported() -> bool {
    SUPPORTED.try_with(|value| *value).unwrap_or(true)
}

pub async fn with_support<T>(on: bool, future: impl std::future::Future<Output = T>) -> T {
    SUPPORTED.scope(on, future).await
}

pub async fn negotiate(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let on = !request.uri().path().starts_with("/api/v1/")
        || request
            .uri()
            .path()
            .starts_with("/api/v1/group-conversations")
        || request
            .headers()
            .get(junto_protocol::message::GROUPS_HEADER)
            .is_some_and(|value| value == "1");
    with_support(on, next.run(request)).await
}

pub fn name(value: &str) -> ApiResult<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 80 || value.chars().any(char::is_control) {
        return Err(ApiError::invalid(
            "use a conversation name of 1 to 80 characters without controls",
        ));
    }
    Ok(value.to_owned())
}

pub async fn load(
    connection: &mut PgConnection,
    id: ConversationId,
) -> ApiResult<GroupConversation> {
    let name: String =
        sqlx::query_scalar("select name from conversations where id=$1 and name is not null")
            .bind(id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or_else(ApiError::not_found)?;
    let account_ids = sqlx::query_scalar(
        "select account_id from conversation_members where conversation_id=$1 order by account_id",
    )
    .bind(id)
    .fetch_all(connection)
    .await?;
    Ok(GroupConversation {
        id,
        name,
        account_ids,
    })
}

pub async fn participant(
    connection: &mut PgConnection,
    access: &Access,
    id: ConversationId,
) -> ApiResult<GroupConversation> {
    if !supported() {
        return Err(ApiError::not_found());
    }
    let group = load(connection, id).await?;
    if !access.allows(Request::DirectMessage(
        if group.account_ids.contains(&access.account_id) {
            Participation::Participant
        } else {
            Participation::Outsider
        },
    )) {
        return Err(ApiError::not_found());
    }
    Ok(group)
}

pub async fn eligible(connection: &mut PgConnection, accounts: &[AccountId]) -> ApiResult<bool> {
    let departed = store::departed_among(connection, accounts).await?;
    for (index, account) in accounts.iter().enumerate() {
        let Some(access) = Access::of_account(connection, *account).await? else {
            if departed.contains(account) {
                continue;
            }
            return Ok(false);
        };
        let others = &accounts[index + 1..];
        let contacts = store::contacts(connection, &access, others).await?;
        if others.iter().any(|other| {
            !departed.contains(other)
                && !contacts
                    .get(other)
                    .is_some_and(|contact| access.allows(Request::Contact(*contact)))
        }) {
            return Ok(false);
        }
    }
    Ok(true)
}

pub async fn readable(
    connection: &mut PgConnection,
    access: &Access,
    id: ConversationId,
) -> ApiResult<GroupConversation> {
    let group = participant(connection, access, id).await?;
    if !eligible(connection, &group.account_ids).await? {
        return Err(ApiError::not_found());
    }
    Ok(group)
}

pub async fn list(
    connection: &mut PgConnection,
    access: &Access,
) -> ApiResult<Vec<GroupConversation>> {
    if !supported() {
        return Ok(Vec::new());
    }
    let ids: Vec<ConversationId> = sqlx::query_scalar("select conversation_id from conversation_members where account_id=$1 order by conversation_id")
        .bind(access.account_id).fetch_all(&mut *connection).await?;
    let mut groups = Vec::new();
    for id in ids {
        match readable(connection, access, id).await {
            Ok(group) => groups.push(group),
            Err(error) if error.code() == junto_protocol::error::ErrorCode::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(groups)
}

pub async fn call_line(
    connection: &mut PgConnection,
    now: time::OffsetDateTime,
    conversation_id: ConversationId,
    call_id: junto_protocol::ids::CallId,
    action: &str,
    actor_id: Option<AccountId>,
    duration_seconds: Option<u64>,
) -> ApiResult<()> {
    let place = super::Place {
        id: conversation_id.as_uuid(),
        value: junto_protocol::message::MessagePlace::Group { conversation_id },
        scope: None,
        archived: false,
    };
    let position = place.next(connection).await?;
    let id = junto_protocol::ids::MessageId::generate();
    let system = junto_protocol::message::SystemMessage {
        action: action.to_owned(),
        actor_id,
        subject_id: None,
        recording_id: None,
        duration_seconds,
        name: None,
        previous_name: None,
    };
    sqlx::query("insert into messages(id,place_id,position,created_at,system,call_id) values($1,$2,$3,$4,$5,$6)")
        .bind(id).bind(place.id).bind(position).bind(now).bind(sqlx::types::Json(system)).bind(call_id).execute(&mut *connection).await?;
    super::created(connection, now, place.value, id, position as u64).await
}

pub const CALL_ARRIVAL_QUERY: &str =
    "select system->>'action' from messages where call_id=$1 and system->>'actor_id'=$2::uuid::text
     and system->>'action' in ('call_started','joined_by_invitation','person_joined','person_left','person_out')
     order by created_at desc,id desc limit 1";
