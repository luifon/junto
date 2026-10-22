//! The changes of a transcript, its details, a new request for it, the parts of a
//! recording, and the language and the cancel of a new recording in the window against the
//! real server (ADR 0616). The glossary page is in `glossary.rs`.
//!
//! The recordings are made by the server of this checkout inside the test process, with
//! its recorder and its media server in memory and a stand-in for the transcription
//! worker (ADR 0723).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};
use junto_client::shell::Incoming;
use junto_client::state::feed::FeedRow;
use junto_client::state::recorded::{
    CardDetail, CardState, JobShown, LineRow, RecordingCard, TranscriptRequest, TranscriptShown,
    When, line_row,
};
use junto_client::state::view::TranscriptPanelState;
use junto_client::text;
use junto_client_core::ServerHandle;
use junto_client_core::flow::access::AccessAction;
use junto_client_core::model::OpenRecording;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::call::CallPlace;
use junto_client_core::protocol::ids::{AccountId, ChannelId, RecordingId, WorkspaceId};
use junto_client_core::protocol::recording::RecordingState;
use junto_client_core::protocol::roles::UpdateWorkspaceRole;
use junto_client_core::protocol::secret::OwnerCode;
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::protocol::transcript::TranscriptState;
use junto_media::calls::{ActiveCall, CallTarget, CallsView, Phase};

use crate::admin_pages::awaited;
use crate::calls::voice_channel;
use crate::harness::{Admin, Window, World, member};
use crate::recording_server::{RecordPlan, RecordingServer, Seat, Spoken, Written};
use crate::support::{Device, FaultProxy, logged_in};

/// The owner of a server of recordings, as a client without a window.
fn owner(world: &World, server: &RecordingServer) -> Admin {
    world.run(async {
        let device = Device::new();
        let handle = device.add(&server.address()).await;
        let claim = AccessAction::ClaimOwner {
            code: OwnerCode::new(server.owner_code()),
            name: "Ada".to_owned(),
            device_name: "First device".to_owned(),
        };
        handle.dispatch(claim).await.unwrap();
        logged_in(&handle);
        Admin { device, handle }
    })
}

fn spoken(account_id: AccountId, speaker: Option<u32>, start_ms: u64, text: &str) -> Spoken {
    Spoken {
        account_id,
        speaker,
        start_ms,
        end_ms: start_ms + 20,
        text: text.to_owned(),
    }
}

/// Gives every member of the workspace a permission, as the owner does.
pub(crate) fn give_everyone(
    world: &World,
    admin: &Admin,
    workspace_id: WorkspaceId,
    permission: WorkspacePermission,
) {
    let api = admin.handle.api();
    let roles = world.run(api.workspace_roles(workspace_id)).unwrap();
    let everyone = roles.iter().find(|role| role.is_everyone).unwrap();
    let mut permissions = everyone.permissions.clone();
    permissions.push(permission);
    let update = UpdateWorkspaceRole {
        name: None,
        permissions: Some(permissions),
    };
    world
        .run(api.update_workspace_role(workspace_id, everyone.id, &update))
        .unwrap();
}

/// A member window on a server of recordings, with the voice channel "Daily".
struct Scene {
    world: World,
    server: RecordingServer,
    admin: Admin,
    window: Window,
    _directory: tempfile::TempDir,
    /// The client of the window.
    luca: ServerHandle,
    own: AccountId,
    ada: AccountId,
    voice: ChannelId,
    workspace_id: WorkspaceId,
}

fn scene(cx: &mut TestAppContext) -> Scene {
    let world = World::new();
    let server = world.run(RecordingServer::start("Acme Studio"));
    let address = server.address();
    scene_at(cx, world, server, &address)
}

/// The scene with the window reaching the server through a proxy, so that a test holds its
/// requests and its event stream. The owner works on the server directly.
fn scene_through_proxy(cx: &mut TestAppContext) -> (Scene, FaultProxy) {
    let world = World::new();
    let server = world.run(RecordingServer::start("Acme Studio"));
    let (listener, port) = server.take_proxy();
    let proxy = world.run(FaultProxy::start_on(listener, port));
    let address = proxy.address();
    (scene_at(cx, world, server, &address), proxy)
}

