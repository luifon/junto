//! The rows of transcripts, of their jobs for the transcription worker, and of the glossary
//! of a workspace (ADR 0603, ADR 0607). Every function works inside the transaction of its
//! caller.

use junto_protocol::ids::{AccountId, RecordingId, RecordingSegmentId, WorkspaceId};
use junto_protocol::transcript::{
    GlossaryUse, LineSource, SegmentReason, SegmentTranscription, SpeakerSeparation,
    SpeakersReason, Transcript, TranscriptFailure, TranscriptLine, TranscriptSegment,
    TranscriptSpeaker, TranscriptState,
};
use sqlx::PgConnection;
use time::{Duration, OffsetDateTime};
use uuid::Uuid;

use crate::db::{self, StoredText};
use crate::error::{ApiError, ApiResult};
use crate::recordings::contract;
use crate::recordings::store::RecordingRow;

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

stored!(TranscriptFailure {
    WorkerFailed => "worker_failed",
    JobRefused => "job_refused",
    WorkerNotReady => "worker_not_ready",
    TimeLimit => "time_limit",
    TranscriptInvalid => "transcript_invalid",
    Interrupted => "interrupted",
});

stored!(SegmentTranscription {
    Ok => "ok",
    Empty => "empty",
    Damaged => "damaged",
    Failed => "failed",
    NotTranscribed => "not_transcribed",
});

stored!(SegmentReason {
    FileMissing => "file_missing",
    NotARegularFile => "not_a_regular_file",
    DecodeFailed => "decode_failed",
    NotOneAudioStream => "not_one_audio_stream",
    DecodeIncomplete => "decode_incomplete",
    TimestampGap => "timestamp_gap",
    TimestampBackward => "timestamp_backward",
    SegmentDurationLimit => "segment_duration_limit",
    JobDurationLimit => "job_duration_limit",
    TimeLimit => "time_limit",
    ProcessingFailed => "processing_failed",
    ShorterThanFile => "shorter_than_file",
    NoFile => "no_file",
    NoStartTime => "no_start_time",
});

stored!(SpeakerSeparation {
    NotApplicable => "not_applicable",
    NotRequested => "not_requested",
    Separated => "separated",
    Unassigned => "unassigned",
    Failed => "failed",
});

stored!(SpeakersReason {
    SegmentFailed => "segment_failed",
    EmbeddingFailed => "embedding_failed",
    ClusteringFailed => "clustering_failed",
    NoVoiceWindow => "no_voice_window",
    SpeakerWindowLimit => "speaker_window_limit",
    TimeLimit => "time_limit",
});

stored!(LineSource {
    Microphone => "microphone",
    Shared => "shared",
});

/// The worker reported less audio than the file holds by more than this: the transcript
/// covers a part of the file (ADR 0611).
const SHORTER_BY: i64 = 1000;

/// Asks for the transcript of a recording whose files are processed: stores its job in the
/// state `waiting`, in place of an earlier transcript. Returns false, and stores nothing,
/// when no audio file of the recording has a place on the timeline.
pub async fn request(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording_id: RecordingId,
) -> ApiResult<bool> {
    let has_audio: bool = sqlx::query_scalar(
        "select exists (
             select 1 from recording_segments
             where recording_id = $1 and source <> 'composite'
               and file_bytes is not null and file_starts_at is not null)",
    )
    .bind(recording_id)
    .fetch_one(&mut *connection)
    .await?;
    sqlx::query("delete from transcripts where recording_id = $1")
        .bind(recording_id)
        .execute(&mut *connection)
        .await?;
    if !has_audio {
        return Ok(false);
    }
    sqlx::query(
        "insert into transcripts (recording_id, state, requested_at) values ($1, 'waiting', $2)",
    )
    .bind(recording_id)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(true)
}

/// The state of the transcript of a recording.
pub async fn state(
    connection: &mut PgConnection,
    recording_id: RecordingId,
) -> ApiResult<TranscriptState> {
    let state: Option<String> =
        sqlx::query_scalar("select state from transcripts where recording_id = $1")
            .bind(recording_id)
            .fetch_optional(connection)
            .await?;
    state
        .as_deref()
        .map_or(Ok(TranscriptState::None), TranscriptState::read)
}

