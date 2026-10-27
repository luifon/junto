//! The stored permission rules of categories and channels (ADR 0304): their subjects, the
//! rules that a channel uses, and what a change of them can add.

use std::collections::BTreeMap;

use junto_authz::{Override, WorkspacePermissions};
use junto_protocol::ids::{AccountId, CategoryId, ChannelId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::structure::{PermissionRule, RuleSubject};
use sqlx::PgConnection;

use super::stored_override;
use crate::db;
use crate::error::ApiResult;

/// The rules of one category or channel, one for each subject that has one.
pub(crate) type Rules = BTreeMap<RuleSubject, Override>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Place {
    /// The channel's own rules, which it uses when it is not synced.
    Channel(ChannelId),
    Category(CategoryId),
}

pub(crate) fn rule(subject: RuleSubject, rule: Override) -> PermissionRule {
    PermissionRule {
        subject,
        allow: db::workspace_list(rule.allow()),
        deny: db::workspace_list(rule.deny()),
    }
}

pub(crate) fn listed(rules: &Rules) -> Vec<PermissionRule> {
    rules
        .iter()
        .map(|(subject, held)| rule(*subject, *held))
        .collect()
}

/// The rules that a channel uses now: the rules of its category while it is synced, its own
/// rules otherwise.
pub(crate) async fn effective(
    connection: &mut PgConnection,
    channel_id: ChannelId,
) -> ApiResult<Rules> {
    let roles: Vec<(WorkspaceRoleId, i64, i64)> = sqlx::query_as(
        "select role_id, allow, deny from effective_channel_overrides where channel_id = $1",
    )
    .bind(channel_id)
    .fetch_all(&mut *connection)
    .await?;
    let people: Vec<(AccountId, i64, i64)> = sqlx::query_as(
        "select account_id, allow, deny from effective_channel_person_overrides
         where channel_id = $1",
    )
    .bind(channel_id)
    .fetch_all(connection)
    .await?;
    collect(roles, people)
}

pub(crate) async fn of_category(
    connection: &mut PgConnection,
    category_id: CategoryId,
) -> ApiResult<Rules> {
    let roles: Vec<(WorkspaceRoleId, i64, i64)> =
        sqlx::query_as("select role_id, allow, deny from category_overrides where category_id = $1")
            .bind(category_id)
            .fetch_all(&mut *connection)
            .await?;
    let people: Vec<(AccountId, i64, i64)> = sqlx::query_as(
        "select account_id, allow, deny from category_person_overrides where category_id = $1",
    )
    .bind(category_id)
    .fetch_all(connection)
    .await?;
    collect(roles, people)
}

fn collect(
    roles: Vec<(WorkspaceRoleId, i64, i64)>,
    people: Vec<(AccountId, i64, i64)>,
) -> ApiResult<Rules> {
    let roles = roles
        .into_iter()
        .map(|(role_id, allow, deny)| (RuleSubject::Role { role_id }, allow, deny));
    let people = people
        .into_iter()
        .map(|(account_id, allow, deny)| (RuleSubject::Person { account_id }, allow, deny));
    roles
        .chain(people)
        .map(|(subject, allow, deny)| Ok((subject, stored_override(allow, deny)?)))
        .collect()
}

pub(crate) fn of_subject(rules: &Rules, subject: RuleSubject) -> Override {
    rules.get(&subject).copied().unwrap_or(Override::NONE)
}

/// The permissions that replacing the rules `before` with the rules `after` can add to a
/// subject: an allow that is new or a deny that is removed (ADR 0306).
pub(crate) fn granted(before: &Rules, after: &Rules) -> WorkspacePermissions {
    before
        .keys()
        .chain(after.keys())
        .fold(WorkspacePermissions::EMPTY, |granted, subject| {
            granted.union(of_subject(before, *subject).granted_by_change_to(of_subject(after, *subject)))
        })
}

/// Stores the rule of `subject` at `place`. A rule with two empty sets is removed.
pub(crate) async fn write(
    connection: &mut PgConnection,
    place: Place,
    workspace_id: WorkspaceId,
    subject: RuleSubject,
    rule: Override,
) -> ApiResult<()> {
    let (table, place_column, place_id) = match (place, subject) {
        (Place::Channel(id), RuleSubject::Role { .. }) => {
            ("channel_overrides", "channel_id", id.as_uuid())
        }
        (Place::Channel(id), RuleSubject::Person { .. }) => {
            ("channel_person_overrides", "channel_id", id.as_uuid())
        }
        (Place::Category(id), RuleSubject::Role { .. }) => {
            ("category_overrides", "category_id", id.as_uuid())
        }
        (Place::Category(id), RuleSubject::Person { .. }) => {
            ("category_person_overrides", "category_id", id.as_uuid())
        }
    };
    let (subject_column, subject_id) = match subject {
        RuleSubject::Role { role_id } => ("role_id", role_id.as_uuid()),
        RuleSubject::Person { account_id } => ("account_id", account_id.as_uuid()),
    };
    if rule.is_none() {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "delete from {table} where {place_column} = $1 and {subject_column} = $2"
        )))
        .bind(place_id)
        .bind(subject_id)
        .execute(connection)
        .await?;
    } else {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "insert into {table} ({place_column}, {subject_column}, workspace_id, allow, deny)
             values ($1, $2, $3, $4, $5)
             on conflict ({place_column}, {subject_column}) do update set
                 allow = excluded.allow, deny = excluded.deny"
        )))
        .bind(place_id)
        .bind(subject_id)
        .bind(workspace_id)
        .bind(db::workspace_bits(rule.allow()))
        .bind(db::workspace_bits(rule.deny()))
        .execute(connection)
        .await?;
    }
    Ok(())
}

