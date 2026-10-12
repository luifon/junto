//! The routes of recordings (ADR 0601, ADR 0604). A route changes the server's record of a
//! recording in one transaction; the controller carries the change to the recorder.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use junto_authz::{ChannelScope, Request, WorkspacePermission};
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{ChannelId, RecordingId, RecordingSegmentId, WorkspaceId};
use junto_protocol::recording::{
    Recording, RecordingDetail, RecordingPage, RecordingQuery, RecordingState, RecordingStopReason,
    StartRecording,
};
use junto_protocol::structure::ChannelKind;
use sqlx::PgConnection;

use crate::access::Access;
use crate::calls::entry::not_in_call;
use crate::calls::store as calls;
use crate::download::{self, Described};
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery, Bearer};
use crate::proof;
use crate::recordings::files::{self, Place};
use crate::recordings::store::{self, NewRecording, Of, RecordingRow};
use crate::routes::calls::{Voice, inside, voice};
use crate::state::AppState;
use crate::storage;
use crate::{read, transact};

const DEFAULT_PAGE: u32 = 50;
const LARGEST_PAGE: u32 = 200;

/// The language of a recording: `auto`, or a code of two or three lower-case letters, which
/// is the form that the transcription job accepts (ADR 0611).
fn language(body: &StartRecording) -> ApiResult<String> {
    let language = body.language.as_deref().unwrap_or("auto");
    let is_code =
        (2..=3).contains(&language.len()) && language.bytes().all(|byte| byte.is_ascii_lowercase());
    if language == "auto" || is_code {
        Ok(language.to_owned())
    } else {
        Err(ApiError::invalid(
            "language is \"auto\" or a language code of two or three lower-case letters",
        ))
    }
}

/// A voice channel in whose call the caller is, with the permission to record calls there.
/// Only a participant of a call starts or stops its recording (ADR 0601): the admission
/// check saw the seat of the caller in the room, and did not see it absent since.
async fn recorder_of(
    connection: &mut PgConnection,
    access: &Access,
    channel_id: ChannelId,
) -> ApiResult<(Voice, calls::CallRow)> {
    let voice = voice(&mut *connection, access, channel_id).await?;
    access.require(Request::Channel {
        scope: &voice.scope,
        permission: WorkspacePermission::RecordCalls,
    })?;
    let (call, member) = inside(connection, access, &voice).await?;
    if member.connected_at.is_none() || member.absent_since.is_some() {
        return Err(not_in_call());
    }
    Ok((voice, call))
}

/// Starts the recording of the call of a voice channel. The caller is in the call and holds
/// `record_calls`. The answer carries the recording in the state `starting`; the events
/// `recording_updated` tell whether the recorder took it. When the call has a recording that
/// is starting or runs, the answer carries that one and nothing changes.
#[utoipa::path(
    put,
    path = "/api/v1/channels/{channel_id}/call/recording",
    tag = "recordings",
    params(("channel_id" = ChannelId, Path)),
    request_body = StartRecording,
    security(("session" = [])),
    responses((status = 200, body = Recording), ApiError)
)]
pub async fn start_recording(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
    ApiJson(body): ApiJson<StartRecording>,
) -> ApiResult<Json<Recording>> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let language = language(&body)?;
    let new_id = RecordingId::generate();
    let limits = &app.config.limits.recordings;
    let free = app.recordings.free_bytes();
    let recording = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (voice, call) = recorder_of(connection, &access, channel_id).await?;
        let of_channel = Of::ActiveInChannel(channel_id);
        if let Some(active) = store::recording(connection, of_channel, false).await? {
            if active.stop.is_some() {
                return Err(ApiError::conflict(
                    "the recording of this call is being stopped",
                ));
            }
            return store::summary(connection, &active).await;
        }
        app.recordings.recording()?;
        app.calls.admitting()?;
        if store::running(connection).await? >= limits.most_concurrent {
            return Err(ApiError::new(
                ErrorCode::RecordingsFull,
                "as many recordings run as this server records at one time",
            )
            .retry_after(time::Duration::seconds(30)));
        }
        if free.is_some_and(|free| storage::at_reserve(free, limits.reserve_bytes)) {
            return Err(ApiError::new(
                ErrorCode::StorageLow,
                "the free disk space of the server is at its reserve",
            ));
        }
        storage::reserve(
            connection,
            now,
            storage::Kind::Recording,
            new_id.as_uuid(),
            limits.reservation_bytes,
            free,
            limits.reserve_bytes,
        )
        .await?;
        let new = NewRecording {
            id: new_id,
            workspace_id: voice.record.channel.workspace_id,
            channel_id,
            call_id: call.id,
            room: call.room.as_str(),
            started_by: access.account_id,
            language: &language,
        };
        let started = store::start(connection, now, new).await?;
        store::summary(connection, &started).await
    })?;
    app.recordings.poke();
    Ok(Json(recording))
}

