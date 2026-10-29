use crate::harness::{World, member};
use gpui::TestAppContext;
use junto_client::shell::Pane;

#[gpui::test]
fn headings_small_text_and_spoilers_are_stored_and_spoilers_reveal_without_editing(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let before = window.messages().len();
    window.type_into(
        "composer-input",
        "# Review\n-# A quiet note\n||the **private** result|| and ||a second result||",
    );
    window.press("enter");
    window.until("the formatted message is sent", |window| {
        window.messages().len() > before
    });
    let message = window.messages().last().unwrap().message.clone();
    let document = message.document.as_ref().unwrap();
    let json = serde_json::to_value(document).unwrap();
    assert_eq!(json["blocks"][0]["kind"], "heading");
    assert_eq!(json["blocks"][0]["level"], 1);
    assert_eq!(json["blocks"][1]["kind"], "small");
    assert_eq!(json["blocks"][2]["content"][0]["kind"], "spoiler");
    window.until("only the spoiler control is painted", |window| {
        let text = window
            .in_shell(|shell, _, _| shell.painted_readonly_text())
            .join("\n");
        text.contains("Spoiler") && !text.contains("private")
    });
    window.type_into("composer-input", "unfinished work");
    let point = window
        .in_shell(|shell, _, _| shell.readonly_text_point("Spoiler", 2))
        .unwrap();
    window.press_at(point, gpui::MouseButton::Left);
    window.until("the spoiler is revealed", |window| {
        window
            .in_shell(|shell, _, _| shell.painted_readonly_text())
            .iter()
            .any(|text| text.contains("the private result"))
    });
    window.press("enter");
    window.until("the next spoiler reveals with the keyboard", |window| {
        window
            .in_shell(|shell, _, _| shell.painted_readonly_text())
            .iter()
            .any(|text| text.contains("a second result"))
    });
    assert!(!window.shows("message-menu"));
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "unfinished work"
    );
    assert_eq!(
        world
            .run(admin.handle.api().message(message.id))
            .unwrap()
            .document
            .as_ref(),
        Some(document)
    );
    window.right_click_named(format!("message:{}", message.id));
    window.click("message-copy-text");
    let copied = window.cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(
        copied.contains("Review\nA quiet note\nthe private result"),
        "{copied}"
    );
}

#[gpui::test]
fn editing_formatted_text_preserves_nested_spoilers_and_escaped_marks(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let before = window.messages().len();
    window.type_into("composer-input", "### Details\n-# Small `code`\n||**secret** and [guide](https://example.test/guide)||\n\\||literal\\||");
    window.press("enter");
    window.until("the marked text is sent", |window| {
        window.messages().len() > before
    });
    let message = window.messages().last().unwrap().message.clone();
    let json = serde_json::to_value(&message.document).unwrap();
    assert_eq!(json["blocks"][0]["kind"], "heading");
    window.in_shell(|shell, window, cx| shell.edit_message(Pane::Main, message.id, window, cx));
    let written = window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx));
    assert!(written.contains("### Details"), "{written}");
    assert!(written.contains("-# Small `code`"), "{written}");
    assert!(
        written.contains("||**secret** and [guide](https://example.test/guide)||"),
        "{written}"
    );
    window.press("enter");
    window.until("the edit completes", |window| {
        window.in_shell(|shell, _, _| shell.compose_state(Pane::Main).1.is_none())
    });
    assert_eq!(
        world
            .run(admin.handle.api().message(message.id))
            .unwrap()
            .document,
        message.document
    );
}

#[gpui::test]
fn formatting_tools_work_in_the_main_and_thread_composers(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.type_into("composer-input", "a surprise");
    window.press("secondary-a");
    window.click("format-spoiler");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "||a surprise||"
    );
    window.press("enter");
    window.until("the spoiler tool sends a typed spoiler", |window| {
        window.messages().iter().any(|row| {
            serde_json::to_value(&row.message.document)
                .unwrap()
                .to_string()
                .contains("\"kind\":\"spoiler\"")
        })
    });
    window.type_into("composer-input", "Section");
    window.click("format-heading");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "# Section"
    );
    window.press("enter");
    window.until("the heading tool sends a typed heading", |window| {
        window.messages().iter().any(|row| {
            serde_json::to_value(&row.message.document).unwrap()["blocks"][0]["kind"] == "heading"
        })
    });
    let root = admin.send(
        &world,
        window.selected_channel(),
        crate::messages::document("a discussion"),
    );
    window.until_message("a discussion");
    window.right_click_named(format!("message:{}", root.id));
    window.click("message-thread");
    window.until("the thread composer opens", |window| {
        window.shows("thread-composer-input")
    });
    window.type_into("thread-composer-input", "a quiet note");
    window.click("thread-format-small");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Thread, cx)),
        "-# a quiet note"
    );
    window.press("enter");
    window.until("the thread sends small text", |window| {
        window.replies().contains(&"a quiet note".to_owned())
    });
    window.until("the thread has a small-text block", |window| window.view().workspace().and_then(|workspace| workspace.thread.as_ref()).and_then(|thread| thread.feed.as_ref()).is_some_and(|feed| feed.rows.iter().any(|row| matches!(row, junto_client::state::feed::FeedRow::Message(row) if serde_json::to_value(&row.message.document).unwrap()["blocks"][0]["kind"] == "small"))));
}
