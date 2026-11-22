//! The permissions of one person: workspace permissions of a member, server permissions of an
//! account, and what a member can use in each channel (ADR 0301, ADR 0306).

use axum::Json;
use axum::extract::State;
use junto_authz::{
    Denial, Request, ServerOverride, ServerPermission, ServerPermissions, Target,
    WorkspacePermission,
};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, WorkspaceId};
use junto_protocol::roles::{
    ChangeServerPersonPermission, ServerPersonPermissions, SetServerPersonPermissions,
};
use junto_protocol::structure::{ChannelPermissions, OverrideState, PersonPermissions};
use sqlx::PgConnection;
use time::OffsetDateTime;

use super::rules::require_member;
use crate::access::Access;
use crate::access::permissions::server_rule;
use crate::audit::{self, About};
use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::events::log::Affected;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::routes::server_roles::require_active_account;
use crate::state::AppState;
use crate::{read, store, transact};

/// The path of the retired routes of the workspace permissions of one member.
pub const MEMBER_RULES_PATH: &str =
    "/api/v1/workspaces/{workspace_id}/members/{account_id}/permissions";

/// Why the routes of the workspace permissions of one member refuse every request
/// (ADR 0201, ADR 0306).
pub const MEMBER_RULES_RETIRED: &str = "Workspace permission rules for one member were removed in \
     Junto 0.3.0: each one became a role of that member. Use a role, or a rule of a channel or a \
     category.";

/// Retired: a workspace has no permission rule for one member any more. Every request is
/// refused with `retired`.
pub async fn get_member_permissions(
    _member: ApiPath<(WorkspaceId, AccountId)>,
) -> ApiResult<Json<PersonPermissions>> {
    Err(ApiError::retired(MEMBER_RULES_RETIRED))
}

/// Retired: a workspace has no permission rule for one member any more. Every request is
/// refused with `retired`.
pub async fn set_member_permissions(
    _member: ApiPath<(WorkspaceId, AccountId)>,
) -> ApiResult<Json<PersonPermissions>> {
    Err(ApiError::retired(MEMBER_RULES_RETIRED))
}

/// Retired: a workspace has no permission rule for one member any more. Every request is
/// refused with `retired`.
pub async fn change_member_permission(
    _member: ApiPath<(WorkspaceId, AccountId)>,
) -> ApiResult<Json<PersonPermissions>> {
    Err(ApiError::retired(MEMBER_RULES_RETIRED))
}

/// What a member can use in each channel of the workspace that the caller can see: the
/// permissions for which the permission function allows a request of the member there.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/members/{account_id}/access",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path), ("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 200, body = [ChannelPermissions]), ApiError)
)]
pub async fn get_member_access(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, account_id)): ApiPath<(WorkspaceId, AccountId)>,
) -> ApiResult<Json<Vec<ChannelPermissions>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::Workspace {
            scope: &scope,
            permission: WorkspacePermission::ManageRoles,
        })?;
        require_member(connection, workspace_id, account_id).await?;
        let seen: std::collections::HashSet<_> = access
            .workspace_channels(connection, workspace_id, &scope)
            .await?
            .into_iter()
            .filter(|(record, channel_scope)| {
                record.deleted_at.is_none()
                    && access.allows(Request::Channel {
                        scope: channel_scope,
                        permission: WorkspacePermission::ViewChannel,
                    })
            })
            .map(|(record, _)| record.channel.id)
            .collect();
        let person = Access::standing_of(connection, account_id).await?;
        let person_scope = person.workspace(connection, workspace_id).await?;
        Ok(person
            .workspace_channels(connection, workspace_id, &person_scope)
            .await?
            .into_iter()
            .filter(|(record, _)| seen.contains(&record.channel.id))
            .map(|(record, channel_scope)| ChannelPermissions {
                channel_id: record.channel.id,
                permissions: person.channel_permissions(&channel_scope),
            })
            .collect())
    })
    .map(Json)
}

async fn server_person_rule(
    connection: &mut PgConnection,
    account_id: AccountId,
) -> ApiResult<ServerOverride> {
    let row: Option<(i64, i64)> =
        sqlx::query_as("select allow, deny from server_person_overrides where account_id = $1")
            .bind(account_id)
            .fetch_optional(connection)
            .await?;
    row.map_or(Ok(ServerOverride::NONE), |(allow, deny)| {
        server_rule(allow, deny)
    })
}

fn server_person_permissions(
    account_id: AccountId,
    rule: ServerOverride,
) -> ServerPersonPermissions {
    ServerPersonPermissions {
        account_id,
        allow: db::server_list(rule.allow()),
        deny: db::server_list(rule.deny()),
    }
}

async fn require_account(connection: &mut PgConnection, account_id: AccountId) -> ApiResult<()> {
    sqlx::query("select id from accounts where id = $1")
        .bind(account_id)
        .fetch_optional(connection)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok(())
}

/// The server permissions that are allowed to or denied to one account, after its server
/// roles.
#[utoipa::path(
    get,
    path = "/api/v1/accounts/{account_id}/server-permissions",
    tag = "server roles",
    params(("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 200, body = ServerPersonPermissions), ApiError)
)]
pub async fn get_server_permissions(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
) -> ApiResult<Json<ServerPersonPermissions>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        require_account(connection, account_id).await?;
        let rule = server_person_rule(connection, account_id).await?;
        Ok(server_person_permissions(account_id, rule))
    })
    .map(Json)
}

