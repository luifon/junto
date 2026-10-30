use gpui::{ClipboardItem, MouseButton, TestAppContext, point, px};

use crate::harness::{World, member, selector};
use crate::messages::document;

#[gpui::test]
fn a_read_only_value_in_settings_has_a_copy_control(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.in_shell(|shell, window, cx| shell.open_version_settings(window, cx));
    window.settle();
    let bounds = window.cx.debug_bounds("version-build").unwrap();
    window.move_pointer(point(bounds.left() + px(8.0), bounds.center().y));
    window
        .cx
        .write_to_clipboard(ClipboardItem::new_string("old".to_owned()));
    window.click("copy-text-value");
    let copied = window.cx.read_from_clipboard().unwrap().text().unwrap();
    assert_eq!(copied, env!("CARGO_PKG_VERSION"));
    assert!(window.shows("copied-confirmation"));
}

#[gpui::test]
fn selected_rich_text_keeps_unicode_and_omits_display_padding(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.write("café `run()` 世界");
    let row = window.until_message("café run() 世界");
    let bounds = window
        .cx
        .debug_bounds(selector(format!("message:{}", row.message.id)))
        .unwrap();
    window.move_pointer(point(bounds.left() + px(110.0), bounds.bottom() - px(12.0)));
    window.click("copy-text-value");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "run()"
    );
    window
        .cx
        .write_to_clipboard(ClipboardItem::new_string("old".to_owned()));
    window.press_at_count(
        point(bounds.left() + px(65.0), bounds.bottom() - px(12.0)),
        MouseButton::Left,
        3,
    );
    window.press("secondary-c");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "café run() 世界"
    );
    window.type_into("composer-input", "Draft retained");
    window.press("secondary-a secondary-c");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "Draft retained"
    );
}

#[gpui::test]
fn copying_a_named_link_uses_its_address(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.write("[the guide](https://example.test/guide)");
    let row = window.until_message("the guide");
    let bounds = window
        .cx
        .debug_bounds(selector(format!("message:{}", row.message.id)))
        .unwrap();
    window.move_pointer(point(bounds.left() + px(65.0), bounds.bottom() - px(12.0)));
    window
        .cx
        .write_to_clipboard(ClipboardItem::new_string("old".to_owned()));
    window.click("copy-text-value");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "https://example.test/guide"
    );
}

#[gpui::test]
fn a_message_menu_copies_the_complete_text_and_code_keeps_its_whitespace(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.write("First paragraph\n\n```rust\nfn main() {\n    run();\n}\n```");
    window.until_message("First paragraph fn main() { run(); }");
    let message = window.messages().pop().unwrap().message.id;
    window
        .cx
        .write_to_clipboard(ClipboardItem::new_string("old".to_owned()));
    window.right_click_named(format!("message:{message}"));
    window.click("message-copy-text");
    let copied = window.cx.read_from_clipboard().unwrap().text().unwrap();
    assert_eq!(copied, "First paragraph\nfn main() {\n    run();\n}");
    window.click_named(format!("copy-code:{message}:1"));
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "fn main() {\n    run();\n}"
    );
    assert!(window.shows("copied-confirmation"));
}

#[gpui::test]
fn dragging_across_message_rows_copies_both_rows(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the history is loaded", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let channel = window.selected_channel();
    let first = admin.send(&world, channel, document("alpha beta"));
    let last = admin.send(&world, channel, document("gamma delta"));
    window.until_message("gamma delta");
    window.settle();
    let start = window
        .cx
        .debug_bounds(selector(format!("message:{}", first.id)))
        .unwrap();
    let end = window
        .cx
        .debug_bounds(selector(format!("message:{}", last.id)))
        .unwrap();
    let from = point(start.left() + px(62.0), start.bottom() - px(12.0));
    let to = point(end.left() + px(150.0), end.bottom() - px(12.0));
    window
        .cx
        .write_to_clipboard(ClipboardItem::new_string("old".to_owned()));
    window.dispatch(gpui::MouseDownEvent {
        position: from,
        button: MouseButton::Left,
        ..Default::default()
    });
    window.dispatch(gpui::MouseMoveEvent {
        position: to,
        pressed_button: Some(MouseButton::Left),
        ..Default::default()
    });
    window.dispatch(gpui::MouseUpEvent {
        position: to,
        button: MouseButton::Left,
        ..Default::default()
    });
    window.press("secondary-c");
    let copied = window.cx.read_from_clipboard().unwrap().text().unwrap();
    assert!(copied.contains("beta"), "{copied:?}");
    assert!(copied.contains("gamma"), "{copied:?}");
}

