use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    store,
};
use junto_authz::{Actor, Contact, Participation, Request, authorize};
use junto_protocol::{
    ids::{AccountId, ConversationId, WorkspaceId},
    message::GroupConversation,
};
use sqlx::PgConnection;
use std::collections::{HashMap, HashSet};

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

pub const MOST_GROUPS: i64 = 256;

tokio::task_local! {
    static READABLE: (AccountId, Vec<GroupConversation>);
}

/// Reuses standing only inside the caller's serializable snapshot transaction.
pub(crate) async fn with_readable<T>(
    account: AccountId,
    groups: Vec<GroupConversation>,
    future: impl std::future::Future<Output = T>,
) -> T {
    READABLE.scope((account, groups), future).await
}

fn cached(account: AccountId) -> Option<Vec<GroupConversation>> {
    READABLE
        .try_with(|(id, groups)| (*id == account).then(|| groups.clone()))
        .ok()
        .flatten()
}

type StandingRow = (AccountId, bool, Vec<i64>, i64, i64, Vec<WorkspaceId>);

struct Standing {
    actor: Actor,
    workspaces: HashSet<WorkspaceId>,
}

async fn standing(
    connection: &mut PgConnection,
    accounts: &[AccountId],
) -> ApiResult<HashMap<AccountId, Standing>> {
    if accounts.is_empty() {
        return Ok(HashMap::new());
    }
    let query = format!(
        "select contact.*,array(select workspace_id from workspace_members where account_id=contact.id) from ({}) contact(id,owner,roles,allow,deny)",
        store::CONTACT_STANDING
    );
    let rows: Vec<StandingRow> = sqlx::query_as(sqlx::AssertSqlSafe(query))
        .bind(accounts)
        .fetch_all(connection)
        .await?;
    rows.into_iter()
        .map(|(id, owner, roles, allow, deny, workspaces)| {
            Ok((
                id,
                Standing {
                    actor: crate::access::visibility::make_actor(owner, roles, (allow, deny))?,
                    workspaces: workspaces.into_iter().collect(),
                },
            ))
        })
        .collect()
}

fn mutually_reachable(accounts: &[AccountId], standing: &HashMap<AccountId, Standing>) -> bool {
    let active: Vec<_> = accounts.iter().filter_map(|id| standing.get(id)).collect();
    active.iter().enumerate().all(|(index, left)| {
        active[index + 1..].iter().all(|right| {
            let shared = !left.workspaces.is_disjoint(&right.workspaces);
            let contact = |other: &Standing| {
                if shared {
                    Contact::SharedWorkspace
                } else if other.actor.is_administrator() {
                    Contact::Administrator
                } else {
                    Contact::Unrelated
                }
            };
            authorize(&left.actor, &Request::Contact(contact(right))).is_allowed()
                && authorize(&right.actor, &Request::Contact(contact(left))).is_allowed()
        })
    })
}

pub async fn eligible(connection: &mut PgConnection, accounts: &[AccountId]) -> ApiResult<bool> {
    Ok(mutually_reachable(
        accounts,
        &standing(connection, accounts).await?,
    ))
}

pub(crate) async fn for_accounts(
    connection: &mut PgConnection,
    accounts: &[AccountId],
    named: Option<&[uuid::Uuid]>,
) -> ApiResult<HashMap<AccountId, Vec<GroupConversation>>> {
    let mut result = HashMap::new();
    if !supported() || accounts.is_empty() || named.is_some_and(<[uuid::Uuid]>::is_empty) {
        return Ok(result);
    }
    let mut missing = Vec::new();
    for account in accounts {
        if let Some(groups) = cached(*account) {
            result.insert(
                *account,
                groups
                    .into_iter()
                    .filter(|group| named.is_none_or(|ids| ids.contains(&group.id.as_uuid())))
                    .collect(),
            );
        } else {
            missing.push(*account);
        }
    }
    if missing.is_empty() {
        return Ok(result);
    }
    let rows:Vec<(ConversationId,String,Vec<AccountId>)>=sqlx::query_as(
        "select c.id,c.name,array(select account_id from conversation_members where conversation_id=c.id order by account_id) from conversations c where c.name is not null and c.id in (select conversation_id from conversation_members where account_id=any($1) and ($2::uuid[] is null or conversation_id=any($2))) order by c.id"
    ).bind(&missing).bind(named).fetch_all(&mut *connection).await?;
    let mut members: Vec<_> = rows
        .iter()
        .flat_map(|(_, _, ids)| ids.iter().copied())
        .collect();
    members.sort_unstable();
    members.dedup();
    let standing = standing(connection, &members).await?;
    for (id, name, account_ids) in rows {
        if !mutually_reachable(&account_ids, &standing) {
            continue;
        }
        let group = GroupConversation {
            id,
            name,
            account_ids,
        };
        for account in &missing {
            if let Some(reader) = standing.get(account)
                && authorize(
                    &reader.actor,
                    &Request::DirectMessage(if group.account_ids.contains(account) {
                        Participation::Participant
                    } else {
                        Participation::Outsider
                    }),
                )
                .is_allowed()
            {
                result
                    .entry(*account)
                    .or_insert_with(Vec::new)
                    .push(group.clone());
            }
        }
    }
    Ok(result)
}

pub async fn readable(
    connection: &mut PgConnection,
    access: &Access,
    id: ConversationId,
) -> ApiResult<GroupConversation> {
    if let Ok(Some(group)) = READABLE.try_with(|(account, groups)| {
        (*account == access.account_id).then(|| groups.iter().find(|group| group.id == id).cloned())
    }) {
        return group.ok_or_else(ApiError::not_found);
    }
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
    if !access.allows(Request::DirectMessage(Participation::Participant)) {
        return Ok(Vec::new());
    }
    Ok(for_accounts(connection, &[access.account_id], None)
        .await?
        .remove(&access.account_id)
        .unwrap_or_default())
}

pub(crate) async fn reserve_memberships(
    connection: &mut PgConnection,
    accounts: &[AccountId],
) -> ApiResult<()> {
    sqlx::query("select id from accounts where id=any($1) order by id for update")
        .bind(accounts)
        .execute(&mut *connection)
        .await?;
    let full:bool=sqlx::query_scalar("select exists(select account_id from conversation_members where account_id=any($1) group by account_id having count(*) >= $2)").bind(accounts).bind(MOST_GROUPS).fetch_one(connection).await?;
    if full {
        return Err(ApiError::invalid(
            "an account belongs to at most 256 groups",
        ));
    }
    Ok(())
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
