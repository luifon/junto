use crate::harness::{World, member};
use gpui::TestAppContext;

#[gpui::test]
fn user_edits_show_typing_to_another_reader_and_stop_on_preview_and_send(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_first, mut writer) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_second, mut reader) = member(cx, &world, &server.address(), &admin, "Marta");
    writer.type_into("composer-input", "a work update");
    reader.until("another reader sees who is typing", |reader| {
        reader.shows("typing-indicator")
    });
    assert!(
        reader
            .in_shell(|shell, _, _| shell.painted_readonly_text())
            .iter()
            .any(|text| text.contains("Luca") && text.contains("typing"))
    );
    assert!(!writer.shows("typing-indicator"));
    writer.click("composer-view-1");
    reader.until("preview stops typing activity", |reader| {
        !reader.shows("typing-indicator")
    });
    writer.click("composer-view-0");
    writer.type_into("composer-input", " for today");
    reader.until("editing resumes activity", |reader| {
        reader.shows("typing-indicator")
    });
    writer.press("enter");
    reader.until_message("a work update for today");
    reader.until("sending stops activity", |reader| {
        !reader.shows("typing-indicator")
    });
}

#[gpui::test]
fn several_typers_expire_without_another_edit(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_first, mut first) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_second, mut second) = member(cx, &world, &server.address(), &admin, "Ada");
    let (_third, mut reader) = member(cx, &world, &server.address(), &admin, "Marta");
    first.type_into("composer-input", "first draft");
    second.type_into("composer-input", "second draft");
    reader.until("both people are listed", |reader| {
        reader
            .in_shell(|shell, _, _| shell.painted_readonly_text())
            .iter()
            .any(|text| text.contains("Ada and Luca are typing"))
    });
    reader.until("idle activity expires", |reader| {
        !reader.shows("typing-indicator")
    });
}

#[gpui::test]
fn thread_activity_stays_out_of_its_parent_and_stops_when_closed(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_first, mut writer) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_second, mut reader) = member(cx, &world, &server.address(), &admin, "Marta");
    let root = admin.send(
        &world,
        writer.selected_channel(),
        crate::messages::document("a discussion"),
    );
    for window in [&mut writer, &mut reader] {
        window.until_message("a discussion");
        window.right_click_named(format!("message:{}", root.id));
        window.click("message-thread");
        window.until("the thread composer opens", |window| {
            window.shows("thread-composer-input")
        });
    }
    writer.type_into("thread-composer-input", "thread draft");
    reader.until("the thread reports activity", |reader| {
        reader.shows("thread-typing-indicator")
    });
    assert!(!reader.shows("typing-indicator"));
    writer.click("close-thread");
    reader.until("closing the thread stops activity", |reader| {
        !reader.shows("thread-typing-indicator")
    });
}

#[gpui::test]
fn navigation_and_private_conversation_activity_keep_their_place(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_first, mut writer) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_second, mut reader) = member(cx, &world, &server.address(), &admin, "Marta");
    let (_third, mut other) = member(cx, &world, &server.address(), &admin, "Ada");
    writer.type_into("composer-input", "channel draft");
    reader.until("the channel sees activity", |reader| {
        reader.shows("typing-indicator")
    });
    writer.open_conversation_with(reader.own_account(), "Marta");
    reader.until("navigation stops the channel activity", |reader| {
        !reader.shows("typing-indicator")
    });
    reader.open_conversation_with(writer.own_account(), "Luca");
    writer.type_into("composer-input", "private draft");
    reader.until("the conversation sees activity", |reader| {
        reader.shows("typing-indicator")
    });
    assert!(!other.shows("typing-indicator"));
}

#[gpui::test]
fn restoring_a_draft_does_not_announce_typing(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut writer) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_second, mut reader) = member(cx, &world, &server.address(), &admin, "Marta");
    writer.type_into("composer-input", "kept draft");
    reader.until("editing is announced", |reader| {
        reader.shows("typing-indicator")
    });
    writer
        .cx
        .update(|_, cx| junto_client::app::save_drafts_at_quit(cx));
    writer.close();
    reader.until("closing the writer stops activity", |reader| {
        !reader.shows("typing-indicator")
    });
    let mut writer = crate::harness::open(cx, directory.path());
    writer.until_ready();
    assert_eq!(
        writer.in_shell(|shell, _, cx| shell.composer_text(junto_client::shell::Pane::Main, cx)),
        "kept draft"
    );
    assert!(!reader.shows("typing-indicator"));
}

#[gpui::test]
fn voice_channel_chat_shows_the_same_transient_activity(cx: &mut TestAppContext) {
    use junto_client_core::protocol::structure::{ChannelKind, CreateChannel};
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_first, mut writer) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_second, mut reader) = member(cx, &world, &server.address(), &admin, "Marta");
    let workspace = writer.view().workspace().unwrap().id;
    world
        .run(admin.handle.api().create_channel(
            workspace,
            &CreateChannel {
                name: "voice-chat".to_owned(),
                topic: String::new(),
                kind: ChannelKind::Voice,
                category_id: None,
                position: None,
            },
        ))
        .unwrap();
    for window in [&mut writer, &mut reader] {
        window.until("the voice channel is listed", |window| {
            window.shows_channel("voice-chat")
        });
        window.click_channel("voice-chat");
        window.until("the voice chat history loads", |window| {
            window.feed().is_some_and(|feed| feed.loaded)
        });
    }
    writer.type_into("composer-input", "voice chat draft");
    reader.until("voice chat has the typing indicator", |reader| {
        reader.shows("typing-indicator")
    });
    writer.press("secondary-a");
    writer.press("backspace");
    reader.until("clearing the draft stops activity", |reader| {
        !reader.shows("typing-indicator")
    });
}

#[gpui::test]
fn deactivating_the_window_sends_an_explicit_stop(cx: &mut TestAppContext) {
    use junto_client_core::live::Notice;
    use std::time::Duration;
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_first, mut writer) = member(cx, &world, &server.address(), &admin, "Luca");
    let (_second, mut reader) = member(cx, &world, &server.address(), &admin, "Marta");
    writer.cx.update(|window, _| window.activate_window());
    writer.settle();
    writer.type_into("composer-input", "a draft in the active window");
    reader.until("the reader sees activity", |reader| {
        reader.shows("typing-indicator")
    });
    let server_id = reader.view().selected.unwrap().id;
    let place =
        junto_client_core::model::PlaceId::Channel(writer.selected_channel()).typing_place();
    let mut notices = reader.state.read_with(&reader.cx, |state, _| {
        state.server(server_id).unwrap().live().notices()
    });
    writer.simulate(|cx| cx.deactivate_window());
    world.run(async {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                if let Some(Ok(Notice::Typing(Some(activity)))) = notices.next().await
                    && activity.place == place
                    && !activity.active
                {
                    break;
                }
            }
        })
        .await
        .expect("deactivation must send a stop, not wait for the display to expire");
    });
    reader.until("deactivation clears activity", |reader| {
        !reader.shows("typing-indicator")
    });
}
