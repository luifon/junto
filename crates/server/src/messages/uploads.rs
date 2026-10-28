//! Attachment metadata, file integrity and delayed filesystem cleanup.

use super::{Place, Purpose};
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
/// The bounds of a file that gets preview metadata (ADR 0408). The largest file that a
/// client reads into memory to draw it.
pub const MOST_PICTURE_BYTES: u64 = 32 * 1024 * 1024;
/// The largest still picture: 128 MiB of pixels as RGBA, which a client decodes before it
/// reduces the picture, and the longest side of a WebP file.
pub const MOST_PICTURE_PIXELS: u64 = 33_554_432;
pub const LONGEST_PICTURE_SIDE: u32 = 16_384;
/// The largest canvas of an animation, 16 MiB as RGBA, which a client decodes once for each
/// frame; the most frames; and the most pixels of all frames together, which bound the
/// time of the decoding.
pub const MOST_FRAME_PIXELS: u64 = 4_194_304;
pub const MOST_FRAMES: u32 = 2_048;
pub const MOST_ANIMATION_PIXELS: u64 = 1_073_741_824;

/// The picture that a client draws for a file, when the file is within the bounds.
pub fn preview(picture: crate::image::Picture) -> Option<crate::image::Picture> {
    let pixels = u64::from(picture.width) * u64::from(picture.height);
    let still =
        pixels <= MOST_PICTURE_PIXELS && picture.width.max(picture.height) <= LONGEST_PICTURE_SIDE;
    let moving = picture.frames <= 1
        || (pixels <= MOST_FRAME_PIXELS
            && picture.frames <= MOST_FRAMES
            && pixels * u64::from(picture.frames) <= MOST_ANIMATION_PIXELS);
    (still && moving).then_some(picture)
}

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
    let app = app.clone();
    tokio::task::spawn_blocking(move || app.attachment_space.free_bytes())
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
    let row = sqlx::query("select * from attachments where id=$1 and owner_id=$2 and not attached and removed_at is null and
             expires_at>$3")
        .bind(id)
        .bind(access.account_id)
        .bind(now)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or_else(ApiError::not_found)?;
    // An upload is for a message that its owner sends: a change of the place.
    let place = Place::load(
        connection,
        access,
        row.try_get("place_id")?,
        Purpose::Change,
    )
    .await?;
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

/// Like [`owned`], for a request that adds bytes to the upload or completes it. The upload
/// of an archived channel takes no change (ADR 0410).
pub async fn writable(
    connection: &mut PgConnection,
    access: &Access,
    id: AttachmentId,
    now: OffsetDateTime,
) -> ApiResult<(Place, Upload)> {
    let found = owned(connection, access, id, now).await?;
    found.0.require_open()?;
    found.0.require_upload(access)?;
    Ok(found)
}

pub async fn of_messages(
    connection: &mut PgConnection,
    ids: &[MessageId],
) -> ApiResult<std::collections::HashMap<MessageId, Vec<Attachment>>> {
    let rows = sqlx::query("select id,message_id,name,bytes,sha256,media_type,width,height,frames from attachments where message_id=any($1) and complete and removed_at is null order by
             position,id")
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
                frames: row
                    .try_get::<Option<i32>, _>("frames")?
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
    if !ids.is_empty() {
        place.require(access, junto_authz::WorkspacePermission::AttachFiles)?;
    }
    let limits = limits(connection, app).await?;
    if ids.len() > limits.files_per_message as usize
        || ids.iter().enumerate().any(|(i, id)| ids[..i].contains(id))
    {
        return Err(ApiError::invalid("too many or repeated attachments"));
    }
    for (position, id) in ids.iter().enumerate() {
        let (uploaded, u) = owned(connection, access, *id, app.now()).await?;
        if uploaded.id != place.id || !u.complete {
            return Err(ApiError::invalid(
                "attachment is incomplete or belongs to another place",
            ));
        }
        sqlx::query("update attachments set message_id=$2,attached=true,position=$3,retention_at=(select created_at from
             messages where id=$2 and not permanent) where id=$1")
            .bind(id)
            .bind(message)
            .bind(position as i32)
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
    Ok(sqlx::query_as("select offset_bytes,bytes,sha256 from attachment_chunks where attachment_id=$1 and stored order by
             offset_bytes")
        .bind(id)
        .fetch_all(connection)
        .await?)
}

/// The stored chunks of an upload read in order as one file. Each chunk is checked against
/// its length and hash when it is read, and adds to the hash of the file.
struct Stored {
    root: PathBuf,
    id: AttachmentId,
    parts: std::vec::IntoIter<(i64, i32, String)>,
    chunk: Vec<u8>,
    at: usize,
    digest: Sha256,
    failure: Option<ApiError>,
}

impl Stored {
    fn next_chunk(&mut self) -> ApiResult<bool> {
        let Some((offset, size, expected)) = self.parts.next() else {
            return Ok(false);
        };
        let mut bytes = Vec::new();
        files::open(&self.root, &chunk(self.id, offset as u64))
            .map_err(io)?
            .take(CHUNK as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io)?;
        if bytes.len() != size as usize || hash(&bytes) != expected {
            return Err(ApiError::conflict(
                "an uploaded chunk failed its integrity check",
            ));
        }
        self.digest.update(&bytes);
        self.chunk = bytes;
        self.at = 0;
        Ok(true)
    }
}

impl Read for Stored {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        while self.failure.is_none() && self.at == self.chunk.len() {
            match self.next_chunk() {
                Ok(true) => {}
                Ok(false) => return Ok(0),
                Err(error) => self.failure = Some(error),
            }
        }
        if self.failure.is_some() {
            return Err(std::io::Error::other("an uploaded chunk was not read"));
        }
        let count = buffer.len().min(self.chunk.len() - self.at);
        buffer[..count].copy_from_slice(&self.chunk[self.at..self.at + count]);
        self.at += count;
        Ok(count)
    }
}

/// Reads every chunk of an upload once: checks each chunk, computes the hash of the file,
/// and reads the preview metadata of a picture within [`MOST_PICTURE_BYTES`] from the
/// parts of the file that state it.
pub fn check(
    root: PathBuf,
    id: AttachmentId,
    parts: Vec<(i64, i32, String)>,
) -> ApiResult<(String, Option<crate::image::Picture>)> {
    let total: u64 = parts.iter().map(|(_, size, _)| *size as u64).sum();
    let mut stored = Stored {
        root,
        id,
        parts: parts.into_iter(),
        chunk: Vec::new(),
        at: 0,
        digest: Sha256::new(),
        failure: None,
    };
    let image = if total <= MOST_PICTURE_BYTES {
        crate::image::picture(std::io::BufReader::new(&mut stored), total).and_then(preview)
    } else {
        None
    };
    let rest = std::io::copy(&mut stored, &mut std::io::sink());
    if let Some(failure) = stored.failure.take() {
        return Err(failure);
    }
    rest.map_err(io)?;
    Ok((
        stored
            .digest
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
    let ids: Option<Vec<(AttachmentId, AttachmentId)>> = transact!(app, |connection| {
        // A backup that runs still copies the files (ADR 0018). The same transaction
        // reads the backup and selects the files: a file that is removed after the start
        // of a backup is not selected.
        if crate::backup::held(&mut *connection, now).await? {
            return Ok(None);
        }
        sqlx::query(EXPIRE_QUERY)
            .bind(now)
            .bind(i64::from(app.config.limits.maintenance_batch))
            .execute(&mut *connection)
            .await?;
        let ids: Vec<(AttachmentId, AttachmentId)> = sqlx::query_as("select id,storage_id from attachments a where removed_at is not null and not exists(select 1 from
             attachments live where live.storage_id=a.storage_id and live.removed_at is null) and touched_at<$1 order
             by touched_at,id limit $2")
            .bind(now - time::Duration::minutes(1))
            .bind(i64::from(app.config.limits.maintenance_batch))
            .fetch_all(&mut *connection)
            .await?;
        Ok(Some(ids))
    })?;
    let Some(ids) = ids else {
        return Ok(());
    };
    for (id, storage_id) in ids {
        if app.attachment_checks.contains(id) {
            continue;
        }
        let directory_root = root.clone();
        let removed = tokio::task::spawn_blocking(move || {
            files::remove(&directory_root, &directory(storage_id))
        })
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
        Ok(sqlx::query_scalar("select id from attachments where complete and not staging_cleaned and removed_at is null and touched_at<$1
             order by touched_at,id limit $2")
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

pub const EXPIRE_QUERY: &str = "update attachments set removed_at=$1 where id in(select id from attachments where removed_at is null and
             not attached and expires_at<=$1 order by expires_at,id limit $2)";

/// Copies metadata while immutable chunks remain shared until the last reference is removed.
pub async fn copy_attachments(
    connection: &mut PgConnection,
    now: time::OffsetDateTime,
    channel: junto_protocol::ids::ChannelId,
    originals: &[MessageId],
    copies: &[MessageId],
) -> ApiResult<()> {
    let mut cursor = uuid::Uuid::nil();
    loop {
        let rows: Vec<(AttachmentId, MessageId)> = sqlx::query_as("select id,message_id from attachments where message_id=any($1) and removed_at is null and id>$2 order by
             id limit 200")
            .bind(originals)
            .bind(cursor)
            .fetch_all(&mut *connection)
            .await?;
        if rows.is_empty() {
            break;
        }
        if let Some(row) = rows.last() {
            cursor = row.0.as_uuid();
        }
        let old_ids: Vec<_> = rows.iter().map(|(id, _)| *id).collect();
        let new_ids: Vec<_> = rows.iter().map(|_| AttachmentId::generate()).collect();
        let message_ids: Vec<_> = rows
            .iter()
            .map(|(_, message)| {
                originals
                    .iter()
                    .position(|id| id == message)
                    .map(|index| copies[index])
                    .ok_or_else(ApiError::not_found)
            })
            .collect::<ApiResult<_>>()?;
        sqlx::query("insert into
             attachments(id,place_id,owner_id,message_id,name,bytes,received,sha256,complete,media_type,width,height,frames,expires_at,touched_at,attached,staging_cleaned,storage_id,retention_at,position)
             select
             mapping.new_id,$1,a.owner_id,mapping.message_id,a.name,a.bytes,a.received,a.sha256,true,a.media_type,a.width,a.height,a.frames,a.expires_at,$2,true,true,a.storage_id,a.retention_at,a.position
             from unnest($3::uuid[],$4::uuid[],$5::uuid[]) mapping(old_id,new_id,message_id) join attachments a on
             a.id=mapping.old_id")
            .bind(channel)
            .bind(now)
            .bind(&old_ids)
            .bind(&new_ids)
            .bind(&message_ids)
            .execute(&mut *connection)
            .await?;
        sqlx::query("insert into attachment_chunks(attachment_id,offset_bytes,bytes,sha256,stored) select
             mapping.new_id,c.offset_bytes,c.bytes,c.sha256,c.stored from unnest($1::uuid[],$2::uuid[])
             mapping(old_id,new_id) join attachment_chunks c on c.attachment_id=mapping.old_id")
            .bind(&old_ids)
            .bind(&new_ids)
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}