/// Removes the own rules of channels, which then use the rules of their category.
pub(crate) async fn sync_channels(
    connection: &mut PgConnection,
    channels: &[ChannelId],
) -> ApiResult<()> {
    for statement in [
        "delete from channel_overrides where channel_id = any($1)",
        "delete from channel_person_overrides where channel_id = any($1)",
        "update channels set permissions_synced = true where id = any($1)",
    ] {
        sqlx::query(statement)
            .bind(channels)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

/// The channels of a category that use its rules, deleted channels excluded.
pub(crate) async fn synced_children(
    connection: &mut PgConnection,
    category_id: CategoryId,
) -> ApiResult<Vec<ChannelId>> {
    Ok(sqlx::query_scalar(
        "select id from channels
         where category_id = $1 and permissions_synced and deleted_at is null
         order by id",
    )
    .bind(category_id)
    .fetch_all(connection)
    .await?)
}

/// Whether a channel has a category and uses its rules.
pub(crate) async fn synced(
    connection: &mut PgConnection,
    channel_id: ChannelId,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "select category_id is not null and permissions_synced from channels where id = $1",
    )
    .bind(channel_id)
    .fetch_optional(connection)
    .await?
    .unwrap_or(false))
}

/// Refuses a new person rule at a place that holds `ceiling` person rules (ADR 0306).
pub(crate) async fn require_room_for_person(
    connection: &mut PgConnection,
    place: Place,
    ceiling: u32,
) -> ApiResult<()> {
    let (statement, id) = match place {
        Place::Channel(id) => (
            "select count(*) from (select 1 from channel_person_overrides
             where channel_id = $1 limit $2) held",
            id.as_uuid(),
        ),
        Place::Category(id) => (
            "select count(*) from (select 1 from category_person_overrides
             where category_id = $1 limit $2) held",
            id.as_uuid(),
        ),
    };
    let held: i64 = sqlx::query_scalar(statement)
        .bind(id)
        .bind(i64::from(ceiling))
        .fetch_one(connection)
        .await?;
    if held >= i64::from(ceiling) {
        return Err(crate::error::ApiError::limit_reached(
            "the place holds the largest number of person rules",
        ));
    }
    Ok(())
}