/// Stops the recording of the call of a voice channel. The caller is in the call and holds
/// `record_calls`. The recorder ends every job, so that the files are complete; the recording
/// keeps the state `recording` until then. Without a recording nothing changes.
#[utoipa::path(
    delete,
    path = "/api/v1/channels/{channel_id}/call/recording",
    tag = "recordings",
    params(("channel_id" = ChannelId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn stop_recording(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(channel_id): ApiPath<ChannelId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        recorder_of(connection, &access, channel_id).await?;
        let of_channel = Of::ActiveInChannel(channel_id);
        let Some(mut active) = store::recording(connection, of_channel, true).await? else {
            return Ok(());
        };
        let (stopped, by) = (RecordingStopReason::Stopped, Some(access.account_id));
        if store::request_stop(connection, now, &mut active, stopped, by).await? {
            store::announce(connection, now, &active, false).await?;
        }
        Ok(())
    })?;
    app.recordings.poke();
    Ok(StatusCode::NO_CONTENT)
}

fn page_size(query: &RecordingQuery) -> ApiResult<i64> {
    match query.limit.unwrap_or(DEFAULT_PAGE) {
        limit @ 1..=LARGEST_PAGE => Ok(i64::from(limit)),
        _ => Err(ApiError::invalid(format!(
            "limit must be from 1 to {LARGEST_PAGE}"
        ))),
    }
}

/// The recordings of a workspace, newest first. A recording is returned when the caller may
/// see its voice channel. While its call runs in a channel that hides who is inside, it is
/// returned only to the people inside.
#[utoipa::path(
    get,
    path = "/api/v1/workspaces/{workspace_id}/recordings",
    tag = "recordings",
    params(("workspace_id" = WorkspaceId, Path), RecordingQuery),
    security(("session" = [])),
    responses((status = 200, body = RecordingPage), ApiError)
)]
pub async fn list_recordings(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(workspace_id): ApiPath<WorkspaceId>,
    ApiQuery(query): ApiQuery<RecordingQuery>,
) -> ApiResult<Json<RecordingPage>> {
    let size = page_size(&query)?;
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let scope = access.workspace(connection, workspace_id).await?;
        access.require(Request::ViewWorkspace(&scope))?;
        let visible: Vec<ChannelId> = access
            .workspace_channels(connection, workspace_id, &scope)
            .await?
            .into_iter()
            .filter(|(record, channel_scope)| {
                record.deleted_at.is_none()
                    && record.channel.kind == ChannelKind::Voice
                    && access.allows(Request::Channel {
                        scope: channel_scope,
                        permission: WorkspacePermission::ViewChannel,
                    })
            })
            .map(|(record, _)| record.channel.id)
            .collect();
        let viewer = access.account_id;
        let rows = store::page(
            connection,
            workspace_id,
            &visible,
            viewer,
            query.before,
            size + 1,
        )
        .await?;
        let shown = usize::try_from(size).unwrap_or(usize::MAX);
        let has_more = rows.len() > shown;
        let mut recordings = Vec::new();
        for row in rows.iter().take(shown) {
            recordings.push(store::summary(connection, row).await?);
        }
        let next_before = has_more
            .then(|| recordings.last().map(|recording| recording.id))
            .flatten();
        Ok(RecordingPage {
            recordings,
            next_before,
        })
    })
    .map(Json)
}

