//! Leaving a workspace, deleting one's own account, and the return of an inactive account
//! (ADR 0121).

use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::{Departing, Request};
use junto_protocol::account::{AccountStatus, Departure};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, DeviceId, WorkspaceId};
use junto_protocol::keys::PublicKey;
use sqlx::PgConnection;
use time::OffsetDateTime;

use crate::access::Access;
use crate::audit::{self, About};
use crate::db::StoredText;
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::extract::{ApiPath, Bearer};
use crate::state::AppState;
use crate::store::{self, SessionEnd};
use crate::transact;

/// Ends the membership of an account in a workspace, for a removal by an administrator and
/// for a departure of the account itself (ADR 0106, ADR 0121): the membership with its role
/// assignments, the grants of the invites that the account created for the workspace, and a
/// seat of the account in a call of the workspace. `not_found` when the account is not a
/// member.
pub(crate) async fn end_membership(
    connection: &mut PgConnection,
    app: &AppState,
    now: OffsetDateTime,
    actor: AccountId,
    account_id: AccountId,
    workspace_id: WorkspaceId,
) -> ApiResult<()> {
    let removed =
        sqlx::query("delete from workspace_members where workspace_id=$1 and account_id=$2")
            .bind(workspace_id)
            .bind(account_id)
            .execute(&mut *connection)
            .await?;
    if removed.rows_affected() == 0 {
        return Err(ApiError::not_found());
    }
    super::invites::withdraw_workspace(connection, now, actor, account_id, workspace_id).await?;
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
    .await
}

