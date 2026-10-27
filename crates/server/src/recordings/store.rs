//! The rows of recordings and of their segments. Every function works inside the transaction
//! of its caller.

use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{
    AccountId, CallId, ChannelId, RecordingId, RecordingSegmentId, WorkspaceId,
};
use junto_protocol::recording::{
    FileCheck, FileProblem, Recording, RecordingFailure, RecordingSegment, RecordingState,
    RecordingStop, RecordingStopReason, SegmentFailure, SegmentFile, SegmentSource, SegmentState,
};
use junto_protocol::transcript::TranscriptState;
use sqlx::{AssertSqlSafe, PgConnection};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::audit::{self, About};
use crate::db::{self, StoredText};
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::recorder::FileResult;
use crate::storage;

macro_rules! stored {
    ($type:ty { $($variant:ident => $text:literal,)+ }) => {
        impl StoredText for $type {
            fn stored(self) -> &'static str {
                match self {
                    $(Self::$variant => $text,)+
                    #[allow(unreachable_patterns)]
                    _ => "unknown",
                }
            }

            fn from_stored(text: &str) -> Option<Self> {
                match text {
                    $($text => Some(Self::$variant),)+
                    _ => None,
                }
            }
        }
    };
}

stored!(RecordingState {
    Starting => "starting",
    Recording => "recording",
    Processing => "processing",
    Ready => "ready",
    PartlyFailed => "partly_failed",
    Failed => "failed",
});

stored!(RecordingStopReason {
    Stopped => "stopped",
    CallEnded => "call_ended",
    StorageReserve => "storage_reserve",
    SegmentLimit => "segment_limit",
    RecorderLost => "recorder_lost",
});

stored!(RecordingFailure {
    RecorderUnavailable => "recorder_unavailable",
    NothingRecorded => "nothing_recorded",
    FilesMissing => "files_missing",
});

stored!(FileProblem {
    Unreadable => "unreadable",
    StreamMissing => "stream_missing",
    StreamShort => "stream_short",
    TimestampHole => "timestamp_hole",
    SoundMissing => "sound_missing",
    PictureBlank => "picture_blank",
    LengthMismatch => "length_mismatch",
    NotChecked => "not_checked",
});

stored!(TranscriptState {
    None => "none",
    Waiting => "waiting",
    Running => "running",
    Ready => "ready",
    Partial => "partial",
    Failed => "failed",
});

stored!(SegmentSource {
    Composite => "composite",
    Microphone => "microphone",
    ScreenShareAudio => "screen_share_audio",
});

stored!(SegmentState {
    Starting => "starting",
    Recording => "recording",
    Ended => "ended",
    Failed => "failed",
});

stored!(SegmentFailure {
    RecorderUnavailable => "recorder_unavailable",
    StartRefused => "start_refused",
    StartInterrupted => "start_interrupted",
    RecorderLost => "recorder_lost",
    RecorderError => "recorder_error",
});

#[derive(Debug, Clone)]
pub struct RecordingRow {
    pub id: RecordingId,
    pub workspace_id: WorkspaceId,
    pub channel_id: ChannelId,
    pub call_id: CallId,
    pub room: String,
    pub state: RecordingState,
    pub started_by: AccountId,
    pub language: String,
    pub requested_at: OffsetDateTime,
    pub recording_since: Option<OffsetDateTime>,
    pub stop: Option<RecordingStop>,
    pub ended_at: Option<OffsetDateTime>,
    pub failure: Option<RecordingFailure>,
    /// When the files were processed. Absent before.
    pub processed_at: Option<OffsetDateTime>,
    pub timeline_started_at: Option<OffsetDateTime>,
}

impl RecordingRow {
    fn about(&self) -> About {
        About::Channel(self.workspace_id, self.channel_id)
    }
}

#[derive(sqlx::FromRow)]
struct RecordingRecord {
    id: RecordingId,
    workspace_id: WorkspaceId,
    channel_id: ChannelId,
    call_id: CallId,
    room_name: String,
    state: String,
    started_by: AccountId,
    language: String,
    requested_at: OffsetDateTime,
    recording_since: Option<OffsetDateTime>,
    stop_reason: Option<String>,
    stopped_by: Option<AccountId>,
    stop_requested_at: Option<OffsetDateTime>,
    ended_at: Option<OffsetDateTime>,
    failure: Option<String>,
    processed_at: Option<OffsetDateTime>,
    timeline_started_at: Option<OffsetDateTime>,
}

const COLUMNS: &str = "id, workspace_id, channel_id, call_id, room_name, state, started_by, \
     language, requested_at, recording_since, stop_reason, stopped_by, stop_requested_at, \
     ended_at, failure, processed_at, timeline_started_at";

fn row(record: RecordingRecord) -> ApiResult<RecordingRow> {
    let stop = match (record.stop_reason, record.stop_requested_at) {
        (Some(reason), Some(at)) => Some(RecordingStop {
            reason: RecordingStopReason::read(&reason)?,
            by: record.stopped_by,
            at,
        }),
        _ => None,
    };
    Ok(RecordingRow {
        id: record.id,
        workspace_id: record.workspace_id,
        channel_id: record.channel_id,
        call_id: record.call_id,
        room: record.room_name,
        state: RecordingState::read(&record.state)?,
        started_by: record.started_by,
        language: record.language,
        requested_at: record.requested_at,
        recording_since: record.recording_since,
        stop,
        ended_at: record.ended_at,
        failure: record
            .failure
            .as_deref()
            .map(RecordingFailure::read)
            .transpose()?,
        processed_at: record.processed_at,
        timeline_started_at: record.timeline_started_at,
    })
}

