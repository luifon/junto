use gpui::{TestAppContext, px, size};

use crate::admin_pages::{invite, open_channel_settings, owner_window};
use crate::harness::{World, named};

#[gpui::test]
fn a_manager_edits_and_clears_a_topic_and_a_member_reads_and_copies_it(
    cx: &mut TestAppContext,
    member_cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Example Studio");
    let (_directory, mut window, owner) = owner_window(cx, &server);
    let code = invite(&world, &owner, Vec::new());
    let (_member_directory, mut member) = named(member_cx, "Rin");
    member.find_server(&server.address());
    member.join_with_code(code.expose());
    member.until_ready();
    let channel = window.selected_channel();
    let name = window.channel_names()[0].clone();
    open_channel_settings(&mut window, &name, channel);
    window.click("channel-topic-edit");
    let topic = format!("Plans\nand links {}", "é".repeat(80));
    window.type_into("channel-topic-input", &topic);
    window.click("admin-form-send");
    window.until("the topic form is saved", |window| {
        !window.shows("channel-topic-input")
    });
    window.press("escape");
    window.until("the overview is closed", |window| window.admin().is_none());
    member.until("the member header follows the new topic", |window| {
        window.shows("channel-topic")
    });
    member.simulate(|cx| cx.simulate_resize(size(px(720.0), px(600.0))));
    member.settle();
    let bounds = member.cx.debug_bounds("channel-topic").unwrap();
    assert!(bounds.right() <= px(720.0));
    member.right_click_named("channel-topic".to_owned());
    assert!(!member.shows("context-action:Edit topic"));
    member.click("context-action:Copy topic");
    assert_eq!(
        member.cx.read_from_clipboard().unwrap().text().unwrap(),
        topic
    );
    window.right_click_named("channel-topic".to_owned());
    window.click("context-action:Edit topic");
    window.until("the topic is editable from the header", |window| {
        window.shows("channel-topic-input")
    });
    window.click("channel-topic-input");
    window.press("secondary-a backspace");
    window.click("admin-form-send");
    member.until("an empty topic leaves the header", |window| {
        !window.shows("channel-topic")
    });
}
