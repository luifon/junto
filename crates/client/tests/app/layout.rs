use gpui::{TestAppContext, px};
use junto_client::shell::Pane;
use junto_client::state::feed::FeedRow;
use junto_client_core::api::Statement;
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::WorkspacePermission;
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
    // The person points at the control of the active window when the notice arrives.
    window.cx.update(|window, _| window.activate_window());
    window.move_pointer(before.center());
    world
        .run(api.remove_workspace_member(other.id, own, &Statement::default()))
        .unwrap();
    window.until("the late notice is drawn", |window| {
        window.shows("moderation-notice")
    });
    window.settle();
    assert_eq!(
        window
            .cx
            .debug_bounds(crate::harness::selector(selector.clone()))
            .unwrap(),
        before
    );
    if let Some(viewport) = viewport {
        assert_eq!(window.cx.debug_bounds("message-list").unwrap(), viewport);
        // The pointer leaves the list: the notice takes its place above the list, and the
        // area has no empty place without it (ADR 0730).
        window.move_pointer(gpui::point(px(4.0), px(4.0)));
        let moved = window.cx.debug_bounds("message-list").unwrap();
        let notice = window.cx.debug_bounds("moderation-notice").unwrap();
        assert!(
            moved.top() >= notice.bottom(),
            "the list {moved:?} starts above the end of the notice {notice:?}"
        );
        assert!(moved.top() > viewport.top());
    }
}

/// A window with a channel of 30 messages, the pointer resting on its message list, and a
/// late moderation notice that waits for the pointer above the list.
fn notice_under_a_resting_pointer(
    cx: &mut TestAppContext,
    active: bool,
) -> (
    World,
    impl Sized,
    crate::harness::Window,
    gpui::Bounds<gpui::Pixels>,
) {
    let world = World::new();
    let server = world.server("Layout");
    let admin = world.owner(&server);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let api = admin.handle.api();
    let own = window.own_account();
    let other = world.run(api.create_workspace("Other".to_owned())).unwrap();
    world.run(api.add_workspace_member(other.id, own)).unwrap();
    window.until("both workspaces are shown", |window| {
        window.view().servers[0].workspaces.len() == 2
    });
    let channel = window.selected_channel();
    for index in 0..30 {
        admin.send(&world, channel, document(&format!("Message {index}")));
    }
    window.until_message("Message 29");
    let viewport = window.cx.debug_bounds("message-list").unwrap();
    // A change waits for the pointer only in the active window (review M1).
    if active {
        window.cx.update(|window, _| window.activate_window());
    } else {
        window.simulate(|cx| cx.deactivate_window());
    }
    window.move_pointer(viewport.center());
    world
        .run(api.remove_workspace_member(other.id, own, &Statement::default()))
        .unwrap();
    window.until("the late notice is drawn", |window| {
        window.shows("moderation-notice")
    });
    window.settle();
    (world, directory, window, viewport)
}

/// A late notice waits for the pointer at most a bound (review M1): with the pointer resting
/// on the message list, the notice takes its place above the list after the bound. A press
/// at once after that move is refused, and the same press later goes to its control.
#[gpui::test]
fn a_late_notice_takes_its_place_after_a_bound_while_the_pointer_rests(cx: &mut TestAppContext) {
    let (_world, _directory, mut window, viewport) = notice_under_a_resting_pointer(cx, true);
    assert_eq!(
        window.cx.debug_bounds("message-list").unwrap(),
        viewport,
        "the notice waits while the pointer rests on the list"
    );
    window
        .cx
        .executor()
        .advance_clock(junto_client::ui::WAIT_BOUND + std::time::Duration::from_millis(50));
    window.settle();
    let moved = window.cx.debug_bounds("message-list").unwrap();
    let notice = window.cx.debug_bounds("moderation-notice").unwrap();
    assert!(
        moved.top() >= notice.bottom() && moved.top() > viewport.top(),
        "after the bound the notice {notice:?} takes its place above the list {moved:?}"
    );
    let seen = window.cx.debug_bounds("moderation-seen").unwrap().center();
    window.press_at(seen, gpui::MouseButton::Left);
    window.settle();
    assert!(
        window.shows("moderation-notice"),
        "a press at once after the move reached the control that moved under the pointer"
    );
    window
        .cx
        .executor()
        .advance_clock(junto_client::ui::MOVED_GUARD);
    window.press_at(seen, gpui::MouseButton::Left);
    window.until("the notice is put away", |window| {
        !window.shows("moderation-notice")
    });
}

/// A window that is not active shows a late notice at once: its last pointer position is not
/// a person pointing at the list (review M1).
#[gpui::test]
fn a_window_that_is_not_active_shows_a_late_notice_at_once(cx: &mut TestAppContext) {
    let (_world, _directory, mut window, viewport) = notice_under_a_resting_pointer(cx, false);
    let moved = window.cx.debug_bounds("message-list").unwrap();
    let notice = window.cx.debug_bounds("moderation-notice").unwrap();
    assert!(
        moved.top() >= notice.bottom() && moved.top() > viewport.top(),
        "the notice {notice:?} takes its place above the list {moved:?} at once"
    );
}

/// Changes of the state derive the view of the window when the window draws, not at each
/// change: each answer of a profile derived the whole view, which with 601 members and 2,001
/// accounts held the window thread for seconds (review M8). The test platform draws a window
/// after each round of work, so the test turns drawing off: 50 changes then derive nothing
/// until the next drawing.
#[gpui::test]
fn changes_of_the_state_derive_the_view_when_the_window_draws(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Layout");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    window.settle();
    let before = window.in_shell(|shell, _, _| shell.derivations());
    cx.skip_drawing();
    for _ in 0..50 {
        window.state.update(&mut window.cx, |_, cx| cx.notify());
    }
    let derived = window.in_shell(|shell, _, _| shell.derivations()) - before;
    assert!(
        derived <= 1,
        "50 changes without a drawing derived the view {derived} times"
    );
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
        .run(api.set_channel_override(channel, everyone, &change))
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

/// A channel whose messages do not fill the list shows them from the top of the list, under
/// the title of the channel, and not at its end with an empty band above them (finding 8 of
/// the second live test). A list that its messages fill still stands at its end.
#[gpui::test]
fn a_short_message_list_starts_at_its_top(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Layout");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let channel = window.selected_channel();
    admin.send(&world, channel, document("First line"));
    let last = admin.send(&world, channel, document("Second line"));
    window.until_message("Second line");
    window.settle();
    let list = window.cx.debug_bounds("message-list").unwrap();
    let start = window.cx.debug_bounds("history-start").unwrap();
    assert!(
        start.top() - list.top() < px(40.0),
        "the first row starts {:?} under the top of the list",
        start.top() - list.top()
    );
    let second = window
        .cx
        .debug_bounds(crate::harness::selector(format!("message:{}", last.id)))
        .unwrap();
    assert!(
        list.bottom() - second.bottom() > px(200.0),
        "the rows stand at the end of the list"
    );

    let messages: Vec<_> = (0..40)
        .map(|index| admin.send(&world, channel, document(&format!("Message {index}"))))
        .collect();
    window.until_message("Message 39");
    window.settle();
    let newest = window
        .cx
        .debug_bounds(crate::harness::selector(format!(
            "message:{}",
            messages[39].id
        )))
        .unwrap();
    assert!(
        list.bottom() - newest.bottom() < px(40.0),
        "a full list stands at its end"
    );
}