/// What a claim found.
pub struct Claim {
    /// The job that this server process runs now, and the number of this attempt.
    pub claimed: Option<(RecordingId, u32)>,
    /// The recordings whose transcript changed its state, for their events.
    pub changed: Vec<RecordingId>,
}

/// Takes the oldest job that waits, for the server process `instance`.
///
/// One job runs at a time for all server processes (ADR 0607): nothing is taken while a
/// job has a heartbeat that is younger than `lease`. A job whose heartbeat is older lost its
/// server process: it waits again, or fails as interrupted when it has no attempt left.
/// Nothing is taken while a recording is starting or runs, because the worker would take
/// CPU cores that the recorder needs (ADR 0003).
pub async fn claim(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    instance: Uuid,
    lease: Duration,
    attempts: u32,
) -> ApiResult<Claim> {
    let attempts = i32::try_from(attempts).unwrap_or(i32::MAX);
    let mut changed: Vec<RecordingId> = sqlx::query_scalar(
        "update transcripts
         set state = case when attempts >= $2 then 'failed' else 'waiting' end,
             failure = case when attempts >= $2 then 'interrupted' end,
             finished_at = case when attempts >= $2 then $3::timestamptz end,
             claimed_by = null, claimed_at = null, heartbeat_at = null
         where state = 'running' and heartbeat_at <= $1
         returning recording_id",
    )
    .bind(now - lease)
    .bind(attempts)
    .bind(now)
    .fetch_all(&mut *connection)
    .await?;
    let busy: bool = sqlx::query_scalar(
        "select exists (select 1 from transcripts where state = 'running')
             or exists (select 1 from recordings where state in ('starting', 'recording'))",
    )
    .fetch_one(&mut *connection)
    .await?;
    if busy {
        return Ok(Claim {
            claimed: None,
            changed,
        });
    }
    let claimed: Option<(RecordingId, i32)> = sqlx::query_as(
        "update transcripts
         set state = 'running', attempts = attempts + 1, claimed_by = $2, claimed_at = $1,
             heartbeat_at = $1, retry_at = null
         where recording_id = (
             select recording_id from transcripts
             where state = 'waiting' and (retry_at is null or retry_at <= $1)
             order by requested_at, recording_id limit 1)
         returning recording_id, attempts",
    )
    .bind(now)
    .bind(instance)
    .fetch_optional(connection)
    .await?;
    let claimed = match claimed {
        Some((id, attempt)) => {
            if !changed.contains(&id) {
                changed.push(id);
            }
            Some((id, db::count(attempt)?))
        }
        None => None,
    };
    Ok(Claim { claimed, changed })
}

/// States that the server process `instance` still runs the job. False when the job is no
/// longer its job: the recording was removed, or another process took the job.
pub async fn renew(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording_id: RecordingId,
    instance: Uuid,
) -> ApiResult<bool> {
    let renewed = sqlx::query(
        "update transcripts set heartbeat_at = $3
         where recording_id = $1 and state = 'running' and claimed_by = $2",
    )
    .bind(recording_id)
    .bind(instance)
    .bind(now)
    .execute(connection)
    .await?;
    Ok(renewed.rows_affected() == 1)
}

/// True when the server process `instance` holds the job. The row of the job is locked until
/// the transaction ends, so that no other process stores a transcript in between.
pub async fn holds(
    connection: &mut PgConnection,
    recording_id: RecordingId,
    instance: Uuid,
) -> ApiResult<bool> {
    let held: Option<Uuid> = sqlx::query_scalar(
        "select claimed_by from transcripts
         where recording_id = $1 and state = 'running' and claimed_by = $2 for update",
    )
    .bind(recording_id)
    .bind(instance)
    .fetch_optional(connection)
    .await?;
    Ok(held.is_some())
}

/// One audio segment of a recording as the job of the worker needs it.
pub struct AudioSegment {
    pub id: RecordingSegmentId,
    pub source: LineSource,
    pub account_id: AccountId,
    pub name: String,
    /// The first sample of the file. Absent without a file, and for a file that has no
    /// place on the timeline.
    pub starts_at: Option<OffsetDateTime>,
    pub has_file: bool,
    pub media_ms: Option<i64>,
}

