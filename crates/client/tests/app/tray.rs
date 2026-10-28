//! The tray icon of the call of this device and its menu, through the kept tray of a start
//! without the tray of the system (ADR 0759), without a media server.

use gpui::TestAppContext;
use junto_client::text::tray as text;
use junto_client::tray::{CallMarks, KeptTray, TrayState};
use junto_client_core::model::PlaceId;
use junto_client_core::protocol::call::CallPlace;
use junto_client_core::protocol::message::SendMessage;
use junto_client_core::protocol::structure::{ChannelKind, CreateChannel};
use junto_media::calls::{ActiveCall, CallTarget, CallsView, Phase};
use junto_media::engine::{MicrophoneState, OutputState};

use crate::calls::voice_channel;
use crate::harness::{Window, World, member};
use crate::messages::document;

/// A connected call of this device in the voice channel with this name.
fn call(window: &mut Window, channel: &str) -> ActiveCall {
    let view = window.view();
    let row = (window.channels().into_iter())
        .find(|row| row.name == channel)
        .unwrap();
    ActiveCall {
        camera: junto_media::camera::CameraState::Off,
        cameras: Vec::new(),
        target: CallTarget {
            server_id: view.selected.as_ref().unwrap().id,
            place: CallPlace::Channel {
                workspace_id: view.workspace().unwrap().id,
                channel_id: row.id,
            },
        },
        identity: "seat".to_owned(),
        phase: Phase::Connected,
        interrupted_at: None,
        microphone: MicrophoneState::Running,
        access: junto_media::permission::MicrophoneAccess::Allowed,
        input_device: None,
        output: OutputState::Running,
        output_device: None,
        microphone_block: None,
        sound_block: None,
        share_limit: junto_client_core::protocol::call::MAX_SHARES,
        own_shares: std::collections::BTreeMap::new(),
        share_ended: None,
        shares: Vec::new(),
        watched: None,
        withheld: Default::default(),
    }
}

fn tray(window: &mut Window) -> KeptTray {
    window
        .state
        .read_with(&window.cx, |state, _| state.kept_tray())
}

/// Presses the entry of the shown tray menu with this label, as a person does.
fn press(window: &mut Window, label: &str) {
    let tray = tray(window);
    let state = window.state.clone();
    let pressed = window.cx.update(|_, cx| tray.press(label, &state, cx));
    assert!(pressed, "the tray menu has no enabled entry {label:?}");
    window.settle();
}

fn shown(window: &mut Window) -> TrayState {
    tray(window).shown()
}

