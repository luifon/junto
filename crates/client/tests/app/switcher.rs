use crate::harness::named;
use gpui::TestAppContext;

#[gpui::test]
fn the_switcher_opens_from_an_input_and_escape_returns_focus(cx: &mut TestAppContext) {
    let (_directory, mut window) = named(cx, "Member");
    window.click("app-settings");
    window.click("settings-section-profile");
    window.click("name-field");
    window.press("secondary-k");
    assert!(window.shows("quick-switcher"));
    window.press("escape");
    assert!(!window.shows("quick-switcher"));
    assert!(window.shows("settings-dialog"));
    window.press("secondary-a");
    window.simulate(|cx| cx.simulate_input("Another member"));
    window.settle();
    window.click("save-name");
    window.until("typing returns to the original name field", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.preset_name() == "Another member"
        })
    });
}

use crate::{
    admin_pages::{default_workspace, text_channel},
    harness::{Window, World, member},
    support::logged_in,
};
use junto_client::state::switcher::{Destination, SwitchView};

fn choices(window: &mut Window) -> SwitchView {
    let owner = window.shell.entity_id();
    window
        .state
        .read_with(&window.cx, |state, _| state.switcher_view(owner).unwrap())
}

fn query(window: &mut Window, part: &str) {
    window.click("switcher-query");
    window.press("secondary-a");
    window.simulate(|cx| cx.simulate_input(part));
    window.settle();
}

#[gpui::test]
fn channels_and_people_are_found_across_servers_and_unread_channels_come_first(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let (harbor, mill) = (world.server("Harbor"), world.server("Mill"));
    let (harbor_admin, mill_admin) = (world.owner(&harbor), world.owner(&mill));
    let harbor_workspace = default_workspace(&world, &harbor_admin.handle);
    let mill_workspace = default_workspace(&world, &mill_admin.handle);
    let harbor_plan = text_channel(&world, &harbor_admin.handle, harbor_workspace, "Planning");
    let mill_plan = text_channel(&world, &mill_admin.handle, mill_workspace, "Planning");
    let (_directory, mut window) = member(cx, &world, &harbor.address(), &harbor_admin, "Member");
    let harbor_person = window.own_account();
    let harbor_id = window.view().selected.unwrap().id;
    window.join_further(&world, &mill, &mill_admin);
    let mill_id = window.view().selected.unwrap().id;
    harbor_admin.mention(&world, harbor_plan, "A plan for ", harbor_person);
    window.until("Harbor has unread messages", |window| {
        window
            .view()
            .servers
            .iter()
            .any(|row| row.id == harbor_id && row.unread.unread())
    });
    window.press("secondary-k");
    query(&mut window, "planning");
    window.until("both planning channels are found", |window| {
        choices(window).rows.len() == 2
    });
    let rows = choices(&mut window).rows;
    assert_eq!(
        rows[0].destination,
        Destination::Channel(harbor_id, harbor_workspace, harbor_plan)
    );
    assert!(rows[0].unread);
    assert_eq!(
        rows[1].destination,
        Destination::Channel(mill_id, mill_workspace, mill_plan)
    );
    window.press("down");
    window.press("enter");
    window.until("the selected channel is on Mill", |window| {
        window.selected_channel() == mill_plan && window.view().selected.unwrap().id == mill_id
    });
    assert!(!window.shows("quick-switcher"));
    assert!(window.view().call.is_none());

    window.press("secondary-k");
    query(&mut window, "ADA");
    window.until("the contacts routes of both servers answered", |window| {
        let view = choices(window);
        !view.reading && view.rows.len() == 2
    });
    let ada = logged_in(&harbor_admin.handle);
    let rows = choices(&mut window).rows;
    let index = rows
        .iter()
        .position(|row| row.destination == Destination::Person(harbor_id, ada))
        .unwrap();
    assert!(rows[index].context.contains("Harbor"));
    window.click_named(format!("switcher-row:{index}"));
    window.until("the new conversation is on Harbor", |window| {
        window
            .view()
            .selected
            .is_some_and(|selected| selected.id == harbor_id)
            && window.view().lists().is_some_and(|lists| {
                lists
                    .conversation
                    .as_ref()
                    .is_some_and(|row| row.account_id == Some(ada))
            })
    });
    window.press("secondary-k");
    query(&mut window, "Ada");
    window.until(
        "the existing conversation replaces the same person",
        |window| !choices(window).reading && choices(window).rows.len() == 2,
    );
    let rows = choices(&mut window).rows;
    assert_eq!(
        rows.iter()
            .filter(|row| row.destination.server() == harbor_id)
            .count(),
        1
    );
    assert!(rows.iter().any(|row| row.destination.server() == harbor_id
        && matches!(row.destination, Destination::Conversation(..))));
    query(&mut window, "PLANNING harbor");
    window.until("multiple words narrow the origin", |window| {
        choices(window).rows.len() == 1
    });
    window.press("enter");
    window.until("the background server channel is selected", |window| {
        window.selected_channel() == harbor_plan
    });
}

