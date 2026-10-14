use std::sync::Arc;

use junto_client_core::model::OpenRecording;
use junto_client_core::protocol::ids::{CallId, ChannelId, WorkspaceId};
use junto_client_core::protocol::recording::{
    FileProblem, Recording, RecordingDetail, RecordingSegment, SegmentFile, SegmentState,
};
use junto_client_core::protocol::transcript::{
    GlossaryUse, LineSource, Transcript, TranscriptLine, TranscriptSpeaker,
};
use junto_client_core::transfers::DownloadStage;
use time::{Duration, UtcOffset};

use super::*;

/// Thursday, 8 October 2026 at 09:41 UTC.
fn started() -> OffsetDateTime {
    OffsetDateTime::from_unix_timestamp(1_791_450_000 + 41 * 60).unwrap()
}

struct Scene {
    server_id: ServerId,
    recording_id: RecordingId,
    channel_id: ChannelId,
    ana: AccountId,
    carla: AccountId,
    names: HashMap<AccountId, String>,
    segments: Vec<RecordingSegment>,
    downloads: HashMap<(ServerId, RecordingSegmentId), DownloadProgress>,
    texts: HashMap<(ServerId, RecordingId), DownloadProgress>,
}

fn file(media_type: &str, bytes: u64, picture: Option<(u32, u32)>) -> SegmentFile {
    SegmentFile {
        media_type: media_type.to_owned(),
        bytes,
        duration_ms: 2_380_000,
        starts_at: Some(started()),
        width: picture.map(|(width, _)| width),
        height: picture.map(|(_, height)| height),
        check: FileCheck::Passed,
        problems: Vec::new(),
        silent: false,
        recovered: false,
    }
}

fn segment(
    source: SegmentSource,
    account_id: Option<AccountId>,
    file: Option<SegmentFile>,
) -> RecordingSegment {
    RecordingSegment {
        id: RecordingSegmentId::generate(),
        source,
        account_id,
        state: SegmentState::Ended,
        failure: None,
        requested_at: started(),
        started_at: Some(started()),
        ended_at: Some(started() + Duration::seconds(2_380)),
        duration_ms: Some(2_380_000),
        file,
    }
}

fn scene() -> Scene {
    let (ana, carla) = (AccountId::generate(), AccountId::generate());
    Scene {
        server_id: ServerId::generate(),
        recording_id: RecordingId::generate(),
        channel_id: ChannelId::generate(),
        ana,
        carla,
        names: HashMap::from([(ana, "Ana".to_owned()), (carla, "Carla".to_owned())]),
        segments: vec![
            segment(
                SegmentSource::Composite,
                None,
                Some(file("video/mp4", 400_000_000, Some((1920, 1200)))),
            ),
            segment(
                SegmentSource::Microphone,
                Some(ana),
                Some(file("audio/ogg", 6_000_000, None)),
            ),
            segment(
                SegmentSource::Microphone,
                Some(carla),
                Some(file("audio/ogg", 5_000_000, None)),
            ),
            segment(
                SegmentSource::ScreenShareAudio,
                Some(carla),
                Some(file("audio/ogg", 1_000_000, None)),
            ),
        ],
        downloads: HashMap::new(),
        texts: HashMap::new(),
    }
}

impl Scene {
    fn recording(&self, state: RecordingState) -> Recording {
        Recording {
            id: self.recording_id,
            workspace_id: WorkspaceId::generate(),
            channel_id: self.channel_id,
            call_id: CallId::generate(),
            state,
            started_by: self.ana,
            requested_at: started(),
            recording_since: Some(started()),
            stop: None,
            ended_at: Some(started() + Duration::seconds(2_380)),
            failure: None,
            language: "en".to_owned(),
            segments: self.segments.len() as u32,
            failed_segments: 0,
            bytes: Some(412_000_000),
            timeline_started_at: Some(started()),
            transcript: TranscriptState::Ready,
            transcript_lines: 3,
        }
    }

    fn transcript(&self, state: TranscriptState) -> Transcript {
        Transcript {
            recording_id: self.recording_id,
            state,
            failure: None,
            language: "en".to_owned(),
            lines: 3,
            speakers: vec![
                TranscriptSpeaker {
                    number: 1,
                    account_id: self.carla,
                    name: Some("Noor".to_owned()),
                    renamed_by: Some(self.ana),
                    renamed_at: Some(started()),
                },
                TranscriptSpeaker {
                    number: 2,
                    account_id: self.carla,
                    name: None,
                    renamed_by: None,
                    renamed_at: None,
                },
            ],
            glossary: GlossaryUse::default(),
            segments: Vec::new(),
        }
    }