/// A recording that the caller may read, with the caller's standing in its voice channel
/// (ADR 0604): the caller sees the channel, and while the call of the recording runs in a
/// channel that hides who is inside, the caller is inside. With `for_change`, the row of
/// the recording is locked. Every route about a recording, its files and its transcript
/// uses this rule.
pub(crate) async fn readable(
    connection: &mut PgConnection,
    access: &Access,
    recording_id: RecordingId,
    for_change: bool,
) -> ApiResult<(RecordingRow, ChannelScope)> {
    let (row, scope, _) = read_by(connection, access, recording_id, for_change).await?;
    Ok((row, scope))
}

/// Like [`readable`], for a route that changes the recording or its transcript. The
/// recording of an archived channel takes no change (ADR 0410).
pub(crate) async fn changeable(
    connection: &mut PgConnection,
    access: &Access,
    recording_id: RecordingId,
    for_change: bool,
) -> ApiResult<(RecordingRow, ChannelScope)> {
    let (row, scope, archived) = read_by(connection, access, recording_id, for_change).await?;
    if archived {
        return Err(crate::access::archived());
    }
    Ok((row, scope))
}

async fn read_by(
    connection: &mut PgConnection,
    access: &Access,
    recording_id: RecordingId,
    for_change: bool,
) -> ApiResult<(RecordingRow, ChannelScope, bool)> {
    let row = store::recording(&mut *connection, Of::Id(recording_id), false)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (record, scope) = access.channel(&mut *connection, row.channel_id).await?;
    access.require(Request::Channel {
        scope: &scope,
        permission: WorkspacePermission::ViewChannel,
    })?;
    if !store::reads_call_of(&mut *connection, row.call_id, access.account_id).await? {
        return Err(ApiError::not_found());
    }
    let archived = record.archived_at.is_some();
    if !for_change {
        return Ok((row, scope, archived));
    }
    let locked = store::recording(connection, Of::Id(recording_id), true)
        .await?
        .ok_or_else(ApiError::not_found)?;
    Ok((locked, scope, archived))
}

/// One recording with its segments. The caller may see its voice channel. While the call of
/// the recording runs in a channel that hides who is inside, only the people inside read it.
#[utoipa::path(
    get,
    path = "/api/v1/recordings/{recording_id}",
    tag = "recordings",
    params(("recording_id" = RecordingId, Path)),
    security(("session" = [])),
    responses((status = 200, body = RecordingDetail), ApiError)
)]
pub async fn get_recording(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(recording_id): ApiPath<RecordingId>,
) -> ApiResult<Json<RecordingDetail>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (row, _) = readable(connection, &access, recording_id, false).await?;
        let segments = store::segments(connection, recording_id).await?;
        Ok(RecordingDetail {
            recording: store::summary(connection, &row).await?,
            segments: segments.iter().map(store::SegmentRow::view).collect(),
        })
    })
    .map(Json)
}

/// Removes a recording with its files and its transcript. The caller holds
/// `manage_recordings` in its voice channel. A recording that is starting, runs or is
/// processed is not removed: its stop is asked for first.
#[utoipa::path(
    delete,
    path = "/api/v1/recordings/{recording_id}",
    tag = "recordings",
    params(("recording_id" = RecordingId, Path)),
    security(("session" = [])),
    responses((status = 204), ApiError)
)]
pub async fn remove_recording(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(recording_id): ApiPath<RecordingId>,
) -> ApiResult<StatusCode> {
    let now = app.now();
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (recording, scope) = changeable(connection, &access, recording_id, true).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ManageRecordings,
        })?;
        if recording.state.is_active() || recording.state == RecordingState::Processing {
            return Err(ApiError::conflict(
                "the recording runs or its files are processed; it can be removed afterwards",
            ));
        }
        store::remove(connection, now, &recording, Some(access.account_id)).await
    })?;
    app.recordings.process_now();
    Ok(StatusCode::NO_CONTENT)
}

/// The bytes of a file as the whole body of a response.
pub struct FileBytes;

