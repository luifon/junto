use crate::harness::{World, member, open};
use gpui::TestAppContext;

#[gpui::test]
fn a_person_keeps_favourite_channels_above_the_server_categories_after_restart(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let channel = admin.text_channel(&world, &mut window, "planning");
    window.right_click_named(format!("channel-row:{channel}"));
    window.click("channel-favourite");
    window.until("favourites are listed first", |window| {
        window
            .view()
            .workspace()
            .unwrap()
            .sections
            .first()
            .is_some_and(|section| section.rows.iter().any(|row| row.id == channel))
    });
    window.close();
    let mut again = open(cx, directory.path());
    again.until_ready();
    assert!(
        again
            .view()
            .workspace()
            .unwrap()
            .sections
            .first()
            .unwrap()
            .rows
            .iter()
            .any(|row| row.id == channel)
    );
    again.right_click_named(format!("channel-row:{channel}"));
    again.click("channel-favourite");
    again.until("the channel returns to its original section", |window| {
        !window.shows("personal-favourites-header")
    });
}

fn drag(window: &mut crate::harness::Window, from: &'static str, to: &'static str) {
    use gpui::{Modifiers, MouseButton, point, px};
    window.settle();
    let start = window.cx.debug_bounds(from).unwrap().center();
    window.simulate(|cx| cx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none()));
    window.simulate(|cx| {
        cx.simulate_mouse_move(
            start + point(px(0.0), px(12.0)),
            MouseButton::Left,
            Modifiers::none(),
        )
    });
    assert!(window.cx.update(|_, cx| cx.has_active_drag()));
    window.settle();
    let end = window.cx.debug_bounds(to).unwrap().center();
    window.simulate(|cx| cx.simulate_mouse_move(end, MouseButton::Left, Modifiers::none()));
    window.simulate(|cx| cx.simulate_mouse_up(end, MouseButton::Left, Modifiers::none()));
    window.run_tasks();
}

fn favourites(
    window: &mut crate::harness::Window,
) -> Option<junto_client::state::view::ChannelSection> {
    window
        .view()
        .workspace()?
        .sections
        .iter()
        .find(|section| section.title == junto_client::state::view::SectionTitle::Favourites)
        .cloned()
}

