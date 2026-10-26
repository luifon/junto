//! Which short tones a change of the call of this device plays (ADR 0707).

use std::collections::BTreeMap;

use junto_client_core::protocol::call::CallParticipant;
use junto_client_core::protocol::ids::AccountId;
use junto_media::audio::DeviceId;
use junto_media::audio::sounds::Sound;
use junto_media::calls::{ActiveCall, CallsView, Seat};

/// What the sounds of a call are decided from: the call of this device, the person's own
/// settings, and the people whom the server lists in that call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Heard {
    /// The seat of this device. Absent outside a call.
    seat: Option<Seat>,
    muted: bool,
    deafened: bool,
    /// The people whom the server lists in the call, each with whether the person shares.
    /// Absent until the server lists the person of this device, so that the people who
    /// were in the call before the person are not heard as people who join.
    present: Option<BTreeMap<AccountId, bool>>,
    /// The output device of the call, on which its sounds play.
    pub(crate) output: Option<DeviceId>,
}

/// What a change of the calls is decided from. `participants` are the people whom the
/// server lists at the place of the call, and `own` is the account of this device on that
/// server.
pub(crate) fn heard(
    calls: &CallsView,
    participants: Option<&[CallParticipant]>,
    own: Option<AccountId>,
) -> Heard {
    let call = calls.call.as_ref();
    let present = call
        .and(participants)
        .zip(own)
        .filter(|(participants, own)| participants.iter().any(|p| p.account_id == *own))
        .map(|(participants, _)| {
            participants
                .iter()
                .map(|p| (p.account_id, p.screen_share.is_some()))
                .collect()
        });
    Heard {
        seat: call.map(ActiveCall::seat),
        muted: calls.muted,
        deafened: calls.deafened,
        present,
        output: call
            .and_then(|call| call.output_device.as_ref())
            .map(|device| device.id.clone()),
    }
}

/// The sounds of the change from `before` to `after`, in the order in which they play.
/// Only the person in the call hears them, and none when the person turned them off.
/// While the sound of the person is off, only the sound that turns it off plays.
pub(crate) fn sounds(before: &Heard, after: &Heard, on: bool) -> Vec<Sound> {
    if !on {
        return Vec::new();
    }
    match (&before.seat, &after.seat) {
        (Some(_), None) if !after.deafened => return vec![Sound::CallLeft],
        (Some(before), Some(after)) if before == after => {}
        // No call, a call that begins, or a move to another call.
        _ => return Vec::new(),
    }
    let mut sounds = Vec::new();
    if before.deafened != after.deafened {
        sounds.push(if after.deafened {
            Sound::SoundOff
        } else {
            Sound::SoundOn
        });
    }
    if after.deafened {
        return sounds;
    }
    if before.muted != after.muted {
        sounds.push(if after.muted {
            Sound::MicrophoneOff
        } else {
            Sound::MicrophoneOn
        });
    }
    let (Some(listed_before), Some(listed_after)) = (&before.present, &after.present) else {
        return sounds;
    };
    if listed_after
        .keys()
        .any(|id| !listed_before.contains_key(id))
    {
        sounds.push(Sound::PersonJoined);
    }
    if listed_before
        .keys()
        .any(|id| !listed_after.contains_key(id))
    {
        sounds.push(Sound::PersonLeft);
    }
    if listed_after
        .iter()
        .any(|(id, shares)| *shares && listed_before.get(id) != Some(&true))
    {
        sounds.push(Sound::ShareStarted);
    }
    sounds
}

#[cfg(test)]
mod tests {
    use super::*;
    use junto_client_core::protocol::call::{CallPlace, ScreenShare, ShareAudio};
    use junto_client_core::protocol::ids::{ChannelId, ServerId, WorkspaceId};
    use junto_media::calls::{CallTarget, Phase};
    use junto_media::engine::{MicrophoneState, OutputState};
    use junto_media::permission::MicrophoneAccess;

