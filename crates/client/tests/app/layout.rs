use gpui::{TestAppContext, px};
use junto_client::shell::Pane;
use junto_client::state::feed::FeedRow;
use junto_client_core::api::Statement;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::structure::RuleSubject;
use junto_client_core::protocol::structure::SetChannelOverride;

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

/// A system line longer than the message list is wide wraps inside the list, at every width
/// of the window. The day separator spans the list.
#[gpui::test]
fn a_long_system_line_wraps_inside_the_message_list(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Layout");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let api = admin.handle.api();
    let name = "Administrator of the north recording studio";
    world.run(api.update_profile(name)).unwrap();
    let channel = window.selected_channel();
    let workspace_id = window.view().workspace().unwrap().id;
    let roles = world.run(api.workspace_roles(workspace_id)).unwrap();
    let everyone = roles.iter().find(|role| role.is_everyone).unwrap().id;
    let change = SetChannelOverride {
        allow: Vec::new(),
        deny: vec![WorkspacePermission::MentionEveryone],
    };
    world
        .run(api.set_channel_rule(channel, RuleSubject::Role { role_id: everyone }, &change))
        .unwrap();
    let sentence = format!("{name} changed the permissions of this channel");
    let mut line = None;
    window.until("the line names the administrator", |window| {
        line = window.feed().and_then(|feed| {
            feed.rows.iter().find_map(|row| match row {
                FeedRow::System(row)
                    if row
                        .parts
                        .iter()
                        .map(|part| part.text.as_str())
                        .collect::<String>()
                        == sentence =>
                {
                    Some(row.message.id)
                }
                _ => None,
            })
        });
        line.is_some()
    });
    let text = crate::harness::selector(format!("system-text:{}", line.unwrap()));
    let mut heights = Vec::new();
    for width in [720.0, 1000.0, 1600.0] {
        window.simulate(|cx| cx.simulate_resize(gpui::size(px(width), px(600.0))));
        window.settle();
        let list = window.cx.debug_bounds("message-list").unwrap();
        let drawn = window.cx.debug_bounds(text).unwrap();
        assert!(
            drawn.left() >= list.left() && drawn.right() <= list.right(),
            "at a width of {width}, the line {drawn:?} passes the edge of the list {list:?}"
        );
        heights.push(drawn.size.height);
        // The day separator spans the list: its rules fill the width beside the date.
        let day = window.cx.debug_bounds("day-separator").unwrap();
        assert_eq!(
            (day.left(), day.size.width),
            (list.left(), list.size.width),
            "at a width of {width}"
        );
    }
    assert!(
        heights[0] > heights[2],
        "the line does not wrap in the narrow window: {heights:?}"
    );
}