#[gpui::test]
fn the_tray_shows_the_call_and_its_menu_mutes_and_turns_the_sound_off(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, "Daily");
    window.click_channel("Daily");
    assert_eq!(shown(&mut window).call, None);

    // The calls state a call of this device, as a session does.
    let active = call(&mut window, "Daily");
    let seat = active.seat();
    let calls = window
        .state
        .read_with(&window.cx, |state, _| state.calls().clone());
    calls.show_view_for_test(CallsView {
        call: Some(active),
        ..CallsView::default()
    });
    window.settle();
    let in_call = TrayState {
        call: Some(CallMarks {
            muted: false,
            held: false,
            deafened: false,
            seat: seat.clone(),
        }),
        generation: 0,
        mentions: 0,
    };
    assert_eq!(shown(&mut window).call, in_call.call);
    assert_eq!(shown(&mut window).tooltip(), text::IN_A_CALL);

    // The entry "Mute microphone" closes the microphone of the call, as the button does.
    press(&mut window, text::MUTE_MICROPHONE);
    assert!(
        window
            .state
            .read_with(&window.cx, |state, _| state.calls().view().muted)
    );
    assert_eq!(shown(&mut window).call.map(|call| call.muted), Some(true));
    assert_eq!(shown(&mut window).tooltip(), text::IN_A_CALL_MUTED);
    assert!(
        window.view().controls.microphone == junto_client::state::view::MicrophoneButton::Muted
    );

    // The entry "Turn sound off" turns the sound of the call off.
    press(&mut window, text::TURN_SOUND_OFF);
    assert!(window.view().controls.sound_off);
    assert_eq!(shown(&mut window).tooltip(), text::IN_A_CALL_SOUND_OFF);
    let menu: Vec<&str> = shown(&mut window)
        .menu()
        .iter()
        .map(|entry| entry.label)
        .collect();
    assert_eq!(
        menu,
        [
            text::UNMUTE_MICROPHONE,
            text::TURN_SOUND_ON,
            text::LEAVE_THE_CALL,
            text::OPEN_JUNTO,
            text::QUIT
        ]
    );
    press(&mut window, text::TURN_SOUND_ON);
    assert!(!window.view().controls.sound_off);

    // Each state was told once, in its order.
    let told = tray(&mut window).told();
    let pictures: Vec<_> = told.iter().map(TrayState::tooltip).collect();
    assert_eq!(
        pictures,
        [
            text::NOT_IN_A_CALL,
            text::IN_A_CALL,
            text::IN_A_CALL_MUTED,
            text::IN_A_CALL_SOUND_OFF,
            text::IN_A_CALL_MUTED,
        ]
    );

    // The call ends: the icon shows no call, and the menu has no call entries.
    calls.show_view_for_test(CallsView::default());
    window.settle();
    assert_eq!(shown(&mut window).call, None);
    let tray = tray(&mut window);
    let state = window.state.clone();
    assert!(
        !window
            .cx
            .update(|_, cx| tray.press(text::LEAVE_THE_CALL, &state, cx))
    );
}

/// The messages for the person on the servers reach the tray, the count of the badge of the
/// application: the icon shows the mention mark while there is one, the tooltip states the
/// number, the menu stays the same menu, reading the server removes the mark, and a direct
/// message sets it again (ADR 0741, ADR 0759).
#[gpui::test]
fn the_unread_mentions_add_the_mark_and_keep_the_menu(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let before = shown(&mut window);
    assert!(!before.icon().mention);
    let workspace = window.view().workspace().unwrap().id;
    let plans = world
        .run(admin.handle.api().create_channel(
            workspace,
            &CreateChannel {
                name: "plans".to_owned(),
                kind: ChannelKind::Text,
                category_id: None,
                position: None,
            },
        ))
        .unwrap()
        .id;
    let luca = window.own_account();
    admin.mention(&world, plans, "Plans for the week, ", luca);
    window.until("the tray shows the unread mention", |window| {
        shown(window).icon().mention
    });
    let marked = shown(&mut window);
    assert_eq!(
        marked.tooltip(),
        format!(
            "{}, 1 unread mention or direct message",
            text::NOT_IN_A_CALL
        )
    );
    assert_eq!(marked.generation, before.generation);
    admin.mention(&world, plans, "And the agenda, ", luca);
    window.until("the tray counts both mentions", |window| {
        shown(window).tooltip()
            == format!(
                "{}, 2 unread mentions and direct messages",
                text::NOT_IN_A_CALL
            )
    });
    let server_id = window.view().selected.as_ref().unwrap().id;
    window.state.update(&mut window.cx, |state, _| {
        state.mark_server_read(server_id, None)
    });
    window.until(
        "the tray mark is gone after the server was read",
        |window| !shown(window).icon().mention,
    );
    let api = admin.handle.api();
    let conversation = world.run(api.open_conversation(luca)).unwrap();
    world
        .run(api.send_message(
            PlaceId::Direct(conversation.id),
            &SendMessage {
                document: document("Are you free at three?"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until("the tray shows the direct message", |window| {
        shown(window).tooltip()
            == format!(
                "{}, 1 unread mention or direct message",
                text::NOT_IN_A_CALL
            )
    });
    assert!(shown(&mut window).icon().mention);
    assert_eq!(window.badge(), Some(1));
}
