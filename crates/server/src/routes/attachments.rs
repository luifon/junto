//! Resumable attachment uploads, verified downloads and administrator limits.

use crate::{
    access::Access,
    error::{ApiError, ApiResult},
    extract::{ApiJson, ApiPath, Bearer},
    messages::{self, Place, uploads as files},
    read,
    state::AppState,
    storage, transact,
};
use axum::{
    Json,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode, header},
    response::Response,
};
use junto_protocol::{
    error::ErrorCode,
    ids::{AttachmentId, MessageId},
    message::{StartUpload, Upload},
};
use sqlx::Row;
use time::Duration;

#[utoipa::path(post,path="/api/v1/attachments",tag="messages",request_body=StartUpload,security(("session"=[])),responses((status=201,body=Upload),ApiError))]
pub async fn start_upload(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<StartUpload>,
) -> ApiResult<(StatusCode, Json<Upload>)> {
    crate::proof::limit(&app.attachment_writes, bearer.0, app.now())?;
    let id = AttachmentId::generate();
    let now = app.now();
    let free = files::free(&app).await;
    if body.bytes == 0
        || body.bytes > 1_073_741_824
        || body.name.trim().is_empty()
        || body.name.chars().count() > 200
        || body.name.chars().any(|character| {
            character.is_control()
                || matches!(character,
                        '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' |
                        '\u{2060}'..='\u{206f}' | '\u{feff}' | '/' | '\\')
        })
        || body.sha256.len() != 64
        || !body
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ApiError::invalid(
            "invalid attachment name, size or SHA-256",
        ));
    }
    let upload = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let place = match (body.channel_id, body.conversation_id) {
            (Some(id), None) => Place::channel(connection, &access, id).await?,
            (None, Some(id)) => {
                Place::direct(connection, &access, id, messages::Purpose::Change).await?
            }
            _ => return Err(ApiError::invalid("give one channel or conversation")),
        };
        place.require_open()?;
        place.require_upload(&access)?;
        // An upload counts while it waits for its message and while its reservation
        // exists, so one account holds at most 20 reservations (ADR 0408).
        let count: i64 = sqlx::query_scalar(
            "select count(*)
                 from attachments a
                 where a.owner_id=$1
                 and not a.attached
                 and (a.removed_at is null or exists(
                     select 1 from storage_reservations r where r.subject_id=a.id))",
        )
        .bind(access.account_id)
        .fetch_one(&mut *connection)
        .await?;
        if count >= 20 {
            return Err(ApiError::limit_reached(
                "at most 20 pending uploads per account",
            ));
        }
        if body.bytes > files::limits(connection, &app).await?.bytes_per_file {
            return Err(ApiError::invalid("file exceeds the attachment size limit"));
        }
        files::root(&app)?;
        storage::reserve(
            connection,
            now,
            storage::Kind::Attachment,
            id.as_uuid(),
            body.bytes,
            free,
            app.storage_reserve_bytes(),
        )
        .await?;
        place.ensure(connection).await?;
        let expires = now + Duration::hours(24);
        sqlx::query(
            "insert into attachments(id,storage_id,place_id,owner_id,name,bytes,sha256,expires_at,touched_at)
             values($1,$1,$2,$3,$4,$5,$6,$7,$8)",
        )
        .bind(id)
        .bind(place.id)
        .bind(access.account_id)
        .bind(&body.name)
        .bind(body.bytes as i64)
        .bind(&body.sha256)
        .bind(expires)
        .bind(now)
        .execute(connection)
        .await?;
        Ok(Upload {
            id,
            received: 0,
            bytes: body.bytes,
            complete: false,
            expires_at: expires,
        })
    })?;
    Ok((StatusCode::CREATED, Json(upload)))
}
#[utoipa::path(get,path="/api/v1/attachments/{attachment_id}/upload",tag="messages",params(("attachment_id"=AttachmentId,Path)),security(("session"=[])),responses((status=200,body=Upload),ApiError))]
pub async fn get_upload(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<AttachmentId>,
) -> ApiResult<Json<Upload>> {
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        Ok(files::owned(connection, &access, id, app.now()).await?.1)
    })
    .map(Json)
}
#[utoipa::path(put,path="/api/v1/attachments/{attachment_id}/chunks/{offset}",tag="messages",params(("attachment_id"=AttachmentId,Path),("offset"=u64,Path)),request_body(content=inline(super::recordings::FileBytes),content_type="application/octet-stream"),security(("session"=[])),responses((status=200,body=Upload),ApiError))]
pub async fn put_chunk(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((id, offset)): ApiPath<(AttachmentId, u64)>,
    bytes: Bytes,
) -> ApiResult<Json<Upload>> {
    crate::proof::limit(&app.attachment_writes, bearer.0, app.now())?;
    if bytes.is_empty() || bytes.len() > files::CHUNK || offset > i64::MAX as u64 {
        return Err(ApiError::invalid("chunks contain 1 to 1048576 bytes"));
    }
    let account = read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        files::writable(connection, &access, id, app.now()).await?;
        Ok(access.account_id)
    })?;
    let permit = app.attachment_transfers.acquire(account).ok_or_else(|| {
        ApiError::new(ErrorCode::Busy, "attachment transfers are busy")
            .retry_after(Duration::seconds(1))
    })?;
    let check = app.attachment_checks.begin(id).ok_or_else(|| {
        ApiError::new(ErrorCode::Busy, "attachment upload is busy")
            .retry_after(Duration::seconds(1))
    })?;
    let digest = files::hash(&bytes);
    let now = app.now();
    crate::error::mark_possible_change();
    let already = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (_, upload) = files::writable(connection, &access, id, now).await?;
        let prior: Option<(i32, String, bool)> = sqlx::query_as(
            "select bytes,sha256,stored
                 from attachment_chunks
                 where attachment_id=$1
                 and offset_bytes=$2",
        )
        .bind(id)
        .bind(offset as i64)
        .fetch_optional(&mut *connection)
        .await?;
        if let Some((size, hash, stored)) = prior {
            if size as usize != bytes.len() || hash != digest {
                return Err(ApiError::conflict("different bytes at an uploaded offset"));
            }
            if stored {
                return Ok(true);
            }
        } else {
            if upload.complete
                || offset != upload.received
                || offset + bytes.len() as u64 > upload.bytes
                || (offset + (bytes.len() as u64) < upload.bytes && bytes.len() != files::CHUNK)
            {
                return Err(ApiError::conflict(
                    "send the next full chunk, or the final remaining bytes",
                ));
            }
            let free = files::free(&app)
                .await
                .ok_or_else(|| ApiError::new(ErrorCode::StorageLow, "free space cannot be read"))?;
            if storage::at_reserve(free, app.storage_reserve_bytes()) {
                return Err(ApiError::new(
                    ErrorCode::StorageLow,
                    "storage reserve reached",
                ));
            }
            sqlx::query("insert into attachment_chunks(attachment_id,offset_bytes,bytes,sha256) values($1,$2,$3,$4)")
                .bind(id)
                .bind(offset as i64)
                .bind(bytes.len() as i32)
                .bind(&digest)
                .execute(&mut *connection)
                .await
                .map_err(ApiError::concurrent)?;
        }
        sqlx::query("update attachments set touched_at=$2 where id=$1")
            .bind(id)
            .bind(now)
            .execute(connection)
            .await?;
        Ok(false)
    })?;
    if !already {
        let root = files::root(&app)?.to_path_buf();
        let content = bytes.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _check = check;
            crate::recordings::files::create_directory(&root, &files::directory(id))
                .map_err(files::io)?;
            let staging = files::directory(id).with(&format!(
                "staging-{}.partial",
                uuid::Uuid::new_v4().simple()
            ));
            crate::recordings::files::write_new(&root, &staging, &content).map_err(files::io)?;
            crate::recordings::files::finish(&root, &staging, &files::chunk(id, offset))
                .map_err(files::io)
        })
        .await
        .map_err(|error| ApiError::internal(&error))??;
    }
    let upload: ApiResult<Upload> = async {
        transact!(app, |connection| {
            let access = Access::open(connection, &bearer, app.now()).await?;
            files::writable(connection, &access, id, app.now()).await?;
            let changed = sqlx::query(
                "update attachment_chunks set stored=true
                 where attachment_id=$1
                 and offset_bytes=$2
                 and not stored",
            )
            .bind(id)
            .bind(offset as i64)
            .execute(&mut *connection)
            .await?
            .rows_affected();
            if changed > 0 {
                sqlx::query("update attachments set received=received+$2 where id=$1")
                    .bind(id)
                    .bind(bytes.len() as i64)
                    .execute(&mut *connection)
                    .await?;
            }
            Ok(files::owned(connection, &access, id, app.now()).await?.1)
        })
    }
    .await;
    upload.map_err(ApiError::after_partial_change).map(Json)
}
#[utoipa::path(post,path="/api/v1/attachments/{attachment_id}/complete",tag="messages",params(("attachment_id"=AttachmentId,Path)),security(("session"=[])),responses((status=200,body=Upload),ApiError))]
pub async fn complete_upload(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<AttachmentId>,
) -> ApiResult<Json<Upload>> {
    crate::proof::limit(&app.attachment_writes, bearer.0, app.now())?;
    let (parts, expected, upload, account) = read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        let (_, upload) = files::writable(connection, &access, id, app.now()).await?;
        if upload.received != upload.bytes {
            return Err(ApiError::conflict("upload is incomplete"));
        }
        let (hash, failed): (String, bool) =
            sqlx::query_as("select sha256,failed from attachments where id=$1")
                .bind(id)
                .fetch_one(&mut *connection)
                .await?;
        if failed {
            return Err(ApiError::invalid("the file does not match its SHA-256"));
        }
        Ok((
            files::chunks(connection, id).await?,
            hash,
            upload,
            access.account_id,
        ))
    })?;
    if upload.complete {
        return Ok(Json(upload));
    }
    let check = app.attachment_checks.begin(id).ok_or_else(|| {
        ApiError::new(ErrorCode::Busy, "attachment is being verified")
            .retry_after(Duration::seconds(1))
    })?;
    let permit = app.attachment_transfers.acquire(account).ok_or_else(|| {
        ApiError::new(ErrorCode::Busy, "attachment transfers are busy")
            .retry_after(Duration::seconds(1))
    })?;
    crate::error::mark_possible_change();
    tokio::spawn(async move {
        let _check = check;
        let root = files::root(&app)?.to_path_buf();
        let (hash, image) = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            files::check(root, id, parts)
        })
        .await
        .map_err(|error| ApiError::internal(&error))??;
        if hash != expected {
            transact!(app.background(), |connection| {
                sqlx::query("update attachments set failed=true where id=$1")
                    .bind(id)
                    .execute(&mut *connection)
                    .await?;
                storage::release(connection, id.as_uuid()).await?;
                Ok(())
            })?;
            return Err(ApiError::invalid("the file does not match its SHA-256"));
        }
        transact!(app.background(), |connection| {
            let access = Access::open(connection, &bearer, app.now()).await?;
            files::writable(connection, &access, id, app.now()).await?;
            let (media, width, height) =
                image.map_or(("application/octet-stream", None, None), |im| {
                    (
                        im.kind.media_type(),
                        Some(im.width as i32),
                        Some(im.height as i32),
                    )
                });
            sqlx::query(
                "update attachments set complete=true,media_type=$2,width=$3,height=$4
                     where id=$1",
            )
            .bind(id)
            .bind(media)
            .bind(width)
            .bind(height)
            .execute(&mut *connection)
            .await?;
            storage::release(connection, id.as_uuid()).await?;
            Ok(files::owned(connection, &access, id, app.now()).await?.1)
        })
    })
    .await
    .map_err(|error| ApiError::internal(&error))?
    .map(Json)
}
#[utoipa::path(delete,path="/api/v1/attachments/{attachment_id}/upload",tag="messages",params(("attachment_id"=AttachmentId,Path)),security(("session"=[])),responses((status=204),ApiError))]
pub async fn cancel_upload(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(id): ApiPath<AttachmentId>,
) -> ApiResult<StatusCode> {
    crate::proof::limit(&app.attachment_writes, bearer.0, app.now())?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        files::owned(connection, &access, id, app.now()).await?;
        sqlx::query("update attachments set removed_at=$2 where id=$1")
            .bind(id)
            .bind(app.now())
            .execute(&mut *connection)
            .await?;
        // No chunk is accepted for a cancelled upload, so it needs no reserved space.
        storage::release(connection, id.as_uuid()).await?;
        Ok(())
    })?;
    Ok(StatusCode::NO_CONTENT)
}
/// The row of an attachment that the caller may download now: the upload is complete and
/// belongs to a message that the caller reads.
async fn downloadable(
    app: &AppState,
    connection: &mut sqlx::PgConnection,
    access: &Access,
    id: AttachmentId,
) -> ApiResult<sqlx::postgres::PgRow> {
    let row = sqlx::query(
        "select message_id,bytes,media_type,sha256,storage_id
             from attachments
             where id=$1
             and attached
             and complete
             and removed_at is null",
    )
    .bind(id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(ApiError::not_found)?;
    let message: Option<MessageId> = row.try_get("message_id")?;
    messages::get(
        connection,
        access,
        message.ok_or_else(ApiError::not_found)?,
        messages::Purpose::Live(app.streams.live_since(
            access.account_id,
            access.device_id,
            access.session_id,
        )),
    )
    .await?;
    Ok(row)
}

/// The stored file of an attachment that the caller of a running download may still read.
async fn stored_decided_again(
    app: &AppState,
    bearer: Bearer,
    id: AttachmentId,
    now: time::OffsetDateTime,
) -> ApiResult<AttachmentId> {
    read!(app.background(), |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let row = downloadable(app, connection, &access, id).await?;
        Ok(row.try_get::<AttachmentId, _>("storage_id")?)
    })
}