    struct Scene {
        own: AccountId,
        target: CallTarget,
    }

    impl Scene {
        fn new() -> Self {
            Self {
                own: AccountId::generate(),
                target: CallTarget {
                    server_id: ServerId::generate(),
                    place: CallPlace::Channel {
                        workspace_id: WorkspaceId::generate(),
                        channel_id: ChannelId::generate(),
                    },
                },
            }
        }

        fn calls(&self, identity: &str) -> CallsView {
            CallsView {
                call: Some(ActiveCall {
                    camera: junto_media::camera::CameraState::Off,
                    cameras: Vec::new(),
                    target: self.target,
                    identity: identity.to_owned(),
                    phase: Phase::Connected,
                    interrupted_at: None,
                    microphone: MicrophoneState::Running,
                    access: MicrophoneAccess::Allowed,
                    input_device: None,
                    output: OutputState::Running,
                    output_device: None,
                    microphone_block: None,
                    sound_block: None,
                    share_limit: junto_client_core::protocol::call::MAX_SHARES,
                    own_shares: BTreeMap::new(),
                    share_ended: None,
                    shares: Vec::new(),
                    watched: None,
                }),
                ..CallsView::default()
            }
        }

        /// The state in the call `seat`, with the server listing `listed`, of whom
        /// `sharing` share.
        fn in_call(&self, listed: &[AccountId], sharing: &[AccountId]) -> Heard {
            self.heard(&self.calls("seat"), listed, sharing)
        }

        fn heard(&self, calls: &CallsView, listed: &[AccountId], sharing: &[AccountId]) -> Heard {
            let participants: Vec<CallParticipant> = listed
                .iter()
                .map(|account_id| participant(*account_id, sharing.contains(account_id)))
                .collect();
            heard(calls, Some(&participants), Some(self.own))
        }
    }

    fn participant(account_id: AccountId, shares: bool) -> CallParticipant {
        let at = time::OffsetDateTime::UNIX_EPOCH;
        CallParticipant {
            avatar: None,
            account_id,
            identity: format!("seat-{account_id}"),
            joined_at: at,
            muted: false,
            deafened: false,
            microphone_block: None,
            sound_block: None,
            hand_raised: false,
            screen_share: shares.then_some(ScreenShare {
                audio: ShareAudio::None,
                started_at: at,
            }),
        }
    }

