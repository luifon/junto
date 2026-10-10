use junto_protocol::audit::{AuditEntry, AuditEvent};
use junto_protocol::ids::{AccountId, AuditEntryId, ChannelId, WorkspaceId};
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;

use crate::error::{ApiError, ApiResult};

/// What an entry is about. It decides who can read the entry.
#[derive(Debug, Clone, Copy)]
pub enum About {
    Server,
    Workspace(WorkspaceId),
    Channel(WorkspaceId, ChannelId),
}

pub async fn record(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    actor: Option<AccountId>,
    about: About,
    event: &AuditEvent,
) -> ApiResult<()> {
    let (workspace_id, channel_id) = match about {
        About::Server => (None, None),
        About::Workspace(workspace_id) => (Some(workspace_id), None),
        About::Channel(workspace_id, channel_id) => (Some(workspace_id), Some(channel_id)),
    };
    sqlx::query(
        "insert into audit_entries
             (id, occurred_at, actor_id, workspace_id, channel_id, action, event)
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(AuditEntryId::generate())
    .bind(now)
    .bind(actor)
    .bind(workspace_id)
    .bind(channel_id)
    .bind(event.action())
    .bind(Json(event))
    .execute(connection)
    .await?;
    Ok(())
}

pub type EntryRow = (
    AuditEntryId,
    OffsetDateTime,
    Option<AccountId>,
    Option<WorkspaceId>,
    Option<ChannelId>,
    Json<serde_json::Value>,
);

pub const ENTRY_COLUMNS: &str = "id, occurred_at, actor_id, workspace_id, channel_id, event";

pub fn entry(row: EntryRow) -> ApiResult<AuditEntry> {
    let (id, occurred_at, actor_id, workspace_id, channel_id, Json(event)) = row;
    let event: AuditEvent =
        serde_json::from_value(event).map_err(|error| ApiError::internal(&error))?;
    if event == AuditEvent::Unknown {
        return Err(ApiError::internal(
            &"a stored audit entry has an action that this server does not know",
        ));
    }
    Ok(AuditEntry {
        id,
        occurred_at,
        actor_id,
        workspace_id,
        channel_id,
        event,
    })
}