/// The audio segments of a recording for which a job of the recorder ran, in the order in
/// which they were asked for, each with the name of its person.
pub async fn audio_segments(
    connection: &mut PgConnection,
    recording_id: RecordingId,
) -> ApiResult<Vec<AudioSegment>> {
    type Row = (
        RecordingSegmentId,
        String,
        AccountId,
        String,
        Option<OffsetDateTime>,
        bool,
        Option<i64>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "select s.id, s.source, s.account_id, a.name, s.file_starts_at,
                s.file_bytes is not null, s.media_ms
         from recording_segments s join accounts a on a.id = s.account_id
         where s.recording_id = $1 and s.source <> 'composite'
           and (s.job_id is not null or s.file_bytes is not null)
         order by s.id",
    )
    .bind(recording_id)
    .fetch_all(connection)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, source, account_id, name, starts_at, has_file, media_ms)| AudioSegment {
                id,
                source: if source == "microphone" {
                    LineSource::Microphone
                } else {
                    LineSource::Shared
                },
                account_id,
                name,
                starts_at,
                has_file,
                media_ms,
            },
        )
        .collect())
}

impl AudioSegment {
    /// Whether the worker gets the file of the segment.
    pub fn is_transcribed(&self) -> bool {
        self.has_file && self.starts_at.is_some()
    }
}

/// The glossary of a workspace in its order of priority.
pub async fn glossary(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
) -> ApiResult<Vec<String>> {
    Ok(sqlx::query_scalar(
        "select term from glossary_terms where workspace_id = $1 order by position",
    )
    .bind(workspace_id)
    .fetch_all(connection)
    .await?)
}

/// Replaces the glossary of a workspace.
pub async fn replace_glossary(
    connection: &mut PgConnection,
    workspace_id: WorkspaceId,
    terms: &[String],
) -> ApiResult<()> {
    sqlx::query("delete from glossary_terms where workspace_id = $1")
        .bind(workspace_id)
        .execute(&mut *connection)
        .await?;
    // Two requests that replace an empty glossary at the same time both insert its first
    // term. The second one lost and runs again (ADR 0009).
    sqlx::query(
        "insert into glossary_terms (workspace_id, position, term)
         select $1, (place - 1)::integer, term
         from unnest($2::text[]) with ordinality as listed (term, place)",
    )
    .bind(workspace_id)
    .bind(terms)
    .execute(connection)
    .await
    .map_err(ApiError::concurrent)?;
    Ok(())
}

/// How a run of the worker ended without a transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ended {
    /// The job fails with this reason; no further run follows.
    Failed(TranscriptFailure),
    /// A further run follows after a wait, unless the job has no attempt left; then it
    /// fails with this reason.
    TryAgain(TranscriptFailure),
}

/// Stores that a run of the worker gave no transcript. Returns the state that the job has
/// now; `None` when the job is no longer the job of `instance`.
pub async fn ended_without(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording_id: RecordingId,
    instance: Uuid,
    ended: Ended,
    attempts: u32,
    retry_wait: Duration,
) -> ApiResult<Option<TranscriptState>> {
    let used: Option<i32> = sqlx::query_scalar(
        "select attempts from transcripts
         where recording_id = $1 and state = 'running' and claimed_by = $2 for update",
    )
    .bind(recording_id)
    .bind(instance)
    .fetch_optional(&mut *connection)
    .await?;
    let Some(used) = used else {
        return Ok(None);
    };
    let used = db::count(used)?;
    let failure = match ended {
        Ended::Failed(failure) => Some(failure),
        Ended::TryAgain(failure) if used >= attempts => Some(failure),
        Ended::TryAgain(_) => None,
    };
    let (state, retry_at) = match failure {
        Some(_) => (TranscriptState::Failed, None),
        None => (
            TranscriptState::Waiting,
            Some(now + retry_wait * i32::try_from(used).unwrap_or(i32::MAX)),
        ),
    };
    sqlx::query(
        "update transcripts
         set state = $2, failure = $3, retry_at = $4, finished_at = $5,
             claimed_by = null, claimed_at = null, heartbeat_at = null
         where recording_id = $1",
    )
    .bind(recording_id)
    .bind(state.stored())
    .bind(failure.map(StoredText::stored))
    .bind(retry_at)
    .bind(failure.map(|_| now))
    .execute(connection)
    .await?;
    Ok(Some(state))
}

