//! The hover bar and the menu of a message take their actions from one list (ADR 0740).

use gpui::{TestAppContext, px};

use crate::admin_pages::owner_window;
use crate::harness::{World, selector};

#[gpui::test]
fn the_bar_and_the_menu_of_a_message_offer_one_set_of_actions(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, _owner) = owner_window(cx, &server);
    window.write("A plan to share");
    let message = window.until_message("A plan to share").message.id;

    // The bar: icons with a tooltip for the frequent actions, and the menu button.
    window.hover_message(message);
    for name in [
        "react",
        "reply",
        "thread",
        "forward",
        "copy-message",
        "edit",
    ] {
        assert!(window.shows_named(format!("{name}:{message}")), "{name}");
    }
    for name in ["pin", "delete", "mark-unread", "copy-link"] {
        assert!(!window.shows_named(format!("{name}:{message}")), "{name}");
    }
    let copy = window
        .cx
        .debug_bounds(selector(format!("copy-message:{message}")))
        .unwrap();
    assert!(copy.size.width < px(40.0), "Copy text is an icon");
    window.click_named(format!("forward:{message}"));
    assert!(window.shows("forward-dialog"));
    window.press("escape");

    // The menu: the reactions first, then every action in groups, with Delete last.
    window.hover_message(message);
    window.click_named(format!("message-more:{message}"));
    assert!(window.shows("message-menu"));
    assert!(window.shows("message-react"));
    let mut above = window.cx.debug_bounds("message-react").unwrap().bottom();
    for name in [
        "message-reply",
        "message-thread",
        "message-forward",
        "message-pin",
        "message-mark-unread",
        "message-copy-text",
        "message-copy-link",
        "message-edit",
        "message-delete",
    ] {
        let bounds = window
            .cx
            .debug_bounds(name)
            .unwrap_or_else(|| panic!("{name} is not in the menu"));
        assert!(bounds.top() >= above, "{name} is out of its order");
        above = bounds.bottom();
    }
    window.click("message-delete");
    assert!(window.shows("confirm-delete-message"));
}

/// An edit happens in the row of the message, and the composer keeps its new message
/// (ADR 0740).
#[gpui::test]
fn a_message_is_edited_in_its_row_while_the_composer_keeps_its_text(cx: &mut TestAppContext) {
    use junto_client::shell::{Pane, Pen};

    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, _owner) = owner_window(cx, &server);
    window.write("A first plan");
    let message = window.until_message("A first plan").message.id;
    window.in_shell(|shell, window, cx| shell.type_message(Pane::Main, "a new note", window, cx));

    window.hover_message(message);
    window.click_named(format!("edit:{message}"));
    assert!(window.shows_named(format!("message-editor:{message}")));
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pen::Edit(Pane::Main), cx)),
        "A first plan"
    );
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "a new note"
    );
    // Escape cancels the edit and leaves the message as it was.
    window.press("escape");
    assert!(!window.shows_named(format!("message-editor:{message}")));

    window.hover_message(message);
    window.click_named(format!("edit:{message}"));
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pen::Edit(Pane::Main), "A changed plan", window, cx);
        shell.focus_composer(Pen::Edit(Pane::Main), window, cx);
    });
    window.press("enter");
    let edited = window.until_message("A changed plan");
    assert_eq!(edited.message.id, message);
    window.until("the editor of the row closes", |window| {
        !window.shows_named(format!("message-editor:{message}"))
    });
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "a new note"
    );
}
