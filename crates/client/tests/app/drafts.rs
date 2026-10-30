use crate::harness::{World, member, open};
use gpui::TestAppContext;
use junto_client::shell::Pane;

fn quit(mut window: crate::harness::Window) {
    window
        .cx
        .update(|_, cx| junto_client::app::save_drafts_at_quit(cx));
    window.close();
}

#[gpui::test]
fn a_refused_edit_keeps_its_text_and_edit_target_after_restart(cx: &mut TestAppContext) {
    use crate::support::{Fault, FaultProxy};
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.write("original message");
    let id = window.until_message("original message").message.id;
    window.right_click_named(format!("message:{id}"));
    window.click("message-edit");
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "precious refused edit 日本語", window, cx)
    });
    proxy.fail(&format!("/api/v1/messages/{id}"), [Fault::Refused]);
    window.press("enter");
    window.until("the edit is refused", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.message_refusal().is_some())
    });
    quit(window);
    let mut window = open(cx, directory.path());
    window.until_ready();
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "precious refused edit 日本語"
    );
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main).1),
        Some(id)
    );
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));
    window.press("enter");
    window.until_message("precious refused edit 日本語");
    window.until("the accepted edit clears its composer", |window| {
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx).is_empty())
    });
    quit(window);
    let mut window = open(cx, directory.path());
    window.until_ready();
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        ""
    );
}

#[gpui::test]
fn an_edit_acceptance_does_not_clear_a_newer_edit_draft(cx: &mut TestAppContext) {
    use crate::support::FaultProxy;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    window.write("original message");
    let id = window.until_message("original message").message.id;
    window.right_click_named(format!("message:{id}"));
    window.click("message-edit");
    window
        .in_shell(|shell, window, cx| shell.type_message(Pane::Main, "submitted edit", window, cx));
    proxy.hold_answer_ending("PUT", &format!("/messages/{id}"));
    window.press("enter");
    window.until_message("submitted edit");
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "newer edit 日本語", window, cx)
    });
    proxy.release_answers();
    window.until("the edit answer arrives", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.message_asks_flying() == 0)
    });
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main).1),
        Some(id)
    );
    quit(window);
    let mut window = open(cx, directory.path());
    window.until_ready();
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "newer edit 日本語"
    );
    assert_eq!(
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main).1),
        Some(id)
    );
}

#[gpui::test]
fn channel_and_thread_drafts_survive_a_process_restart_and_clear_after_send(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let root = admin.send(
        &world,
        window.selected_channel(),
        crate::messages::document("draft discussion"),
    );
    window.until_message("draft discussion");
    window.type_into("composer-input", "日本語 **kept main**");
    window.right_click_named(format!("message:{}", root.id));
    window.click("message-thread");
    window.until("the thread opens", |window| {
        window.shows("thread-composer-input")
    });
    window.type_into("thread-composer-input", "kept thread");
    window
        .cx
        .update(|_, cx| junto_client::app::save_drafts_at_quit(cx));
    let store = window.state.read_with(&window.cx, |state, _| {
        junto_client_core::drafts::DraftStore::new(state.client().data_dir().clone())
    });
    let saved = store.load::<serde_json::Value>().unwrap();
    assert!(
        saved[0]["texts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|draft| draft["text"] == "kept thread")
    );
    window.click("close-thread");
    assert!(window.shows_named(format!("draft-thread:{}", root.id)));
    quit(window);
    let mut window = open(cx, directory.path());
    window.until_ready();
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "日本語 **kept main**"
    );
    let channel = window.selected_channel();
    assert!(window.shows_named(format!("draft-channel:{channel}")));
    window.click_named(format!("draft-thread:{}", root.id));
    window.until("the restored thread opens", |window| {
        window.shows("thread-composer-input")
    });
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Thread, cx)),
        "kept thread"
    );
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));
    window.press("enter");
    window.until_message("日本語 kept main");
    assert!(!window.shows_named(format!("draft-channel:{channel}")));
    quit(window);
    let mut window = open(cx, directory.path());
    window.until_ready();
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        ""
    );
}