/// Which recording a read names.
#[derive(Debug, Clone, Copy)]
pub enum Of {
    Id(RecordingId),
    /// The recording of a voice channel that is starting or runs.
    ActiveInChannel(ChannelId),
    /// The recording of a call that is starting or runs.
    ActiveOfCall(CallId),
}

const ACTIVE: &str = "state in ('starting', 'recording')";

/// Reads one recording. With `for_change`, the row is locked and its revision grows, so that
/// two transactions that change the same recording never both commit from the same state.
pub async fn recording(
    connection: &mut PgConnection,
    of: Of,
    for_change: bool,
) -> ApiResult<Option<RecordingRow>> {
    let (filter, id) = match of {
        Of::Id(id) => ("id = $1".to_owned(), id.as_uuid()),
        Of::ActiveInChannel(id) => (format!("channel_id = $1 and {ACTIVE}"), id.as_uuid()),
        Of::ActiveOfCall(id) => (format!("call_id = $1 and {ACTIVE}"), id.as_uuid()),
    };
    let statement = if for_change {
        format!("update recordings set revision = revision + 1 where {filter} returning {COLUMNS}")
    } else {
        format!("select {COLUMNS} from recordings where {filter}")
    };
    let record: Option<RecordingRecord> = sqlx::query_as(AssertSqlSafe(statement))
        .bind(id)
        .fetch_optional(connection)
        .await?;
    record.map(row).transpose()
}

/// The identifiers of the recordings that are starting or run, oldest first.
pub async fn active(connection: &mut PgConnection) -> ApiResult<Vec<RecordingId>> {
    Ok(sqlx::query_scalar(AssertSqlSafe(format!(
        "select id from recordings where {ACTIVE} order by id"
    )))
    .fetch_all(connection)
    .await?)
}

/// The recordings of a workspace in the channels of `channel_ids`, newest first, as far as
/// `viewer` may see each: while the call of a recording runs in a channel that hides who is
/// inside, the recording is shown only to the people inside (ADR 0514).
pub async fn page(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    channel_ids: &[ChannelId],
    viewer: AccountId,
    before: Option<RecordingId>,
    limit: i64,
) -> ApiResult<Vec<RecordingRow>> {
    let records: Vec<RecordingRecord> = sqlx::query_as(AssertSqlSafe(format!(
        "select {COLUMNS} from recordings r
         where r.workspace_id = $1 and r.channel_id = any($2)
           and ($3::uuid is null or r.id < $3)
           and reads_call(r.call_id, $4)
         order by r.id desc
         limit $5"
    )))
    .bind(workspace_id)
    .bind(channel_ids)
    .bind(before)
    .bind(viewer)
    .bind(limit)
    .fetch_all(connection)
    .await?;
    records.into_iter().map(row).collect()
}

/// The recording as events and lists carry it: with the numbers of its segments.
pub async fn summary(
    connection: &mut PgConnection,
    recording: &RecordingRow,
) -> ApiResult<Recording> {
    let (segments, failed, bytes): (i64, i64, i64) = sqlx::query_as(
        "select count(*), count(*) filter (where state = 'failed'),
                coalesce(sum(file_bytes), 0)::bigint
         from recording_segments where recording_id = $1",
    )
    .bind(recording.id)
    .fetch_one(&mut *connection)
    .await?;
    let transcript: Option<(String, i32)> =
        sqlx::query_as("select state, lines from transcripts where recording_id = $1")
            .bind(recording.id)
            .fetch_optional(connection)
            .await?;
    let (transcript, transcript_lines) = match transcript {
        Some((state, lines)) => (TranscriptState::read(&state)?, db::count(lines)?),
        None => (TranscriptState::None, 0),
    };
    let number = |count: i64| u32::try_from(count).map_err(|error| ApiError::internal(&error));
    Ok(Recording {
        id: recording.id,
        workspace_id: recording.workspace_id,
        channel_id: recording.channel_id,
        call_id: recording.call_id,
        state: recording.state,
        started_by: recording.started_by,
        requested_at: recording.requested_at,
        recording_since: recording.recording_since,
        stop: recording.stop,
        ended_at: recording.ended_at,
        failure: recording.failure,
        language: recording.language.clone(),
        segments: number(segments)?,
        failed_segments: number(failed)?,
        bytes: recording
            .processed_at
            .map(|_| u64::try_from(bytes).unwrap_or(0)),
        timeline_started_at: recording.timeline_started_at,
        transcript,
        transcript_lines,
    })
}

/// The recording of a call that is starting or runs, for the view of the call.
pub async fn active_of_call(
    connection: &mut PgConnection,
    call_id: CallId,
) -> ApiResult<Option<Recording>> {
    match recording(&mut *connection, Of::ActiveOfCall(call_id), false).await? {
        Some(row) => Ok(Some(summary(connection, &row).await?)),
        None => Ok(None),
    }
}

/// Whether the call of a recording still runs. Until it has ended, the recording is shown
/// only to the accounts that may see who is inside (ADR 0604).
pub async fn call_runs(connection: &mut PgConnection, call_id: CallId) -> ApiResult<bool> {
    Ok(
        sqlx::query_scalar("select exists (select 1 from calls where id = $1)")
            .bind(call_id)
            .fetch_one(connection)
            .await?,
    )
}

/// The call of a recording while it still runs, for the audience of an event about the
/// recording.
pub async fn running_call(
    connection: &mut PgConnection,
    call_id: CallId,
) -> ApiResult<Option<CallId>> {
    Ok(call_runs(connection, call_id).await?.then_some(call_id))
}

