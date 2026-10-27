//! The permissions of one person: workspace permissions of a member, server permissions of an
//! account, and what a member can use in each channel (ADR 0301, ADR 0306).

use axum::Json;
use axum::extract::State;
use junto_authz::{
    ChannelGrant, Denial, Override, Request, ServerOverride, ServerPermission, ServerPermissions,
    Target, WorkspacePermission,
};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, WorkspaceId};
use junto_protocol::roles::{
    ChangeServerPersonPermission, ServerPersonPermissions, SetServerPersonPermissions,
};
use junto_protocol::structure::{
    ChangeChannelOverride, OverrideState, PersonPermissions,
    SetChannelOverride,
};
use sqlx::PgConnection;
use time::OffsetDateTime;

use super::rules::{Change, require_member};
use crate::access::permissions::server_rule;
use crate::access::{Access, stored_override};
use crate::audit::{self, About};
use crate::db;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::events::log::Affected;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::routes::server_roles::require_active_account;
use crate::state::AppState;
use crate::{read, store, transact};

async fn workspace_rule(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    account_id: AccountId,
) -> ApiResult<Override> {
    let row: Option<(i64, i64)> = sqlx::query_as(
        "select allow, deny from workspace_person_overrides
         where workspace_id = $1 and account_id = $2",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_optional(connection)
    .await?;
    row.map_or(Ok(Override::NONE), |(allow, deny)| stored_override(allow, deny))
}

fn person_permissions(account_id: AccountId, rule: Override) -> PersonPermissions {
    PersonPermissions {
        account_id,
        allow: db::workspace_list(rule.allow()),
        deny: db::workspace_list(rule.deny()),
    }
}

/// The workspace permissions that are allowed to or denied to one member.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/members/{account_id}/permissions",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path), ("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 200, body = PersonPermissions), ApiError)
)]
pub async fn get_member_permissions(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, account_id)): ApiPath<(WorkspaceId, AccountId)>,
) -> ApiResult<Json<PersonPermissions>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::Workspace {
            scope: &scope,
            permission: WorkspacePermission::ManageRoles,
        })?;
        require_member(connection, workspace_id, account_id).await?;
        let rule = workspace_rule(connection, workspace_id, account_id).await?;
        Ok(person_permissions(account_id, rule))
    })
    .map(Json)
}

/// Sets the workspace permissions that are allowed to and denied to one member. Two empty
/// sets remove them. The caller outranks the member and holds what the change adds in the
/// workspace and in every channel where it adds it (ADR 0306).
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/members/{account_id}/permissions",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path), ("account_id" = AccountId, Path)),
    request_body = SetChannelOverride,
    security(("session" = [])),
    responses((status = 200, body = PersonPermissions), ApiError)
)]
pub async fn set_member_permissions(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, account_id)): ApiPath<(WorkspaceId, AccountId)>,
    ApiJson(body): ApiJson<SetChannelOverride>,
) -> ApiResult<Json<PersonPermissions>> {
    let change = Change::set(&body)?;
    member_permissions(app, bearer, (workspace_id, account_id), change).await
}

/// Changes one workspace permission of one member.
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}/members/{account_id}/permissions",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path), ("account_id" = AccountId, Path)),
    request_body = ChangeChannelOverride,
    security(("session" = [])),
    responses((status = 200, body = PersonPermissions), ApiError)
)]
pub async fn change_member_permission(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, account_id)): ApiPath<(WorkspaceId, AccountId)>,
    ApiJson(body): ApiJson<ChangeChannelOverride>,
) -> ApiResult<Json<PersonPermissions>> {
    let change = Change::one(&body);
    member_permissions(app, bearer, (workspace_id, account_id), change).await
}

async fn member_permissions(
    app: AppState,
    bearer: Bearer,
    member: (WorkspaceId, AccountId),
    change: Change,
) -> ApiResult<Json<PersonPermissions>> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        set_member_permissions_in(connection, &access, now, member, change).await
    })
    .map(Json)
}