/// Downloads the file, or one byte range. An outside range gets `range_not_satisfiable`.
#[utoipa::path(
    get,
    path = "/api/v1/attachments/{attachment_id}/file",
    tag = "messages",
    params(
        ("attachment_id" = AttachmentId, Path),
        ("Range" = Option<String>, Header, description = "One range of bytes")
    ),
    security(("session" = [])),
    responses(
        (status = 200, description = "The whole file.", content_type = "application/octet-stream", body = inline(super::recordings::FileBytes)),
        (status = 206, description = "The range stated by Content-Range.", content_type = "application/octet-stream", body = inline(super::recordings::FileBytes)),
        ApiError
    )
)]
pub async fn download(
    State(app): State<AppState>,
    headers: HeaderMap,
    bearer: Bearer,
    ApiPath(id): ApiPath<AttachmentId>,
) -> ApiResult<Response> {
    let now = app.now();
    let (parts, size, media, hash, account_id, storage_id) = read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let row = downloadable(&app, connection, &access, id).await?;
        Ok((
            files::chunks(connection, id).await?,
            row.try_get::<i64, _>("bytes")?,
            row.try_get::<String, _>("media_type")?,
            row.try_get::<String, _>("sha256")?,
            access.account_id,
            row.try_get::<AttachmentId, _>("storage_id")?,
        ))
    })?;
    let permit = app
        .attachment_downloads
        .acquire(account_id)
        .ok_or_else(|| {
            ApiError::new(ErrorCode::Busy, "attachment downloads are busy")
                .retry_after(Duration::seconds(1))
        })?;
    let root = files::root(&app)?.to_path_buf();
    let first = match crate::download::asked(&headers, size as u64) {
        crate::download::Asked::Range { first, .. } => first,
        _ => 0,
    };
    let parts: Vec<_> = parts
        .into_iter()
        .filter(|(offset, length, _)| (*offset + i64::from(*length)) as u64 > first)
        .collect();
    let source_offset = parts.first().map_or(0, |(offset, _, _)| *offset as u64);
    let stream = futures_util::stream::try_unfold(
        (parts.into_iter(), root),
        move |(mut parts, root)| async move {
            let Some((offset, length, expected)) = parts.next() else {
                return Ok::<_, std::io::Error>(None);
            };
            let directory = root.clone();
            let bytes = tokio::task::spawn_blocking(move || {
                use std::io::Read;
                let mut bytes = Vec::new();
                crate::recordings::files::open(
                    &directory,
                    &files::chunk(storage_id, offset as u64),
                )?
                .take(files::CHUNK as u64 + 1)
                .read_to_end(&mut bytes)?;
                if bytes.len() != length as usize || files::hash(&bytes) != expected {
                    return Err(std::io::Error::other("attachment integrity check failed"));
                }
                Ok(bytes)
            })
            .await
            .map_err(std::io::Error::other)??;
            Ok(Some((Bytes::from(bytes), (parts, root))))
        },
    );
    let stall = app
        .config
        .limits
        .recordings
        .download_stall
        .try_into()
        .map_err(|error| ApiError::internal(&error))?;
    // A download that runs ends when its caller may no longer read the message, or the
    // message no longer has this file.
    let still_allowed = crate::download::decided_again(&app, now, move |app, at| async move {
        stored_decided_again(&app, bearer, id, at).await.ok() == Some(storage_id)
    });
    let stream = crate::download::while_allowed(stream, still_allowed);
    let mut response = crate::download::streamed(
        stream,
        size as u64,
        source_offset,
        &headers,
        &media,
        permit,
        stall,
    )?;
    response.headers_mut().insert(
        header::ETAG,
        format!("\"{hash}\"")
            .parse()
            .map_err(|error| ApiError::internal(&error))?,
    );
    Ok(response)
}