/// One segment of a job with the result that the worker reported.
pub struct Transcribed<'a> {
    pub segment: &'a AudioSegment,
    pub result: &'a contract::SegmentResult,
}

const LINES_AT_ONCE: usize = 2000;

/// Stores the transcript that the worker wrote for the job of `instance`: its lines, its
/// speaker labels and the result of every audio segment of the recording. Returns the state
/// of the transcript; `None`, and stores nothing, when the job is no longer the job of
/// `instance`.
pub async fn store(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording_id: RecordingId,
    instance: Uuid,
    segments: &[AudioSegment],
    transcript: &contract::Transcript,
) -> ApiResult<Option<TranscriptState>> {
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "select claimed_by from transcripts
         where recording_id = $1 and state = 'running' and claimed_by = $2 for update",
    )
    .bind(recording_id)
    .bind(instance)
    .fetch_optional(&mut *connection)
    .await?;
    if claimed.is_none() {
        return Ok(None);
    }
    let in_job: Vec<&AudioSegment> = segments
        .iter()
        .filter(|segment| segment.is_transcribed())
        .collect();
    if in_job.len() != transcript.segments.len() {
        return Err(ApiError::internal(
            &"a transcript does not have the segments of its job",
        ));
    }
    let mut whole = segments.len() == in_job.len();
    for segment in segments.iter().filter(|segment| !segment.is_transcribed()) {
        let reason = if segment.has_file {
            SegmentReason::NoStartTime
        } else {
            SegmentReason::NoFile
        };
        let speakers = match segment.source {
            LineSource::Microphone => SpeakerSeparation::NotApplicable,
            LineSource::Shared => SpeakerSeparation::NotRequested,
        };
        sqlx::query(
            "insert into transcript_segments (recording_id, segment_id, status, reason, speakers)
             values ($1, $2, 'not_transcribed', $3, $4)",
        )
        .bind(recording_id)
        .bind(segment.id)
        .bind(reason.stored())
        .bind(speakers.stored())
        .execute(&mut *connection)
        .await?;
    }
    for (segment, result) in in_job.iter().zip(&transcript.segments) {
        let audio_ms = i64::try_from(result.audio_ms).unwrap_or(i64::MAX);
        let shorter = segment
            .media_ms
            .is_some_and(|media_ms| audio_ms.saturating_add(SHORTER_BY) < media_ms);
        let (status, reason) = match result.status {
            SegmentTranscription::Ok | SegmentTranscription::Empty if shorter => (
                SegmentTranscription::Damaged,
                Some(SegmentReason::ShorterThanFile),
            ),
            status => (status, result.reason),
        };
        whole &= matches!(
            status,
            SegmentTranscription::Ok | SegmentTranscription::Empty
        );
        sqlx::query(
            "insert into transcript_segments
                 (recording_id, segment_id, status, reason, audio_ms, processed_ms, lines,
                  speakers, speakers_reason)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
        )
        .bind(recording_id)
        .bind(segment.id)
        .bind(status.stored())
        .bind(reason.map(StoredText::stored))
        .bind(audio_ms)
        .bind(i64::try_from(result.processed_ms).unwrap_or(i64::MAX))
        .bind(i32::try_from(result.lines).unwrap_or(i32::MAX))
        .bind(result.speakers.stored())
        .bind(result.speakers_reason.map(StoredText::stored))
        .execute(&mut *connection)
        .await?;
    }
    for (number, shared) in &transcript.speakers {
        sqlx::query(
            "insert into transcript_speakers (recording_id, speaker, account_id)
             values ($1, $2, $3)",
        )
        .bind(recording_id)
        .bind(i32::try_from(*number).unwrap_or(i32::MAX))
        .bind(in_job[*shared].account_id)
        .execute(&mut *connection)
        .await?;
    }
    let milliseconds = |time: u64| i64::try_from(time).unwrap_or(i64::MAX);
    for (chunk, lines) in transcript.lines.chunks(LINES_AT_ONCE).enumerate() {
        let first = chunk * LINES_AT_ONCE;
        let (mut positions, mut starts, mut ends) = (Vec::new(), Vec::new(), Vec::new());
        let (mut segment_ids, mut sources, mut accounts) = (Vec::new(), Vec::new(), Vec::new());
        let (mut speakers, mut texts, mut languages) = (Vec::new(), Vec::new(), Vec::new());
        for (offset, line) in lines.iter().enumerate() {
            let segment = in_job[line.segment];
            positions.push(i32::try_from(first + offset).unwrap_or(i32::MAX));
            starts.push(milliseconds(line.start_ms));
            ends.push(milliseconds(line.end_ms));
            segment_ids.push(segment.id);
            sources.push(segment.source.stored());
            accounts.push(segment.account_id);
            speakers.push(line.speaker.and_then(|speaker| i32::try_from(speaker).ok()));
            texts.push(line.text.as_str());
            languages.push(line.language.as_str());
        }
        sqlx::query(
            "insert into transcript_lines
                 (recording_id, line, start_ms, end_ms, segment_id, source, account_id, speaker,
                  text, language)
             select $1, * from unnest(
                 $2::integer[], $3::bigint[], $4::bigint[], $5::uuid[], $6::text[], $7::uuid[],
                 $8::integer[], $9::text[], $10::text[])",
        )
        .bind(recording_id)
        .bind(positions)
        .bind(starts)
        .bind(ends)
        .bind(segment_ids)
        .bind(sources)
        .bind(accounts)
        .bind(speakers)
        .bind(texts)
        .bind(languages)
        .execute(&mut *connection)
        .await?;
    }
    let state = if whole {
        TranscriptState::Ready
    } else {
        TranscriptState::Partial
    };
    sqlx::query(
        "update transcripts
         set state = $2, finished_at = $3, lines = $4, glossary_used = $5, glossary_cut = $6,
             claimed_by = null, claimed_at = null, heartbeat_at = null
         where recording_id = $1",
    )
    .bind(recording_id)
    .bind(state.stored())
    .bind(now)
    .bind(i32::try_from(transcript.lines.len()).unwrap_or(i32::MAX))
    .bind(&transcript.glossary_used)
    .bind(&transcript.glossary_cut)
    .execute(connection)
    .await?;
    Ok(Some(state))
}