#[gpui::test]
fn closed_queries_failures_logout_and_two_windows_keep_separate_searches(cx: &mut TestAppContext) {
    use crate::support::{Fault, FaultProxy};
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Member");
    let server_id = window.view().selected.unwrap().id;
    proxy.hold_request("GET", "/contacts");
    window.press("secondary-k");
    query(&mut window, "M");
    window.until("the old query is held", |_| {
        proxy
            .all()
            .iter()
            .any(|request| request.path.contains("q=M&"))
    });
    window.press("escape");
    window.press("secondary-k");
    query(&mut window, "A");
    window.until("the new query is held", |_| {
        proxy
            .all()
            .iter()
            .any(|request| request.path.contains("q=A&"))
    });
    proxy.release_requests();
    window.until("only the reopened query is shown", |window| {
        let view = choices(window);
        !view.reading && view.rows.iter().any(|row| row.name == "Ada")
    });
    window.press("escape");
    proxy.fail_ending("?q=Ada&limit=50", [Fault::Refused]);
    window.press("secondary-k");
    query(&mut window, "Ada");
    window.until_read(
        "the lookup failure is shown",
        |window| {
            (
                window.shows("switcher-failed"),
                choices(window),
                proxy
                    .all()
                    .iter()
                    .filter(|request| request.path.contains("/contacts"))
                    .map(|request| {
                        (
                            request.path.clone(),
                            request.answer.as_ref().map(|(status, _)| *status),
                        )
                    })
                    .collect::<Vec<_>>(),
            )
        },
        |(shown, _, _)| *shown,
    );
    window.click("switcher-retry");
    window.until("retry offers Ada", |window| {
        let view = choices(window);
        !view.reading && !view.failed && view.rows.len() == 1
    });
    proxy.hold_request("GET", "/contacts");
    let before = proxy
        .all()
        .iter()
        .filter(|request| request.path.contains("/contacts"))
        .count();
    window.state.update(&mut window.cx, |state, cx| {
        state.notice_fixture(
            server_id,
            &junto_client_core::live::Notice::Event {
                id: 0,
                at: time::OffsetDateTime::now_utc(),
                event: std::sync::Arc::new(
                    junto_client_core::protocol::events::Event::PermissionsChanged {
                        workspace_id: None,
                    },
                ),
            },
            cx,
        )
    });
    window.until("permissions invalidate the contact cache", |_| {
        proxy
            .all()
            .iter()
            .filter(|request| request.path.contains("/contacts"))
            .count()
            > before
    });
    proxy.release_requests();
    window.until("the permitted contacts are read again", |window| {
        !choices(window).reading && choices(window).rows.len() == 1
    });
    let old_destination = choices(&mut window).rows[0].destination;
    cx.update(|cx| junto_client::shell::detached::open(window.state.clone(), cx));
    let mut detached = window.call_window();
    detached.press("secondary-k");
    query(&mut detached, "Harbor");
    assert!(
        choices(&mut detached)
            .rows
            .iter()
            .any(|row| row.destination == Destination::Server(server_id))
    );
    assert_eq!(choices(&mut window).rows[0].destination, old_destination);
    detached.press("escape");
    assert!(window.shows("quick-switcher"));
    window
        .state
        .update(&mut window.cx, |state, cx| state.log_out(server_id, cx));
    window.until("logout removes contact destinations", |window| {
        choices(window).rows.is_empty()
    });
    let owner = window.shell.entity_id();
    assert!(
        !window
            .state
            .update(&mut window.cx, |state, cx| state.choose_switcher(
                owner,
                old_destination,
                cx
            ))
    );
}

