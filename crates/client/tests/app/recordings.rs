//! A finished recording in the window against the real server (ADR 0744, view G of the
//! design). The server runs inside the test process with its recorder in memory, so the
//! files of the recording are real media files that the server processed.

use std::path::Path;
use std::time::Duration;

use gpui::TestAppContext;
use junto_client::state::feed::FeedRow;
use junto_client::state::recorded::{
    CardDetail, CardState, FileKind, JobShown, LineFrom, RecordingCard, SHOWN_LINES,
};
use junto_client::state::view::TranscriptPanelState;
use junto_client::text;
use junto_client_core::flow::access::AccessAction;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::ids::{RecordingId, RecordingSegmentId};
use junto_client_core::protocol::roles::UpdateWorkspaceRole;
use junto_client_core::protocol::secret::OwnerCode;
use junto_client_core::protocol::structure::ChannelKind;
use junto_client_core::protocol::transcript::TranscriptState;
use junto_client_core::transfers::DownloadStage;

use crate::calls::voice_channel;
use crate::harness::{Admin, Window, World, member};
use crate::recording_server::{RecordingServer, Spoken};
use crate::support::{Device, logged_in};

impl Window {
    /// The card of a recording that the chat of the shown voice channel draws.
    fn card(&mut self, recording_id: RecordingId) -> Option<RecordingCard> {
        self.feed()?.rows.iter().find_map(|row| match row {
            FeedRow::Recording(row) if row.card.recording_id == recording_id => {
                Some(row.card.clone())
            }
            _ => None,
        })
    }

    fn detail(&mut self, recording_id: RecordingId) -> Option<CardDetail> {
        match self.card(recording_id)?.state {
            CardState::Shown(detail) => Some(*detail),
            _ => None,
        }
    }

    /// The files of a recording that its row on the page of the recordings lists.
    fn row_files(&mut self, recording_id: RecordingId) -> Option<CardDetail> {
        let row = recordings_page(self)?
            .rows
            .into_iter()
            .find(|row| row.id == recording_id)?;
        match row.files?.state {
            CardState::Shown(detail) => Some(*detail),
            _ => None,
        }
    }

    /// Waits until a file of the recording is saved, and returns its path.
    #[track_caller]
    fn until_saved(
        &mut self,
        recording_id: RecordingId,
        segment_id: Option<RecordingSegmentId>,
    ) -> std::path::PathBuf {
        self.until_saved_in(Self::detail, recording_id, segment_id)
    }

    /// Waits until a file of the recording is saved as `read` states it, and returns its
    /// path.
    #[track_caller]
    fn until_saved_in(
        &mut self,
        read: fn(&mut Self, RecordingId) -> Option<CardDetail>,
        recording_id: RecordingId,
        segment_id: Option<RecordingSegmentId>,
    ) -> std::path::PathBuf {
        let mut saved = None;
        self.until("the file of the recording is saved", |window| {
            let Some(detail) = read(window, recording_id) else {
                return false;
            };
            let progress = match segment_id {
                Some(segment_id) => detail
                    .files
                    .iter()
                    .find(|file| file.segment_id == segment_id)
                    .and_then(|file| file.download.clone()),
                None => detail.text_download.clone(),
            };
            match progress.map(|progress| progress.stage) {
                Some(DownloadStage::Saved(path)) => {
                    saved = Some(path);
                    true
                }
                Some(DownloadStage::Failed(failure)) => panic!("the download failed: {failure:?}"),
                _ => false,
            }
        });
        saved.unwrap()
    }
}

/// The file of a segment in the directory of the recordings of the server.
fn on_disk(server: &RecordingServer, recording_id: RecordingId, name: &str) -> Vec<u8> {
    std::fs::read(
        server
            .recordings_dir()
            .join(recording_id.to_string())
            .join(name),
    )
    .unwrap()
}

