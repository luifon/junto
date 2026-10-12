use std::sync::Arc;

use junto_protocol::error::ErrorCode;
use junto_protocol::events::Event;
use junto_protocol::ids::{
    AccountId, CallId, ChannelId, RecordingId, RecordingSegmentId, WorkspaceId,
};
use junto_protocol::recording::{
    FileCheck, Recording, RecordingDetail, RecordingSegment, RecordingState, SegmentFile,
    SegmentSource, SegmentState,
};
use junto_protocol::transcript::{
    GlossaryUse, LineSource, SegmentTranscription, SpeakerSeparation, Transcript, TranscriptLine,
    TranscriptSegment, TranscriptSpeaker, TranscriptState,
};
use time::OffsetDateTime;

use super::tests::{Scene, scene, time};
use super::*;

/// A recording whose files are processed and whose transcript has `lines` lines.
pub(crate) fn recording(id: RecordingId, channel_id: ChannelId, lines: u32) -> Recording {
    Recording {
        id,
        workspace_id: WorkspaceId::generate(),
        channel_id,
        call_id: CallId::generate(),
        state: RecordingState::Ready,
        started_by: AccountId::generate(),
        requested_at: time(),
        recording_since: Some(time()),
        stop: None,
        ended_at: Some(time()),
        failure: None,
        language: "auto".to_owned(),
        segments: 2,
        failed_segments: 0,
        bytes: Some(3_000),
        timeline_started_at: Some(time()),
        transcript: if lines > 0 {
            TranscriptState::Ready
        } else {
            TranscriptState::None
        },
        transcript_lines: lines,
    }
}

/// A segment that ended in order, with a file of `bytes` bytes.
pub(crate) fn segment(source: SegmentSource, bytes: u64) -> RecordingSegment {
    let video = source == SegmentSource::Composite;
    RecordingSegment {
        id: RecordingSegmentId::generate(),
        source,
        account_id: (!video).then(AccountId::generate),
        state: SegmentState::Ended,
        failure: None,
        requested_at: time(),
        started_at: Some(time()),
        ended_at: Some(time()),
        duration_ms: Some(2_000),
        file: Some(SegmentFile {
            media_type: if video { "video/mp4" } else { "audio/ogg" }.to_owned(),
            bytes,
            duration_ms: 2_000,
            starts_at: Some(time()),
            width: video.then_some(320),
            height: video.then_some(180),
            check: FileCheck::Passed,
            problems: Vec::new(),
            silent: false,
            recovered: false,
        }),
    }
}

/// A recording with its video and one audio track.
pub(crate) fn detail(recording: Recording) -> RecordingDetail {
    RecordingDetail {
        recording,
        segments: vec![
            segment(SegmentSource::Composite, 2_000),
            segment(SegmentSource::Microphone, 1_000),
        ],
    }
}

/// The transcript that a recording states, without a speaker label.
pub(crate) fn transcript(recording: &Recording) -> Transcript {
    Transcript {
        recording_id: recording.id,
        state: recording.transcript,
        failure: None,
        language: "auto".to_owned(),
        lines: recording.transcript_lines,
        speakers: Vec::new(),
        glossary: GlossaryUse::default(),
        segments: Vec::new(),
    }
}

pub(crate) fn line(position: u32, text: &str) -> TranscriptLine {
    TranscriptLine {
        line: position,
        start_ms: u64::from(position) * 1_000,
        end_ms: u64::from(position) * 1_000 + 900,
        segment_id: RecordingSegmentId::generate(),
        source: LineSource::Microphone,
        account_id: AccountId::generate(),
        speaker: None,
        text: text.to_owned(),
        language: "en".to_owned(),
        original_text: None,
        corrected_by: None,
        corrected_at: None,
    }
}

pub(crate) fn speaker(number: u32, name: Option<&str>) -> TranscriptSpeaker {
    TranscriptSpeaker {
        number,
        account_id: AccountId::generate(),
        name: name.map(str::to_owned),
        renamed_by: name.map(|_| AccountId::generate()),
        renamed_at: name.map(|_| time()),
    }
}

