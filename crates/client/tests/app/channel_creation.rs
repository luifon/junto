use gpui::TestAppContext;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::structure::{
    ChannelKind, CreateCategory, CreateChannel, RuleSubject, SetChannelOverride,
};

use crate::admin_pages::{invite, owner_window};
use crate::harness::{World, named, selector};

#[gpui::test]
fn category_buttons_and_menus_create_channels_only_where_the_person_may(
    cx: &mut TestAppContext,
    member_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let workspace = window.view().workspace().unwrap().id;
    let category = world
        .run(owner.api().create_category(
            workspace,
            &CreateCategory {
                name: "Planning".to_owned(),
                position: None,
            },
        ))
        .unwrap();
    world
        .run(owner.api().create_channel(
            workspace,
            &CreateChannel {
                name: "agenda".to_owned(),
                topic: String::new(),
                kind: ChannelKind::Text,
                category_id: Some(category.id),
                position: None,
            },
        ))
        .unwrap();
    let code = invite(&world, &owner, Vec::new());
    let (_member_directory, mut member) = named(member_cx, "Rin");
    member.find_server(&server.address());
    member.join_with_code(code.expose());
    member.until_ready();
    let head = selector(format!("category-header:{}", category.id));
    let create = selector(format!("category-create:{}", category.id));
    member.until("the category is visible to the reader", |window| {
        window.shows(head)
    });
    assert!(!member.shows(create));
    window.until("the category is visible to its manager", |window| {
        window.shows(head)
    });
    assert!(
        window.shows(create),
        "the category manager needs its creation button"
    );
    window.click(head);
    window.click(create);
    window.type_into("channel-name", "milestones");
    window.click("admin-form-send");
    window.until("the text channel is created", |window| {
        window
            .channels()
            .iter()
            .any(|row| row.name == "milestones" && row.kind == ChannelKind::Text)
    });
    window.press("escape");
    assert!(
        window
            .view()
            .workspace()
            .unwrap()
            .sections
            .iter()
            .find(|section| section.category_id == Some(category.id))
            .unwrap()
            .collapsed
    );
    window.right_click_named(head.to_owned());
    window.click("context-action:New channel");
    window.type_into("channel-name", "standup");
    window.click("channel-kind-1");
    window.click("admin-form-send");
    window.until("the voice channel is created", |window| {
        window
            .channels()
            .iter()
            .any(|row| row.name == "standup" && row.kind == ChannelKind::Voice)
    });
    let channels = world.run(owner.api().channels(workspace)).unwrap().channels;
    assert!(
        channels
            .iter()
            .filter(|channel| ["milestones", "standup"].contains(&channel.name.as_str()))
            .all(|channel| channel.category_id == Some(category.id))
    );
    window.press("escape");
    window.click(selector(format!("section-create:{workspace}:voice")));
    window.type_into("channel-name", "lobby");
    window.click("admin-form-send");
    window.until("a voice section starts with the voice kind", |window| {
        window
            .channels()
            .iter()
            .any(|row| row.name == "lobby" && row.kind == ChannelKind::Voice)
    });
    assert!(
        world
            .run(owner.api().channels(workspace))
            .unwrap()
            .channels
            .iter()
            .any(|channel| channel.name == "lobby" && channel.category_id.is_none())
    );
    window.press("escape");
    let subject = RuleSubject::Person {
        account_id: member.own_account(),
    };
    world
        .run(owner.api().set_category_rule(
            (workspace, category.id),
            subject,
            &SetChannelOverride {
                allow: vec![WorkspacePermission::ManageChannel],
                deny: Vec::new(),
            },
        ))
        .unwrap();
    member.until("a category manager gets the creation button", |window| {
        window.shows(create)
    });
    member.click(create);
    member.type_into("channel-name", "estimates");
    member.click("admin-form-send");
    member.until("category permission alone creates the channel", |window| {
        window.channels().iter().any(|row| row.name == "estimates")
    });
    member.press("escape");
    world
        .run(owner.api().set_category_rule(
            (workspace, category.id),
            subject,
            &SetChannelOverride {
                allow: Vec::new(),
                deny: vec![WorkspacePermission::ManageChannel],
            },
        ))
        .unwrap();
    member.until("losing category management removes the button", |window| {
        !window.shows(create)
    });
}
