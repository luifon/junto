use gpui::TestAppContext;
use junto_client_core::protocol::structure::{ChannelKind, CreateCategory, CreateChannel};

use crate::harness::{World, member, open, selector};

#[gpui::test]
fn a_person_collapses_a_category_with_the_menu_pointer_and_keyboard_and_keeps_it_after_restart(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let workspace = window.view().workspace().unwrap().id;
    let category = world
        .run(admin.handle.api().create_category(
            workspace,
            &CreateCategory {
                name: "Planning".to_owned(),
                position: None,
            },
        ))
        .unwrap();
    let mut channels = Vec::new();
    for name in ["roadmap", "estimates"] {
        channels.push(
            world
                .run(admin.handle.api().create_channel(
                    workspace,
                    &CreateChannel {
                        name: name.to_owned(),
                        kind: ChannelKind::Text,
                        category_id: Some(category.id),
                        position: None,
                    },
                ))
                .unwrap(),
        );
    }
    let head = selector(format!("category-header:{}", category.id));
    let first = selector(format!("channel-row:{}", channels[0].id));
    let second = selector(format!("channel-row:{}", channels[1].id));
    window.until("both category channels are drawn", |window| {
        window.shows(first) && window.shows(second)
    });
    let selected = window.selected_channel();
    window.right_click_named(head.to_owned());
    window.click("context-action:Collapse category");
    window.until("both children are hidden", |window| {
        !window.shows(first) && !window.shows(second)
    });
    assert_eq!(window.selected_channel(), selected);
    let root = admin.send(
        &world,
        channels[1].id,
        crate::notices::mention("A new estimate for ", window.own_account()),
    );
    let unread = selector(format!("category-unread:{}", category.id));
    window.until("the collapsed header shows unread chat", |window| {
        window.shows(unread)
    });
    window.close();
    let mut again = open(cx, directory.path());
    again.until_ready();
    again.cx.update(|window, _| window.activate_window());
    assert!(!again.shows(first));
    assert!(!again.shows(second));
    again.click(head);
    again.until("a pointer press expands the category", |window| {
        window.shows(first) && window.shows(second)
    });
    again.press("enter");
    again.until("Enter collapses the focused category", |window| {
        !window.shows(first) && !window.shows(second)
    });
    again.press("space");
    again.until("Space expands the focused category", |window| {
        window.shows(first) && window.shows(second)
    });
    again.click(second);
    again.until("the second channel is read", |window| {
        window.selected_channel() == channels[1].id
            && window
                .channels()
                .iter()
                .any(|row| row.id == channels[1].id && row.unread.messages == 0)
    });
    again.click(first);
    again.until("the first category channel is selected", |window| {
        window.selected_channel() == channels[0].id
    });
    again.click(head);
    again.until(
        "the selected channel stays visible in the collapsed category",
        |window| window.shows(first) && !window.shows(second),
    );
    assert_eq!(again.selected_channel(), channels[0].id);
    assert!(!again.shows(unread));
    admin.reply(
        &world,
        root.id,
        crate::notices::mention("An estimate for ", again.own_account()),
    );
    again.until(
        "a hidden unread thread is reported by the category",
        |window| {
            window.shows(unread)
                && window.channels().iter().any(|row| {
                    row.id == channels[1].id
                        && row.unread.messages == 0
                        && row.threads.iter().any(|thread| thread.unread > 0)
                })
        },
    );
    again.right_click_named(head.to_owned());
    again.click("context-action:Mark as read");
    again.until(
        "mark category as read includes its hidden thread",
        |window| !window.shows(unread),
    );
}