    fn line(
        &self,
        line: u32,
        start_ms: u64,
        source: LineSource,
        speaker: Option<u32>,
    ) -> TranscriptLine {
        TranscriptLine {
            line,
            start_ms,
            end_ms: start_ms + 2_000,
            segment_id: self.segments[1].id,
            source,
            account_id: match source {
                LineSource::Microphone => self.ana,
                LineSource::Shared => self.carla,
            },
            speaker,
            text: format!("line {line}"),
            language: "en".to_owned(),
            original_text: None,
            corrected_by: None,
            corrected_at: None,
        }
    }

    fn open(&self, state: RecordingState, transcript: TranscriptState) -> OpenRecording {
        let mut open = OpenRecording {
            id: self.recording_id,
            detail: Loadable::Loaded(RecordingDetail {
                recording: self.recording(state),
                segments: self.segments.clone(),
            }),
            transcript: Loadable::Loaded(self.transcript(transcript)),
            lines: TranscriptLines::default(),
            failed: None,
            gone: false,
            stale: false,
        };
        open.lines.loaded = true;
        open.lines.lines = vec![
            Arc::new(self.line(0, 6_000, LineSource::Microphone, None)),
            Arc::new(self.line(1, 662_000, LineSource::Shared, Some(1))),
            Arc::new(self.line(2, 669_000, LineSource::Shared, Some(2))),
        ];
        open
    }

    fn card(&self, open: Option<&OpenRecording>, may_remove: bool) -> RecordingCard {
        card(&CardSource {
            server_id: self.server_id,
            recording_id: self.recording_id,
            open,
            channel: "Daily",
            channel_id: self.channel_id,
            names: &self.names,
            zone: Zone::Fixed(UtcOffset::UTC),
            today: Some(started().date()),
            may_remove,
            downloads: &self.downloads,
            texts: &self.texts,
            removing: false,
            refused: None,
        })
    }

    fn detail(&self, open: &OpenRecording) -> CardDetail {
        match self.card(Some(open), false).state {
            CardState::Shown(detail) => *detail,
            other => panic!("the card shows no recording: {other:?}"),
        }
    }
}

#[test]
fn a_card_shows_the_times_the_files_and_the_people_of_a_recording() {
    let scene = scene();
    let open = scene.open(RecordingState::Ready, TranscriptState::Ready);
    let detail = scene.detail(&open);
    assert_eq!(detail.channel, "Daily");
    assert_eq!(detail.day, "Today");
    assert_eq!(detail.span, Some(("09:41".to_owned(), "10:20".to_owned())));
    assert_eq!(detail.length, Some(std::time::Duration::from_secs(2_380)));
    assert_eq!(detail.started_by.as_deref(), Some("Ana"));
    assert_eq!(detail.picture, Some((1920, 1200)));
    assert_eq!((detail.microphones, detail.share_sounds), (2, 1));
    // The size is the sum of the files that a person can save, not the size that the
    // recording states.
    assert_eq!(detail.bytes, Some(412_000_000));
    let mut larger = open.clone();
    if let Loadable::Loaded(read) = &mut larger.detail {
        read.segments[0].file.as_mut().unwrap().bytes = 5_000_000_000;
    }
    assert_eq!(scene.detail(&larger).bytes, Some(5_012_000_000));
    assert_eq!(detail.job, JobShown::Ready);

    // Each file can be saved under a name that tells what it is.
    let kinds: Vec<(FileKind, Option<&str>)> = detail
        .files
        .iter()
        .map(|file| (file.kind, file.person.as_deref()))
        .collect();
    assert_eq!(
        kinds,
        [
            (FileKind::Video, None),
            (FileKind::Microphone, Some("Ana")),
            (FileKind::Microphone, Some("Carla")),
            (FileKind::ShareSound, Some("Carla")),
        ]
    );
    let names: Vec<&str> = detail.files.iter().map(|file| file.stem.as_str()).collect();
    assert_eq!(
        names,
        [
            "Daily 2026-10-08 09.41 video",
            "Daily 2026-10-08 09.41 microphone Ana",
            "Daily 2026-10-08 09.41 microphone Carla",
            "Daily 2026-10-08 09.41 shared sound Carla",
        ]
    );
    assert_eq!(detail.text_stem, "Daily 2026-10-08 09.41 transcript");
    assert!(
        detail
            .files
            .iter()
            .all(|file| file.problem.is_none() && file.download.is_none())
    );
    assert_eq!(detail.video, Some(scene.segments[0].id));
}