fn scene_at(
    cx: &mut TestAppContext,
    world: World,
    server: RecordingServer,
    address: &str,
) -> Scene {
    let admin = owner(&world, &server);
    let (directory, mut window) = member(cx, &world, address, &admin, "Luca");
    let server_id = window.view().selected.unwrap().id;
    let own = window.own_account();
    let ada = logged_in(&admin.handle);
    voice_channel(&world, &admin, &mut window, "Daily");
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.name == "Daily")
        .unwrap()
        .id;
    let workspace_id = window.view().workspace().unwrap().id;
    let luca = window
        .state
        .read_with(&window.cx, |state, _| state.server(server_id))
        .unwrap();
    Scene {
        world,
        server,
        admin,
        window,
        _directory: directory,
        luca,
        own,
        ada,
        voice,
        workspace_id,
    }
}

/// The transcript of the side panel, the recording, the names of the accounts, and the
/// zone and the day by which its lines are derived.
type Panel = (
    TranscriptShown,
    Arc<OpenRecording>,
    Arc<HashMap<AccountId, String>>,
    When,
);

impl Window {
    fn recording_card(&mut self, recording_id: RecordingId) -> Option<RecordingCard> {
        self.feed()?.rows.iter().find_map(|row| match row {
            FeedRow::Recording(row) if row.card.recording_id == recording_id => {
                Some(row.card.clone())
            }
            _ => None,
        })
    }

    fn card_detail(&mut self, recording_id: RecordingId) -> Option<CardDetail> {
        match self.recording_card(recording_id)?.state {
            CardState::Shown(detail) => Some(*detail),
            _ => None,
        }
    }

    /// The transcript of the side panel with what its lines are derived from.
    fn panel(&mut self) -> Option<Panel> {
        let panel = self.view().workspace()?.transcript.clone()?;
        match panel.state {
            TranscriptPanelState::Shown {
                transcript,
                open,
                names,
                when,
            } => Some((*transcript, open, names, when)),
            _ => None,
        }
    }

    /// One line of the transcript of the side panel, as the panel derives it.
    fn panel_line(&mut self, line: u32) -> Option<LineRow> {
        let (_, open, names, when) = self.panel()?;
        let held = open.line(line)?.clone();
        Some(line_row(&open, &held, &names, when))
    }

    fn type_transcript(&mut self, typed: &str) {
        self.in_shell(|shell, window, cx| shell.type_transcript_text(typed, window, cx));
    }
}

/// Opens the transcript of a recording in the side panel once its card shows it.
fn open_panel(window: &mut Window, recording_id: RecordingId, lines: usize) {
    window.click_channel("Daily");
    window.until("the card of the recording with its transcript", |window| {
        window.card_detail(recording_id).is_some_and(|detail| {
            matches!(
                detail.transcript.state,
                TranscriptState::Ready | TranscriptState::Partial
            ) && detail.transcript.held == lines
        })
    });
    window.click_named(format!("show-transcript:{recording_id}"));
    window.until("the transcript in the side panel", |window| {
        window
            .panel()
            .is_some_and(|(_, open, _, _)| open.lines.lines.len() == lines)
    });
}