pub(crate) async fn set_member_permissions_in(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    (workspace_id, account_id): (WorkspaceId, AccountId),
    change: Change,
) -> ApiResult<PersonPermissions> {
    let scope = access.workspace(connection, workspace_id).await?;
    access.require(Request::Workspace {
        scope: &scope,
        permission: WorkspacePermission::ManageRoles,
    })?;
    sqlx::query(
        "select account_id from workspace_members
         where workspace_id = $1 and account_id = $2 for update",
    )
    .bind(workspace_id)
    .bind(account_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(ApiError::not_found)?;
    access
        .require_above_workspace_account(connection, workspace_id, account_id)
        .await?;
    let previous = workspace_rule(connection, workspace_id, account_id).await?;
    let next = change.applied_to(previous)?;
    let granting = previous.granted_by_change_to(next);

    let mut added = Vec::new();
    let held = if granting.is_empty() {
        Vec::new()
    } else {
        let person = Access::standing_of(connection, account_id).await?;
        let person_scope = person.workspace(connection, workspace_id).await?;
        let theirs = person
            .workspace_channels(connection, workspace_id, &person_scope)
            .await?;
        for (record, before) in theirs {
            if record.deleted_at.is_some() {
                continue;
            }
            let mut after = before.clone();
            after.workspace.person_override = next;
            let gained = after
                .usable_permissions()
                .difference(before.usable_permissions());
            if !gained.is_empty() {
                added.push((record.channel.id, gained));
            }
        }
        access
            .workspace_channels(connection, workspace_id, &scope)
            .await?
    };
    let in_channels: Vec<ChannelGrant<'_>> = added
        .iter()
        .filter_map(|(channel_id, granting)| {
            held.iter()
                .find(|(record, _)| record.channel.id == *channel_id)
                .map(|(_, scope)| ChannelGrant {
                    scope,
                    granting: *granting,
                })
        })
        .collect();
    access.require(Request::ManageWorkspaceRoles {
        scope: &scope,
        granting,
        in_channels: &in_channels,
    })?;
    if !granting.is_empty() {
        require_active_account(connection, account_id).await?;
    }
    let result = person_permissions(account_id, next);
    if previous == next {
        return Ok(result);
    }

    if next.is_none() {
        sqlx::query(
            "delete from workspace_person_overrides where workspace_id = $1 and account_id = $2",
        )
        .bind(workspace_id)
        .bind(account_id)
        .execute(&mut *connection)
        .await?;
    } else {
        sqlx::query(
            "insert into workspace_person_overrides (workspace_id, account_id, allow, deny)
             values ($1, $2, $3, $4)
             on conflict (workspace_id, account_id) do update set
                 allow = excluded.allow, deny = excluded.deny",
        )
        .bind(workspace_id)
        .bind(account_id)
        .bind(db::workspace_bits(next.allow()))
        .bind(db::workspace_bits(next.deny()))
        .execute(&mut *connection)
        .await?;
    }
    audit::record(
        connection,
        now,
        Some(access.account_id),
        About::Workspace(workspace_id),
        &AuditEvent::WorkspacePersonPermissionsSet {
            account_id,
            allow: result.allow.clone(),
            deny: result.deny.clone(),
        },
    )
    .await?;
    let set = Event::WorkspacePersonPermissionsSet {
        workspace_id,
        account_id,
        allow: result.allow.clone(),
        deny: result.deny.clone(),
    };
    events::publish(connection, now, &set).await?;
    let affected = Affected::Account(account_id);
    events::permissions_changed(connection, now, affected, Some(workspace_id)).await?;
    Ok(result)
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

fn server_person_permissions(account_id: AccountId, rule: ServerOverride) -> ServerPersonPermissions {
    ServerPersonPermissions {
        account_id,
        allow: db::server_list(rule.allow()),
        deny: db::server_list(rule.deny()),
    }
}

async fn require_account(connection: &mut PgConnection, account_id: AccountId) -> ApiResult<()> {
    sqlx::query("select id from accounts where id = $1 for share")
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
