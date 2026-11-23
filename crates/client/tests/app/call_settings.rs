//! "Room settings" on the call bar: the settings of the room of the call of this device in a
//! window attached to the bar, apart from the personal settings of Settings (ADR 0739).

use gpui::TestAppContext;
use junto_client::shell::{Open, SettingsSection};
use junto_client::state::view::{CallRoom, RoomView};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::{call::CallPlace, ids::AccountId};

use crate::harness::{Window, World, member};

/// The room controls of the window.
const ROOM_CONTROLS: [&str; 5] = [
    "room-meeting",
    "room-lock",
    "room-limit",
    "room-hide",
    "room-settings",
];

fn show_call(window: &mut Window, call: junto_media::calls::ActiveCall) {
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    calls.show_view_for_test(junto_media::calls::CallsView {
        call: Some(call),
        ..Default::default()
    });
    window.settle();
}

/// The room of the call of this device as the view derives it.
fn room(window: &mut Window) -> RoomView {
    match window.view().call_room {
        Some(CallRoom::Channel(room)) => room,
        other => panic!("no room of a voice channel: {other:?}"),
    }
}

/// Settings holds this device only: it has no section of the call, and outside a call no
/// window of the room exists.
#[gpui::test]
fn settings_have_no_call_section_and_outside_a_call_there_is_no_room_window(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    window.click("app-settings");
    assert!(
        SettingsSection::ALL
            .iter()
            .all(|section| section.selector() != "settings-section-call")
    );
    assert!(!window.shows("settings-section-call"));
    window.press("escape");
    assert!(!window.shows("room-settings-open"));
    window.in_shell(|shell, window, cx| shell.open(Open::RoomSettings, window, cx));
    window.settle();
    for control in ROOM_CONTROLS {
        assert!(!window.shows(control), "{control} outside a call");
    }
}

/// In the call of a voice channel the call bar has "Room settings", also while the window
/// shows another channel. Its window has the four settings of the room for everyone in the
/// call: a person without "manage channel" sees them and cannot change the ones that need
/// it; with it they change. A press on the button of the open window closes it, as Escape
/// does. Members lists people and no room control.
#[gpui::test]
fn the_call_bar_opens_the_room_settings_with_and_without_the_permission(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    crate::calls::voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    let call = crate::calls::connected_call(&mut window, "Daily");
    show_call(&mut window, call);
    window.until("the panel of the call", |window| {
        window.shows("call-tab-members")
    });
    window.click("call-tab-members");
    for control in ROOM_CONTROLS {
        assert!(!window.shows(control), "{control} in Members");
    }

    // The window shows another channel: the call bar still opens the room of the call.
    window.click_channel("general");
    window.click("room-settings-open");
    for control in ROOM_CONTROLS {
        assert!(window.shows(control), "{control} in the window");
    }
    let held = room(&mut window);
    assert_eq!(held.name, "Daily");
    assert!(held.ready && !held.allowed.manage);
    assert!(!held.locked && held.user_limit.is_none() && !held.hides_occupants);
    // Without the permission the switch of the hide setting takes no click.
    window.click("room-hide");
    assert!(!room(&mut window).hides_occupants);
    assert!(window.shows("room-settings"));
    // The press on the button of the open window closes it and does not open it again.
    window.click("room-settings-open");
    assert!(!window.shows("room-settings"));
    window.click("room-settings-open");
    assert!(window.shows("room-settings"));
    window.press("escape");
    assert!(!window.shows("room-settings"));

    // "manage channel" for everyone in Daily: the person may change the room.
    let workspace_id = window.view().workspace().unwrap().id;
    let daily = (window.channels().into_iter())
        .find(|row| row.name == "Daily")
        .unwrap()
        .id;
    let role = crate::channel_use::everyone(&world, &admin, workspace_id);
    crate::channel_use::rule(
        &world,
        &admin,
        daily,
        role,
        &[WorkspacePermission::ManageChannel],
        &[],
    );
    window.until("the room may be changed", |window| {
        room(window).allowed.manage
    });
    window.click("room-settings-open");
    for control in ROOM_CONTROLS {
        assert!(window.shows(control), "{control} with the permission");
    }
}

/// A direct-message call has no room (ADR 0501): the call bar has no "Room settings".
#[gpui::test]
fn a_direct_call_has_no_room_settings(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    crate::calls::voice_channel(&world, &admin, &mut window, "Daily");
    let mut call = crate::calls::connected_call(&mut window, "Daily");
    let (low, high) = (AccountId::generate(), AccountId::generate());
    call.target.place = CallPlace::Direct {
        low_account_id: low.min(high),
        high_account_id: low.max(high),
    };
    show_call(&mut window, call);
    assert_eq!(window.view().call_room, Some(CallRoom::Direct));
    assert!(!window.shows("room-settings-open"));
    window.in_shell(|shell, window, cx| shell.open(Open::RoomSettings, window, cx));
    window.settle();
    for control in ROOM_CONTROLS {
        assert!(!window.shows(control), "{control} in a direct call");
    }
}