/// Whether an account may read a recording of a voice channel that it sees: the call of
/// the recording has ended, or does not hide who is inside, or the account is inside it
/// (ADR 0517, ADR 0604).
pub async fn reads_call_of(
    connection: &mut PgConnection,
    call_id: CallId,
    account_id: AccountId,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar("select reads_call($1, $2)")
        .bind(call_id)
        .bind(account_id)
        .fetch_one(connection)
        .await?)
}

/// Records the event of a recording for the accounts that may see it now (ADR 0604).
pub async fn announce(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording: &RecordingRow,
    started: bool,
) -> ApiResult<()> {
    let running = running_call(&mut *connection, recording.call_id).await?;
    announce_to(connection, now, recording, started, running).await
}

async fn announce_to(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording: &RecordingRow,
    started: bool,
    running_call: Option<CallId>,
) -> ApiResult<()> {
    let recording = Box::new(summary(&mut *connection, recording).await?);
    let event = if started {
        Event::RecordingStarted { recording }
    } else {
        Event::RecordingUpdated { recording }
    };
    events::tell_of_recording(connection, now, running_call, &event).await
}

/// Keeps what happened to a recording for the system lines of the text chat of its voice
/// channel (ADR 0501).
async fn note(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording: &RecordingRow,
    kind: &'static str,
    account_id: Option<AccountId>,
) -> ApiResult<()> {
    let message_id = junto_protocol::ids::MessageId::generate();
    sqlx::query(
        "insert into call_notes
             (id, occurred_at, workspace_id, channel_id, call_id, kind, account_id, recording_id)
         values ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(message_id)
    .bind(now)
    .bind(recording.workspace_id)
    .bind(recording.channel_id)
    .bind(recording.call_id)
    .bind(kind)
    .bind(account_id)
    .bind(recording.id)
    .execute(&mut *connection)
    .await?;
    crate::messages::system(
        connection,
        now,
        recording.channel_id,
        message_id,
        &junto_protocol::message::SystemMessage {
            action: kind.to_owned(),
            actor_id: account_id,
            recording_id: Some(recording.id),
            duration_seconds: None,
            subject_id: None,
        },
        Some(recording.call_id),
    )
    .await?;
    Ok(())
}

/// Deletes the notes of one kind of a recording with their lines in the text chat, and
/// tells the readers of each line that it is gone.
async fn remove_notes(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording: &RecordingRow,
    kind: &'static str,
) -> ApiResult<()> {
    let lines: Vec<(
        junto_protocol::ids::MessageId,
        i64,
        Option<Vec<AccountId>>,
        OffsetDateTime,
    )> = sqlx::query_as(
        "select id, position, readers, created_at from messages
         where recording_id = $1 and system->>'action' = $2",
    )
    .bind(recording.id)
    .bind(kind)
    .fetch_all(&mut *connection)
    .await?;
    for (id, position, readers, created_at) in lines {
        crate::messages::erase(connection, now, id).await?;
        events::log::record(
            connection,
            now,
            &events::audience::Audience::SystemMessage {
                workspace_id: recording.workspace_id,
                channel_id: recording.channel_id,
                call_id: recording.call_id,
                readers,
                created_at: Some(created_at),
            },
            &Event::MessageChanged {
                place: junto_protocol::message::MessagePlace::Channel {
                    workspace_id: recording.workspace_id,
                    channel_id: recording.channel_id,
                },
                message_id: id,
                position: position as u64,
                deleted: true,
            },
        )
        .await?;
    }
    sqlx::query("delete from call_notes where recording_id = $1 and kind = $2")
        .bind(recording.id)
        .bind(kind)
        .execute(connection)
        .await?;
    Ok(())
}

/// No segment of the recording has a failure any more, because a file result replaced the
/// declaration of a lost job (ADR 0604): the note of a failure goes, with its line.
pub async fn failure_replaced(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording: &RecordingRow,
) -> ApiResult<()> {
    remove_notes(connection, now, recording, "recording_failed").await
}

/// Keeps one note that a part of the recording failed. A recording has at most one.
pub async fn note_failure(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording: &RecordingRow,
) -> ApiResult<()> {
    let noted: bool = sqlx::query_scalar(
        "select exists (
             select 1 from call_notes where recording_id = $1 and kind = 'recording_failed')",
    )
    .bind(recording.id)
    .fetch_one(&mut *connection)
    .await?;
    if noted {
        return Ok(());
    }
    note(connection, now, recording, "recording_failed", None).await
}

/// What a request to start a recording states.
pub struct NewRecording<'a> {
    pub id: RecordingId,
    pub workspace_id: WorkspaceId,
    pub channel_id: ChannelId,
    pub call_id: CallId,
    pub room: &'a str,
    pub started_by: AccountId,
    pub language: &'a str,
}

