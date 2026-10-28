//! Each transaction visits one place and removes at most one configured batch.
use crate::{error::ApiResult, state::AppState, transact};
use junto_protocol::{
    ids::{
        AccountId, AttachmentId, ChannelId, ConversationId, MessageId, RecordingId, WorkspaceId,
    },
    message::MessagePlace,
};
use uuid::Uuid;

pub const FILE_BATCH_QUERY: &str = "update attachments set removed_at=$1 where id in(
    select id from attachments where place_id=$3 and removed_at is null and retention_at is not null
    and retention_at<=$1-make_interval(days=>$4) order by retention_at,id limit $2) returning id,message_id";
pub const DIRECT_BATCH_QUERY: &str =
    "select id,position from messages where place_id=$3 and not deleted
    and created_at<=$1-make_interval(days=>$4) order by created_at,id limit $2";
pub const RECORDING_BATCH_QUERY: &str = "select id from recordings where workspace_id=$3
    and state in ('ready','partly_failed','failed') and requested_at<=$1-make_interval(days=>$4)
    order by requested_at,id limit $2";
pub const NOTICE_CAP_QUERY: &str = "delete from notifications where account_id=$1 and id in(
    select id from notifications where account_id=$1 order by created_at desc,id desc offset 500 limit $2)";

pub async fn run(app: &AppState) -> ApiResult<()> {
    let batch = i64::from(app.config.limits.maintenance_batch.max(1));
    let mut cursor = Uuid::nil();
    loop {
        let outcome = transact!(app, |connection| {
            let place: Option<(ChannelId, WorkspaceId, i32)> = sqlx::query_as("select ch.id,ch.workspace_id,r.days from channels ch join workspace_file_retention r
                 on r.workspace_id=ch.workspace_id where r.days is not null and ch.id>=$1
                 and ch.deleted_at is null and exists(select 1 from attachments a
                     where a.place_id=ch.id and a.removed_at is null and a.retention_at is not null
                     and a.retention_at<=$2-make_interval(days=>r.days)) order by ch.id limit 1")
                .bind(cursor)
                .bind(app.now())
                .fetch_optional(&mut *connection)
                .await?;
            let Some((channel, workspace, days)) = place else {
                return Ok(None);
            };
            let rows: Vec<(AttachmentId, MessageId)> = sqlx::query_as(FILE_BATCH_QUERY)
                .bind(app.now())
                .bind(batch)
                .bind(channel)
                .bind(days)
                .fetch_all(&mut *connection)
                .await?;
            let mut notified = std::collections::HashSet::new();
            for (_, id) in &rows {
                if !notified.insert(*id) {
                    continue;
                }
                crate::exports::message_deleted(connection, app.now(), *id).await?;
                let position: i64 = sqlx::query_scalar("update messages set files_removed_by_retention=true where id=$1 returning position")
                    .bind(id)
                    .fetch_one(&mut *connection)
                    .await?;
                super::changed(
                    connection,
                    app.now(),
                    MessagePlace::Channel {
                        workspace_id: workspace,
                        channel_id: channel,
                    },
                    *id,
                    position as u64,
                    false,
                )
                .await?;
            }
            Ok(Some(channel.as_uuid()))
        })?;
        let Some(place) = outcome else {
            break;
        };
        cursor = place;
    }
    cursor = Uuid::nil();
    loop {
        let outcome = transact!(app, |connection| {
            let place: Option<(ConversationId, AccountId, AccountId, i32)> =
                sqlx::query_as("select c.id,c.low_account_id,c.high_account_id,r.days from conversations c
                 cross join direct_message_retention r where r.days is not null and c.id>=$1 and exists(select 1 from messages m
                     where m.place_id=c.id and not m.deleted and m.created_at<=$2-make_interval(days=>r.days))
                 order by c.id limit 1")
                    .bind(cursor)
                    .bind(app.now())
                    .fetch_optional(&mut *connection)
                    .await?;
            let Some((conversation, low, high, days)) = place else {
                return Ok(None);
            };
            let rows: Vec<(MessageId, i64)> = sqlx::query_as(DIRECT_BATCH_QUERY)
                .bind(app.now())
                .bind(batch)
                .bind(conversation)
                .bind(days)
                .fetch_all(&mut *connection)
                .await?;
            for (id, position) in &rows {
                super::erase(connection, app.now(), *id).await?;
                super::changed(
                    connection,
                    app.now(),
                    MessagePlace::Direct {
                        conversation_id: conversation,
                        low_account_id: low,
                        high_account_id: high,
                    },
                    *id,
                    *position as u64,
                    true,
                )
                .await?;
            }
            Ok(Some(conversation.as_uuid()))
        })?;
        let Some(place) = outcome else {
            break;
        };
        cursor = place;
    }
    cursor = Uuid::nil();
    loop {
        let outcome = transact!(app, |connection| {
            let place: Option<(WorkspaceId, i32)> = sqlx::query_as("select workspace_id,days from workspace_recording_retention where days is not null
                 and workspace_id>=$1 and exists(select 1 from recordings r
                    where r.workspace_id=workspace_recording_retention.workspace_id
                    and r.state in ('ready','partly_failed','failed')
                    and r.requested_at<=$2-make_interval(days=>days)) order by workspace_id limit 1")
                .bind(cursor)
                .bind(app.now())
                .fetch_optional(&mut *connection)
                .await?;
            let Some((workspace, days)) = place else {
                return Ok(None);
            };
            let ids: Vec<RecordingId> = sqlx::query_scalar(RECORDING_BATCH_QUERY)
                .bind(app.now())
                .bind(batch)
                .bind(workspace)
                .bind(days)
                .fetch_all(&mut *connection)
                .await?;
            for id in &ids {
                let recording = crate::recordings::store::recording(
                    connection,
                    crate::recordings::store::Of::Id(*id),
                    true,
                )
                .await?
                .ok_or_else(crate::error::ApiError::not_found)?;
                let lease = app.config.limits.recordings.transcription.lease;
                crate::recordings::store::remove(connection, app.now(), &recording, None, lease)
                    .await?;
            }
            Ok(Some(workspace.as_uuid()))
        })?;
        let Some(place) = outcome else {
            break;
        };
        // A recording of a message that the retention removed can have a run of its
        // transcription, which looks at its job at once (ADR 0608).
        app.recordings.recording_removed();
        cursor = place;
    }
    trim_notices(app, batch).await
}

async fn trim_notices(app: &AppState, batch: i64) -> ApiResult<()> {
    loop {
        let count = transact!(app, |connection| {
            Ok(sqlx::query(
                "delete from notifications where (account_id,id) in(
                select account_id,id from notifications where created_at<=$1-interval '90 days'
                order by created_at,account_id,id limit $2)",
            )
            .bind(app.now())
            .bind(batch)
            .execute(connection)
            .await?
            .rows_affected())
        })?;
        if count < batch as u64 {
            break;
        }
    }
    loop {
        let visited = transact!(app, |connection| {
            let account: Option<AccountId> = sqlx::query_scalar(
                "select account_id from notification_trim_accounts order by account_id limit 1",
            )
            .fetch_optional(&mut *connection)
            .await?;
            let Some(account) = account else {
                return Ok(false);
            };
            let count = sqlx::query(NOTICE_CAP_QUERY)
                .bind(account)
                .bind(batch)
                .execute(&mut *connection)
                .await?
                .rows_affected();
            if count < batch as u64 {
                sqlx::query("delete from notification_trim_accounts where account_id=$1")
                    .bind(account)
                    .execute(connection)
                    .await?;
            }
            Ok(true)
        })?;
        if !visited {
            break;
        }
    }
    Ok(())
}