/// Sets the server permissions that are allowed to and denied to one account. Two empty sets
/// remove them. The caller administers the server and outranks the account; only the server
/// owner denies the administrator permission, and the owner's account takes no rule.
#[utoipa::path(
    put,
    path = "/api/v1/accounts/{account_id}/server-permissions",
    tag = "server roles",
    params(("account_id" = AccountId, Path)),
    request_body = SetServerPersonPermissions,
    security(("session" = [])),
    responses((status = 200, body = ServerPersonPermissions), ApiError)
)]
pub async fn set_server_permissions(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
    ApiJson(body): ApiJson<SetServerPersonPermissions>,
) -> ApiResult<Json<ServerPersonPermissions>> {
    let allow: ServerPermissions = body.allow.iter().copied().collect();
    let deny: ServerPermissions = body.deny.iter().copied().collect();
    let next = ServerOverride::new(allow, deny)
        .map_err(|_| ApiError::invalid("a permission is in both allow and deny"))?;
    server_permissions(app, bearer, account_id, ServerChange::Set(next)).await
}

/// Changes one server permission of one account.
#[utoipa::path(
    patch,
    path = "/api/v1/accounts/{account_id}/server-permissions",
    tag = "server roles",
    params(("account_id" = AccountId, Path)),
    request_body = ChangeServerPersonPermission,
    security(("session" = [])),
    responses((status = 200, body = ServerPersonPermissions), ApiError)
)]
pub async fn change_server_permission(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(account_id): ApiPath<AccountId>,
    ApiJson(body): ApiJson<ChangeServerPersonPermission>,
) -> ApiResult<Json<ServerPersonPermissions>> {
    let change = ServerChange::One(body.permission, body.state);
    server_permissions(app, bearer, account_id, change).await
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum ServerChange {
    Set(ServerOverride),
    One(ServerPermission, OverrideState),
}

impl ServerChange {
    fn applied_to(self, previous: ServerOverride) -> ApiResult<ServerOverride> {
        match self {
            Self::Set(next) => Ok(next),
            Self::One(permission, state) => {
                let changed: ServerPermissions = [permission].into_iter().collect();
                let mut allow = previous.allow().difference(changed);
                let mut deny = previous.deny().difference(changed);
                match state {
                    OverrideState::Allow => allow = allow.union(changed),
                    OverrideState::Deny => deny = deny.union(changed),
                    OverrideState::Inherit => {}
                }
                ServerOverride::new(allow, deny).map_err(|error| ApiError::internal(&error))
            }
        }
    }
}

async fn server_permissions(
    app: AppState,
    bearer: Bearer,
    account_id: AccountId,
    change: ServerChange,
) -> ApiResult<Json<ServerPersonPermissions>> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        set_server_permissions_in(connection, &access, now, account_id, change).await
    })
    .map(Json)
}

pub(crate) async fn set_server_permissions_in(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    account_id: AccountId,
    change: ServerChange,
) -> ApiResult<ServerPersonPermissions> {
    access.require(Request::Server(ServerPermission::Administrator))?;
    require_account(connection, account_id).await?;
    if access.target(account_id) == Target::Owner {
        return Err(ApiError::invalid(
            "the account of the server owner takes no server permission rule",
        ));
    }
    access.require_above_account(connection, account_id).await?;
    let previous = server_person_rule(connection, account_id).await?;
    let next = change.applied_to(previous)?;
    let administrator = ServerPermission::Administrator;
    if next.deny().contains(administrator)
        && !previous.deny().contains(administrator)
        && !access.is_owner()
    {
        return Err(Denial::OwnerOnly.into());
    }
    let granting = previous.granted_by_change_to(next);
    access.require(Request::ManageServerRoles { granting })?;
    if !granting.is_empty() {
        require_active_account(connection, account_id).await?;
    }
    let result = server_person_permissions(account_id, next);
    if previous == next {
        return Ok(result);
    }

    if next.is_none() {
        sqlx::query("delete from server_person_overrides where account_id = $1")
            .bind(account_id)
            .execute(&mut *connection)
            .await?;
    } else {
        sqlx::query(
            "insert into server_person_overrides (account_id, allow, deny) values ($1, $2, $3)
             on conflict (account_id) do update set allow = excluded.allow, deny = excluded.deny",
        )
        .bind(account_id)
        .bind(db::server_bits(next.allow()))
        .bind(db::server_bits(next.deny()))
        .execute(&mut *connection)
        .await?;
    }
    audit::record(
        connection,
        now,
        Some(access.account_id),
        About::Server,
        &AuditEvent::ServerPersonPermissionsSet {
            account_id,
            allow: result.allow.clone(),
            deny: result.deny.clone(),
        },
    )
    .await?;
    let set = Event::ServerPersonPermissionsSet {
        account_id,
        allow: result.allow.clone(),
        deny: result.deny.clone(),
    };
    events::publish(connection, now, &set).await?;
    events::permissions_changed(connection, now, Affected::Account(account_id), None).await?;
    let touched = |rule: ServerOverride| {
        rule.allow().contains(administrator) || rule.deny().contains(administrator)
    };
    if touched(previous) || touched(next) {
        store::contact_changed(connection, now, account_id).await?;
    }
    Ok(result)
}
