//! The screens of a finished recording (ADR 0744, view G of the design): the card in the
//! chat of its voice channel with each state of its job and of its transcript, the list
//! of its files, the downloads, the question before a removal, the transcript in the side
//! panel, and the files of the recording on the page of the recordings (ADR 0784).

use std::sync::Arc;

use junto_client::shell::Shell;
use junto_client::state::admin::{AdminOpen, AdminPage, RecordingList};
use junto_client::state::transcript_work::TranscriptChange;
use junto_client::state::view::{Known, ServerSnapshot};
use junto_client::text;
use junto_client_core::model::{
    Loadable, OpenRecording, ReadFailure, ServerModel, TranscriptLines,
};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{AccountId, RecordingId, RecordingSegmentId, ServerId};
use junto_client_core::protocol::recording::SegmentFailure;
use junto_client_core::protocol::recording::{
    FileCheck, FileProblem, Recording, RecordingDetail, RecordingFailure, RecordingSegment,
    RecordingState, SegmentFile, SegmentSource, SegmentState,
};
use junto_client_core::protocol::transcript::{
    GlossaryUse, LineSource, SegmentReason, SegmentTranscription, SpeakerSeparation,
    SpeakersReason, Transcript, TranscriptFailure, TranscriptLine, TranscriptSegment,
    TranscriptSpeaker, TranscriptState,
};
use junto_client_core::transfers::{DownloadProgress, DownloadStage, TransferFailure};
use time::Duration;

use super::messages::{Made, acme_with, morning, none, prepare};
use super::{Screen, Studio, harbor, id, person, select_channel};

const DAILY: u32 = 34;

fn server_id() -> ServerId {
    id(1)
}

fn recording_id() -> RecordingId {
    id(6500)
}

fn segment_id(number: u32) -> RecordingSegmentId {
    id(6510 + number)
}

/// The start of the recording of the design: 09:41.
fn started() -> time::OffsetDateTime {
    morning() + Duration::minutes(41)
}

fn segment(
    number: u32,
    source: SegmentSource,
    of: Option<&str>,
    bytes: u64,
    picture: Option<(u32, u32)>,
) -> RecordingSegment {
    RecordingSegment {
        id: segment_id(number),
        source,
        account_id: of.map(person),
        state: SegmentState::Ended,
        failure: None,
        requested_at: started(),
        started_at: Some(started()),
        ended_at: Some(started() + Duration::seconds(2_380)),
        duration_ms: Some(2_380_000),
        file: Some(SegmentFile {
            media_type: if picture.is_some() {
                "video/mp4"
            } else {
                "audio/ogg"
            }
            .to_owned(),
            bytes,
            duration_ms: 2_380_000,
            starts_at: Some(started()),
            width: picture.map(|(width, _)| width),
            height: picture.map(|(_, height)| height),
            check: FileCheck::Passed,
            problems: Vec::new(),
            silent: false,
            recovered: false,
        }),
    }
}

fn segments() -> Vec<RecordingSegment> {
    let mut segments = vec![segment(
        0,
        SegmentSource::Composite,
        None,
        361_000_000,
        Some((1920, 1200)),
    )];
    for (number, name) in ["Ana", "Bruno", "Carla", "Marta", "Luca"]
        .iter()
        .enumerate()
    {
        segments.push(segment(
            number as u32 + 1,
            SegmentSource::Microphone,
            Some(name),
            9_400_000,
            None,
        ));
    }
    segments.push(segment(
        6,
        SegmentSource::ScreenShareAudio,
        Some("Carla"),
        4_000_000,
        None,
    ));
    segments
}

fn line(
    number: u32,
    seconds: u64,
    of: &str,
    speaker: Option<u32>,
    text: &str,
) -> Arc<TranscriptLine> {
    Arc::new(TranscriptLine {
        line: number,
        start_ms: seconds * 1_000,
        end_ms: seconds * 1_000 + 3_000,
        segment_id: segment_id(1),
        source: if speaker.is_some() {
            LineSource::Shared
        } else {
            LineSource::Microphone
        },
        account_id: person(of),
        speaker,
        text: text.to_owned(),
        language: "en".to_owned(),
        original_text: None,
        corrected_by: None,
        corrected_at: None,
    })
}

