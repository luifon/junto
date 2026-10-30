use gpui::TestAppContext;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::message::SendMessage;

use crate::harness::{Window, World, member};
use crate::messages::document;

fn triggers(window: &mut Window, target: &str, menu: &str) {
    window.right_click_named(target.to_owned());
    assert!(
        window.shows_named(menu.to_owned()),
        "pointer menu for {target}"
    );
    window.press("escape");
    for key in ["menu", "shift-f10"] {
        window.click_named(target.to_owned());
        window.press(key);
        if target == "server-0" {
            let row = window
                .cx
                .debug_bounds(crate::harness::selector(target.to_owned()))
                .unwrap();
            let menu = window.cx.debug_bounds("row-context-menu").unwrap();
            assert!(
                menu.left() <= row.left() + gpui::px(16.0),
                "the row keyboard menu must use the row's anchor"
            );
        }
        assert!(
            window.shows_named(menu.to_owned()),
            "{key} menu for {target}"
        );
        window.press("escape");
    }
}

#[gpui::test]
fn server_rows_in_both_book_forms_offer_captured_actions_without_selecting_the_target(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let first = world.server("Example Studio");
    let second = world.server("Harbor");
    let first_admin = world.owner(&first);
    let second_admin = world.owner(&second);
    let (_directory, mut window) = member(cx, &world, &first.address(), &first_admin, "Rin");
    window.join_further(&world, &second, &second_admin);
    let target = first_admin.handle.id();
    let other = second_admin.handle.id();
    for collapsed in [false, true] {
        window.state.update(&mut window.cx, |state, cx| {
            state.set_server_book_collapsed(collapsed, cx);
            state.select_server(other, cx);
        });
        window.settle();
        let before = window.state.read_with(&window.cx, |state, _| {
            state.server(target).unwrap().entry().connection_mode
        });
        window.right_click_named("server-0".to_owned());
        assert!(window.shows("row-context-menu"));
        assert_eq!(window.view().selected.unwrap().id, other);
        window.click_named("context-action:Keep connected in the background".to_owned());
        window.until(
            "only the captured server changes its connection mode",
            |window| {
                window.state.read_with(&window.cx, |state, _| {
                    state.server(target).unwrap().entry().connection_mode != before
                })
            },
        );
        assert_eq!(window.view().selected.unwrap().id, other);
        triggers(&mut window, "server-0", "row-context-menu");
    }
}

#[gpui::test]
fn server_workspace_text_voice_and_direct_titles_have_pointer_and_keyboard_menus(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    triggers(&mut window, "server-title", "row-context-menu");
    triggers(&mut window, "workspace-title", "row-context-menu");
    triggers(&mut window, "channel-title", "row-context-menu");
    crate::calls::voice_channel(&world, &admin, &mut window, "Standup");
    window.click_channel("Standup");
    triggers(&mut window, "channel-title", "row-context-menu");
    let direct = world
        .run(admin.handle.api().open_conversation(window.own_account()))
        .unwrap();
    world
        .run(admin.handle.api().send_message(
            PlaceId::Direct(direct.id),
            &SendMessage {
                document: document("A conversation title"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    let server_id = admin.handle.id();
    window.state.update(&mut window.cx, |state, cx| {
        state.select_conversation(server_id, direct.id, cx)
    });
    window.until("the direct title is shown", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.place == PlaceId::Direct(direct.id))
    });
    triggers(&mut window, "channel-title", "row-context-menu");
    window
        .state
        .update(&mut window.cx, |state, cx| state.log_out(server_id, cx));
    window.until(
        "the logged-out server title replaces the workspace title",
        |window| {
            window.view().workspace().is_none()
                && window.state.read_with(&window.cx, |state, _| {
                    state
                        .server(server_id)
                        .is_some_and(|server| server.owner().is_none())
                })
        },
    );
    triggers(&mut window, "workspace-title", "row-context-menu");
}

#[gpui::test]
fn a_caption_popup_closes_when_another_layer_covers_its_source(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    window.right_click_named("workspace-title".to_owned());
    assert!(window.shows("row-context-menu"));
    window
        .in_shell(|shell, window, cx| shell.open(junto_client::shell::Open::Settings, window, cx));
    window.settle();
    assert!(window.shows("settings-dialog"));
    assert!(!window.shows("row-context-menu"));
    window.press("escape");
    assert!(!window.shows("settings-dialog"));
    assert!(!window.shows("row-context-menu"));
}
