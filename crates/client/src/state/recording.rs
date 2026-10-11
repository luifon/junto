//! What the application tells about the recording of the call of this device beyond the
//! state of its server (ADR 0738): how the last recording ended, and the requests of this
//! device that start or stop one.
//!
//! The answers of the requests and the events of the server arrive on two connections, in
//! no fixed order. Each statement here follows from one of them only.

use junto_client_core::protocol::events::Event;
use junto_client_core::protocol::ids::{CallId, ServerId};
use junto_client_core::protocol::recording::Recording;

use super::view::is_recording_problem;

#[derive(Debug, Default)]
pub struct RecordingTold {
    /// The last recording of the call that failed or that the server stopped, with the
    /// server of the call.
    problem: Option<(ServerId, Box<Recording>)>,
    /// Why the last request failed.
    request_failed: Option<String>,
    /// The request that is on its way.
    asking: Option<u64>,
    requests: u64,
    /// The place of this device in the call that the statements are about: the identity
    /// that the server gave for it.
    place: Option<String>,
}

impl RecordingTold {
    pub fn problem(&self) -> Option<&(ServerId, Box<Recording>)> {
        self.problem.as_ref()
    }

    pub fn request_failed(&self) -> Option<&str> {
        self.request_failed.as_deref()
    }

    pub fn busy(&self) -> bool {
        self.asking.is_some()
    }

    /// The person asks for a start or a stop. Returns the number of the request, or
    /// nothing while another request is on its way. A request for a new recording puts
    /// away what was told about the one before.
    pub fn request(&mut self, start: bool) -> Option<u64> {
        if self.asking.is_some() {
            return None;
        }
        self.requests += 1;
        self.asking = Some(self.requests);
        self.request_failed = None;
        if start {
            self.problem = None;
        }
        Some(self.requests)
    }

    /// The server answered a request. An answer that belongs to an earlier call, whose
    /// statements have ended with it, tells nothing. An answer never changes what an
    /// event told: the failure of the recording that this request started can arrive
    /// before the answer.
    pub fn answered(&mut self, request: u64, result: Result<(), String>) {
        if self.asking != Some(request) {
            return;
        }
        self.asking = None;
        self.request_failed = result.err();
    }

    /// An event of a server. `own_call`: the call of this device on that server.
    pub fn event(&mut self, server_id: ServerId, own_call: Option<CallId>, event: &Event) {
        match event {
            Event::RecordingStarted { recording } if Some(recording.call_id) == own_call => {
                self.problem = None;
            }
            Event::RecordingUpdated { recording }
                if Some(recording.call_id) == own_call
                    && !recording.state.is_active()
                    && is_recording_problem(recording) =>
            {
                self.problem = Some((server_id, recording.clone()));
            }
            _ => {}
        }
    }

    /// The person put the statement about a refused request away.
    pub fn dismiss_request_failure(&mut self) {
        self.request_failed = None;
    }

    /// The place of this device in a call now, or nothing without a call. What was told
    /// about the recording of a call ends with the place in it: a request of that call
    /// that is still on its way tells nothing when it is answered.
    pub fn in_call(&mut self, place: Option<&str>) {
        if self.place.as_deref() != place {
            self.place = place.map(str::to_owned);
            self.problem = None;
            self.request_failed = None;
            self.asking = None;
        }
    }

    /// Draws this state in place of what the servers told. The screen tool uses it.
    #[cfg(feature = "test-support")]
    pub fn show(&mut self, problem: Option<(ServerId, Recording)>, request_failed: Option<String>) {
        self.problem = problem.map(|(server_id, recording)| (server_id, Box::new(recording)));
        self.request_failed = request_failed;
    }
}

#[cfg(test)]
mod tests {
    use junto_client_core::protocol::ids::{AccountId, ChannelId, RecordingId, WorkspaceId};
    use junto_client_core::protocol::recording::{
        RecordingFailure, RecordingState, RecordingStop, RecordingStopReason,
    };
    use junto_client_core::protocol::transcript::TranscriptState;
    use time::OffsetDateTime;

    use super::*;

    fn recording(call_id: CallId, state: RecordingState) -> Box<Recording> {
        let at = OffsetDateTime::from_unix_timestamp(1_800_000_000).unwrap();
        Box::new(Recording {
            id: RecordingId::generate(),
            workspace_id: WorkspaceId::generate(),
            channel_id: ChannelId::generate(),
            call_id,
            state,
            started_by: AccountId::generate(),
            requested_at: at,
            recording_since: None,
            stop: None,
            ended_at: None,
            failure: None,
            language: "auto".to_owned(),
            segments: 0,
            failed_segments: 0,
            bytes: None,
            timeline_started_at: None,
            transcript: TranscriptState::None,
            transcript_lines: 0,
        })
    }

