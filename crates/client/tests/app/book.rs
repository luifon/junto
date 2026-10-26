//! The arrangement of the server book against real servers (ADR 0775): groups of servers made
//! and changed by dragging and by their menus, the order and the collapse of the workspaces
//! of a server, the signs of what they hold, mark as read, and a second start that shows the
//! same server book. Each drag also has its way through a menu that the keyboard reaches
//! (ADR 0702).

use gpui::{Modifiers, MouseButton, TestAppContext, point, px};
use junto_client::state::view::BookPart;
use junto_client_core::arrangement::GroupColour;
use junto_client_core::protocol::ids::WorkspaceId;
use junto_client_core::protocol::structure::{ChannelKind, CreateChannel};

use crate::harness::{Window, World, member, open, selector};
use crate::messages::document;

/// Drags from the middle of the element `from` to the point of the element `to` at `at` of
/// its height, as the pointer does: a press, a move that starts the drag, a move to the
/// target and the release. Returns whether a drag started.
#[track_caller]
fn drag(window: &mut Window, from: &str, to: &str, at: f32) -> bool {
    window.settle();
    let (from, to) = (selector(from.to_owned()), selector(to.to_owned()));
    let start = window
        .cx
        .debug_bounds(from)
        .unwrap_or_else(|| panic!("{from} is not drawn"))
        .center();
    let target = window
        .cx
        .debug_bounds(to)
        .unwrap_or_else(|| panic!("{to} is not drawn"));
    let end = point(target.center().x, target.origin.y + target.size.height * at);
    window.simulate(|cx| cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none()));
    window.simulate(|cx| {
        cx.simulate_mouse_move(
            start + point(px(0.0), px(6.0)),
            MouseButton::Left,
            Modifiers::none(),
        )
    });
    let started = window.cx.update(|_, cx| cx.has_active_drag());
    window.simulate(|cx| cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none()));
    window.simulate(|cx| cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none()));
    window.settle();
    started
}

impl Window {
    /// The names, the colours and the collapse of the groups, with the number of their
    /// servers, in the order of the server book.
    fn groups(&mut self) -> Vec<(String, GroupColour, bool, usize)> {
        self.view()
            .groups
            .iter()
            .map(|group| {
                (
                    group.shown_name.clone(),
                    group.colour,
                    group.collapsed,
                    group.servers.len(),
                )
            })
            .collect()
    }

    fn workspaces_of_first(&mut self) -> Vec<String> {
        self.view().servers[0]
            .workspaces
            .iter()
            .map(|workspace| workspace.name.clone())
            .collect()
    }

    fn workspace_row(&mut self, name: &str) -> String {
        let id: WorkspaceId = self.view().servers[0]
            .workspaces
            .iter()
            .find(|workspace| workspace.name == name)
            .unwrap_or_else(|| panic!("no workspace {name}"))
            .id;
        format!("workspace-row-{}", id.as_uuid().as_u128())
    }
}

