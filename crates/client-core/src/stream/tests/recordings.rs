//! Recordings and transcripts in the stream client, against routes that the test plays:
//! what a reader opens, what each event changes or reads, the pages of lines, a reload,
//! and answers that a server must not give.

use junto_protocol::ids::RecordingId;
use junto_protocol::recording::{RecordingDetail, RecordingState};
use junto_protocol::transcript::{Transcript, TranscriptLine, TranscriptLinePage, TranscriptState};

use super::messages::{json, not_found};
use super::*;
use crate::model::recording_tests::{
    corrected, detail, recording, removed, renamed, speaker, spoken, transcript, updated,
};
use crate::model::{LINES_PAGE, OpenRecording, ReadFailure};

/// The recordings as the played server holds them.
#[derive(Default)]
pub(super) struct Tape {
    details: Vec<RecordingDetail>,
    transcripts: Vec<Transcript>,
    lines: Vec<TranscriptLine>,
    /// A request for one of these recordings is answered with another recording.
    answered_wrongly: Vec<RecordingId>,
    /// The queries of the requests for lines, in their order.
    asked_lines: Vec<String>,
}

impl Tape {
    /// The answer to a request of a recording route. `None` for another route.
    pub(super) fn answer(
        &mut self,
        request: &HttpRequest,
    ) -> Option<Result<HttpResponse, TransportError>> {
        let rest = request.url.path().strip_prefix("/api/v1/recordings/")?;
        let (id, route) = rest.split_once('/').unwrap_or((rest, ""));
        let id: RecordingId = id.parse().unwrap();
        let wrong = self.answered_wrongly.contains(&id);
        let held = |of: RecordingId| (of == id) != wrong;
        Some(match route {
            "" => match self.details.iter().find(|read| held(read.recording.id)) {
                Some(read) => json(200, read),
                None => not_found(),
            },
            "transcript" => match self
                .transcripts
                .iter()
                .find(|header| held(header.recording_id))
            {
                Some(header) => json(200, header),
                None => not_found(),
            },
            "transcript/lines" => {
                self.asked_lines
                    .push(request.url.query().unwrap_or_default().to_owned());
                let value = |name: &str| {
                    request
                        .url
                        .query_pairs()
                        .find(|(key, _)| key == name)
                        .map(|(_, value)| value.parse::<u32>().unwrap())
                };
                let after = value("after");
                let limit = value("limit").unwrap_or(200) as usize;
                let rest: Vec<TranscriptLine> = self
                    .lines
                    .iter()
                    .filter(|held| after.is_none_or(|after| held.line > after))
                    .cloned()
                    .collect();
                let lines: Vec<TranscriptLine> = rest.iter().take(limit).cloned().collect();
                let next_after = (rest.len() > limit)
                    .then(|| lines.last().map(|last| last.line))
                    .flatten();
                json(200, &TranscriptLinePage { lines, next_after })
            }
            other => panic!("the stream client asked for the route {other} of a recording"),
        })
    }
}

fn tape<R>(rig: &Rig, act: impl FnOnce(&mut Tape) -> R) -> R {
    act(&mut rig.http.routes.lock().unwrap().tape)
}

/// A rig whose played server holds one recording of the voice channel, with a transcript
/// of `stated` lines.
fn recorded(stated: u32) -> (Rig, RecordingDetail) {
    let rig = rig();
    let read = detail(recording(RecordingId::generate(), rig.scene.daily, stated));
    tape(&rig, |tape| {
        tape.details.push(read.clone());
        tape.transcripts.push(transcript(&read.recording));
        tape.lines = spoken(0..stated);
    });
    (rig, read)
}

async fn opened(rig: &Rig, id: RecordingId) -> Arc<ServerModel> {
    soon("the recording", rig.live.open_recording(id))
        .await
        .unwrap();
    rig.model()
}

fn open(model: &ServerModel, id: RecordingId) -> &OpenRecording {
    model.recording(id).unwrap()
}

/// The requests for the recording and for its transcript that the client sent.
fn reads(rig: &Rig, id: RecordingId) -> (usize, usize) {
    (
        rig.http.count(&format!("/recordings/{id}")),
        rig.http.count(&format!("/recordings/{id}/transcript")),
    )
}

fn line_reads(rig: &Rig) -> usize {
    rig.http.count("/transcript/lines")
}