#[gpui::test]
fn dragging_at_the_edge_selects_history_across_several_screens(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the history is loaded", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let channel = window.selected_channel();
    let lines: Vec<_> = (0..90)
        .map(|index| format!("row {index:03} café 世界"))
        .collect();
    for line in &lines {
        admin.send(&world, channel, document(line));
    }
    window.until_message(&lines[89]);
    window.settle();
    let from = window
        .in_shell(|shell, _, _| shell.readonly_text_point(&lines[89], "row 089 café".len()))
        .unwrap();
    let viewport = window.cx.debug_bounds("message-list").unwrap();
    let edge = point(from.x, viewport.top() + px(2.0));
    window.dispatch(gpui::MouseDownEvent {
        position: from,
        button: MouseButton::Left,
        ..Default::default()
    });
    window.dispatch(gpui::MouseMoveEvent {
        position: edge,
        pressed_button: Some(MouseButton::Left),
        ..Default::default()
    });
    window.until_read(
        "dragging at the edge scrolls to the first message",
        |window| {
            window.shell.read_with(&window.cx, |shell, _| {
                (
                    shell
                        .painted_readonly_text()
                        .into_iter()
                        .filter(|text| text.starts_with("row "))
                        .collect::<Vec<_>>(),
                    shell
                        .readonly_text_point(&lines[0], "row 000 ".len())
                        .is_some_and(|point| point.y > viewport.top() + px(40.0)),
                )
            })
        },
        |(_, reached)| *reached,
    );
    let to = window
        .shell
        .read_with(&window.cx, |shell, _| {
            shell.readonly_text_point(&lines[0], "row 000 ".len())
        })
        .unwrap();
    window.dispatch(gpui::MouseMoveEvent {
        position: to,
        pressed_button: Some(MouseButton::Left),
        ..Default::default()
    });
    window.dispatch(gpui::MouseUpEvent {
        position: to,
        button: MouseButton::Left,
        ..Default::default()
    });
    window.press("secondary-c");
    let copied = window.cx.read_from_clipboard().unwrap().text().unwrap();
    let mut expected = vec!["café 世界".to_owned()];
    expected.extend_from_slice(&lines[1..89]);
    expected.push("row 089 café".to_owned());
    assert_eq!(copied, expected.join("\n"));
    // The anchor remains valid when neither endpoint is painted.
    let middle = window.feed().unwrap().rows.len() / 2;
    window
        .in_shell(|shell, _, cx| shell.show_feed_row(junto_client::shell::Pane::Main, middle, cx));
    window.settle();
    assert!(
        window
            .in_shell(|shell, _, _| shell.readonly_text_point(&lines[0], 0))
            .is_none()
    );
    assert!(
        window
            .in_shell(|shell, _, _| shell.readonly_text_point(&lines[89], 0))
            .is_none()
    );
    window.press("secondary-c");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        expected.join("\n")
    );
}