    #[test]
    fn a_person_who_joins_or_leaves_the_call_plays_a_sound_of_its_own() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let alone = scene.in_call(&[scene.own], &[]);
        let with_other = scene.in_call(&[scene.own, other], &[]);
        assert_eq!(sounds(&alone, &with_other, true), vec![Sound::PersonJoined]);
        assert_eq!(sounds(&with_other, &alone, true), vec![Sound::PersonLeft]);
    }

    #[test]
    fn the_people_present_when_the_person_enters_play_no_sound() {
        let scene = Scene::new();
        let (first, second) = (AccountId::generate(), AccountId::generate());
        let outside = Heard::default();
        // The device has its seat, and the server does not list the person yet.
        let entering = scene.in_call(&[first, second], &[first]);
        assert_eq!(entering.present, None);
        let listed = scene.in_call(&[first, second, scene.own], &[first]);
        assert!(sounds(&outside, &entering, true).is_empty());
        assert!(sounds(&entering, &listed, true).is_empty());
        // In a call that hides who is inside, the server lists everybody at once when it
        // has seen the device.
        let hidden = scene.in_call(&[], &[]);
        assert!(sounds(&outside, &hidden, true).is_empty());
        assert!(sounds(&hidden, &listed, true).is_empty());
        let third = scene.in_call(&[first, second, scene.own, AccountId::generate()], &[first]);
        assert_eq!(sounds(&listed, &third, true), vec![Sound::PersonJoined]);
    }

    #[test]
    fn the_microphone_and_the_sound_of_the_person_play_a_sound_for_off_and_one_for_on() {
        let scene = Scene::new();
        let open = scene.in_call(&[scene.own], &[]);
        let closed = Heard {
            muted: true,
            ..open.clone()
        };
        let deaf = Heard {
            deafened: true,
            ..open.clone()
        };
        assert_eq!(sounds(&open, &closed, true), vec![Sound::MicrophoneOff]);
        assert_eq!(sounds(&closed, &open, true), vec![Sound::MicrophoneOn]);
        assert_eq!(sounds(&open, &deaf, true), vec![Sound::SoundOff]);
        assert_eq!(sounds(&deaf, &open, true), vec![Sound::SoundOn]);
    }

    #[test]
    fn a_share_that_starts_plays_a_sound_and_one_that_continues_does_not() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let none = scene.in_call(&[scene.own, other], &[]);
        let other_shares = scene.in_call(&[scene.own, other], &[other]);
        let both_share = scene.in_call(&[scene.own, other], &[other, scene.own]);
        assert_eq!(
            sounds(&none, &other_shares, true),
            vec![Sound::ShareStarted]
        );
        assert_eq!(
            sounds(&other_shares, &both_share, true),
            vec![Sound::ShareStarted]
        );
        assert!(sounds(&both_share, &both_share, true).is_empty());
        assert!(sounds(&other_shares, &none, true).is_empty());
    }

    #[test]
    fn the_person_who_leaves_the_call_hears_one_sound() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let in_call = scene.in_call(&[scene.own, other], &[]);
        let outside = Heard::default();
        assert_eq!(sounds(&in_call, &outside, true), vec![Sound::CallLeft]);
        let deaf_in_call = Heard {
            deafened: true,
            ..in_call
        };
        let deaf_outside = Heard {
            deafened: true,
            ..Heard::default()
        };
        assert!(sounds(&deaf_in_call, &deaf_outside, true).is_empty());
    }

    #[test]
    fn a_device_outside_the_call_hears_nothing_of_it() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let outside = Heard::default();
        // Other people join and leave calls in which this device is not; the setting of
        // the microphone changes outside a call.
        let still_outside = Heard {
            muted: true,
            ..Heard::default()
        };
        assert!(sounds(&outside, &still_outside, true).is_empty());
        let listed = heard(
            &CallsView::default(),
            Some(&[participant(other, true)]),
            Some(scene.own),
        );
        assert_eq!(listed, outside);
    }

    #[test]
    fn a_move_into_another_call_plays_no_sound_for_the_people_there() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let before = scene.in_call(&[scene.own], &[]);
        let moved = scene.heard(&scene.calls("next seat"), &[scene.own, other], &[other]);
        assert!(sounds(&before, &moved, true).is_empty());
    }

    #[test]
    fn the_setting_off_plays_no_sound() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let alone = scene.in_call(&[scene.own], &[]);
        let changed = Heard {
            muted: true,
            ..scene.in_call(&[scene.own, other], &[other])
        };
        assert_eq!(
            sounds(&alone, &changed, true),
            vec![
                Sound::MicrophoneOff,
                Sound::PersonJoined,
                Sound::ShareStarted
            ]
        );
        assert!(sounds(&alone, &changed, false).is_empty());
        assert!(sounds(&alone, &Heard::default(), false).is_empty());
    }

    #[test]
    fn while_the_sound_is_off_only_the_change_of_the_sound_plays() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let deaf = Heard {
            deafened: true,
            ..scene.in_call(&[scene.own], &[])
        };
        let deaf_and_changed = Heard {
            deafened: true,
            muted: true,
            ..scene.in_call(&[scene.own, other], &[other])
        };
        assert!(sounds(&deaf, &deaf_and_changed, true).is_empty());
        let open = scene.in_call(&[scene.own], &[]);
        let deaf_and_muted = Heard {
            muted: true,
            ..deaf
        };
        assert_eq!(sounds(&open, &deaf_and_muted, true), vec![Sound::SoundOff]);
    }
}
