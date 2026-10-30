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

fn section(
    window: &mut crate::harness::Window,
    name: &str,
) -> Option<junto_client::state::view::ChannelSection> {
    window.view().workspace()?.sections.iter().find(|section| matches!(&section.title, junto_client::state::view::SectionTitle::Personal(held) if held == name)).cloned()
}

fn section_id(
    window: &mut crate::harness::Window,
    name: &str,
) -> junto_client_core::arrangement::channels::SectionId {
    let Some(junto_client_core::arrangement::channels::Location::Section(id)) =
        section(window, name).unwrap().personal
    else {
        panic!("a named section has its identifier");
    };
    id
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

#[gpui::test]
fn personal_sections_name_order_collapse_mark_read_and_return_their_channels(
    cx: &mut TestAppContext,
) {
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
    window.right_click_named(format!("channel-row:{first}"));
    window.click("channel-new-section");
    window.type_into("personal-section-name", "Projects");
    window.press("enter");
    window.until("the channel is in its new section", |window| {
        section(window, "Projects")
            .is_some_and(|section| section.rows.iter().any(|row| row.id == first))
    });
    let id = section_id(&mut window, "Projects");
    let head = selector(format!("personal-section-header:{id}"));
    window.right_click_named(format!("channel-row:{second}"));
    window.click(selector(format!("channel-section:{id}")));
    window.until("both channels belong to the section", |window| {
        section(window, "Projects").is_some_and(|section| section.rows.len() == 2)
    });
    window.right_click_named(format!("channel-row:{second}"));
    window.click("channel-personal-up");
    window.until("manual channel order changes", |window| {
        section(window, "Projects").is_some_and(|section| section.rows[0].id == second)
    });
    drag(
        &mut window,
        selector(format!("channel-row:{}", voice.id)),
        head,
    );
    window.until("a voice channel is dragged into the section", |window| {
        section(window, "Projects")
            .is_some_and(|section| section.rows.iter().any(|row| row.id == voice.id))
    });
    drag(
        &mut window,
        selector(format!("channel-row:{}", voice.id)),
        "personal-return-target",
    );
    window.until("a drag returns to the server grouping", |window| {
        section(window, "Projects")
            .is_some_and(|section| !section.rows.iter().any(|row| row.id == voice.id))
    });
    window.right_click_named(head.to_owned());
    window.click("context-action:Rename section");
    window.click("personal-section-name");
    window.press("secondary-a backspace");
    window.click("personal-section-save");
    assert!(window.shows("personal-section-dialog"));
    window.type_into("personal-section-name", "Roadmap");
    window.click("personal-section-save");
    window.until("the section is renamed", |window| {
        section(window, "Roadmap").is_some()
    });
    assert!(
        !section(&mut window, "Roadmap").unwrap().collapsed,
        "renaming must preserve expansion"
    );
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
    window.until("Space expands the focused personal header", |window| {
        window.shows(selector(format!("channel-row:{first}")))
    });
    window.press("enter");
    window.until("Enter collapses the focused personal header", |window| {
        !window.shows(selector(format!("channel-row:{first}")))
    });
    let root = admin.send(
        &world,
        second,
        crate::notices::mention("A plan for ", window.own_account()),
    );
    let unread = selector(format!("personal-unread:{id}"));
    window.until("hidden unread activity is counted", |window| {
        window.shows(unread)
    });
    window.close();
    let mut again = open(cx, directory.path());
    again.until_ready();
    assert!(section(&mut again, "Roadmap").unwrap().collapsed);
    assert_eq!(
        section(&mut again, "Roadmap")
            .unwrap()
            .rows
            .iter()
            .map(|row| row.id)
            .collect::<Vec<_>>(),
        [second, first]
    );
    again.right_click_named(head.to_owned());
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
    again.click(selector(format!("channel-row:{first}")));
    again.until("the personal channel is selected", |window| {
        window.selected_channel() == first
    });
    again.click(head);
    assert!(again.shows(selector(format!("channel-row:{first}"))));
    assert!(!again.shows(selector(format!("channel-row:{second}"))));
    again.click("new-personal-section");
    again.type_into("personal-section-name", "Reference");
    again.click("personal-section-save");
    again.until("the empty named section is listed", |window| {
        section(window, "Reference").is_some()
    });
    let other = section_id(&mut again, "Reference");
    again.click(selector(format!("personal-section-header:{other}")));
    again.press("menu");
    again.click("context-action:Move section up");
    again.until("personal sections have their own order", |window| {
        window.view().workspace().unwrap().sections[0].personal
            == Some(junto_client_core::arrangement::channels::Location::Section(
                other,
            ))
    });
    again.press("menu");
    again.click("context-action:Copy name");
    assert_eq!(
        again.cx.read_from_clipboard().unwrap().text().unwrap(),
        "Reference"
    );
    again.right_click_named(head.to_owned());
    again.click("context-action:Remove section");
    again.until("removing a section returns its channels", |window| {
        section(window, "Roadmap").is_none()
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
fn a_hidden_channel_cannot_remain_in_favourites_or_an_open_section_dialog(cx: &mut TestAppContext) {
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
    window.right_click_named(format!("channel-row:{channel}"));
    window.click("channel-new-section");
    window.type_into("personal-section-name", "Private");
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
        "permission loss closes the source dialog and hides the row",
        |window| {
            !window.shows("personal-section-dialog")
                && !window.shows_channel("planning")
                && !window.shows("personal-favourites-header")
        },
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