#[gpui::test]
fn a_member_corrects_a_line_names_a_voice_and_reads_who_changed_them(cx: &mut TestAppContext) {
    let Scene {
        world,
        server,
        admin,
        mut window,
        luca,
        own,
        ada,
        voice,
        _directory,
        ..
    } = scene(cx);
    let recording_id = world.run(server.record(voice, &[&admin.handle, &luca]));
    let said = [
        spoken(ada, Some(1), 100, "Thank you all for joining."),
        spoken(own, None, 130, "We start with the quarts review."),
        spoken(ada, None, 160, "Can you hear me?"),
        spoken(own, None, 190, "Yes."),
    ];
    world.run(server.transcribe(recording_id, &said));
    // The lines of the microphone of Ada show her picture: nothing else on screen names her.
    let picture = crate::people::set_avatar(&world, &admin, &crate::people::png(48, 200));
    open_panel(&mut window, recording_id, 4);
    window.until("the picture of Ada at her lines", |window| {
        window.shows_picture(ada, picture) && window.draws_picture(ada)
    });

    // The person corrects the second line. A text that the server would refuse is not
    // sent.
    window.click_named("correct-line:1".to_owned());
    assert!(window.shows("transcript-form-field"));
    window.type_transcript("   ");
    window.click("transcript-form-send");
    assert!(window.shows("transcript-form-invalid"));
    window.type_transcript("We start with the quartz review.");
    // The answer closes the form at a time that the person does not choose: the line
    // under it keeps its place (ADR 0730).
    let under = window.cx.debug_bounds("transcript-line:2").unwrap();
    window.click("transcript-form-send");
    window.until("the corrected line", |window| {
        !window.shows("transcript-form-field")
            && window
                .panel_line(1)
                .is_some_and(|row| row.text == "We start with the quartz review.")
    });
    assert_eq!(window.cx.debug_bounds("transcript-line:2").unwrap(), under);
    // The mark of the correction shows the text of the speech model and who corrected
    // the line.
    let row = window.panel_line(1).unwrap();
    assert!(row.corrected);
    assert_eq!(
        row.original.as_deref(),
        Some("We start with the quarts review.")
    );
    assert_eq!(
        row.changed.as_ref().map(|changed| changed.by.as_str()),
        Some("Luca")
    );
    assert!(!window.shows_named("line-original-text:1".to_owned()));
    window.click_named("line-original:1".to_owned());
    assert!(window.shows_named("line-original-text:1".to_owned()));
    assert!(window.shows_named("line-changed:1".to_owned()));
    window.click_named("line-original:1".to_owned());
    assert!(!window.shows_named("line-original-text:1".to_owned()));

    // The person names the voice of the shared sound in the details, and renames it.
    window.click("transcript-details");
    assert!(window.shows("transcript-details-panel"));
    assert!(!window.shows_named("voice-changed:1".to_owned()));
    for name in ["Noor", "Noor Haddad"] {
        window.click_named("name-voice:1".to_owned());
        window.type_transcript(name);
        window.click("transcript-form-send");
        window.until("the voice has its name", |window| {
            !window.shows("transcript-form-field")
                && window.panel().is_some_and(|(transcript, ..)| {
                    transcript.voices[0].name.as_deref() == Some(name)
                })
        });
    }
    let (transcript, ..) = window.panel().unwrap();
    let named = transcript.voices[0].changed.clone().unwrap();
    assert_eq!(named.by, "Luca");
    assert!(window.shows_named("voice-changed:1".to_owned()));
    // The lines of the voice carry the name, with the label of the voice.
    let first = window.panel_line(0).unwrap();
    assert_eq!(
        (first.name.as_str(), first.was.as_deref()),
        ("Noor Haddad", Some("Speaker 1"))
    );

    // A correction of another person reaches the window by its event, with its author.
    world
        .run(admin.handle.api().correct_transcript_line(
            recording_id,
            2,
            "Can everyone hear me?",
            None,
        ))
        .unwrap();
    window.until("the correction of the other person", |window| {
        window.panel_line(2).is_some_and(|row| {
            row.text == "Can everyone hear me?"
                && row
                    .changed
                    .as_ref()
                    .is_some_and(|changed| changed.by == "Ada")
        })
    });
}

