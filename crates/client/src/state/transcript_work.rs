//! What the application does to change the transcript of a recording (ADR 0616): it
//! corrects a line, names a voice of shared sound, and asks for the transcript again. The
//! change arrives with the state of the server: the events of a correction and of a name
//! make the client core read the line or the transcript again (ADR 0723).

use gpui::Context;
use junto_client_core::api::{ApiError, ErrorCategory};
use junto_client_core::flow::FlowError;
use junto_client_core::protocol::error::ErrorCode;
use junto_client_core::protocol::ids::{RecordingId, ServerId};

use super::AppState;
use crate::text;

/// The longest correction of a line, in characters (ADR 0607).
pub const LONGEST_CORRECTION: usize = 2000;
/// The longest name of a voice, in characters.
pub const LONGEST_NAME: usize = 64;

/// What the person changes in a transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TranscriptChange {
    /// The text of the line at this position.
    Line(u32),
    /// The name of the voice with the label `Speaker N`.
    Speaker(u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeState {
    Sending,
    /// Why the server did not do it.
    Refused(String),
}

/// A change of a transcript that the person sent. The server did the change when the
/// entry is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptAsk {
    pub what: TranscriptChange,
    pub state: ChangeState,
    /// The number of the request. An answer to an earlier request changes nothing.
    number: u64,
}

/// Why a text cannot be the correction of a line. The rules are those of the server.
pub fn correction_problem(text: &str) -> Option<&'static str> {
    let length = text.chars().count();
    if length == 0 {
        Some(text::transcripts::CORRECTION_EMPTY)
    } else if length > LONGEST_CORRECTION {
        Some(text::transcripts::CORRECTION_LONG)
    } else if text.chars().any(char::is_control) {
        Some(text::transcripts::CONTROL_CHARACTER)
    } else {
        None
    }
}

/// Why a text cannot be the name of a voice.
pub fn name_problem(name: &str) -> Option<&'static str> {
    let length = name.chars().count();
    if length == 0 {
        Some(text::transcripts::NAME_EMPTY)
    } else if length > LONGEST_NAME {
        Some(text::transcripts::NAME_LONG)
    } else if name.chars().any(char::is_control) {
        Some(text::transcripts::CONTROL_CHARACTER)
    } else {
        None
    }
}

/// The sentence of a refusal of a correction or of a name.
fn change_refusal(what: TranscriptChange, error: &ApiError) -> String {
    let fixed = match (error.code(), error.category()) {
        (Some(ErrorCode::LimitReached), _) => text::transcripts::CORRECTIONS_FULL,
        (Some(ErrorCode::InvalidRequest), _) => match what {
            TranscriptChange::Line(_) => text::transcripts::CORRECTION_REFUSED,
            TranscriptChange::Speaker(_) => text::transcripts::NAME_REFUSED,
        },
        // A recording of an archived channel takes no change (ADR 0410).
        (_, Some(ErrorCategory::Conflict)) => text::transcripts::ARCHIVED,
        (_, Some(ErrorCategory::NotAllowed)) => text::transcripts::CHANGE_NOT_ALLOWED,
        (_, Some(ErrorCategory::NotFound)) => text::transcripts::CHANGE_GONE,
        _ => return text::flow_error(&FlowError::from(error)),
    };
    fixed.to_owned()
}

/// The sentence of a refusal of a new request for a transcript.
fn request_refusal(error: &ApiError) -> String {
    let fixed = match error.category() {
        Some(ErrorCategory::NotAllowed) => text::transcripts::REQUEST_NOT_ALLOWED,
        Some(ErrorCategory::Conflict) => text::transcripts::REQUEST_CONFLICT,
        Some(ErrorCategory::NotFound) => text::RECORDING_GONE,
        _ => return text::flow_error(&FlowError::from(error)),
    };
    fixed.to_owned()
}

impl AppState {
    /// The recording is held open and belongs to a channel of the lists: an archived
    /// channel takes no change (ADR 0410).
    fn changeable_recording(&self, server_id: ServerId, recording_id: RecordingId) -> bool {
        self.client
            .server(server_id)
            .and_then(|server| server.live().view().model)
            .is_some_and(|model| {
                model
                    .recording(recording_id)
                    .and_then(|open| open.detail.loaded())
                    .is_some_and(|detail| model.channel(detail.recording.channel_id).is_some())
            })
    }

    /// The text of a line or the name of a voice as the state holds it now.
    fn shown_now(
        &self,
        server_id: ServerId,
        recording_id: RecordingId,
        what: TranscriptChange,
    ) -> Option<String> {
        let model = self.client.server(server_id)?.live().view().model?;
        let open = model.recording(recording_id)?;
        match what {
            TranscriptChange::Line(position) => open.line(position).map(|line| line.text.clone()),
            TranscriptChange::Speaker(number) => open.speaker(number)?.name.clone(),
        }
    }

