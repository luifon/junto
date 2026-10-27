use junto_protocol::audit::{AuditEntry, AuditEvent};
use junto_protocol::ids::{AccessTokenId, AccountId, AuditEntryId, ChannelId, WorkspaceId};
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;

use crate::error::{ApiError, ApiResult};

/// The setting of a transaction that names the access token of its request. `Access::open`
/// sets it, and every entry of the transaction is written with it, so that no writer of an
/// entry can leave the token out (ADR 0210).
pub const ACCESS_TOKEN_SETTING: &str = "junto.access_token";

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
    // An entry that names a recording keeps the call of that recording, so that it is
    // returned by the rule of the recording also when the recording is gone (ADR 0604).
    let named_recording = match event {
        AuditEvent::RecordingStarted { recording_id }
        | AuditEvent::RecordingStopped { recording_id, .. }
        | AuditEvent::RecordingRemoved { recording_id }
        | AuditEvent::TranscriptRequested { recording_id } => Some(*recording_id),
        _ => None,
    };
    sqlx::query(
        "insert into audit_entries
             (id, occurred_at, actor_id, workspace_id, channel_id, action, event,
              access_token_id, call_id)
         values ($1, $2, $3, $4, $5, $6, $7, nullif(current_setting($8, true), '')::uuid,
                 (select call_id from recordings where id = $9))",
    )
    .bind(AuditEntryId::generate())
    .bind(now)
    .bind(actor)
    .bind(workspace_id)
    .bind(channel_id)
    .bind(event.action())
    .bind(Json(event))
    .bind(ACCESS_TOKEN_SETTING)
    .bind(named_recording)
    .execute(&mut *connection)
    .await?;
    // A recording writes its own notes, and a change of a channel writes the line of a
    // rename only (ADR 0408).
    if let Some(channel_id) = channel_id
        && !matches!(
            event,
            AuditEvent::RecordingStarted { .. }
                | AuditEvent::RecordingStopped { .. }
                | AuditEvent::ChannelUpdated { .. }
        )
    {
        let recording_id = match event {
            AuditEvent::RecordingRemoved { recording_id }
            | AuditEvent::TranscriptRequested { recording_id } => Some(*recording_id),
            _ => None,
        };
        let call_id = if let Some(recording_id) = recording_id {
            sqlx::query_scalar("select call_id from recordings where id=$1")
                .bind(recording_id)
                .fetch_optional(&mut *connection)
                .await?
        } else if event.action().starts_with("call_")
            || event.action().starts_with("recording_")
            || event.action().starts_with("transcript_")
        {
            sqlx::query_scalar("select id from calls where channel_id=$1")
                .bind(channel_id)
                .fetch_optional(&mut *connection)
                .await?
        } else {
            None
        };
        crate::messages::system(
            connection,
            now,
            channel_id,
            junto_protocol::ids::MessageId::generate(),
            &junto_protocol::message::SystemMessage {
                action: event.action().to_owned(),
                actor_id: actor,
                recording_id: None,
                duration_seconds: None,
                subject_id: match event {
                    AuditEvent::CallMicrophoneBlocked { account_id }
                    | AuditEvent::CallMicrophoneUnblocked { account_id }
                    | AuditEvent::CallSoundBlocked { account_id }
                    | AuditEvent::CallSoundUnblocked { account_id }
                    | AuditEvent::CallMemberDisconnected { account_id }
                    | AuditEvent::CallMemberMovedOut { account_id }
                    | AuditEvent::CallMemberMovedIn { account_id } => Some(*account_id),
                    _ => None,
                },
                name: None,
                previous_name: None,
            },
            call_id,
        )
        .await?;
    }
    Ok(())
}

pub type EntryRow = (
    AuditEntryId,
    OffsetDateTime,
    Option<AccountId>,
    Option<WorkspaceId>,
    Option<ChannelId>,
    Json<serde_json::Value>,
    Option<AccessTokenId>,
);

pub const ENTRY_COLUMNS: &str =
    "id, occurred_at, actor_id, workspace_id, channel_id, event, access_token_id";

pub fn entry(row: EntryRow) -> ApiResult<AuditEntry> {
    let (id, occurred_at, actor_id, workspace_id, channel_id, Json(event), access_token_id) = row;
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
        access_token_id,
        workspace_id,
        channel_id,
        event,
    })
}