async fn recording_when(
    rig: &Rig,
    id: RecordingId,
    what: &str,
    mut holds: impl FnMut(&OpenRecording) -> bool,
) -> Arc<ServerModel> {
    rig.model_when(what, |model| {
        model.recording(id).is_some_and(|open| holds(open))
    })
    .await
}

#[tokio::test(start_paused = true)]
async fn a_recording_opens_with_its_segments_and_its_transcript_and_follows_its_events() {
    let (mut rig, read) = recorded(0);
    let id = read.recording.id;
    tape(&rig, |tape| {
        tape.transcripts[0].speakers = vec![speaker(1, None)]
    });
    // Without a connection there is nothing to read from.
    let off = soon("the answer", rig.live.open_recording(id)).await;
    assert!(matches!(off, Err(LoadError::Off)));

    // A reader that asks before the state has arrived is answered when the recording is
    // in the state.
    let gate = rig.http.hold("/snapshot");
    rig.want(ConnectionLevel::Selected, "jst_one");
    rig.when("a connection attempt", |view| {
        view.status == StreamStatus::Connecting
    })
    .await;
    let early = rig.live.open_recording(id);
    gate.add_permits(8);
    let (peer, _, _) = rig.served().await;
    soon("the recording", early).await.unwrap();
    let model = rig.model();
    assert_eq!(open(&model, id).detail.loaded(), Some(&read));
    let header = tape(&rig, |tape| tape.transcripts[0].clone());
    assert_eq!(open(&model, id).transcript.loaded(), Some(&header));
    assert_eq!(reads(&rig, id), (1, 1));
    // A second reader of a loaded recording costs no request.
    opened(&rig, id).await;
    assert_eq!(reads(&rig, id), (1, 1));

    // The transcript is built. The event carries the recording without its segments and
    // without its transcript: both routes are read again.
    let mut ready = read.recording.clone();
    ready.transcript = TranscriptState::Ready;
    ready.transcript_lines = 2;
    tape(&rig, |tape| {
        tape.details[0].recording = ready.clone();
        tape.transcripts[0].state = TranscriptState::Ready;
        tape.transcripts[0].lines = 2;
    });
    peer.event(41, updated(&ready)).await;
    let model = recording_when(&rig, id, "the transcript", |open| {
        open.transcript
            .loaded()
            .is_some_and(|header| header.state == TranscriptState::Ready)
    })
    .await;
    assert_eq!(open(&model, id).detail.loaded().unwrap().recording, ready);
    // The transcript states lines that the state does not hold.
    assert!(open(&model, id).lines.more);
    assert_eq!(reads(&rig, id), (2, 2));

    // A rename of a speaker changes the loaded label without a request.
    let named = speaker(1, Some("Noor"));
    peer.event(42, renamed(&ready, &named)).await;
    recording_when(&rig, id, "the name", |open| open.speaker(1) == Some(&named)).await;
    assert_eq!(reads(&rig, id), (2, 2));

    // One of the two readers releases the recording: it stays for the other one.
    rig.live.release_recording(id);
    peer.event(43, removed(&ready)).await;
    let model = recording_when(&rig, id, "the removal", |open| open.gone).await;
    assert_eq!(open(&model, id).detail, Loadable::NotLoaded);
    assert_eq!(reads(&rig, id), (2, 2));
    rig.live.release_recording(id);
    rig.model_when("no recording", |model| model.recording(id).is_none())
        .await;
}

