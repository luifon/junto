//! What the window gives the accessibility interface of the system (ADR 0702): the window
//! activates accessibility as a screen reader does, and the test reads the tree of the frame
//! that gpui gives the platform.

use gpui::TestAppContext;
use junto_client_core::protocol::structure::{ChannelKind, CreateChannel};

use crate::harness::{World, member};

/// The nodes of the accessibility tree of the last frame of the window, after accessibility
/// was activated and the window drew again.
fn tree(window: &mut crate::harness::Window) -> serde_json::Value {
    let handle = window.cx.update(|window, _| window.window_handle());
    assert!(
        window.cx.cx.test_window(handle).simulate_a11y_activation(),
        "the window gave no accessibility callbacks"
    );
    window.cx.update(|window, _| window.refresh());
    window.settle();
    let json = window
        .cx
        .update(|window, _| window.debug_a11y_tree_json())
        .expect("the window built no accessibility tree");
    serde_json::from_str(&json).unwrap()
}

/// A node of the tree: its role, its label, its position in its set and the size of the set.
type Node = (String, Option<String>, Option<u64>, Option<u64>);

/// The nodes of the tree.
fn nodes(tree: &serde_json::Value) -> Vec<Node> {
    tree["nodes"]
        .as_object()
        .unwrap()
        .values()
        .map(|node| {
            let aria = &node["aria"];
            (
                aria["role"].as_str().unwrap_or_default().to_owned(),
                aria["label"].as_str().map(str::to_owned),
                aria["position_in_set"].as_u64(),
                aria["size_of_set"].as_u64(),
            )
        })
        .collect()
}

/// TG-16: the window gives the accessibility interface a name for every item of a list, the
/// role of a list item, and for the items of a list of the administration their place in the
/// list and the size of the list.
#[gpui::test]
fn every_list_item_has_a_name_and_the_administration_lists_their_places(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    let workspace_id = window.view().workspace().unwrap().id;
    let names = ["plans", "notes", "drafts"];
    let mut rows = Vec::new();
    for name in names {
        let channel = world
            .run(admin.handle.api().create_channel(
                workspace_id,
                &CreateChannel {
                    topic: String::new(),
                    name: name.to_owned(),
                    kind: ChannelKind::Text,
                    category_id: None,
                    position: None,
                },
            ))
            .unwrap();
        world
            .run(admin.handle.api().archive_channel(channel.id))
            .unwrap();
        rows.push(crate::harness::selector(format!(
            "admin-row-archived-{}",
            channel.id
        )));
    }
    window.open_administration();
    window.click("admin-nav-archive");
    window.until("the archived channels are listed", |window| {
        rows.iter().all(|row| window.shows(row))
    });
    let nodes = nodes(&tree(&mut window));
    let items: Vec<_> = nodes.iter().filter(|node| node.0 == "ListItem").collect();
    assert!(!items.is_empty(), "the window gives no list items");
    for (_, label, position, size) in &items {
        assert!(
            label
                .as_deref()
                .is_some_and(|label| !label.trim().is_empty()),
            "a list item has no name: position {position:?} of {size:?}"
        );
    }
    // The server, its workspace and a channel of the side panels.
    for name in ["Acme Studio", "general"] {
        assert!(
            items.iter().any(|item| item.1.as_deref() == Some(name)),
            "no list item is named {name}"
        );
    }
    // The archived channels: each item has its name, its place and the size of the list.
    let mut archived: Vec<(u64, u64, String)> = items
        .iter()
        .filter_map(|(_, label, position, size)| {
            let label = label.clone()?;
            names.contains(&label.as_str()).then(|| {
                (
                    position.unwrap_or_default(),
                    size.unwrap_or_default(),
                    label,
                )
            })
        })
        .collect();
    archived.sort();
    assert_eq!(
        archived
            .iter()
            .map(|item| (item.0, item.1))
            .collect::<Vec<_>>(),
        [(1, 3), (2, 3), (3, 3)],
        "the archived channels do not state their places in a list of three: {archived:?}"
    );
}
