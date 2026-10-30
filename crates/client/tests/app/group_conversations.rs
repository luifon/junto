use crate::{
    admin_pages::{invite, joined},
    harness::{World, member},
    messages::document,
    support::logged_in,
};
use gpui::TestAppContext;
use junto_client::shell::Pane;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::{ids::ConversationId, message::SendMessage};

#[gpui::test]
fn a_person_creates_names_extends_and_leaves_a_private_group(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_reader_device, reader) = joined(&world, &server, &code, "Reader");
    let code = invite(&world, &admin.handle, Vec::new());
    let (_third_device, third) = joined(&world, &server, &code, "Third");
    let code = invite(&world, &admin.handle, Vec::new());
    let (_added_device, added) = joined(&world, &server, &code, "Added");
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Writer");
    window.click("new-direct");
    window.until("new direct chooser", |window| window.shows("new-group"));
    window.click("new-group");
    for account in [logged_in(&reader), logged_in(&third)] {
        window.until("group participant is offered", |window| {
            window.shows_named(format!("contact-{account}"))
        });
        window.click_named(format!("contact-{account}"));
    }
    window.type_into("group-name", "Project");
    window.click("group-save");
    window.until("the named group opens", |window| {
        window
            .view()
            .lists()
            .and_then(|lists| lists.conversation.as_ref())
            .is_some_and(|row| row.name.as_deref() == Some("Project") && row.account_id.is_none())
    });
    let id: ConversationId = window
        .view()
        .lists()
        .unwrap()
        .conversation
        .as_ref()
        .unwrap()
        .id;
    window.until("the group history loads", |window| {
        window.feed().is_some_and(|feed| feed.loaded)
    });
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "Group plan", window, cx);
        shell.focus_composer(Pane::Main, window, cx);
    });
    window.press("enter");
    window.until_message("Group plan");
    assert_eq!(
        world
            .run(
                reader
                    .api()
                    .messages(PlaceId::Direct(id), &Default::default())
            )
            .unwrap()
            .messages
            .len(),
        1
    );
    assert!(
        world
            .run(
                admin
                    .handle
                    .api()
                    .messages(PlaceId::Direct(id), &Default::default())
            )
            .is_err()
    );
    window.in_shell(|shell, window, cx| {
        shell.type_message(Pane::Main, "@", window, cx);
        shell.focus_composer(Pane::Main, window, cx);
    });
    window.until("only the two other group members are offered", |window| {
        window.in_shell(|shell, _, _| shell.mention_names(Pane::Main))
            == vec!["Reader".to_owned(), "Third".to_owned()]
    });
    window.in_shell(|shell, window, cx| shell.type_message(Pane::Main, "@Re", window, cx));
    window.click_named("candidate:0".to_owned());
    window.press("enter");
    let reader_id = logged_in(&reader);
    window.until("the group mention is sent", |window| window.messages().iter().any(|row| row.message.document.as_ref().is_some_and(|document| document.blocks.iter().any(|block| matches!(block, junto_client_core::protocol::message::Block::Paragraph { content } if content.contains(&junto_client_core::protocol::message::Inline::Account { account_id: reader_id }))))));
    window.right_click_named(format!("direct-row:{id}"));
    window.click("direct-mute-0");
    window.until("group mute", |window| {
        window
            .view()
            .lists()
            .unwrap()
            .directs
            .iter()
            .any(|row| row.id == id && row.mute.is_on())
    });
    window.right_click_named(format!("direct-row:{id}"));
    window.click("direct-unmute");
    window.until("group unmute", |window| {
        window
            .view()
            .lists()
            .unwrap()
            .directs
            .iter()
            .any(|row| row.id == id && !row.mute.is_on())
    });
    window.right_click_named(format!("direct-row:{id}"));
    window.click("group-details-open");
    window.until("group member names", |window| {
        window.shows_named(format!("group-member-{}", logged_in(&reader)))
    });
    window.click("group-name");
    window.press("secondary-a backspace");
    window.type_into("group-name", "Planning");
    window.click("group-rename");
    window.until("renamed group", |window| {
        window
            .view()
            .lists()
            .unwrap()
            .conversation
            .as_ref()
            .is_some_and(|row| row.name.as_deref() == Some("Planning"))
    });
    window.right_click_named(format!("direct-row:{id}"));
    window.click("group-details-open");
    window.click("group-add");
    let added_id = logged_in(&added);
    window.until("additional person is offered", |window| {
        window.shows_named(format!("contact-{added_id}"))
    });
    window.click_named(format!("contact-{added_id}"));
    window.click("group-save");
    window.until("added member is confirmed", |window| {
        !window.shows("new-direct-dialog")
    });
    world
        .run(added.api().send_message(
            PlaceId::Direct(id),
            &SendMessage {
                document: document("Added to the project"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until_message("Added to the project");
    window.right_click_named(format!("direct-row:{id}"));
    window.click("group-details-open");
    window.click("group-leave");
    window.click("group-leave-confirm");
    window.until("left group disappears", |window| {
        !window
            .view()
            .lists()
            .unwrap()
            .directs
            .iter()
            .any(|row| row.id == id)
    });
    assert!(
        world
            .run(
                reader
                    .api()
                    .messages(PlaceId::Direct(id), &Default::default())
            )
            .is_ok()
    );
}

#[gpui::test]
fn a_group_member_menu_ends_when_that_person_leaves(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_reader_device, reader) = joined(&world, &server, &code, "Reader");
    let code = invite(&world, &admin.handle, Vec::new());
    let (_third_device, third) = joined(&world, &server, &code, "Third");
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Writer");
    let writer = window.view().person.account_id.unwrap();
    let third_id = logged_in(&third);
    let group = world
        .run(
            reader
                .api()
                .create_group("Project".to_owned(), vec![writer, third_id]),
        )
        .unwrap();
    window.until("group row", |window| {
        window.shows_named(format!("direct-row:{}", group.id))
    });
    window.right_click_named(format!("direct-row:{}", group.id));
    window.click("group-details-open");
    window.until("group person", |window| {
        window.shows_named(format!("group-member-{third_id}"))
    });
    window.right_click_named(format!("group-member-{third_id}"));
    window.until("group member context menu", |window| {
        window.shows("account-menu")
    });
    world.run(third.api().leave_group(group.id)).unwrap();
    window.until("the departed person's menu closes", |window| {
        !window.shows("account-menu")
    });
    assert!(
        world
            .run(
                reader
                    .api()
                    .messages(PlaceId::Direct(group.id), &Default::default())
            )
            .is_ok()
    );
}

fn late_group_creation_keeps_selection(cx: &mut TestAppContext, event_first: bool) {
    let world = World::new();
    let server = world.server("Harbor");
    let proxy = world.run(crate::support::FaultProxy::start(&server));
    let admin = world.owner(&server);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_reader_device, reader) = joined(&world, &server, &code, "Reader");
    let code = invite(&world, &admin.handle, Vec::new());
    let (_third_device, third) = joined(&world, &server, &code, "Third");
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Writer");
    let channel = window.selected_channel();
    let workspace = window.view().workspace().unwrap().id;
    window.click("new-direct");
    window.click("new-group");
    for account in [logged_in(&reader), logged_in(&third)] {
        window.until("group participant", |window| {
            window.shows_named(format!("contact-{account}"))
        });
        window.click_named(format!("contact-{account}"));
    }
    window.type_into("group-name", "Project");
    proxy.hold_events();
    proxy.hold_answer_ending("POST", "/group-conversations");
    window.click("group-save");
    window.until("creation answer is held", |_| proxy.answers_waiting() == 1);
    window.state.update(&mut window.cx, |state, cx| {
        state.select_channel(workspace, channel, cx)
    });
    let has_group = |window: &mut crate::harness::Window| {
        window
            .view()
            .lists()
            .unwrap()
            .directs
            .iter()
            .any(|row| row.name.as_deref() == Some("Project"))
    };
    if event_first {
        proxy.release_events();
        window.until(
            "the creation event is applied before its HTTP answer",
            has_group,
        );
        assert_eq!(proxy.answers_waiting(), 1);
        assert_eq!(window.selected_channel(), channel);
        assert!(window.view().lists().unwrap().conversation.is_none());
        proxy.release_answers();
        window.until("the completed chooser closes", |window| {
            !window.shows("new-direct-dialog")
        });
    } else {
        proxy.release_answers();
        window.until(
            "the HTTP answer closes the chooser before its event",
            |window| !window.shows("new-direct-dialog"),
        );
        assert_eq!(window.selected_channel(), channel);
        assert!(window.view().lists().unwrap().conversation.is_none());
        eprintln!(
            "created group row before its held event: {}",
            has_group(&mut window)
        );
        proxy.release_events();
        window.until(
            "the creation event is applied after its HTTP answer",
            has_group,
        );
    }
    assert_eq!(window.selected_channel(), channel);
    assert!(window.view().lists().unwrap().conversation.is_none());
    assert!(
        window
            .view()
            .lists()
            .unwrap()
            .directs
            .iter()
            .any(|row| row.name.as_deref() == Some("Project"))
    );
}

#[gpui::test]
fn a_late_group_creation_does_not_replace_a_newer_selection(cx: &mut TestAppContext) {
    late_group_creation_keeps_selection(cx, false);
}

#[gpui::test]
fn a_late_group_creation_does_not_replace_a_newer_selection_when_its_event_arrives_first(
    cx: &mut TestAppContext,
) {
    late_group_creation_keeps_selection(cx, true);
}

#[gpui::test]
fn an_unavailable_group_can_be_left_from_the_membership_list(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_writer_device, writer) = joined(&world, &server, &code, "Writer");
    let code = invite(&world, &admin.handle, Vec::new());
    let (_reader_device, reader) = joined(&world, &server, &code, "Reader");
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Departing");
    let own = window.own_account();
    let group = world
        .run(
            writer
                .api()
                .create_group("Project".to_owned(), vec![logged_in(&reader), own]),
        )
        .unwrap();
    window.until("the group is listed", |window| {
        window.shows_named(format!("direct-row:{}", group.id))
    });
    let workspace = window.view().workspace().unwrap().id;
    let other = world
        .run(admin.handle.api().create_workspace("Other".to_owned()))
        .unwrap();
    world
        .run(admin.handle.api().add_workspace_member(other.id, own))
        .unwrap();
    world
        .run(admin.handle.api().remove_workspace_member(
            workspace,
            own,
            &junto_client_core::api::Statement::default(),
        ))
        .unwrap();
    window.until("unavailable group metadata is withdrawn", |window| {
        window
            .view()
            .lists()
            .is_none_or(|lists| !lists.directs.iter().any(|row| row.id == group.id))
            && window.shows("new-direct")
    });
    window.click("new-direct");
    window.click("manage-group-memberships");
    window.until("the membership is listed", |window| {
        window.shows_named(format!("membership-leave:{}", group.id))
    });
    let server_id = window.handle().id();
    window.event(
        server_id,
        junto_client_core::protocol::events::Event::GroupConversationLeft {
            conversation_id: group.id,
            account_id: own,
        },
    );
    window.until(
        "a withdrawn membership closes the management source",
        |window| !window.shows("group-memberships"),
    );
    window.click("new-direct");
    window.click("manage-group-memberships");
    window.until(
        "a fresh membership read restores its current source",
        |window| window.shows_named(format!("membership-leave:{}", group.id)),
    );
    window.click_named(format!("membership-leave:{}", group.id));
    window.click("membership-leave-confirm");
    window.until("membership manager closes", |window| {
        !window.shows("group-memberships")
    });
    let groups = world.run(writer.api().groups()).unwrap();
    assert!(
        groups
            .iter()
            .any(|held| held.id == group.id && !held.account_ids.contains(&own))
    );
    assert!(
        world
            .run(
                window
                    .handle()
                    .api()
                    .messages(PlaceId::Direct(group.id), &Default::default())
            )
            .is_err()
    );
}