/// Leaves a workspace. Leaving the last workspace makes the account inactive: its sessions,
/// access tokens, server roles and invites end, and it returns with an invite or an approved
/// join request.
#[utoipa::path(
    post,
    path = "/api/v1/workspaces/{workspace_id}/departure",
    tag = "workspaces",
    params(("workspace_id" = WorkspaceId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn leave_workspace(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let account_id = access.account_id;
        super::moderation::locked_status(connection, account_id).await?;
        let memberships: Vec<WorkspaceId> =
            sqlx::query_scalar("select workspace_id from workspace_members where account_id = $1")
                .bind(account_id)
                .fetch_all(&mut *connection)
                .await?;
        if !memberships.contains(&workspace_id) {
            return Err(ApiError::not_found());
        }
        let last = memberships.len() == 1;
        access.require(Request::Depart(if last {
            Departing::Server
        } else {
            Departing::Workspace
        }))?;
        end_membership(connection, &app, now, account_id, account_id, workspace_id).await?;
        record_left(connection, account_id, workspace_id).await?;
        audit::record(
            connection,
            now,
            Some(account_id),
            About::Workspace(workspace_id),
            &AuditEvent::WorkspaceMemberLeft { account_id },
        )
        .await?;
        if last {
            end_account(connection, &app, now, account_id, Ending::Inactive).await?;
        }
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Deletes the caller's account. Its messages, files, reactions, threads, direct
/// conversations, recordings, transcripts and audit entries stay with its name; its
/// memberships, roles, devices, sessions, access tokens, invites and avatar end. A deleted
/// account cannot return.
#[utoipa::path(
    delete,
    path = "/api/v1/account",
    tag = "account",
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn delete_account(State(app): State<AppState>, bearer: Bearer) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let account_id = access.account_id;
        access.require(Request::Depart(Departing::Server))?;
        super::moderation::locked_status(connection, account_id).await?;
        let workspace_ids: Vec<WorkspaceId> = sqlx::query_scalar(
            "select workspace_id from workspace_members where account_id = $1
             order by workspace_id",
        )
        .bind(account_id)
        .fetch_all(&mut *connection)
        .await?;
        for workspace_id in &workspace_ids {
            end_membership(connection, &app, now, account_id, account_id, *workspace_id).await?;
            record_left(connection, account_id, *workspace_id).await?;
            audit::record(
                connection,
                now,
                Some(account_id),
                About::Workspace(*workspace_id),
                &AuditEvent::WorkspaceMemberLeft { account_id },
            )
            .await?;
        }
        end_account(
            connection,
            &app,
            now,
            account_id,
            Ending::Deleted { workspace_ids },
        )
        .await
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Stores that an account left a workspace by itself: the members of that workspace are
/// told of its departure from the server and see its profile after it (ADR 0121).
async fn record_left(
    connection: &mut PgConnection,
    account_id: AccountId,
    workspace_id: WorkspaceId,
) -> ApiResult<()> {
    sqlx::query(
        "insert into account_departures (account_id, workspace_id) values ($1, $2)
         on conflict do nothing",
    )
    .bind(account_id)
    .bind(workspace_id)
    .execute(connection)
    .await?;
    Ok(())
}

/// How an account ends its presence on the server.
enum Ending {
    /// It left its last workspace.
    Inactive,
    /// Its person deleted it, and it left these workspaces.
    Deleted { workspace_ids: Vec<WorkspaceId> },
}

/// Makes an account that is a member of no workspace inactive or deleted, in the transaction
/// of its departure.
async fn end_account(
    connection: &mut PgConnection,
    app: &AppState,
    now: OffsetDateTime,
    account_id: AccountId,
    ending: Ending,
) -> ApiResult<()> {
    let (status, departure, session_end) = match ending {
        Ending::Inactive => (
            AccountStatus::Inactive,
            Departure::Inactive,
            SessionEnd::Inactive,
        ),
        Ending::Deleted { .. } => (
            AccountStatus::Deleted,
            Departure::Deleted,
            SessionEnd::Deleted,
        ),
    };
    // The time comes first: the check of the table needs it with the state.
    sqlx::query("update accounts set departed_at = $2 where id = $1")
        .bind(account_id)
        .bind(now)
        .execute(&mut *connection)
        .await?;
    super::moderation::set_status(connection, now, account_id, status).await?;
    events::log::departure_changed(connection, now, account_id, Some(departure)).await?;
    store::contact_changed(connection, now, account_id).await?;
    store::end_sessions(connection, &app.calls, now, account_id, session_end).await?;
    crate::exports::account_ended(connection, now, account_id).await?;
    let roles = sqlx::query("delete from account_server_roles where account_id = $1")
        .bind(account_id)
        .execute(&mut *connection)
        .await?;
    if roles.rows_affected() > 0 {
        let changed = Event::AccountServerRolesChanged {
            account_id,
            server_role_ids: Vec::new(),
        };
        events::publish(connection, now, &changed).await?;
    }
    super::invites::revoke_all_of(connection, now, account_id).await?;
    let entry = match ending {
        Ending::Inactive => AuditEvent::AccountLeft { account_id },
        Ending::Deleted { workspace_ids } => {
            sqlx::query("delete from devices where account_id = $1")
                .bind(account_id)
                .execute(&mut *connection)
                .await?;
            sqlx::query("delete from account_avatars where account_id = $1")
                .bind(account_id)
                .execute(&mut *connection)
                .await?;
            AuditEvent::AccountDeleted {
                account_id,
                workspace_ids,
            }
        }
    };
    audit::record(connection, now, Some(account_id), About::Server, &entry).await
}

/// The inactive account to which a device key belongs, with the device of the key, for a
/// join or a join request of that key (ADR 0121). `None` for a key that belongs to no
/// account. A key of an account in another state gets `key_already_registered`, as before.
pub(crate) async fn returning_account(
    connection: &mut PgConnection,
    public_key: &PublicKey,
) -> ApiResult<Option<(AccountId, DeviceId)>> {
    let row: Option<(AccountId, DeviceId, String)> = sqlx::query_as(
        "select a.id, d.id, a.status from devices d
         join accounts a on a.id = d.account_id
         where d.public_key = $1
         for update of a",
    )
    .bind(public_key.as_bytes().as_slice())
    .fetch_optional(&mut *connection)
    .await?;
    match row {
        None => Ok(None),
        Some((account_id, device_id, status)) => match AccountStatus::read(&status)? {
            AccountStatus::Inactive => Ok(Some((account_id, device_id))),
            _ => Err(store::key_already_registered()),
        },
    }
}

/// Makes an inactive account active again, as a member of `workspace_ids` without roles.
pub(crate) async fn reactivate(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    workspace_ids: &[WorkspaceId],
) -> ApiResult<()> {
    super::moderation::set_status(connection, now, account_id, AccountStatus::Active).await?;
    sqlx::query("update accounts set departed_at = null where id = $1")
        .bind(account_id)
        .execute(&mut *connection)
        .await?;
    events::log::departure_changed(connection, now, account_id, None).await?;
    store::add_members(connection, now, account_id, workspace_ids).await?;
    store::contact_changed(connection, now, account_id).await
}
