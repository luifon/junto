use gpui::TestAppContext;
use junto_client::shell::Pane;
use junto_client_core::api::Statement;
use junto_client_core::model::PlaceId;

use crate::harness::{World, member};
use crate::messages::document;

#[gpui::test]
fn a_late_notice_keeps_a_scrolled_message_in_place(cx: &mut TestAppContext) {
    moderation_notice_keeps_controls(cx, false);
}

#[gpui::test]
fn a_late_notice_keeps_the_direct_call_control_in_place(cx: &mut TestAppContext) {
    moderation_notice_keeps_controls(cx, true);
}

fn moderation_notice_keeps_controls(cx: &mut TestAppContext, direct: bool) {
    let world = World::new();
    let server = world.server("Layout");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let api = admin.handle.api();
    let own = window.own_account();
    let other = world.run(api.create_workspace("Other".to_owned())).unwrap();
    world.run(api.add_workspace_member(other.id, own)).unwrap();
    window.until("both workspaces are shown", |window| {
        window.view().servers[0].workspaces.len() == 2
    });
    let selector = if direct {
        window.state.update(&mut window.cx, |state, cx| {
            state.open_conversation(admin.handle.id(), admin.handle.account().unwrap(), cx);
        });
        window.until("the direct call control is drawn", |window| {
            window.shows("start-direct-call")
        });
        "start-direct-call".to_owned()
    } else {
        let channel = window.selected_channel();
        let messages: Vec<_> = (0..60)
            .map(|index| admin.send(&world, channel, document(&format!("Message {index}"))))
            .collect();
        window.until_message("Message 59");
        let target = messages[10].id;
        window.in_shell(|shell, _, cx| shell.show_feed_row(Pane::Main, 0, cx));
        // A real scroll also records its anchor for a later snapshot reload.
        let server_id = admin.handle.id();
        let place = PlaceId::Channel(channel);
        let anchor = messages[0].position;
        window.state.update(&mut window.cx, |state, _| {
            state.set_anchor(server_id, place, Some(anchor));
        });
        window.until("the scrolled message anchor is recorded", |window| {
            window.state.read_with(&window.cx, |state, _| {
                state
                    .server(server_id)
                    .unwrap()
                    .live()
                    .view()
                    .model
                    .as_ref()
                    .and_then(|model| model.timeline(place))
                    .is_some_and(|timeline| timeline.anchor == Some(anchor))
            })
        });
        let selector = format!("message:{target}");
        window.until("the target message is drawn", |window| {
            window.shows_named(selector.clone())
        });
        selector
    };
    assert!(!window.shows("moderation-notice"));
    let viewport = (!direct).then(|| window.cx.debug_bounds("message-list").unwrap());
    let before = window
        .cx
        .debug_bounds(crate::harness::selector(selector.clone()))
        .unwrap();
    world
        .run(api.remove_workspace_member(other.id, own, &Statement::default()))
        .unwrap();
    window.until("the late notice is drawn", |window| {
        window.shows("moderation-notice")
    });
    assert_eq!(
        window
            .cx
            .debug_bounds(crate::harness::selector(selector.clone()))
            .unwrap(),
        before
    );
    if let Some(viewport) = viewport {
        assert_eq!(window.cx.debug_bounds("message-list").unwrap(), viewport);
    }
}
