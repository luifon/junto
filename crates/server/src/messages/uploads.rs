//! Attachment metadata, file integrity and delayed filesystem cleanup.

use super::Place;
use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    recordings::files,
    state::AppState,
    storage, transact,
};
use junto_protocol::{
    ids::{AttachmentId, MessageId},
    message::{Attachment, Upload},
};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};
use std::{
    io::Read,
    path::{Path, PathBuf},
};
use time::OffsetDateTime;

pub const CHUNK: usize = 1024 * 1024;
pub fn root(app: &AppState) -> ApiResult<&Path> {
    app.config.attachments_directory.as_deref().ok_or_else(|| {
        ApiError::new(
            junto_protocol::error::ErrorCode::Unavailable,
            "attachment storage is not configured",
        )
    })
}
pub fn directory(id: AttachmentId) -> files::Place {
    files::Place::attachment(id)
}
pub fn chunk(id: AttachmentId, offset: u64) -> files::Place {
    directory(id).with(&offset.to_string())
}
pub fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn io(e: std::io::Error) -> ApiError {
    ApiError::internal(&e)
}
pub async fn free(app: &AppState) -> Option<u64> {
    use storage::Disk;
    let directory = root(app).ok()?.to_path_buf();
    tokio::task::spawn_blocking(move || storage::Directory::new(directory).free_bytes().ok())
        .await
        .ok()
        .flatten()
}