/// Stores a recording in the state `starting`, with its audit entry, its note and its event.
pub async fn start(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    new: NewRecording<'_>,
) -> ApiResult<RecordingRow> {
    sqlx::query(
        "insert into recordings
             (id, workspace_id, channel_id, call_id, room_name, state, started_by, language,
              requested_at)
         values ($1, $2, $3, $4, $5, 'starting', $6, $7, $8)",
    )
    .bind(new.id)
    .bind(new.workspace_id)
    .bind(new.channel_id)
    .bind(new.call_id)
    .bind(new.room)
    .bind(new.started_by)
    .bind(new.language)
    .bind(now)
    .execute(&mut *connection)
    .await
    .map_err(ApiError::concurrent)?;
    let recording = RecordingRow {
        id: new.id,
        workspace_id: new.workspace_id,
        channel_id: new.channel_id,
        call_id: new.call_id,
        room: new.room.to_owned(),
        state: RecordingState::Starting,
        started_by: new.started_by,
        language: new.language.to_owned(),
        requested_at: now,
        recording_since: None,
        stop: None,
        ended_at: None,
        failure: None,
        processed_at: None,
        timeline_started_at: None,
    };
    let entry = AuditEvent::RecordingStarted {
        recording_id: new.id,
    };
    let actor = Some(new.started_by);
    audit::record(&mut *connection, now, actor, recording.about(), &entry).await?;
    note(
        &mut *connection,
        now,
        &recording,
        "recording_started",
        actor,
    )
    .await?;
    announce(connection, now, &recording, true).await?;
    Ok(recording)
}

/// Asks for the stop of a recording: the controller ends its jobs. Returns false, and changes
/// nothing, when a stop was asked for before. The caller records the event.
pub async fn request_stop(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording: &mut RecordingRow,
    reason: RecordingStopReason,
    by: Option<AccountId>,
) -> ApiResult<bool> {
    if recording.stop.is_some() {
        return Ok(false);
    }
    sqlx::query(
        "update recordings set stop_reason = $2, stopped_by = $3, stop_requested_at = $4
         where id = $1",
    )
    .bind(recording.id)
    .bind(reason.stored())
    .bind(by)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    recording.stop = Some(RecordingStop {
        reason,
        by,
        at: now,
    });
    let entry = AuditEvent::RecordingStopped {
        recording_id: recording.id,
        reason,
    };
    audit::record(&mut *connection, now, by, recording.about(), &entry).await?;
    note(connection, now, recording, "recording_stopped", by).await?;
    Ok(true)
}

/// The most recordings of one call that one change tells the people outside of. A call with
/// more leaves the others to the list of recordings.
const MOST_TOLD_AT_ONCE: i64 = 50;

/// The newest recordings of a call, at most `most`, oldest first. With `ended_only`, without
/// the one that is starting or runs.
async fn of_call(
    connection: &mut PgConnection,
    call_id: CallId,
    ended_only: bool,
    most: i64,
) -> ApiResult<Vec<RecordingRow>> {
    let records: Vec<RecordingRecord> = sqlx::query_as(AssertSqlSafe(format!(
        "select {COLUMNS} from recordings
         where call_id = $1 and (not $2 or state not in ('starting', 'recording'))
         order by id desc limit $3"
    )))
    .bind(call_id)
    .bind(ended_only)
    .bind(most)
    .fetch_all(connection)
    .await?;
    records.into_iter().rev().map(row).collect()
}

/// What the end of a call does to its recordings, in the transaction that ends the call
/// (ADR 0604). The stop of the recording that is starting or runs is asked for. The
/// recordings of a call that hid who is inside were shown to the people inside only; from
/// now on the accounts that may see the channel read them, and are told of them.
pub async fn call_ended(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    call_id: CallId,
    hides: bool,
) -> ApiResult<()> {
    let active = recording(&mut *connection, Of::ActiveOfCall(call_id), true).await?;
    if hides {
        let most = MOST_TOLD_AT_ONCE - i64::from(active.is_some());
        for ended in of_call(&mut *connection, call_id, true, most).await? {
            announce_to(&mut *connection, now, &ended, false, None).await?;
        }
    }
    let Some(mut active) = active else {
        return Ok(());
    };
    let ended = RecordingStopReason::CallEnded;
    let stopped = request_stop(&mut *connection, now, &mut active, ended, None).await?;
    if stopped || hides {
        announce_to(connection, now, &active, false, None).await?;
    }
    Ok(())
}

/// The first job of a recording runs.
pub async fn began(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording: &mut RecordingRow,
) -> ApiResult<()> {
    sqlx::query("update recordings set state = 'recording', recording_since = $2 where id = $1")
        .bind(recording.id)
        .bind(now)
        .execute(connection)
        .await?;
    recording.state = RecordingState::Recording;
    recording.recording_since = Some(now);
    Ok(())
}

