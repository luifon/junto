//! The removal of the workspace permission rules of one member (ADR 0301, ADR 0005). Each rule
//! of a member becomes a role that only that member holds, at the bottom of the role order of
//! its workspace (ADR 0309). A role only grants, so a deny of such a rule has no place in the
//! new model, and a member without a role who gets one stands above the members without a
//! role: `check-upgrade` lists every account that the change would give more than it has now,
//! in permissions or in authority over people, and the server does not apply the change while
//! the list is not empty. A banned member keeps its memberships and roles, which an unban
//! restores, so it is compared as a member.

use std::collections::{BTreeMap, HashMap};

use junto_authz::{Request, ServerPermission, WorkspacePermission, WorkspacePermissions};
use junto_protocol::ids::{AccountId, WorkspaceId, WorkspaceRoleId};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::Access;
use crate::config::Limits;
use crate::db;
use crate::error::{ApiError, ApiResult};

/// The start of the name of a role made from the rule of one member; the name of the member
/// follows. The name of these roles is kept in this one place.
pub const GENERATED_ROLE_PREFIX: &str = "Rules of ";

/// The longest name of a role (`workspace_roles.name`).
const MOST_ROLE_NAME: usize = 64;

/// The name of the role made from the rule of the member of this name, cut to the longest
/// name of a role.
pub fn generated_role_name(member: &str) -> String {
    GENERATED_ROLE_PREFIX
        .chars()
        .chain(member.chars())
        .take(MOST_ROLE_NAME)
        .collect()
}

/// One case that keeps the server from removing the rules of single members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Case {
    /// The member would hold permissions that the deny of the rule takes away now, because
    /// a role grants them and a role cannot deny.
    Gains {
        member: String,
        workspace: String,
        permissions: Vec<WorkspacePermission>,
    },
    /// The member would stand above these members, or above the roles made from their
    /// rules, who stand at or above the member now, and holds permissions that act on the
    /// people and the roles below: "manage roles" gives and takes those roles, and "manage
    /// channel" sets the rules of those people in channels and categories (ADR 0309).
    Outranks {
        member: String,
        workspace: String,
        over: Vec<String>,
        permissions: Vec<WorkspacePermission>,
    },
    /// The account left the workspace and its rule stays; a role is held only by a member,
    /// so the change removes the rule.
    Former {
        account: String,
        workspace: String,
        allow: Vec<WorkspacePermission>,
        deny: Vec<WorkspacePermission>,
    },
    /// The workspace would hold more roles than the server allows.
    WorkspaceRoles {
        workspace: String,
        roles: usize,
        most: u32,
    },
    /// The member would hold more roles of the workspace than the server allows.
    MemberRoles {
        member: String,
        workspace: String,
        roles: usize,
        most: u32,
    },
}

fn names(permissions: &[WorkspacePermission]) -> String {
    permissions
        .iter()
        .map(|permission| permission.name())
        .collect::<Vec<_>>()
        .join(", ")
}

impl Case {
    /// The line that `check-upgrade` prints for the case.
    pub fn line(&self) -> String {
        match self {
            Self::Gains {
                member,
                workspace,
                permissions,
            } => format!(
                "{member} in {workspace} would gain {}: a rule of the member denies them now and a \
                 role grants them. Take the member out of that role, or deny them in the channels.",
                names(permissions)
            ),
            Self::Outranks {
                member,
                workspace,
                over,
                permissions,
            } => format!(
                "{member} in {workspace} holds {} and would stand above {}, or above the roles made \
                 from their rules, who stand at or above {member} now. Remove the rule of \
                 {member} or theirs, or take {} from {member}.",
                names(permissions),
                over.join(", "),
                names(permissions)
            ),
            Self::Former {
                account,
                workspace,
                allow,
                deny,
            } => format!(
                "{account} left {workspace} and keeps a rule (allows: {}; denies: {}). The upgrade \
                 removes it. Remove the rule, or invite the account again and give it a role.",
                if allow.is_empty() {
                    "none".to_owned()
                } else {
                    names(allow)
                },
                if deny.is_empty() {
                    "none".to_owned()
                } else {
                    names(deny)
                },
            ),
            Self::WorkspaceRoles {
                workspace,
                roles,
                most,
            } => format!(
                "{workspace} would hold {roles} roles, and the server allows {most}. Delete roles or \
                 rules there."
            ),
            Self::MemberRoles {
                member,
                workspace,
                roles,
                most,
            } => format!(
                "{member} in {workspace} would hold {roles} roles, and the server allows {most}. \
                 Take roles from the member there."
            ),
        }
    }
}