impl utoipa::PartialSchema for FileBytes {
    fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
        use utoipa::openapi::schema::{KnownFormat, ObjectBuilder, SchemaFormat, Type};
        ObjectBuilder::new()
            .schema_type(Type::String)
            .format(Some(SchemaFormat::KnownFormat(KnownFormat::Binary)))
            .into()
    }
}

impl utoipa::ToSchema for FileBytes {}

/// The file of one segment of a recording: the video of a composite as MP4, or an audio
/// track as Ogg with Opus. The caller may read the recording. A request with the header
/// `Range: bytes=<first>-<last>` gets that part of the file with the status 206; a range
/// outside the file gets `range_not_satisfiable`.
#[utoipa::path(
    get,
    path = "/api/v1/recordings/{recording_id}/segments/{segment_id}/file",
    tag = "recordings",
    params(
        ("recording_id" = RecordingId, Path),
        ("segment_id" = RecordingSegmentId, Path),
        ("Range" = Option<String>, Header, description = "One range of bytes")
    ),
    security(("session" = [])),
    responses(
        (status = 200, description = "The whole file.", content(
            (inline(FileBytes) = "video/mp4"),
            (inline(FileBytes) = "audio/ogg")
        )),
        (status = 206, description = "The range of the file that `Content-Range` states.", content(
            (inline(FileBytes) = "video/mp4"),
            (inline(FileBytes) = "audio/ogg")
        )),
        ApiError
    )
)]
pub async fn get_segment_file(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((recording_id, segment_id)): ApiPath<(RecordingId, RecordingSegmentId)>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    let now = app.now();
    let (account_id, is_track) = read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let is_track = file_of(connection, &access, recording_id, segment_id).await?;
        Ok((access.account_id, is_track))
    })?;
    let backend = app.recordings.backend().ok_or_else(ApiError::not_found)?;
    let place = app.recordings.download(account_id).ok_or_else(|| {
        ApiError::new(
            ErrorCode::Busy,
            "the server sends as many files as it sends at one time; send the request again",
        )
        .retry_after(time::Duration::seconds(1))
    })?;
    let (file_place, media_type, extension) = if is_track {
        (Place::track(recording_id, segment_id), "audio/ogg", "ogg")
    } else {
        (Place::video(recording_id, segment_id), "video/mp4", "mp4")
    };
    let root = backend.settings.directory.clone();
    let opened = tokio::task::spawn_blocking(move || files::open(&root, &file_place))
        .await
        .map_err(|error| ApiError::internal(&error))?;
    // A file that is gone, a link in its place and a file that cannot be opened give the
    // same answer: no file of the recording is at that place.
    let file = opened.map_err(|error| {
        if !files::is_absent(&error) {
            tracing::warn!(%error, segment = %segment_id, "a file of a recording cannot be opened");
        }
        ApiError::not_found()
    })?;
    let file_name = format!("{segment_id}.{extension}");
    let described = Described {
        media_type,
        file_name: &file_name,
    };
    let stall = app.config.limits.recordings.download_stall.unsigned_abs();
    // A download that runs ends when its caller may no longer read the recording, or the
    // segment no longer has this file.
    let still_allowed = download::decided_again(&app, now, move |app, at| async move {
        let decided = file_decided_again(&app, bearer, recording_id, segment_id, at).await;
        decided.ok() == Some(is_track)
    });
    download::send_while(file, &headers, described, place, stall, still_allowed).await
}

async fn file_decided_again(
    app: &AppState,
    bearer: Bearer,
    recording_id: RecordingId,
    segment_id: RecordingSegmentId,
    now: time::OffsetDateTime,
) -> ApiResult<bool> {
    read!(app.background(), |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        file_of(connection, &access, recording_id, segment_id).await
    })
}

/// The file of a segment that the caller may download now: true for an audio track, false
/// for the video of a composite. A recording that the caller may not read, a segment that
/// the recording does not have and a segment without a file are not found.
async fn file_of(
    connection: &mut PgConnection,
    access: &Access,
    recording_id: RecordingId,
    segment_id: RecordingSegmentId,
) -> ApiResult<bool> {
    readable(connection, access, recording_id, false).await?;
    store::file_is_track(connection, recording_id, segment_id)
        .await?
        .ok_or_else(ApiError::not_found)
}