/// Every job of a recording has ended: the recording leaves the active states. A recording of
/// which nothing was recorded fails, which ends its reservation of disk space.
pub async fn finish(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording: &mut RecordingRow,
    failure: Option<RecordingFailure>,
) -> ApiResult<()> {
    let state = match failure {
        Some(_) => RecordingState::Failed,
        None => RecordingState::Processing,
    };
    sqlx::query("update recordings set state = $2, failure = $3, ended_at = $4 where id = $1")
        .bind(recording.id)
        .bind(state.stored())
        .bind(failure.map(StoredText::stored))
        .bind(now)
        .execute(&mut *connection)
        .await?;
    recording.state = state;
    recording.failure = failure;
    recording.ended_at = Some(now);
    if failure.is_some() {
        storage::release(&mut *connection, recording.id.as_uuid()).await?;
        note_failure(connection, now, recording).await?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct SegmentRow {
    pub id: RecordingSegmentId,
    pub source: SegmentSource,
    pub account_id: Option<AccountId>,
    pub track_id: Option<String>,
    pub state: SegmentState,
    pub failure: Option<SegmentFailure>,
    pub job_id: Option<String>,
    pub requested_path: String,
    pub requested_at: OffsetDateTime,
    pub job_started_at: Option<OffsetDateTime>,
    pub stop_delivered_at: Option<OffsetDateTime>,
    /// The job was declared lost before the recorder took a stop for it, and no stop that
    /// followed the declaration has had an end yet (ADR 0604).
    pub stop_owed: bool,
    pub started_at: Option<OffsetDateTime>,
    pub ended_at: Option<OffsetDateTime>,
    pub duration_ms: Option<i64>,
    /// What the processing found of the file. Absent before, and without a file.
    pub file: Option<FileRow>,
}

/// The file of a segment as the processing found and checked it (ADR 0606).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRow {
    pub bytes: u64,
    pub media_ms: i64,
    /// The first sample of the file on the timeline.
    pub starts_at: Option<OffsetDateTime>,
    pub size: Option<(u32, u32)>,
    pub problems: Vec<FileProblem>,
    pub silent: bool,
    pub recovered: bool,
}

impl FileRow {
    fn view(&self, source: SegmentSource) -> SegmentFile {
        let media_type = match source {
            SegmentSource::Composite => "video/mp4",
            _ => "audio/ogg",
        };
        SegmentFile {
            media_type: media_type.to_owned(),
            bytes: self.bytes,
            duration_ms: u64::try_from(self.media_ms).unwrap_or(0),
            starts_at: self.starts_at,
            width: self.size.map(|(width, _)| width),
            height: self.size.map(|(_, height)| height),
            check: if self.problems.is_empty() {
                FileCheck::Passed
            } else {
                FileCheck::Failed
            },
            problems: self.problems.clone(),
            silent: self.silent,
            recovered: self.recovered,
        }
    }
}

impl SegmentRow {
    pub fn view(&self) -> RecordingSegment {
        RecordingSegment {
            id: self.id,
            source: self.source,
            account_id: self.account_id,
            state: self.state,
            failure: self.failure,
            requested_at: self.requested_at,
            started_at: self.started_at,
            ended_at: self.ended_at,
            duration_ms: self
                .duration_ms
                .and_then(|length| u64::try_from(length).ok()),
            file: self.file.as_ref().map(|file| file.view(self.source)),
        }
    }

    /// True for the audio track of a person, false for the composite.
    pub fn is_track(&self) -> bool {
        self.source != SegmentSource::Composite
    }
}

#[derive(sqlx::FromRow)]
struct SegmentRecord {
    id: RecordingSegmentId,
    source: String,
    account_id: Option<AccountId>,
    track_id: Option<String>,
    state: String,
    failure: Option<String>,
    job_id: Option<String>,
    requested_path: String,
    requested_at: OffsetDateTime,
    job_started_at: Option<OffsetDateTime>,
    stop_delivered_at: Option<OffsetDateTime>,
    stop_owed: bool,
    started_at: Option<OffsetDateTime>,
    ended_at: Option<OffsetDateTime>,
    duration_ms: Option<i64>,
    file_bytes: Option<i64>,
    media_ms: Option<i64>,
    file_starts_at: Option<OffsetDateTime>,
    width: Option<i32>,
    height: Option<i32>,
    file_problems: Vec<String>,
    file_silent: bool,
    file_recovered: bool,
}

fn file_row(record: &SegmentRecord) -> ApiResult<Option<FileRow>> {
    let (Some(bytes), Some(media_ms)) = (record.file_bytes, record.media_ms) else {
        return Ok(None);
    };
    let side = |side: Option<i32>| side.and_then(|side| u32::try_from(side).ok());
    Ok(Some(FileRow {
        bytes: u64::try_from(bytes).unwrap_or(0),
        media_ms,
        starts_at: record.file_starts_at,
        size: side(record.width).zip(side(record.height)),
        problems: record
            .file_problems
            .iter()
            .map(|problem| FileProblem::read(problem))
            .collect::<ApiResult<_>>()?,
        silent: record.file_silent,
        recovered: record.file_recovered,
    }))
}

/// The segments of a recording, in the order in which they were asked for.
pub async fn segments(
    connection: &mut PgConnection,
    recording_id: RecordingId,
) -> ApiResult<Vec<SegmentRow>> {
    let records: Vec<SegmentRecord> = sqlx::query_as(
        "select id, source, account_id, track_id, state, failure, job_id, requested_path,
                requested_at, job_started_at, stop_delivered_at, stop_owed, started_at, ended_at,
                duration_ms, file_bytes, media_ms, file_starts_at, width, height, file_problems,
                file_silent, file_recovered
         from recording_segments where recording_id = $1 order by id",
    )
    .bind(recording_id)
    .fetch_all(connection)
    .await?;
    records
        .into_iter()
        .map(|record| {
            Ok(SegmentRow {
                file: file_row(&record)?,
                id: record.id,
                source: SegmentSource::read(&record.source)?,
                account_id: record.account_id,
                track_id: record.track_id,
                state: SegmentState::read(&record.state)?,
                failure: record
                    .failure
                    .as_deref()
                    .map(SegmentFailure::read)
                    .transpose()?,
                job_id: record.job_id,
                requested_path: record.requested_path,
                requested_at: record.requested_at,
                job_started_at: record.job_started_at,
                stop_delivered_at: record.stop_delivered_at,
                stop_owed: record.stop_owed,
                started_at: record.started_at,
                ended_at: record.ended_at,
                duration_ms: record.duration_ms,
            })
        })
        .collect()
}

/// The file of a segment of a recording, when the processing found one: true for the file
/// of an audio track, false for the video of a composite.
pub async fn file_is_track(
    connection: &mut PgConnection,
    recording_id: RecordingId,
    segment_id: RecordingSegmentId,
) -> ApiResult<Option<bool>> {
    let source: Option<String> = sqlx::query_scalar(
        "select source from recording_segments
         where id = $1 and recording_id = $2
           and file_bytes is not null and media_ms is not null",
    )
    .bind(segment_id)
    .bind(recording_id)
    .fetch_optional(connection)
    .await?;
    source
        .map(|source| Ok(SegmentSource::read(&source)? != SegmentSource::Composite))
        .transpose()
}

/// The names among `jobs` that are the job of no segment.
pub async fn jobs_without_segment(
    connection: &mut PgConnection,
    jobs: &[String],
) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "select wanted.name from unnest($1::text[]) as wanted (name)
         where not exists (
             select 1 from recording_segments where job_id = wanted.name)",
    )
    .bind(jobs)
    .fetch_all(connection)
    .await?)
}