#[gpui::test]
fn closing_and_reopening_the_main_shell_keeps_drafts_while_state_lives(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.type_into("composer-input", "close and reopen");
    let state = window.state.clone();
    let old = window.shell.downgrade();
    let mut reopened = window.reopen_shell(cx);
    assert!(old.upgrade().is_none());
    assert_eq!(
        reopened.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "close and reopen"
    );
    let mut newer = reopened.another_shell(cx);
    assert_eq!(
        newer.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "close and reopen"
    );
    newer.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "newer window draft", window, cx)
    });
    let stale = reopened.shell.downgrade();
    reopened.cx.update(|window, _| window.remove_window());
    drop(reopened);
    for _ in 0..3 {
        cx.update(|_| {});
    }
    assert!(stale.upgrade().is_none());
    newer.run_tasks();
    cx.update(junto_client::state::finish_writes);
    let store = state.read_with(cx, |state, _| {
        junto_client_core::drafts::DraftStore::new(state.client().data_dir().clone())
    });
    let saved = store.load::<serde_json::Value>().unwrap();
    assert!(
        saved[0]["texts"]
            .as_array()
            .unwrap()
            .iter()
            .any(|draft| draft["text"] == "newer window draft"),
        "{saved:?}"
    );
    newer.in_shell(|shell, window, cx| {
        assert_eq!(shell.composer_text(Pane::Main, cx), "newer window draft");
        shell.type_message(Pane::Main, "", window, cx);
    });
    drop(state);
    newer.close();
}

#[gpui::test]
fn replies_edits_and_selected_files_recover_after_restart(cx: &mut TestAppContext) {
    use junto_client_core::transfers::UploadStage;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let root = admin.send(
        &world,
        window.selected_channel(),
        crate::messages::document("reply target"),
    );
    window.until_message("reply target");
    window.right_click_named(format!("message:{}", root.id));
    window.click("message-reply");
    window.type_into("composer-input", "kept reply");
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("kept.txt");
    std::fs::write(&path, b"draft file").unwrap();
    window.in_shell(|shell, _, cx| shell.attach_paths(Pane::Main, vec![path.clone()], cx));
    window.until("the file is uploaded", |window| {
        window
            .view()
            .lists()
            .unwrap()
            .draft
            .iter()
            .any(|file| matches!(file.progress.stage, UploadStage::Complete(_)))
    });
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));
    let picture = crate::harness::picture_png(24, 24);
    window
        .cx
        .write_to_clipboard(gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(
            gpui::ImageFormat::Png,
            picture,
        )));
    window.press("secondary-v");
    window.until("both draft files are uploaded", |window| {
        window.view().lists().unwrap().draft.len() == 2
            && window
                .view()
                .lists()
                .unwrap()
                .draft
                .iter()
                .all(|file| matches!(file.progress.stage, UploadStage::Complete(_)))
    });
    quit(window);
    let mut window = open(cx, directory.path());
    window.until_ready();
    window.until("the draft file is restored", |window| {
        window.view().lists().unwrap().draft.len() == 2
            && window
                .view()
                .lists()
                .unwrap()
                .draft
                .iter()
                .all(|file| matches!(file.progress.stage, UploadStage::Complete(_)))
    });
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));
    window.press("enter");
    let reply = window.until_message("kept reply");
    assert_eq!(reply.message.reply_to, Some(root.id));
    assert_eq!(reply.message.attachments.len(), 2);
    window.right_click_named(format!("message:{}", reply.message.id));
    window.click("message-edit");
    window.in_shell(|shell, window, cx| shell.type_message(Pane::Main, "kept edit", window, cx));
    quit(window);
    let mut window = open(cx, directory.path());
    window.until_ready();
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "kept edit"
    );
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));
    window.press("enter");
    window.until_message("kept edit");
    assert!(
        window
            .messages()
            .iter()
            .any(|row| row.message.id == reply.message.id && row.message.edited_at.is_some())
    );
}

#[gpui::test]
fn missing_recovered_files_remain_visible_until_removed(cx: &mut TestAppContext) {
    use junto_client_core::transfers::UploadStage;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("gone.txt");
    std::fs::write(&path, b"removed before restart").unwrap();
    window.type_into("composer-input", "keep this text");
    window.in_shell(|shell, _, cx| shell.attach_paths(Pane::Main, vec![path.clone()], cx));
    window.until("the original file is uploaded", |window| {
        window
            .view()
            .lists()
            .unwrap()
            .draft
            .iter()
            .any(|file| matches!(file.progress.stage, UploadStage::Complete(_)))
    });
    quit(window);
    std::fs::remove_file(path).unwrap();
    let mut window = open(cx, directory.path());
    window.until_ready();
    window.until("the missing file is a failed draft attachment", |window| {
        window.view().lists().unwrap().draft.iter().any(|file| {
            file.name == "gone.txt" && matches!(file.progress.stage, UploadStage::Failed(_))
        })
    });
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "keep this text"
    );
    window.click_named("remove-file:0".to_owned());
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));
    window.press("enter");
    window.until_message("keep this text");
    assert!(
        window
            .messages()
            .last()
            .unwrap()
            .message
            .attachments
            .is_empty()
    );
}