/// The lines at the positions of `range`, each with the text `line <position>`.
pub(crate) fn spoken(range: std::ops::Range<u32>) -> Vec<TranscriptLine> {
    range
        .map(|position| line(position, &format!("line {position}")))
        .collect()
}

pub(crate) fn updated(recording: &Recording) -> Event {
    Event::RecordingUpdated {
        recording: Box::new(recording.clone()),
    }
}

pub(crate) fn removed(recording: &Recording) -> Event {
    Event::RecordingRemoved {
        recording_id: recording.id,
        workspace_id: recording.workspace_id,
        channel_id: recording.channel_id,
    }
}

pub(crate) fn corrected(
    recording: &Recording,
    position: u32,
    text: &str,
    by: AccountId,
    at: OffsetDateTime,
) -> Event {
    Event::TranscriptLineCorrected {
        recording_id: recording.id,
        workspace_id: recording.workspace_id,
        channel_id: recording.channel_id,
        line: position,
        text: text.to_owned(),
        corrected_by: by,
        corrected_at: at,
    }
}

pub(crate) fn renamed(recording: &Recording, speaker: &TranscriptSpeaker) -> Event {
    Event::TranscriptSpeakerRenamed {
        recording_id: recording.id,
        workspace_id: recording.workspace_id,
        channel_id: recording.channel_id,
        speaker: speaker.clone(),
    }
}

fn first_page(count: usize) -> LinesAsk {
    LinesAsk {
        after: None,
        count,
        replace: true,
    }
}

fn answer(ask: LinesAsk, range: std::ops::Range<u32>, more: bool) -> LinesRead {
    LinesRead {
        ask,
        lines: spoken(range),
        more,
    }
}

/// A model in which a reader has a recording of the voice channel open, with the answers
/// of its routes in the state. Its transcript states `stated` lines.
fn opened(scene: &Scene, stated: u32) -> (ServerModel, RecordingDetail) {
    let mut model = scene.model();
    let id = RecordingId::generate();
    let held = detail(recording(id, scene.daily, stated));
    model.open_recording(id);
    assert!(model.set_recording(id, held.clone(), transcript(&held.recording)));
    (model, held)
}

fn held(model: &ServerModel, id: RecordingId) -> &OpenRecording {
    model.recording(id).unwrap()
}

fn positions(model: &ServerModel, id: RecordingId) -> Vec<u32> {
    held(model, id)
        .lines
        .lines
        .iter()
        .map(|line| line.line)
        .collect()
}

#[test]
fn a_reader_opens_a_recording_and_the_answers_of_its_routes_fill_it() {
    let scene = scene();
    let mut model = scene.model();
    let id = RecordingId::generate();
    assert!(model.recording(id).is_none());

    model.open_recording(id);
    model.open_recording(id);
    assert_eq!(model.recordings.len(), 1);
    let open = held(&model, id);
    assert_eq!(open.detail, Loadable::NotLoaded);
    assert_eq!(open.transcript, Loadable::NotLoaded);
    model.set_recording_reading(id);
    let open = held(&model, id);
    assert_eq!(open.detail, Loadable::Loading);
    assert_eq!(open.transcript, Loadable::Loading);

    let read = detail(recording(id, scene.daily, 3));
    let header = transcript(&read.recording);
    assert!(model.set_recording(id, read.clone(), header.clone()));
    let open = held(&model, id);
    assert_eq!(open.detail.loaded(), Some(&read));
    assert_eq!(open.transcript.loaded(), Some(&header));
    assert!(!open.gone && !open.stale && open.failed.is_none());
    // The transcript states three lines and the state holds none: a reader asks for them.
    assert!(open.lines.more && !open.lines.loaded && open.lines.lines.is_empty());
    // A read that is sent again does not hide what is loaded.
    model.set_recording_reading(id);
    assert_eq!(held(&model, id).detail.loaded(), Some(&read));

    model.close_recording(id);
    assert!(model.recording(id).is_none());
}