/// The recording of view G, as the client core holds it open.
fn open(state: RecordingState, transcript: TranscriptState) -> OpenRecording {
    let segments = segments();
    let lines = vec![
        line(
            0,
            6,
            "Ana",
            None,
            "Recording is on. Bruno, start with staging.",
        ),
        line(
            1,
            15,
            "Luca",
            None,
            "Before that: the demo invite for Marta goes out today.",
        ),
        line(
            2,
            662,
            "Carla",
            Some(1),
            "Thank you all for joining the weekly review.",
        ),
        line(
            3,
            669,
            "Carla",
            Some(2),
            "Can we see the export timings first?",
        ),
        line(
            4,
            691,
            "Carla",
            Some(1),
            "The mixdown takes eleven seconds now, down from ninety-six.",
        ),
        line(
            5,
            1_267,
            "Ana",
            None,
            "Diego writes the retention spec, and Luca sends Marta the demo invite.",
        ),
        line(6, 1_301, "Bruno", None, "I take the release notes."),
    ];
    let ready = matches!(
        transcript,
        TranscriptState::Ready | TranscriptState::Partial
    );
    OpenRecording {
        id: recording_id(),
        detail: Loadable::Loaded(RecordingDetail {
            recording: Recording {
                id: recording_id(),
                workspace_id: id(10),
                channel_id: id(DAILY),
                call_id: id(6501),
                state,
                started_by: person("Ana"),
                requested_at: started(),
                recording_since: Some(started()),
                stop: None,
                ended_at: Some(started() + Duration::seconds(2_380)),
                failure: None,
                language: "en".to_owned(),
                segments: segments.len() as u32,
                failed_segments: 0,
                bytes: Some(412_000_000),
                timeline_started_at: Some(started()),
                transcript,
                transcript_lines: if ready { 212 } else { 0 },
            },
            segments,
        }),
        transcript: Loadable::Loaded(Transcript {
            recording_id: recording_id(),
            state: transcript,
            failure: (transcript == TranscriptState::Failed)
                .then_some(TranscriptFailure::WorkerNotReady),
            language: "en".to_owned(),
            lines: if ready { 212 } else { 0 },
            speakers: vec![
                TranscriptSpeaker {
                    number: 1,
                    account_id: person("Carla"),
                    name: Some("Noor".to_owned()),
                    renamed_by: Some(person("Ana")),
                    renamed_at: Some(started()),
                },
                TranscriptSpeaker {
                    number: 2,
                    account_id: person("Carla"),
                    name: None,
                    renamed_by: None,
                    renamed_at: None,
                },
            ],
            glossary: GlossaryUse::default(),
            segments: Vec::new(),
        }),
        lines: TranscriptLines {
            lines: if ready { lines } else { Vec::new() },
            loaded: ready,
            more: ready,
            reading: false,
            failed: None,
            stale: false,
            more_before: false,
            around: None,
        },
        failed: None,
        gone: false,
        stale: false,
    }
}

/// The chat of Daily after the call of the design, with the post of the recording.
fn chat() -> Made {
    let mut made = Made::new(DAILY);
    made.system(38, "call_started", Some("Ana"), None);
    made.system(41, "recording_started", Some("Ana"), None);
    let bruno = made.message(
        "Bruno",
        76,
        "Two more minutes on the screen share, then we close.",
    );
    bruno.edited_at = Some(morning() + Duration::minutes(77));
    made.system(80, "recording_stopped", Some("Ana"), None);
    made.system(81, "call_ended", None, None);
    made.system(84, "recording_ready", None, None);
    if let Some(post) = made.messages.last_mut()
        && let Some(system) = &mut Arc::make_mut(post).system
    {
        system.recording_id = Some(recording_id());
    }
    made
}

/// "Acme Studio" with the chat of Daily and this state of its recording. Nobody is in
/// the call.
fn acme(held: Option<OpenRecording>, change: impl FnOnce(&mut ServerModel)) -> Vec<ServerSnapshot> {
    vec![
        acme_with(|model| {
            let studio = Arc::make_mut(&mut model.workspaces[0]);
            for channel in &mut studio.channels {
                if channel.channel.id == id(DAILY) {
                    channel.call = None;
                    channel.in_use = false;
                }
            }
            model.timelines = vec![Arc::new(chat().timeline())];
            model.recordings = held.into_iter().map(Arc::new).collect();
            change(model);
        }),
        harbor(),
    ]
}

fn show(shell: &mut Shell, change: impl FnOnce(&mut Known), cx: &mut gpui::Context<Shell>) {
    prepare(shell, none(), cx);
    select_channel(shell, "Daily", cx);
    shell.state().clone().update(cx, |app, cx| {
        app.show_notices_fixture(morning() + Duration::minutes(100), |_| {}, cx);
        app.show_recordings_fixture(change, cx);
    });
}

