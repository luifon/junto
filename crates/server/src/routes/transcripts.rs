//! The routes of transcripts (ADR 0603, ADR 0607). A transcript is read, corrected and
//! requested by the rule of its recording (ADR 0604).

use std::collections::HashMap;
use std::fmt::Write as _;

use axum::Json;
use axum::extract::State;
use axum::http::HeaderValue;
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE, X_CONTENT_TYPE_OPTIONS,
};
use axum::response::{IntoResponse, Response};
use junto_authz::{Request, WorkspacePermission};
use junto_protocol::audit::AuditEvent;
use junto_protocol::events::Event;
use junto_protocol::ids::{AccountId, RecordingId};
use junto_protocol::recording::{Recording, RecordingState};
use junto_protocol::transcript::{
    CorrectLine, LineSource, RenameSpeaker, Transcript, TranscriptLine, TranscriptLinePage,
    TranscriptLineQuery, TranscriptSpeaker, TranscriptState,
};
use sqlx::PgConnection;

use crate::access::Access;
use crate::audit::{self, About};
use crate::error::{ApiError, ApiResult};
use crate::events;
use crate::extract::{ApiJson, ApiPath, ApiQuery, Bearer};
use crate::proof;
use crate::recordings::store::{self, RecordingRow};
use crate::recordings::transcripts::{self, NotCorrected};
use crate::routes::recordings::{changeable, readable};
use crate::state::AppState;
use crate::validate;
use crate::{read, transact};

const DEFAULT_PAGE: u32 = 200;
const LARGEST_PAGE: u32 = 500;
/// The longest text of a correction. With every character at its longest encoding, the
/// event of a correction stays below the bound of an event (ADR 0011).
const LONGEST_CORRECTION: usize = 2000;
/// The lines that the text form of a transcript reads at one time.
const TEXT_PAGE: i64 = 2000;

/// The transcript of a recording without its lines: its state, its speaker labels with
/// their names, the glossary terms that the speech model used, and the result for each
/// audio segment. The caller may read the recording.
#[utoipa::path(
    get,
    path = "/api/v1/recordings/{recording_id}/transcript",
    tag = "recordings",
    params(("recording_id" = RecordingId, Path)),
    security(("session" = [])),
    responses((status = 200, body = Transcript), ApiError)
)]
pub async fn get_transcript(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(recording_id): ApiPath<RecordingId>,
) -> ApiResult<Json<Transcript>> {
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (recording, _) = readable(connection, &access, recording_id, false).await?;
        transcripts::header(connection, &recording).await
    })
    .map(Json)
}

/// Asks for the transcript of a recording again. The caller holds `manage_recordings` in
/// the voice channel. The recording has files, and it has no transcript or its transcript
/// failed; otherwise the request gets `conflict`.
#[utoipa::path(
    post,
    path = "/api/v1/recordings/{recording_id}/transcript",
    tag = "recordings",
    params(("recording_id" = RecordingId, Path)),
    security(("session" = [])),
    responses((status = 200, body = Recording), ApiError)
)]
pub async fn request_transcript(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(recording_id): ApiPath<RecordingId>,
) -> ApiResult<Json<Recording>> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let transcribes = app
        .recordings
        .backend()
        .is_some_and(|backend| backend.settings.transcriber.is_some());
    let recording = transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (recording, scope) = changeable(connection, &access, recording_id, true).await?;
        access.require(Request::Channel {
            scope: &scope,
            permission: WorkspacePermission::ManageRecordings,
        })?;
        if !transcribes {
            return Err(ApiError::conflict(
                "this server has no transcription worker",
            ));
        }
        let processed = matches!(
            recording.state,
            RecordingState::Ready | RecordingState::PartlyFailed
        );
        let state = transcripts::state(connection, recording_id).await?;
        if !processed || !matches!(state, TranscriptState::None | TranscriptState::Failed) {
            return Err(ApiError::conflict(
                "a transcript is asked for again when the files of the recording are processed \
                 and it has no transcript or its transcript failed",
            ));
        }
        if !transcripts::request(connection, now, recording_id).await? {
            return Err(ApiError::conflict(
                "the recording has no audio file with a place on its timeline",
            ));
        }
        let entry = AuditEvent::TranscriptRequested { recording_id };
        let about = About::Channel(recording.workspace_id, recording.channel_id);
        audit::record(connection, now, Some(access.account_id), about, &entry).await?;
        store::announce(connection, now, &recording, false).await?;
        store::summary(connection, &recording).await
    })?;
    app.recordings.transcribe_now();
    Ok(Json(recording))
}

