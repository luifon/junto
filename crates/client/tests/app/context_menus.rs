use gpui::{ClipboardItem, TestAppContext, px, size};
use junto_client::shell::Pane;

use crate::harness::{Window, World, member};
use crate::threads::with_root;

fn edit_menu(window: &mut Window, pane: Pane, name: &'static str) {
    let input = match pane {
        Pane::Main => "composer-input",
        Pane::Thread => "thread-composer-input",
    };
    window.type_into(input, name);
    window.press("secondary-a");
    window.right_click_named(input.to_owned());
    assert!(window.shows("composer-menu"));
    window.click("composer-copy");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        name
    );
    window.press("shift-f10");
    window.click("composer-cut");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(pane, cx)),
        ""
    );
    window
        .cx
        .write_to_clipboard(ClipboardItem::new_string("β pasted".to_owned()));
    window.right_click_named(input.to_owned());
    window.click("composer-paste");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(pane, cx)),
        "β pasted"
    );
    window.press("shift-f10");
    window.click("composer-select-all");
    window.press("secondary-c");
    assert_eq!(
        window.cx.read_from_clipboard().unwrap().text().unwrap(),
        "β pasted"
    );
    window.press("shift-f10 escape");
    assert!(!window.shows("composer-menu"));
}

#[gpui::test]
fn the_main_composer_has_one_edit_menu_for_mouse_and_keyboard(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    edit_menu(&mut window, Pane::Main, "café draft");
}

#[gpui::test]
fn the_thread_composer_uses_the_same_edit_menu(cx: &mut TestAppContext) {
    let world = World::new();
    let (_server, _admin, _directory, mut window, _channel, root) = with_root(cx, &world);
    window.open_thread_of(root);
    edit_menu(&mut window, Pane::Thread, "thread draft");
}

#[gpui::test]
fn a_message_menu_stays_inside_a_small_window(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    window.simulate(|cx| cx.simulate_resize(size(px(720.0), px(480.0))));
    for number in 0..12 {
        window.write(&format!("message {number}"));
    }
    let message = window.until_message("message 11").message.id;
    window.right_click_named(format!("message:{message}"));
    let bounds = window.cx.debug_bounds("message-menu").unwrap();
    assert!(
        bounds.left() >= px(8.0) && bounds.top() >= px(8.0),
        "{bounds:?}"
    );
    assert!(
        bounds.right() <= px(712.0) && bounds.bottom() <= px(472.0),
        "{bounds:?}"
    );
    window.press("escape");
    assert!(!window.shows("message-menu"));
}

#[gpui::test]
fn a_composer_menu_ends_when_navigation_changes_its_place(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let other = admin.text_channel(&world, &mut window, "other");
    let workspace = window.view().workspace().unwrap().id;
    window.type_into("composer-input", "keep this draft");
    window.press("secondary-a shift-f10");
    assert!(window.shows("composer-menu"));
    window.state.update(&mut window.cx, |state, cx| {
        state.select_channel(workspace, other, cx)
    });
    window.until("navigation closes the menu", |window| {
        !window.shows("composer-menu")
    });
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        ""
    );
    window.click_channel("general");
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "keep this draft"
    );
}