#[gpui::test]
fn the_details_of_a_transcript_state_its_glossary_terms_and_the_result_of_each_audio_file(
    cx: &mut TestAppContext,
) {
    let Scene {
        world,
        server,
        admin,
        mut window,
        luca,
        own,
        ada,
        voice,
        workspace_id,
        _directory,
        ..
    } = scene(cx);
    let glossary = ["Quorvex".to_owned(), "Lumitrax".to_owned()];
    world
        .run(
            admin
                .handle
                .api()
                .set_glossary(workspace_id, &glossary, None),
        )
        .unwrap();
    let recording_id = world.run(server.record(voice, &[&admin.handle, &luca]));
    let said = [
        spoken(own, None, 100, "The Quorvex release."),
        spoken(ada, None, 130, "Agreed."),
    ];
    let written = Written {
        lines: &said,
        glossary_used: &["Quorvex"],
        glossary_cut: &["Lumitrax"],
        damaged: Some(own),
    };
    assert_eq!(
        world.run(server.transcribe_written(recording_id, &written)),
        "partial"
    );
    open_panel(&mut window, recording_id, 2);
    assert!(!window.shows("glossary-used"));
    // The control lies inside the panel at every width of its head.
    let (control, panel) = (
        window.cx.debug_bounds("transcript-details").unwrap(),
        window.cx.debug_bounds("transcript-panel").unwrap(),
    );
    assert!(
        control.left() >= panel.left() && control.right() <= panel.right(),
        "{control:?} {panel:?}"
    );
    window.click("transcript-details");
    assert!(window.shows("glossary-used") && window.shows("glossary-cut"));
    let (transcript, ..) = window.panel().unwrap();
    assert_eq!(transcript.used, ["Quorvex"]);
    assert_eq!(transcript.cut, ["Lumitrax"]);
    // One result for each audio file: the microphone of the person was read in part.
    assert_eq!(transcript.segments.len(), 3, "{:?}", transcript.segments);
    let damaged = transcript
        .segments
        .iter()
        .find(|result| result.title == "Microphone of Luca")
        .unwrap();
    assert!(damaged.failed);
    assert!(
        damaged
            .sentence
            .ends_with("the sound could be decoded only in part"),
        "{}",
        damaged.sentence
    );
    let whole = transcript
        .segments
        .iter()
        .find(|result| result.title == "Microphone of Ada")
        .unwrap();
    assert_eq!(
        (whole.failed, whole.sentence.as_str()),
        (false, "transcribed, 1 line")
    );
    for index in 0..3 {
        assert!(window.shows_named(format!("transcript-segment:{index}")));
    }
    window.click("transcript-details");
    assert!(!window.shows("glossary-used"));
}

#[gpui::test]
fn a_failed_transcript_is_asked_for_again_from_its_card(cx: &mut TestAppContext) {
    let Scene {
        world,
        server,
        admin,
        mut window,
        luca,
        own,
        voice,
        workspace_id,
        _directory,
        ..
    } = scene(cx);
    let recording_id = world.run(server.record(voice, &[&admin.handle, &luca]));
    world.run(server.refuse_transcript(recording_id));
    window.click_channel("Daily");
    window.until("the card states the failed transcript", |window| {
        window
            .card_detail(recording_id)
            .is_some_and(|detail| detail.transcript.state == TranscriptState::Failed)
    });
    // The roles of a member do not hold "manage recordings": the control says so.
    assert_eq!(
        window.recording_card(recording_id).unwrap().request,
        TranscriptRequest::NotAllowed
    );
    assert!(window.shows("request-transcript"));

    give_everyone(
        &world,
        &admin,
        workspace_id,
        WorkspacePermission::ManageRecordings,
    );
    window.until("the person may ask for the transcript", |window| {
        window.recording_card(recording_id).is_some_and(|card| {
            card.request == TranscriptRequest::Allowed { sending: false }
                && matches!(card.state, CardState::Shown(_))
        }) && window.shows("request-transcript")
    });
    window.click("request-transcript");
    window.until("the transcript waits for the worker", |window| {
        window
            .card_detail(recording_id)
            .is_some_and(|detail| detail.transcript.state == TranscriptState::Waiting)
            && window.recording_card(recording_id).unwrap().request == TranscriptRequest::NotOffered
    });
    assert!(!window.shows("request-transcript"));
    world.run(server.transcribe(recording_id, &[spoken(own, None, 100, "Hello again.")]));
    window.until("the transcript of the new request", |window| {
        window.card_detail(recording_id).is_some_and(|detail| {
            detail.transcript.state == TranscriptState::Ready
                && detail.transcript.lines.first().map(|row| row.text.as_str())
                    == Some("Hello again.")
        })
    });
}

/// A time of day "HH:MM:SS" in seconds.
fn seconds(clock: &str) -> i64 {
    let parts: Vec<i64> = clock.split(':').map(|part| part.parse().unwrap()).collect();
    parts[0] * 3600 + parts[1] * 60 + parts[2]
}

