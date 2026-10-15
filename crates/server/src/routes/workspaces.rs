use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::{ChannelContent, Request, ServerPermission};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, WorkspaceId, WorkspaceRoleId};
use junto_protocol::roles::WorkspaceRole;
use junto_protocol::structure::{CreateWorkspace, UpdateWorkspace, Workspace, WorkspaceMember};
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::Access;
use crate::audit::{self, About};
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::extract::{ApiJson, ApiPath, Bearer};
use crate::routes::server_roles::require_active_account;
use crate::state::AppState;
use crate::{bootstrap, store, validate};
use crate::{read, transact};

/// The workspaces that the caller can see.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces",
    tag = "workspaces",
    security(("session" = [])),
    responses((status = 200, body = [Workspace]), ApiError)
)]
pub async fn list_workspaces(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<Vec<Workspace>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let default_workspace = store::default_workspace(connection).await?;
        let rows: Vec<(WorkspaceId, String)> =
            sqlx::query_as("select id, name from workspaces where deleted_at is null order by id")
                .fetch_all(&mut *connection)
                .await?;
        let mut workspaces = Vec::new();
        for (id, name) in rows {
            let scope = access.workspace(connection, id).await?;
            if access.allows(Request::ViewWorkspace(&scope)) {
                workspaces.push(Workspace {
                    id,
                    name,
                    is_default: id == default_workspace,
                });
            }
        }
        Ok(workspaces)
    })
    .map(Json)
}

/// Creates a workspace. The caller becomes its first member.
#[utoipa::path(
    post,
    path = "/api/v1/workspaces",
    tag = "workspaces",
    request_body = CreateWorkspace,
    security(("session" = [])),
    responses((status = 201, body = Workspace), ApiError)
)]
pub async fn create_workspace(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<CreateWorkspace>,
) -> ApiResult<(StatusCode, Json<Workspace>)> {
    let name = validate::name("name", &body.name)?;
    let now = app.now();
    let (workspace, _) = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        create(connection, &access, now, &name).await
    })?;
    Ok((StatusCode::CREATED, Json(workspace)))
}

/// Creates a workspace with its "everyone" role. The caller becomes its first member.
pub(crate) async fn create(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    name: &str,
) -> ApiResult<(Workspace, WorkspaceRole)> {
    access.require(Request::Server(ServerPermission::CreateWorkspace))?;
    let (workspace_id, everyone) = bootstrap::create_workspace(connection, name, now).await?;
    let workspace = Workspace {
        id: workspace_id,
        name: name.to_owned(),
        is_default: false,
    };
    let created = Event::WorkspaceCreated {
        workspace: workspace.clone(),
    };
    events::publish(connection, now, &created).await?;
    let role = Event::WorkspaceRoleCreated {
        role: everyone.clone(),
    };
    events::publish(connection, now, &role).await?;
    events::permissions_changed_in_new_workspace(connection, now, workspace_id).await?;
    store::add_members(connection, now, access.account_id, &[workspace_id]).await?;
    audit::record(
        connection,
        now,
        Some(access.account_id),
        About::Server,
        &AuditEvent::WorkspaceCreated {
            workspace_id,
            name: name.to_owned(),
        },
    )
    .await?;
    Ok((workspace, everyone))
}

/// Renames a workspace.
#[utoipa::path(
    patch,
    path = "/api/v1/workspaces/{workspace_id}",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path)),
    request_body = UpdateWorkspace,
    security(("session" = [])),
    responses((status = 200, body = Workspace), ApiError)
)]
pub async fn update_workspace(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiJson(body): ApiJson<UpdateWorkspace>,
) -> ApiResult<Json<Workspace>> {
    let name = validate::name("name", &body.name)?;
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        rename(connection, &access, now, workspace_id, &name).await
    })
    .map(Json)
}

pub(crate) async fn rename(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    workspace_id: WorkspaceId,
    name: &str,
) -> ApiResult<Workspace> {
    access.require(Request::Server(ServerPermission::Administrator))?;
    let renamed =
        sqlx::query("update workspaces set name = $2 where id = $1 and deleted_at is null")
            .bind(workspace_id)
            .bind(name)
            .execute(&mut *connection)
            .await?;
    if renamed.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    let default_workspace = store::default_workspace(connection).await?;
    audit::record(
        connection,
        now,
        Some(access.account_id),
        About::Server,
        &AuditEvent::WorkspaceRenamed {
            workspace_id,
            name: name.to_owned(),
        },
    )
    .await?;
    let workspace = Workspace {
        id: workspace_id,
        name: name.to_owned(),
        is_default: workspace_id == default_workspace,
    };
    let updated = Event::WorkspaceUpdated {
        workspace: workspace.clone(),
    };
    events::publish(connection, now, &updated).await?;
    Ok(workspace)
}

