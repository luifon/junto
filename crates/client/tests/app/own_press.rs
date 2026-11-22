//! A control placed on a row or on another clickable element does only its own action
//! (ADR 0731): a press on it does not select the row, open its place, start a drag of the row
//! or select text, and the menu button of a row opens the same menu as a right click.

use gpui::TestAppContext;

use crate::admin_pages::{default_workspace, owner_window, text_channel};
use crate::harness::{World, selector};

#[gpui::test]
fn the_menu_button_of_a_row_opens_its_menu_and_changes_nothing_else(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let workspace = default_workspace(&world, &owner);
    let target = text_channel(&world, &owner, workspace, "planning");
    window.until("the target channel is listed", |window| {
        window.shows_channel("planning")
    });
    let shown = window.selected_channel();
    assert_ne!(shown, target);

    let button = selector(format!("menu-button:channel:{target}"));
    window.hover_named(format!("channel-row:{target}"));
    window.press_alone(button);
    assert!(window.shows("channel-menu"));
    assert_eq!(window.selected_channel(), shown);
    window.press("escape");
    assert!(!window.shows("channel-menu"));

    window.hover_named("server-0".to_owned());
    let server_id = window.view().selected.as_ref().unwrap().id;
    // The button of a server opens the menu of the server (ADR 0780).
    window.press_alone(selector(format!("menu-button:server:{server_id}")));
    assert!(window.shows("server-menu"));
    window.press("escape");
    assert!(!window.shows("server-menu"));
}

#[gpui::test]
fn a_control_on_a_row_does_only_its_own_action(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, _owner) = owner_window(cx, &server);
    // The control that collapses the workspaces lies on the head of the server, which can be
    // dragged: before the rule a press with a small move started the drag of the server.
    window.hover_named("server-0".to_owned());
    window.press_alone("workspaces-toggle-0");
    window.press_alone("workspaces-toggle-0");

    window.write("A message with actions");
    let message = window.until_message("A message with actions").message.id;
    window.hover_message(message);
    window.press_alone(selector(format!("reply:{message}")));
    window.hover_message(message);
    window.press_alone(selector(format!("message-more:{message}")));
    assert!(window.shows("message-menu"));
    window.press("escape");
}