#[test]
fn a_card_states_each_state_of_the_job_and_what_was_not_saved() {
    let mut scene = scene();
    for (state, shown) in [
        (RecordingState::Starting, JobShown::Starting),
        (RecordingState::Recording, JobShown::Recording),
        (RecordingState::Processing, JobShown::Processing),
        (RecordingState::Ready, JobShown::Ready),
    ] {
        let open = scene.open(state, TranscriptState::None);
        assert_eq!(scene.detail(&open).job, shown);
    }

    // A recording of which a part was saved: a segment without a file is no file of the
    // card, and a file with a problem says so.
    scene.segments[2].state = SegmentState::Failed;
    scene.segments[2].file = None;
    let mut damaged = file("audio/ogg", 1_000, None);
    damaged.check = FileCheck::Failed;
    damaged.problems = vec![FileProblem::StreamShort, FileProblem::TimestampHole];
    scene.segments[3].file = Some(damaged);
    let mut open = scene.open(RecordingState::PartlyFailed, TranscriptState::Partial);
    if let Loadable::Loaded(detail) = &mut open.detail {
        detail.recording.failed_segments = 1;
    }
    let detail = scene.detail(&open);
    assert_eq!(
        detail.job,
        JobShown::PartlyFailed {
            failed: 1,
            segments: 4
        }
    );
    assert_eq!(detail.files.len(), 3);
    assert_eq!((detail.microphones, detail.share_sounds), (1, 1));
    assert!(detail.files[2].problem.is_some());
    assert!(detail.files[0].problem.is_none());

    // Nothing was saved.
    for segment in &mut scene.segments {
        segment.file = None;
    }
    let mut open = scene.open(RecordingState::Failed, TranscriptState::None);
    if let Loadable::Loaded(detail) = &mut open.detail {
        detail.recording.failure = Some(RecordingFailure::FilesMissing);
        detail.recording.bytes = None;
    }
    let detail = scene.detail(&open);
    assert_eq!(
        detail.job,
        JobShown::Failed(Some(RecordingFailure::FilesMissing))
    );
    assert!(detail.files.is_empty() && detail.video.is_none());
    assert_eq!(detail.picture, None);
}

#[test]
fn a_card_tells_a_recording_that_is_read_gone_or_not_readable() {
    let scene = scene();
    assert_eq!(scene.card(None, false).state, CardState::Reading);
    let mut open = scene.open(RecordingState::Ready, TranscriptState::Ready);
    open.detail = Loadable::Loading;
    assert_eq!(scene.card(Some(&open), false).state, CardState::Reading);
    open.detail = Loadable::NotLoaded;
    open.failed = Some(ReadFailure::NoAnswer);
    assert_eq!(scene.card(Some(&open), false).state, CardState::Failed);
    open.gone = true;
    assert_eq!(scene.card(Some(&open), true).state, CardState::Gone);

    // The permission of the roles, and a removal that runs or was refused.
    let open = scene.open(RecordingState::Ready, TranscriptState::Ready);
    assert!(!scene.card(Some(&open), false).may_remove);
    assert!(scene.card(Some(&open), true).may_remove);
    let removing = card(&CardSource {
        server_id: scene.server_id,
        recording_id: scene.recording_id,
        open: Some(&open),
        channel: "Daily",
        channel_id: scene.channel_id,
        names: &scene.names,
        zone: Zone::Fixed(UtcOffset::UTC),
        today: None,
        may_remove: true,
        downloads: &scene.downloads,
        texts: &scene.texts,
        removing: true,
        refused: Some("Not allowed."),
    });
    assert!(removing.removing);
    assert_eq!(removing.refused.as_deref(), Some("Not allowed."));
}

#[test]
fn the_transcript_names_the_speaker_of_each_line() {
    let scene = scene();
    let mut open = scene.open(RecordingState::Ready, TranscriptState::Ready);
    let transcript = scene.detail(&open).transcript;
    assert_eq!(transcript.state, TranscriptState::Ready);
    assert_eq!((transcript.total, transcript.language.as_str()), (3, "en"));
    assert!(!transcript.more && !transcript.reading);
    let lines = &transcript.lines;
    assert_eq!(lines.len(), 3);

    // A line of a microphone has the name of its person.
    assert_eq!(lines[0].time, "00:00:06");
    assert_eq!(lines[0].name, "Ana");
    assert_eq!(lines[0].source, LineFrom::Microphone);
    assert_eq!((lines[0].was.as_deref(), lines[0].voice), (None, None));
    assert_eq!(lines[0].text, "line 0");

    // A voice of shared sound that a person named, and one that has its number.
    assert_eq!(lines[1].time, "00:11:02");
    assert_eq!(lines[1].name, "Noor");
    assert_eq!(lines[1].was.as_deref(), Some("Speaker 1"));
    assert_eq!(lines[1].voice, Some(1));
    assert_eq!(
        lines[1].source,
        LineFrom::Shared {
            of: "Carla".to_owned()
        }
    );
    assert_eq!(lines[2].name, "Speaker 2");
    assert_eq!((lines[2].was.as_deref(), lines[2].voice), (None, Some(2)));

    // A line of shared sound without a label is named by the person who shared, and a
    // corrected line says so.
    let mut unlabelled = scene.line(3, 700_000, LineSource::Shared, None);
    unlabelled.original_text = Some("lime 3".to_owned());
    unlabelled.corrected_by = Some(scene.ana);
    open.lines.lines.push(Arc::new(unlabelled));
    open.lines.more = true;
    open.lines.reading = true;
    let transcript = scene.detail(&open).transcript;
    assert_eq!(transcript.lines[3].name, "Carla");
    assert_eq!(transcript.lines[3].voice, None);
    assert!(transcript.lines[3].corrected && !transcript.lines[0].corrected);
    assert!(transcript.more && transcript.reading);

    // A recording of more than an hour counts the hours.
    assert_eq!(text::clock_of(3_723_000), "01:02:03");
}

