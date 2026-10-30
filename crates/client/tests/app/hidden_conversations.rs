use crate::{
    harness::{World, member},
    messages::document,
};
use gpui::TestAppContext;
use junto_client_core::{model::PlaceId, protocol::message::SendMessage};

#[gpui::test]
fn hiding_a_conversation_keeps_the_channel_and_a_new_message_restores_its_row(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    let channel = window.selected_channel();
    let pair = world
        .run(admin.handle.api().open_conversation(window.own_account()))
        .unwrap();
    let send = |words: &str| {
        world
            .run(admin.handle.api().send_message(
                PlaceId::Direct(pair.id),
                &SendMessage {
                    document: document(words),
                    reply_to: None,
                    attachments: Vec::new(),
                },
            ))
            .unwrap()
    };
    send("Earlier plan");
    window.until("conversation row", |window| {
        window.shows_named(format!("direct-row:{}", pair.id))
    });
    window.right_click_named(format!("direct-row:{}", pair.id));
    assert!(window.shows("direct-hide"));
    window.click("direct-hide");
    window.until("hidden row", |window| {
        !window.shows_named(format!("direct-row:{}", pair.id))
    });
    assert_eq!(window.selected_channel(), channel);
    send("New plan");
    window.until("new message restores the row", |window| {
        window.shows_named(format!("direct-row:{}", pair.id))
    });
    assert!(
        window
            .view()
            .lists()
            .unwrap()
            .directs
            .iter()
            .find(|row| row.id == pair.id)
            .unwrap()
            .unread
            .messages
            > 0
    );
    window.click_named(format!("direct-row:{}", pair.id));
    window.until_message("New plan");
    window.right_click_named(format!("direct-row:{}", pair.id));
    window.click("direct-hide");
    window.until(
        "hiding the selected conversation returns to the channel",
        |window| window.view().lists().unwrap().conversation.is_none(),
    );
    assert_eq!(window.selected_channel(), channel);
    window.click("new-direct");
    let other = admin.handle.account().unwrap();
    window.until("the contact reopens the conversation", |window| {
        window.shows_named(format!("contact-{other}"))
    });
    window.click_named(format!("contact-{other}"));
    window.until("explicit contact reopen", |window| {
        window
            .view()
            .lists()
            .unwrap()
            .conversation
            .as_ref()
            .is_some_and(|row| row.id == pair.id)
    });
}