#[gpui::test]
fn servers_are_grouped_by_dragging_and_by_their_menus_and_a_second_start_shows_the_groups(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let (acme, harbor, mill) = (
        world.server("Acme Studio"),
        world.server("Harbor"),
        world.server("Mill"),
    );
    let (acme_admin, harbor_admin, mill_admin) =
        (world.owner(&acme), world.owner(&harbor), world.owner(&mill));
    let (directory, mut window) = member(cx, &world, &acme.address(), &acme_admin, "Luca");
    window.join_further(&world, &harbor, &harbor_admin);
    window.join_further(&world, &mill, &mill_admin);
    let mill_general = window.selected_channel();
    assert_eq!(window.server_names(), ["Acme Studio", "Harbor", "Mill"]);
    assert!(window.view().groups.is_empty());

    // Mill dropped on the middle of Acme Studio: a group of the two at the place of Acme
    // Studio, named by its servers.
    assert!(drag(&mut window, "server-2", "server-0", 0.5));
    window.until("the two servers are a group", |window| {
        window.view().groups.len() == 1
    });
    assert_eq!(window.server_names(), ["Acme Studio", "Mill", "Harbor"]);
    assert_eq!(
        window.view().book,
        [BookPart::Group(0), BookPart::Server(2)]
    );
    assert_eq!(
        window.groups(),
        [("Acme Studio, Mill".to_owned(), GroupColour::Bone, false, 2)]
    );
    assert!(window.shows("group-0") && window.shows("group-servers-0"));

    // The menu of the group from the secondary press: a name and a colour.
    window.right_click_named("group-0".to_owned());
    assert!(window.shows("group-menu"));
    window.click("rename-group");
    assert!(window.shows("rename-group-dialog"));
    window.type_into("group-name", "Work");
    window.click("save-group-name");
    window.until("the group has its name", |window| {
        window.groups()[0].0 == "Work"
    });
    window.right_click_named("group-0".to_owned());
    window.click("group-colour-4");
    window.until("the group has its colour", |window| {
        window.groups()[0].1 == GroupColour::Cypress
    });
    // The menu stays open after a colour, as after a move.
    assert!(window.shows("group-menu"));
    window.press("escape");
    assert!(!window.shows("group-menu"));

    // Harbor dropped on the middle of the head of the group joins it at its end, and the
    // free space after the last server takes it out to the end of the server book.
    assert!(drag(&mut window, "server-2", "group-0", 0.5));
    window.until("Harbor is in the group", |window| window.groups()[0].3 == 3);
    assert_eq!(window.server_names(), ["Acme Studio", "Mill", "Harbor"]);
    assert!(drag(&mut window, "server-2", "server-book-end", 0.5));
    window.until("Harbor is out of the group, at the end", |window| {
        window.groups()[0].3 == 2 && window.view().book == [BookPart::Group(0), BookPart::Server(2)]
    });
    // Within the group: Mill dropped on the upper third of Acme Studio stands before it.
    assert!(drag(&mut window, "server-1", "server-0", 0.1));
    window.until("Mill is the first server of the group", |window| {
        window.server_names() == ["Mill", "Acme Studio", "Harbor"]
    });
    // The group dropped on the lower half of Harbor: the group after it.
    assert!(drag(&mut window, "group-0", "server-2", 0.9));
    window.until("the group is after Harbor", |window| {
        window.server_names() == ["Harbor", "Mill", "Acme Studio"]
            && window.view().book == [BookPart::Server(0), BookPart::Group(0)]
    });

    // A press on the head collapses the group: its servers are hidden, and the head counts
    // them and shows what they hold unread.
    window.click("group-0");
    window.until("the group is collapsed", |window| window.groups()[0].2);
    assert!(!window.shows("server-1") && !window.shows("server-2"));
    assert!(window.shows("server-0") && window.shows("group-count-0"));
    assert!(!window.shows("group-unread-0"));
    mill_admin.send(&world, mill_general, document("A new plan"));
    window.until("the head of the group shows the unread message", |window| {
        window.shows("group-unread-0") && window.view().groups[0].unread.any()
    });

    // The head keeps the keyboard focus after the press; the menu key opens its menu.
    window.press("menu");
    assert!(window.shows("group-menu"));
    window.click("group-move-up");
    window.until("the group is first", |window| {
        window.server_names() == ["Mill", "Acme Studio", "Harbor"]
    });
    assert!(window.shows("group-menu"));
    window.click("mark-group-read");
    assert!(!window.shows("group-menu"));
    window.until("the group holds nothing unread", |window| {
        !window.view().groups[0].unread.any() && !window.shows("group-unread-0")
    });

    // The menu of a server gives each move of a drag: into a group, out of it, and a new
    // group. Harbor is the selected server.
    window.click("server-2");
    window.until("Harbor is selected", |window| {
        window
            .view()
            .selected
            .is_some_and(|server| server.name == "Harbor")
    });
    window.open_server_menu();
    window.click("menu-server-group");
    assert!(window.shows("group-choice"));
    window.click("move-into-group-0");
    window.until("Harbor is in the group again", |window| {
        window.groups()[0].3 == 3
    });
    window.open_server_menu();
    window.click("menu-server-group");
    window.click("remove-from-group");
    window.until("Harbor stands after the group", |window| {
        window.groups()[0].3 == 2 && window.view().book == [BookPart::Group(0), BookPart::Server(2)]
    });
    window.open_server_menu();
    window.click("menu-server-group");
    window.click("new-group");
    window.until("Harbor has a group of its own", |window| {
        window.view().groups.len() == 2
    });
    let book = (window.server_names(), window.groups());
    assert_eq!(
        book.1,
        [
            ("Work".to_owned(), GroupColour::Cypress, true, 2),
            ("Harbor".to_owned(), GroupColour::Bone, false, 1),
        ]
    );

    // A second start shows the same server book.
    window.close();
    let mut again = open(cx, directory.path());
    again.until_ready();
    assert_eq!((again.server_names(), again.groups()), book);

    // Dissolve: the servers stay in their place, outside a group.
    again.right_click_named("group-0".to_owned());
    again.click("dissolve-group");
    again.until("the first group is dissolved", |window| {
        window.view().groups.len() == 1
    });
    assert_eq!(again.server_names(), book.0);
    assert_eq!(
        again.view().book,
        [BookPart::Server(0), BookPart::Server(1), BookPart::Group(0)]
    );
}