pub async fn owned(
    connection: &mut PgConnection,
    access: &Access,
    id: AttachmentId,
    now: OffsetDateTime,
) -> ApiResult<(Place, Upload)> {
    let row = sqlx::query(
        "select *
             from attachments
             where id=$1
             and owner_id=$2
             and not attached
             and removed_at is null
             and expires_at>$3",
    )
    .bind(id)
    .bind(access.account_id)
    .bind(now)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(ApiError::not_found)?;
    let place = Place::load(connection, access, row.try_get("place_id")?).await?;
    Ok((
        place,
        Upload {
            id,
            received: row.try_get::<i64, _>("received")? as u64,
            bytes: row.try_get::<i64, _>("bytes")? as u64,
            complete: row.try_get("complete")?,
            expires_at: row.try_get("expires_at")?,
        },
    ))
}
pub async fn of_messages(
    connection: &mut PgConnection,
    ids: &[MessageId],
) -> ApiResult<std::collections::HashMap<MessageId, Vec<Attachment>>> {
    let rows = sqlx::query(
        "select *
             from attachments
             where message_id=any($1)
             and complete
             and removed_at is null
             order by id",
    )
    .bind(ids)
    .fetch_all(connection)
    .await?;
    let mut result: std::collections::HashMap<MessageId, Vec<Attachment>> =
        std::collections::HashMap::new();
    for row in rows {
        result
            .entry(row.try_get("message_id")?)
            .or_default()
            .push(Attachment {
                id: row.try_get("id")?,
                name: row.try_get("name")?,
                bytes: row.try_get::<i64, _>("bytes")? as u64,
                sha256: row.try_get("sha256")?,
                media_type: row.try_get("media_type")?,
                width: row
                    .try_get::<Option<i32>, _>("width")?
                    .map(|value| value as u32),
                height: row
                    .try_get::<Option<i32>, _>("height")?
                    .map(|value| value as u32),
            });
    }
    Ok(result)
}
pub async fn attach(
    connection: &mut PgConnection,
    app: &AppState,
    access: &Access,
    place: &Place,
    message: MessageId,
    ids: &[AttachmentId],
) -> ApiResult<()> {
    let limits = limits(connection, app).await?;
    if ids.len() > limits.files_per_message as usize
        || ids.iter().enumerate().any(|(i, id)| ids[..i].contains(id))
    {
        return Err(ApiError::invalid("too many or repeated attachments"));
    }
    for id in ids {
        let (uploaded, u) = owned(connection, access, *id, app.now()).await?;
        if uploaded.id != place.id || !u.complete {
            return Err(ApiError::invalid(
                "attachment is incomplete or belongs to another place",
            ));
        }
        sqlx::query("update attachments set message_id=$2,attached=true where id=$1")
            .bind(id)
            .bind(message)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}
pub async fn remove_message(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    id: MessageId,
) -> ApiResult<()> {
    sqlx::query("update attachments set removed_at=$2 where message_id=$1")
        .bind(id)
        .bind(now)
        .execute(connection)
        .await?;
    Ok(())
}
pub async fn chunks(
    connection: &mut PgConnection,
    id: AttachmentId,
) -> ApiResult<Vec<(i64, i32, String)>> {
    Ok(sqlx::query_as(
        "select offset_bytes,bytes,sha256
             from attachment_chunks
             where attachment_id=$1
             and stored
             order by offset_bytes",
    )
    .bind(id)
    .fetch_all(connection)
    .await?)
}
pub fn check(
    root: PathBuf,
    id: AttachmentId,
    parts: Vec<(i64, i32, String)>,
) -> ApiResult<(String, Option<crate::image::Image>)> {
    let mut digest = Sha256::new();
    let mut preview = Vec::new();
    let mut preview_fits = true;
    for (offset, size, expected) in parts {
        let mut bytes = Vec::new();
        files::open(&root, &chunk(id, offset as u64))
            .map_err(io)?
            .take(CHUNK as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() != size as usize || hash(&bytes) != expected {
            return Err(ApiError::conflict(
                "an uploaded chunk failed its integrity check",
            ));
        }
        digest.update(&bytes);
        if preview_fits && preview.len() + bytes.len() <= 8 * CHUNK {
            preview.extend(&bytes);
        } else {
            preview.clear();
            preview_fits = false;
        }
    }
    let image = if preview_fits {
        crate::image::inspect(&preview).filter(|im| im.width <= 1024 && im.height <= 1024)
    } else {
        None
    };
    Ok((
        digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        image,
    ))
}

pub async fn cleanup(app: &AppState) -> ApiResult<()> {
    let Some(root) = app.config.attachments_directory.clone() else {
        return Ok(());
    };
    let now = app.now();
    let ids: Vec<AttachmentId> = transact!(app, |connection| {
        sqlx::query(EXPIRE_QUERY)
            .bind(now)
            .bind(i64::from(app.config.limits.maintenance_batch))
            .execute(&mut *connection)
            .await?;
        let ids: Vec<AttachmentId> = sqlx::query_scalar(
            "select id
                 from attachments
                 where removed_at is not null
                 and touched_at<$1
                 order by touched_at,id limit $2",
        )
        .bind(now - time::Duration::minutes(1))
        .bind(i64::from(app.config.limits.maintenance_batch))
        .fetch_all(&mut *connection)
        .await?;
        Ok(ids)
    })?;
    for id in ids {
        if app.attachment_checks.contains(id) {
            continue;
        }
        let directory_root = root.clone();
        let removed =
            tokio::task::spawn_blocking(move || files::remove(&directory_root, &directory(id)))
                .await
                .map_err(|error| ApiError::internal(&error))?;
        if let Err(error) = removed
            && !files::is_absent(&error)
        {
            tracing::warn!(attachment=%id,%error,"attachment removal will be retried");
            transact!(app, |connection| {
                sqlx::query("update attachments set touched_at=$2 where id=$1")
                    .bind(id)
                    .bind(now)
                    .execute(connection)
                    .await?;
                Ok(())
            })?;
            continue;
        }
        transact!(app, |connection| {
            storage::release(connection, id.as_uuid()).await?;
            sqlx::query("delete from attachments where id=$1 and removed_at is not null")
                .bind(id)
                .execute(connection)
                .await?;
            Ok(())
        })?;
    }
    let complete: Vec<AttachmentId> = crate::read!(app, |connection| {
        Ok(sqlx::query_scalar(
            "select id
                 from attachments
                 where complete
                 and not staging_cleaned
                 and removed_at is null
                 and touched_at<$1
                 order by touched_at,id limit $2",
        )
        .bind(now - time::Duration::minutes(1))
        .bind(i64::from(app.config.limits.maintenance_batch))
        .fetch_all(connection)
        .await?)
    })?;
    for id in complete {
        if app.attachment_checks.contains(id) {
            continue;
        }
        let directory_root = root.clone();
        let cleaned = tokio::task::spawn_blocking(move || {
            let directory = directory(id);
            for name in files::unfinished(&directory_root, &directory)? {
                files::remove(&directory_root, &directory.with(&name))?;
            }
            Ok::<_, std::io::Error>(())
        })
        .await
        .map_err(|error| ApiError::internal(&error))?;
        if let Err(error) = cleaned {
            tracing::warn!(attachment=%id,%error,"attachment staging cleanup will be retried");
            transact!(app, |connection| {
                sqlx::query("update attachments set touched_at=$2 where id=$1")
                    .bind(id)
                    .bind(now)
                    .execute(connection)
                    .await?;
                Ok(())
            })?;
            continue;
        }
        transact!(app, |connection| {
            sqlx::query("update attachments set staging_cleaned=true where id=$1")
                .bind(id)
                .execute(connection)
                .await?;
            Ok(())
        })?;
    }
    Ok(())
}

pub async fn limits(
    connection: &mut PgConnection,
    app: &AppState,
) -> ApiResult<junto_protocol::message::AttachmentLimits> {
    let row: Option<(i64, i32)> =
        sqlx::query_as("select bytes_per_file,files_per_message from attachment_limits")
            .fetch_optional(connection)
            .await?;
    Ok(row.map_or(
        junto_protocol::message::AttachmentLimits {
            bytes_per_file: app.config.limits.attachment_bytes,
            files_per_message: app.config.limits.attachments_per_message as u32,
        },
        |(bytes, files)| junto_protocol::message::AttachmentLimits {
            bytes_per_file: bytes as u64,
            files_per_message: files as u32,
        },
    ))
}

pub const EXPIRE_QUERY: &str = "update attachments set removed_at=$1
    where id in(select id from attachments
        where removed_at is null and not attached and expires_at<=$1
        order by expires_at,id limit $2)";
