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

/// What the client of a request reads of group conversations (ADR 0302).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Support {
    /// The client reads group places on the routes that existed before them.
    pub groups: bool,
    /// The client reads a group without a name. A client of 0.2 drops such a group, so it
    /// receives a name made of the names of the other members, which is not stored.
    pub optional_names: bool,
}

impl Support {
    const ALL: Self = Self {
        groups: true,
        optional_names: true,
    };
}

tokio::task_local! { static SUPPORTED: Support; }

pub fn supported() -> bool {
    support().groups
}

pub fn optional_names() -> bool {
    support().optional_names
}

pub fn support() -> Support {
    SUPPORTED.try_with(|value| *value).unwrap_or(Support::ALL)
}

pub async fn with_support<T>(support: Support, future: impl std::future::Future<Output = T>) -> T {
    SUPPORTED.scope(support, future).await
}

pub async fn negotiate(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use junto_protocol::message::{GROUPS_WITH_OPTIONAL_NAMES, GROUPS_WITH_REQUIRED_NAMES};
    let value = request
        .headers()
        .get(junto_protocol::message::GROUPS_HEADER)
        .and_then(|value| value.to_str().ok());
    let support = Support {
        groups: !request.uri().path().starts_with("/api/v1/")
            || request
                .uri()
                .path()
                .starts_with("/api/v1/group-conversations")
            || matches!(
                value,
                Some(GROUPS_WITH_REQUIRED_NAMES | GROUPS_WITH_OPTIONAL_NAMES)
            ),
        optional_names: value != Some(GROUPS_WITH_REQUIRED_NAMES),
    };
    with_support(support, next.run(request)).await
}

/// The name that a client of 0.2 receives for a group without a name: the names of the
/// members other than the reader in the order of their names, at most 80 characters, which
/// that client requires (ADR 0302).
fn name_of_members(
    group: &GroupConversation,
    reader: Option<AccountId>,
    names: &HashMap<AccountId, String>,
) -> String {
    let mut others: Vec<&str> = group
        .account_ids
        .iter()
        .filter(|id| Some(**id) != reader)
        .filter_map(|id| names.get(id).map(String::as_str))
        .collect();
    others.sort_by_key(|name| name.to_lowercase());
    let joined = others.join(", ");
    if joined.is_empty() {
        return "Group conversation".to_owned();
    }
    if joined.chars().count() <= 80 {
        return joined;
    }
    let mut short: String = joined.chars().take(79).collect();
    short.push('…');
    short
}

/// Gives each group without a name the name of [`name_of_members`] when the client of the
/// request requires a name. The stored name stays empty.
pub async fn named_for_reader(
    connection: &mut PgConnection,
    reader: AccountId,
    groups: &mut [GroupConversation],
) -> ApiResult<()> {
    if optional_names() {
        return Ok(());
    }
    name_unnamed(connection, Some(reader), groups).await
}

/// Gives each group without a name the names of its members other than the reader, for a
/// text that the server writes itself, such as the title of an export.
pub async fn name_unnamed(
    connection: &mut PgConnection,
    reader: Option<AccountId>,
    groups: &mut [GroupConversation],
) -> ApiResult<()> {
    if groups.iter().all(|group| !group.name.is_empty()) {
        return Ok(());
    }
    let mut members: Vec<AccountId> = groups
        .iter()
        .filter(|group| group.name.is_empty())
        .flat_map(|group| group.account_ids.iter().copied())
        .filter(|id| Some(*id) != reader)
        .collect();
    members.sort_unstable();
    members.dedup();
    let names: HashMap<AccountId, String> =
        sqlx::query_as("select id,name from accounts where id=any($1) and name is not null")
            .bind(&members)
            .fetch_all(connection)
            .await?
            .into_iter()
            .collect();
    for group in groups.iter_mut().filter(|group| group.name.is_empty()) {
        group.name = name_of_members(group, reader, &names);
    }
    Ok(())
}

/// The name of a group conversation: at most 80 characters without controls. An empty
/// name is a group without a name, which the client shows by the names of its members
/// (ADR 0302).
pub fn name(value: &str) -> ApiResult<String> {
    let value = value.trim();
    if value.chars().count() > 80 || value.chars().any(char::is_control) {
        return Err(ApiError::invalid(
            "use a conversation name of at most 80 characters without controls",
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