#[test]
fn a_file_shows_the_state_of_its_download() {
    let mut scene = scene();
    let open = scene.open(RecordingState::Ready, TranscriptState::Ready);
    let receiving = DownloadProgress {
        stage: DownloadStage::Receiving,
        received: 1_000,
        bytes: 400_000_000,
        interrupted: false,
    };
    scene
        .downloads
        .insert((scene.server_id, scene.segments[0].id), receiving.clone());
    // A download of another server with the same identifier is not this file.
    scene.downloads.insert(
        (ServerId::generate(), scene.segments[1].id),
        receiving.clone(),
    );
    scene
        .texts
        .insert((scene.server_id, scene.recording_id), receiving.clone());
    let detail = scene.detail(&open);
    assert_eq!(detail.files[0].download, Some(receiving.clone()));
    assert_eq!(detail.files[1].download, None);
    assert_eq!(detail.text_download, Some(receiving));
}

#[test]
fn a_card_holds_the_first_lines_of_a_long_transcript() {
    let scene = scene();
    let mut open = scene.open(RecordingState::Ready, TranscriptState::Ready);
    open.lines.lines = (0..500)
        .map(|line| {
            Arc::new(scene.line(line, u64::from(line) * 1_000, LineSource::Microphone, None))
        })
        .collect();
    let transcript = scene.detail(&open).transcript;
    assert_eq!(transcript.held, 500);
    assert_eq!(transcript.lines.len(), SHOWN_LINES);
    assert_eq!(transcript.lines[SHOWN_LINES - 1].text, "line 5");
    // One line of the side panel is derived when it is drawn.
    let row = line_row(&open, &open.lines.lines[321], &scene.names);
    assert_eq!((row.line, row.time.as_str()), (321, "00:05:21"));
}

#[test]
fn a_recording_of_another_channel_than_the_shown_one_is_not_shown() {
    let scene = scene();
    let mut open = scene.open(RecordingState::Ready, TranscriptState::Ready);
    if let Loadable::Loaded(read) = &mut open.detail {
        read.recording.channel_id = ChannelId::generate();
    }
    assert_eq!(scene.card(Some(&open), true).state, CardState::Gone);
}

#[test]
fn the_window_holds_the_recordings_around_the_place_of_the_list() {
    // Thirty recordings, posted at the positions 10, 20, and so on.
    let named: Vec<(u64, RecordingId)> = (1..=30)
        .map(|number| (number * 10, RecordingId::generate()))
        .collect();
    let ids = |range: std::ops::Range<usize>| -> Vec<RecordingId> {
        named[range].iter().map(|(_, id)| *id).collect()
    };
    let sorted = |mut ids: Vec<RecordingId>| {
        ids.sort();
        ids
    };
    // A list at its end shows the newest ones.
    let at_end = near_recordings(&named, None, 24);
    assert_eq!(at_end.len(), 24);
    assert_eq!(sorted(at_end), sorted(ids(6..30)));
    // A list whose first row is the fourth recording: that recording and the ones after
    // it come first, then the ones before it.
    let scrolled = near_recordings(&named, Some(40), 24);
    assert_eq!(scrolled.len(), 24);
    assert_eq!(scrolled[..4], ids(3..7));
    assert_eq!(sorted(scrolled), sorted(ids(0..24)));
    // A first row between two recordings starts with the next one.
    assert_eq!(near_recordings(&named, Some(45), 2), ids(4..6));
    assert_eq!(
        near_recordings(&named, Some(1_000), 2),
        [named[29].1, named[28].1]
    );
    assert!(near_recordings(&[], Some(5), 24).is_empty());
}

#[test]
fn a_failed_refresh_keeps_the_transcript_but_reports_the_failure() {
    let scene = scene();
    let mut open = scene.open(RecordingState::Ready, TranscriptState::Ready);
    let before = scene.detail(&open).transcript;
    open.failed = Some(ReadFailure::NoAnswer);
    let shown = scene.detail(&open).transcript;
    assert_eq!(shown.lines, before.lines);
    assert!(shown.failed);
}