#[gpui::test]
fn the_workspaces_of_a_server_are_ordered_collapsed_and_marked_read_and_a_second_start_shows_it(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let own = window.own_account();
    let api = admin.handle.api();
    let mut plans = None;
    for name in ["Lab", "Mill"] {
        let workspace = world.run(api.create_workspace(name.to_owned())).unwrap();
        world
            .run(api.add_workspace_member(workspace.id, own))
            .unwrap();
        if name == "Lab" {
            let channel = world
                .run(api.create_channel(
                    workspace.id,
                    &CreateChannel {
                        name: "plans".to_owned(),
                        kind: ChannelKind::Text,
                        category_id: None,
                        position: None,
                    },
                ))
                .unwrap();
            plans = Some(channel.id);
        }
    }
    let plans = plans.unwrap();
    window.until("the server book lists the three workspaces", |window| {
        window.workspaces_of_first().len() == 3
    });
    let listed = window.workspaces_of_first();
    let (first, last) = (listed[0].clone(), listed[2].clone());

    // The last workspace dropped on the upper half of the first one: it is the first.
    let (from, to) = (window.workspace_row(&last), window.workspace_row(&first));
    assert!(drag(&mut window, &from, &to, 0.2));
    window.until("the last workspace is the first", |window| {
        window.workspaces_of_first()[0] == last
    });
    let dragged = window.workspaces_of_first();
    assert_eq!(
        dragged,
        [last.clone(), listed[0].clone(), listed[1].clone()]
    );

    // Its menu from the secondary press: one place down, and the item at the end says why
    // it does nothing.
    let row = window.workspace_row(&last);
    window.right_click_named(row);
    assert!(window.shows("workspace-menu"));
    window.click("workspace-move-down");
    window.until("the workspace moved one place down", |window| {
        window.workspaces_of_first()[1] == last
    });
    window.press("escape");
    // The keyboard: a press on the row selects the workspace and keeps the focus there,
    // and the menu key opens the menu of the row.
    let row = window.workspace_row(&last);
    window.click_named(row);
    window.press("menu");
    assert!(window.shows("workspace-menu"));
    window.click("workspace-move-up");
    window.until("the workspace is the first again", |window| {
        window.workspaces_of_first()[0] == last
    });
    window.press("escape");
    let ordered = window.workspaces_of_first();

    // The workspaces of the server collapse; the head of the server then shows what they
    // hold unread.
    window.click("workspaces-toggle-0");
    window.until("the workspaces are collapsed", |window| {
        window.view().servers[0].workspaces_collapsed
    });
    let hidden = window.workspace_row(&last);
    assert!(!window.shows_named(hidden));
    admin.send(&world, plans, document("Plans for the week"));
    window.until(
        "the head of the server shows the unread message",
        |window| window.shows("server-head-unread-0"),
    );
    // The menu of the server expands them again.
    window.open_server_menu();
    window.click("menu-server-workspaces");
    window.until("the workspaces are expanded", |window| {
        !window.view().servers[0].workspaces_collapsed
    });
    window.press("escape");
    assert!(!window.shows("server-head-unread-0"));
    let lab = window.workspace_row("Lab");
    window.right_click_named(lab);
    window.click("mark-workspace-read");
    window.until("Lab holds nothing unread", |window| {
        !window.view().servers[0]
            .workspaces
            .iter()
            .any(|workspace| workspace.unread.any())
    });

    // One control collapses every server and group, and expands them again.
    window.click("collapse-everything");
    window.until("everything is collapsed", |window| {
        window.view().servers[0].workspaces_collapsed
    });
    window.click("collapse-everything");
    window.until("everything is expanded", |window| {
        !window.view().servers[0].workspaces_collapsed
    });
    window.click("workspaces-toggle-0");
    window.until("the workspaces are collapsed", |window| {
        window.view().servers[0].workspaces_collapsed
    });

    // Mark the server as read from its menu.
    admin.send(&world, plans, document("And one more"));
    window.until("the head shows the unread message", |window| {
        window.shows("server-head-unread-0")
    });
    window.open_server_menu();
    window.click("menu-mark-server-read");
    window.until("the server holds nothing unread", |window| {
        !window.view().servers[0].unread.any()
    });

    // A second start shows the order and the collapse.
    window.close();
    let mut again = open(cx, directory.path());
    again.until_ready();
    assert_eq!(again.workspaces_of_first(), ordered);
    assert!(again.view().servers[0].workspaces_collapsed);
}