/// The speaker labels of a transcript, each with its newest name.
pub async fn speakers(
    connection: &mut PgConnection,
    recording_id: RecordingId,
    only: Option<u32>,
) -> ApiResult<Vec<TranscriptSpeaker>> {
    type Row = (
        i32,
        AccountId,
        Option<String>,
        Option<AccountId>,
        Option<OffsetDateTime>,
    );
    let rows: Vec<Row> = sqlx::query_as(
        "select s.speaker, s.account_id, n.name, n.renamed_by, n.renamed_at
         from transcript_speakers s
         left join lateral (
             select name, renamed_by, renamed_at from transcript_speaker_names
             where recording_id = s.recording_id and speaker = s.speaker
             order by id desc limit 1) n on true
         where s.recording_id = $1 and ($2::integer is null or s.speaker = $2)
         order by s.speaker",
    )
    .bind(recording_id)
    .bind(only.and_then(|number| i32::try_from(number).ok()))
    .fetch_all(connection)
    .await?;
    rows.into_iter()
        .map(|(number, account_id, name, renamed_by, renamed_at)| {
            Ok(TranscriptSpeaker {
                number: db::count(number)?,
                account_id,
                name,
                renamed_by,
                renamed_at,
            })
        })
        .collect()
}

/// Gives a speaker label a name. `None` when the transcript has no such label.
pub async fn rename_speaker(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording_id: RecordingId,
    number: u32,
    name: &str,
    by: AccountId,
) -> ApiResult<Option<TranscriptSpeaker>> {
    let Ok(speaker) = i32::try_from(number) else {
        return Ok(None);
    };
    let known: Option<i32> = sqlx::query_scalar(
        "select speaker from transcript_speakers
         where recording_id = $1 and speaker = $2 for update",
    )
    .bind(recording_id)
    .bind(speaker)
    .fetch_optional(&mut *connection)
    .await?;
    if known.is_none() {
        return Ok(None);
    }
    sqlx::query(
        "insert into transcript_speaker_names
             (id, recording_id, speaker, name, renamed_by, renamed_at)
         values ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(recording_id)
    .bind(speaker)
    .bind(name)
    .bind(by)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    Ok(speakers(connection, recording_id, Some(number))
        .await?
        .into_iter()
        .next())
}

/// The transcript of a recording without its lines.
pub async fn header(
    connection: &mut PgConnection,
    recording: &RecordingRow,
) -> ApiResult<Transcript> {
    type Row = (String, Option<String>, i32, Vec<String>, Vec<String>);
    let row: Option<Row> = sqlx::query_as(
        "select state, failure, lines, glossary_used, glossary_cut
         from transcripts where recording_id = $1",
    )
    .bind(recording.id)
    .fetch_optional(&mut *connection)
    .await?;
    let Some((state, failure, lines, used, cut)) = row else {
        return Ok(Transcript {
            recording_id: recording.id,
            state: TranscriptState::None,
            failure: None,
            language: recording.language.clone(),
            lines: 0,
            speakers: Vec::new(),
            glossary: GlossaryUse::default(),
            segments: Vec::new(),
        });
    };
    type SegmentRow = (
        RecordingSegmentId,
        String,
        Option<String>,
        Option<i64>,
        Option<i64>,
        i32,
        String,
        Option<String>,
    );
    let segments: Vec<SegmentRow> = sqlx::query_as(
        "select segment_id, status, reason, audio_ms, processed_ms, lines, speakers,
                speakers_reason
         from transcript_segments where recording_id = $1 order by segment_id",
    )
    .bind(recording.id)
    .fetch_all(&mut *connection)
    .await?;
    let length = |length: Option<i64>| length.and_then(|length| u64::try_from(length).ok());
    let segments = segments
        .into_iter()
        .map(|row| {
            let (segment_id, status, reason, audio_ms, processed_ms, lines, speakers, why) = row;
            Ok(TranscriptSegment {
                segment_id,
                status: SegmentTranscription::read(&status)?,
                reason: reason.as_deref().map(SegmentReason::read).transpose()?,
                audio_ms: length(audio_ms),
                processed_ms: length(processed_ms),
                lines: db::count(lines)?,
                speakers: SpeakerSeparation::read(&speakers)?,
                speakers_reason: why.as_deref().map(SpeakersReason::read).transpose()?,
            })
        })
        .collect::<ApiResult<_>>()?;
    Ok(Transcript {
        recording_id: recording.id,
        state: TranscriptState::read(&state)?,
        failure: failure
            .as_deref()
            .map(TranscriptFailure::read)
            .transpose()?,
        language: recording.language.clone(),
        lines: db::count(lines)?,
        speakers: speakers(&mut *connection, recording.id, None).await?,
        glossary: GlossaryUse { used, cut },
        segments,
    })
}

#[derive(sqlx::FromRow)]
struct LineRecord {
    line: i32,
    start_ms: i64,
    end_ms: i64,
    segment_id: RecordingSegmentId,
    source: String,
    account_id: AccountId,
    speaker: Option<i32>,
    text: String,
    language: String,
    corrected_text: Option<String>,
    corrected_by: Option<AccountId>,
    corrected_at: Option<OffsetDateTime>,
}

fn line(record: LineRecord) -> ApiResult<TranscriptLine> {
    let time = |time: i64| u64::try_from(time).map_err(|error| ApiError::internal(&error));
    let corrected = record.corrected_text.is_some();
    Ok(TranscriptLine {
        line: db::count(record.line)?,
        start_ms: time(record.start_ms)?,
        end_ms: time(record.end_ms)?,
        segment_id: record.segment_id,
        source: LineSource::read(&record.source)?,
        account_id: record.account_id,
        speaker: record.speaker.map(db::count).transpose()?,
        original_text: corrected.then(|| record.text.clone()),
        text: record.corrected_text.unwrap_or(record.text),
        language: record.language,
        corrected_by: record.corrected_by,
        corrected_at: record.corrected_at,
    })
}

const LINE_COLUMNS: &str = "l.line, l.start_ms, l.end_ms, l.segment_id, l.source, l.account_id, \
     l.speaker, l.text, l.language, c.text as corrected_text, c.corrected_by, c.corrected_at";

const NEWEST_CORRECTION: &str = "left join lateral (
         select text, corrected_by, corrected_at from transcript_corrections
         where recording_id = l.recording_id and line = l.line
         order by id desc limit 1) c on true";