#[gpui::test]
fn the_parts_of_a_recording_saved_in_part_state_their_time_ranges(cx: &mut TestAppContext) {
    let Scene {
        world,
        server,
        admin,
        mut window,
        luca,
        voice,
        _directory,
        ..
    } = scene(cx);
    let plan = RecordPlan {
        fail_shared_sound: true,
        ..RecordPlan::default()
    };
    let recording_id = world.run(server.record_with(voice, &[&admin.handle, &luca], plan));
    window.click_channel("Daily");
    let detail = awaited(&mut window, "the card of the recording", |window| {
        window
            .card_detail(recording_id)
            .filter(|detail| matches!(detail.job, JobShown::PartlyFailed { .. }))
    });
    assert!(!window.shows("recording-parts-list"));
    window.click("recording-parts");
    assert!(window.shows("recording-parts-list"));
    for index in 0..detail.parts.len() {
        assert!(window.shows_named(format!("recording-part:{index}")));
    }
    // Each part states the range that the recorder reported for it, in the time of day of
    // the person: the seconds between its start and its end are those of the server.
    let held = world
        .run(admin.handle.api().recording(recording_id))
        .unwrap();
    assert_eq!(detail.parts.len(), held.segments.len());
    for (part, segment) in detail.parts.iter().zip(&held.segments) {
        let span = part.span.clone().unwrap();
        let (from, until) = span.split_once(" – ").unwrap();
        let (started, ended) = (segment.started_at.unwrap(), segment.ended_at.unwrap());
        assert_eq!(
            seconds(until) - seconds(from),
            ended.unix_timestamp() - started.unix_timestamp(),
            "{part:?} {segment:?}"
        );
        assert_eq!(seconds(from) % 60, started.unix_timestamp() % 60);
    }
    // The shared sound failed; the other parts were saved.
    let failed: Vec<&str> = detail
        .parts
        .iter()
        .filter(|part| part.failed)
        .map(|part| part.title.as_str())
        .collect();
    assert_eq!(failed, ["Shared sound of Ada"], "{:?}", detail.parts);
    assert!(
        detail
            .parts
            .iter()
            .any(|part| part.title == text::RECORDING_VIDEO)
    );
    window.press("escape");
    assert!(!window.shows("recording-parts-list"));
}

/// The connected call of this device in the voice channel, as the media engine states it,
/// for the seat that the media server in memory gave the client of the window.
fn show_own_call(window: &mut Window, seat: &Seat, workspace_id: WorkspaceId, voice: ChannelId) {
    use junto_media::engine::{MicrophoneState, OutputState, ShareState};
    let server_id = window.view().selected.unwrap().id;
    let call = ActiveCall {
        camera: junto_media::camera::CameraState::Off,
        cameras: Vec::new(),
        target: CallTarget {
            server_id,
            place: CallPlace::Channel {
                workspace_id,
                channel_id: voice,
            },
        },
        identity: seat.identity.clone(),
        phase: Phase::Connected,
        interrupted_at: None,
        microphone: MicrophoneState::Running,
        access: junto_media::permission::MicrophoneAccess::Allowed,
        input_device: None,
        output: OutputState::Running,
        output_device: None,
        microphone_block: None,
        sound_block: None,
        share: ShareState::Off,
        shares: Vec::new(),
        watched: None,
    };
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    calls.show_view_for_test(CallsView {
        call: Some(call),
        ..CallsView::default()
    });
    window.settle();
}

/// Scrolls the languages of a new recording until a language is in their visible part.
fn reveal_language(window: &mut Window, selector: &'static str) {
    window.until(
        "the language is in the visible part of the list",
        |window| {
            window.cx.run_until_parked();
            let (Some(list), Some(item)) = (
                window.cx.debug_bounds("record-language-list"),
                window.cx.debug_bounds(selector),
            ) else {
                return false;
            };
            if item.top() >= list.top() && item.bottom() <= list.bottom() {
                return true;
            }
            let distance = list.center().y - item.center().y;
            window.cx.simulate_event(ScrollWheelEvent {
                position: list.center(),
                delta: ScrollDelta::Pixels(point(px(0.0), distance)),
                ..Default::default()
            });
            false
        },
    );
}

