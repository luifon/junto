use junto_protocol::events::{Cursor, Event};
use junto_protocol::ids::{AccountId, ChannelId, StreamEpoch, WorkspaceId, WorkspaceRoleId};
use sqlx::PgConnection;
use sqlx::types::Json;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::db::StoredText;
use crate::error::{ApiError, ApiResult};
use crate::events::audience::{Audience, Class, KeptOverride, revalidated_account};
use crate::state::AppState;
use crate::transact;

/// The largest payload and the largest audience that an event is recorded with, as compact
/// JSON. The ceilings of ADR 0306 keep every event of the server below them.
pub const PAYLOAD_BYTES: usize = 12 * 1024;
pub const AUDIENCE_BYTES: usize = 48 * 1024;

/// The sizes that the database refuses and that a read does not load, measured as the text
/// of the stored JSON (ADR 0011).
pub const STORED_PAYLOAD_BYTES: i32 = 16 * 1024;
pub const STORED_AUDIENCE_BYTES: i32 = 64 * 1024;

/// Records an event whose content fixes its audience. The event is written in the transaction
/// of the change that it describes and gets its position when that transaction commits
/// (ADR 0010).
pub async fn publish(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    event: &Event,
) -> ApiResult<()> {
    let audience = Audience::fixed_by(event).ok_or_else(|| {
        ApiError::internal(&format_args!(
            "the event {} needs a stated audience",
            event.name()
        ))
    })?;
    record(connection, now, &audience, event).await
}

/// Records an event for the connections of one account. An event that names voice channels
/// is delivered only while the account sees them (ADR 0510).
pub async fn tell_account(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    event: &Event,
) -> ApiResult<()> {
    record(
        connection,
        now,
        &Audience::addressed(account_id, event),
        event,
    )
    .await
}

/// Records an event for the connections of one account about something at `places` that
/// the event itself does not name. It is delivered only while the account may be at every
/// one of them, so an account that lost a place learns no outcome of what it waited for
/// there (ADR 0510).
pub async fn tell_account_at(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    account_id: AccountId,
    places: &[junto_protocol::call::CallPlace],
    event: &Event,
) -> ApiResult<()> {
    let audience = Audience::addressed_at(account_id, places);
    record(connection, now, &audience, event).await
}

/// Records an event about a request to enter the call of a voice channel for the accounts
/// inside that call.
pub async fn tell_inside(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    event: &Event,
) -> ApiResult<()> {
    let audience = crate::events::audience::of_the_inside(event).ok_or_else(|| {
        ApiError::internal(&format_args!(
            "the event {} is not for the people inside a call",
            event.name()
        ))
    })?;
    record(connection, now, &audience, event).await
}

/// Records an event about a recording or its transcript: for the accounts that may see who
/// is inside `running_call` while the call of the recording runs, and for the accounts that
/// may see the voice channel when the call has ended (ADR 0604).
pub async fn tell_of_recording(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    running_call: Option<junto_protocol::ids::CallId>,
    event: &Event,
) -> ApiResult<()> {
    let Some((workspace_id, channel_id)) = crate::events::audience::recording_place(event) else {
        return Err(ApiError::internal(&format_args!(
            "the event {} is not about a recording",
            event.name()
        )));
    };
    let audience = crate::events::audience::of_recording(workspace_id, channel_id, running_call);
    record(connection, now, &audience, event).await
}

/// Whose permissions a change can have altered.
#[derive(Debug, Clone, Copy)]
pub enum Affected {
    Account(AccountId),
    ServerRoleHolders(junto_protocol::ids::ServerRoleId),
    WorkspaceRoleHolders(WorkspaceId, WorkspaceRoleId),
}

/// Records the notice that tells the affected accounts to load a snapshot. The notice names
/// the workspace and nothing else (ADR 0208). The change can end the access of a person to a
/// call, so the rooms that it can concern are marked for the admission check (ADR 0509).
pub async fn permissions_changed(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    affected: Affected,
    workspace_id: Option<WorkspaceId>,
) -> ApiResult<()> {
    let audience = match affected {
        Affected::Account(account_id) => Audience::Account { account_id },
        Affected::ServerRoleHolders(role_id) => Audience::ServerRoleHolders { role_id },
        Affected::WorkspaceRoleHolders(workspace_id, role_id) => Audience::WorkspaceRoleHolders {
            workspace_id,
            role_id,
        },
    };
    let event = Event::PermissionsChanged { workspace_id };
    record(&mut *connection, now, &audience, &event).await?;
    crate::calls::store::mark_after_permission_change(connection, workspace_id).await
}

