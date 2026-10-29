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

#[gpui::test]
fn a_notice_menu_reads_one_notice_and_opens_another_notices_message(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let target = admin.text_channel(&world, &mut window, "planning");
    let own = window.own_account();
    let first = admin.mention(&world, target, "First notice ", own);
    let second = admin.mention(&world, target, "Second notice ", own);
    window.until("the two mentions make unread notices", |window| {
        window.unread_notices() == 2
    });
    window.open_notices();
    window.until("the notices have their message targets", |window| {
        window.notices().is_some_and(|panel| {
            panel.rows.len() == 2 && panel.rows.iter().all(|row| row.jump.is_some())
        })
    });
    let first_notice = window
        .notices()
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.jump.is_some_and(|jump| jump.message_id == first.id))
        .unwrap()
        .id;
    let second_notice = window
        .notices()
        .unwrap()
        .rows
        .into_iter()
        .find(|row| row.jump.is_some_and(|jump| jump.message_id == second.id))
        .unwrap()
        .id;
    window.right_click_named(format!("notice:{first_notice}"));
    window.click("notice-mark-read");
    window.until("only the chosen notice is read", |window| {
        window.notices().is_some_and(|panel| {
            panel.unread == 1
                && panel
                    .rows
                    .iter()
                    .any(|row| row.id == first_notice && row.read)
                && panel
                    .rows
                    .iter()
                    .any(|row| row.id == second_notice && !row.read)
        })
    });
    assert_eq!(window.selected_channel(), selected);
    assert!(!window.shows("notice-menu"));
    window.right_click_named(format!("notice:{second_notice}"));
    window.click("notice-go-to");
    window.until("the notice opens its message and is read", |window| {
        window.selected_channel() == target
            && window.unread_notices() == 0
            && window
                .texts()
                .iter()
                .any(|text| text.starts_with("Second notice"))
    });
    assert!(!window.shows("notice-menu"));
}

#[gpui::test]
fn copy_target_links_from_channel_direct_and_notice_menus(cx: &mut TestAppContext) {
    use junto_client_core::model::PlaceId;
    use junto_client_core::protocol::message::SendMessage;

    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let selected = window.selected_channel();
    let origin = window.handle().entry().address.as_str().to_owned();
    let own = window.own_account();
    let conversation = world
        .run(admin.handle.api().open_conversation(own))
        .unwrap();
    let message = world
        .run(admin.handle.api().send_message(
            PlaceId::Direct(conversation.id),
            &SendMessage {
                document: document("A linked update"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until("the direct conversation is listed", |window| {
        window
            .view()
            .lists()
            .is_some_and(|lists| lists.directs.iter().any(|row| row.id == conversation.id))
    });
    window.right_click_named(format!("channel-row:{selected}"));
    window.click("channel-copy-link");
    assert_target_link(&mut window, "channel", selected, &origin);
    window.right_click_named(format!("direct-row:{}", conversation.id));
    window.click("direct-copy-link");
    assert_target_link(&mut window, "conversation", conversation.id, &origin);
    assert_eq!(window.selected_channel(), selected);
    window.open_notices();
    let notice = window.until_value("the direct notice has its message target", |window| {
        window
            .notices()?
            .rows
            .into_iter()
            .find(|row| row.jump.is_some_and(|jump| jump.message_id == message.id))
            .map(|row| row.id)
    });
    window.right_click_named(format!("notice:{notice}"));
    window.click("notice-copy-link");
    assert_target_link(&mut window, "message", message.id, &origin);
    assert_eq!(window.selected_channel(), selected);
}

fn assert_target_link(
    window: &mut crate::harness::Window,
    kind: &str,
    id: impl std::fmt::Display,
    origin: &str,
) {
    let copied = window
        .cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap();
    let parsed = url::Url::parse(&copied).unwrap();
    assert_eq!(parsed.scheme(), "junto");
    assert_eq!(parsed.host_str(), Some(kind));
    assert_eq!(
        parsed.query_pairs().into_owned().collect::<Vec<_>>(),
        [
            ("server".to_owned(), origin.to_owned()),
            ("id".to_owned(), id.to_string()),
        ]
    );
}