#[gpui::test]
fn history_selection_masks_concealed_text_and_a_source_edit_invalidates_it(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.until("the history is loaded", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    let channel = window.selected_channel();
    admin.send(&world, channel, document("first café"));
    let middle = admin.send(
        &world,
        channel,
        junto_client::compose::parse("before ||secret|| after", &[]),
    );
    admin.send(&world, channel, document("last 世界"));
    window.until_message("last 世界");
    window.settle();
    let from = window
        .in_shell(|shell, _, _| shell.readonly_text_point("first café", 0))
        .unwrap();
    let to = window
        .in_shell(|shell, _, _| shell.readonly_text_point("last 世界", "last 世界".len()))
        .unwrap();
    window.dispatch(gpui::MouseDownEvent {
        position: from,
        button: MouseButton::Left,
        ..Default::default()
    });
    window.dispatch(gpui::MouseMoveEvent {
        position: to,
        pressed_button: Some(MouseButton::Left),
        ..Default::default()
    });
    window.dispatch(gpui::MouseUpEvent {
        position: to,
        button: MouseButton::Left,
        ..Default::default()
    });
    window.press("secondary-c");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "first café\nbefore Spoiler after\nlast 世界"
    );
    world
        .run(
            admin
                .handle
                .api()
                .edit_message(middle.id, document("changed source")),
        )
        .unwrap();
    window.until_message("changed source");
    window.settle();
    window.cx.write_to_clipboard(ClipboardItem::new_string(
        "selection invalidated".to_owned(),
    ));
    window.press("secondary-c");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "selection invalidated"
    );
}

#[gpui::test]
fn hovering_an_adjacent_paragraph_copies_that_paragraph(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.write("Earlier paragraph\n\nLater paragraph");
    window.until_message("Earlier paragraph Later paragraph");
    let earlier = window
        .in_shell(|shell, _, _| shell.readonly_text_point("Earlier paragraph", 2))
        .unwrap();
    window.move_pointer(earlier);
    assert!(window.shows("copy-text-value"));
    let later = window
        .in_shell(|shell, _, _| shell.readonly_text_point("Later paragraph", 2))
        .unwrap();
    assert!(
        later.y > earlier.y && later.y - earlier.y < px(40.0),
        "the paragraphs must be adjacent"
    );
    window.move_pointer(later);
    window.click("copy-text-value");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "Later paragraph"
    );
}

#[gpui::test]
fn mention_profiles_and_presence_keep_message_selection_plain(cx: &mut TestAppContext) {
    use junto_client_core::protocol::message::{Block, Document, Inline};
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let channel = window.selected_channel();
    let account = window.own_account();
    let message = admin.send(
        &world,
        channel,
        Document {
            blocks: vec![Block::Paragraph {
                content: vec![
                    Inline::Text {
                        text: "café ".to_owned(),
                        marks: Vec::new(),
                    },
                    Inline::Account {
                        account_id: account,
                    },
                    Inline::Text {
                        text: " ".to_owned(),
                        marks: Vec::new(),
                    },
                    Inline::Code {
                        text: "run()".to_owned(),
                    },
                ],
            }],
        },
    );
    window.until("the mention and profile target are drawn", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == message.id)
            && window.shows("profile-text")
    });
    let bounds = window.cx.debug_bounds("profile-text").unwrap();
    window.press_at_count(
        point(bounds.left() + px(3.0), bounds.center().y),
        MouseButton::Left,
        3,
    );
    window.press("secondary-c");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "café @Member run()"
    );
}

#[gpui::test]
fn moving_to_the_next_message_moves_its_copy_control_away_from_its_actions(
    cx: &mut TestAppContext,
) {
    use junto_client_core::protocol::WorkspacePermission;
    let world = World::new();
    let (_server, admin, _directory, mut window, channel, _) =
        crate::threads::with_root(cx, &world);
    admin.allow_everyone(&world, &[WorkspacePermission::ManageMessages]);
    let first = admin.send(&world, channel, document("First point"));
    let second = admin.send(&world, channel, document("Second point"));
    window.until_message("Second point");
    window.until("the next message offers pin", |window| {
        window
            .messages()
            .iter()
            .any(|row| row.message.id == second.id && row.can_pin)
    });
    window.hover_named(format!("message:{}", first.id));
    assert!(window.shows("copy-text-value"));
    window.hover_named(format!("message:{}", second.id));
    let copy = window.cx.debug_bounds("copy-text-value").unwrap();
    let pin = window
        .cx
        .debug_bounds(selector(format!("pin:{}", second.id)))
        .unwrap();
    assert!(
        !copy.contains(&pin.center()),
        "copy {copy:?} covers pin {pin:?}"
    );
    window.click("copy-text-value");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "Second point"
    );
}