#[test]
fn an_answer_for_another_recording_is_not_taken() {
    let scene = scene();
    let mut model = scene.model();
    let id = RecordingId::generate();
    model.open_recording(id);
    model.set_recording_reading(id);
    let own = detail(recording(id, scene.daily, 0));
    let other = detail(recording(RecordingId::generate(), scene.daily, 0));

    let foreign = transcript(&other.recording);
    assert!(!model.set_recording(id, other.clone(), foreign.clone()));
    // One of the two answers is of the recording, and the other is not.
    assert!(!model.set_recording(id, own.clone(), foreign));
    assert!(!model.set_recording(id, other, transcript(&own.recording)));
    assert_eq!(held(&model, id).detail, Loadable::Loading);
    assert_eq!(held(&model, id).transcript, Loadable::Loading);
}

#[test]
fn what_a_server_can_make_grow_is_cut_at_its_bound() {
    let scene = scene();
    let mut model = scene.model();
    let id = RecordingId::generate();
    model.open_recording(id);
    let mut read = detail(recording(id, scene.daily, 0));
    let track = segment(SegmentSource::Microphone, 1);
    read.segments = vec![track.clone(); MOST_SEGMENTS + 5];
    let mut header = transcript(&read.recording);
    header.speakers = (1..=u32::try_from(MOST_SPEAKERS).unwrap() + 5)
        .map(|number| speaker(number, None))
        .collect();
    header.glossary.used = vec!["Quorvex".to_owned(); 300];
    header.glossary.cut = vec!["Lumitrax".to_owned(); 300];
    header.segments = vec![
        TranscriptSegment {
            segment_id: track.id,
            status: SegmentTranscription::Ok,
            reason: None,
            audio_ms: Some(2_000),
            processed_ms: Some(2_000),
            lines: 1,
            speakers: SpeakerSeparation::NotApplicable,
            speakers_reason: None,
        };
        MOST_SEGMENTS + 5
    ];
    assert!(model.set_recording(id, read.clone(), header));
    let open = held(&model, id);
    assert_eq!(open.detail.loaded().unwrap().segments.len(), MOST_SEGMENTS);
    let stored = open.transcript.loaded().unwrap();
    assert_eq!(stored.segments.len(), MOST_SEGMENTS);
    assert_eq!(stored.speakers.len(), MOST_SPEAKERS);
    assert_eq!(
        (stored.glossary.used.len(), stored.glossary.cut.len()),
        (200, 200)
    );

    // A rename of a label that the full list does not hold adds nothing.
    let further = speaker(u32::try_from(MOST_SPEAKERS).unwrap() + 9, Some("Noor"));
    model.apply(&renamed(&read.recording, &further));
    let stored = held(&model, id).transcript.loaded().unwrap();
    assert_eq!(stored.speakers.len(), MOST_SPEAKERS);

    // The lines of one answer, and of a page after the bound.
    let most = u32::try_from(MOST_LINES).unwrap();
    model.add_transcript_lines(id, answer(first_page(MOST_LINES + 10), 0..most + 10, true));
    let open = held(&model, id);
    assert_eq!(open.lines.lines.len(), MOST_LINES);
    assert!(!open.lines.more);
    assert_eq!(model.lines_ask(id), None);
}

#[test]
fn recording_updated_replaces_the_recording_and_asks_for_its_routes() {
    let scene = scene();
    let (mut model, read) = opened(&scene, 0);
    let id = read.recording.id;
    let asks = |part| Follow {
        reload: false,
        load: vec![part],
    };

    // The transcript of the recording was asked for: the event carries the recording
    // without its segments and without its transcript.
    let mut changed = read.recording.clone();
    changed.transcript = TranscriptState::Waiting;
    let event = updated(&changed);
    assert_eq!(model.apply(&event), asks(Part::Recording(id)));
    let stored = held(&model, id).detail.loaded().unwrap();
    assert_eq!(stored.recording, changed);
    assert_eq!(stored.segments, read.segments);
    let once = model.clone();
    assert_eq!(model.apply(&event), asks(Part::Recording(id)));
    assert_eq!(model, once);

    // A recording that no reader has open asks for nothing.
    let other = recording(RecordingId::generate(), scene.daily, 0);
    let before = model.clone();
    assert_eq!(model.apply(&updated(&other)), Follow::default());
    assert_eq!(model, before);

    // Before the first answer the event replaces nothing, and the routes are read.
    model.open_recording(other.id);
    let started = Event::RecordingStarted {
        recording: Box::new(other.clone()),
    };
    assert_eq!(model.apply(&started), asks(Part::Recording(other.id)));
    assert_eq!(held(&model, other.id).detail, Loadable::NotLoaded);
}

