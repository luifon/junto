use junto_client_core::protocol::call::{BlockReason, CallPlace, MicrophoneBlock};
use junto_client_core::protocol::ids::{AccountId, ChannelId, ServerId, WorkspaceId};
use junto_media::calls::{ActiveCall, CallTarget, CallsView, Phase};
use junto_media::engine::{MicrophoneState, OutputState};

use super::*;

fn call() -> ActiveCall {
    ActiveCall {
        camera: junto_media::camera::CameraState::Off,
        cameras: Vec::new(),
        target: CallTarget {
            server_id: ServerId::generate(),
            place: CallPlace::Channel {
                workspace_id: WorkspaceId::generate(),
                channel_id: ChannelId::generate(),
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
        own_shares: std::collections::BTreeMap::new(),
        share_ended: None,
        shares: Vec::new(),
        watched: None,
    }
}

fn call_at(identity: &str) -> ActiveCall {
    ActiveCall {
        identity: identity.to_owned(),
        ..call()
    }
}

fn calls(call: Option<ActiveCall>, muted: bool, deafened: bool) -> CallsView {
    CallsView {
        call,
        muted,
        deafened,
        ..CallsView::default()
    }
}

fn labels(state: &TrayState) -> Vec<&'static str> {
    state.menu().iter().map(|entry| entry.label).collect()
}

#[test]
fn outside_a_call_the_icon_is_plain_and_the_menu_opens_and_quits() {
    // A setting of the microphone and the sound holds outside a call too; the icon shows
    // only the call.
    let state = derive(&calls(None, true, true));
    assert_eq!(state.call, None);
    assert_eq!(state.picture(), Picture::Plain);
    assert_eq!(state.tooltip(), text::NOT_IN_A_CALL);
    assert_eq!(labels(&state), [text::OPEN_JUNTO, text::QUIT]);
    assert!(
        state
            .menu()
            .iter()
            .all(|entry| entry.enabled && !entry.separated)
    );
}

#[test]
fn in_a_call_the_menu_offers_what_the_controls_do() {
    let active = call();
    let seat = active.seat();
    let state = derive(&calls(Some(active), false, false));
    assert_eq!(state.picture(), Picture::Call);
    assert_eq!(state.tooltip(), text::IN_A_CALL);
    assert_eq!(
        labels(&state),
        [
            text::MUTE_MICROPHONE,
            text::TURN_SOUND_OFF,
            text::LEAVE_THE_CALL,
            text::OPEN_JUNTO,
            text::QUIT
        ]
    );
    let menu = state.menu();
    assert_eq!(
        menu[0].action,
        TrayAction::Microphone(MicrophoneAction::Close)
    );
    assert_eq!(menu[1].action, TrayAction::Sound(true));
    assert_eq!(menu[2].action, TrayAction::Leave);
    assert!(menu[3].separated);
    assert_eq!(state.call.as_ref().map(|call| &call.seat), Some(&seat));
}

#[test]
fn a_muted_microphone_offers_to_open_it_at_the_seat_of_the_call() {
    let active = call();
    let seat = active.seat();
    let state = derive(&calls(Some(active), true, false));
    assert_eq!(state.picture(), Picture::Muted);
    assert_eq!(state.tooltip(), text::IN_A_CALL_MUTED);
    let menu = state.menu();
    assert_eq!(menu[0].label, text::UNMUTE_MICROPHONE);
    assert_eq!(
        menu[0].action,
        TrayAction::Microphone(MicrophoneAction::Open(Some(seat)))
    );
    assert!(menu[0].enabled);
    assert_eq!(menu[1].label, text::TURN_SOUND_OFF);
}

#[test]
fn sound_off_shows_its_own_picture_and_offers_the_sound_again() {
    let state = derive(&calls(Some(call()), true, true));
    assert_eq!(state.picture(), Picture::SoundOff);
    assert_eq!(state.tooltip(), text::IN_A_CALL_SOUND_OFF);
    let menu = state.menu();
    assert_eq!(menu[1].label, text::TURN_SOUND_ON);
    assert_eq!(menu[1].action, TrayAction::Sound(false));
}

#[test]
fn a_microphone_that_the_server_holds_shows_muted_and_cannot_be_opened() {
    let mut active = call();
    active.microphone_block = Some(MicrophoneBlock {
        by: AccountId::generate(),
        reason: BlockReason::Moderator,
    });
    // The own setting is open: the person can still close it, for after the block.
    let open = derive(&calls(Some(active.clone()), false, false));
    assert_eq!(open.picture(), Picture::Muted);
    assert_eq!(open.menu()[0].label, text::MUTE_MICROPHONE);
    assert!(open.menu()[0].enabled);
    let closed = derive(&calls(Some(active), true, false));
    assert_eq!(closed.menu()[0].label, text::UNMUTE_MICROPHONE);
    assert!(!closed.menu()[0].enabled);
}

#[test]
fn the_build_rendered_each_picture() {
    for picture in [
        Picture::Plain,
        Picture::Call,
        Picture::Muted,
        Picture::SoundOff,
    ] {
        let renderings: Vec<_> = renderings(picture).collect();
        assert!(!renderings.is_empty(), "{picture:?}");
        for (side, bytes) in renderings {
            if cfg!(target_os = "macos") {
                assert!(bytes.starts_with(b"\x89PNG"), "{picture:?} {side}");
            } else {
                assert_eq!(
                    bytes.len(),
                    (side * side * 4) as usize,
                    "{picture:?} {side}"
                );
            }
        }
    }
    // The marks differ from each other and from the plain icon at the corner of the mark.
    #[cfg(not(target_os = "macos"))]
    {
        let corner = |picture| {
            let (side, bytes) = renderings(picture)
                .find(|(side, _)| *side == 32)
                .unwrap_or_default();
            let at = ((side * 3 / 4) * side + side * 3 / 4) as usize * 4;
            bytes.get(at..at + 4).map(<[u8]>::to_vec)
        };
        let corners = [
            corner(Picture::Plain),
            corner(Picture::Call),
            corner(Picture::Muted),
        ];
        assert!(corners.iter().all(Option::is_some));
        assert_ne!(corners[0], corners[1]);
        assert_ne!(corners[1], corners[2]);
    }
}

/// A menu that was drawn before a change of the call names entries of an earlier generation:
/// a press on it is refused, so "Mute" does not become "Unmute" and an entry of one call does
/// not act on another (ADR 0788).
#[test]
fn a_press_on_a_menu_of_an_earlier_state_is_refused() {
    let mut first = derive(&calls(Some(call_at("first")), false, false));
    first.generation = 4;
    let mute = first
        .menu()
        .into_iter()
        .find(|entry| entry.label == text::MUTE_MICROPHONE)
        .unwrap();
    assert_eq!(
        first.action(mute.code),
        Some(TrayAction::Microphone(MicrophoneAction::Close))
    );

    // The microphone was closed meanwhile: the old "Mute" does not open it.
    let mut muted = derive(&calls(Some(call_at("first")), true, false));
    muted.generation = 5;
    assert_eq!(muted.action(mute.code), None);

    // The person was moved to another call: the old "Unmute" does not open its microphone.
    let unmute = muted
        .menu()
        .into_iter()
        .find(|entry| entry.label == text::UNMUTE_MICROPHONE)
        .unwrap();
    let mut moved = derive(&calls(Some(call_at("second")), true, false));
    moved.generation = 6;
    assert_eq!(moved.action(unmute.code), None);
    assert!(moved.menu().iter().all(|entry| entry.code != unmute.code));
}