/// Which lines of a transcript a read asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Window {
    /// The lines after a position, or from the first line.
    After(Option<u32>),
    /// The lines before a position, the ones nearest to it.
    Before(u32),
    /// The lines from a position on, that position included.
    From(u32),
}

/// The lines of a transcript in `window`, at most `most`, in the order of the timeline,
/// each with its newest correction.
pub async fn lines(
    connection: &mut PgConnection,
    recording_id: RecordingId,
    window: Window,
    most: i64,
) -> ApiResult<Vec<TranscriptLine>> {
    let (condition, bound, order) = match window {
        Window::After(after) => ("l.line > $2", after.map_or(-1, i64::from), "asc"),
        Window::From(first) => ("l.line >= $2", i64::from(first), "asc"),
        Window::Before(before) => ("l.line < $2", i64::from(before), "desc"),
    };
    let records: Vec<LineRecord> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {LINE_COLUMNS} from transcript_lines l {NEWEST_CORRECTION}
         where l.recording_id = $1 and {condition}
         order by l.line {order} limit $3"
    )))
    .bind(recording_id)
    .bind(bound)
    .bind(most)
    .fetch_all(connection)
    .await?;
    let mut lines = records
        .into_iter()
        .map(line)
        .collect::<ApiResult<Vec<_>>>()?;
    if matches!(window, Window::Before(_)) {
        lines.reverse();
    }
    Ok(lines)
}