#[test]
fn a_removed_recording_and_a_deleted_channel_leave_nothing_of_the_recording() {
    let scene = scene();
    let (mut model, read) = opened(&scene, 2);
    let id = read.recording.id;
    model.add_transcript_lines(id, answer(first_page(LINES_PAGE), 0..2, false));

    // Another recording of the channel.
    let other = recording(RecordingId::generate(), scene.daily, 0);
    let before = model.clone();
    model.apply(&removed(&other));
    assert_eq!(model, before);

    assert_eq!(model.apply(&removed(&read.recording)), Follow::default());
    let open = held(&model, id);
    assert!(open.gone);
    assert_eq!(open.detail, Loadable::NotLoaded);
    assert_eq!(open.transcript, Loadable::NotLoaded);
    assert_eq!(open.lines, TranscriptLines::default());
    assert_eq!(model.lines_ask(id), None);
    let once = model.clone();
    model.apply(&removed(&read.recording));
    assert_eq!(model, once);

    // The recordings of a deleted channel are read by nobody (ADR 0604).
    let (mut model, read) = opened(&scene, 0);
    let elsewhere = RecordingId::generate();
    let kept = detail(recording(elsewhere, scene.hidden, 0));
    model.open_recording(elsewhere);
    assert!(model.set_recording(elsewhere, kept.clone(), transcript(&kept.recording)));
    model.apply(&Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.daily,
    });
    assert!(held(&model, read.recording.id).gone);
    assert!(!held(&model, elsewhere).gone);

    // The server answers a read with `not_found`: the account may not read the recording
    // any more. A later answer brings it back.
    model.fail_recording(elsewhere, ReadFailure::NotFound);
    let open = held(&model, elsewhere);
    assert!(open.gone && open.failed.is_none());
    assert_eq!(open.detail, Loadable::NotLoaded);
    assert!(model.set_recording(elsewhere, kept.clone(), transcript(&kept.recording)));
    assert!(!held(&model, elsewhere).gone);
}

#[test]
fn a_correction_changes_a_loaded_line_without_a_request() {
    let scene = scene();
    let (mut model, read) = opened(&scene, 3);
    let id = read.recording.id;
    model.add_transcript_lines(id, answer(first_page(LINES_PAGE), 0..3, false));
    let before = model.clone();
    let (ada, ben) = (AccountId::generate(), AccountId::generate());
    let second = time() + time::Duration::seconds(1);

    let event = corrected(&read.recording, 1, "eleven seconds", ada, time());
    assert_eq!(model.apply(&event), Follow::default());
    let open = held(&model, id);
    let line = open.line(1).unwrap();
    assert_eq!(line.text, "eleven seconds");
    // The event does not carry the text of the worker. The line held it.
    assert_eq!(line.original_text.as_deref(), Some("line 1"));
    assert_eq!(
        (line.corrected_by, line.corrected_at),
        (Some(ada), Some(time()))
    );
    // The other lines are the ones of the state before.
    assert!(Arc::ptr_eq(
        open.line(0).unwrap(),
        held(&before, id).line(0).unwrap()
    ));
    let once = model.clone();
    model.apply(&event);
    assert_eq!(model, once);

    // A second correction keeps the text of the worker.
    model.apply(&corrected(
        &read.recording,
        1,
        "twelve seconds",
        ben,
        second,
    ));
    let line = held(&model, id).line(1).unwrap();
    assert_eq!(line.text, "twelve seconds");
    assert_eq!(line.original_text.as_deref(), Some("line 1"));
    assert_eq!(
        (line.corrected_by, line.corrected_at),
        (Some(ben), Some(second))
    );
    // An event that was kept for a read can be older than the line that the read gave.
    let newest = model.clone();
    model.apply(&event);
    assert_eq!(model, newest);

    // A line that is not loaded, and a recording that no reader has open.
    model.apply(&corrected(&read.recording, 7, "no such line", ada, second));
    let other = recording(RecordingId::generate(), scene.daily, 3);
    model.apply(&corrected(&other, 1, "elsewhere", ada, second));
    assert_eq!(model, newest);
}