#[gpui::test]
fn conversation_tokens_and_channel_drafts_stay_separate_after_restart(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let general = window.selected_channel();
    window.type_into("composer-input", "channel draft");
    window.open_conversation_with(admin.handle.account().unwrap(), "Ada");
    window.type_into("composer-input", "Read #gen");
    window.until("the channel can be referenced", |window| {
        window
            .in_shell(|shell, _, _| shell.mention_names(Pane::Main))
            .contains(&"general".to_owned())
    });
    assert!(window.in_shell(|shell, window, cx| shell.pick_mention(
        Pane::Main,
        "general",
        window,
        cx
    )));
    quit(window);
    let mut window = open(cx, directory.path());
    window.until_ready();
    window.open_conversation_with(admin.handle.account().unwrap(), "Ada");
    assert!(
        window
            .in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx))
            .contains("Read #general")
    );
    window.in_shell(|shell, window, cx| shell.focus_composer(Pane::Main, window, cx));
    window.press("enter");
    window.until_message("Read #general");
    let document = window
        .messages()
        .last()
        .unwrap()
        .message
        .document
        .clone()
        .unwrap();
    assert!(document.blocks.iter().any(|block| match block { junto_client_core::protocol::message::Block::Paragraph { content } => content.iter().any(|node| matches!(node, junto_client_core::protocol::message::Inline::Channel { channel_id } if *channel_id == general)), _ => false }));
    window.click_channel("general");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "channel draft"
    );
}

#[gpui::test]
fn a_draft_from_another_entry_or_account_is_not_restored(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.type_into("composer-input", "a private draft");
    let store = window.state.read_with(&window.cx, |state, _| {
        junto_client_core::drafts::DraftStore::new(state.client().data_dir().clone())
    });
    quit(window);
    cx.update(junto_client::state::finish_writes);
    let original = store.load::<serde_json::Value>().unwrap();
    let mut replaced = original.clone();
    let generation = replaced[0]["texts"][0]["entry_generation"]
        .as_u64()
        .unwrap();
    replaced[0]["texts"][0]["entry_generation"] = generation.wrapping_add(1).into();
    store.save(&replaced).unwrap();
    let mut window = open(cx, directory.path());
    window.until_ready();
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        ""
    );
    quit(window);
    cx.update(junto_client::state::finish_writes);
    let mut replaced = original;
    replaced[0]["texts"][0]["desk"][1] = junto_client_core::protocol::ids::AccountId::generate()
        .to_string()
        .into();
    store.save(&replaced).unwrap();
    let mut window = open(cx, directory.path());
    window.until_ready();
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        ""
    );
}

#[gpui::test]
fn completed_and_saved_picture_buffers_are_released(cx: &mut TestAppContext) {
    use junto_client_core::{
        model::PlaceId,
        transfers::{UploadSource, UploadStage},
    };
    use std::sync::Arc;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let server = window.view().selected.unwrap().id;
    let place = PlaceId::Channel(window.selected_channel());
    let bytes = Arc::new(b"a pasted buffer".to_vec());
    let released = Arc::downgrade(&bytes);
    let measured = junto_client::state::measure(vec![UploadSource::Bytes {
        name: "buffer.txt".to_owned(),
        bytes,
    }]);
    window.cx.update(|_, cx| {
        window.state.update(cx, |state, cx| {
            assert!(state.attach(server, place, measured, cx).is_empty());
        })
    });
    window.until("the buffer upload completes", |window| {
        window
            .view()
            .lists()
            .unwrap()
            .draft
            .iter()
            .any(|file| matches!(file.progress.stage, UploadStage::Complete(_)))
    });
    window
        .cx
        .update(|_, cx| junto_client::app::save_drafts_at_quit(cx));
    window.until(
        "saved draft bookkeeping no longer holds the image buffer",
        |_| released.upgrade().is_none(),
    );
}