fn names(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[gpui::test]
fn a_recording_is_a_card_with_its_files_its_transcript_and_its_removal(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.run(RecordingServer::start("Acme Studio"));
    let admin = world.run(async {
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
    });
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let server_id = window.view().selected.unwrap().id;
    let own = window.own_account();
    let ada = logged_in(&admin.handle);
    voice_channel(&world, &admin, &mut window, "Daily");
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.name == "Daily")
        .unwrap();
    let downloads = tempfile::tempdir().unwrap();
    let target = downloads.path().to_path_buf();
    window.state.update(&mut window.cx, |state, _| {
        state.set_download_directory(target)
    });

    // Ada and Luca are in a call of the voice channel. Ada shares the sound of a screen,
    // records, and stops. The worker writes three lines.
    let luca = window
        .state
        .read_with(&window.cx, |state, _| state.server(server_id))
        .unwrap();
    let recording_id = world.run(server.record(voice.id, &[&admin.handle, &luca]));
    let spoken = |account_id, speaker, start_ms, text: &str| Spoken {
        account_id,
        speaker,
        start_ms,
        end_ms: start_ms + 20,
        text: text.to_owned(),
    };
    // The worker writes 40 lines: more than the side panel shows at one time.
    let mut said = vec![
        spoken(ada, Some(1), 100, "Thank you all for joining."),
        spoken(own, None, 130, "Hello."),
        spoken(ada, None, 160, "Can you hear me?"),
    ];
    for number in 0..36u64 {
        said.push(spoken(
            own,
            None,
            190 + number * 30,
            &format!("Point {number} of the agenda."),
        ));
    }
    said.push(spoken(
        ada,
        None,
        1_270,
        "The last point is the quartz review.",
    ));
    world.run(server.transcribe(recording_id, &said));

    // The chat of the voice channel shows the recording as a card: its times, its files
    // and the start of its transcript.
    window.click_channel("Daily");
    window.until("the card of the recording with its transcript", |window| {
        window.detail(recording_id).is_some_and(|detail| {
            detail.transcript.state == TranscriptState::Ready && detail.transcript.held == 40
        })
    });
    assert!(window.shows_named(format!("recording:{recording_id}")));
    let detail = window.detail(recording_id).unwrap();
    assert_eq!(detail.channel, "Daily");
    assert_eq!(detail.job, JobShown::Ready);
    assert_eq!(detail.started_by.as_deref(), Some("Ada"));
    let kinds: Vec<(FileKind, Option<&str>)> = detail
        .files
        .iter()
        .map(|file| (file.kind, file.person.as_deref()))
        .collect();
    assert_eq!(kinds.len(), 4, "{kinds:?}");
    assert!(kinds.contains(&(FileKind::Video, None)));
    assert!(kinds.contains(&(FileKind::Microphone, Some("Ada"))));
    assert!(kinds.contains(&(FileKind::Microphone, Some("Luca"))));
    assert!(kinds.contains(&(FileKind::ShareSound, Some("Ada"))));
    assert!(detail.picture.is_some() && detail.bytes.is_some());
    let lines = &detail.transcript.lines;
    assert_eq!(
        (lines[0].name.as_str(), lines[0].voice),
        ("Speaker 1", Some(1))
    );
    assert_eq!(
        lines[0].source,
        LineFrom::Shared {
            of: "Ada".to_owned()
        }
    );
    assert_eq!(
        (lines[1].name.as_str(), &lines[1].source),
        ("Luca", &LineFrom::Microphone)
    );
    assert_eq!(lines.len(), SHOWN_LINES);
    assert_eq!(lines[2].text, "Can you hear me?");
    assert!(window.shows_named(format!("transcript-line:{}", lines[2].line)));
    // The roles of a member do not hold "manage recordings": the control says so.
    assert!(!window.card(recording_id).unwrap().may_remove);

    // "Download" lists the files. The microphone of Luca is saved and is the file that
    // the server holds.
    window.click("download-recording");
    assert!(window.shows("recording-files") && window.shows("recorded-text"));
    let (position, own_file) = detail
        .files
        .iter()
        .enumerate()
        .find(|(_, file)| {
            file.kind == FileKind::Microphone && file.person.as_deref() == Some("Luca")
        })
        .map(|(position, file)| (position, file.clone()))
        .unwrap();
    window.click_named(format!("recorded-file-{position}"));
    assert!(!window.shows("recording-files"));
    let saved = window.until_saved(recording_id, Some(own_file.segment_id));
    assert_eq!(
        saved,
        downloads.path().join(format!("{}.ogg", own_file.stem))
    );
    let held = on_disk(
        &server,
        recording_id,
        &format!("{}.ogg", own_file.segment_id),
    );
    assert_eq!(held.len() as u64, own_file.bytes);
    assert!(std::fs::read(&saved).unwrap() == held);
    assert!(window.shows_named(format!("recorded-download-{position}")));
    // "Show in folder" hands the saved file to the file manager of the system. A test has
    // no file manager: the application keeps the path.
    window.click_named(format!("show-recorded-{}", own_file.segment_id));
    let shown = window
        .state
        .read_with(&window.cx, |state, _| state.kept_shown().to_vec());
    assert_eq!(shown, std::slice::from_ref(&saved));

    // The transcript is saved as a text file with the time and the speaker of each line.
    window.click("download-recording");
    window.click("recorded-text");
    let saved = window.until_saved(recording_id, None);
    assert_eq!(
        saved,
        downloads.path().join(format!("{}.txt", detail.text_stem))
    );
    let written = std::fs::read_to_string(&saved).unwrap();
    assert!(written.contains("Luca: Hello."), "{written}");
    assert!(written.contains("Can you hear me?"), "{written}");
    assert!(
        written.contains("The last point is the quartz review."),
        "{written}"
    );

    // "Open" saves the video and gives it to the player of the system. A test has no
    // player: the application keeps the path.
    window.click("open-recording");
    let video = detail.video.unwrap();
    let saved = window.until_saved(recording_id, Some(video));
    window.until("the video is given to the player", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.kept_opened().to_vec())
            == [saved.clone()]
    });
    assert!(
        std::fs::read(&saved).unwrap() == on_disk(&server, recording_id, &format!("{video}.mp4"))
    );
    assert_eq!(names(downloads.path()).len(), 3);
    let opened = |window: &mut Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.kept_opened().to_vec())
    };
    // "Open" again gives the saved file to the player, without a second download.
    window.click("open-recording");
    assert_eq!(opened(&mut window), [saved.clone(), saved.clone()]);
    assert_eq!(names(downloads.path()).len(), 3);
    // The person deletes the saved video: "Open" saves it again.
    std::fs::remove_file(&saved).unwrap();
    window.click("open-recording");
    window.until("the video is saved and opened again", |window| {
        opened(window).len() == 3
    });
    let again = opened(&mut window)[2].clone();
    assert!(
        std::fs::read(&again).unwrap() == on_disk(&server, recording_id, &format!("{video}.mp4"))
    );
    assert_eq!(names(downloads.path()).len(), 3);

    // "Show all" opens the transcript in the side panel.
    window.click_named(format!("show-transcript:{recording_id}"));
    window.until("the transcript in the side panel", |window| {
        window.shows("transcript-panel")
            && window
                .view()
                .workspace()
                .and_then(|workspace| workspace.transcript.clone())
                .is_some_and(|panel| {
                    matches!(panel.state, TranscriptPanelState::Shown { open, .. }
                        if open.lines.lines.len() == 40)
                })
    });
    // The panel draws the lines in view: the first ones, and not the last one. No line
    // is marked.
    let last = 39;
    assert!(window.shows("transcript-lines"));
    for line in &lines[..3] {
        assert!(window.shows_named(format!("transcript-line:{}", line.line)));
        assert!(!window.shows_named(format!("marked-line:{}", line.line)));
    }
    assert!(!window.shows_named(format!("transcript-line:{last}")));
    window.click("close-transcript");
    assert!(!window.shows("transcript-panel"));

    // A search finds the line of the transcript, and its result opens the transcript at
    // that line.
    window.in_shell(|shell, window, cx| {
        shell.type_search("quartz", window, cx);
        shell.submit_search(window, cx);
    });
    window.until("the line of the transcript is found", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.search.clone())
            .is_some_and(|panel| panel.transcripts().len() == 1)
    });
    window.click("search-transcript-0");
    let at_line = |window: &mut Window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.transcript.clone())
            .is_some_and(|panel| panel.line == Some(last))
            && window.shows_named(format!("marked-line:{last}"))
    };
    // The list goes to the line of the result, which is far below the first rows, and
    // marks it.
    window.until("the transcript at the found line", at_line);
    assert!(!window.shows_named(format!("marked-line:{}", lines[0].line)));
    // The person moves the list away and closes the panel. The same result goes to its
    // line again.
    window.in_shell(|shell, _, cx| shell.show_transcript_row(0, cx));
    window.until("the start of the transcript", |window| {
        !window.shows_named(format!("marked-line:{last}"))
    });
    window.click("close-transcript");
    assert!(!window.shows("transcript-panel"));
    window.click("search-transcript-0");
    window.until("the transcript at the found line again", at_line);
    window.click("close-transcript");
    window.click("close-search");

    // The server refuses the removal by a person without the permission.
    window.state.update(&mut window.cx, |state, cx| {
        state.remove_recording(server_id, recording_id, cx);
    });
    window.until("the refusal of the removal", |window| {
        window.card(recording_id).is_some_and(|card| {
            card.refused.as_deref() == Some(text::RECORDING_REMOVE_NOT_ALLOWED) && !card.removing
        })
    });
    assert!(window.shows("recording-refused"));
    window.click("dismiss-recording-refusal");

    // With "manage recordings" the control asks first and then removes the recording:
    // its card leaves the chat.
    let api = admin.handle.api();
    let read = window
        .detail(recording_id)
        .expect("the card shows the recording");
    let workspace_id = window.view().workspace().unwrap().id;
    let roles = world.run(api.workspace_roles(workspace_id)).unwrap();
    let everyone = roles.iter().find(|role| role.is_everyone).unwrap();
    let mut permissions = everyone.permissions.clone();
    permissions.push(WorkspacePermission::ManageRecordings);
    let update = UpdateWorkspaceRole {
        name: None,
        permissions: Some(permissions),
    };
    world
        .run(api.update_workspace_role(workspace_id, everyone.id, &update))
        .unwrap();
    // Each change of the permissions withdraws the messages of the workspace until the
    // place is read again (ADR 0748). The card draws its control before the lines of its
    // transcript are read again. The lines are added below the control, and the chat
    // keeps its end in place, so the control moves up between its drawing and a press.
    // It is pressed once the card shows everything that it showed before the change.
    window.until(
        "the permission to remove on the card read again",
        |window| {
            window
                .card(recording_id)
                .is_some_and(|card| card.may_remove)
                && window.detail(recording_id).as_ref() == Some(&read)
                && window.shows("remove-recording")
        },
    );
    window.click("remove-recording");
    assert!(window.shows("confirm-remove-recording"));
    window.click("cancel");
    assert!(window.card(recording_id).is_some());
    window.click("remove-recording");
    window.click("confirm");
    window.until("the card is gone", |window| {
        !window.shows("confirm-remove-recording") && window.card(recording_id).is_none()
    });
    // The saved copies stay with the person.
    assert_eq!(names(downloads.path()).len(), 3);
}