/// Whether the transcript has a line before `position` (`before`), or at or after it.
pub async fn has_line(
    connection: &mut PgConnection,
    recording_id: RecordingId,
    position: u32,
    before: bool,
) -> ApiResult<bool> {
    let condition = if before { "line < $2" } else { "line >= $2" };
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "select exists (
             select 1 from transcript_lines where recording_id = $1 and {condition})"
    )))
    .bind(recording_id)
    .bind(i64::from(position))
    .fetch_one(connection)
    .await?)
}

/// Why a correction was not stored.
pub enum NotCorrected {
    /// The transcript has no line at the position.
    NoLine,
    /// The line has as many corrections as the server keeps.
    Full,
}

/// Stores a correction of one line and returns the line with it.
pub async fn correct(
    connection: &mut PgConnection,
    now: OffsetDateTime,
    recording_id: RecordingId,
    position: u32,
    text: &str,
    by: AccountId,
    most: u32,
) -> ApiResult<Result<TranscriptLine, NotCorrected>> {
    let Ok(position) = i32::try_from(position) else {
        return Ok(Err(NotCorrected::NoLine));
    };
    let known: Option<i32> = sqlx::query_scalar(
        "select line from transcript_lines where recording_id = $1 and line = $2 for update",
    )
    .bind(recording_id)
    .bind(position)
    .fetch_optional(&mut *connection)
    .await?;
    if known.is_none() {
        return Ok(Err(NotCorrected::NoLine));
    }
    let made: i64 = sqlx::query_scalar(
        "select count(*) from transcript_corrections where recording_id = $1 and line = $2",
    )
    .bind(recording_id)
    .bind(position)
    .fetch_one(&mut *connection)
    .await?;
    if made >= i64::from(most) {
        return Ok(Err(NotCorrected::Full));
    }
    sqlx::query(
        "insert into transcript_corrections
             (id, recording_id, line, text, corrected_by, corrected_at)
         values ($1, $2, $3, $4, $5, $6)",
    )
    .bind(Uuid::now_v7())
    .bind(recording_id)
    .bind(position)
    .bind(text)
    .bind(by)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    let record: LineRecord = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select {LINE_COLUMNS} from transcript_lines l {NEWEST_CORRECTION}
         where l.recording_id = $1 and l.line = $2"
    )))
    .bind(recording_id)
    .bind(position)
    .fetch_one(connection)
    .await?;
    line(record).map(Ok)
}