#[test]
fn a_rename_changes_a_speaker_of_the_loaded_transcript_without_a_request() {
    let scene = scene();
    let mut model = scene.model();
    let id = RecordingId::generate();
    let read = detail(recording(id, scene.daily, 3));
    let mut header = transcript(&read.recording);
    header.speakers = vec![speaker(1, None), speaker(3, None)];
    model.open_recording(id);
    // The labels are not in the state before the transcript is.
    let before = model.clone();
    model.apply(&renamed(&read.recording, &speaker(1, Some("Noor"))));
    assert_eq!(model, before);
    assert!(model.set_recording(id, read.clone(), header.clone()));

    let named = TranscriptSpeaker {
        name: Some("Noor".to_owned()),
        renamed_by: Some(scene.account_id),
        renamed_at: Some(time()),
        ..header.speakers[0].clone()
    };
    let event = renamed(&read.recording, &named);
    assert_eq!(model.apply(&event), Follow::default());
    assert_eq!(held(&model, id).speaker(1), Some(&named));
    assert_eq!(held(&model, id).speaker(3), Some(&header.speakers[1]));
    let once = model.clone();
    model.apply(&event);
    assert_eq!(model, once);

    // A label that the transcript did not list is put in the order of the numbers.
    model.apply(&renamed(&read.recording, &speaker(2, Some("Kim"))));
    let numbers: Vec<u32> = held(&model, id)
        .transcript
        .loaded()
        .unwrap()
        .speakers
        .iter()
        .map(|held| held.number)
        .collect();
    assert_eq!(numbers, [1, 2, 3]);
}

#[test]
fn the_lines_are_read_in_pages_from_the_first_line() {
    let scene = scene();
    let (mut model, read) = opened(&scene, 450);
    let id = read.recording.id;
    let page = u32::try_from(LINES_PAGE).unwrap();

    // The first page.
    let ask = model.lines_ask(id).unwrap();
    assert_eq!(ask, first_page(LINES_PAGE));
    model.set_lines_reading(id, true);
    assert!(held(&model, id).lines.reading);
    model.add_transcript_lines(id, answer(ask, 0..page, true));
    let lines = &held(&model, id).lines;
    assert_eq!(lines.lines.len(), LINES_PAGE);
    assert!(lines.loaded && lines.more && !lines.reading && lines.failed.is_none());

    // The page after the last loaded line.
    let next = model.lines_ask(id).unwrap();
    let after_first = LinesAsk {
        after: Some(page - 1),
        count: LINES_PAGE,
        replace: false,
    };
    assert_eq!(next, after_first);
    // An answer that continues other lines than the loaded ones is not taken.
    let elsewhere = LinesAsk {
        after: Some(150),
        ..after_first
    };
    model.add_transcript_lines(id, answer(elsewhere, 151..300, true));
    assert_eq!(held(&model, id).lines.lines.len(), LINES_PAGE);
    model.add_transcript_lines(id, answer(next, page..450, false));
    assert_eq!(positions(&model, id), (0..450).collect::<Vec<u32>>());
    assert!(!held(&model, id).lines.more);
    assert_eq!(model.lines_ask(id), None);

    // A page that adds no line promises no more lines.
    let (mut model, read) = opened(&scene, 450);
    let id = read.recording.id;
    model.add_transcript_lines(id, answer(first_page(LINES_PAGE), 0..0, true));
    let lines = &held(&model, id).lines;
    assert!(lines.loaded && !lines.more && lines.lines.is_empty());

    // The page names no recording. Its lines are taken while each position is above the
    // one before, and the lines say that the read failed.
    let (mut model, read) = opened(&scene, 450);
    let id = read.recording.id;
    let mut mixed = spoken(0..2);
    mixed.extend(spoken(1..4));
    let unordered = LinesRead {
        ask: first_page(LINES_PAGE),
        lines: mixed,
        more: true,
    };
    model.add_transcript_lines(id, unordered);
    assert_eq!(positions(&model, id), [0, 1]);
    let lines = &held(&model, id).lines;
    assert_eq!(lines.failed, Some(ReadFailure::NoAnswer));
    assert!(!lines.more);
    // A page that starts at the line that it was asked to follow adds nothing.
    let next = LinesAsk {
        after: Some(1),
        count: LINES_PAGE,
        replace: false,
    };
    model.add_transcript_lines(id, answer(next, 1..3, true));
    assert_eq!(positions(&model, id), [0, 1]);
}