#[gpui::test]
fn a_search_result_opens_a_long_transcript_around_its_line_and_reads_the_earlier_lines(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.run(RecordingServer::start("Acme Studio"));
    let admin = world.run(async {
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
    });
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let server_id = window.view().selected.unwrap().id;
    let own = window.own_account();
    voice_channel(&world, &admin, &mut window, "Daily");
    let luca = window
        .state
        .read_with(&window.cx, |state, _| state.server(server_id))
        .unwrap();
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.name == "Daily")
        .unwrap();
    let recording_id = world.run(server.record(voice.id, &[&admin.handle, &luca]));
    // 450 lines: more than the first page of 200. The line that the search finds is the
    // 421st.
    let found = 420u64;
    let said: Vec<Spoken> = (0..450u64)
        .map(|number| Spoken {
            account_id: own,
            speaker: None,
            start_ms: 100 + number * 1_000,
            end_ms: 100 + number * 1_000 + 20,
            text: if number == found {
                "The quartz review is the last point.".to_owned()
            } else {
                format!("Point {number} of the agenda.")
            },
        })
        .collect();
    world.run(server.transcribe(recording_id, &said));
    window.click_channel("Daily");
    window.until("the card of the recording with its transcript", |window| {
        window
            .detail(recording_id)
            .is_some_and(|detail| detail.transcript.state == TranscriptState::Ready)
    });

    // The result shows the time of its line from the start of the recording, and opens
    // the transcript around that line in place of the first page (ADR 0744).
    window.in_shell(|shell, window, cx| {
        shell.type_search("quartz", window, cx);
        shell.submit_search(window, cx);
    });
    let panel = window.until_value("the line of the transcript is found", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.search.clone())
            .filter(|panel| panel.transcripts().len() == 1)
    });
    assert_eq!(
        panel.transcripts()[0].time,
        text::clock_of(100 + found * 1_000)
    );
    window.click("search-transcript-0");
    let line = u32::try_from(found).unwrap();
    let held = |window: &mut Window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.transcript.clone())
            .and_then(|panel| match panel.state {
                TranscriptPanelState::Shown { open, .. } => Some(
                    open.lines
                        .lines
                        .iter()
                        .map(|held| held.line)
                        .collect::<Vec<u32>>(),
                ),
                _ => None,
            })
            .unwrap_or_default()
    };
    window.until("the transcript around the found line, marked", |window| {
        held(window).contains(&line) && window.shows_named(format!("marked-line:{line}"))
    });
    let around = held(&mut window);
    assert!(
        around[0] > 0,
        "the lines were read from the start: {around:?}"
    );
    assert!(around.windows(2).all(|pair| pair[0] < pair[1]));

    // "Show earlier lines" reads the page before the first held line.
    let first = around[0];
    // The control stands at the head of the list.
    window.in_shell(|shell, _, cx| shell.show_transcript_row(0, cx));
    let idle = |window: &mut Window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.transcript.clone())
            .is_some_and(|panel| match panel.state {
                TranscriptPanelState::Shown { transcript, .. } => !transcript.reading,
                _ => false,
            })
    };
    window.until("the head of the list is drawn and no read runs", |window| {
        window.shows("earlier-lines") && idle(window)
    });
    window.click("earlier-lines");
    window.until("the earlier lines are held", |window| {
        held(window).first().is_some_and(|line| *line < first)
    });
    let held_now = held(&mut window);
    assert!(held_now.contains(&first) && held_now.contains(&line));
    assert!(held_now.windows(2).all(|pair| pair[0] < pair[1]));
}