/// Deletes a workspace with its members, roles, categories and channels. The caller holds
/// the administrator permission; only the server owner deletes a workspace of which a
/// channel contains permanent messages. The default workspace is not deleted.
#[utoipa::path(
    delete,
    path = "/api/v1/workspaces/{workspace_id}",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn delete_workspace(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        delete(connection, &access, now, workspace_id).await
    })?;
    app.purges.wake();
    Ok(StatusCode::NO_CONTENT)
}

/// The statements that remove what a workspace holds besides its channels, in their
/// order. Each names the workspace as `$1`. The roles go before the members, and with
/// them every assignment and every override.
const EMPTY_WORKSPACE: [&str; 11] = [
    "delete from structure_identifiers
     where (kind = 'workspace_role'
            and object_id in (select id from workspace_roles where workspace_id = $1))
        or (kind = 'category'
            and object_id in (select id from categories where workspace_id = $1))",
    "update channels set category_id = null
     where workspace_id = $1 and category_id is not null",
    "delete from categories where workspace_id = $1",
    "delete from workspace_roles where workspace_id = $1",
    "delete from workspace_members where workspace_id = $1",
    "delete from glossary_terms where workspace_id = $1",
    "delete from workspace_notification_defaults where workspace_id = $1",
    "delete from workspace_file_retention where workspace_id = $1",
    "delete from workspace_recording_retention where workspace_id = $1",
    "delete from workspace_archive_periods where workspace_id = $1",
    "delete from thread_forks
     where channel_id in (select id from channels where workspace_id = $1)",
];

/// Deletes a workspace. The request removes the structure: the members, the roles with
/// their assignments and overrides, the categories and the settings, and it marks every
/// channel as deleted. The row of the workspace stays, because audit entries refer to it
/// (ADR 0305). The content of the channels is removed afterwards, in bounded parts
/// (ADR 0017).
pub(crate) async fn delete(
    connection: &mut PgConnection,
    access: &Access,
    now: OffsetDateTime,
    workspace_id: WorkspaceId,
) -> ApiResult<()> {
    access.require(Request::Server(ServerPermission::Administrator))?;
    let name: Option<String> = sqlx::query_scalar(
        "select name from workspaces where id = $1 and deleted_at is null for update",
    )
    .bind(workspace_id)
    .fetch_optional(&mut *connection)
    .await?;
    let name = name.ok_or_else(ApiError::not_found)?;
    if workspace_id == store::default_workspace(connection).await? {
        return Err(ApiError::conflict(
            "the default workspace is not deleted: an invite that names no workspace grants it",
        ));
    }
    let permanent: bool = sqlx::query_scalar(
        "select exists (
             select 1 from channels
             where workspace_id = $1 and deleted_at is null and permanent_message_count > 0)",
    )
    .bind(workspace_id)
    .fetch_one(&mut *connection)
    .await?;
    access.require(Request::DeleteWorkspace {
        content: if permanent {
            ChannelContent::PermanentMessages
        } else {
            ChannelContent::NoPermanentMessages
        },
    })?;

    events::workspace_deleted(connection, now, workspace_id).await?;
    for statement in EMPTY_WORKSPACE {
        sqlx::query(statement)
            .bind(workspace_id)
            .execute(&mut *connection)
            .await?;
    }
    sqlx::query(
        "with deleted as (
             update channels set deleted_at = $2
             where workspace_id = $1 and deleted_at is null
             returning id)
         insert into purges (id, kind, requested_at)
         select id, 'channel', $2 from deleted
         on conflict (id) do nothing",
    )
    .bind(workspace_id)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    let subject = crate::purge::Subject::Workspace(workspace_id);
    crate::purge::queue(connection, now, subject).await?;
    sqlx::query("update workspaces set deleted_at = $2 where id = $1")
        .bind(workspace_id)
        .bind(now)
        .execute(&mut *connection)
        .await?;
    audit::record(
        connection,
        now,
        Some(access.account_id),
        About::Server,
        &AuditEvent::WorkspaceDeleted { workspace_id, name },
    )
    .await
}

/// The members of a workspace (`$1`) with their roles and avatars, in the order of their
/// accounts. The name and the avatar of each member are read by its account, once for each
/// member: a join could read the accounts and avatars of the whole server (ADR 0119).
pub const MEMBER_LIST: &str = "select m.account_id,
                    (select a.name from accounts a where a.id = m.account_id) as name,
                    array(select r.role_id from member_roles r
                          where r.workspace_id = m.workspace_id and r.account_id = m.account_id
                          order by r.role_id) as role_ids,
                    (select jsonb_build_object(
                        'hash', encode(av.hash, 'hex'), 'media_type', av.media_type,
                        'bytes', octet_length(av.content), 'width', av.width, 'height', av.height)
                     from account_avatars av where av.account_id = m.account_id) as avatar
             from workspace_members m
             where m.workspace_id = $1
             order by m.account_id";

