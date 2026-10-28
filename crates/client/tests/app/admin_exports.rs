use super::*;
use junto_client::state::feed::FeedRow;
use junto_client_core::protocol::export::{ExportManifest, ExportState, ExportedRecording};
use junto_client_core::protocol::message::{Message, MessagePlace};
use junto_client_core::transfers::UploadSource;
use junto_server::exports::zip;

use crate::recording_server::{RecordingServer, Spoken};

fn recording_owner(
    cx: &mut TestAppContext,
    server: &RecordingServer,
) -> (tempfile::TempDir, Window, ServerHandle) {
    let (directory, mut window) = named(cx, "Mira");
    window.find_server(&server.address());
    window.join_with_code(&server.owner_code());
    window.until_ready();
    let owner = window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0);
    (directory, window, owner)
}

fn downloads(window: &mut Window) -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().to_path_buf();
    window.state.update(&mut window.cx, |state, _| {
        state.set_download_directory(target)
    });
    directory
}

fn saved_export(window: &mut Window, id: ExportId) -> Vec<zip::Entry> {
    awaited(window, "the export is ready to save", |window| {
        export_line(window, id).filter(|row| row.status == ExportStatus::Ready && row.can_save)
    });
    click_in_page(window, sel("export-save", id));
    let path = awaited(
        window,
        "the export's saved path is shown",
        |window| match export_line(window, id)?.save {
            Some(ExportSave::Saved(path)) if window.shows(sel("export-saved", id)) => Some(path),
            _ => None,
        },
    );
    zip::read(&std::fs::read(path).unwrap()).unwrap()
}

fn content<'a>(entries: &'a [zip::Entry], name: &str) -> &'a [u8] {
    &entries
        .iter()
        .find(|entry| entry.name == name)
        .unwrap_or_else(|| panic!("missing {name}"))
        .content
}