    fn failed(call_id: CallId) -> Event {
        let mut recording = recording(call_id, RecordingState::Failed);
        recording.failure = Some(RecordingFailure::RecorderUnavailable);
        Event::RecordingUpdated { recording }
    }

    #[test]
    fn the_answer_to_a_start_does_not_put_away_a_failure_that_an_event_told_before_it() {
        let (server_id, call_id) = (ServerId::generate(), CallId::generate());
        let mut told = RecordingTold::default();
        let request = told.request(true).unwrap();
        assert!(told.busy());
        // The recorder took nothing: the event arrives before the answer of the request.
        told.event(server_id, Some(call_id), &failed(call_id));
        assert!(told.problem().is_some());
        told.answered(request, Ok(()));
        assert!(!told.busy());
        assert!(told.problem().is_some(), "the answer erased the failure");
        assert_eq!(told.request_failed(), None);
    }

    #[test]
    fn one_request_at_a_time_and_a_new_start_puts_away_the_recording_before() {
        let (server_id, call_id) = (ServerId::generate(), CallId::generate());
        let mut told = RecordingTold::default();
        told.event(server_id, Some(call_id), &failed(call_id));
        // A stop keeps what was told; a second request waits for the first.
        let stop = told.request(false).unwrap();
        assert_eq!(told.request(true), None);
        assert!(told.problem().is_some());
        told.answered(stop, Err("No.".to_owned()));
        assert_eq!(told.request_failed(), Some("No."));
        // The person asks for a new recording: the failure and the refusal are put away.
        let start = told.request(true).unwrap();
        assert_ne!(start, stop);
        assert_eq!(
            (told.problem().is_none(), told.request_failed()),
            (true, None)
        );
        told.answered(start, Ok(()));
        assert!(!told.busy());
    }

    #[test]
    fn the_answer_of_a_request_of_an_earlier_call_tells_nothing() {
        let mut told = RecordingTold::default();
        told.in_call(Some("first"));
        let before = told.request(true).unwrap();
        // The device is in another call at once: the state of the calls can pass from one
        // call to the next without a moment without a call.
        told.in_call(Some("second"));
        assert!(!told.busy());
        // The person is in another call when the server answers.
        told.answered(before, Err("The server has no recorder.".to_owned()));
        assert_eq!(told.request_failed(), None);
        let now = told.request(true).unwrap();
        told.answered(before, Err("late".to_owned()));
        assert!(told.busy());
        told.answered(now, Ok(()));
        assert!(!told.busy());
    }

    #[test]
    fn only_the_end_of_a_recording_of_the_own_call_with_a_problem_is_kept() {
        let (server_id, call_id) = (ServerId::generate(), CallId::generate());
        let mut told = RecordingTold::default();
        // Another call, and no call of this device.
        told.event(server_id, Some(CallId::generate()), &failed(call_id));
        told.event(server_id, None, &failed(call_id));
        assert!(told.problem().is_none());
        // A recording that still runs, and one that a person stopped.
        let running = Event::RecordingUpdated {
            recording: recording(call_id, RecordingState::Recording),
        };
        told.event(server_id, Some(call_id), &running);
        let mut stopped = recording(call_id, RecordingState::Processing);
        stopped.stop = Some(RecordingStop {
            reason: RecordingStopReason::Stopped,
            by: None,
            at: stopped.requested_at,
        });
        told.event(
            server_id,
            Some(call_id),
            &Event::RecordingUpdated { recording: stopped },
        );
        assert!(told.problem().is_none());
        // The recorder was lost.
        let mut lost = recording(call_id, RecordingState::Processing);
        lost.stop = Some(RecordingStop {
            reason: RecordingStopReason::RecorderLost,
            by: None,
            at: lost.requested_at,
        });
        told.event(
            server_id,
            Some(call_id),
            &Event::RecordingUpdated { recording: lost },
        );
        assert_eq!(told.problem().map(|(of, _)| *of), Some(server_id));
        // A new recording of the call, and the end of the call, put it away.
        let started = Event::RecordingStarted {
            recording: recording(call_id, RecordingState::Starting),
        };
        told.event(server_id, Some(call_id), &started);
        assert!(told.problem().is_none());
        told.in_call(Some("place"));
        told.event(server_id, Some(call_id), &failed(call_id));
        // The same place again changes nothing; the end of the call puts it away.
        told.in_call(Some("place"));
        assert!(told.problem().is_some());
        told.in_call(None);
        assert!(told.problem().is_none());
    }
}