#[gpui::test]
fn a_transcript_read_error_keeps_the_pagination_control_in_place(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.run(RecordingServer::start("Acme Studio"));
    let admin = world.run(async {
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
    });
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let server_id = window.view().selected.unwrap().id;
    let own = window.own_account();
    voice_channel(&world, &admin, &mut window, "Daily");
    let luca = window
        .state
        .read_with(&window.cx, |state, _| state.server(server_id))
        .unwrap();
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.name == "Daily")
        .unwrap();
    let recording_id = world.run(server.record(voice.id, &[&admin.handle, &luca]));
    let found = 420u64;
    let said: Vec<Spoken> = (0..450u64)
        .map(|number| Spoken {
            account_id: own,
            speaker: None,
            start_ms: 100 + number * 1_000,
            end_ms: 100 + number * 1_000 + 20,
            text: if number == found {
                "The quartz review is the last point.".to_owned()
            } else {
                format!("Point {number} of the agenda.")
            },
        })
        .collect();
    world.run(server.transcribe(recording_id, &said));
    window.click_channel("Daily");
    window.until("the card of the recording with its transcript", |window| {
        window
            .detail(recording_id)
            .is_some_and(|detail| detail.transcript.state == TranscriptState::Ready)
    });

    window.click_named(format!("show-transcript:{recording_id}"));
    window.until("the first transcript page is drawn", |window| {
        window
            .view()
            .workspace()
            .and_then(|workspace| workspace.transcript.clone())
            .is_some_and(|panel| {
                matches!(panel.state,
                TranscriptPanelState::Shown { open, transcript, .. }
                if open.lines.lines.len() == 200 && !transcript.reading)
            })
    });
    window.in_shell(|shell, _, cx| shell.show_transcript_row(200, cx));
    window.until("the pagination control is drawn", |window| {
        window.shows("more-lines")
    });
    let before = window.cx.debug_bounds("more-lines").unwrap();
    drop(server);
    window.state.update(&mut window.cx, |state, _| {
        state.read_transcript_lines(server_id, recording_id);
    });
    window.until("the transcript read error is drawn", |window| {
        window.shows("transcript-lines-failed")
    });
    assert_eq!(window.cx.debug_bounds("more-lines"), Some(before));
}

