//! The changes of a transcript, its details, a new request for it, the parts of a
//! recording, the language and the cancel of a new recording, and the glossary of a
//! workspace in the window against the real server (ADR 0616).
//!
//! The recordings are made by the server of this checkout inside the test process, with
//! its recorder and its media server in memory and a stand-in for the transcription
//! worker (ADR 0723).

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{ScrollDelta, ScrollWheelEvent, TestAppContext, point, px};
use junto_client::state::admin::view::{Listed, Scope, ServerPage};
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

use crate::admin_pages::{awaited, click_in_page, open_page, sel};
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
fn give_everyone(
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
    let admin = owner(&world, &server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
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
    open_panel(&mut window, recording_id, 4);

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
        .run(
            admin
                .handle
                .api()
                .correct_transcript_line(recording_id, 2, "Can everyone hear me?"),
        )
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

/// The glossary page of the shown workspace as the window draws it.
fn glossary_page(
    window: &mut Window,
) -> Option<junto_client::state::admin::view::glossary::GlossaryPage> {
    match window.admin().map(|view| view.scope) {
        Some(Scope::Server {
            page: ServerPage::Glossary(page),
            ..
        }) => Some(page),
        _ => None,
    }
}

fn terms(window: &mut Window) -> Option<Vec<String>> {
    match glossary_page(window)?.terms {
        Listed::Loaded(terms) => Some(terms),
        _ => None,
    }
}

fn list(terms: &[&str]) -> Vec<String> {
    terms.iter().map(|term| (*term).to_owned()).collect()
}

#[gpui::test]
fn a_member_reads_the_glossary_and_a_person_who_manages_it_adds_changes_orders_and_removes_terms(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let proxy = world.run(FaultProxy::start(&server));
    // The owner works through the API; the window of a member reaches the server
    // through the proxy, so that its event stream can be held.
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let workspace_id = window.view().workspace().unwrap().id;
    let api = admin.handle.api();
    let stored = |world: &World| world.run(api.glossary(workspace_id)).unwrap().terms;
    world
        .run(api.set_glossary(workspace_id, &list(&["Quorvex", "Lumitrax"]), None))
        .unwrap();

    // Every member reads the glossary. Without "manage glossary" nothing changes it.
    window.open_administration();
    open_page(&mut window, "admin-nav-glossary", |window| {
        glossary_page(window).is_some()
    });
    window.until("the terms are read", |window| {
        terms(window) == Some(list(&["Quorvex", "Lumitrax"]))
    });
    assert!(window.shows("glossary-row-0") && window.shows("glossary-row-1"));
    assert!(!glossary_page(&mut window).unwrap().may_manage);
    window.click("glossary-add");
    assert!(!window.shows("glossary-term"));

    give_everyone(
        &world,
        &admin,
        workspace_id,
        WorkspacePermission::ManageGlossary,
    );
    window.until("the person may change the glossary", |window| {
        glossary_page(window).is_some_and(|page| page.held().is_none())
            && window.shows("glossary-add")
    });

    // A new term goes to the end of the list.
    click_in_page(&mut window, "glossary-add");
    window.in_shell(|shell, window, cx| shell.type_admin_name("Ysolde Varkonen", window, cx));
    click_in_page(&mut window, "admin-form-send");
    window.until("the new term", |window| {
        terms(window) == Some(list(&["Quorvex", "Lumitrax", "Ysolde Varkonen"]))
            && !window.shows("glossary-term")
    });
    assert_eq!(
        stored(&world),
        list(&["Quorvex", "Lumitrax", "Ysolde Varkonen"])
    );

    // A term that the list holds is refused before it is sent.
    click_in_page(&mut window, "glossary-add");
    window.in_shell(|shell, window, cx| shell.type_admin_name("Quorvex", window, cx));
    click_in_page(&mut window, "admin-form-send");
    assert!(window.shows("admin-form-invalid"));
    click_in_page(&mut window, "admin-form-cancel");

    // A changed term. The answer closes the form: the row under it keeps its place.
    let settled =
        |window: &mut Window| glossary_page(window).is_some_and(|page| page.held().is_none());
    window.until("the page takes changes", settled);
    click_in_page(&mut window, sel("glossary-change", 1));
    window.in_shell(|shell, window, cx| shell.type_admin_name("Lumitrax Ltd", window, cx));
    let under = window.cx.debug_bounds("glossary-row-2").unwrap();
    click_in_page(&mut window, "admin-form-send");
    window.until("the changed term", |window| {
        terms(window) == Some(list(&["Quorvex", "Lumitrax Ltd", "Ysolde Varkonen"]))
            && !window.shows("glossary-term")
    });
    assert_eq!(window.cx.debug_bounds("glossary-row-2").unwrap(), under);

    // The last term moves up by one place.
    window.until("the page takes changes", settled);
    click_in_page(&mut window, sel("glossary-up", 2));
    window.until("the moved term", |window| {
        terms(window) == Some(list(&["Quorvex", "Ysolde Varkonen", "Lumitrax Ltd"]))
    });

    // The first term is removed after a question.
    window.until("the page takes changes", settled);
    click_in_page(&mut window, sel("glossary-remove", 0));
    click_in_page(&mut window, "glossary-remove-cancel");
    assert_eq!(terms(&mut window).unwrap().len(), 3);
    click_in_page(&mut window, sel("glossary-remove", 0));
    click_in_page(&mut window, "glossary-remove-confirm");
    window.until("the removed term", |window| {
        terms(window) == Some(list(&["Ysolde Varkonen", "Lumitrax Ltd"]))
    });
    assert_eq!(stored(&world), list(&["Ysolde Varkonen", "Lumitrax Ltd"]));

    // Another person changes the glossary while the window does not follow the server: a
    // change made from the list of the window is refused, and changes nothing.
    window.until("the page takes changes", settled);
    proxy.hold_events();
    world
        .run(api.set_glossary(workspace_id, &list(&["Kestrel"]), None))
        .unwrap();
    click_in_page(&mut window, sel("glossary-down", 0));
    window.until("the refusal of the change", |window| {
        window.shows("glossary-refused")
    });
    assert_eq!(stored(&world), list(&["Kestrel"]));
    proxy.release_events();
    window.until("the glossary of the other person", |window| {
        terms(window) == Some(list(&["Kestrel"]))
    });
}