/// Stores a segment in the state `starting`: the server is about to ask the recorder for its
/// job.
pub async fn add_segment(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording_id: RecordingId,
    segment: SegmentRow,
) -> ApiResult<SegmentRow> {
    sqlx::query(
        "insert into recording_segments
             (id, recording_id, source, account_id, track_id, state, requested_path, requested_at)
         values ($1, $2, $3, $4, $5, 'starting', $6, $7)",
    )
    .bind(segment.id)
    .bind(recording_id)
    .bind(segment.source.stored())
    .bind(segment.account_id)
    .bind(segment.track_id.as_deref())
    .bind(&segment.requested_path)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(segment)
}

/// The longest identifier of a track or of a job that the record stores.
const LONGEST_IDENTIFIER: usize = 128;

/// Whether the record can store an identifier that the media server or the recorder gave.
pub fn fits(identifier: &str) -> bool {
    (1..=LONGEST_IDENTIFIER).contains(&identifier.chars().count())
}

/// The length of a file in whole milliseconds.
fn milliseconds(file: &FileResult) -> Option<i64> {
    let length = file
        .duration
        .or_else(|| Some(file.ended_at? - file.started_at?))?;
    i64::try_from(length.whole_milliseconds())
        .ok()
        .filter(|length| *length >= 0)
}

/// Stores what the recorder reported for the file of a segment, and the state that follows
/// from it: `ended` without a failure, `failed` with one. Of a failed segment the times state
/// the range that was saved, as far as the recorder reported it (ADR 0601). No stop is owed
/// for the segment afterwards; a declaration states it with [`owe_stop`].
pub async fn close_segment(
    connection: &mut PgConnection,
    segment: &mut SegmentRow,
    failure: Option<SegmentFailure>,
    file: Option<&FileResult>,
) -> ApiResult<()> {
    let state = match failure {
        Some(_) => SegmentState::Failed,
        None => SegmentState::Ended,
    };
    let started_at = file.and_then(|file| file.started_at).or(segment.started_at);
    let ended_at = file.and_then(|file| file.ended_at).or(segment.ended_at);
    let duration_ms = file.and_then(milliseconds).or(segment.duration_ms);
    sqlx::query(
        "update recording_segments
         set state = $2, failure = $3, file_name = coalesce($4, file_name), started_at = $5,
             ended_at = $6, duration_ms = $7, stop_owed = false
         where id = $1",
    )
    .bind(segment.id)
    .bind(state.stored())
    .bind(failure.map(StoredText::stored))
    .bind(file.and_then(|file| file.file_name.as_deref()))
    .bind(started_at)
    .bind(ended_at)
    .bind(duration_ms)
    .execute(connection)
    .await?;
    segment.state = state;
    segment.failure = failure;
    segment.stop_owed = false;
    segment.started_at = started_at;
    segment.ended_at = ended_at;
    segment.duration_ms = duration_ms;
    Ok(())
}

/// The job of a segment was declared lost before the recorder took a stop for it: the stop
/// that follows the declaration is owed until it has had an end (ADR 0604).
pub async fn owe_stop(connection: &mut PgConnection, segment: &mut SegmentRow) -> ApiResult<()> {
    sqlx::query("update recording_segments set stop_owed = true where id = $1")
        .bind(segment.id)
        .execute(connection)
        .await?;
    segment.stop_owed = true;
    Ok(())
}

/// The answer of the recorder to the request that starts the job of a segment.
pub async fn job_started(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    segment_id: RecordingSegmentId,
    job_id: &str,
) -> ApiResult<()> {
    sqlx::query(
        "update recording_segments set state = 'recording', job_id = $2, job_started_at = $3
         where id = $1 and state = 'starting'",
    )
    .bind(segment_id)
    .bind(job_id)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(())
}

pub async fn start_failed(
    connection: &mut PgConnection,
    segment_id: RecordingSegmentId,
    failure: SegmentFailure,
) -> ApiResult<()> {
    sqlx::query(
        "update recording_segments set state = 'failed', failure = $2
         where id = $1 and state = 'starting'",
    )
    .bind(segment_id)
    .bind(failure.stored())
    .execute(connection)
    .await?;
    Ok(())
}

/// The stop of the job of a segment has had its end. `delivered`: the recorder took it.
/// Without that it had no answer, which is not delivered (ADR 0601). In both cases the stop
/// is no longer owed.
pub async fn stop_answered(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    segment_id: RecordingSegmentId,
    delivered: bool,
) -> ApiResult<()> {
    sqlx::query(
        "update recording_segments
         set stop_owed = false,
             stop_delivered_at = case when $3 then $2 else stop_delivered_at end
         where id = $1 and job_id is not null",
    )
    .bind(segment_id)
    .bind(now)
    .bind(delivered)
    .execute(connection)
    .await?;
    Ok(())
}