/// A server with the recorder in memory, its owner Ada, and the window of the member Luca
/// with the voice channel "Daily". Returns the identifier of the channel and the client of
/// the server of the window.
fn recorded_scene(
    cx: &mut TestAppContext,
    world: &World,
) -> (
    RecordingServer,
    Admin,
    tempfile::TempDir,
    Window,
    junto_client_core::protocol::ids::ChannelId,
    junto_client_core::ServerHandle,
) {
    let server = world.run(RecordingServer::start("Acme Studio"));
    let admin = world.run(async {
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
    });
    let (directory, mut window) = member(cx, world, &server.address(), &admin, "Luca");
    voice_channel(world, &admin, &mut window, "Daily");
    let voice = window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.name == "Daily")
        .unwrap()
        .id;
    let luca = window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0);
    (server, admin, directory, window, voice, luca)
}

/// The page of the recordings that the open administration shows.
fn recordings_page(
    window: &mut Window,
) -> Option<junto_client::state::admin::view::recordings::RecordingsPage> {
    use junto_client::state::admin::view::{Scope, ServerPage};
    match window.admin()?.scope {
        Scope::Server {
            page: ServerPage::Recordings(page),
            ..
        } => Some(page),
        _ => None,
    }
}

#[gpui::test]
fn the_recordings_page_reads_again_at_the_events_of_a_recording_and_lists_its_content(
    cx: &mut TestAppContext,
) {
    use junto_client::state::admin::view::recordings::RecordingsRead;
    use junto_client_core::protocol::recording::RecordingState;

    let world = World::new();
    let (server, admin, _directory, mut window, voice, luca) = recorded_scene(cx, &world);
    let ada = logged_in(&admin.handle);
    window.open_administration();
    window.until("the page of the recordings is offered", |window| {
        window.shows("admin-nav-recordings")
    });
    window.click("admin-nav-recordings");
    window.until("the page states that there is no recording", |window| {
        window.shows("recordings-empty")
    });

    // Ada and Luca are in a call of "Daily", and Ada records it. The events of the
    // recording make the open page read its list again, without a click.
    let first = world.run(server.record(voice, &[&admin.handle, &luca]));
    // Each event of the recording reads the list again, at most once in a second: the row
    // can be ready while the read of a later event is on its way. The page is loaded when
    // the last read has ended.
    window.until("the page lists the finished recording", |window| {
        window.cx.executor().advance_clock(Duration::from_secs(1));
        recordings_page(window).is_some_and(|page| {
            page.state == RecordingsRead::Loaded
                && page.rows.len() == 1
                && page.rows[0].state == RecordingState::Ready
        })
    });
    let page = recordings_page(&mut window).unwrap();
    assert_eq!(page.state, RecordingsRead::Loaded);
    let row = &page.rows[0];
    assert_eq!(row.id, first);
    assert_eq!(row.channel.as_deref(), Some("Daily"));
    assert_eq!(row.started_by.as_deref(), Some("Ada"));
    assert!(row.started);
    assert_eq!(row.length, Some(std::time::Duration::from_secs(2)));
    // The video of the composite, the two microphones and the shared sound.
    assert_eq!((row.segments, row.failed_segments), (4, 0));
    assert!(row.bytes.is_some_and(|bytes| bytes > 0));
    assert_ne!(row.transcript, TranscriptState::Ready);
    assert!(window.shows_named(format!("recording-row-{first}")));
    assert!(!window.shows("recordings-empty"));

    // The worker writes the transcript: the row tells it.
    let line = Spoken {
        account_id: ada,
        speaker: None,
        start_ms: 100,
        end_ms: 120,
        text: "The plan for the week.".to_owned(),
    };
    world.run(server.transcribe(first, std::slice::from_ref(&line)));
    // A read that an event causes starts at the earliest one second after the read before
    // (ADR 0784). The clock of the test platform moves only when the test moves it.
    window.until_read(
        "the row tells the transcript",
        |window| {
            window.cx.executor().advance_clock(Duration::from_secs(1));
            recordings_page(window).map(|page| {
                page.rows
                    .iter()
                    .map(|row| (row.state, row.transcript, row.transcript_lines))
                    .collect::<Vec<_>>()
            })
        },
        |rows| {
            rows.as_deref()
                .is_some_and(|rows| rows == [(RecordingState::Ready, TranscriptState::Ready, 1)])
        },
    );

    // A second recording comes first: the newest first.
    let second = world.run(server.record(voice, &[&admin.handle, &luca]));
    window.until("the page lists both recordings", |window| {
        window.cx.executor().advance_clock(Duration::from_secs(1));
        recordings_page(window).is_some_and(|page| {
            page.rows.iter().map(|row| row.id).collect::<Vec<_>>() == [second, first]
                && page.rows[0].state == RecordingState::Ready
        })
    });
    assert!(window.shows_named(format!("recording-row-{second}")));
    assert!(window.shows_named(format!("recording-row-{first}")));
}