/// Records the notice for the accounts that see a channel that appears with overrides, by
/// a fork or by a restore: an override can give an account another list of permissions in
/// the channel than its snapshot states (ADR 0310). A channel without overrides has the
/// list of the workspace for every account, and nothing is recorded.
pub async fn permissions_changed_by_overrides_of(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    workspace_id: WorkspaceId,
    channel_id: ChannelId,
) -> ApiResult<()> {
    let has_overrides: bool =
        sqlx::query_scalar("select exists (select 1 from channel_overrides where channel_id = $1)")
            .bind(channel_id)
            .fetch_one(&mut *connection)
            .await?;
    if !has_overrides {
        return Ok(());
    }
    let audience = Audience::Channel {
        workspace_id,
        channel_id,
        permission: junto_authz::WorkspacePermission::ViewChannel,
    };
    let event = Event::PermissionsChanged {
        workspace_id: Some(workspace_id),
    };
    record(connection, now, &audience, &event).await
}

/// Records the notice for the accounts that see a new workspace. An account that sees it
/// without being a member, by the administrator permission, holds permissions in it that
/// no other event states (ADR 0310).
pub async fn permissions_changed_in_new_workspace(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    workspace_id: WorkspaceId,
) -> ApiResult<()> {
    let audience = Audience::Workspace { workspace_id };
    let event = Event::PermissionsChanged {
        workspace_id: Some(workspace_id),
    };
    record(connection, now, &audience, &event).await
}

/// The accounts that an event is for, selected before the change that ends the rule that
/// names them.
#[derive(Debug, Clone, Copy)]
pub enum Receivers {
    WorkspaceRoleHolders(WorkspaceRoleId),
    ServerRoleHolders(junto_protocol::ids::ServerRoleId),
    WorkspaceMembers(WorkspaceId),
    /// The accounts that have a direct-message conversation with the account.
    ConversationPeers(AccountId),
    /// The accounts that have a direct-message conversation with a holder of the role.
    ConversationPeersOfServerRole(junto_protocol::ids::ServerRoleId),
}

/// Stores the accounts that `receivers` selects now as the list `list`, which is the
/// identifier of the event that the list belongs to. The event is recorded before. Returns
/// the number of accounts.
async fn list_receivers(
    connection: &mut PgConnection,
    list: Uuid,
    receivers: Receivers,
) -> ApiResult<u64> {
    let (statement, id) = match receivers {
        Receivers::WorkspaceRoleHolders(id) => (
            "insert into event_receivers (event_id, account_id)
             select $1, account_id from member_roles where role_id = $2",
            id.as_uuid(),
        ),
        Receivers::ServerRoleHolders(id) => (
            "insert into event_receivers (event_id, account_id)
             select $1, account_id from account_server_roles where role_id = $2",
            id.as_uuid(),
        ),
        Receivers::WorkspaceMembers(id) => (
            "insert into event_receivers (event_id, account_id)
             select $1, account_id from workspace_members where workspace_id = $2",
            id.as_uuid(),
        ),
        Receivers::ConversationPeers(id) => (
            "insert into event_receivers (event_id, account_id)
             select $1, high_account_id from conversations where low_account_id = $2
             union
             select $1, low_account_id from conversations where high_account_id = $2",
            id.as_uuid(),
        ),
        Receivers::ConversationPeersOfServerRole(id) => (
            "insert into event_receivers (event_id, account_id)
             select $1, c.high_account_id
             from account_server_roles h
             join conversations c on c.low_account_id = h.account_id
             where h.role_id = $2
             union
             select $1, c.low_account_id
             from account_server_roles h
             join conversations c on c.high_account_id = h.account_id
             where h.role_id = $2",
            id.as_uuid(),
        ),
    };
    let listed = sqlx::query(statement)
        .bind(list)
        .bind(id)
        .execute(connection)
        .await?;
    Ok(listed.rows_affected())
}

/// Records the notice that tells the accounts that `holders` selects now to load a
/// snapshot, before the role that they hold is deleted. Returns the number of accounts.
pub async fn permissions_changed_for(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    holders: Receivers,
    workspace_id: Option<WorkspaceId>,
) -> ApiResult<u64> {
    let list = Uuid::now_v7();
    let event = Event::PermissionsChanged { workspace_id };
    record_at_id(
        &mut *connection,
        now,
        list,
        &Audience::Listed { list },
        &event,
    )
    .await?;
    let listed = list_receivers(&mut *connection, list, holders).await?;
    crate::calls::store::mark_after_permission_change(connection, workspace_id).await?;
    Ok(listed)
}

/// Records the deletion of a workspace for the accounts that could see it: its members,
/// who are listed before their rows are deleted, and the accounts that see every workspace.
pub async fn workspace_deleted(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    workspace_id: WorkspaceId,
) -> ApiResult<()> {
    let members = Uuid::now_v7();
    let audience = Audience::DeletedWorkspace {
        workspace_id,
        members,
    };
    let event = Event::WorkspaceDeleted { workspace_id };
    record_at_id(&mut *connection, now, members, &audience, &event).await?;
    list_receivers(
        &mut *connection,
        members,
        Receivers::WorkspaceMembers(workspace_id),
    )
    .await?;
    crate::calls::store::mark_after_permission_change(connection, Some(workspace_id)).await
}

