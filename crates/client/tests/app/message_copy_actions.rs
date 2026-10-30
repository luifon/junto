use crate::harness::{Window, World, member};
use crate::messages::document;
use gpui::TestAppContext;
use junto_client_core::links::{self, AppLink};
use junto_client_core::protocol::ids::MessageId;

fn read_actions(window: &mut Window, id: MessageId, text: &str) {
    for action in [
        "message-copy-text",
        "message-copy-link",
        "message-forward",
        "message-mark-unread",
    ] {
        window.right_click_named(format!("shown-message:{id}"));
        for selector in [
            "message-copy-text",
            "message-copy-link",
            "message-forward",
            "message-mark-unread",
        ] {
            assert!(
                window.shows(selector),
                "{selector} is missing from the message copy"
            );
        }
        for selector in [
            "message-edit",
            "message-delete",
            "message-react",
            "message-pin",
        ] {
            assert!(
                !window.shows(selector),
                "{selector} belongs to the original presentation"
            );
        }
        window.click(action);
        match action {
            "message-copy-text" => assert_eq!(
                window.cx.read_from_clipboard().unwrap().text().unwrap(),
                text
            ),
            "message-copy-link" => assert!(
                matches!(links::parse(&window.cx.read_from_clipboard().unwrap().text().unwrap()), Some(AppLink::Message { id: copied, .. }) if copied == id)
            ),
            "message-forward" => {
                assert!(window.shows("forward-dialog"));
                window.press("escape");
            }
            _ => assert!(!window.shows("message-menu")),
        }
    }
}

#[gpui::test]
fn a_thread_root_offers_the_read_actions_of_its_original(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let root = admin.send(&world, window.selected_channel(), document("thread source"));
    window.until_message("thread source");
    window.right_click_named(format!("message:{}", root.id));
    window.click("message-thread");
    window.until("the root is drawn", |window| {
        window.shows_named(format!("shown-message:{}", root.id))
    });
    read_actions(&mut window, root.id, "thread source");
}

#[gpui::test]
fn a_pinned_copy_offers_the_read_actions_of_its_original(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let message = admin.send(&world, window.selected_channel(), document("pinned source"));
    window.until_message("pinned source");
    world
        .run(admin.handle.api().pin_message(message.id))
        .unwrap();
    window.click("pins-button");
    window.until("the pin is drawn", |window| {
        window.shows_named(format!("shown-message:{}", message.id))
    });
    read_actions(&mut window, message.id, "pinned source");
}