#[utoipa::path(get,path="/api/v1/attachment-limits",tag="messages",security(("session"=[])),responses((status=200,body=junto_protocol::message::AttachmentLimits),ApiError))]
pub async fn get_limits(
    State(app): State<AppState>,
    bearer: Bearer,
) -> ApiResult<Json<junto_protocol::message::AttachmentLimits>> {
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        access.require(junto_authz::Request::OwnAccount)?;
        files::limits(connection, &app).await
    })
    .map(Json)
}
#[utoipa::path(put,path="/api/v1/attachment-limits",tag="messages",request_body=junto_protocol::message::AttachmentLimits,security(("session"=[])),responses((status=200,body=junto_protocol::message::AttachmentLimits),ApiError))]
pub async fn set_limits(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiJson(body): ApiJson<junto_protocol::message::AttachmentLimits>,
) -> ApiResult<Json<junto_protocol::message::AttachmentLimits>> {
    transact!(app,|connection| {
        let access = Access::open(connection, &bearer, app.now()).await?;
        access.require(junto_authz::Request::Server(
            junto_authz::ServerPermission::Administrator,
        ))?;
        if !(1..=1_073_741_824).contains(&body.bytes_per_file)
            || !(1..=100).contains(&body.files_per_message)
        {
            return Err(ApiError::invalid("invalid attachment limits"));
        }
        if files::limits(connection, &app).await? != body {
            sqlx::query("insert into attachment_limits(bytes_per_file,files_per_message)
             values($1,$2)
             on conflict(singleton) do update
             set bytes_per_file=excluded.bytes_per_file,files_per_message=excluded.files_per_message")
                 .bind(body.bytes_per_file as i64)
                 .bind(body.files_per_message as i32)
                 .execute(&mut *connection)
                 .await?;
            crate::audit::record(
                connection,
                app.now(),
                Some(access.account_id),
                crate::audit::About::Server,
                &junto_protocol::audit::AuditEvent::AttachmentLimitsChanged {
                    bytes_per_file: body.bytes_per_file,
                    files_per_message: body.files_per_message,
                },
            )
            .await?;
            crate::events::publish(
                connection,
                app.now(),
                &junto_protocol::events::Event::AttachmentLimitsChanged {
                    limits: body.clone(),
                },
            )
            .await?;
        }
        Ok(body.clone())
    }).map(Json)
}