/// The recording of the voice channel as the server lists it.
fn recording_of(
    world: &World,
    admin: &Admin,
    workspace_id: WorkspaceId,
) -> Option<(RecordingState, String, bool)> {
    let page = world
        .run(admin.handle.api().recordings(workspace_id, None, None))
        .unwrap();
    page.recordings.first().map(|recording| {
        (
            recording.state,
            recording.language.clone(),
            recording.stop.is_some(),
        )
    })
}

#[gpui::test]
fn a_recording_starts_in_a_chosen_language_and_is_cancelled_while_the_recorder_starts(
    cx: &mut TestAppContext,
) {
    let Scene {
        world,
        server,
        admin,
        mut window,
        luca,
        voice,
        workspace_id,
        _directory,
        ..
    } = scene(cx);
    give_everyone(
        &world,
        &admin,
        workspace_id,
        WorkspacePermission::RecordCalls,
    );
    window.click_channel("Daily");
    let seat = world.run(server.enter(voice, &luca, false));
    show_own_call(&mut window, &seat, workspace_id, voice);
    window.until("the person may record", |window| {
        window.shows("start-recording") && !window.shows("no-record-permission")
    });

    // "Record" opens the languages; the choice of one starts the recording with it.
    window.click("start-recording");
    assert!(window.shows("record-language-menu"));
    // A second click on the button closes the list.
    window.click("start-recording");
    assert!(!window.shows("record-language-menu"));
    window.click("start-recording");
    reveal_language(&mut window, "record-language:pt");
    window.click("record-language:pt");
    assert!(!window.shows("record-language-menu"));
    window.until("the recorder starts the recording", |window| {
        window.shows("cancel-recording") && window.shows("recording-indicator")
    });
    let (state, language, _) = recording_of(&world, &admin, workspace_id).unwrap();
    assert_eq!((state, language.as_str()), (RecordingState::Starting, "pt"));

    // "Cancel" stops the recording before the recorder runs it.
    window.click("cancel-recording");
    let stopped = |world: &World| {
        recording_of(world, &admin, workspace_id).is_some_and(|(_, _, stopped)| stopped)
    };
    window.until("the server takes the stop", |_| stopped(&world));
    world.run(server.settle_recordings());
    window.until("the recording no longer runs", |window| {
        !window.shows("cancel-recording")
            && !window.shows("recording-indicator")
            && window.shows("start-recording")
    });
    let (state, _, _) = recording_of(&world, &admin, workspace_id).unwrap();
    assert!(
        !state.is_active(),
        "the cancelled recording is still active: {state:?}"
    );

    // A recording with the language of each sentence detected. "Cancel" becomes "Stop"
    // at the same place when the recorder runs it (ADR 0730).
    window.click("start-recording");
    window.click("record-language:auto");
    window.until("the recorder starts the second recording", |window| {
        window.shows("cancel-recording")
    });
    let cancel = window.cx.debug_bounds("cancel-recording").unwrap();
    world.run(server.tend());
    window.until("the recording runs", |window| {
        window.shows("stop-recording")
    });
    let stop = window.cx.debug_bounds("stop-recording").unwrap();
    assert_eq!(stop.origin, cancel.origin);
    let (state, language, _) = recording_of(&world, &admin, workspace_id).unwrap();
    assert_eq!(
        (state, language.as_str()),
        (RecordingState::Recording, "auto")
    );
    window.click("stop-recording");
    window.until("the server takes the stop", |_| stopped(&world));
    world.run(server.leave(&[(&luca, seat)]));
}

impl Window {
    fn transcript_field(&mut self) -> String {
        self.in_shell(|shell, _, cx| shell.transcript_text(cx))
    }
}