#[gpui::test]
fn favourites_are_ordered_collapsed_marked_read_and_return_their_channels(cx: &mut TestAppContext) {
    use crate::harness::selector;
    use junto_client_core::protocol::structure::{ChannelKind, CreateChannel};
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let workspace = window.view().workspace().unwrap().id;
    let original = window.selected_channel();
    let first = admin.text_channel(&world, &mut window, "planning");
    let second = admin.text_channel(&world, &mut window, "ideas");
    let voice = world
        .run(admin.handle.api().create_channel(
            workspace,
            &CreateChannel {
                name: "call-room".to_owned(),
                topic: String::new(),
                kind: ChannelKind::Voice,
                category_id: None,
                position: None,
            },
        ))
        .unwrap();
    window.until("the voice channel is listed", |window| {
        window.shows_channel("call-room")
    });
    let head = "personal-favourites-header";
    for channel in [first, second] {
        window.right_click_named(format!("channel-row:{channel}"));
        assert!(!window.shows("channel-new-section"));
        window.click("channel-favourite");
    }
    window.until("both channels are favourites", |window| {
        favourites(window).is_some_and(|section| section.rows.len() == 2)
    });
    window.right_click_named(format!("channel-row:{second}"));
    window.click("channel-personal-up");
    window.until("the order of the favourites changes", |window| {
        favourites(window).is_some_and(|section| section.rows[0].id == second)
    });
    drag(
        &mut window,
        selector(format!("channel-row:{}", voice.id)),
        head,
    );
    window.until("a voice channel is dragged into the favourites", |window| {
        favourites(window).is_some_and(|section| section.rows.iter().any(|row| row.id == voice.id))
    });
    drag(
        &mut window,
        selector(format!("channel-row:{}", voice.id)),
        "personal-return-target",
    );
    window.until("a drag returns to the server grouping", |window| {
        favourites(window).is_some_and(|section| !section.rows.iter().any(|row| row.id == voice.id))
    });
    assert_eq!(
        window.selected_channel(),
        original,
        "arrangement keeps the selection"
    );
    window.click(head);
    window.until("collapse hides its unselected channels", |window| {
        !window.shows(selector(format!("channel-row:{first}")))
            && !window.shows(selector(format!("channel-row:{second}")))
    });
    window.press("space");
    window.until("Space expands the focused favourites header", |window| {
        window.shows(selector(format!("channel-row:{first}")))
    });
    window.press("enter");
    window.until("Enter collapses the focused favourites header", |window| {
        !window.shows(selector(format!("channel-row:{first}")))
    });
    let root = admin.send(
        &world,
        second,
        crate::notices::mention("A plan for ", window.own_account()),
    );
    let unread = "personal-favourites-unread";
    window.until("hidden unread activity is counted", |window| {
        window.shows(unread)
    });
    window.close();
    let mut again = open(cx, directory.path());
    again.until_ready();
    assert!(favourites(&mut again).unwrap().collapsed);
    assert_eq!(
        favourites(&mut again)
            .unwrap()
            .rows
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        [second, first]
    );
    again.right_click_named(head.to_owned());
    for gone in [
        "Rename section",
        "Remove section",
        "Move section up",
        "New section",
    ] {
        assert!(!again.shows(selector(format!("context-action:{gone}"))));
    }
    again.click("context-action:Mark as read");
    again.until("hidden messages are marked read", |window| {
        !window.shows(unread)
    });
    admin.reply(
        &world,
        root.id,
        crate::notices::mention("An update for ", again.own_account()),
    );
    again.until("hidden thread replies count too", |window| {
        window.shows(unread)
    });
    again.right_click_named(head.to_owned());
    again.click("context-action:Mark as read");
    again.until("mark as read includes the hidden thread", |window| {
        !window.shows(unread)
    });
    again.click(head);
    again.until("the favourites are expanded", |window| {
        favourites(window).is_some_and(|section| !section.collapsed)
    });
    again.right_click_named(head.to_owned());
    again.click("context-action:Collapse favourites");
    again.until("the menu collapses the favourites", |window| {
        favourites(window).is_some_and(|section| section.collapsed)
    });
    again.right_click_named(head.to_owned());
    again.click("context-action:Expand favourites");
    again.until("the menu expands the favourites", |window| {
        favourites(window).is_some_and(|section| !section.collapsed)
    });
    for channel in [first, second] {
        again.right_click_named(format!("channel-row:{channel}"));
        again.click("channel-favourite");
    }
    again.until("removing the favourites returns their channels", |window| {
        favourites(window).is_none()
            && window.shows(selector(format!("channel-row:{first}")))
            && window.shows(selector(format!("channel-row:{second}")))
    });
    assert_eq!(
        world
            .run(admin.handle.api().channels(workspace))
            .unwrap()
            .channels
            .iter()
            .filter(|channel| [first, second, voice.id].contains(&channel.id))
            .count(),
        3
    );
}

#[gpui::test]
fn new_category_at_the_end_of_the_list_creates_a_server_category_only_for_a_person_who_may(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, owner) = crate::admin_pages::owner_window(cx, &server);
    let workspace = window.view().workspace().unwrap().id;
    window.click("channel-list-new-category");
    window.until("the form of a new category is open", |window| {
        window.shows("category-name")
    });
    window.in_shell(|shell, window, cx| shell.type_admin_name("Meetings", window, cx));
    window.click("admin-form-send");
    window.until("the server holds the category", |window| {
        window.view().workspace().is_some_and(|workspace| {
            workspace.sections.iter().any(|section| {
                section.title
                    == junto_client::state::view::SectionTitle::Category("Meetings".to_owned())
            })
        })
    });
    assert!(
        world
            .run(owner.api().channels(workspace))
            .unwrap()
            .categories
            .iter()
            .any(|category| category.name == "Meetings")
    );
    let code = crate::admin_pages::invite(&world, &owner, Vec::new());
    let (_member_directory, mut member_window) = crate::harness::named(cx, "Rin");
    member_window.find_server(&server.address());
    member_window.join_with_code(code.expose());
    member_window.until_ready();
    assert!(
        !member_window
            .view()
            .workspace()
            .unwrap()
            .may_create_category
    );
    assert!(!member_window.shows("channel-list-new-category"));

    // A role that grants "manage channel" shows the button to the member, and its
    // withdrawal takes it away, while the list is open.
    use junto_client_core::protocol::WorkspacePermission;
    use junto_client_core::protocol::roles::CreateWorkspaceRole;
    let rin = member_window.own_account();
    let role = world
        .run(owner.api().create_workspace_role(
            workspace,
            &CreateWorkspaceRole {
                color: None,
                hoist: false,
                name: "Arrangers".to_owned(),
                permissions: vec![WorkspacePermission::ManageChannel],
            },
        ))
        .unwrap();
    world
        .run(owner.api().assign_workspace_role(workspace, rin, role.id))
        .unwrap();
    member_window.until("the role shows the button", |window| {
        window.shows("channel-list-new-category")
    });
    world
        .run(owner.api().unassign_workspace_role(workspace, rin, role.id))
        .unwrap();
    member_window.until("the withdrawal takes the button away", |window| {
        !window.shows("channel-list-new-category")
            && window
                .view()
                .workspace()
                .is_some_and(|workspace| !workspace.may_create_category)
    });
}