#[gpui::test]
fn a_cold_switcher_finds_a_departed_conversation_peer_on_another_server(
    cx: &mut TestAppContext,
    other_cx: &mut TestAppContext,
) {
    let world = World::new();
    let (harbor, mill) = (world.server("Harbor"), world.server("Mill"));
    let (harbor_admin, mill_admin) = (world.owner(&harbor), world.owner(&mill));
    let (directory, mut window) = member(cx, &world, &harbor.address(), &harbor_admin, "Member");
    let (_peer_directory, mut peer) =
        member(other_cx, &world, &harbor.address(), &harbor_admin, "Marta");
    let harbor_id = window.view().selected.unwrap().id;
    let conversation = world
        .run(window.handle().api().open_conversation(peer.own_account()))
        .unwrap();
    let workspace = default_workspace(&world, &harbor_admin.handle);
    world
        .run(peer.handle().api().leave_workspace(workspace))
        .unwrap();
    peer.close();
    window.join_further(&world, &mill, &mill_admin);
    let mill_id = window.view().selected.unwrap().id;
    window.state.update(&mut window.cx, |state, cx| {
        state.move_server(mill_id, junto_client::state::book::Step::Up, cx)
    });
    window.until("Mill is first for the next cold start", |window| {
        window.view().servers[0].id == mill_id
    });
    window.close();
    let mut cold = crate::harness::open(cx, directory.path());
    cold.until_ready();
    assert_eq!(cold.view().selected.unwrap().id, mill_id);
    cold.press("secondary-k");
    cold.until(
        "the background server's retained conversation loads",
        |window| {
            choices(window)
                .rows
                .iter()
                .any(|row| row.destination == Destination::Conversation(harbor_id, conversation.id))
        },
    );
    query(&mut cold, "Marta");
    cold.until("the name and contact requests finish", |window| {
        !choices(window).reading
    });
    let rows = choices(&mut cold).rows;
    assert!(
        rows.iter().any(|row| row.destination
            == Destination::Conversation(harbor_id, conversation.id)
            && row.name == "Marta (inactive)"),
        "historical counterpart name is missing: {rows:?}"
    );
    cold.press("enter");
    cold.until("the retained conversation opens on Harbor", |window| {
        window
            .view()
            .selected
            .as_ref()
            .is_some_and(|row| row.id == harbor_id)
            && window.view().lists().is_some_and(|lists| {
                lists
                    .conversation
                    .as_ref()
                    .is_some_and(|row| row.id == conversation.id)
            })
    });
}

#[gpui::test]
fn named_groups_follow_unread_order_rename_and_membership_in_the_switcher(cx: &mut TestAppContext) {
    use crate::admin_pages::{invite, joined};
    use junto_client_core::{model::PlaceId, protocol::message::SendMessage};
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let code = invite(&world, &admin.handle, Vec::new());
    let (_writer_device, writer) = joined(&world, &server, &code, "Writer");
    let code = invite(&world, &admin.handle, Vec::new());
    let (_third_device, third) = joined(&world, &server, &code, "Third");
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    let mut groups = Vec::new();
    for name in ["Planning", "Planning later"] {
        groups.push(
            world
                .run(
                    window
                        .handle()
                        .api()
                        .create_group(name.to_owned(), vec![logged_in(&writer), logged_in(&third)]),
                )
                .unwrap(),
        );
    }
    let server_id = window.handle().id();
    window.until("both group sources are current", |window| {
        groups
            .iter()
            .all(|group| window.shows_named(format!("direct-row:{}", group.id)))
    });
    world
        .run(writer.api().send_message(
            PlaceId::Direct(groups[1].id),
            &SendMessage {
                document: crate::messages::document("an unread plan"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until("the second group has unread content", |window| {
        window.view().lists().is_some_and(|lists| {
            lists
                .directs
                .iter()
                .any(|row| row.id == groups[1].id && row.unread.unread())
        })
    });
    window.press("secondary-k");
    query(&mut window, "plan");
    window.until("contact discovery is finished", |window| {
        !choices(window).reading
    });
    let rows = choices(&mut window).rows;
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].destination,
        Destination::Conversation(server_id, groups[1].id)
    );
    assert!(rows[0].unread);
    assert_eq!(
        rows[1].destination,
        Destination::Conversation(server_id, groups[0].id)
    );
    world
        .run(
            writer
                .api()
                .rename_group(groups[0].id, "Plans revised".to_owned()),
        )
        .unwrap();
    window.until("the same group identity has its fresh label", |window| {
        choices(window).rows.iter().any(|row| {
            row.destination == Destination::Conversation(server_id, groups[0].id)
                && row.name == "Plans revised"
        })
    });
    window.press("enter");
    window.until("the first unread group opens", |window| {
        window
            .feed()
            .is_some_and(|feed| feed.place == PlaceId::Direct(groups[1].id))
    });
    assert!(!window.shows("quick-switcher"));
    window.press("secondary-k");
    query(&mut window, "Plans revised");
    window.until("the renamed group is the only destination", |window| {
        !choices(window).reading && choices(window).rows.len() == 1
    });
    world
        .run(window.handle().api().leave_group(groups[0].id))
        .unwrap();
    window.until("a departed group is no longer selectable", |window| {
        !choices(window).reading
            && choices(window)
                .rows
                .iter()
                .all(|row| row.destination != Destination::Conversation(server_id, groups[0].id))
    });
    window.press("enter");
    assert!(
        window
            .feed()
            .is_none_or(|feed| feed.place != PlaceId::Direct(groups[0].id))
    );
    assert!(
        world
            .run(
                window
                    .handle()
                    .api()
                    .messages(PlaceId::Direct(groups[0].id), &Default::default())
            )
            .is_err()
    );
}