/// Another person corrects a line or names a voice while the form of the person is open:
/// the form shows the change. A form that the person did not change takes the new text and
/// sends nothing; a changed text is sent from the value of now, and replaces it.
#[gpui::test]
fn a_change_of_another_person_while_a_form_is_open_is_shown_in_the_form(cx: &mut TestAppContext) {
    let Scene {
        world,
        server,
        admin,
        mut window,
        luca,
        own,
        ada,
        voice,
        _directory,
        ..
    } = scene(cx);
    let recording_id = world.run(server.record(voice, &[&admin.handle, &luca]));
    let said = [
        spoken(ada, Some(1), 100, "Thank you all for joining."),
        spoken(own, None, 130, "We start with the review."),
        spoken(ada, None, 160, "Can you hear me?"),
        spoken(own, None, 190, "Yes."),
    ];
    world.run(server.transcribe(recording_id, &said));
    open_panel(&mut window, recording_id, 4);
    let api = admin.handle.api();
    let stored = |world: &World, line: usize| {
        world
            .run(api.transcript_lines(recording_id, None, None))
            .unwrap()
            .lines[line]
            .clone()
    };

    // The person opens the form and changes nothing. The other correction arrives in the
    // form; Save then sends nothing, and the correction of the other person stays.
    window.click_named("correct-line:2".to_owned());
    world
        .run(api.correct_transcript_line(recording_id, 2, "Can everyone hear me?", None))
        .unwrap();
    window.until("the change of the other person in the form", |window| {
        window.shows("transcript-form-changed")
            && window.transcript_field() == "Can everyone hear me?"
    });
    window.click("transcript-form-send");
    assert!(!window.shows("transcript-form-field"));
    let kept = stored(&world, 2);
    assert_eq!(
        (kept.text.as_str(), kept.corrected_by),
        ("Can everyone hear me?", Some(ada))
    );

    // The person types a correction, and another one arrives: the text of the person
    // stays, the form tells the other text, and Save replaces it from that text.
    window.click_named("correct-line:3".to_owned());
    window.type_transcript("Yes, I can.");
    world
        .run(api.correct_transcript_line(recording_id, 3, "Yes, we can.", None))
        .unwrap();
    window.until("the other text is told in the form", |window| {
        window.shows("transcript-form-changed") && window.transcript_field() == "Yes, I can."
    });
    window.click("transcript-form-send");
    window.until("the correction of the person", |window| {
        window.panel_line(3).is_some_and(|row| {
            row.text == "Yes, I can."
                && row
                    .changed
                    .as_ref()
                    .is_some_and(|changed| changed.by == "Luca")
        })
    });

    // A voice that another person names while its form is open, without a name before.
    window.click("transcript-details");
    window.click_named("name-voice:1".to_owned());
    world
        .run(api.rename_transcript_speaker(recording_id, 1, "Noor", None))
        .unwrap();
    window.until("the name of the other person in the form", |window| {
        window.shows("transcript-form-changed") && window.transcript_field() == "Noor"
    });
}

/// The window does not follow the server while another person corrects the line of an open
/// form: the server refuses the stale correction, and when the change arrives the form names
/// the new text in place of the refusal. A second Save replaces it knowingly (ADR 0616).
#[gpui::test]
fn a_correction_refused_as_stale_shows_the_new_text_when_it_arrives(cx: &mut TestAppContext) {
    let (
        Scene {
            world,
            server,
            admin,
            mut window,
            luca,
            own,
            ada,
            voice,
            _directory,
            ..
        },
        proxy,
    ) = scene_through_proxy(cx);
    let recording_id = world.run(server.record(voice, &[&admin.handle, &luca]));
    let said = [
        spoken(ada, None, 100, "Good morning."),
        spoken(own, None, 130, "Can you hear me?"),
    ];
    world.run(server.transcribe(recording_id, &said));
    open_panel(&mut window, recording_id, 2);
    window.click_named("correct-line:1".to_owned());
    window.type_transcript("Can you all hear me?");

    proxy.hold_events();
    world
        .run(admin.handle.api().correct_transcript_line(
            recording_id,
            1,
            "Can everyone hear me?",
            None,
        ))
        .unwrap();
    window.click("transcript-form-send");
    window.until("the refusal of the stale correction", |window| {
        window.shows("transcript-form-refused")
    });
    proxy.release_events();
    window.until("the new text in the form", |window| {
        window
            .panel_line(1)
            .is_some_and(|row| row.text == "Can everyone hear me?")
            && window.shows("transcript-form-changed")
    });
    assert!(!window.shows("transcript-form-refused"));
    assert_eq!(window.transcript_field(), "Can you all hear me?");
    assert_eq!(
        window.in_shell(|shell, _, _| shell.transcript_incoming()),
        Some(Incoming::Kept("Can everyone hear me?".to_owned()))
    );

    window.click("transcript-form-send");
    window.until("the correction of the person", |window| {
        !window.shows("transcript-form-field")
            && window.panel_line(1).is_some_and(|row| {
                row.text == "Can you all hear me?"
                    && row
                        .changed
                        .as_ref()
                        .is_some_and(|changed| changed.by == "Luca")
            })
    });
}