/// True while the database holds the table of the rules of single members, that is before
/// the change was applied.
pub async fn pending(connection: &mut PgConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("select to_regclass('public.workspace_person_overrides') is not null")
        .fetch_one(connection)
        .await
}

struct Rule {
    workspace_id: WorkspaceId,
    account_id: AccountId,
    allow: WorkspacePermissions,
    deny: WorkspacePermissions,
    member: bool,
    account: String,
    workspace: String,
}

struct Role {
    id: WorkspaceRoleId,
    position: i32,
    permissions: WorkspacePermissions,
}

/// The roles of one workspace with the roles of each member, before the change.
struct Workspace {
    name: String,
    everyone: WorkspacePermissions,
    everyone_id: Option<WorkspaceRoleId>,
    roles: Vec<Role>,
    held: HashMap<AccountId, Vec<WorkspaceRoleId>>,
    names: HashMap<AccountId, String>,
}

impl Workspace {
    fn rank(&self, account_id: AccountId) -> i32 {
        self.held.get(&account_id).map_or(0, |held| {
            self.roles
                .iter()
                .filter(|role| held.contains(&role.id))
                .map(|role| role.position)
                .max()
                .unwrap_or(0)
        })
    }

    fn granted(&self, account_id: AccountId) -> WorkspacePermissions {
        let held = self.held.get(&account_id);
        self.roles
            .iter()
            .filter(|role| held.is_some_and(|held| held.contains(&role.id)))
            .fold(self.everyone, |granted, role| {
                granted.union(role.permissions)
            })
    }
}

async fn rules(connection: &mut PgConnection) -> ApiResult<Vec<Rule>> {
    let rows: Vec<(WorkspaceId, AccountId, i64, i64, bool, String, String)> = sqlx::query_as(
        "select o.workspace_id, o.account_id, o.allow, o.deny,
                exists(select 1 from workspace_members m
                       where m.workspace_id = o.workspace_id and m.account_id = o.account_id),
                a.name, w.name
         from workspace_person_overrides o
         join accounts a on a.id = o.account_id
         join workspaces w on w.id = o.workspace_id
         where w.deleted_at is null
         order by w.name, w.id, a.name, a.id",
    )
    .fetch_all(&mut *connection)
    .await?;
    rows.into_iter()
        .map(
            |(workspace_id, account_id, allow, deny, member, account, workspace)| {
                Ok(Rule {
                    workspace_id,
                    account_id,
                    allow: db::workspace_permissions(allow)?,
                    deny: db::workspace_permissions(deny)?,
                    member,
                    account,
                    workspace,
                })
            },
        )
        .collect()
}

