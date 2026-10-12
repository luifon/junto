use axum::Json;
use axum::extract::State;
use junto_authz::{AuditScope, Request};
use junto_protocol::audit::{AuditPage, AuditQuery};
use junto_protocol::ids::{AuditEntryId, ChannelId, WorkspaceId};
use sqlx::AssertSqlSafe;

use crate::access::Access;
use crate::audit::{self, ENTRY_COLUMNS, EntryRow};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiPath, ApiQuery, Bearer};
use crate::read;
use crate::state::AppState;

const DEFAULT_PAGE: u32 = 50;
const LARGEST_PAGE: u32 = 200;

fn page_size(query: &AuditQuery) -> ApiResult<i64> {
    match query.limit.unwrap_or(DEFAULT_PAGE) {
        limit @ 1..=LARGEST_PAGE => Ok(i64::from(limit)),
        _ => Err(ApiError::invalid(format!(
            "limit must be from 1 to {LARGEST_PAGE}"
        ))),
    }
}

fn page(rows: Vec<EntryRow>, size: i64) -> ApiResult<AuditPage> {
    let size = usize::try_from(size).unwrap_or(usize::MAX);
    let has_more = rows.len() > size;
    let entries = rows
        .into_iter()
        .take(size)
        .map(audit::entry)
        .collect::<ApiResult<Vec<_>>>()?;
    let next_before: Option<AuditEntryId> = if has_more {
        entries.last().map(|entry| entry.id)
    } else {
        None
    };
    Ok(AuditPage {
        entries,
        next_before,
    })
}

/// The audit entries about the server: accounts, invites, bans, recoveries and server roles.
#[utoipa::path(
    get,
    path = "/api/v1/audit-log",
    tag = "audit",
    params(AuditQuery),
    security(("session" = [])),
    responses((status = 200, body = AuditPage), ApiError)
)]
pub async fn get_server_audit_log(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiQuery(query): ApiQuery<AuditQuery>,
) -> ApiResult<Json<AuditPage>> {
    let size = page_size(&query)?;
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        access.require(Request::AuditEntry(AuditScope::Server))?;
        let rows: Vec<EntryRow> = sqlx::query_as(AssertSqlSafe(format!(
            "select {ENTRY_COLUMNS} from audit_entries
             where workspace_id is null and ($1::uuid is null or id < $1)
             order by id desc
             limit $2"
        )))
        .bind(query.before)
        .bind(size + 1)
        .fetch_all(&mut *connection)
        .await?;
        page(rows, size)
    })
    .map(Json)
}

/// The audit entries of a workspace. An entry about a channel is returned only when the caller
/// can see that channel; an entry about a deleted channel only to a server administrator.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/audit-log",
    tag = "audit",
    params(("workspace_id" = WorkspaceId, Path), AuditQuery),
    security(("session" = [])),
    responses((status = 200, body = AuditPage), ApiError)
)]
pub async fn get_workspace_audit_log(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiQuery(query): ApiQuery<AuditQuery>,
) -> ApiResult<Json<AuditPage>> {
    let size = page_size(&query)?;
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let deleted: bool = sqlx::query_scalar(
            "select exists (select 1 from workspaces where id = $1 and deleted_at is not null)",
        )
        .bind(workspace_id)
        .fetch_one(&mut *connection)
        .await?;
        if deleted {
            // The entries of a deleted workspace stay for the administrators, like the
            // entries of a deleted channel (ADR 0303). Every other caller gets the answer
            // of a workspace that does not exist.
            if !access.allows(Request::AuditEntry(AuditScope::DeletedWorkspace)) {
                return Err(ApiError::not_found());
            }
            let rows: Vec<EntryRow> = sqlx::query_as(AssertSqlSafe(format!(
                "select {ENTRY_COLUMNS} from audit_entries
                 where workspace_id = $1 and ($2::uuid is null or id < $2)
                 order by id desc
                 limit $3"
            )))
            .bind(workspace_id)
            .bind(query.before)
            .bind(size + 1)
            .fetch_all(&mut *connection)
            .await?;
            return page(rows, size);
        }
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::AuditEntry(AuditScope::Workspace(&scope)))?;

        let readable_channels: Vec<ChannelId> = access
            .workspace_channels(connection, workspace_id, &scope)
            .await?
            .into_iter()
            .filter(|(record, channel_scope)| {
                access.allows(Request::AuditEntry(match record.deleted_at {
                    Some(_) => AuditScope::DeletedChannel,
                    None => AuditScope::Channel(channel_scope),
                }))
            })
            .map(|(record, _)| record.channel.id)
            .collect();

        // An entry about a recording names a person inside its call. While that call runs
        // and hides who is inside, the entry is for the people inside, as the recording is
        // (ADR 0604). The entry keeps the call of its recording. An entry from before that
        // column existed is decided by the row of its recording, and when that row is
        // gone, by the call that the channel had when the entry was written.
        let rows: Vec<EntryRow> = sqlx::query_as(AssertSqlSafe(format!(
            "select {ENTRY_COLUMNS} from audit_entries e
             where workspace_id = $1
               and (channel_id is null or channel_id = any($2))
               and ($3::uuid is null or id < $3)
               and case
                       when e.call_id is not null then reads_call(e.call_id, $5)
                       when not (e.event ? 'recording_id') then true
                       else coalesce((
                               select reads_call(r.call_id, $5) from recordings r
                               where r.id = (e.event ->> 'recording_id')::uuid),
                           not exists (
                               select 1 from calls k
                               where k.channel_id = e.channel_id
                                 and k.created_at <= e.occurred_at
                                 and not reads_call(k.id, $5)))
                   end
             order by id desc
             limit $4"
        )))
        .bind(workspace_id)
        .bind(&readable_channels)
        .bind(query.before)
        .bind(size + 1)
        .bind(access.account_id)
        .fetch_all(&mut *connection)
        .await?;
        page(rows, size)
    })
    .map(Json)
}