fn page_size(query: &TranscriptLineQuery) -> ApiResult<i64> {
    match query.limit.unwrap_or(DEFAULT_PAGE) {
        limit @ 1..=LARGEST_PAGE => Ok(i64::from(limit)),
        _ => Err(ApiError::invalid(format!(
            "limit must be from 1 to {LARGEST_PAGE}"
        ))),
    }
}

/// The lines of the transcript of a recording in the order of the timeline, in pages. Each
/// line carries its newest correction and, when it was corrected, the text of the worker.
/// The caller may read the recording.
#[utoipa::path(
    get,
    path = "/api/v1/recordings/{recording_id}/transcript/lines",
    tag = "recordings",
    params(("recording_id" = RecordingId, Path), TranscriptLineQuery),
    security(("session" = [])),
    responses((status = 200, body = TranscriptLinePage), ApiError)
)]
pub async fn list_transcript_lines(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(recording_id): ApiPath<RecordingId>,
    ApiQuery(query): ApiQuery<TranscriptLineQuery>,
) -> ApiResult<Json<TranscriptLinePage>> {
    let size = page_size(&query)?;
    let now = app.now();
    read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        readable(connection, &access, recording_id, false).await?;
        let mut lines = transcripts::lines(connection, recording_id, query.after, size + 1).await?;
        let shown = usize::try_from(size).unwrap_or(usize::MAX);
        let has_more = lines.len() > shown;
        lines.truncate(shown);
        let next_after = has_more
            .then(|| lines.last().map(|line| line.line))
            .flatten();
        Ok(TranscriptLinePage { lines, next_after })
    })
    .map(Json)
}

fn correction(body: &CorrectLine) -> ApiResult<String> {
    let text = body.text.trim();
    let length = text.chars().count();
    if length == 0 || length > LONGEST_CORRECTION {
        return Err(ApiError::invalid(format!(
            "text has 1 to {LONGEST_CORRECTION} characters"
        )));
    }
    if text.chars().any(char::is_control) {
        return Err(ApiError::invalid("text contains a control character"));
    }
    Ok(text.to_owned())
}

/// Records the event of a change of a transcript for the accounts that may read its
/// recording now.
async fn tell(
    connection: &mut PgConnection,
    now: time::OffsetDateTime,
    recording: &RecordingRow,
    event: &Event,
) -> ApiResult<()> {
    let call_runs = store::call_runs(&mut *connection, recording.call_id).await?;
    events::tell_of_recording(connection, now, call_runs, event).await
}

/// Corrects one line of the transcript of a recording. Every person who may read the
/// recording corrects a line. The correction names its author, and the text that the
/// worker wrote stays readable (ADR 0603).
#[utoipa::path(
    put,
    path = "/api/v1/recordings/{recording_id}/transcript/lines/{line}",
    tag = "recordings",
    params(
        ("recording_id" = RecordingId, Path),
        ("line" = u32, Path, description = "The position of the line")
    ),
    request_body = CorrectLine,
    security(("session" = [])),
    responses((status = 200, body = TranscriptLine), ApiError)
)]
pub async fn correct_transcript_line(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((recording_id, position)): ApiPath<(RecordingId, u32)>,
    ApiJson(body): ApiJson<CorrectLine>,
) -> ApiResult<Json<TranscriptLine>> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let text = correction(&body)?;
    let most = app.config.limits.recordings.most_corrections;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (recording, _) = changeable(connection, &access, recording_id, false).await?;
        let by = access.account_id;
        let corrected =
            transcripts::correct(connection, now, recording_id, position, &text, by, most).await?;
        let line = match corrected {
            Ok(line) => line,
            Err(NotCorrected::NoLine) => return Err(ApiError::not_found()),
            Err(NotCorrected::Full) => {
                return Err(ApiError::limit_reached(format!(
                    "the line has {most} corrections, which is the most that the server keeps"
                )));
            }
        };
        let event = Event::TranscriptLineCorrected {
            recording_id,
            workspace_id: recording.workspace_id,
            channel_id: recording.channel_id,
            line: line.line,
            text: line.text.clone(),
            corrected_by: by,
            corrected_at: now,
        };
        tell(connection, now, &recording, &event).await?;
        Ok(line)
    })
    .map(Json)
}

