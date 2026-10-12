//! Archived channels (ADR 0410): the archive period of a workspace, the notice that
//! precedes the automatic deletion of an archived channel, and that deletion.

use junto_protocol::events::Event;
use junto_protocol::ids::{ChannelId, MessageId, WorkspaceId};
use junto_protocol::message::SystemMessage;
use junto_protocol::structure::ArchivedChannel;
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::access::{ChannelRecord, stored_channel};
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::routes::channels::{Removed, remove_channel};
use crate::state::AppState;
use crate::transact;

/// The period of a workspace that has no setting.
pub const DEFAULT_DAYS: u32 = 30;
/// The notice of a deletion comes this long before the deletion, so no period is shorter.
pub const NOTICE_DAYS: u32 = 7;
pub const MOST_DAYS: u32 = 36_500;
const NOTICE: Duration = Duration::days(NOTICE_DAYS as i64);

/// The action of the system line that the notice writes into the channel.
pub const NOTICE_ACTION: &str = "channel_deletion_due";

/// The archive period of a workspace in days. `None`: never.
pub async fn period(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
) -> ApiResult<Option<u32>> {
    let stored: Option<Option<i32>> =
        sqlx::query_scalar("select days from workspace_archive_periods where workspace_id = $1")
            .bind(workspace_id)
            .fetch_optional(connection)
            .await?;
    match stored {
        None => Ok(Some(DEFAULT_DAYS)),
        Some(None) => Ok(None),
        Some(Some(days)) => Ok(Some(
            u32::try_from(days).map_err(|error| ApiError::internal(&error))?,
        )),
    }
}

/// The earliest time at which the server deletes an archived channel. `None` when it does
/// not: the workspace has no period, or the channel contains permanent messages (ADR 0402).
/// The deletion follows its notice by 7 days; before the notice, the time is the end of the
/// period, and not less than 7 days from now.
pub fn deletes_at(
    record: &ChannelRecord,
    notice_at: Option<OffsetDateTime>,
    days: Option<u32>,
    now: OffsetDateTime,
) -> Option<OffsetDateTime> {
    let archived_at = record.archived_at?;
    if record.permanent_message_count > 0 {
        return None;
    }
    let days = days?;
    Some(match notice_at {
        Some(notice_at) => notice_at + NOTICE,
        None => (archived_at + Duration::days(i64::from(days))).max(now + NOTICE),
    })
}

/// An archived channel as the routes and the events state it.
pub async fn view(
    connection: &mut PgConnection,
    record: ChannelRecord,
    days: Option<u32>,
    now: OffsetDateTime,
) -> ApiResult<ArchivedChannel> {
    let archived_at = record
        .archived_at
        .ok_or_else(|| ApiError::internal(&"the channel is not archived"))?;
    let notice_at: Option<OffsetDateTime> =
        sqlx::query_scalar("select deletion_notice_at from channels where id = $1")
            .bind(record.channel.id)
            .fetch_one(connection)
            .await?;
    Ok(ArchivedChannel {
        deletes_at: deletes_at(&record, notice_at, days, now),
        channel: record.channel,
        archived_at,
    })
}

/// The next workspace, from `$1` on, that has an archived channel whose deletion is 7 days
/// away or less and that has no notice yet. `$2` is the time.
pub const NOTICE_WORKSPACE: &str = "select w.id, coalesce(p.days, 30)
     from workspaces w
     left join workspace_archive_periods p on p.workspace_id = w.id
     where w.id >= $1 and w.deleted_at is null
       and (p.workspace_id is null or p.days is not null)
       and exists (
           select 1 from channels c
           where c.workspace_id = w.id and c.archived_at is not null and c.deleted_at is null
             and c.permanent_message_count = 0 and c.deletion_notice_at is null
             and c.archived_at <= $2 - make_interval(days => coalesce(p.days, 30) - 7))
     order by w.id limit 1";

/// The channels of the workspace `$1` that get their notice now, at most `$4`. `$2` is the
/// time and `$3` the period in days.
pub const NOTICE_BATCH: &str = "update channels set deletion_notice_at = $2 where id in (
         select id from channels
         where workspace_id = $1 and archived_at is not null and deleted_at is null
           and permanent_message_count = 0 and deletion_notice_at is null
           and archived_at <= $2 - make_interval(days => $3 - 7)
         order by archived_at, id limit $4)
     returning id";

/// The archived channel whose notice is 7 days old or older. `$1` is the time.
pub const DELETION_NEXT: &str = "select id from channels
     where archived_at is not null and deleted_at is null and permanent_message_count = 0
       and deletion_notice_at is not null and deletion_notice_at <= $1 - interval '7 days'
     order by deletion_notice_at, id limit 1";

/// Records the notice for every archived channel whose deletion is 7 days away or less:
/// the event `channel_deletion_due` for the people who see the channel, and a system line
/// in the channel. Each transaction takes at most one maintenance batch of one workspace.
pub async fn notices(app: &AppState) -> ApiResult<()> {
    let batch = i64::from(app.config.limits.maintenance_batch.max(1));
    let mut cursor = Uuid::nil();
    loop {
        let now = app.now();
        let visited = transact!(app.background(), |connection| {
            let due: Option<(WorkspaceId, i32)> = sqlx::query_as(NOTICE_WORKSPACE)
                .bind(cursor)
                .bind(now)
                .fetch_optional(&mut *connection)
                .await?;
            let Some((workspace_id, days)) = due else {
                return Ok(None);
            };
            let channels: Vec<ChannelId> = sqlx::query_scalar(NOTICE_BATCH)
                .bind(workspace_id)
                .bind(now)
                .bind(days)
                .bind(batch)
                .fetch_all(&mut *connection)
                .await?;
            let period = Some(u32::try_from(days).map_err(|error| ApiError::internal(&error))?);
            for channel_id in channels {
                let record = stored_channel(&mut *connection, channel_id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                let channel = view(&mut *connection, record, period, now).await?;
                let line = SystemMessage {
                    action: NOTICE_ACTION.to_owned(),
                    subject_id: None,
                    actor_id: None,
                    recording_id: None,
                };
                let id = MessageId::generate();
                crate::messages::system(&mut *connection, now, channel_id, id, &line, None).await?;
                let due = Event::ChannelDeletionDue { channel };
                events::publish(&mut *connection, now, &due).await?;
            }
            Ok(Some(workspace_id.as_uuid()))
        })?;
        match visited {
            Some(workspace) => cursor = workspace,
            None => return Ok(()),
        }
    }
}

/// Deletes every archived channel whose notice is 7 days old, each in its own transaction,
/// with the function of the manual deletion and no acting account.
pub async fn delete_due(app: &AppState) -> ApiResult<()> {
    loop {
        let now = app.now();
        let deleting = async {
            transact!(app.background(), |connection| {
                let due: Option<ChannelId> = sqlx::query_scalar(DELETION_NEXT)
                    .bind(now)
                    .fetch_optional(&mut *connection)
                    .await?;
                let Some(channel_id) = due else {
                    return Ok(false);
                };
                let record = stored_channel(&mut *connection, channel_id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                let removed = Removed {
                    id: channel_id,
                    workspace_id: record.channel.workspace_id,
                    name: &record.channel.name,
                    permanent_message_count: record.permanent_message_count,
                };
                remove_channel(connection, &app.calls, now, &removed, None).await?;
                Ok(true)
            })
        };
        let deleted: bool = crate::calls::following(app, deleting).await?;
        if !deleted {
            app.purges.wake();
            return Ok(());
        }
    }
}