#[test]
fn a_read_that_fails_tells_why_and_keeps_what_is_loaded() {
    let scene = scene();
    let mut model = scene.model();
    let id = RecordingId::generate();
    model.open_recording(id);
    model.set_recording_reading(id);
    let refused = ReadFailure::Refused(ErrorCode::Forbidden);
    model.fail_recording(id, refused);
    let open = held(&model, id);
    assert_eq!(open.failed, Some(refused));
    assert_eq!(open.detail, Loadable::NotLoaded);
    assert_eq!(open.transcript, Loadable::NotLoaded);
    assert!(!open.gone);

    // A read after the first one: what is loaded stays, and the next answer clears the
    // failure.
    let read = detail(recording(id, scene.daily, 2));
    assert!(model.set_recording(id, read.clone(), transcript(&read.recording)));
    assert_eq!(held(&model, id).failed, None);
    model.fail_recording(id, ReadFailure::NoAnswer);
    let open = held(&model, id);
    assert_eq!(open.failed, Some(ReadFailure::NoAnswer));
    assert_eq!(open.detail.loaded(), Some(&read));

    // The lines have a failure of their own.
    model.set_lines_reading(id, true);
    model.fail_transcript_lines(id, ReadFailure::NoAnswer);
    let lines = &held(&model, id).lines;
    assert_eq!(lines.failed, Some(ReadFailure::NoAnswer));
    assert!(!lines.reading && !lines.loaded);
    model.add_transcript_lines(id, answer(first_page(LINES_PAGE), 0..2, false));
    assert_eq!(held(&model, id).lines.failed, None);
    // The route of the lines answers `not_found` when the recording is gone.
    model.fail_transcript_lines(id, ReadFailure::NotFound);
    assert!(held(&model, id).gone);
    // A page that was on its way then is not taken: nothing of a recording that is gone
    // is held.
    model.add_transcript_lines(id, answer(first_page(LINES_PAGE), 0..2, false));
    let open = held(&model, id);
    assert!(open.gone && open.lines.lines.is_empty() && !open.lines.loaded);
}

#[test]
fn a_reload_keeps_what_is_open_as_not_current_until_it_is_read_again() {
    let scene = scene();
    let (mut before, read) = opened(&scene, 300);
    let id = read.recording.id;
    before.add_transcript_lines(id, answer(first_page(300), 0..300, false));
    before.fail_recording(id, ReadFailure::NoAnswer);
    let unread = RecordingId::generate();
    let released = RecordingId::generate();
    before.open_recording(released);

    let mut model = scene.model();
    model.carry_recordings(Some(&before), &[id, unread]);
    assert!(model.recording(released).is_none());
    let open = held(&model, id);
    assert!(open.stale && open.failed.is_none());
    assert_eq!(open.detail.loaded(), Some(&read));
    assert!(open.lines.stale);
    assert_eq!(open.lines.lines.len(), 300);
    // The lines are read again from the first one, as many as were held.
    assert_eq!(model.lines_ask(id), Some(first_page(300)));
    // A recording that a reader asked for before the first state.
    let fresh = held(&model, unread);
    assert!(!fresh.stale && !fresh.lines.stale);
    assert_eq!(fresh.detail, Loadable::NotLoaded);

    let mut changed = read.clone();
    changed.recording.transcript_lines = 301;
    let mut header = transcript(&changed.recording);
    header.lines = 301;
    assert!(model.set_recording(id, changed.clone(), header));
    let open = held(&model, id);
    assert!(!open.stale && open.lines.stale);
    assert_eq!(open.detail.loaded(), Some(&changed));
    model.add_transcript_lines(id, answer(first_page(300), 0..300, true));
    let lines = &held(&model, id).lines;
    assert!(!lines.stale && lines.more);
    assert_eq!(lines.lines.len(), 300);

    // The state of another account keeps nothing of this one.
    let other = super::tests::scene();
    let mut model = other.model();
    model.carry_recordings(Some(&before), &[id]);
    let open = held(&model, id);
    assert_eq!(open.detail, Loadable::NotLoaded);
    assert!(!open.stale && open.lines.lines.is_empty());
}