/// The number of recordings that are starting or run.
pub async fn running(connection: &mut PgConnection) -> ApiResult<u32> {
    let count: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
        "select count(*) from recordings where {ACTIVE}"
    )))
    .fetch_one(connection)
    .await?;
    db::count(i32::try_from(count).unwrap_or(i32::MAX))
}

/// The server process that asks for an attempt, and the time after which an attempt of
/// another process without a heartbeat is over.
#[derive(Debug, Clone, Copy)]
pub struct Worker {
    pub instance: Uuid,
    pub lease: time::Duration,
}

/// The recordings whose files wait for their processing, oldest first, without those that
/// wait for a new attempt and those whose attempt another server process holds.
pub async fn processing(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    worker: Worker,
) -> ApiResult<Vec<RecordingId>> {
    Ok(sqlx::query_scalar(
        "select id from recordings
         where state = 'processing'
           and (processing_retry_at is null or processing_retry_at <= $1)
           and (processing_by is null or processing_by = $2 or processing_heartbeat_at <= $3)
         order by id",
    )
    .bind(now)
    .bind(worker.instance)
    .bind(now - worker.lease)
    .fetch_all(connection)
    .await?)
}

/// A recording whose processing starts an attempt.
pub struct Begun {
    pub recording: RecordingRow,
    pub segments: Vec<SegmentRow>,
    /// The number of this attempt, from 1.
    pub attempt: u32,
}

/// Takes one attempt to process the files of a recording for a server process. The attempt
/// counts when it starts, so that a server that stops during every attempt does not try
/// without end. One process holds an attempt: `None` while another process holds one whose
/// heartbeat is young, and for a recording that does not wait for its processing.
pub async fn begin_processing(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    id: RecordingId,
    worker: Worker,
) -> ApiResult<Option<Begun>> {
    let Some(recording) = recording(&mut *connection, Of::Id(id), true).await? else {
        return Ok(None);
    };
    if recording.state != RecordingState::Processing {
        return Ok(None);
    }
    let attempts: Option<i32> = sqlx::query_scalar(
        "update recordings
         set processing_attempts = processing_attempts + 1, processing_by = $2,
             processing_heartbeat_at = $3
         where id = $1
           and (processing_by is null or processing_by = $2 or processing_heartbeat_at <= $4)
         returning processing_attempts",
    )
    .bind(id)
    .bind(worker.instance)
    .bind(now)
    .bind(now - worker.lease)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(attempts) = attempts else {
        return Ok(None);
    };
    let segments = segments(connection, id).await?;
    Ok(Some(Begun {
        recording,
        segments,
        attempt: db::count(attempts)?,
    }))
}

/// States that the server process `instance` still runs the attempt. False when the attempt
/// is no longer its attempt.
pub async fn renew_processing(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    id: RecordingId,
    instance: Uuid,
) -> ApiResult<bool> {
    let renewed = sqlx::query(
        "update recordings set processing_heartbeat_at = $3
         where id = $1 and state = 'processing' and processing_by = $2",
    )
    .bind(id)
    .bind(instance)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(renewed.rows_affected() == 1)
}

/// True when the server process `instance` holds the attempt of a recording that waits for
/// its processing. The row of the recording is locked by its caller.
pub async fn holds_processing(
    connection: &mut PgConnection,
    id: RecordingId,
    instance: Uuid,
) -> ApiResult<bool> {
    Ok(sqlx::query_scalar(
        "select exists (
             select 1 from recordings
             where id = $1 and state = 'processing' and processing_by = $2)",
    )
    .bind(id)
    .bind(instance)
    .fetch_one(connection)
    .await?)
}

/// The attempt of `instance` did not end; the next one starts at `at`.
pub async fn retry_processing(
    connection: &mut PgConnection,
    id: RecordingId,
    instance: Uuid,
    at: OffsetDateTime,
) -> ApiResult<()> {
    sqlx::query(
        "update recordings
         set processing_retry_at = $3, processing_by = null, processing_heartbeat_at = null
         where id = $1 and state = 'processing' and processing_by = $2",
    )
    .bind(id)
    .bind(instance)
    .bind(at)
    .execute(connection)
    .await?;
    Ok(())
}

/// Stores what the processing found of the file of a segment.
pub async fn store_file(
    connection: &mut PgConnection,
    segment_id: RecordingSegmentId,
    file: Option<&FileRow>,
) -> ApiResult<()> {
    let side = |side: u32| i32::try_from(side).ok();
    let problems: Option<Vec<&'static str>> = file.map(|file| {
        file.problems
            .iter()
            .map(|problem| problem.stored())
            .collect()
    });
    let check = problems.as_ref().map(|problems| {
        if problems.is_empty() {
            "passed"
        } else {
            "failed"
        }
    });
    sqlx::query(
        "update recording_segments
         set file_bytes = $2, media_ms = $3, file_starts_at = $4, width = $5, height = $6,
             file_check = $7, file_problems = $8, file_silent = $9, file_recovered = $10
         where id = $1",
    )
    .bind(segment_id)
    .bind(file.map(|file| i64::try_from(file.bytes).unwrap_or(i64::MAX)))
    .bind(file.map(|file| file.media_ms))
    .bind(file.and_then(|file| file.starts_at))
    .bind(
        file.and_then(|file| file.size)
            .and_then(|(width, _)| side(width)),
    )
    .bind(
        file.and_then(|file| file.size)
            .and_then(|(_, height)| side(height)),
    )
    .bind(check)
    .bind(problems.unwrap_or_default())
    .bind(file.is_some_and(|file| file.silent))
    .bind(file.is_some_and(|file| file.recovered))
    .execute(connection)
    .await?;
    Ok(())
}

