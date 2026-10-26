//! Which short sounds a change of the call of this device plays (ADR 0707).

use std::collections::{BTreeMap, BTreeSet};

use junto_client_core::protocol::call::CallParticipant;
use junto_client_core::protocol::ids::{AccountId, EntryRequestId, RecordingId};
use junto_media::audio::DeviceId;
use junto_media::audio::sounds::Sound;
use junto_media::calls::{ActiveCall, CallsView, Phase, Seat};

/// What the state of the server of the call says of the call of this device.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Listed<'a> {
    /// The account of this device on that server.
    pub own: AccountId,
    /// The people whom the server lists at the place of the call.
    pub participants: &'a [CallParticipant],
    /// The person may lower the hands of others: the presenter of meeting mode, or a
    /// moderator who may close microphones (ADR 0516).
    pub lowers_hands: bool,
    /// The requests of people outside to enter the call.
    pub entry_requests: &'a [EntryRequestId],
    /// The recording of the call, and whether a stop was asked for.
    pub recording: Option<(RecordingId, bool)>,
    /// A moderator moves the person into the place of the call.
    pub moved_here: bool,
}

/// What the sounds of a call are decided from: the call of this device, the person's own
/// settings, and what the server lists in that call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Heard {
    /// The seat of this device. Absent outside a call.
    seat: Option<Seat>,
    phase: Option<Phase>,
    /// The microphone is closed: by the person, or by a block of a moderator or of meeting
    /// mode.
    closed: bool,
    /// The person hears nothing: by the person's own setting, or by a block.
    deafened: bool,
    /// The seat began with a move of a moderator: its entry plays the sound of the move.
    moved: bool,
    /// The people whom the server lists in the call, each with whether the person shares.
    /// Absent until the server lists the person of this device, so that the people who
    /// were in the call before the person are not heard as people who join.
    present: Option<BTreeMap<AccountId, bool>>,
    /// The people with a raised hand, for a person who may lower them.
    hands: Option<BTreeSet<AccountId>>,
    entry_requests: BTreeSet<EntryRequestId>,
    /// The recording that runs, with no stop asked for.
    recording: Option<RecordingId>,
    /// The people who watch the share of this device, while it runs (ADR 0502).
    viewers: Option<BTreeSet<AccountId>>,
    /// The output device of the call, on which its sounds play.
    pub(crate) output: Option<DeviceId>,
}

/// What a change of the calls is decided from. `before` is what the last change was
/// decided from: a seat keeps whether it began with a move.
pub(crate) fn heard(calls: &CallsView, listed: Option<Listed<'_>>, before: &Heard) -> Heard {
    let call = calls.call.as_ref();
    let seat = call.map(ActiveCall::seat);
    let listed = call.and(listed);
    let present = listed
        .filter(|listed| {
            listed
                .participants
                .iter()
                .any(|participant| participant.account_id == listed.own)
        })
        .map(|listed| {
            listed
                .participants
                .iter()
                .map(|participant| (participant.account_id, participant.screen_share.is_some()))
                .collect()
        });
    let moved = if seat.is_some() && seat == before.seat {
        before.moved
    } else {
        listed.is_some_and(|listed| listed.moved_here)
    };
    Heard {
        phase: call.map(|call| call.phase),
        closed: calls.muted || call.is_some_and(|call| call.microphone_block.is_some()),
        deafened: calls.deafened || call.is_some_and(|call| call.sound_block.is_some()),
        moved,
        hands: listed
            .filter(|listed| listed.lowers_hands && present.is_some())
            .map(|listed| {
                listed
                    .participants
                    .iter()
                    .filter(|participant| {
                        participant.hand_raised && participant.account_id != listed.own
                    })
                    .map(|participant| participant.account_id)
                    .collect()
            }),
        entry_requests: listed
            .map(|listed| listed.entry_requests.iter().copied().collect())
            .unwrap_or_default(),
        recording: listed
            .and_then(|listed| listed.recording)
            .filter(|(_, stopping)| !stopping)
            .map(|(id, _)| id),
        viewers: listed
            .filter(|listed| {
                listed.participants.iter().any(|participant| {
                    participant.account_id == listed.own && participant.screen_share.is_some()
                })
            })
            .map(|listed| {
                listed
                    .participants
                    .iter()
                    .filter(|participant| {
                        participant.account_id != listed.own
                            && participant
                                .watching
                                .is_some_and(|watched| watched.account_id == listed.own)
                    })
                    .map(|participant| participant.account_id)
                    .collect()
            }),
        present,
        seat,
        output: call
            .and_then(|call| call.output_device.as_ref())
            .map(|device| device.id.clone()),
    }
}