async fn workspace(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    name: String,
) -> ApiResult<Workspace> {
    let roles: Vec<(WorkspaceRoleId, i32, i64, bool)> = sqlx::query_as(
        "select id, position, permissions, is_everyone from workspace_roles
         where workspace_id = $1 order by position, id",
    )
    .bind(workspace_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut everyone = WorkspacePermissions::EMPTY;
    let mut everyone_id = None;
    let mut kept = Vec::new();
    for (id, position, bits, is_everyone) in roles {
        let permissions = db::workspace_permissions(bits)?;
        if is_everyone {
            everyone = permissions;
            everyone_id = Some(id);
        } else {
            kept.push(Role {
                id,
                position,
                permissions,
            });
        }
    }
    let members: Vec<(AccountId, String, Option<WorkspaceRoleId>)> = sqlx::query_as(
        "select m.account_id, a.name, r.role_id from workspace_members m
         join accounts a on a.id = m.account_id
         left join member_roles r on r.workspace_id = m.workspace_id and r.account_id = m.account_id
         where m.workspace_id = $1 and a.status in ('active', 'banned')",
    )
    .bind(workspace_id)
    .fetch_all(&mut *connection)
    .await?;
    let mut held: HashMap<AccountId, Vec<WorkspaceRoleId>> = HashMap::new();
    let mut names = HashMap::new();
    for (account_id, account, role_id) in members {
        names.insert(account_id, account);
        let roles = held.entry(account_id).or_default();
        roles.extend(role_id);
    }
    Ok(Workspace {
        name,
        everyone,
        everyone_id,
        roles: kept,
        held,
        names,
    })
}

/// The permissions that act by role order (ADR 0309).
const BY_RANK: [WorkspacePermission; 2] = [
    WorkspacePermission::ManageRoles,
    WorkspacePermission::ManageChannel,
];

/// What a channel or category rule of the workspace allows the member: by the "everyone"
/// role, by a role that the member holds, or by a rule of the member there.
async fn allowed_in_places(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    account_id: AccountId,
    roles: &[WorkspaceRoleId],
) -> ApiResult<WorkspacePermissions> {
    let bits: i64 = sqlx::query_scalar(
        "select coalesce(bit_or(allow), 0)::bigint from (
             select allow from channel_overrides where workspace_id = $1 and role_id = any($2)
             union all
             select allow from category_overrides where workspace_id = $1 and role_id = any($2)
             union all
             select allow from channel_person_overrides where workspace_id = $1 and account_id = $3
             union all
             select allow from category_person_overrides where workspace_id = $1 and account_id = $3
         ) allowed",
    )
    .bind(workspace_id)
    .bind(roles)
    .bind(account_id)
    .fetch_one(&mut *connection)
    .await?;
    db::workspace_permissions(bits)
}

async fn is_administrator(connection: &mut PgConnection, account_id: AccountId) -> ApiResult<bool> {
    Ok(Access::standing_of(connection, account_id)
        .await?
        .allows(Request::Server(ServerPermission::Administrator)))
}

