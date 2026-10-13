use axum::Json;
use axum::extract::State;
use junto_authz::{AuditScope, Request};
use junto_protocol::audit::{AuditFilterQuery, AuditPage};
use junto_protocol::ids::{AuditEntryId, ChannelId, WorkspaceId};
use sqlx::{Postgres, QueryBuilder};

use crate::access::Access;
use crate::audit::{self, ENTRY_COLUMNS, EntryRow};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiPath, ApiQuery, Bearer};
use crate::read;
use crate::state::AppState;

const DEFAULT_PAGE: u32 = 50;
const LARGEST_PAGE: u32 = 200;

fn page_size(query: &AuditFilterQuery) -> ApiResult<i64> {
    if query.action.as_ref().is_some_and(|action| {
        action.is_empty()
            || action.len() > 64
            || !action.bytes().all(|b| b.is_ascii_lowercase() || b == b'_')
    }) {
        return Err(ApiError::invalid(
            "action must contain 1 to 64 lowercase ASCII letters or underscores",
        ));
    }
    if let (Some(since), Some(until)) = (query.since, query.until)
        && since >= until
    {
        return Err(ApiError::invalid("since must precede until"));
    }
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

fn source(statement: &mut QueryBuilder<Postgres>, query: &AuditFilterQuery, id_cursor: bool) {
    statement.push(" from audit_entries e");
    if let Some(target) = query.target_id {
        statement
            .push(" join audit_entry_targets t on t.entry_id=e.id and t.target_id=")
            .push_bind(target);
    }
    statement.push(" where true");
    if id_cursor && let Some(before) = query.before {
        statement.push(" and e.id < ").push_bind(before);
    }
    if let Some(action) = &query.action {
        statement.push(" and e.action = ").push_bind(action.clone());
    }
    if let Some(actor) = query.actor_id {
        statement.push(" and e.actor_id = ").push_bind(actor);
    }
    if let Some(since) = query.since {
        statement.push(" and e.occurred_at >= ").push_bind(since);
    }
    if let Some(until) = query.until {
        statement.push(" and e.occurred_at < ").push_bind(until);
    }
}

/// The audit entries about the server: accounts, invites, bans, recoveries and server roles.
#[utoipa::path(
    get,
    path = "/api/v1/audit-log",
    tag = "audit",
    params(AuditFilterQuery),
    security(("session" = [])),
    responses((status = 200, body = AuditPage), ApiError)
)]
pub async fn get_server_audit_log(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiQuery(query): ApiQuery<AuditFilterQuery>,
) -> ApiResult<Json<AuditPage>> {
    let size = page_size(&query)?;
    let now = app.now();
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        if !counted {
            crate::proof::limit(&app.audit_reads, access.account_id, now)?;
            counted = true;
        }
        access.require(Request::AuditEntry(AuditScope::Server))?;
        let mut statement = statement(&query, None, None, size);
        let rows = statement
            .build_query_as()
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
    params(("workspace_id" = WorkspaceId, Path), AuditFilterQuery),
    security(("session" = [])),
    responses((status = 200, body = AuditPage), ApiError)
)]
pub async fn get_workspace_audit_log(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiQuery(query): ApiQuery<AuditFilterQuery>,
) -> ApiResult<Json<AuditPage>> {
    let size = page_size(&query)?;
    let now = app.now();
    let mut counted = false;
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        if !counted {
            crate::proof::limit(&app.audit_reads, access.account_id, now)?;
            counted = true;
        }
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
            let mut statement = statement(&query, Some(workspace_id), None, size);
            let rows = statement
                .build_query_as()
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
        let mut statement = statement(
            &query,
            Some(workspace_id),
            Some((access.account_id, readable_channels)),
            size,
        );
        let rows = statement
            .build_query_as()
            .fetch_all(&mut *connection)
            .await?;
        page(rows, size)
    })
    .map(Json)
}

fn scope(
    statement: &mut QueryBuilder<Postgres>,
    workspace_id: Option<WorkspaceId>,
    visibility: &Option<(junto_protocol::ids::AccountId, Vec<ChannelId>)>,
) {
    if let Some(workspace_id) = workspace_id {
        statement
            .push(" and e.workspace_id = ")
            .push_bind(workspace_id);
    } else {
        statement.push(" and e.workspace_id is null");
    }
    if let Some((account_id, readable_channels)) = visibility {
        statement.push(" and (e.channel_id is null or e.channel_id = any(")
            .push_bind(readable_channels.clone()).push(")) and case when e.call_id is not null then reads_call(e.call_id, ")
            .push_bind(*account_id)
            .push(") when not (e.event ? 'recording_id') then true else coalesce((select reads_call(r.call_id, ")
            .push_bind(*account_id)
            .push(") from recordings r where r.id = (e.event ->> 'recording_id')::uuid), not exists (select 1 from calls k where k.channel_id = e.channel_id and k.created_at <= e.occurred_at and not reads_call(k.id, ")
            .push_bind(*account_id).push("))) end");
    }
}

/// The bounded audit-page statement, after the route has established its current audience.
pub fn statement(
    query: &AuditFilterQuery,
    workspace_id: Option<WorkspaceId>,
    visibility: Option<(junto_protocol::ids::AccountId, Vec<ChannelId>)>,
    size: i64,
) -> QueryBuilder<Postgres> {
    let columns = ENTRY_COLUMNS
        .split(", ")
        .map(|column| format!("e.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = QueryBuilder::new(format!("select {columns}"));
    let by_time = query.since.is_some() || query.until.is_some();
    source(&mut statement, query, !by_time);
    scope(&mut statement, workspace_id, &visibility);
    if by_time && let Some(before) = query.before {
        statement.push(" and (e.occurred_at,e.id) < (select e.occurred_at,e.id");
        source(&mut statement, query, false);
        scope(&mut statement, workspace_id, &visibility);
        statement.push(" and e.id = ").push_bind(before).push(")");
    }
    statement
        .push(if by_time {
            " order by e.occurred_at desc,e.id desc limit "
        } else {
            " order by e.id desc limit "
        })
        .push_bind(size + 1);
    statement
}