#[gpui::test]
fn the_archived_list_opens_the_channel_and_shows_its_seven_day_warning(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.run(RecordingServer::start("Harbor"));
    let (_directory, mut window, owner) = recording_owner(cx, &server);
    let workspace = default_workspace(&world, &owner);
    let channel = text_channel(&world, &owner, workspace, "old-plans");
    let message = world
        .run(owner.api().send_message(
            PlaceId::Channel(channel),
            &SendMessage {
                document: document("The archived plan"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    world.run(owner.api().archive_channel(channel)).unwrap();
    server.advance(23 * 24 * 60 * 60);
    world.run(server.on(|state| async move {
        junto_server::archive::notices(&state).await.unwrap();
    }));
    let archived = world.run(owner.api().archived_channels(workspace)).unwrap();
    let row = archived
        .iter()
        .find(|row| row.channel.id == channel)
        .unwrap();
    assert_eq!(row.deletes_at, Some(row.archived_at + Duration::days(30)));
    window.open_administration();
    open_page(&mut window, "admin-nav-archive", |window| {
        server_page!(window, Archive).is_some()
    });
    click_in_page(&mut window, sel("archived-open", channel));
    window.until("the archived channel is open", |window| {
        window.admin().is_none()
            && window.view().workspace().is_some_and(|workspace| {
                workspace
                    .channel
                    .as_ref()
                    .is_some_and(|row| row.id == channel)
                    && workspace.archived.is_some()
            })
    });
    assert_eq!(
        window.until_message("The archived plan").message.id,
        message.id
    );
    window.until("the seven-day deletion warning is visible", |window| {
        window.feed().is_some_and(|feed| {
            feed.rows.iter().any(|row| match row {
                FeedRow::System(row) => {
                    row.parts
                        .iter()
                        .map(|part| part.text.as_str())
                        .collect::<String>()
                        == "The server deletes this archived channel in 7 days"
                }
                _ => false,
            })
        })
    });
    let messages = world
        .run(
            owner
                .api()
                .messages(PlaceId::Channel(channel), &Default::default()),
        )
        .unwrap();
    assert!(messages.messages.iter().any(|row| {
        row.system
            .as_ref()
            .is_some_and(|line| line.action == "channel_deletion_due")
    }));
    assert!(!window.shows_channel("old-plans"));
}

#[gpui::test]
fn a_direct_conversation_is_exported_from_its_header_and_saved(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let code = invite(&world, &owner, Vec::new());
    let (_device, member) = joined(&world, &server, &code, "Mira");
    let conversation = world
        .run(owner.api().open_conversation(logged_in(&member)))
        .unwrap();
    let sent = world
        .run(member.api().send_message(
            PlaceId::Direct(conversation.id),
            &SendMessage {
                document: document("A private plan"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    let _downloads = downloads(&mut window);
    let row: &'static str = Box::leak(format!("direct-row:{}", conversation.id).into_boxed_str());
    window.until("the direct conversation has a row", |window| {
        window.shows(row)
    });
    window.click(row);
    window.until_message("A private plan");
    window.click("export-conversation");
    let export = awaited(&mut window, "the direct export is listed", |window| {
        server_page!(window, Exports)?
            .exports
            .loaded()?
            .first()
            .cloned()
    });
    let stored = world.run(owner.api().export(export.id)).unwrap();
    assert!(
        matches!(stored.place, MessagePlace::Direct { conversation_id, .. } if conversation_id == conversation.id)
    );
    let entries = saved_export(&mut window, export.id);
    let messages: Vec<Message> =
        serde_json::from_slice(content(&entries, "messages.json")).unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0], sent);
    let manifest: ExportManifest =
        serde_json::from_slice(content(&entries, "export.json")).unwrap();
    assert_eq!(manifest.place, stored.place);
    assert_eq!(manifest.requested_by, logged_in(&owner));
    assert!(
        std::str::from_utf8(content(&entries, "messages.html"))
            .unwrap()
            .contains("A private plan")
    );
}

/// The navigation counts a ready export until its file is saved, and a restart of the
/// client on the same data directory does not count it again while the server still holds
/// it (ADR 0761).
#[gpui::test]
fn a_saved_export_is_not_counted_again_after_a_restart(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let (directory, mut window, owner) = owner_window(cx, &server);
    let general = window.selected_channel();
    let export = world.run(owner.api().export_channel(general)).unwrap();
    drop(owner);
    let _downloads = downloads(&mut window);
    window.open_administration();
    window.until("the navigation counts the ready export", |window| {
        window.drawn_number("admin-nav-exports").as_deref() == Some("1")
    });
    open_page(&mut window, "admin-nav-exports", |window| {
        server_page!(window, Exports).is_some()
    });
    saved_export(&mut window, export.id);
    window.until("the saved export is not counted", |window| {
        window.drawn_number("admin-nav-exports").is_none()
    });
    let file = window.state.read_with(&window.cx, |state, _| {
        state.client().data_dir().root().join("saved-exports.json")
    });
    let stored = export.id.to_string();
    window.until("the saved export is in the data directory", |_| {
        std::fs::read_to_string(&file).is_ok_and(|text| text.contains(&stored))
    });

    window.close();
    let mut window = crate::harness::open(cx, directory.path());
    window.until_ready();
    window.open_administration();
    open_page(&mut window, "admin-nav-exports", |window| {
        server_page!(window, Exports).is_some()
    });
    awaited(&mut window, "the export is listed as ready", |window| {
        export_line(window, export.id).filter(|row| row.status == ExportStatus::Ready)
    });
    let owner = window
        .state
        .read_with(&window.cx, |state, _| state.client().servers())
        .remove(0);
    assert_eq!(
        world.run(owner.api().export(export.id)).unwrap().state,
        ExportState::Ready
    );
    assert_eq!(window.drawn_number("admin-nav-exports"), None);

    // A removal outside the page takes the export from the list of the server, and the
    // data directory keeps it as saved no longer.
    world.run(owner.api().remove_export(export.id)).unwrap();
    window.until(
        "the export removed outside the page is not kept",
        |window| {
            server_page!(window, Exports)
                .and_then(|page| page.exports.loaded().cloned())
                .is_some_and(|rows| rows.iter().all(|row| row.id != export.id))
                && std::fs::read_to_string(&file).is_ok_and(|text| !text.contains(&stored))
        },
    );
}

#[gpui::test]
fn a_saved_channel_export_contains_messages_threads_files_recordings_and_transcripts(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.run(RecordingServer::start("Harbor"));
    let (_directory, mut window, owner) = recording_owner(cx, &server);
    let workspace = default_workspace(&world, &owner);
    let channel = world
        .run(owner.api().create_channel(
            workspace,
            &CreateChannel {
                name: "Daily".to_owned(),
                kind: ChannelKind::Voice,
                category_id: None,
                position: None,
            },
        ))
        .unwrap();
    let bytes = b"The plan and its details".to_vec();
    let attachment = world
        .run(
            owner
                .transfers()
                .upload(
                    PlaceId::Channel(channel.id),
                    UploadSource::Bytes {
                        name: "plan.txt".to_owned(),
                        bytes: Arc::new(bytes.clone()),
                    },
                )
                .finished(),
        )
        .unwrap();
    let root = world
        .run(owner.api().send_message(
            PlaceId::Channel(channel.id),
            &SendMessage {
                document: document("The plan <draft>"),
                reply_to: None,
                attachments: vec![attachment],
            },
        ))
        .unwrap();
    let reply = world
        .run(owner.api().reply_in_thread(
            root.id,
            &SendMessage {
                document: document("Details in the thread"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    world.run(owner.api().add_reaction(root.id, "👍")).unwrap();
    let recording = world.run(server.record(channel.id, &[&owner]));
    world.run(server.transcribe(
        recording,
        &[Spoken {
            account_id: logged_in(&owner),
            speaker: None,
            start_ms: 0,
            end_ms: 1000,
            text: "The recorded plan.".to_owned(),
        }],
    ));
    let page = world
        .run(
            owner
                .api()
                .messages(PlaceId::Channel(channel.id), &Default::default()),
        )
        .unwrap();
    assert!(!page.has_more);
    let thread = world
        .run(owner.api().thread(root.id, &Default::default()))
        .unwrap();
    assert!(!thread.replies.has_more);
    let mut expected = page.messages;
    expected.extend(thread.replies.messages);
    expected.sort_by_key(|message| message.id);
    let expected_root = expected
        .iter()
        .find(|message| message.id == root.id)
        .unwrap();
    assert_eq!(expected_root.attachments[0].id, attachment);
    assert_eq!(expected_root.reactions[0].emoji, "👍");
    assert!(expected.iter().any(|message| message.system.is_some()));
    let _downloads = downloads(&mut window);
    open_channel_settings(&mut window, "Daily", channel.id);
    click_in_page(&mut window, "channel-nav-archive");
    click_in_page(&mut window, "channel-export");
    let id = awaited(
        &mut window,
        "the export request is shown",
        |window| match window.admin()?.made.as_slice() {
            [Made::Export(id)] => Some(*id),
            _ => None,
        },
    );
    world.run(server.on(|state| async move {
        assert!(
            junto_server::exports::build::run_next(&state)
                .await
                .unwrap()
        );
    }));
    click_in_page(&mut window, "show-exports");
    let entries = saved_export(&mut window, id);
    let manifest: ExportManifest =
        serde_json::from_slice(content(&entries, "export.json")).unwrap();
    let mut messages: Vec<Message> =
        serde_json::from_slice(content(&entries, "messages.json")).unwrap();
    assert_eq!(manifest.export_id, id);
    assert_eq!(
        manifest.place,
        world.run(owner.api().export(id)).unwrap().place
    );
    assert_eq!(manifest.messages, expected.len() as u64);
    messages.sort_by_key(|message| message.id);
    assert_eq!(messages, expected);
    assert_eq!((manifest.attachments, manifest.recordings), (1, 1));
    assert_eq!(manifest.requested_by, logged_in(&owner));
    let exported_root = messages.iter().find(|row| row.id == root.id).unwrap();
    assert_eq!(exported_root.document, root.document);
    assert_eq!(exported_root.reactions[0].count, 1);
    assert_eq!(
        messages
            .iter()
            .find(|row| row.id == reply.id)
            .unwrap()
            .thread_root,
        Some(root.id)
    );
    assert!(messages.iter().any(|row| row.system.is_some()));
    assert_eq!(
        content(&entries, &format!("attachments/{attachment}/plan.txt")),
        bytes
    );
    let html = std::str::from_utf8(content(&entries, "messages.html")).unwrap();
    assert!(html.contains("The plan &lt;draft&gt;") && html.contains("Details in the thread"));
    let description: ExportedRecording = serde_json::from_slice(content(
        &entries,
        &format!("recordings/{recording}/recording.json"),
    ))
    .unwrap();
    assert_eq!(
        description.recording,
        world.run(owner.api().recording(recording)).unwrap()
    );
    assert!(description.transcript);
    assert_eq!(description.files.len(), 3);
    for file in &description.files {
        let expected = std::fs::read(
            server
                .recordings_dir()
                .join(recording.to_string())
                .join(&file.name),
        )
        .unwrap();
        assert_eq!(
            content(&entries, &format!("recordings/{recording}/{}", file.name)),
            expected
        );
    }
    assert!(
        std::str::from_utf8(content(
            &entries,
            &format!("recordings/{recording}/transcript.txt")
        ))
        .unwrap()
        .contains("The recorded plan.")
    );
}

#[gpui::test]
fn the_recordings_page_shows_rows_loads_older_and_refreshes_after_an_event(
    cx: &mut TestAppContext,
) {
    use junto_client::state::admin::view::recordings::RecordingsRead;
    use junto_client_core::protocol::recording::RecordingState;
    let world = World::new();
    let server = world.run(RecordingServer::start("Harbor"));
    let (_directory, mut window, owner) = recording_owner(cx, &server);
    let workspace = default_workspace(&world, &owner);
    let channel = world
        .run(owner.api().create_channel(
            workspace,
            &CreateChannel {
                name: "Daily".to_owned(),
                kind: ChannelKind::Voice,
                category_id: None,
                position: None,
            },
        ))
        .unwrap();
    let account = logged_in(&owner);
    world.run(server.on(move |state| async move {
        sqlx::query(
            "insert into recordings
            (id, workspace_id, channel_id, call_id, room_name, state, started_by, language,
             requested_at, ended_at, failure)
            select gen_random_uuid(), $1, $2, gen_random_uuid(),
                   'jr-000000000000-00000000000000000000000000000000', 'failed', $3, 'auto',
                   $4, $4, 'nothing_recorded' from generate_series(1, 51)",
        )
        .bind(workspace.as_uuid())
        .bind(channel.id.as_uuid())
        .bind(account.as_uuid())
        .bind(state.now())
        .execute(state.database.pool())
        .await
        .unwrap();
    }));
    let stored = world
        .run(owner.api().recordings(workspace, None, Some(100)))
        .unwrap()
        .recordings;
    assert_eq!(stored.len(), 51);
    window.open_administration();
    open_page(&mut window, "admin-nav-recordings", |window| {
        server_page!(window, Recordings).is_some()
    });
    let page = awaited(
        &mut window,
        "the first page of recordings is shown",
        |window| {
            server_page!(window, Recordings)
                .filter(|page| page.state == RecordingsRead::Loaded && page.rows.len() == 50)
        },
    );
    assert!(page.older);
    assert_eq!(
        page.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        stored[..50].iter().map(|row| row.id).collect::<Vec<_>>()
    );
    assert!(
        page.rows
            .iter()
            .all(|row| row.channel.as_deref() == Some("Daily")
                && row.state == RecordingState::Failed)
    );
    assert!(window.shows(sel("recording-row", stored[0].id)));
    click_in_page(&mut window, "recordings-older");
    let page = awaited(&mut window, "the older recording is shown", |window| {
        server_page!(window, Recordings)
            .filter(|page| page.state == RecordingsRead::Loaded && page.rows.len() == 51)
    });
    assert!(!page.older);
    assert_eq!(
        page.rows.iter().map(|row| row.id).collect::<Vec<_>>(),
        stored.iter().map(|row| row.id).collect::<Vec<_>>()
    );
    reveal_in_page(&mut window, sel("recording-row", stored[50].id));
    reveal_in_page(&mut window, sel("recording-row", stored[0].id));
    assert!(window.shows(sel("recording-row", stored[0].id)));
    world
        .run(owner.api().remove_recording(stored[0].id))
        .unwrap();
    let mut clock = std::time::Instant::now();
    window.until("the removal event refreshes the first page", |window| {
        let now = std::time::Instant::now();
        window.cx.executor().advance_clock(now - clock);
        clock = now;
        server_page!(window, Recordings).is_some_and(|page| {
            page.state == RecordingsRead::Loaded
                && page.rows.len() == 50
                && page.rows.iter().all(|row| row.id != stored[0].id)
        })
    });
    reveal_in_page(&mut window, sel("recording-row", stored[1].id));
    assert!(!window.shows(sel("recording-row", stored[0].id)));
    assert_eq!(
        world
            .run(owner.api().recordings(workspace, None, Some(100)))
            .unwrap()
            .recordings
            .len(),
        50
    );
}
