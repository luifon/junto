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
        share_limit: junto_client_core::protocol::call::MAX_SHARES,
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
    // The own setting is open: the person can still close it, for after the block. The
    // picture is the one of a microphone that the call holds closed.
    let open = derive(&calls(Some(active.clone()), false, false));
    assert_eq!(open.picture(), Picture::Held);
    assert_eq!(open.tooltip(), text::IN_A_CALL_HELD);
    assert_eq!(open.menu()[0].label, text::MUTE_MICROPHONE);
    assert!(open.menu()[0].enabled);
    let closed = derive(&calls(Some(active.clone()), true, false));
    assert_eq!(closed.picture(), Picture::Held);
    assert_eq!(closed.menu()[0].label, text::UNMUTE_MICROPHONE);
    assert!(!closed.menu()[0].enabled);
    // The sound off takes precedence over the held microphone.
    let deaf = derive(&calls(Some(active), true, true));
    assert_eq!(deaf.picture(), Picture::SoundOff);
}

#[test]
fn a_mention_adds_the_mark_to_every_picture_and_its_number_to_the_tooltip() {
    for (mut state, picture) in [
        (derive(&calls(None, false, false)), Picture::Plain),
        (derive(&calls(Some(call()), false, false)), Picture::Call),
        (derive(&calls(Some(call()), true, false)), Picture::Muted),
        (derive(&calls(Some(call()), true, true)), Picture::SoundOff),
    ] {
        assert_eq!(
            state.icon(),
            IconPicture {
                picture,
                mention: false
            }
        );
        let plain = state.tooltip();
        state.mentions = 3;
        assert_eq!(
            state.icon(),
            IconPicture {
                picture,
                mention: true
            }
        );
        assert_eq!(
            state.tooltip(),
            format!("{plain}, 3 unread mentions and direct messages")
        );
        state.mentions = 1;
        assert_eq!(
            state.tooltip(),
            format!("{plain}, 1 unread mention or direct message")
        );
    }
    assert_eq!(
        IconPicture {
            picture: Picture::Held,
            mention: true
        }
        .name(),
        "held-mention"
    );
}

#[test]
fn the_build_rendered_each_picture() {
    let pictures = [
        Picture::Plain,
        Picture::Call,
        Picture::Muted,
        Picture::Held,
        Picture::SoundOff,
    ];
    for picture in pictures {
        for mention in [false, true] {
            let shown = IconPicture { picture, mention };
            let renderings: Vec<_> = renderings(shown).collect();
            // macOS: the template picture at 1x and at 2x; elsewhere every side of the
            // notification area of Windows and of the status area of Linux.
            let sides: Vec<u32> = renderings.iter().map(|(side, _)| *side).collect();
            if cfg!(target_os = "macos") {
                assert_eq!(sides, [18, 36], "{shown:?}");
            } else {
                assert_eq!(sides, [16, 20, 22, 24, 28, 32, 36, 40, 48, 64], "{shown:?}");
            }
            for (side, bytes) in renderings {
                if cfg!(target_os = "macos") {
                    assert!(bytes.starts_with(b"\x89PNG"), "{shown:?} {side}");
                } else {
                    assert_eq!(bytes.len(), (side * side * 4) as usize, "{shown:?} {side}");
                }
            }
        }
    }
    // Each picture differs from every other one, also from the same picture with the mark.
    let mut all: Vec<&[u8]> = Vec::new();
    for picture in pictures {
        for mention in [false, true] {
            let (_, bytes) = renderings(IconPicture { picture, mention })
                .last()
                .unwrap_or_default();
            assert!(!all.contains(&bytes), "{picture:?} {mention}");
            all.push(bytes);
        }
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
