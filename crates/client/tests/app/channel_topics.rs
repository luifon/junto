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

#[gpui::test]
fn older_servers_do_not_offer_topic_editing_and_refuse_direct_topic_writes(
    cx: &mut TestAppContext,
) {
    use crate::support::FaultProxy;
    use junto_client_core::protocol::structure::UpdateChannel;
    let world = World::new();
    let server = world.server("Example Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) =
        crate::admin_pages::owner_window_at(cx, &server, &proxy.address());
    proxy.answer_json("GET", "/api/server", serde_json::to_vec(&serde_json::json!({
        "server_id": owner.id(), "name": "Example Studio", "logo_path": null, "api_versions": [1]
    })).unwrap());
    world.run(owner.connect()).unwrap();
    window.settle();
    let channel = window.selected_channel();
    let name = window.channel_names()[0].clone();
    open_channel_settings(&mut window, &name, channel);
    assert!(window.shows("channel-rename"));
    assert!(!window.shows("channel-topic-edit"));
    let server_id = window.handle().id();
    let before = proxy
        .seen(&format!("/api/v1/channels/{channel}"))
        .iter()
        .filter(|request| request.method == "PATCH")
        .count();
    window.state.update(&mut window.cx, |state, cx| {
        state.update_channel(
            server_id,
            channel,
            UpdateChannel {
                topic: Some("Lost topic".into()),
                ..Default::default()
            },
            cx,
        )
    });
    window.settle();
    assert_eq!(
        proxy
            .seen(&format!("/api/v1/channels/{channel}"))
            .iter()
            .filter(|request| request.method == "PATCH")
            .count(),
        before
    );
    assert_eq!(
        window
            .admin()
            .unwrap()
            .refused(junto_client::state::admin::AskKind::UpdateChannel(channel)),
        Some("This server does not support channel topics. Upgrade the server to edit a topic.")
    );
}

#[gpui::test]
fn a_success_response_without_a_topic_keeps_the_editor_open_and_states_the_refusal(
    cx: &mut TestAppContext,
) {
    use crate::support::FaultProxy;
    let world = World::new();
    let server = world.server("Example Studio");
    let proxy = world.run(FaultProxy::start(&server));
    let (_directory, mut window, owner) =
        crate::admin_pages::owner_window_at(cx, &server, &proxy.address());
    let channel = window.selected_channel();
    let mut old = serde_json::to_value(world.run(owner.api().channel(channel)).unwrap()).unwrap();
    old.as_object_mut().unwrap().remove("topic");
    proxy.answer_json(
        "PATCH",
        &format!("/channels/{channel}"),
        serde_json::to_vec(&old).unwrap(),
    );
    let name = window.channel_names()[0].clone();
    open_channel_settings(&mut window, &name, channel);
    window.click("channel-topic-edit");
    window.type_into("channel-topic-input", "A topic the old server discards");
    window.click("admin-form-send");
    window.until("the unsupported topic response is refused", |window| {
        window.shows("channel-topic-refused")
    });
    assert!(window.shows("channel-topic-input"));
    assert_eq!(
        window
            .admin()
            .unwrap()
            .refused(junto_client::state::admin::AskKind::UpdateChannel(channel)),
        Some("This server does not support channel topics. Upgrade the server to edit a topic.")
    );
    assert!(
        proxy
            .seen(&format!("/api/v1/channels/{channel}"))
            .iter()
            .any(|request| request.method == "PATCH"
                && request
                    .answer
                    .as_ref()
                    .is_some_and(|(status, _)| *status == 200))
    );
}