#[test]
fn the_events_that_arrive_during_a_read_are_applied_after_its_answer() {
    let scene = scene();
    let mut model = scene.model();
    let id = RecordingId::generate();
    model.open_recording(id);
    let read = detail(recording(id, scene.daily, 0));
    let other = recording(RecordingId::generate(), scene.daily, 2);
    let ada = AccountId::generate();
    let mut later = read.recording.clone();
    later.transcript = TranscriptState::Ready;
    later.transcript_lines = 2;
    let named = speaker(1, Some("Noor"));
    let deleted = Event::ChannelDeleted {
        workspace_id: scene.workspace_id,
        channel_id: scene.hidden,
    };

    let part = Part::Recording(id);
    assert!(part.is_read_again());
    assert!(part.concerns(&updated(&later), &model));
    assert!(part.concerns(&renamed(&later, &named), &model));
    assert!(part.concerns(&removed(&later), &model));
    assert!(part.concerns(&deleted, &model));
    // A change of a channel can change what the account may read of a recording
    // (ADR 0604): a read that is on its way is made again.
    let changed = Event::ChannelUpdated {
        channel: model.channel(scene.daily).unwrap().channel.clone(),
    };
    assert!(part.concerns(&changed, &model));
    assert!(!part.concerns(&updated(&other), &model));
    assert!(!part.concerns(&renamed(&other, &named), &model));
    assert!(!part.concerns(&corrected(&later, 1, "a", ada, time()), &model));

    // The answer is older than the two events. They are applied to it, and the event of
    // the recording asks for the routes again.
    let mut header = transcript(&read.recording);
    header.speakers = vec![speaker(1, None)];
    assert!(model.set_recording(id, read.clone(), header));
    let kept = [updated(&later), renamed(&later, &named), deleted];
    assert_eq!(model.replay_reads(part, &kept), [part]);
    let open = held(&model, id);
    assert_eq!(open.detail.loaded().unwrap().recording, later);
    assert_eq!(open.speaker(1), Some(&named));
    assert!(!open.gone);

    let lines_part = Part::TranscriptLines(id);
    assert!(lines_part.is_read_again());
    let fixed = corrected(&later, 1, "eleven seconds", ada, time());
    assert!(lines_part.concerns(&fixed, &model));
    assert!(lines_part.concerns(&removed(&later), &model));
    assert!(!lines_part.concerns(&updated(&later), &model));
    assert!(!lines_part.concerns(&corrected(&other, 1, "a", ada, time()), &model));
    model.add_transcript_lines(id, answer(first_page(LINES_PAGE), 0..2, false));
    assert_eq!(model.replay_reads(lines_part, [&fixed]), []);
    assert_eq!(held(&model, id).line(1).unwrap().text, "eleven seconds");

    // The recording was removed while it was read.
    assert!(model.set_recording(id, read.clone(), transcript(&read.recording)));
    assert_eq!(model.replay_reads(part, [&removed(&later)]), []);
    assert!(held(&model, id).gone);
}

#[test]
fn a_changed_channel_asks_for_its_open_recordings_again() {
    let scene = scene();
    let (mut model, read) = opened(&scene, 0);
    let id = read.recording.id;
    // A recording of another channel that a reader has open.
    let other = detail(recording(RecordingId::generate(), scene.general, 0));
    model.open_recording(other.recording.id);
    assert!(model.set_recording(
        other.recording.id,
        other.clone(),
        transcript(&other.recording)
    ));

    // The voice channel starts to hide who is inside: the server gives a recording of
    // its running call only to the people inside then (ADR 0604).
    let mut channel = model.channel(scene.daily).unwrap().channel.clone();
    channel.hides_occupants = true;
    let follow = model.apply(&Event::ChannelUpdated { channel });
    assert_eq!(follow.load, [Part::Recording(id)]);
}