#[gpui::test]
fn the_recordings_page_saves_a_file_and_the_transcript_of_a_recording(cx: &mut TestAppContext) {
    let world = World::new();
    let (server, admin, _directory, mut window, voice, luca) = recorded_scene(cx, &world);
    let own = window.own_account();
    let downloads = tempfile::tempdir().unwrap();
    let target = downloads.path().to_path_buf();
    window.state.update(&mut window.cx, |state, _| {
        state.set_download_directory(target)
    });
    let recording_id = world.run(server.record(voice, &[&admin.handle, &luca]));
    let line = Spoken {
        account_id: own,
        speaker: None,
        start_ms: 100,
        end_ms: 120,
        text: "The plan for the week.".to_owned(),
    };
    world.run(server.transcribe(recording_id, std::slice::from_ref(&line)));
    let held_open = |window: &mut Window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.client().servers())
            .remove(0)
            .live()
            .view()
            .model
            .is_some_and(|model| model.recording(recording_id).is_some())
    };

    window.open_administration();
    window.until("the page of the recordings is offered", |window| {
        window.shows("admin-nav-recordings")
    });
    window.click("admin-nav-recordings");
    window.until("the row offers the files of the recording", |window| {
        window.cx.executor().advance_clock(Duration::from_secs(1));
        recordings_page(window).is_some_and(|page| {
            page.rows.len() == 1
                && page.rows[0].downloads
                && page.rows[0].transcript == TranscriptState::Ready
        }) && window.shows_named(format!("recording-download-{recording_id}"))
    });
    // The window reads the files of a recording only for the row that lists them.
    assert!(!held_open(&mut window));

    // "Download" lists the video, the two microphones, the shared sound and the
    // transcript in the row.
    window.click_named(format!("recording-download-{recording_id}"));
    let detail = window.until_read(
        "the row lists the files of the recording",
        |window| window.row_files(recording_id),
        |detail| {
            detail
                .as_ref()
                .is_some_and(|detail| detail.files.len() == 4)
        },
    );
    let detail = detail.unwrap();
    assert!(window.shows_named(format!("recording-files-{recording_id}")));
    assert!(window.shows_named(format!("recording-text-{recording_id}")));
    let (position, own_file) = detail
        .files
        .iter()
        .enumerate()
        .find(|(_, file)| {
            file.kind == FileKind::Microphone && file.person.as_deref() == Some("Luca")
        })
        .map(|(position, file)| (position, file.clone()))
        .unwrap();

    // The microphone of Luca is saved and is the file that the server holds.
    window.click_named(format!("recording-file-{recording_id}-{position}"));
    let saved = window.until_saved_in(Window::row_files, recording_id, Some(own_file.segment_id));
    assert_eq!(
        saved,
        downloads.path().join(format!("{}.ogg", own_file.stem))
    );
    let held = on_disk(
        &server,
        recording_id,
        &format!("{}.ogg", own_file.segment_id),
    );
    assert_eq!(held.len() as u64, own_file.bytes);
    assert!(std::fs::read(&saved).unwrap() == held);
    assert!(window.shows_named(format!("recorded-download-{position}")));

    // The transcript is saved as a text file with the speaker of each line.
    window.click_named(format!("recording-text-{recording_id}"));
    let saved = window.until_saved_in(Window::row_files, recording_id, None);
    assert_eq!(
        saved,
        downloads.path().join(format!("{}.txt", detail.text_stem))
    );
    let written = std::fs::read_to_string(&saved).unwrap();
    assert!(
        written.contains("Luca: The plan for the week."),
        "{written}"
    );
    assert_eq!(names(downloads.path()).len(), 2);

    // "Download" again closes the list, and the window releases the recording.
    window.click_named(format!("recording-download-{recording_id}"));
    assert!(!window.shows_named(format!("recording-files-{recording_id}")));
    window.until("the recording is released", |window| !held_open(window));
}

