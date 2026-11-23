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
    assert!(window.has_focused_menu());
    assert_eq!(window.selected_channel(), shown);
    window.press("escape");
    assert!(!window.shows("channel-menu"));

    window.hover_named("server-0".to_owned());
    let server_id = window.view().selected.as_ref().unwrap().id;
    window.press_alone(selector(format!("menu-button:server:{server_id}")));
    assert!(window.shows("row-context-menu"));
    assert!(window.has_focused_menu());
    window.press("escape");
    assert!(!window.shows("row-context-menu"));

    // A row of an administration page has the menu button too.
    window.open_administration();
    window.click("admin-nav-workspaces");
    let row = format!("admin-row-workspace-{workspace}");
    window.until("the workspaces are listed", |window| {
        window.shows_named(row.clone())
    });
    window.hover_named(row);
    window.press_alone(selector(format!("menu-button:workspace-{workspace}")));
    assert!(window.shows("row-context-menu"));
    assert!(window.has_focused_menu());
    window.press("escape");
}

#[gpui::test]
fn a_control_on_a_row_does_only_its_own_action(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, _owner) = owner_window(cx, &server);
    // The control that collapses the workspaces lies on the head of the server, which can be
    // dragged: before the rule a press with a small move started the drag of the server.
    let collapsed =
        |window: &mut crate::harness::Window| window.view().servers[0].workspaces_collapsed;
    let before = collapsed(&mut window);
    window.hover_named("server-0".to_owned());
    window.press_alone("workspaces-toggle-0");
    window.until("the workspaces collapse", |window| {
        collapsed(window) != before
    });
    window.press_alone("workspaces-toggle-0");
    window.until("the workspaces expand again", |window| {
        collapsed(window) == before
    });

    window.write("A message with actions");
    let message = window.until_message("A message with actions").message.id;
    window.hover_message(message);
    window.press_alone(selector(format!("reply:{message}")));
    assert!(window.shows("compose-state"), "the reply did not start");
    window.press("escape");
    window.hover_message(message);
    window.press_alone(selector(format!("message-more:{message}")));
    assert!(window.shows("message-menu"));
    assert!(window.has_focused_menu());
    window.press("escape");
}

/// The chips of a message (a reaction, the thread) are controls on the message: a press with
/// a small move does their action and selects no text.
#[gpui::test]
fn the_chips_of_a_message_do_only_their_own_action(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, admin, _directory, mut window, _channel, root) =
        crate::threads::with_root(cx, &world);
    world
        .run(admin.handle.api().add_reaction(root, "👍"))
        .unwrap();
    let own = |window: &mut crate::harness::Window| {
        window
            .messages()
            .into_iter()
            .find(|row| row.message.id == root)
            .and_then(|row| row.message.reactions.first().map(|reaction| reaction.own))
    };
    window.until("the reaction is drawn", |window| own(window) == Some(false));
    window.press_alone(selector(format!("reaction:{root}:0")));
    window.until("the own reaction is added", |window| {
        own(window) == Some(true)
    });

    admin.reply(&world, root, crate::messages::document("Tracks."));
    window.until("the thread chip is drawn", |window| {
        window.shows_named(format!("thread-chip:{root}"))
    });
    window.press_alone(selector(format!("thread-chip:{root}")));
    window.until("the thread opens", |window| window.shows("thread-composer"));
}

/// The actions of a message, with its menu button, are drawn while the message holds the
/// keyboard focus, also after the pointer left it.
#[gpui::test]
fn the_actions_of_a_message_show_while_it_holds_the_focus(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, _owner) = owner_window(cx, &server);
    window.write("A focused message");
    let message = window.until_message("A focused message").message.id;
    window.click_row_padding(selector(format!("message:{message}")));
    let away = window.cx.debug_bounds("channel-title").unwrap().center();
    window.move_pointer(away);
    assert!(window.shows_named(format!("message-more:{message}")));
}