/// The cases that keep the server from removing the rules of single members, in the order of
/// the workspaces and of the members. Empty when the change gives nobody more than now, and
/// once the change was applied.
pub async fn cases(connection: &mut PgConnection, limits: &Limits) -> ApiResult<Vec<Case>> {
    if !pending(connection).await? {
        return Ok(Vec::new());
    }
    let rules = rules(connection).await?;
    let mut cases = Vec::new();
    let mut by_workspace: BTreeMap<(String, WorkspaceId), Vec<&Rule>> = BTreeMap::new();
    for rule in &rules {
        if rule.member {
            by_workspace
                .entry((rule.workspace.clone(), rule.workspace_id))
                .or_default()
                .push(rule);
        } else {
            cases.push(Case::Former {
                account: rule.account.clone(),
                workspace: rule.workspace.clone(),
                allow: db::workspace_list(rule.allow),
                deny: db::workspace_list(rule.deny),
            });
        }
    }
    let mut administrators: HashMap<AccountId, bool> = HashMap::new();
    for ((name, workspace_id), held_rules) in by_workspace {
        let held = workspace(connection, workspace_id, name).await?;
        let generated: Vec<&Rule> = held_rules
            .iter()
            .copied()
            .filter(|rule| !rule.allow.is_empty())
            .collect();
        let roles = held.roles.len() + generated.len();
        if roles > limits.most_roles_per_workspace as usize {
            cases.push(Case::WorkspaceRoles {
                workspace: held.name.clone(),
                roles,
                most: limits.most_roles_per_workspace,
            });
        }
        let rule_of: HashMap<AccountId, &Rule> = held_rules
            .iter()
            .map(|rule| (rule.account_id, *rule))
            .collect();
        for rule in &generated {
            let roles = held.held.get(&rule.account_id).map_or(0, Vec::len) + 1;
            if roles > limits.most_roles_per_member as usize {
                cases.push(Case::MemberRoles {
                    member: rule.account.clone(),
                    workspace: held.name.clone(),
                    roles,
                    most: limits.most_roles_per_member,
                });
            }
        }
        for rule in &held_rules {
            let gains = rule.deny.intersection(held.granted(rule.account_id));
            if gains.is_empty() {
                continue;
            }
            let administrator = match administrators.get(&rule.account_id) {
                Some(known) => *known,
                None => {
                    let known = is_administrator(connection, rule.account_id).await?;
                    administrators.insert(rule.account_id, known);
                    known
                }
            };
            // An administrator holds every permission whatever the rules say.
            if !administrator {
                cases.push(Case::Gains {
                    member: rule.account.clone(),
                    workspace: held.name.clone(),
                    permissions: db::workspace_list(gains),
                });
            }
        }
        if generated.is_empty() {
            continue;
        }
        // The new order: the generated roles at the bottom, in the order of the names of
        // their members, and the roles of now above them in their order.
        let generated_at: HashMap<AccountId, i32> = generated
            .iter()
            .enumerate()
            .map(|(index, rule)| {
                (
                    rule.account_id,
                    i32::try_from(index).unwrap_or(i32::MAX) + 1,
                )
            })
            .collect();
        let lowest = i32::try_from(generated.len()).unwrap_or(i32::MAX);
        let shifted: HashMap<WorkspaceRoleId, i32> = held
            .roles
            .iter()
            .enumerate()
            .map(|(index, role)| {
                (
                    role.id,
                    lowest.saturating_add(i32::try_from(index).unwrap_or(i32::MAX) + 1),
                )
            })
            .collect();
        let rank_after = |account_id: AccountId| {
            held.held
                .get(&account_id)
                .into_iter()
                .flatten()
                .filter_map(|role_id| shifted.get(role_id).copied())
                .chain(generated_at.get(&account_id).copied())
                .max()
                .unwrap_or(0)
        };
        let before_of = |account_id: AccountId| {
            let own = rule_of.get(&account_id);
            held.granted(account_id)
                .difference(own.map_or(WorkspacePermissions::EMPTY, |rule| rule.deny))
                .union(own.map_or(WorkspacePermissions::EMPTY, |rule| rule.allow))
        };
        let mut accounts: Vec<(&AccountId, &String)> = held.names.iter().collect();
        accounts.sort_by(|left, right| left.1.cmp(right.1).then(left.0.cmp(right.0)));
        // An administrator compares by a server role and stands above every workspace role
        // (ADR 0309): nobody gains authority over one, and one gains none that counts.
        for (account_id, _) in &accounts {
            if !administrators.contains_key(*account_id) {
                let known = is_administrator(connection, **account_id).await?;
                administrators.insert(**account_id, known);
            }
        }
        let administrator = |account_id: AccountId| administrators.get(&account_id) == Some(&true);
        for (account_id, member) in &accounts {
            let account_id = **account_id;
            let own = rule_of.get(&account_id);
            let (rank_before, rank_now) = (held.rank(account_id), rank_after(account_id));
            // The members that the member would outrank and does not outrank now.
            if administrator(account_id) {
                continue;
            }
            let mut over: Vec<String> = accounts
                .iter()
                .filter(|(other, _)| **other != account_id && !administrator(**other))
                .filter(|(other, _)| {
                    rank_before <= held.rank(**other) && rank_now > rank_after(**other)
                })
                .map(|(_, name)| (*name).clone())
                .collect();
            // The roles made from the rules of others that the member would stand above,
            // whose rule the member could not change now (ADR 0306).
            let managed_before = |other: AccountId| {
                before_of(account_id).contains(WorkspacePermission::ManageRoles)
                    && rank_before > held.rank(other)
            };
            for rule in &generated {
                let below = generated_at
                    .get(&rule.account_id)
                    .is_some_and(|position| *position < rank_now);
                if rule.account_id != account_id
                    && below
                    && !managed_before(rule.account_id)
                    && !over.contains(&rule.account)
                {
                    over.push(rule.account.clone());
                }
            }
            if over.is_empty() {
                continue;
            }
            let mut roles: Vec<WorkspaceRoleId> =
                held.held.get(&account_id).cloned().unwrap_or_default();
            roles.extend(held.everyone_id);
            let after = held
                .granted(account_id)
                .union(own.map_or(WorkspacePermissions::EMPTY, |rule| rule.allow))
                .union(allowed_in_places(connection, workspace_id, account_id, &roles).await?);
            let acting: Vec<WorkspacePermission> = BY_RANK
                .into_iter()
                .filter(|permission| after.contains(*permission))
                .collect();
            if !acting.is_empty() {
                cases.push(Case::Outranks {
                    member: (*member).clone(),
                    workspace: held.name.clone(),
                    over,
                    permissions: acting,
                });
            }
        }
    }
    Ok(cases)
}