/// The account of the window is removed while a correction is open, and the person joins
/// again with another account: nothing of the draft of the first account is shown.
#[gpui::test]
fn a_draft_of_a_correction_does_not_stay_for_another_account(cx: &mut TestAppContext) {
    let Scene {
        world,
        server,
        admin,
        mut window,
        luca,
        own,
        ada,
        voice,
        _directory,
        ..
    } = scene(cx);
    let recording_id = world.run(server.record(voice, &[&admin.handle, &luca]));
    let said = [
        spoken(ada, None, 100, "Good morning."),
        spoken(own, None, 130, "Hello."),
    ];
    world.run(server.transcribe(recording_id, &said));
    open_panel(&mut window, recording_id, 2);
    window.click_named("correct-line:1".to_owned());
    window.type_transcript("A draft of the first account");

    world.run(admin.handle.api().remove_account(own)).unwrap();
    window.until("the removal is shown", |window| window.shows("no-account"));
    let code = admin.invite(&world, 1);
    window.click("join-server");
    window.join_with_code(code.expose());
    window.until_ready();
    assert_ne!(window.own_account(), own);

    open_panel(&mut window, recording_id, 2);
    assert!(!window.shows("transcript-form-field"));
    window.click_named("correct-line:1".to_owned());
    assert_eq!(window.transcript_field(), "Hello.");
}

/// The account of the window asks for a failed transcript again and is removed while the
/// request waits at the proxy. The answer of that request is told to nobody: the card of the
/// next account of the window states no refusal (ADR 0745).
#[gpui::test]
fn the_answer_to_a_request_of_a_removed_account_is_told_to_nobody(cx: &mut TestAppContext) {
    let (
        Scene {
            world,
            server,
            admin,
            mut window,
            luca,
            own,
            voice,
            workspace_id,
            _directory,
            ..
        },
        proxy,
    ) = scene_through_proxy(cx);
    give_everyone(
        &world,
        &admin,
        workspace_id,
        WorkspacePermission::ManageRecordings,
    );
    let recording_id = world.run(server.record(voice, &[&admin.handle, &luca]));
    world.run(server.refuse_transcript(recording_id));
    window.click_channel("Daily");
    window.until("the person may ask for the transcript", |window| {
        window.recording_card(recording_id).is_some_and(|card| {
            card.request == TranscriptRequest::Allowed { sending: false }
                && matches!(card.state, CardState::Shown(_))
        }) && window.shows("request-transcript")
    });
    proxy.hold_request("POST", "/transcript");
    window.click("request-transcript");
    let asked = |proxy: &FaultProxy| {
        proxy
            .all()
            .into_iter()
            .find(|seen| seen.method == "POST" && seen.path.ends_with("/transcript"))
    };
    window.until("the request waits at the proxy", |_| {
        asked(&proxy).is_some()
    });

    world.run(admin.handle.api().remove_account(own)).unwrap();
    window.until("the removal is shown", |window| window.shows("no-account"));
    let code = admin.invite(&world, 1);
    window.click("join-server");
    window.join_with_code(code.expose());
    window.until_ready();
    assert_ne!(window.own_account(), own);
    window.click_channel("Daily");
    window.until("the card for the next account", |window| {
        window
            .recording_card(recording_id)
            .is_some_and(|card| matches!(card.state, CardState::Shown(_)))
    });

    // The request reaches the server with the session of the removed account.
    proxy.release();
    window.until("the server answered the request", |_| {
        asked(&proxy).is_some_and(|seen| seen.answer.is_some())
    });
    // A message of the next account goes to the server and comes back after that answer.
    window.write("After the request");
    window.until_message("After the request");
    let card = window.recording_card(recording_id).unwrap();
    assert_eq!(card.refused, None);
    assert_eq!(card.request, TranscriptRequest::Allowed { sending: false });
}