/// The sounds of the change from `before` to `after`, in the order in which they play.
/// Only the person in the call hears them. While the sound of the person is off, only the
/// sound that turns it off plays. The switches of the kinds are applied by the caller.
pub(crate) fn sounds(before: &Heard, after: &Heard) -> Vec<Sound> {
    match (&before.seat, &after.seat) {
        (Some(_), None) if !after.deafened => return vec![Sound::CallLeft],
        (Some(before), Some(after)) if before == after => {}
        // A call that begins, or a seat in another call: the sound of the entry plays when
        // the server lists the person; a move plays its sound at once.
        (_, Some(_)) if after.moved && !after.deafened => return vec![Sound::Moved],
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
    if before.closed != after.closed {
        sounds.push(if after.closed {
            Sound::MicrophoneOff
        } else {
            Sound::MicrophoneOn
        });
    }
    if before.phase == Some(Phase::Connected) && after.phase == Some(Phase::Reconnecting) {
        sounds.push(Sound::ConnectionLost);
    }
    let Some(listed_after) = &after.present else {
        return sounds;
    };
    let Some(listed_before) = &before.present else {
        // The server lists the person now: the person entered, and hears whether the call
        // is recorded.
        if !after.moved {
            sounds.push(Sound::CallJoined);
        }
        if after.recording.is_some() {
            sounds.push(Sound::RecordingStarted);
        }
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
    if listed_before
        .iter()
        .any(|(id, shared)| *shared && listed_after.get(id) == Some(&false))
    {
        sounds.push(Sound::ShareEnded);
    }
    // A person who starts or stops watching the share of this device; a person who leaves
    // the call is heard leaving.
    if let (Some(viewers_before), Some(viewers_after)) = (&before.viewers, &after.viewers) {
        if viewers_after.iter().any(|id| !viewers_before.contains(id)) {
            sounds.push(Sound::ViewerJoined);
        }
        if viewers_before
            .iter()
            .any(|id| !viewers_after.contains(id) && listed_after.contains_key(id))
        {
            sounds.push(Sound::ViewerLeft);
        }
    }
    if let (Some(hands_before), Some(hands_after)) = (&before.hands, &after.hands)
        && hands_after.iter().any(|id| !hands_before.contains(id))
    {
        sounds.push(Sound::HandRaised);
    }
    if after
        .entry_requests
        .iter()
        .any(|id| !before.entry_requests.contains(id))
    {
        sounds.push(Sound::Request);
    }
    match (before.recording, after.recording) {
        (None, Some(_)) => sounds.push(Sound::RecordingStarted),
        (Some(_), None) => sounds.push(Sound::RecordingStopped),
        (Some(one), Some(other)) if one != other => sounds.push(Sound::RecordingStarted),
        _ => {}
    }
    sounds
}

#[cfg(test)]
mod tests {
    use super::*;
    use junto_client_core::protocol::call::{
        BlockReason, CallPlace, MicrophoneBlock, ScreenShare, ShareAudio,
    };
    use junto_client_core::protocol::ids::{ChannelId, ServerId, WorkspaceId};
    use junto_media::calls::CallTarget;
    use junto_media::engine::{MicrophoneState, OutputState, ShareState};
    use junto_media::permission::MicrophoneAccess;

    struct Scene {
        own: AccountId,
        target: CallTarget,
    }

    /// What the server lists, for a test.
    #[derive(Default, Clone)]
    struct Server {
        listed: Vec<AccountId>,
        sharing: Vec<AccountId>,
        hands: Vec<AccountId>,
        lowers_hands: bool,
        requests: Vec<EntryRequestId>,
        recording: Option<(RecordingId, bool)>,
        moved_here: bool,
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
                    share: ShareState::Off,
                    shares: Vec::new(),
                    watched: None,
                }),
                ..CallsView::default()
            }
        }

        /// The state in the call `seat`, with the server listing `listed`, of whom
        /// `sharing` share.
        fn in_call(&self, listed: &[AccountId], sharing: &[AccountId]) -> Heard {
            self.with(
                &self.calls("seat"),
                &Server {
                    listed: listed.to_vec(),
                    sharing: sharing.to_vec(),
                    ..Server::default()
                },
                &Heard::default(),
            )
        }

        fn with(&self, calls: &CallsView, server: &Server, before: &Heard) -> Heard {
            let participants: Vec<CallParticipant> = server
                .listed
                .iter()
                .map(|account_id| {
                    let mut listed = participant(*account_id, server.sharing.contains(account_id));
                    listed.hand_raised = server.hands.contains(account_id);
                    listed
                })
                .collect();
            heard(
                calls,
                Some(Listed {
                    own: self.own,
                    participants: &participants,
                    lowers_hands: server.lowers_hands,
                    entry_requests: &server.requests,
                    recording: server.recording,
                    moved_here: server.moved_here,
                }),
                before,
            )
        }
    }

    fn block() -> MicrophoneBlock {
        MicrophoneBlock {
            by: AccountId::generate(),
            reason: BlockReason::Moderator,
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
            watching: None,
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
        assert_eq!(sounds(&alone, &with_other), vec![Sound::PersonJoined]);
        assert_eq!(sounds(&with_other, &alone), vec![Sound::PersonLeft]);
    }

    #[test]
    fn the_person_hears_the_own_entry_and_not_the_people_present() {
        let scene = Scene::new();
        let (first, second) = (AccountId::generate(), AccountId::generate());
        let outside = Heard::default();
        // The device has its seat, and the server does not list the person yet.
        let entering = scene.in_call(&[first, second], &[first]);
        assert_eq!(entering.present, None);
        let listed = scene.in_call(&[first, second, scene.own], &[first]);
        assert!(sounds(&outside, &entering).is_empty());
        assert_eq!(sounds(&entering, &listed), vec![Sound::CallJoined]);
        // In a call that hides who is inside, the server lists everybody at once when it
        // has seen the device.
        let hidden = scene.in_call(&[], &[]);
        assert!(sounds(&outside, &hidden).is_empty());
        assert_eq!(sounds(&hidden, &listed), vec![Sound::CallJoined]);
        let third = scene.in_call(&[first, second, scene.own, AccountId::generate()], &[first]);
        assert_eq!(sounds(&listed, &third), vec![Sound::PersonJoined]);
    }

    #[test]
    fn the_microphone_and_the_sound_of_the_person_play_a_sound_for_off_and_one_for_on() {
        let scene = Scene::new();
        let open = scene.in_call(&[scene.own], &[]);
        let closed = Heard {
            closed: true,
            ..open.clone()
        };
        let deaf = Heard {
            deafened: true,
            ..open.clone()
        };
        assert_eq!(sounds(&open, &closed), vec![Sound::MicrophoneOff]);
        assert_eq!(sounds(&closed, &open), vec![Sound::MicrophoneOn]);
        assert_eq!(sounds(&open, &deaf), vec![Sound::SoundOff]);
        assert_eq!(sounds(&deaf, &open), vec![Sound::SoundOn]);
    }

    #[test]
    fn a_block_of_a_moderator_plays_the_sound_of_a_closed_microphone_and_its_end_the_open_one() {
        let scene = Scene::new();
        let server = Server {
            listed: vec![scene.own],
            ..Server::default()
        };
        let open = scene.with(&scene.calls("seat"), &server, &Heard::default());
        let mut blocked_calls = scene.calls("seat");
        if let Some(call) = blocked_calls.call.as_mut() {
            call.microphone_block = Some(block());
        }
        let blocked = scene.with(&blocked_calls, &server, &open);
        assert_eq!(sounds(&open, &blocked), vec![Sound::MicrophoneOff]);
        assert_eq!(sounds(&blocked, &open), vec![Sound::MicrophoneOn]);
        // A block of a microphone that the person had closed changes nothing that is heard.
        let muted = CallsView {
            muted: true,
            ..scene.calls("seat")
        };
        let closed = scene.with(&muted, &server, &open);
        let mut muted_and_blocked = muted.clone();
        if let Some(call) = muted_and_blocked.call.as_mut() {
            call.microphone_block = Some(block());
        }
        assert!(sounds(&closed, &scene.with(&muted_and_blocked, &server, &closed)).is_empty());
    }

    #[test]
    fn a_share_that_starts_or_ends_plays_a_sound_and_one_that_continues_does_not() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let none = scene.in_call(&[scene.own, other], &[]);
        let other_shares = scene.in_call(&[scene.own, other], &[other]);
        let both_share = scene.in_call(&[scene.own, other], &[other, scene.own]);
        assert_eq!(sounds(&none, &other_shares), vec![Sound::ShareStarted]);
        assert_eq!(
            sounds(&other_shares, &both_share),
            vec![Sound::ShareStarted]
        );
        assert!(sounds(&both_share, &both_share).is_empty());
        assert_eq!(sounds(&other_shares, &none), vec![Sound::ShareEnded]);
        // A person who leaves while sharing is heard leaving.
        let alone = scene.in_call(&[scene.own], &[]);
        assert_eq!(sounds(&other_shares, &alone), vec![Sound::PersonLeft]);
    }

    #[test]
    fn the_person_who_leaves_the_call_hears_one_sound() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let in_call = scene.in_call(&[scene.own, other], &[]);
        let outside = Heard::default();
        assert_eq!(sounds(&in_call, &outside), vec![Sound::CallLeft]);
        let deaf_in_call = Heard {
            deafened: true,
            ..in_call
        };
        let deaf_outside = Heard {
            deafened: true,
            ..Heard::default()
        };
        assert!(sounds(&deaf_in_call, &deaf_outside).is_empty());
    }

    #[test]
    fn a_device_outside_the_call_hears_nothing_of_it() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let outside = Heard::default();
        let still_outside = Heard {
            closed: true,
            ..Heard::default()
        };
        assert!(sounds(&outside, &still_outside).is_empty());
        let participants = [participant(other, true)];
        let listed = heard(
            &CallsView::default(),
            Some(Listed {
                own: scene.own,
                participants: &participants,
                lowers_hands: true,
                entry_requests: &[EntryRequestId::generate()],
                recording: Some((RecordingId::generate(), false)),
                moved_here: false,
            }),
            &outside,
        );
        assert_eq!(listed, outside);
    }

    #[test]
    fn a_switch_into_another_call_plays_the_entry_there_and_not_the_people_present() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let before = scene.in_call(&[scene.own], &[]);
        let server = Server {
            listed: vec![other],
            sharing: vec![other],
            ..Server::default()
        };
        let seated = scene.with(&scene.calls("next seat"), &server, &before);
        assert!(sounds(&before, &seated).is_empty());
        let listed = scene.with(
            &scene.calls("next seat"),
            &Server {
                listed: vec![other, scene.own],
                ..server
            },
            &seated,
        );
        assert_eq!(sounds(&seated, &listed), vec![Sound::CallJoined]);
    }

    #[test]
    fn a_move_of_a_moderator_plays_the_sound_of_the_move_once() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let before = scene.in_call(&[scene.own], &[]);
        let moving = Server {
            listed: vec![other],
            moved_here: true,
            ..Server::default()
        };
        let seated = scene.with(&scene.calls("next seat"), &moving, &before);
        assert_eq!(sounds(&before, &seated), vec![Sound::Moved]);
        // The move ends before the server lists the person: the seat keeps that it began
        // with a move, and the entry plays nothing more.
        let listed = scene.with(
            &scene.calls("next seat"),
            &Server {
                listed: vec![other, scene.own],
                ..Server::default()
            },
            &seated,
        );
        assert!(sounds(&seated, &listed).is_empty());
    }

    #[test]
    fn a_lost_connection_plays_a_sound() {
        let scene = Scene::new();
        let connected = scene.in_call(&[scene.own], &[]);
        let mut calls = scene.calls("seat");
        if let Some(call) = calls.call.as_mut() {
            call.phase = Phase::Reconnecting;
        }
        let lost = scene.with(
            &calls,
            &Server {
                listed: vec![scene.own],
                ..Server::default()
            },
            &connected,
        );
        assert_eq!(sounds(&connected, &lost), vec![Sound::ConnectionLost]);
        assert!(sounds(&lost, &connected).is_empty());
    }

    #[test]
    fn a_raised_hand_plays_a_sound_for_a_person_who_may_lower_it() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let calls = scene.calls("seat");
        let down = Server {
            listed: vec![scene.own, other],
            lowers_hands: true,
            ..Server::default()
        };
        let up = Server {
            hands: vec![other],
            ..down.clone()
        };
        let before = scene.with(&calls, &down, &Heard::default());
        let after = scene.with(&calls, &up, &before);
        assert_eq!(sounds(&before, &after), vec![Sound::HandRaised]);
        assert!(sounds(&after, &before).is_empty());
        // A person who may not lower hands hears nothing of them.
        let plain = |server: &Server| Server {
            lowers_hands: false,
            ..server.clone()
        };
        let before = scene.with(&calls, &plain(&down), &Heard::default());
        assert!(sounds(&before, &scene.with(&calls, &plain(&up), &before)).is_empty());
    }

    #[test]
    fn a_request_to_enter_plays_a_sound_for_the_people_inside() {
        let scene = Scene::new();
        let calls = scene.calls("seat");
        let quiet = Server {
            listed: vec![scene.own],
            ..Server::default()
        };
        let asked = Server {
            requests: vec![EntryRequestId::generate()],
            ..quiet.clone()
        };
        let before = scene.with(&calls, &quiet, &Heard::default());
        let after = scene.with(&calls, &asked, &before);
        assert_eq!(sounds(&before, &after), vec![Sound::Request]);
        assert!(sounds(&after, &after).is_empty());
        assert!(sounds(&after, &before).is_empty());
    }

    #[test]
    fn a_recording_plays_a_sound_when_it_starts_when_it_stops_and_at_an_entry_into_it() {
        let scene = Scene::new();
        let calls = scene.calls("seat");
        let recording = RecordingId::generate();
        let plain = Server {
            listed: vec![scene.own],
            ..Server::default()
        };
        let recorded = Server {
            recording: Some((recording, false)),
            ..plain.clone()
        };
        let stopping = Server {
            recording: Some((recording, true)),
            ..plain.clone()
        };
        let before = scene.with(&calls, &plain, &Heard::default());
        let running = scene.with(&calls, &recorded, &before);
        assert_eq!(sounds(&before, &running), vec![Sound::RecordingStarted]);
        let stopped = scene.with(&calls, &stopping, &running);
        assert_eq!(sounds(&running, &stopped), vec![Sound::RecordingStopped]);
        assert!(sounds(&stopped, &scene.with(&calls, &plain, &stopped)).is_empty());
        // A person who enters a recorded call hears that it is recorded.
        let entering = scene.with(
            &calls,
            &Server {
                listed: Vec::new(),
                ..recorded.clone()
            },
            &Heard::default(),
        );
        let entered = scene.with(&calls, &recorded, &entering);
        assert_eq!(
            sounds(&entering, &entered),
            vec![Sound::CallJoined, Sound::RecordingStarted]
        );
    }

    #[test]
    fn a_person_who_starts_or_stops_watching_the_own_share_plays_a_sound() {
        let scene = Scene::new();
        let other = AccountId::generate();
        let calls = scene.calls("seat");
        let watching = |watches: bool| {
            let mut own = participant(scene.own, true);
            own.watching = None;
            let mut viewer = participant(other, false);
            viewer.watching = watches.then_some(junto_client_core::protocol::call::WatchedShare {
                account_id: scene.own,
                slot: 0,
            });
            vec![own, viewer]
        };
        let listed = |participants: &[CallParticipant], before: &Heard| {
            heard(
                &calls,
                Some(Listed {
                    own: scene.own,
                    participants,
                    lowers_hands: false,
                    entry_requests: &[],
                    recording: None,
                    moved_here: false,
                }),
                before,
            )
        };
        let (off, on) = (watching(false), watching(true));
        let before = listed(&off, &Heard::default());
        let after = listed(&on, &before);
        assert_eq!(sounds(&before, &after), vec![Sound::ViewerJoined]);
        assert_eq!(sounds(&after, &before), vec![Sound::ViewerLeft]);
        // A viewer who leaves the call is heard leaving.
        let alone = [participant(scene.own, true)];
        assert_eq!(
            sounds(&after, &listed(&alone, &after)),
            vec![Sound::PersonLeft]
        );
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
            closed: true,
            ..scene.in_call(&[scene.own, other], &[other])
        };
        assert!(sounds(&deaf, &deaf_and_changed).is_empty());
        let open = scene.in_call(&[scene.own], &[]);
        let deaf_and_muted = Heard {
            closed: true,
            ..deaf
        };
        assert_eq!(sounds(&open, &deaf_and_muted), vec![Sound::SoundOff]);
    }
}