/// Makes the roles of the rules of current members and removes every rule of a single
/// member. The caller applies it only when [`cases`] is empty, in one transaction with
/// nothing else. Returns the number of roles made.
pub async fn convert(connection: &mut PgConnection, now: OffsetDateTime) -> ApiResult<usize> {
    if !pending(connection).await? {
        return Ok(0);
    }
    let rules = rules(connection).await?;
    let mut by_workspace: BTreeMap<WorkspaceId, Vec<&Rule>> = BTreeMap::new();
    for rule in rules
        .iter()
        .filter(|rule| rule.member && !rule.allow.is_empty())
    {
        by_workspace
            .entry(rule.workspace_id)
            .or_default()
            .push(rule);
    }
    let mut made = 0;
    for (workspace_id, generated) in by_workspace {
        let count = i32::try_from(generated.len())
            .map_err(|_| ApiError::internal(&"too many rules in one workspace"))?;
        // The roles of now move above the new ones, at consecutive positions in their order,
        // which also keeps every position far below the integer limit (ADR 0309).
        sqlx::query(
            "update workspace_roles r set position = ordered.place + $2
             from (select id, row_number() over (order by position, id)::int as place
                   from workspace_roles where workspace_id = $1 and not is_everyone) ordered
             where r.id = ordered.id",
        )
        .bind(workspace_id)
        .bind(count)
        .execute(&mut *connection)
        .await?;
        for (index, rule) in generated.iter().enumerate() {
            let role_id = WorkspaceRoleId::generate();
            let position = i32::try_from(index).unwrap_or(i32::MAX) + 1;
            sqlx::query(
                "insert into workspace_roles (id, workspace_id, name, permissions, is_everyone,
                                              created_at, position)
                 values ($1, $2, $3, $4, false, $5, $6)",
            )
            .bind(role_id)
            .bind(workspace_id)
            .bind(generated_role_name(&rule.account))
            .bind(db::workspace_bits(rule.allow))
            .bind(now)
            .bind(position)
            .execute(&mut *connection)
            .await?;
            sqlx::query(
                "insert into member_roles (workspace_id, account_id, role_id) values ($1, $2, $3)",
            )
            .bind(workspace_id)
            .bind(rule.account_id)
            .bind(role_id)
            .execute(&mut *connection)
            .await?;
            made += 1;
        }
    }
    sqlx::query("delete from workspace_person_overrides")
        .execute(&mut *connection)
        .await?;
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_generated_role_is_named_by_its_member_within_the_longest_name() {
        assert_eq!(generated_role_name("Bruno"), "Rules of Bruno");
        let long = "n".repeat(64);
        let name = generated_role_name(&long);
        assert_eq!(name.chars().count(), 64);
        assert!(name.starts_with(GENERATED_ROLE_PREFIX));
    }
}