fn progress(stage: DownloadStage, received: u64, bytes: u64) -> DownloadProgress {
    DownloadProgress {
        stage,
        received,
        bytes,
        interrupted: false,
    }
}

/// Gives the "everyone" role of "Studio" the permission that removes a recording.
fn may_remove(model: &mut ServerModel) {
    let studio = Arc::make_mut(&mut model.workspaces[0]);
    if let Some(everyone) = studio.roles.iter_mut().find(|role| role.is_everyone) {
        everyone
            .permissions
            .push(WorkspacePermission::ManageRecordings);
    }
}

pub(crate) fn draw(studio: &mut Studio) -> Result<(), String> {
    let plain = |shell: &mut Shell, _: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
        show(shell, |_| {}, cx);
    };
    let ready = || open(RecordingState::Ready, TranscriptState::Ready);

    // View G of the design: the card with its transcript.
    studio.draw(
        Screen::new("recording-card", acme(Some(ready()), may_remove)),
        plain,
    )?;
    studio.draw(
        Screen::new("recording-card-150", acme(Some(ready()), may_remove)).scale(150),
        plain,
    )?;
    // A person without "manage recordings".
    studio.draw(
        Screen::new("recording-card-member", acme(Some(ready()), |_| {})),
        plain,
    )?;

    // The states of the job.
    for (name, state) in [
        ("recording-job-starting", RecordingState::Starting),
        ("recording-job-recording", RecordingState::Recording),
        ("recording-job-processing", RecordingState::Processing),
    ] {
        let mut held = open(state, TranscriptState::None);
        if let Loadable::Loaded(detail) = &mut held.detail {
            detail.recording.bytes = None;
            detail.recording.ended_at = None;
            for segment in &mut detail.segments {
                segment.file = None;
            }
        }
        studio.draw(Screen::new(name, acme(Some(held), |_| {})), plain)?;
    }
    let mut partly = open(RecordingState::PartlyFailed, TranscriptState::Partial);
    if let Loadable::Loaded(detail) = &mut partly.detail {
        detail.recording.failed_segments = 1;
        detail.segments[2].state = SegmentState::Failed;
        detail.segments[2].file = None;
        if let Some(file) = &mut detail.segments[6].file {
            file.check = FileCheck::Failed;
            file.problems = vec![FileProblem::StreamShort, FileProblem::TimestampHole];
        }
    }
    studio.draw(
        Screen::new(
            "recording-job-partly-failed",
            acme(Some(partly.clone()), |_| {}),
        ),
        plain,
    )?;
    studio.draw(
        Screen::new("recording-files-with-problem", acme(Some(partly), |_| {})),
        |shell, window, cx| {
            show(shell, |_| {}, cx);
            shell.open_recording_files(recording_id(), window, cx);
        },
    )?;
    let mut failed = open(RecordingState::Failed, TranscriptState::None);
    if let Loadable::Loaded(detail) = &mut failed.detail {
        detail.recording.failure = Some(RecordingFailure::FilesMissing);
        detail.recording.bytes = None;
        for segment in &mut detail.segments {
            segment.file = None;
        }
    }
    studio.draw(
        Screen::new("recording-job-failed", acme(Some(failed), |_| {})),
        plain,
    )?;

    // The states of the transcript.
    for (name, state) in [
        ("recording-transcript-none", TranscriptState::None),
        ("recording-transcript-waiting", TranscriptState::Waiting),
        ("recording-transcript-running", TranscriptState::Running),
        ("recording-transcript-failed", TranscriptState::Failed),
    ] {
        studio.draw(
            Screen::new(name, acme(Some(open(RecordingState::Ready, state)), |_| {})),
            plain,
        )?;
    }

    // A recording that is read, that could not be read, and that is gone.
    studio.draw(Screen::new("recording-reading", acme(None, |_| {})), plain)?;
    let mut unread = ready();
    unread.detail = Loadable::NotLoaded;
    unread.transcript = Loadable::NotLoaded;
    unread.failed = Some(ReadFailure::NoAnswer);
    studio.draw(
        Screen::new("recording-not-read", acme(Some(unread), |_| {})),
        plain,
    )?;
    let mut gone = ready();
    gone.gone = true;
    studio.draw(
        Screen::new("recording-gone", acme(Some(gone), |_| {})),
        plain,
    )?;

    // The list of the files, the downloads, and the removal.
    studio.draw(
        Screen::new("recording-files", acme(Some(ready()), may_remove)),
        |shell, window, cx| {
            show(shell, |_| {}, cx);
            shell.open_recording_files(recording_id(), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("recording-downloads", acme(Some(ready()), may_remove)),
        |shell, _, cx| {
            show(
                shell,
                |known| {
                    known.segment_downloads.insert(
                        (server_id(), segment_id(0)),
                        progress(DownloadStage::Receiving, 148_000_000, 361_000_000),
                    );
                    known.segment_downloads.insert(
                        (server_id(), segment_id(1)),
                        progress(
                            DownloadStage::Saved("/downloads/Daily microphone Ana.ogg".into()),
                            9_400_000,
                            9_400_000,
                        ),
                    );
                    known.segment_downloads.insert(
                        (server_id(), segment_id(6)),
                        progress(
                            DownloadStage::Failed(TransferFailure::Corrupt),
                            1_000_000,
                            4_000_000,
                        ),
                    );
                    known.text_downloads.insert(
                        (server_id(), recording_id()),
                        progress(DownloadStage::Receiving, 12_000, 0),
                    );
                },
                cx,
            );
        },
    )?;
    // The page of the recordings of the administration lists the files of the recording
    // in its row, with a download that runs and one that ended.
    studio.draw(
        Screen::new("admin-recording-files", acme(Some(ready()), |_| {})),
        move |shell, window, cx| {
            show(
                shell,
                |known| {
                    known.segment_downloads.insert(
                        (server_id(), segment_id(0)),
                        progress(DownloadStage::Receiving, 148_000_000, 361_000_000),
                    );
                    known.text_downloads.insert(
                        (server_id(), recording_id()),
                        progress(
                            DownloadStage::Saved("/downloads/Daily transcript.txt".into()),
                            12_000,
                            12_000,
                        ),
                    );
                },
                cx,
            );
            let open = AdminOpen::Server {
                server_id: server_id(),
                page: AdminPage::Recordings,
            };
            shell.open_admin(open, window, cx);
            let recordings = match ready().detail {
                Loadable::Loaded(detail) => vec![detail.recording],
                _ => Vec::new(),
            };
            let list =
                RecordingList::loaded(server_id(), id(10), person("Luca"), recordings, false)
                    .with_files_open(recording_id());
            shell
                .state()
                .clone()
                .update(cx, |state, cx| state.show_recording_list_fixture(list, cx));
        },
    )?;
    studio.draw(
        Screen::new("recording-remove-question", acme(Some(ready()), may_remove)),
        |shell, window, cx| {
            show(shell, |_| {}, cx);
            shell.ask_to_remove_recording(server_id(), recording_id(), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("recording-removing", acme(Some(ready()), may_remove)),
        |shell, _, cx| {
            show(
                shell,
                |known| {
                    known
                        .recordings_removing
                        .insert((server_id(), recording_id()));
                },
                cx,
            );
        },
    )?;
    studio.draw(
        Screen::new("recording-remove-refused", acme(Some(ready()), |_| {})),
        |shell, _, cx| {
            show(
                shell,
                |known| {
                    known.recording_refused.insert(
                        (server_id(), recording_id()),
                        text::RECORDING_REMOVE_NOT_ALLOWED.to_owned(),
                    );
                },
                cx,
            );
        },
    )?;

    // The transcript in the side panel.
    let transcript = |line: Option<u32>| {
        move |shell: &mut Shell, _: &mut gpui::Window, cx: &mut gpui::Context<Shell>| {
            show(shell, |_| {}, cx);
            shell.open_transcript(server_id(), recording_id(), line, cx);
        }
    };
    studio.draw(
        Screen::new("transcript-panel", acme(Some(ready()), |_| {})),
        transcript(None),
    )?;
    studio.draw(
        Screen::new("transcript-panel-at-line", acme(Some(ready()), |_| {})),
        transcript(Some(5)),
    )?;
    studio.draw(
        Screen::new("transcript-panel-150", acme(Some(ready()), |_| {})).scale(150),
        transcript(None),
    )?;
    let mut lines_failed = ready();
    lines_failed.lines.failed = Some(ReadFailure::NoAnswer);
    studio.draw(
        Screen::new(
            "transcript-panel-lines-failed",
            acme(Some(lines_failed), |_| {}),
        ),
        transcript(None),
    )?;
    studio.draw(
        Screen::new(
            "transcript-panel-waiting",
            acme(
                Some(open(RecordingState::Ready, TranscriptState::Waiting)),
                |_| {},
            ),
        ),
        transcript(None),
    )?;
    draw_changes(studio)?;
    let _: AccountId = person("Luca");
    Ok(())
}

/// The recording of view G with a corrected line, the glossary terms of its transcript,
/// a damaged audio file, voices without a label for some lines, and a part that failed.
fn changed() -> OpenRecording {
    let mut held = open(RecordingState::PartlyFailed, TranscriptState::Partial);
    let corrected = Arc::make_mut(&mut held.lines.lines[4]);
    corrected.original_text =
        Some("The mix down takes a leaven seconds now, down from ninety-six.".to_owned());
    corrected.corrected_by = Some(person("Luca"));
    corrected.corrected_at = Some(started() + Duration::minutes(50));
    if let Loadable::Loaded(transcript) = &mut held.transcript {
        transcript.glossary = GlossaryUse {
            used: vec![
                "Marta".to_owned(),
                "mixdown".to_owned(),
                "staging".to_owned(),
            ],
            cut: vec!["Quorvex".to_owned()],
        };
        let result = |number, status, reason, speakers, speakers_reason| TranscriptSegment {
            segment_id: segment_id(number),
            status,
            reason,
            audio_ms: Some(2_380_000),
            processed_ms: Some(1_200_000),
            lines: 40,
            speakers,
            speakers_reason,
        };
        transcript.segments = vec![
            result(
                1,
                SegmentTranscription::Ok,
                None,
                SpeakerSeparation::NotApplicable,
                None,
            ),
            result(
                2,
                SegmentTranscription::Damaged,
                Some(SegmentReason::DecodeIncomplete),
                SpeakerSeparation::NotApplicable,
                None,
            ),
            result(
                6,
                SegmentTranscription::Ok,
                None,
                SpeakerSeparation::Unassigned,
                Some(SpeakersReason::NoVoiceWindow),
            ),
        ];
    }
    // The microphone of Luca was lost after 20 minutes.
    if let Loadable::Loaded(detail) = &mut held.detail
        && let Some(lost) = detail.segments.get_mut(5)
    {
        lost.state = SegmentState::Failed;
        lost.failure = Some(SegmentFailure::RecorderLost);
        lost.ended_at = Some(started() + Duration::seconds(1_200));
        lost.file = None;
    }
    held
}

/// The changes of a transcript (ADR 0616): the original text of a corrected line, the
/// form that corrects a line, the details with the voices and the result of each audio
/// file, the form that names a voice, the parts of a recording, and a new request for a
/// transcript.
fn draw_changes(studio: &mut Studio) -> Result<(), String> {
    let panel = |shell: &mut Shell, cx: &mut gpui::Context<Shell>| {
        show(shell, |_| {}, cx);
        shell.open_transcript(server_id(), recording_id(), None, cx);
    };
    studio.draw(
        Screen::new("transcript-original", acme(Some(changed()), |_| {})),
        move |shell, _, cx| {
            panel(shell, cx);
            shell.toggle_original(recording_id(), 4, cx);
        },
    )?;
    studio.draw(
        Screen::new("transcript-correct-form", acme(Some(changed()), |_| {})),
        move |shell, window, cx| {
            panel(shell, cx);
            let shown = "Before that: the demo invite for Marta goes out today.";
            let what = TranscriptChange::Line(1);
            shell.open_transcript_form(server_id(), recording_id(), what, Some(shown), window, cx);
        },
    )?;
    studio.draw(
        Screen::new("transcript-details", acme(Some(changed()), |_| {})),
        move |shell, _, cx| {
            panel(shell, cx);
            shell.toggle_transcript_details(recording_id(), cx);
        },
    )?;
    studio.draw(
        Screen::new("transcript-name-voice", acme(Some(changed()), |_| {})),
        move |shell, window, cx| {
            panel(shell, cx);
            shell.toggle_transcript_details(recording_id(), cx);
            let what = TranscriptChange::Speaker(2);
            shell.open_transcript_form(server_id(), recording_id(), what, None, window, cx);
        },
    )?;
    studio.draw(
        Screen::new("recording-parts", acme(Some(changed()), |_| {})),
        |shell, window, cx| {
            show(shell, |_| {}, cx);
            shell.open_recording_parts(recording_id(), window, cx);
        },
    )?;
    let mut failed = open(RecordingState::Ready, TranscriptState::Failed);
    if let Loadable::Loaded(transcript) = &mut failed.transcript {
        transcript.failure = Some(TranscriptFailure::JobRefused);
    }
    studio.draw(
        Screen::new(
            "recording-transcript-request",
            acme(Some(failed), may_remove),
        ),
        |shell, _, cx| show(shell, |_| {}, cx),
    )
}