#[gpui::test]
fn the_window_holds_open_the_24_recordings_around_the_place_of_the_list(cx: &mut TestAppContext) {
    use std::collections::HashSet;

    use gpui::{ScrollDelta, ScrollWheelEvent, point, px};
    use junto_client::shell::Pane;

    let world = World::new();
    let (server, admin, _directory, mut window, voice, luca) = recorded_scene(cx, &world);
    let made: Vec<RecordingId> = (0..26)
        .map(|_| world.run(server.record(voice, &[&admin.handle, &luca])))
        .collect();
    let open = |window: &mut Window| -> HashSet<RecordingId> {
        window
            .state
            .read_with(&window.cx, |state, _| state.client().servers())
            .remove(0)
            .live()
            .view()
            .model
            .map(|model| {
                model
                    .recordings
                    .iter()
                    .map(|recording| recording.id)
                    .collect()
            })
            .unwrap_or_default()
    };
    let set = |ids: &[RecordingId]| ids.iter().copied().collect::<HashSet<_>>();
    let shown = |window: &mut Window, recording_id| {
        window
            .card(recording_id)
            .is_some_and(|card| matches!(card.state, CardState::Shown(_)))
    };

    // The wheel over the message list moves it by this much, up or down.
    let wheel = |window: &mut Window, by: f32| {
        let list = window
            .in_shell(|shell, _, _| shell.feed_bounds(Pane::Main))
            .expect("the list was drawn");
        window.dispatch(ScrollWheelEvent {
            position: list.center(),
            delta: ScrollDelta::Pixels(point(px(0.0), px(by))),
            ..Default::default()
        });
    };

    // The chat of "Daily" names each recording in a message. The person moves the list
    // up until the window has read every message.
    window.click_channel("Daily");
    window.until("the list of the chat is drawn", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
            && window
                .in_shell(|shell, _, _| shell.feed_bounds(Pane::Main))
                .is_some()
    });
    window.until_read(
        "the chat holds every recording",
        |window| {
            wheel(window, 4000.0);
            window.feed().map(|feed| {
                let recordings = feed
                    .rows
                    .iter()
                    .filter(|row| matches!(row, FeedRow::Recording(_)))
                    .count();
                (feed.more_before, feed.rows.len(), recordings)
            })
        },
        |read| matches!(read, Some((false, _, count)) if *count == made.len()),
    );

    // The list stands at its first row: the 24 oldest recordings are open, and the two
    // newest are not.
    window.until("the oldest recordings are open", |window| {
        open(window) == set(&made[..24])
    });
    window.until("the card of the oldest recording is shown", |window| {
        shown(window, made[0])
    });
    assert!(!shown(&mut window, made[25]));

    // At the end of the list the 24 newest are open: the two oldest are released.
    window.until_read(
        "the newest recordings are open",
        |window| {
            // The person moves the list down; it reads the newer messages near its end.
            wheel(window, -4000.0);
            let at_end = window.in_shell(|shell, _, _| shell.feed_is_at_end(Pane::Main))
                && window.feed().is_some_and(|feed| !feed.more_after);
            let open = open(window);
            let indices: Vec<usize> = (0..made.len())
                .filter(|index| open.contains(&made[*index]))
                .collect();
            (at_end, indices)
        },
        |(_, indices)| *indices == (2..made.len()).collect::<Vec<_>>(),
    );
    window.until("the card of the newest recording is shown", |window| {
        shown(window, made[25])
    });
    assert!(!shown(&mut window, made[0]));
}
