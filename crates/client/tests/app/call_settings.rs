//! The settings section "Call": the settings of the room of the call of this device, apart
//! from the personal settings at "Voice and video" (ADR 0735, ADR 0739).

use gpui::TestAppContext;
use junto_client::shell::{Open, SettingsSection};
use junto_client::state::view::{CallRoom, RoomView};
use junto_client_core::protocol::WorkspacePermission;
use junto_client_core::protocol::{call::CallPlace, ids::AccountId};

use crate::harness::{Window, World, member};

/// The room controls of the section; Members has none of them.
const ROOM_CONTROLS: [&str; 5] = [
    "room-meeting",
    "room-lock",
    "room-limit",
    "room-hide",
    "room-settings",
];

fn page(section: SettingsSection) -> String {
    format!("settings-page-{}", section.selector())
}

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

fn open_call_section(window: &mut Window) {
    window.in_shell(|shell, window, cx| shell.open(Open::RoomSettings, window, cx));
    window.settle();
    assert!(
        window.shows_named(page(SettingsSection::Call)),
        "Open::RoomSettings opens the settings at Call"
    );
}

/// The room of the call of this device as the view derives it.
fn room(window: &mut Window) -> RoomView {
    match window.view().call_room {
        Some(CallRoom::Channel(room)) => room,
        other => panic!("no room of a voice channel: {other:?}"),
    }
}

/// Outside a call the section states what it holds and has no control.
#[gpui::test]
fn outside_a_call_the_call_section_states_it_and_has_no_control(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Harbor");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Reader");
    window.click("app-settings");
    window.click("settings-section-call");
    assert!(window.shows_named(page(SettingsSection::Call)));
    assert!(window.shows("call-section-statement:no-call"));
    for control in ROOM_CONTROLS {
        assert!(!window.shows(control), "{control} outside a call");
    }
}

/// In the call of a voice channel the section names the channel of the call and has the room
/// controls, also while the window shows another channel. A person without "manage channel"
/// sees their state and the statement of the missing permission; with it the statement goes.
/// Members lists people and no room control.
#[gpui::test]
fn in_a_call_the_call_section_has_the_room_controls_with_and_without_the_permission(
    cx: &mut TestAppContext,
) {
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

    // The window shows another channel: the section holds the room of the call.
    window.click_channel("general");
    open_call_section(&mut window);
    window.until(
        "the section names the voice channel of the call",
        |window| window.shows_named("call-section-channel:Daily".to_owned()),
    );
    for control in ROOM_CONTROLS {
        assert!(window.shows(control), "{control} in the section");
    }
    assert!(window.shows("room-no-manage"));
    let held = room(&mut window);
    assert_eq!(held.name, "Daily");
    assert!(held.ready && !held.allowed.manage);
    assert!(!held.locked && held.user_limit.is_none() && !held.hides_occupants);

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
        !window.shows("room-no-manage")
    });
    assert!(room(&mut window).allowed.manage);
    for control in ROOM_CONTROLS {
        assert!(window.shows(control), "{control} with the permission");
    }
}

/// A direct-message call has no room (ADR 0501): the section states it.
#[gpui::test]
fn in_a_direct_call_the_call_section_states_that_it_has_no_room(cx: &mut TestAppContext) {
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
    open_call_section(&mut window);
    assert_eq!(window.view().call_room, Some(CallRoom::Direct));
    assert!(window.shows("call-section-statement:no-room"));
    for control in ROOM_CONTROLS {
        assert!(!window.shows(control), "{control} in a direct call");
    }
}
