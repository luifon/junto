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
