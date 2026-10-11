//! The routes of recordings (ADR 0601, ADR 0604). A route changes the server's record of a
//! recording in one transaction; the controller carries the change to the recorder.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use junto_authz::{Request, WorkspacePermission};
use junto_protocol::error::ErrorCode;
use junto_protocol::ids::{ChannelId, RecordingId, WorkspaceId};
use junto_protocol::recording::{
    Recording, RecordingDetail, RecordingPage, RecordingQuery, RecordingStopReason, StartRecording,
};
use junto_protocol::structure::ChannelKind;
use sqlx::PgConnection;

use crate::access::Access;
use crate::calls::entry::not_in_call;
use crate::calls::store as calls;
use crate::error::{ApiError, ApiResult};
use crate::extract::{ApiJson, ApiPath, ApiQuery, Bearer};
use crate::proof;
use crate::recordings::store::{self, NewRecording, Of};
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
        let row = store::recording(connection, Of::Id(recording_id), false)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let (record, scope) = access.channel(connection, row.channel_id).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ViewChannel,
        })?;
        if record.channel.hides_occupants && store::call_runs(connection, row.call_id).await? {
            let member = calls::member_of(connection, access.account_id, false).await?;
            if member.is_none_or(|member| member.call_id != row.call_id) {
                return Err(ApiError::not_found());
            }
        }
        let segments = store::segments(connection, recording_id).await?;
        Ok(RecordingDetail {
            recording: store::summary(connection, &row).await?,
            segments: segments.iter().map(store::SegmentRow::view).collect(),
        })
    })
    .map(Json)
}