#[gpui::test]
fn a_hidden_group_can_be_reopened_without_another_message(cx: &mut TestAppContext) {
    use crate::admin_pages::{invite, joined};
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_reader_device, reader) = joined(&world, &server, &code, "Reader");
    let code = invite(&world, &admin.handle, Vec::new());
    let (_third_device, third) = joined(&world, &server, &code, "Third");
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Writer");
    let group = world
        .run(reader.api().create_group(
            "Project".to_owned(),
            vec![window.own_account(), third.account().unwrap()],
        ))
        .unwrap();
    window.until("group row", |window| {
        window.shows_named(format!("direct-row:{}", group.id))
    });
    window.right_click_named(format!("direct-row:{}", group.id));
    window.click("direct-hide");
    window.until("hidden group", |window| {
        !window.shows_named(format!("direct-row:{}", group.id))
    });
    window.click("new-direct");
    window.until("hidden group choice", |window| {
        window.shows_named(format!("hidden-direct:{}", group.id))
    });
    window.click_named(format!("hidden-direct:{}", group.id));
    assert!(
        !window.shows("new-direct-dialog"),
        "hidden group choice closed its dialog"
    );
    window.until("reopened group", |window| {
        window
            .view()
            .lists()
            .unwrap()
            .conversation
            .as_ref()
            .is_some_and(|row| row.id == group.id)
    });
    assert!(window.shows_named(format!("direct-row:{}", group.id)));
    world
        .run(reader.api().send_message(
            PlaceId::Direct(group.id),
            &SendMessage {
                document: document("Still the same project"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until_message("Still the same project");
    window.right_click_named(format!("direct-row:{}", group.id));
    window.click("direct-hide");
    window.until("hidden again", |window| {
        !window.shows_named(format!("direct-row:{}", group.id))
    });
    window.search_for("project");
    let panel = window.until_searched();
    let result = panel
        .messages()
        .into_iter()
        .find(|row| row.text == "Still the same project")
        .unwrap();
    assert!(
        matches!(&result.place, junto_client::state::notices::Whereabouts::Direct { name: Some(name) } if name == "Project")
    );
    let junto_client::state::search::ResultTarget::Message(message_id) = result.target else {
        panic!("message result");
    };
    window.click_named(format!("search-result:{message_id}"));
    window.until(
        "the hidden group search result selects and marks its message",
        |window| {
            window
                .messages()
                .iter()
                .any(|row| row.message.id == message_id && row.target)
        },
    );
    assert!(window.shows_named(format!("direct-row:{}", group.id)));
    window.right_click_named(format!("direct-row:{}", group.id));
    window.click("direct-hide");
    window.until("hidden before link", |window| {
        !window.shows_named(format!("direct-row:{}", group.id))
    });
    let origin = junto_client_core::address::ServerAddress::parse(&server.address()).unwrap();
    assert!(window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation_link(&origin, group.id, cx)
    }));
    window.until("the link reopens the same hidden group", |window| {
        window
            .view()
            .lists()
            .unwrap()
            .conversation
            .as_ref()
            .is_some_and(|row| row.id == group.id)
    });
}

#[gpui::test]
fn a_late_reopen_does_not_replace_a_newer_channel_selection(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let proxy = world.run(crate::support::FaultProxy::start(&server));
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Reader");
    let channel = window.selected_channel();
    let pair = world
        .run(admin.handle.api().open_conversation(window.own_account()))
        .unwrap();
    world
        .run(admin.handle.api().send_message(
            PlaceId::Direct(pair.id),
            &SendMessage {
                document: document("Earlier plan"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until("conversation row", |window| {
        window.shows_named(format!("direct-row:{}", pair.id))
    });
    window.right_click_named(format!("direct-row:{}", pair.id));
    window.click("direct-hide");
    window.until("hidden conversation", |window| {
        !window.shows_named(format!("direct-row:{}", pair.id))
    });
    let route = format!("/api/v1/conversations/{}/visibility", pair.id);
    proxy.hold_request("PUT", &route);
    window.click("new-direct");
    window.until("hidden choice", |window| {
        window.shows_named(format!("hidden-direct:{}", pair.id))
    });
    window.click_named(format!("hidden-direct:{}", pair.id));
    assert!(
        !window.shows("new-direct-dialog"),
        "hidden pair choice closed its dialog"
    );
    window.until("reopen request is held", |_| {
        proxy.all().iter().any(|request| {
            request.method == "PUT"
                && request.path == route
                && request.answer.is_none()
                && serde_json::from_slice::<serde_json::Value>(&request.body).unwrap()["hidden"]
                    == false
        })
    });
    window.click_named(format!("channel-row:{channel}"));
    proxy.release();
    window.until("reopened row returns", |window| {
        window.shows_named(format!("direct-row:{}", pair.id))
    });
    assert_eq!(window.selected_channel(), channel);
    assert!(window.view().lists().unwrap().conversation.is_none());
}

#[gpui::test]
fn a_hidden_pair_message_jump_reopens_and_marks_the_message(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    let pair = world
        .run(admin.handle.api().open_conversation(window.own_account()))
        .unwrap();
    let message = world
        .run(admin.handle.api().send_message(
            PlaceId::Direct(pair.id),
            &SendMessage {
                document: document("Quartz decision"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until("conversation row", |window| {
        window.shows_named(format!("direct-row:{}", pair.id))
    });
    window.right_click_named(format!("direct-row:{}", pair.id));
    window.click("direct-hide");
    window.until("hidden pair", |window| {
        !window.shows_named(format!("direct-row:{}", pair.id))
    });
    let server_id = window.view().selected.unwrap().id;
    window.state.update(&mut window.cx, |state, cx| {
        state.go_to(
            junto_client::state::notices::Jump::to(server_id, &message),
            cx,
        )
    });
    window.until(
        "notification-style jump reopens and marks the message",
        |window| {
            window
                .messages()
                .iter()
                .any(|row| row.message.id == message.id && row.target)
        },
    );
    assert!(window.shows_named(format!("direct-row:{}", pair.id)));
}
