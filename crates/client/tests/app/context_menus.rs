use gpui::TestAppContext;
use junto_client::state::admin::view::Scope;

use crate::admin_pages::{default_workspace, owner_window, text_channel};
use crate::harness::{World, member};
use crate::messages::document;

#[gpui::test]
fn a_channel_menu_marks_its_channel_read_without_selecting_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let target = admin.text_channel(&world, &mut window, "planning");
    admin.send(&world, target, document("A new plan"));
    window.until("the other channel has an unread message", |window| {
        window
            .channels()
            .iter()
            .any(|row| row.id == target && row.unread.messages == 1)
    });
    window.right_click_named(format!("channel-row:{target}"));
    window.click("channel-mark-read");
    window.until("the target channel is read", |window| {
        window
            .channels()
            .iter()
            .any(|row| row.id == target && row.unread.messages == 0)
    });
    assert_eq!(window.selected_channel(), selected);
    assert!(!window.shows("channel-menu"));
    assert_eq!(
        world
            .run(
                window
                    .handle()
                    .api()
                    .message_state(junto_client_core::model::PlaceId::Channel(target))
            )
            .unwrap()
            .unread,
        0
    );
}

#[gpui::test]
fn a_channel_menu_edits_its_target_without_selecting_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let selected = window.selected_channel();
    let workspace = default_workspace(&world, &owner);
    let target = text_channel(&world, &owner, workspace, "planning");
    window.until("the target channel is listed", |window| {
        window.shows_channel("planning")
    });
    window.right_click_named(format!("channel-row:{target}"));
    window.click("channel-edit");
    window.until("the settings of the target are open", |window| {
        matches!(window.admin().map(|view| view.scope), Some(Scope::Channel(settings)) if settings.channel_id == target)
    });
    assert_eq!(window.selected_channel(), selected);
    window.click("channel-rename");
    window.in_shell(|shell, window, cx| shell.type_admin_name("roadmap", window, cx));
    window.click("admin-form-send");
    window.until("the target has its new name", |window| {
        window.channel_names().contains(&"roadmap".to_owned()) && !window.shows("channel-name")
    });
    assert_eq!(window.selected_channel(), selected);
}

#[gpui::test]
fn a_member_without_manage_channel_has_no_edit_in_the_channel_menu(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let target = window.selected_channel();
    window.right_click_named(format!("channel-row:{target}"));
    assert!(window.shows("channel-menu"));
    assert!(window.shows("channel-mark-read"));
    assert!(!window.shows("channel-edit"));
}

#[gpui::test]
fn a_direct_menu_marks_its_conversation_read_without_opening_it(cx: &mut TestAppContext) {
    use junto_client_core::model::PlaceId;
    use junto_client_core::protocol::message::SendMessage;

    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let own = window.own_account();
    let conversation = world
        .run(admin.handle.api().open_conversation(own))
        .unwrap();
    world
        .run(admin.handle.api().send_message(
            PlaceId::Direct(conversation.id),
            &SendMessage {
                document: document("A direct update"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until("the direct message is unread", |window| {
        window.view().lists().is_some_and(|lists| {
            lists
                .directs
                .iter()
                .any(|row| row.id == conversation.id && row.unread.messages == 1)
        })
    });
    window.right_click_named(format!("direct-row:{}", conversation.id));
    window.click("direct-mark-read");
    window.until("the conversation is read", |window| {
        window.view().lists().is_some_and(|lists| {
            lists
                .directs
                .iter()
                .any(|row| row.id == conversation.id && row.unread.messages == 0)
        })
    });
    assert_eq!(window.selected_channel(), selected);
    assert!(!window.shows("direct-menu"));
    assert_eq!(
        world
            .run(
                window
                    .handle()
                    .api()
                    .message_state(PlaceId::Direct(conversation.id))
            )
            .unwrap()
            .unread,
        0
    );
}