/// The members of a workspace with their roles.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/members",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path)),
    security(("session" = [])),
    responses((status = 200, body = [WorkspaceMember]), ApiError)
)]
pub async fn list_members(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
) -> ApiResult<Json<Vec<WorkspaceMember>>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ViewWorkspace(&scope))?;
        let rows: Vec<MemberRow> = sqlx::query_as(MEMBER_LIST)
            .bind(workspace_id)
            .fetch_all(&mut *connection)
            .await?;
        Ok(rows
            .into_iter()
            .map(|row| WorkspaceMember {
                avatar: row.avatar.map(|value| value.0),
                account_id: row.account_id,
                name: row.name,
                role_ids: row.role_ids,
            })
            .collect())
    })
    .map(Json)
}

#[derive(sqlx::FromRow)]
struct MemberRow {
    account_id: AccountId,
    name: String,
    role_ids: Vec<WorkspaceRoleId>,
    avatar: Option<sqlx::types::Json<junto_protocol::account::Avatar>>,
}

/// Adds an existing account to a workspace. Only a server administrator can do this.
#[utoipa::path(
    put,
    path = "/api/v1/workspaces/{workspace_id}/members/{account_id}",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path), ("account_id" = AccountId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn add_member(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, account_id)): ApiPath<(WorkspaceId, AccountId)>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        store::require_workspaces_exist(connection, &[workspace_id]).await?;
        require_active_account(connection, account_id).await?;
        let added = sqlx::query(
            "insert into workspace_members (workspace_id, account_id, joined_at)
             values ($1, $2, $3)
             on conflict do nothing",
        )
        .bind(workspace_id)
        .bind(account_id)
        .bind(now)
        .execute(&mut *connection)
        .await?;
        if added.rows_affected() > 0 {
            audit::record(
                connection,
                now,
                Some(access.account_id),
                About::Workspace(workspace_id),
                &AuditEvent::WorkspaceMemberAdded { account_id },
            )
            .await?;
            store::announce_members(connection, now, account_id, &[workspace_id]).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Removes one workspace membership and its roles. Server access and other memberships stay.
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/members/{account_id}/removal",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path), ("account_id" = AccountId, Path)),
    request_body = Option<junto_protocol::account::ModerationReason>,
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn remove_member(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((workspace_id, account_id)): ApiPath<(WorkspaceId, AccountId)>,
    body: axum::body::Bytes,
) -> ApiResult<StatusCode> {
    let (reason, recipient_message) = super::moderation::moderation_reason(&body)?;
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ViewWorkspace(&scope))?;
        access.require(Request::Server(ServerPermission::Administrator))?;
        access.require(Request::Moderate {
            target: access.target(account_id),
        })?;
        access
            .require_above_workspace_account(connection, workspace_id, account_id)
            .await?;
        let removed =
            sqlx::query("delete from workspace_members where workspace_id=$1 and account_id=$2")
                .bind(workspace_id)
                .bind(account_id)
                .execute(&mut *connection)
                .await?;
        if removed.rows_affected() == 0 {
            return Err(ApiError::not_found());
        }
        super::moderation_notices::record(
            connection,
            now,
            account_id,
            Some(workspace_id),
            junto_protocol::account::ModerationNoticeKind::WorkspaceRemoved,
            recipient_message.as_deref(),
        )
        .await?;
        super::invites::withdraw_workspace(
            connection,
            now,
            access.account_id,
            account_id,
            workspace_id,
        )
        .await?;
        if let Some(member) = crate::calls::store::member_of(connection, account_id, true).await?
            && let Some(call) = crate::calls::store::call(
                connection,
                crate::calls::store::By::Id(member.call_id),
                true,
            )
            .await?
            && matches!(call.place, junto_protocol::call::CallPlace::Channel { workspace_id: id, .. } if id == workspace_id)
        {
            crate::calls::entry::leave(
                connection,
                now,
                account_id,
                None,
                junto_protocol::call::LeaveReason::AccessEnded,
            )
            .await?;
            app.calls.mark(call.room.as_str());
        }
        store::contact_changed(connection, now, account_id).await?;
        events::permissions_changed(
            connection,
            now,
            events::log::Affected::Account(account_id),
            Some(workspace_id),
        )
        .await?;
        events::publish(
            connection,
            now,
            &Event::WorkspaceMemberRemoved {
                workspace_id,
                account_id,
            },
        )
        .await?;
        audit::record(
            connection,
            now,
            Some(access.account_id),
            About::Workspace(workspace_id),
            &AuditEvent::WorkspaceMemberRemoved {
                account_id,
                reason: reason.clone(),
                recipient_message: recipient_message.clone(),
            },
        )
        .await?;
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}