/// Gives a name to one voice in the shared sound of a recording. The name replaces the
/// label `Speaker N` in this recording only. Every person who may read the recording
/// renames a voice, and the rename names its author (ADR 0603).
#[utoipa::path(
    put,
    path = "/api/v1/recordings/{recording_id}/transcript/speakers/{number}",
    tag = "recordings",
    params(
        ("recording_id" = RecordingId, Path),
        ("number" = u32, Path, description = "N of the label `Speaker N`")
    ),
    request_body = RenameSpeaker,
    security(("session" = [])),
    responses((status = 200, body = TranscriptSpeaker), ApiError)
)]
pub async fn rename_transcript_speaker(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath((recording_id, number)): ApiPath<(RecordingId, u32)>,
    ApiJson(body): ApiJson<RenameSpeaker>,
) -> ApiResult<Json<TranscriptSpeaker>> {
    let now = app.now();
    proof::limit(&app.calls.actions, bearer.0, now)?;
    let name = validate::name("name", &body.name)?;
    transact!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        let (recording, _) = changeable(connection, &access, recording_id, false).await?;
        let by = access.account_id;
        let speaker = transcripts::rename_speaker(connection, now, recording_id, number, &name, by)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let event = Event::TranscriptSpeakerRenamed {
            recording_id,
            workspace_id: recording.workspace_id,
            channel_id: recording.channel_id,
            speaker: speaker.clone(),
        };
        tell(connection, now, &recording, &event).await?;
        Ok(speaker)
    })
    .map(Json)
}

/// A time of the timeline as hours, minutes and seconds.
fn clock(milliseconds: u64) -> String {
    let seconds = milliseconds / 1000;
    format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

/// One line of the text form of a transcript.
fn text_line(
    line: &TranscriptLine,
    names: &HashMap<AccountId, String>,
    speakers: &[TranscriptSpeaker],
) -> String {
    let person = names
        .get(&line.account_id)
        .map_or_else(|| line.account_id.to_string(), Clone::clone);
    let voice = line.speaker.map(|number| {
        speakers
            .iter()
            .find(|speaker| speaker.number == number)
            .and_then(|speaker| speaker.name.clone())
            .unwrap_or_else(|| format!("Speaker {number}"))
    });
    let who = match (line.source, voice) {
        (LineSource::Microphone, _) => person,
        (LineSource::Shared, Some(voice)) => format!("{voice} (shared audio of {person})"),
        (LineSource::Shared, None) => format!("Shared audio of {person}"),
    };
    format!("[{}] {who}: {}\n", clock(line.start_ms), line.text)
}

/// The text form of the transcript of a recording. `None` for a recording without a
/// transcript that is ready or partial. The caller has decided that the reader may read the
/// recording.
pub(crate) async fn text_of(
    connection: &mut sqlx::PgConnection,
    recording_id: RecordingId,
) -> ApiResult<Option<String>> {
    let state = transcripts::state(connection, recording_id).await?;
    if !matches!(state, TranscriptState::Ready | TranscriptState::Partial) {
        return Ok(None);
    }
    let speakers = transcripts::speakers(connection, recording_id, None).await?;
    let names: Vec<(AccountId, String)> = sqlx::query_as(
        "select id, name from accounts where id in (
             select account_id from recording_segments where recording_id = $1)",
    )
    .bind(recording_id)
    .fetch_all(&mut *connection)
    .await?;
    let names: HashMap<AccountId, String> = names.into_iter().collect();
    let mut text = String::new();
    let mut after = None;
    loop {
        let lines = transcripts::lines(connection, recording_id, after, TEXT_PAGE).await?;
        let Some(last) = lines.last() else {
            break;
        };
        after = Some(last.line);
        for line in &lines {
            let _ = write!(text, "{}", text_line(line, &names, &speakers));
        }
    }
    Ok(Some(text))
}