#[tokio::test(start_paused = true)]
async fn the_lines_are_read_in_pages_and_a_correction_changes_a_loaded_line() {
    let (mut rig, read) = recorded(450);
    let id = read.recording.id;
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let model = opened(&rig, id).await;
    // Opening reads no line.
    assert!(open(&model, id).lines.more && !open(&model, id).lines.loaded);
    assert_eq!(line_reads(&rig), 0);

    rig.live.read_transcript_lines(id);
    let model = recording_when(&rig, id, "the first page", |open| open.lines.loaded).await;
    let lines = &open(&model, id).lines;
    assert_eq!(lines.lines.len(), LINES_PAGE);
    assert!(lines.more && !lines.reading);

    // A correction of a loaded line needs no request.
    let ada = AccountId::generate();
    peer.event(
        41,
        corrected(&read.recording, 5, "eleven seconds", ada, time()),
    )
    .await;
    let model = recording_when(&rig, id, "the correction", |open| {
        open.line(5)
            .is_some_and(|line| line.text == "eleven seconds")
    })
    .await;
    let fixed = open(&model, id).line(5).unwrap();
    assert_eq!(fixed.original_text.as_deref(), Some("line 5"));
    assert_eq!(fixed.corrected_by, Some(ada));
    assert_eq!(line_reads(&rig), 1);

    // A correction that arrives while the next page is read: the answer is older than
    // the event, and the event is applied to it.
    let gate = rig.http.hold("/transcript/lines");
    rig.live.read_transcript_lines(id);
    recording_when(&rig, id, "the read of the page", |open| open.lines.reading).await;
    soon("the request", async {
        while line_reads(&rig) < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    peer.event(
        42,
        corrected(&read.recording, 300, "said later", ada, time()),
    )
    .await;
    rig.model_when("the event", |model| model.cursor.position == 42)
        .await;
    gate.add_permits(8);
    let model = recording_when(&rig, id, "the second page", |open| {
        open.lines.lines.len() == 2 * LINES_PAGE
    })
    .await;
    assert_eq!(open(&model, id).line(300).unwrap().text, "said later");
    assert_eq!(
        tape(&rig, |tape| tape.asked_lines.clone()),
        ["limit=200", "after=199&limit=200"]
    );

    // The last page, and no request after it.
    rig.live.read_transcript_lines(id);
    let model = recording_when(&rig, id, "the last page", |open| !open.lines.more).await;
    assert_eq!(open(&model, id).lines.lines.len(), 450);
    rig.live.read_transcript_lines(id);
    tokio::time::sleep(Duration::from_secs(60)).await;
    assert_eq!(line_reads(&rig), 3);
}

#[tokio::test(start_paused = true)]
async fn a_reload_reads_the_open_recordings_and_their_lines_again() {
    let (mut rig, read) = recorded(450);
    let id = read.recording.id;
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    opened(&rig, id).await;
    for held in [200, 400] {
        rig.live.read_transcript_lines(id);
        recording_when(&rig, id, "a page", |open| open.lines.lines.len() == held).await;
    }
    // A recording that its reader released is not read again.
    let other = detail(recording(RecordingId::generate(), rig.scene.daily, 0));
    tape(&rig, |tape| {
        tape.details.push(other.clone());
        tape.transcripts.push(transcript(&other.recording));
    });
    opened(&rig, other.recording.id).await;
    rig.live.release_recording(other.recording.id);

    // The server changed a line and the recording while the client did not follow.
    tape(&rig, |tape| {
        tape.lines[7].text = "changed meanwhile".to_owned();
        tape.details[0].recording.state = RecordingState::PartlyFailed;
    });
    let gate = rig.http.hold("/transcript");
    rig.set_snapshot(90, |_| {});
    peer.send(&ServerMessage::Resync {
        reason: ResyncReason::TooFarBehind,
    })
    .await;
    // What the reader had stays for display, with the mark that it is not current.
    let model = recording_when(&rig, id, "the kept recording", |open| open.stale).await;
    assert_eq!(model.cursor.position, 90);
    assert_eq!(open(&model, id).detail.loaded(), Some(&read));
    assert_eq!(open(&model, id).lines.lines.len(), 400);

    gate.add_permits(8);
    let model = recording_when(&rig, id, "the recording after the reload", |open| {
        !open.stale && !open.lines.stale
    })
    .await;
    let held = open(&model, id);
    let state = held.detail.loaded().unwrap().recording.state;
    assert_eq!(state, RecordingState::PartlyFailed);
    // As many lines as were held, in one request.
    assert_eq!(held.lines.lines.len(), 400);
    assert!(held.lines.more);
    assert_eq!(held.line(7).unwrap().text, "changed meanwhile");
    assert_eq!(reads(&rig, id), (2, 2));
    assert_eq!(
        tape(&rig, |tape| tape.asked_lines.last().cloned()).as_deref(),
        Some("limit=400")
    );
    assert_eq!(reads(&rig, other.recording.id), (1, 1));
    assert!(model.recording(other.recording.id).is_none());
}

#[tokio::test(start_paused = true)]
async fn an_answer_for_another_recording_is_not_taken_and_not_asked_for_again() {
    let (mut rig, other) = recorded(0);
    let id = RecordingId::generate();
    tape(&rig, |tape| tape.answered_wrongly.push(id));
    rig.want(ConnectionLevel::Selected, "jst_one");
    rig.served().await;

    let answered = soon("the answer", rig.live.open_recording(id)).await;
    assert!(matches!(answered, Err(LoadError::Request(_))));
    let model = rig.model();
    let held = open(&model, id);
    assert_eq!(held.failed, Some(ReadFailure::NoAnswer));
    assert_eq!(held.detail, Loadable::NotLoaded);
    assert_eq!(held.transcript, Loadable::NotLoaded);
    assert!(!held.gone);
    assert!(model.recording(other.recording.id).is_none());
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert_eq!(reads(&rig, id), (1, 1));
}

#[tokio::test(start_paused = true)]
async fn a_recording_that_the_server_does_not_give_is_gone_and_a_failed_read_is_sent_again() {
    let (mut rig, read) = recorded(0);
    let id = read.recording.id;
    rig.want(ConnectionLevel::Selected, "jst_one");
    rig.served().await;

    // The server has no such recording for the account.
    let unknown = RecordingId::generate();
    let answered = soon("the answer", rig.live.open_recording(unknown)).await;
    assert!(matches!(answered, Err(LoadError::Request(_))));
    let model = rig.model();
    assert!(open(&model, unknown).gone);
    assert_eq!(open(&model, unknown).failed, None);
    tokio::time::sleep(Duration::from_secs(600)).await;
    assert_eq!(reads(&rig, unknown).0, 1);

    // The server fails once. The reader is told at once, and the client reads again by
    // itself: no event tells a change a second time.
    rig.http.fail("/transcript", 503, "unavailable");
    let answered = soon("the answer", rig.live.open_recording(id)).await;
    assert!(matches!(answered, Err(LoadError::Request(_))));
    let model = rig.model();
    assert_eq!(
        open(&model, id).failed,
        Some(ReadFailure::Refused(ErrorCode::Unavailable))
    );
    assert_eq!(open(&model, id).detail, Loadable::NotLoaded);
    let model = recording_when(&rig, id, "the second read", |open| {
        open.detail.loaded().is_some()
    })
    .await;
    assert_eq!(open(&model, id).failed, None);
    assert_eq!(reads(&rig, id), (2, 2));

    // A page of lines that is refused is read when the reader asks again.
    tape(&rig, |tape| tape.lines = spoken(0..3));
    rig.http.fail("/transcript/lines", 403, "forbidden");
    rig.live.read_transcript_lines(id);
    let model = recording_when(&rig, id, "the refusal", |open| open.lines.failed.is_some()).await;
    assert_eq!(
        open(&model, id).lines.failed,
        Some(ReadFailure::Refused(ErrorCode::Forbidden))
    );
    rig.live.read_transcript_lines(id);
    let model = recording_when(&rig, id, "the lines", |open| open.lines.loaded).await;
    assert_eq!(open(&model, id).lines.lines.len(), 3);
    assert_eq!(open(&model, id).lines.failed, None);
}

#[tokio::test(start_paused = true)]
async fn the_recordings_that_readers_open_at_once_are_read_a_few_at_a_time() {
    let mut rig = rig();
    let ids: Vec<RecordingId> = (0..10).map(|_| RecordingId::generate()).collect();
    tape(&rig, |tape| {
        for id in &ids {
            let read = detail(recording(*id, rig.scene.daily, 0));
            tape.transcripts.push(transcript(&read.recording));
            tape.details.push(read);
        }
    });
    rig.want(ConnectionLevel::Selected, "jst_one");
    rig.served().await;

    // No request of a recording is answered yet.
    let gate = rig.http.hold("/recordings/");
    let opening: Vec<_> = ids.iter().map(|id| rig.live.open_recording(*id)).collect();
    let asked = |rig: &Rig| {
        let routes = rig.http.routes.lock().unwrap();
        routes
            .seen
            .iter()
            .filter(|(path, _)| path.contains("/recordings/"))
            .count()
    };
    soon("the first requests", async {
        while asked(&rig) < 8 {
            tokio::task::yield_now().await;
        }
    })
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    // Four recordings are on their way, each with its two routes.
    assert_eq!(asked(&rig), 8);

    gate.add_permits(64);
    for opening in opening {
        soon("a recording", opening).await.unwrap();
    }
    let model = rig.model();
    assert!(
        ids.iter()
            .all(|id| open(&model, *id).detail.loaded().is_some())
    );
    assert_eq!(asked(&rig), 20);
}