    /// Sends a correction of a line or a name of a voice. Returns whether a request was
    /// sent, or why it was not: a text that the server would refuse, or one that changes
    /// nothing, sends nothing.
    pub fn change_transcript(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        what: TranscriptChange,
        typed: &str,
        cx: &mut Context<Self>,
    ) -> Result<bool, &'static str> {
        let typed = typed.trim().to_owned();
        let problem = match what {
            TranscriptChange::Line(_) => correction_problem(&typed),
            TranscriptChange::Speaker(_) => name_problem(&typed),
        };
        if let Some(problem) = problem {
            return Err(problem);
        }
        if !self.changeable_recording(server_id, recording_id) {
            return Err(text::transcripts::ARCHIVED);
        }
        if self.shown_now(server_id, recording_id, what).as_deref() == Some(typed.as_str()) {
            return Ok(false);
        }
        let Some(server) = self.client.server(server_id) else {
            return Err(text::transcripts::ARCHIVED);
        };
        let key = (server_id, recording_id);
        self.transcript_asks += 1;
        let number = self.transcript_asks;
        self.known.transcript_asks.insert(
            key,
            TranscriptAsk {
                what,
                state: ChangeState::Sending,
                number,
            },
        );
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let answer = match what {
                TranscriptChange::Line(position) => api
                    .correct_transcript_line(recording_id, position, &typed)
                    .await
                    .map(|_| ()),
                TranscriptChange::Speaker(label) => api
                    .rename_transcript_speaker(recording_id, label, &typed)
                    .await
                    .map(|_| ()),
            };
            let _ = this.update(cx, |this, cx| {
                // Only the answer to the newest request of the person changes what the
                // window tells.
                if this
                    .known
                    .transcript_asks
                    .get(&key)
                    .is_none_or(|held| held.number != number)
                {
                    return;
                }
                match answer {
                    Ok(()) => {
                        this.known.transcript_asks.remove(&key);
                    }
                    Err(error) => {
                        this.known.transcript_asks.insert(
                            key,
                            TranscriptAsk {
                                what,
                                state: ChangeState::Refused(change_refusal(what, &error)),
                                number,
                            },
                        );
                    }
                }
                this.changed(cx);
            });
        })
        .detach();
        self.changed(cx);
        Ok(true)
    }

    /// Puts away what the window tells about the last change of a transcript: the person
    /// closed its form. A request that is on its way still changes the transcript.
    pub fn dismiss_transcript_ask(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        cx: &mut Context<Self>,
    ) {
        if self
            .known
            .transcript_asks
            .remove(&(server_id, recording_id))
            .is_some()
        {
            self.changed(cx);
        }
    }

    /// Asks for the transcript of a recording again: the recording has no transcript, or
    /// its transcript failed (ADR 0607). The roles of the person predict "manage
    /// recordings"; the server decides. The new state arrives with the event of the
    /// recording.
    pub fn request_transcript(
        &mut self,
        server_id: ServerId,
        recording_id: RecordingId,
        cx: &mut Context<Self>,
    ) {
        if !self.changeable_recording(server_id, recording_id) {
            return;
        }
        let Some(server) = self.client.server(server_id) else {
            return;
        };
        let key = (server_id, recording_id);
        if !self.known.transcripts_requesting.insert(key) {
            return;
        }
        self.known.recording_refused.remove(&key);
        let api = server.api();
        cx.spawn(async move |this, cx| {
            let asked = api.request_transcript(recording_id).await;
            let _ = this.update(cx, |this, cx| {
                this.known.transcripts_requesting.remove(&key);
                if let Err(error) = asked {
                    this.known
                        .recording_refused
                        .insert(key, request_refusal(&error));
                }
                this.changed(cx);
            });
        })
        .detach();
        self.changed(cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_correction_and_a_name_follow_the_rules_of_the_server() {
        assert_eq!(
            correction_problem(""),
            Some(text::transcripts::CORRECTION_EMPTY)
        );
        assert_eq!(correction_problem(&"a".repeat(LONGEST_CORRECTION)), None);
        assert_eq!(
            correction_problem(&"a".repeat(LONGEST_CORRECTION + 1)),
            Some(text::transcripts::CORRECTION_LONG)
        );
        assert_eq!(
            correction_problem("two\nlines"),
            Some(text::transcripts::CONTROL_CHARACTER)
        );
        assert_eq!(name_problem(""), Some(text::transcripts::NAME_EMPTY));
        assert_eq!(name_problem(&"n".repeat(LONGEST_NAME)), None);
        assert_eq!(
            name_problem(&"n".repeat(LONGEST_NAME + 1)),
            Some(text::transcripts::NAME_LONG)
        );
        assert_eq!(
            name_problem("Noor\t"),
            Some(text::transcripts::CONTROL_CHARACTER)
        );
    }
}