#[gpui::test]
fn the_named_sections_of_a_saved_book_go_and_their_channels_return_to_the_server_grouping(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let workspace = window.view().workspace().unwrap().id;
    let favourite = admin.text_channel(&world, &mut window, "planning");
    let placed = admin.text_channel(&world, &mut window, "meetings");
    window.right_click_named(format!("channel-row:{favourite}"));
    window.click("channel-favourite");
    window.until("the channel is a favourite", |window| {
        window.shows("personal-favourites-header")
    });
    let account = window.own_account();
    window.close();
    let path = directory.path().join("junto").join("servers.json");
    let mut book: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    book["format_version"] = serde_json::json!(9);
    let layout = &mut book["arrangement"]["servers"][0]["channel_layouts"][account.to_string()]
        [workspace.to_string()];
    layout["sections"] = serde_json::json!([{
        "id": "0191a0f0-0000-7000-8000-000000000005",
        "name": "Meetings",
        "collapsed": true,
        "channels": [placed],
    }]);
    std::fs::write(&path, serde_json::to_vec(&book).unwrap()).unwrap();
    let mut again = open(cx, directory.path());
    again.until_ready();
    let sections = again.view().workspace().unwrap().sections.clone();
    assert_eq!(
        sections
            .iter()
            .filter(|section| section.personal.is_some())
            .map(|section| section.rows.iter().map(|row| row.id).collect::<Vec<_>>())
            .collect::<Vec<_>>(),
        [vec![favourite]]
    );
    assert!(sections.iter().any(
        |section| section.personal.is_none() && section.rows.iter().any(|row| row.id == placed)
    ));
    assert!(again.shows(crate::harness::selector(format!("channel-row:{placed}"))));
}

#[gpui::test]
fn a_hidden_channel_cannot_remain_in_favourites(cx: &mut TestAppContext) {
    use junto_client_core::protocol::WorkspacePermission;
    use junto_client_core::protocol::structure::{RuleSubject, SetChannelOverride};
    let world = World::new();
    let server = world.server("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    let channel = admin.text_channel(&world, &mut window, "planning");
    window.right_click_named(format!("channel-row:{channel}"));
    window.click("channel-favourite");
    window.until("the channel is a favourite", |window| {
        window.shows("personal-favourites-header")
    });
    let subject = RuleSubject::Person {
        account_id: window.own_account(),
    };
    world
        .run(admin.handle.api().set_channel_rule(
            channel,
            subject,
            &SetChannelOverride {
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ViewChannel],
            },
        ))
        .unwrap();
    window.until(
        "permission loss hides the row and its favourites",
        |window| !window.shows_channel("planning") && !window.shows("personal-favourites-header"),
    );
    world
        .run(admin.handle.api().set_channel_rule(
            channel,
            subject,
            &SetChannelOverride {
                allow: Vec::new(),
                deny: Vec::new(),
            },
        ))
        .unwrap();
    window.until(
        "restored visibility restores its personal position",
        |window| window.shows("personal-favourites-header") && window.shows_channel("planning"),
    );
}