/// Puts a file or a directory below the directory of the recordings into the queue of
/// removals. The server deletes it after the transaction has committed (ADR 0608).
pub async fn queue_removal(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    place: &crate::recordings::files::Place,
) -> ApiResult<()> {
    sqlx::query("insert into file_removals (id, path, requested_at) values ($1, $2, $3)")
        .bind(Uuid::now_v7())
        .bind(place.stored())
        .bind(now)
        .execute(connection)
        .await?;
    Ok(())
}

/// The oldest removals that wait, at most `most`, without those whose deletion failed and
/// is tried again later.
pub async fn removals(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    most: i64,
) -> ApiResult<Vec<(Uuid, String)>> {
    Ok(sqlx::query_as(
        "select id, path from file_removals
         where retry_at is null or retry_at <= $1
         order by id limit $2",
    )
    .bind(now)
    .bind(most)
    .fetch_all(connection)
    .await?)
}

/// A deletion failed; it is tried again at `at`.
pub async fn retry_removal(
    connection: &mut PgConnection,
    id: Uuid,
    at: OffsetDateTime,
) -> ApiResult<()> {
    sqlx::query("update file_removals set retry_at = $2 where id = $1")
        .bind(id)
        .bind(at)
        .execute(connection)
        .await?;
    Ok(())
}

pub async fn removal_done(connection: &mut PgConnection, id: Uuid) -> ApiResult<()> {
    sqlx::query("delete from file_removals where id = $1")
        .bind(id)
        .execute(connection)
        .await?;
    Ok(())
}

/// The processing of the files of a recording has ended: the recording gets its final
/// state, and its reservation of disk space ends (ADR 0605). A recording with a file gets
/// the note for its post in the text chat (ADR 0602); one that is not whole gets the note
/// of a failure. The directories of the files that were saved of its lost jobs are deleted.
pub async fn processed(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording: &mut RecordingRow,
    state: RecordingState,
    timeline_started_at: Option<OffsetDateTime>,
) -> ApiResult<()> {
    let failure = (state == RecordingState::Failed).then_some(RecordingFailure::FilesMissing);
    sqlx::query(
        "update recordings
         set state = $2, failure = $3, processed_at = $4, timeline_started_at = $5,
             processing_retry_at = null, processing_by = null, processing_heartbeat_at = null
         where id = $1",
    )
    .bind(recording.id)
    .bind(state.stored())
    .bind(failure.map(StoredText::stored))
    .bind(now)
    .bind(timeline_started_at)
    .execute(&mut *connection)
    .await?;
    recording.state = state;
    recording.failure = failure;
    recording.processed_at = Some(now);
    recording.timeline_started_at = timeline_started_at;
    storage::release(&mut *connection, recording.id.as_uuid()).await?;
    if state != RecordingState::Ready {
        note_failure(&mut *connection, now, recording).await?;
    }
    if state != RecordingState::Failed {
        note(&mut *connection, now, recording, "recording_ready", None).await?;
    }
    // The directory of a lost job is deleted when the server has what it held: the audio
    // file of the segment. The directory of a composite holds nothing that the server uses.
    let lost_jobs: Vec<String> = sqlx::query_scalar(
        "select job_id from recording_segments
         where recording_id = $1 and failure = 'recorder_lost' and job_id is not null
           and (source = 'composite' or file_bytes is not null)",
    )
    .bind(recording.id)
    .fetch_all(&mut *connection)
    .await?;
    for job_id in lost_jobs {
        if let Some(rescued) = crate::recordings::files::Place::rescued(&job_id) {
            queue_removal(&mut *connection, now, &rescued).await?;
        }
    }
    Ok(())
}

/// Removes a recording with its segments and its transcript, and puts its directory into
/// the queue of removals. The notes of its start and its stop stay without the recording;
/// the note of its post goes with it. The caller has checked that the recording is in a
/// final state.
pub async fn remove(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording: &RecordingRow,
    by: Option<AccountId>,
) -> ApiResult<()> {
    let running = running_call(&mut *connection, recording.call_id).await?;
    let event = Event::RecordingRemoved {
        recording_id: recording.id,
        workspace_id: recording.workspace_id,
        channel_id: recording.channel_id,
    };
    events::tell_of_recording(&mut *connection, now, running, &event).await?;
    crate::exports::recording_removed(&mut *connection, now, recording.channel_id, recording.id)
        .await?;
    remove_notes(&mut *connection, now, recording, "recording_ready").await?;
    // Saved audio of a lost job that the server could not take is still in the directory of
    // that job. It goes with the recording.
    let lost_jobs: Vec<String> = sqlx::query_scalar(
        "select job_id from recording_segments
         where recording_id = $1 and failure = 'recorder_lost' and job_id is not null",
    )
    .bind(recording.id)
    .fetch_all(&mut *connection)
    .await?;
    for job_id in lost_jobs {
        if let Some(rescued) = crate::recordings::files::Place::rescued(&job_id) {
            queue_removal(&mut *connection, now, &rescued).await?;
        }
    }
    let entry = AuditEvent::RecordingRemoved {
        recording_id: recording.id,
    };
    audit::record(connection, now, by, recording.about(), &entry).await?;
    sqlx::query("delete from recordings where id = $1")
        .bind(recording.id)
        .execute(&mut *connection)
        .await?;
    storage::release(&mut *connection, recording.id.as_uuid()).await?;
    let place = crate::recordings::files::Place::recording(recording.id);
    queue_removal(&mut *connection, now, &place).await?;
    Ok(())
}
