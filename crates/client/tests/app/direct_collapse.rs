use gpui::TestAppContext;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::message::SendMessage;

use crate::admin_pages::{invite, joined};
use crate::harness::{World, member, open, selector};
use crate::messages::document;

#[gpui::test]
fn a_person_collapses_the_direct_messages_with_the_menu_pointer_and_keyboard_and_keeps_it_after_restart(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_reader_device, reader) = joined(&world, &server, &code, "Reader");
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let own = window.own_account();
    let first = world
        .run(admin.handle.api().open_conversation(own))
        .unwrap();
    let second = world.run(reader.api().open_conversation(own)).unwrap();
    let send = |api: &junto_client_core::api::ServerApi, conversation, words: &str| {
        world
            .run(api.send_message(
                PlaceId::Direct(conversation),
                &SendMessage {
                    document: document(words),
                    reply_to: None,
                    attachments: Vec::new(),
                },
            ))
            .unwrap()
    };
    send(&admin.handle.api(), first.id, "A plan");
    send(&reader.api(), second.id, "A question");
    let first_row = selector(format!("direct-row:{}", first.id));
    let second_row = selector(format!("direct-row:{}", second.id));
    window.until("both conversations are listed", |window| {
        window.shows(first_row) && window.shows(second_row)
    });
    let channel = window.selected_channel();
    window.right_click_named("directs-heading".to_owned());
    window.click("context-action:Collapse direct messages");
    window.until("the conversations are hidden", |window| {
        !window.shows(first_row) && !window.shows(second_row) && !window.shows("new-direct")
    });
    assert_eq!(window.selected_channel(), channel);
    window.until("the heading shows the hidden unread messages", |window| {
        window.shows("directs-unread")
    });
    window.close();
    let mut again = open(cx, directory.path());
    again.until_ready();
    again.cx.update(|window, _| window.activate_window());
    again.until("the conversations are known again", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.directs.len() == 2)
    });
    assert!(again.view().lists().unwrap().directs_collapsed);
    assert!(!again.shows(first_row) && !again.shows(second_row));
    again.click("directs-heading");
    again.until("a pointer press expands the group", |window| {
        window.shows(first_row) && window.shows(second_row) && window.shows("new-direct")
    });
    assert!(!again.shows("directs-unread"));
    again.press("enter");
    again.until("Enter collapses the focused heading", |window| {
        !window.shows(first_row) && !window.shows(second_row)
    });
    again.press("space");
    again.until("Space expands the focused heading", |window| {
        window.shows(first_row) && window.shows(second_row)
    });
    again.click(first_row);
    again.until("the first conversation is open and read", |window| {
        window.view().lists().is_some_and(|lists| {
            lists
                .conversation
                .as_ref()
                .is_some_and(|shown| shown.id == first.id)
                && lists
                    .directs
                    .iter()
                    .any(|row| row.id == first.id && !row.unread.unread())
        })
    });
    again.click("directs-heading");
    again.until(
        "the open conversation stays visible in the collapsed group",
        |window| window.shows(first_row) && !window.shows(second_row),
    );
    assert!(again.shows("directs-unread"));
    again.right_click_named("directs-heading".to_owned());
    again.click("context-action:Expand direct messages");
    again.until("the menu expands the group", |window| {
        window.shows(first_row) && window.shows(second_row)
    });
    again.click(second_row);
    again.until("the second conversation is read", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.directs.iter().all(|row| !row.unread.unread()))
    });
    again.click("directs-heading");
    again.until("only the open conversation stays", |window| {
        window.shows(second_row) && !window.shows(first_row)
    });
    assert!(!again.shows("directs-unread"));
    send(&admin.handle.api(), first.id, "Another plan");
    again.until(
        "a message of a hidden conversation is shown on the heading",
        |window| window.shows("directs-unread"),
    );
    again.right_click_named("directs-heading".to_owned());
    again.click("context-action:New direct message");
    again.until("the menu of the heading starts a conversation", |window| {
        window.shows("new-direct-dialog")
    });
}
