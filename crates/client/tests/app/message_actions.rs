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
    for name in ["react", "reply", "thread", "forward", "copy-message", "edit"] {
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
