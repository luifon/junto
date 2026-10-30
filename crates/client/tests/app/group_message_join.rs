use crate::{
    admin_pages::{invite, joined},
    harness::{World, member},
    messages::document,
    support::logged_in,
};
use gpui::TestAppContext;
use junto_client::shell::Pane;
use junto_client_core::{model::PlaceId, protocol::message::SendMessage};

#[gpui::test]
fn group_forwarding_preserves_its_draft_and_manual_unread_boundary(cx: &mut TestAppContext) {
    use junto_client::state::profile::ProfileRead;
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_writer_device, writer) = joined(&world, &server, &code, "Writer");
    let code = invite(&world, &admin.handle, Vec::new());
    let (_third_device, third) = joined(&world, &server, &code, "Third");
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    let server_id = window.handle().id();
    let source_channel = window.selected_channel();
    let source = admin.send(&world, source_channel, document("the shared plan"));
    window.until_message("the shared plan");
    let group = world
        .run(window.handle().api().create_group(
            "Project".to_owned(),
            vec![logged_in(&writer), logged_in(&third)],
        ))
        .unwrap();
    let second = world
        .run(window.handle().api().create_group(
            "Another".to_owned(),
            vec![logged_in(&writer), logged_in(&third)],
        ))
        .unwrap();
    window.until("both readable groups are listed", |window| {
        window.shows_named(format!("direct-row:{}", group.id))
            && window.shows_named(format!("direct-row:{}", second.id))
    });
    window.click_named(format!("direct-row:{}", group.id));
    window.until("the first group loads", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.loaded && feed.place == PlaceId::Direct(group.id))
    });
    window.type_into("composer-input", "first group draft");
    window.click_named(format!("direct-row:{}", second.id));
    window.until("the second group loads", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.loaded && feed.place == PlaceId::Direct(second.id))
    });
    window.type_into("composer-input", "second group draft");
    world
        .run(writer.api().rename_group(group.id, "Planning".to_owned()))
        .unwrap();
    window.click_channel("general");
    window.until_message("the shared plan");
    window.right_click_named(format!("message:{}", source.id));
    window.click("message-forward");
    let destination = format!("forward-destination:{}:direct:{}", server_id, group.id);
    assert!(window.shows_named(destination.clone()));
    window.click_named(destination.clone());
    window.click("forward-send");
    window.until(
        "the verified copy is delivered to the renamed group",
        |window| {
            window
                .feed()
                .is_some_and(|feed| feed.place == PlaceId::Direct(group.id) && feed.loaded)
                && window.shows_named(format!("forward-message:{}", source.id))
                && window
                    .view()
                    .lists()
                    .and_then(|lists| lists.conversation.as_ref())
                    .is_some_and(|conversation| conversation.name.as_deref() == Some("Planning"))
        },
    );
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "first group draft"
    );
    let reminder = world
        .run(writer.api().send_message(
            PlaceId::Direct(group.id),
            &SendMessage {
                document: document("return to this group"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until_message("return to this group");
    window.cx.update(|window, _| window.activate_window());
    window.settle();
    window.until("the current group is read", |window| {
        window.feed().is_some_and(|feed| feed.unread == 0)
    });
    window.right_click_named(format!("message:{}", reminder.id));
    window.click("message-mark-unread");
    window.until("the explicit group boundary is retained", |window| {
        window.feed().is_some_and(|feed| feed.unread == 1)
    });
    assert_eq!(
        world
            .run(
                window
                    .handle()
                    .api()
                    .message_state(PlaceId::Direct(group.id))
            )
            .unwrap()
            .last_read,
        reminder.position - 1
    );
    window.right_click_named(format!("message:{}", reminder.id));
    window.click("message-forward");
    window.click_named(format!(
        "forward-destination:{}:channel:{source_channel}",
        server_id
    ));
    window.click("forward-send");
    window.until(
        "the private group source can be read for its verified copy",
        |window| {
            window
                .feed()
                .is_some_and(|feed| feed.place == PlaceId::Channel(source_channel))
                && window.shows_named(format!("forward-message:{}", reminder.id))
                && window.shows_named(format!("forward-author-profile:{}", reminder.id))
        },
    );
    window.click_named(format!("forward-author-profile:{}", reminder.id));
    window.until("the verified group author opens its profile", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| {
                state.profile_view(window.shell.entity_id())
            })
            .is_some_and(|view| {
                view.account == logged_in(&writer) && matches!(view.read, ProfileRead::Shown(_))
            })
    });
    window.press("escape");
    window.click_named(format!("direct-row:{}", second.id));
    window.until(
        "the other group's draft belongs to the current feed",
        |window| {
            window
                .feed()
                .is_some_and(|feed| feed.loaded && feed.place == PlaceId::Direct(second.id))
        },
    );
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "second group draft"
    );
    window.click_channel("general");
    window.until_message("the shared plan");
    window.right_click_named(format!("message:{}", source.id));
    window.click("message-forward");
    window.click_named(destination.clone());
    let before = world
        .run(
            writer
                .api()
                .messages(PlaceId::Direct(group.id), &Default::default()),
        )
        .unwrap()
        .messages
        .len();
    world
        .run(window.handle().api().leave_group(group.id))
        .unwrap();
    window.until(
        "a departed group is no longer a forward destination",
        |window| !window.shows_named(destination.clone()),
    );
    if window.shows("forward-send") {
        window.click("forward-send");
    }
    assert_eq!(
        world
            .run(
                writer
                    .api()
                    .messages(PlaceId::Direct(group.id), &Default::default())
            )
            .unwrap()
            .messages
            .len(),
        before
    );
    window.press("escape");
    window.until(
        "losing group access removes its forward attribution",
        |window| {
            window.shows_named(format!("forward-unavailable:{}", reminder.id))
                && !window.shows_named(format!("forward-author-profile:{}", reminder.id))
        },
    );
    assert!(
        window
            .in_shell(|shell, _, _| shell.painted_readonly_text())
            .join(" ")
            .contains("return to this group")
    );
    window.click_named(format!("direct-row:{}", second.id));
    window.until(
        "the other group's draft belongs to the current feed",
        |window| {
            window
                .feed()
                .is_some_and(|feed| feed.loaded && feed.place == PlaceId::Direct(second.id))
        },
    );
    assert_eq!(
        window.in_shell(|shell, _, cx| shell.composer_text(Pane::Main, cx)),
        "second group draft"
    );
    assert!(
        world
            .run(
                admin
                    .handle
                    .api()
                    .messages(PlaceId::Direct(group.id), &Default::default())
            )
            .is_err()
    );
}