/// Records the deletion of a channel for the accounts that could see it. It reads the
/// overrides of the channel, so it runs before they are deleted.
pub async fn channel_deleted(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    workspace_id: WorkspaceId,
    channel_id: ChannelId,
) -> ApiResult<()> {
    let rows: Vec<(WorkspaceRoleId, bool, i64, i64)> = sqlx::query_as(
        "select o.role_id, r.is_everyone, o.allow, o.deny
         from channel_overrides o
         join workspace_roles r on r.id = o.role_id
         where o.channel_id = $1
         order by o.role_id",
    )
    .bind(channel_id)
    .fetch_all(&mut *connection)
    .await?;
    let overrides = rows
        .into_iter()
        .map(|(role_id, is_everyone, allow, deny)| KeptOverride {
            role_id,
            is_everyone,
            allow,
            deny,
        })
        .collect();
    let audience = Audience::DeletedChannel {
        workspace_id,
        channel_id,
        overrides,
    };
    let event = Event::ChannelDeleted {
        workspace_id,
        channel_id,
    };
    record(connection, now, &audience, &event).await
}

pub(crate) async fn record(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    audience: &Audience,
    event: &Event,
) -> ApiResult<()> {
    record_at_id(connection, now, Uuid::now_v7(), audience, event).await
}

pub(crate) async fn record_at_id(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    id: Uuid,
    audience: &Audience,
    event: &Event,
) -> ApiResult<()> {
    if !audience.fits(event) {
        return Err(ApiError::internal(&format_args!(
            "the event {} cannot have this audience",
            event.name()
        )));
    }
    let too_large = |part: &str| {
        ApiError::internal(&format_args!(
            "the {part} of the event {} is too large",
            event.name()
        ))
    };
    let size = |value: Result<Vec<u8>, serde_json::Error>| {
        value.map(|bytes| bytes.len()).unwrap_or(usize::MAX)
    };
    if size(serde_json::to_vec(event)) > PAYLOAD_BYTES {
        return Err(too_large("payload"));
    }
    if size(serde_json::to_vec(audience)) > AUDIENCE_BYTES {
        return Err(too_large("audience"));
    }
    sqlx::query(
        "insert into events
             (id, recorded_at, class, kind, audience, audience_account_id,
              revalidates_account_id, payload)
         values ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(id)
    .bind(now)
    .bind(Class::of(event).stored())
    .bind(event.name())
    .bind(Json(audience))
    .bind(audience.account_id())
    .bind(revalidated_account(audience, event))
    .bind(Json(event))
    .execute(connection)
    .await?;
    Ok(())
}

/// The state of the log in one snapshot of the database.
#[derive(Debug, Clone, Copy)]
pub struct LogState {
    pub epoch: StreamEpoch,
    /// Every position at or below this one has been removed from the log.
    pub trimmed_through: i64,
    /// The highest position that the log has reached.
    pub head: i64,
}

impl LogState {
    pub fn cursor(&self, position: i64) -> ApiResult<Cursor> {
        Ok(Cursor {
            epoch: self.epoch,
            position: u64::try_from(position).map_err(|error| ApiError::internal(&error))?,
        })
    }
}

pub async fn state(connection: &mut PgConnection) -> ApiResult<LogState> {
    let (epoch, trimmed_through, newest): (StreamEpoch, i64, Option<i64>) = sqlx::query_as(
        "select epoch, trimmed_through, (select max(position) from events) from event_stream",
    )
    .fetch_one(connection)
    .await?;
    Ok(LogState {
        epoch,
        trimmed_through,
        head: newest.unwrap_or(0).max(trimmed_through),
    })
}

/// Removes the events that lie more than the kept number of positions behind the newest one,
/// in batches (ADR 0207). Returns the number of removed events.
pub async fn trim(app: &AppState) -> ApiResult<u64> {
    let kept = i64::from(app.config.limits.stream.events_kept);
    let batch = i64::from(app.config.limits.maintenance_batch.max(1));
    let mut removed = 0;
    loop {
        let deleted = transact!(app, |connection| {
            let log = state(connection).await?;
            let through = log.head - kept;
            if through <= 0 {
                return Ok(0);
            }
            if through > log.trimmed_through {
                sqlx::query("update event_stream set trimmed_through = $1")
                    .bind(through)
                    .execute(&mut *connection)
                    .await?;
            }
            let deleted = sqlx::query(
                "delete from events where id in (
                     select id from events where position <= $1 limit $2)",
            )
            .bind(through)
            .bind(batch)
            .execute(&mut *connection)
            .await?;
            Ok(deleted.rows_affected())
        })?;
        removed += deleted;
        if i64::try_from(deleted).unwrap_or(i64::MAX) < batch {
            return Ok(removed);
        }
    }
}