/// The transcript of a recording as plain text: one line of text for each line of the
/// transcript, with its time, its speaker and its newest correction. The caller may read
/// the recording.
#[utoipa::path(
    get,
    path = "/api/v1/recordings/{recording_id}/transcript/text",
    tag = "recordings",
    params(("recording_id" = RecordingId, Path)),
    security(("session" = [])),
    responses(
        (status = 200, description = "The transcript.", content_type = "text/plain", body = String),
        ApiError
    )
)]
pub async fn get_transcript_text(
    State(app): State<AppState>,
    bearer: Bearer,
    ApiPath(recording_id): ApiPath<RecordingId>,
) -> ApiResult<Response> {
    let now = app.now();
    let text = read!(app, |connection| {
        let access = Access::open(connection, &bearer, now).await?;
        readable(connection, &access, recording_id, false).await?;
        text_of(connection, recording_id)
            .await?
            .ok_or_else(ApiError::not_found)
    })?;
    let disposition = format!("attachment; filename=\"{recording_id}.txt\"");
    let disposition =
        HeaderValue::from_str(&disposition).map_err(|error| ApiError::internal(&error))?;
    Ok((
        [
            (
                CONTENT_TYPE,
                HeaderValue::from_static("text/plain; charset=utf-8"),
            ),
            (CONTENT_DISPOSITION, disposition),
            (CACHE_CONTROL, HeaderValue::from_static("private, no-cache")),
            (X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff")),
        ],
        text,
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use junto_protocol::ids::RecordingSegmentId;

    use super::*;

    fn line(source: LineSource, speaker: Option<u32>, account_id: AccountId) -> TranscriptLine {
        TranscriptLine {
            line: 0,
            start_ms: 3_671_900,
            end_ms: 3_673_000,
            segment_id: RecordingSegmentId::generate(),
            source,
            account_id,
            speaker,
            text: "Thanks for joining.".to_owned(),
            language: "en".to_owned(),
            original_text: None,
            corrected_by: None,
            corrected_at: None,
        }
    }

    #[test]
    fn the_text_form_names_the_time_and_the_speaker_of_a_line() {
        let (carla, unknown) = (AccountId::generate(), AccountId::generate());
        let names = HashMap::from([(carla, "Carla".to_owned())]);
        let speakers = [
            TranscriptSpeaker {
                number: 1,
                account_id: carla,
                name: Some("Noor".to_owned()),
                renamed_by: Some(carla),
                renamed_at: None,
            },
            TranscriptSpeaker {
                number: 2,
                account_id: carla,
                name: None,
                renamed_by: None,
                renamed_at: None,
            },
        ];
        let written = |source, speaker, account_id| {
            text_line(&line(source, speaker, account_id), &names, &speakers)
        };
        assert_eq!(
            written(LineSource::Microphone, None, carla),
            "[01:01:11] Carla: Thanks for joining.\n"
        );
        assert_eq!(
            written(LineSource::Shared, Some(1), carla),
            "[01:01:11] Noor (shared audio of Carla): Thanks for joining.\n"
        );
        assert_eq!(
            written(LineSource::Shared, Some(2), carla),
            "[01:01:11] Speaker 2 (shared audio of Carla): Thanks for joining.\n"
        );
        assert_eq!(
            written(LineSource::Shared, None, carla),
            "[01:01:11] Shared audio of Carla: Thanks for joining.\n"
        );
        assert_eq!(
            written(LineSource::Microphone, None, unknown),
            format!("[01:01:11] {unknown}: Thanks for joining.\n")
        );
        assert_eq!(clock(0), "00:00:00");
        assert_eq!(clock(359_999_999), "99:59:59");
    }

    #[test]
    fn a_correction_is_trimmed_and_bounded() {
        let body = |text: &str| CorrectLine {
            text: text.to_owned(),
        };
        assert_eq!(
            correction(&body("  eleven seconds ")).unwrap(),
            "eleven seconds"
        );
        assert!(correction(&body("   ")).is_err());
        assert!(correction(&body("a\nb")).is_err());
        assert_eq!(
            correction(&body(&"é".repeat(LONGEST_CORRECTION)))
                .unwrap()
                .chars()
                .count(),
            LONGEST_CORRECTION
        );
        assert!(correction(&body(&"a".repeat(LONGEST_CORRECTION + 1))).is_err());
    }
}
