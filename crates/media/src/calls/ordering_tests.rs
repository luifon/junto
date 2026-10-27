#![allow(clippy::unwrap_used)]

use super::*;
use crate::engine::Asked;
use junto_client_core::ClientOptions;
use junto_client_core::address::ServerAddress;
use junto_client_core::book::{ConnectionMode, Membership, ProfileSync, ServerBook, ServerEntry};
use junto_client_core::paths::DataDir;
use junto_client_core::protocol::account::OwnAccount;
use junto_client_core::protocol::call::ChannelCall;
use junto_client_core::protocol::call::{BlockReason, CallParticipant, Meeting, MeetingEnd};
use junto_client_core::protocol::ids::{CallId, ChannelId, DeviceId, WorkspaceId};
use junto_client_core::protocol::secret::CallToken;
use junto_client_core::protocol::snapshot::Snapshot;
use junto_client_core::protocol::snapshot::WorkspaceState;
use junto_client_core::protocol::structure::{Channel, ChannelKind, Workspace};

struct Fixture {
    session: Session,
    model: ServerModel,
    answer: CallJoin,
    _directory: tempfile::TempDir,
}

impl Fixture {
    fn call(&mut self) -> &mut junto_client_core::model::CallModel {
        Arc::make_mut(&mut self.model.workspaces[0]).channels[0]
            .call
            .as_mut()
            .unwrap()
    }
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("client");
    let server_id = ServerId::generate();
    let account_id = AccountId::generate();
    let device_id = DeviceId::generate();
    ServerBook::new(DataDir::open(&root).unwrap())
        .add(ServerEntry {
            server_id,
            generation: 0,
            address: ServerAddress::parse("http://127.0.0.1:1").unwrap(),
            name: "Test".into(),
            logo_path: None,
            api_versions: vec![1],
            connection_mode: ConnectionMode::Always,
            membership: Membership::Member {
                account_id,
                device_id,
            },
            last_cursor: None,
            profile_sync: ProfileSync::NotNeeded,
            moderation_notices_shown: Vec::new(),
            logged_out: false,
        })
        .unwrap();
    let client = Client::open(ClientOptions::new(root)).unwrap();
    let server = client.server(server_id).unwrap();
    let options = MediaOptions {
        camera: Arc::new(crate::camera::synthetic::SyntheticCamera::new()),
        host: Arc::new(crate::audio::synthetic::SyntheticHost::with_tone(440.0)),
        permission: Arc::new(crate::permission::FixedPermission::allowed()),
        capture: Arc::new(crate::share::synthetic::SyntheticCapture::new()),
        timing: CallTiming::default(),
    };
    let calls = Calls::new(client, options.clone());
    let workspace_id = WorkspaceId::generate();
    let channel_id = ChannelId::generate();
    let call = Call {
        id: CallId::generate(),
        place: CallPlace::Channel {
            workspace_id,
            channel_id,
        },
        started_at: Some(OffsetDateTime::UNIX_EPOCH),
        lock: None,
        meeting: None,
        hides_occupants: false,
        recording: None,
        participants: vec![CallParticipant {
            avatar: None,
            account_id,
            identity: "seat".into(),
            joined_at: OffsetDateTime::UNIX_EPOCH,
            muted: false,
            deafened: false,
            microphone_block: None,
            sound_block: None,
            hand_raised: false,
            screen_share: None,
        }],
    };
    let answer = CallJoin {
        cursor: Some(Cursor {
            epoch: StreamEpoch::generate(),
            position: 45,
        }),
        call: call.clone(),
        media_url: "ws://127.0.0.1:1".into(),
        token: CallToken::new("unused"),
        token_expires_at: OffsetDateTime::UNIX_EPOCH,
        identity: "seat".into(),
        entry_requests: vec![],
        meeting_grant: None,
        microphone_block: None,
        sound_block: None,
    };
    let model = ServerModel::from_snapshot(Snapshot {
        cursor: Cursor {
            epoch: answer.cursor.unwrap().epoch,
            position: 40,
        },
        account: OwnAccount {
            id: account_id,
            name: "Person".into(),
            avatar: None,
            is_owner: false,
            server_role_ids: vec![],
            server_permissions: vec![],
            workspace_ids: vec![],
        },
        workspaces: vec![WorkspaceState {
            workspace: Workspace {
                id: workspace_id,
                name: "Calls".into(),
                is_default: true,
            },
            own_role_ids: vec![],
            roles: vec![],
            categories: vec![],
            channels: vec![Channel {
                id: channel_id,
                workspace_id,
                category_id: None,
                kind: ChannelKind::Voice,
                name: "Voice".into(),
                position: 0,
                permanent_messages: false,
                user_limit: None,
                hides_occupants: false,
            }],
            calls: vec![ChannelCall {
                workspace_id,
                channel_id,
                in_use: true,
                call: Some(call.clone()),
            }],
            permissions: vec![],
            channel_permissions: vec![],
            notification_default: None,
        }],
        own_call: Some(OwnCall {
            hides_occupants: Some(false),
            call_id: call.id,
            place: call.place,
            device_id,
            identity: "seat".into(),
        }),
        direct_calls: vec![],
        call_invitations: vec![],
        sent_call_invitations: vec![],
        call_moves: vec![],
        entry_requests: vec![],
        direct_mutes: vec![],
        messages: vec![],
        threads: vec![],
        threads_more: false,
        threads_next: None,
        notifications: vec![],
        notification_settings: vec![],
    });
    let controls = Controls::default();
    let engine = Engine::start(
        &calls.inner.runtime,
        EngineOptions {
            camera: options.camera,
            host: options.host,
            permission: options.permission,
            capture: options.capture,
        },
        media_join(&answer),
        VoiceConfig::default(),
        controls,
        HashMap::new(),
        Watching::default(),
    );
    let timing = options.timing;
    let session = Session {
        inner: calls.inner,
        key: 1,
        server,
        target: CallTarget {
            server_id,
            place: call.place,
        },
        place: Place::new("seat".into(), Instant::now(), timing.settle),
        fate: Fate::Held,
        review: false,
        engine,
        controls,
        microphone_block: None,
        sound_block: None,
        withheld: Withheld::default(),
        blocks_untold: false,
        refresh: None,
        removal_read: None,
        accepted: model.cursor,
        seat_told: None,
        grants: MeetingHistory::new(model.cursor, &call, account_id, false),
        admitted: true,
        connected_once: true,
        interrupted_at: None,
        retry: None,
        backoff: Backoff::new(timing.reconnect_first, timing.reconnect_longest),
        rejoining: true,
        renew_open: false,
        renewals: Vec::new(),
        grant_closed: false,
        state_report: Reporter::new(timing.state_spacing, None),
        speaking_report: Reporter::new(timing.speaking_spacing, Some(timing.speaking_renewal)),
        share_report: Reporter::holding(timing.share_spacing, None),
        moving: None,
        probes: Vec::new(),
        speaking: false,
        timing,
    };
    Fixture {
        session,
        model,
        answer,
        _directory: directory,
    }
}

#[tokio::test]
async fn a_renewal_cannot_be_overwritten_by_the_older_model() {
    let mut f = fixture();
    let by = AccountId::generate();
    f.answer.microphone_block = Some(MicrophoneBlock {
        by,
        reason: BlockReason::Moderator,
    });
    f.answer.sound_block = Some(SoundBlock { by });
    let (taken, _) = oneshot::channel();
    f.session
        .finished(Done::Rejoined(Box::new(Ok(f.answer.clone())), taken));
    f.session.read_blocks(&f.model);
    assert!(f.session.microphone_block.is_some() && f.session.sound_block.is_some());
    assert!(
        f.session.controls.microphone_blocked,
        "the old model reopened capture"
    );
    assert!(
        f.session.controls.sound_blocked,
        "the old model reopened playout"
    );
    assert!(
        f.session.admitted,
        "an answer for the seat that the session holds ended the admission"
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn an_older_renewal_does_not_remove_blocks_already_learned() {
    let mut f = fixture();
    let by = AccountId::generate();
    f.session.set_blocks((
        Some(MicrophoneBlock {
            by,
            reason: BlockReason::Moderator,
        }),
        Some(SoundBlock { by }),
    ));
    f.session.accepted.position = 46;
    let (taken, _) = oneshot::channel();
    f.session
        .finished(Done::Rejoined(Box::new(Ok(f.answer.clone())), taken));
    assert!(f.session.microphone_block.is_some() && f.session.sound_block.is_some());
    assert!(
        f.session.controls.microphone_blocked,
        "the late answer reopened capture"
    );
    assert!(
        f.session.controls.sound_blocked,
        "the late answer reopened playout"
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn unrelated_events_and_an_old_identity_do_not_release_the_gate() {
    let mut f = fixture();
    f.answer.identity = "new-seat".into();
    let (taken, _) = oneshot::channel();
    f.session
        .finished(Done::Rejoined(Box::new(Ok(f.answer.clone())), taken));
    f.model.cursor.position += 1;
    f.session.read_blocks(&f.model);
    assert!(f.session.controls.microphone_blocked && f.session.controls.sound_blocked);
    assert!(!f.session.admitted);
    f.session.refresh = None;
    f.model.cursor.position = 46;
    f.session.read_blocks(&f.model);
    assert!(f.session.controls.microphone_blocked && f.session.controls.sound_blocked);
    assert!(!f.session.admitted);
    f.model.cursor.position = 47;
    f.call().participants[0].identity = "new-seat".into();
    f.session.read_blocks(&f.model);
    // The state lists the seat. The events up to the answer that gave it are not read
    // yet: the seat can still be void (ADR 0785).
    assert!(!f.session.admitted);
    let (call_id, place) = (f.call().id, f.session.target.place);
    let participant = f.call().participants[0].clone();
    tell(
        &mut f,
        vec![(
            47,
            Event::CallParticipantJoined {
                call_id,
                place,
                participant,
            },
        )],
    );
    f.session.read_blocks(&f.model);
    assert!(!f.session.controls.microphone_blocked && !f.session.controls.sound_blocked);
    assert!(f.session.admitted);
    f.session.engine.close().await;
}

/// ADR 0517: a server that says that its admission check has not seen the seat is believed
/// before the list of the call. A server that says nothing of the seat is read by the list.
#[tokio::test]
async fn a_seat_that_the_server_says_is_not_inside_is_not_admitted() {
    use junto_client_core::model::SeatStated;
    let stated = |identity: &str, inside| {
        Some(SeatStated {
            identity: identity.into(),
            inside,
        })
    };
    let mut f = fixture();
    f.session.rejoining = false;
    f.model.own_seat = stated("seat", false);
    f.session.read_blocks(&f.model);
    assert!(
        !f.session.admitted,
        "the list admitted a seat that the server says it has not seen"
    );
    assert!(f.session.controls.microphone_blocked && f.session.controls.sound_blocked);
    // What the server says of another seat of the account says nothing of this one.
    f.model.own_seat = stated("other-seat", false);
    f.session.read_blocks(&f.model);
    assert!(f.session.admitted);
    assert!(!f.session.controls.microphone_blocked && !f.session.controls.sound_blocked);
    f.model.own_seat = stated("seat", false);
    f.session.read_blocks(&f.model);
    assert!(!f.session.admitted);
    // Not stated is not "outside".
    f.model.own_seat = None;
    f.session.read_blocks(&f.model);
    assert!(f.session.admitted);
    f.model.own_seat = stated("seat", true);
    f.session.read_blocks(&f.model);
    assert!(f.session.admitted);
    assert!(!f.session.controls.microphone_blocked && !f.session.controls.sound_blocked);
    f.session.engine.close().await;
}

#[tokio::test]
async fn only_the_fresh_meeting_state_can_close_the_local_microphone_on_unblock() {
    use junto_client_core::protocol::call::Meeting;
    for still_presenting in [false, true] {
        let mut f = fixture();
        let presenter = AccountId::generate();
        f.session.set_blocks((
            Some(MicrophoneBlock {
                by: presenter,
                reason: BlockReason::Meeting,
            }),
            None,
        ));
        f.call().meeting = Some(Meeting {
            presenter,
            started_at: OffsetDateTime::UNIX_EPOCH,
        });
        f.call().participants[0].hand_raised = true;
        f.model.cursor.position = 39;
        f.answer.cursor.as_mut().unwrap().position = 39;
        let (taken, _) = oneshot::channel();
        f.session
            .finished(Done::Rejoined(Box::new(Ok(f.answer.clone())), taken));
        f.session.read_blocks(&f.model);
        assert!(
            !f.session.inner.shared().muted,
            "the old meeting changed the local setting"
        );
        if !still_presenting {
            f.call().meeting = None;
        }
        f.call().participants[0].hand_raised = false;
        f.session.refresh = None;
        f.model.cursor.position = 46;
        f.session.read_blocks(&f.model);
        // With the meeting the end is a grant. Without it, the block of this test has no
        // position among the events, so its end cannot be read: the setting is closed
        // too, and the person is told (ADR 0785).
        assert!(f.session.inner.shared().muted);
        assert!(!f.session.controls.microphone_blocked);
        assert!(f.session.controls.muted);
        assert_eq!(
            f.session.inner.view.borrow().closed,
            (!still_presenting).then_some(MicrophoneClosed::EndOfBlockNotRead)
        );
        f.session.engine.close().await;
    }
}

#[tokio::test]
async fn a_model_without_the_current_participant_revokes_admission() {
    let mut f = fixture();
    f.call().participants.clear();
    f.session.read_blocks(&f.model);
    assert!(!f.session.admitted);
    assert!(f.session.controls.microphone_blocked && f.session.controls.sound_blocked);
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_move_present_in_the_refreshed_model_is_started_without_its_notice() {
    let mut f = fixture();
    let call_move = CallMove {
        id: CallMoveId::generate(),
        account_id: f.model.account.id,
        from: f.session.target.place,
        to: CallPlace::Channel {
            workspace_id: WorkspaceId::generate(),
            channel_id: ChannelId::generate(),
        },
        moved_by: AccountId::generate(),
        expires_at: OffsetDateTime::now_utc() + time::Duration::seconds(30),
    };
    f.model.moves.push(call_move.clone());
    f.session.review_move(Some(&f.model));
    assert_eq!(
        f.session.moving.as_ref().map(|moving| moving.call_move.id),
        Some(call_move.id)
    );
    f.session.end_move();
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_meeting_grant_notice_still_requires_unmute_after_a_newer_model_ended_meeting() {
    use junto_client_core::protocol::call::{Meeting, MeetingEnd};
    let mut f = fixture();
    let presenter = AccountId::generate();
    let call_id = f.call().id;
    let place = f.session.target.place;
    let mut blocked = f.call().participants[0].clone();
    blocked.microphone_block = Some(MicrophoneBlock {
        by: presenter,
        reason: BlockReason::Meeting,
    });
    let mut granted = blocked.clone();
    granted.microphone_block = None;
    granted.hand_raised = true;
    f.model.cursor.position = 45;
    f.session.read_blocks(&f.model);
    assert!(!f.session.inner.shared().muted);
    for (id, event) in [
        (
            41,
            Event::CallMeetingStarted {
                call_id,
                place,
                meeting: Meeting {
                    presenter,
                    started_at: OffsetDateTime::UNIX_EPOCH,
                },
            },
        ),
        (
            42,
            Event::CallParticipantUpdated {
                call_id,
                place,
                participant: blocked,
            },
        ),
        (
            43,
            Event::CallParticipantUpdated {
                call_id,
                place,
                participant: granted,
            },
        ),
        (
            44,
            Event::CallMeetingEnded {
                call_id,
                place,
                reason: MeetingEnd::TurnedOff,
            },
        ),
    ] {
        f.session.note(&Notice::Event {
            id,
            at: OffsetDateTime::UNIX_EPOCH,
            event: Arc::new(event),
        });
    }
    assert!(
        f.session.inner.shared().muted,
        "the grant was lost behind the newer model"
    );
    assert!(f.session.controls.muted);
    f.session.engine.close().await;
}

// The end of a microphone block, and the grant of meeting mode that only an event tells
// (ADR 0785).

/// The controls that the engine was told from its request number `from` on.
fn told_from(f: &Fixture, from: usize) -> Vec<Controls> {
    f.session
        .engine
        .asked()
        .into_iter()
        .skip(from)
        .filter_map(|asked| match asked {
            Asked::Controls(controls) => Some(controls),
            Asked::Camera | Asked::Microphone | Asked::Share | Asked::Talking(..) => None,
        })
        .collect()
}

fn lets_the_microphone_send(controls: &Controls) -> bool {
    !controls.muted && !controls.microphone_blocked
}

/// A fixture whose person is inside, not muted, and whose state at 42 shows the block of
/// `reason`. With the reason of meeting mode the state shows the meeting of `by`.
fn blocked(by: AccountId, reason: BlockReason) -> Fixture {
    let mut f = fixture();
    f.session.rejoining = false;
    f.session.apply_controls();
    if reason == BlockReason::Meeting {
        f.call().meeting = Some(Meeting {
            presenter: by,
            started_at: OffsetDateTime::UNIX_EPOCH,
        });
    }
    f.call().participants[0].microphone_block = Some(MicrophoneBlock { by, reason });
    f.model.cursor.position = 42;
    f.session.read_blocks(&f.model);
    assert!(f.session.controls.microphone_blocked && !f.session.controls.muted);
    f
}

/// The state at `position`: no meeting, and the person without a block.
fn state_without_the_block(f: &mut Fixture, position: u64) {
    f.call().meeting = None;
    f.call().participants[0].microphone_block = None;
    f.model.cursor.position = position;
    f.session.read_blocks(&f.model);
}

/// Tells events, and reads the state again as the session does after each notice.
fn tell_and_read(f: &mut Fixture, events: Vec<(u64, Event)>) {
    for event in events {
        tell(f, vec![event]);
        f.session.read_blocks(&f.model);
    }
}

/// The events of the server for a person who is in the call when another person turns
/// meeting mode on at `first` and lets the person speak at `first + 2`. The start of the
/// meeting carries no participant (ADR 0719).
fn a_meeting_and_a_grant(f: &mut Fixture, presenter: AccountId, first: u64) -> Vec<(u64, Event)> {
    let mut events = a_grant(f, presenter, first);
    events.remove(1);
    events
}

fn meeting_ended(f: &mut Fixture, id: u64) -> (u64, Event) {
    let event = Event::CallMeetingEnded {
        call_id: f.call().id,
        place: f.session.target.place,
        reason: MeetingEnd::TurnedOff,
    };
    (id, event)
}

#[tokio::test]
async fn a_grant_stays_closed_before_its_notice() {
    let presenter = AccountId::generate();
    let mut f = blocked(presenter, BlockReason::Meeting);
    let from = f.session.engine.asked().len();
    // The presenter lets the person speak at 43 and ends meeting mode at 44. The state of
    // 44 is read before the notices of these events.
    state_without_the_block(&mut f, 44);
    assert!(
        f.session.controls.microphone_blocked,
        "the state from after the meeting opened the microphone before the notice of the grant"
    );
    assert!(f.session.microphone_block.is_some());
    let mut events = a_meeting_and_a_grant(&mut f, presenter, 41);
    events.push(meeting_ended(&mut f, 44));
    tell_and_read(&mut f, events);
    assert!(
        f.session.inner.shared().muted,
        "the grant did not close the microphone setting"
    );
    assert!(f.session.microphone_block.is_none());
    assert!(f.session.controls.muted && !f.session.controls.microphone_blocked);
    let told = told_from(&f, from);
    assert!(
        !told.iter().any(lets_the_microphone_send),
        "a control let the microphone send before the person opened it: {told:?}"
    );
    // The person opens the microphone.
    f.session.inner.shared().muted = false;
    f.session.command(SessionCommand::Muted(false));
    assert!(lets_the_microphone_send(&f.session.controls));
    f.session.engine.close().await;
}

#[tokio::test]
async fn the_end_of_meeting_mode_opens_the_microphone_when_its_event_is_read() {
    let presenter = AccountId::generate();
    let mut f = blocked(presenter, BlockReason::Meeting);
    state_without_the_block(&mut f, 44);
    assert!(f.session.controls.microphone_blocked);
    // No grant: the events tell the start of the meeting and its end.
    let mut events = a_meeting_and_a_grant(&mut f, presenter, 41);
    events.truncate(1);
    tell_and_read(&mut f, events);
    assert!(f.session.controls.microphone_blocked);
    let ended = meeting_ended(&mut f, 44);
    tell_and_read(&mut f, vec![ended]);
    assert!(
        lets_the_microphone_send(&f.session.controls),
        "the end of meeting mode did not open the microphone of a person who was not muted"
    );
    assert!(!f.session.inner.shared().muted);
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_block_of_a_moderator_ends_when_its_event_is_read() {
    let mut f = blocked(AccountId::generate(), BlockReason::Moderator);
    state_without_the_block(&mut f, 46);
    assert!(f.session.controls.microphone_blocked);
    let (call_id, place) = (f.call().id, f.session.target.place);
    let participant = f.call().participants[0].clone();
    tell_and_read(
        &mut f,
        vec![(
            46,
            Event::CallParticipantUpdated {
                call_id,
                place,
                participant,
            },
        )],
    );
    assert!(lets_the_microphone_send(&f.session.controls));
    assert!(!f.session.inner.shared().muted);
    f.session.engine.close().await;
}

/// The events of the time in which the block ended were not delivered: notices were
/// missed, or a snapshot replaced the state.
#[tokio::test]
async fn a_block_whose_end_was_not_told_closes_the_setting() {
    let missed = |f: &mut Fixture| f.session.missed();
    let reloaded = |f: &mut Fixture| {
        let cursor = Cursor {
            epoch: f.model.cursor.epoch,
            position: 44,
        };
        f.session.note(&Notice::Reloaded { cursor });
    };
    let losses: [&dyn Fn(&mut Fixture); 2] = [&missed, &reloaded];
    for lose in losses {
        // The loss is known before the state without the block, and after it.
        for state_first in [false, true] {
            let mut f = blocked(AccountId::generate(), BlockReason::Meeting);
            let from = f.session.engine.asked().len();
            if state_first {
                state_without_the_block(&mut f, 44);
                assert!(f.session.controls.microphone_blocked);
                lose(&mut f);
                f.session.read_blocks(&f.model);
            } else {
                lose(&mut f);
                state_without_the_block(&mut f, 44);
            }
            assert!(
                f.session.inner.shared().muted,
                "a grant can be among the lost events, and the setting stayed open"
            );
            assert!(f.session.controls.muted && !f.session.controls.microphone_blocked);
            let told = told_from(&f, from);
            assert!(
                !told.iter().any(lets_the_microphone_send),
                "a control let the microphone send: {told:?}"
            );
            f.session.engine.close().await;
        }
    }
}

#[tokio::test]
async fn an_end_that_was_taken_as_a_grant_is_not_told_again_by_the_next_event() {
    // The state after the lost events shows the meeting still, or no meeting.
    for meeting_runs in [false, true] {
        let presenter = AccountId::generate();
        let mut f = blocked(presenter, BlockReason::Meeting);
        // The events told the start of the meeting. A snapshot at 50 replaces the state:
        // the block ended inside it.
        let started = a_meeting_and_a_grant(&mut f, presenter, 41).remove(0);
        tell_and_read(&mut f, vec![started]);
        let cursor = Cursor {
            epoch: f.model.cursor.epoch,
            position: 50,
        };
        f.session.note(&Notice::Reloaded { cursor });
        if meeting_runs {
            f.call().participants[0].microphone_block = None;
            f.model.cursor.position = 50;
            f.session.read_blocks(&f.model);
        } else {
            state_without_the_block(&mut f, 50);
        }
        assert!(f.session.inner.shared().muted && f.session.microphone_block.is_none());
        // The person opens the microphone, and the report of that returns as an event.
        f.session.inner.shared().muted = false;
        f.session.command(SessionCommand::Muted(false));
        let (call_id, place) = (f.call().id, f.session.target.place);
        let participant = f.call().participants[0].clone();
        tell_and_read(
            &mut f,
            vec![(
                51,
                Event::CallParticipantUpdated {
                    call_id,
                    place,
                    participant,
                },
            )],
        );
        assert!(
            !f.session.inner.shared().muted,
            "the event of the person's own report closed the microphone as a grant"
        );
        assert!(lets_the_microphone_send(&f.session.controls));
        f.session.engine.close().await;
    }
}

#[tokio::test]
async fn a_kept_block_ends_at_its_event_while_the_state_is_behind_an_answer() {
    let mut f = blocked(AccountId::generate(), BlockReason::Moderator);
    // The answer to a request for a new token at 45 shows no block, and is read before
    // the event of the end at 43. The state of the stream stays behind the answer.
    f.session.rejoining = true;
    renewed(&mut f);
    assert!(f.session.microphone_block.is_some());
    let (call_id, place) = (f.call().id, f.session.target.place);
    let mut participant = f.call().participants[0].clone();
    participant.microphone_block = None;
    tell(
        &mut f,
        vec![(
            43,
            Event::CallParticipantUpdated {
                call_id,
                place,
                participant,
            },
        )],
    );
    assert!(
        f.session.microphone_block.is_none(),
        "the block waits for a state at the position of the answer"
    );
    assert!(lets_the_microphone_send(&f.session.controls));
    f.session.engine.close().await;
    // Lost events end a kept block too, as a grant.
    let mut f = blocked(AccountId::generate(), BlockReason::Moderator);
    f.session.rejoining = true;
    renewed(&mut f);
    f.session.missed();
    assert!(f.session.microphone_block.is_none() && f.session.inner.shared().muted);
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_seat_on_another_device_from_before_a_sign_of_this_seat_ends_nothing() {
    let other = DeviceId::generate();
    // A place that no state and no answer has shown yet, as after an answer without a
    // position: an event of another device is not after a sign of this seat.
    let mut f = fixture();
    let own = own_device(&f);
    f.session.place =
        Place::new("seat".into(), Instant::now(), Duration::from_secs(10)).on_device(Some(own));
    f.session.rejoining = false;
    let event = another_seat(&f, "seat-of-the-other-device", other);
    tell(&mut f, vec![(39, event)]);
    assert_eq!(
        f.session.read_state(StreamStatus::Live, Some(&f.model)),
        Fate::Held
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn events_after_a_loss_are_read_as_before() {
    let presenter = AccountId::generate();
    let mut f = blocked(presenter, BlockReason::Meeting);
    f.session.missed();
    // The first event after the loss is 43. The state of 44 still shows the block, so
    // its end is after the loss.
    let (call_id, place) = (f.call().id, f.session.target.place);
    let participant = f.call().participants[0].clone();
    tell(
        &mut f,
        vec![(
            43,
            Event::CallParticipantUpdated {
                call_id,
                place,
                participant,
            },
        )],
    );
    f.model.cursor.position = 44;
    f.session.read_blocks(&f.model);
    state_without_the_block(&mut f, 46);
    assert!(f.session.controls.microphone_blocked);
    let ended = meeting_ended(&mut f, 46);
    tell_and_read(&mut f, vec![ended]);
    assert!(lets_the_microphone_send(&f.session.controls));
    assert!(
        !f.session.inner.shared().muted,
        "an end that the events told was taken as a grant"
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_block_that_ended_before_the_notices_were_read_closes_the_setting() {
    // The answer of the join told the block of meeting mode at 45.
    let presenter = AccountId::generate();
    let mut f = fixture();
    f.session.rejoining = false;
    f.session.apply_controls();
    let block = MicrophoneBlock {
        by: presenter,
        reason: BlockReason::Meeting,
    };
    f.session.accepted.position = 45;
    f.session.grants =
        MeetingHistory::new(f.session.accepted, &f.answer.call, f.model.account.id, true);
    f.session.set_blocks((Some(block), None));
    // The session reads the notices from the state of 46 on: the meeting ended at 46, and
    // the notice of that event was given before.
    f.model.cursor.position = 46;
    f.session.subscribed(Some(&f.model));
    f.session.read_blocks(&f.model);
    assert!(
        f.session.microphone_block.is_none(),
        "the block waits for an event that was told before the session read notices"
    );
    assert!(f.session.inner.shared().muted);
    assert!(f.session.controls.muted && !f.session.controls.microphone_blocked);
    f.session.engine.close().await;

    // No event lies between the answer and the first notice: the end is waited for.
    let mut f = blocked(presenter, BlockReason::Meeting);
    f.session.subscribed(Some(&f.model));
    state_without_the_block(&mut f, 44);
    assert!(f.session.microphone_block.is_some() && !f.session.inner.shared().muted);
    f.session.engine.close().await;
}

#[tokio::test]
async fn the_person_who_opens_the_microphone_ends_the_wait_for_the_end_of_a_block() {
    let mut f = blocked(AccountId::generate(), BlockReason::Moderator);
    state_without_the_block(&mut f, 46);
    assert!(f.session.microphone_block.is_some());
    // The person closes and opens the microphone: an action of the person.
    f.session.inner.shared().muted = false;
    f.session.command(SessionCommand::Muted(false));
    assert!(
        f.session.microphone_block.is_none(),
        "the block is kept after the person opened the microphone"
    );
    assert!(lets_the_microphone_send(&f.session.controls));
    f.session.engine.close().await;
}

// A request for a new token that is on its way when the server ends the seat by a rule
// (ADR 0734, ADR 0785).

fn another_seat(f: &Fixture, identity: &str, device_id: DeviceId) -> Event {
    let own = f.model.own_call.as_ref().unwrap();
    Event::OwnCallChanged {
        call: Some(OwnCall {
            identity: identity.into(),
            device_id,
            ..own.clone()
        }),
        left: Some(LeaveReason::Switched),
    }
}

/// The state of the stream at `position`, with the account in no call.
fn state_without_the_seat(f: &mut Fixture, position: u64) -> Fate {
    f.model.cursor.position = position;
    f.model.own_call = None;
    f.session.read_state(StreamStatus::Live, Some(&f.model))
}

/// The answer to the request for a new token names another seat, at `position`.
fn renewed_at_another_seat(f: &mut Fixture, position: u64) {
    f.answer.identity = "seat-2".into();
    f.answer.cursor.as_mut().unwrap().position = position;
    f.answer.call.participants[0].identity = "seat-2".into();
    renewed(f);
}

const FORCED: [Option<LeaveReason>; 6] = [
    Some(LeaveReason::Disconnected),
    Some(LeaveReason::Moved),
    Some(LeaveReason::Switched),
    Some(LeaveReason::AccessEnded),
    Some(LeaveReason::Left),
    None,
];

#[tokio::test]
async fn a_pending_renewal_cannot_undo_a_removal() {
    for reason in FORCED {
        // The fixture has a request for a new token on its way.
        let mut f = fixture();
        assert!(f.session.rejoining);
        assert_eq!(
            f.session.read_state(StreamStatus::Live, Some(&f.model)),
            Fate::Held
        );
        tell(&mut f, vec![(41, ended(reason))]);
        assert_eq!(
            state_without_the_seat(&mut f, 41),
            Fate::End(EndReason::Server(reason)),
            "the end of the seat waited for the answer of the renewal"
        );
        f.session.engine.close().await;
    }
}

#[tokio::test]
async fn a_renewal_continues_after_the_server_ended_a_seat_without_media() {
    for reason in [LeaveReason::ConnectionLost, LeaveReason::NeverConnected] {
        let mut f = fixture();
        assert_eq!(
            f.session.read_state(StreamStatus::Live, Some(&f.model)),
            Fate::Held
        );
        tell(&mut f, vec![(41, ended(Some(reason)))]);
        assert_eq!(state_without_the_seat(&mut f, 41), Fate::Undecided);
        renewed_at_another_seat(&mut f, 43);
        assert_eq!(f.session.place.identity(), "seat-2");
        assert_eq!(f.session.place.void(), None);
        assert_eq!(
            f.session.read_state(StreamStatus::Live, Some(&f.model)),
            Fate::Held,
            "the answer that gave the seat is not the membership"
        );
        f.session.engine.close().await;
    }
}

#[tokio::test]
async fn a_seat_given_by_a_renewal_after_a_removal_is_given_up() {
    for reason in FORCED {
        let mut f = fixture();
        assert_eq!(
            f.session.read_state(StreamStatus::Live, Some(&f.model)),
            Fate::Held
        );
        // The state without the seat is read before the notice of its event, and the
        // answer of the renewal before both: the reason is not known yet.
        renewed_at_another_seat(&mut f, 43);
        assert_eq!(f.session.place.identity(), "seat-2");
        assert_eq!(state_without_the_seat(&mut f, 41), Fate::Held);
        assert!(!f.session.admitted);
        assert!(f.session.controls.microphone_blocked && f.session.controls.sound_blocked);
        // The event from before the answer tells that the server ended the earlier seat.
        tell(&mut f, vec![(41, ended(reason))]);
        assert_eq!(
            f.session.place.void(),
            Some(&EndReason::Server(reason)),
            "the seat that the renewal gave after the end is used"
        );
        assert_eq!(
            f.session.read_state(StreamStatus::Live, Some(&f.model)),
            Fate::End(EndReason::Server(reason))
        );
        // The end of the session gives up the seat of the answer.
        assert_eq!(f.session.place.identity(), "seat-2");
        f.session.engine.close().await;
    }
    // An end because no media connection used the seat is the case of the renewal.
    let mut f = fixture();
    renewed_at_another_seat(&mut f, 43);
    tell(&mut f, vec![(41, ended(Some(LeaveReason::ConnectionLost)))]);
    assert_eq!(f.session.place.void(), None);
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_renewal_does_not_take_the_call_back_from_another_device() {
    let other = DeviceId::generate();
    // The state shows the seat of another device while the request is on its way.
    let mut f = fixture();
    let own = f.model.own_call.as_ref().unwrap().device_id;
    f.session.place =
        Place::new("seat".into(), Instant::now(), Duration::from_secs(10)).on_device(Some(own));
    assert_eq!(
        f.session.read_state(StreamStatus::Live, Some(&f.model)),
        Fate::Held
    );
    f.model.cursor.position = 41;
    let seat = f.model.own_call.as_mut().unwrap();
    seat.identity = "seat-of-the-other-device".into();
    assert_eq!(
        f.session.read_state(StreamStatus::Live, Some(&f.model)),
        Fate::Undecided,
        "a new seat on this device is the one of the request"
    );
    f.model.own_call.as_mut().unwrap().device_id = other;
    assert_eq!(
        f.session.read_state(StreamStatus::Live, Some(&f.model)),
        Fate::End(EndReason::Elsewhere),
        "the call on another device waited for the answer of the renewal"
    );
    f.session.engine.close().await;

    // The answer of the renewal is read before the event of the other device.
    let mut f = fixture();
    f.session.place = Place::new("seat".into(), Instant::now(), Duration::from_secs(10))
        .on_device(Some(own_device(&f)));
    renewed_at_another_seat(&mut f, 43);
    let event = another_seat(&f, "seat-of-the-other-device", other);
    tell(&mut f, vec![(41, event)]);
    assert_eq!(f.session.place.void(), Some(&EndReason::Elsewhere));
    assert_eq!(
        f.session.read_state(StreamStatus::Live, Some(&f.model)),
        Fate::End(EndReason::Elsewhere)
    );
    f.session.engine.close().await;

    // The events of the other device and of the seat of the request are read before the
    // answer: the state shows the seat of the request, on this device.
    let mut f = fixture();
    let own = own_device(&f);
    f.session.place =
        Place::new("seat".into(), Instant::now(), Duration::from_secs(10)).on_device(Some(own));
    assert_eq!(
        f.session.read_state(StreamStatus::Live, Some(&f.model)),
        Fate::Held
    );
    let event = another_seat(&f, "seat-of-the-other-device", other);
    tell(&mut f, vec![(41, event)]);
    f.model.cursor.position = 43;
    f.model.own_call.as_mut().unwrap().identity = "seat-2".into();
    assert_eq!(
        f.session.read_state(StreamStatus::Live, Some(&f.model)),
        Fate::End(EndReason::Elsewhere),
        "the request took the call back from the other device"
    );
    f.session.engine.close().await;

    // An earlier seat of this device, told late, is no other device.
    let mut f = fixture();
    let own = own_device(&f);
    f.session.place =
        Place::new("seat".into(), Instant::now(), Duration::from_secs(10)).on_device(Some(own));
    renewed_at_another_seat(&mut f, 43);
    let event = another_seat(&f, "seat", own);
    tell(&mut f, vec![(41, event)]);
    assert_eq!(f.session.place.void(), None);
    f.session.engine.close().await;
}

fn own_device(f: &Fixture) -> DeviceId {
    f.model.own_call.as_ref().unwrap().device_id
}

#[tokio::test]
async fn a_renewed_seat_is_void_when_events_were_lost() {
    // Notices are missed while the request is on its way, and the state shows no seat:
    // why the seat ended is not known.
    let mut f = fixture();
    assert_eq!(
        f.session.read_state(StreamStatus::Live, Some(&f.model)),
        Fate::Held
    );
    f.session.missed();
    assert_eq!(state_without_the_seat(&mut f, 41), Fate::Undecided);
    renewed_at_another_seat(&mut f, 43);
    assert_eq!(f.session.place.void(), Some(&EndReason::NotRead));
    assert!(!f.session.admitted);
    assert_eq!(
        f.session.read_state(StreamStatus::Live, Some(&f.model)),
        Fate::End(EndReason::NotRead)
    );
    f.session.engine.close().await;

    // Notices are missed before the events have reached the position of the answer.
    let mut f = fixture();
    renewed_at_another_seat(&mut f, 43);
    assert_eq!(f.session.place.void(), None);
    f.session.missed();
    assert_eq!(f.session.place.void(), Some(&EndReason::NotRead));
    f.session.engine.close().await;

    // A state that shows the seat after the loss tells that the seat was not ended.
    let mut f = fixture();
    f.session.missed();
    assert_eq!(
        f.session.read_state(StreamStatus::Live, Some(&f.model)),
        Fate::Held
    );
    tell(&mut f, vec![(41, ended(Some(LeaveReason::ConnectionLost)))]);
    assert_eq!(state_without_the_seat(&mut f, 41), Fate::Undecided);
    renewed_at_another_seat(&mut f, 43);
    assert_eq!(f.session.place.void(), None);
    f.session.engine.close().await;

    // Events after the position of the answer were read: the loss is after them.
    let mut f = fixture();
    renewed_at_another_seat(&mut f, 43);
    let (call_id, place) = (f.call().id, f.session.target.place);
    let participant = f.call().participants[0].clone();
    tell(
        &mut f,
        vec![(
            44,
            Event::CallParticipantUpdated {
                call_id,
                place,
                participant,
            },
        )],
    );
    f.session.missed();
    assert_eq!(f.session.place.void(), None);
    f.session.engine.close().await;
}

// An action that opens a microphone, a camera or a share names its call and seat
// (ADR 0788).

fn a_share() -> CaptureRequest {
    CaptureRequest {
        picture: crate::share::PictureSource::Display(1),
        sound: crate::share::SoundChoice::System,
        quality: crate::share::ShareQuality::default(),
    }
}

fn unused(f: &Fixture) -> Option<Unused> {
    let unused = f.session.inner.view.borrow().unused;
    f.session
        .inner
        .view
        .send_if_modified(|view| view.unused.take().is_some());
    unused
}

#[tokio::test]
async fn an_action_for_another_seat_opens_nothing() {
    let mut f = fixture();
    let held = Seat {
        target: f.session.target,
        identity: "seat".into(),
    };
    let earlier_seat = Seat {
        identity: "earlier-seat".into(),
        ..held.clone()
    };
    let other_call = Seat {
        target: CallTarget {
            server_id: ServerId::generate(),
            ..held.target
        },
        ..held.clone()
    };
    f.session.inner.shared().muted = true;
    f.session.apply_controls();
    let asked = f.session.engine.asked();
    for stale in [earlier_seat, other_call] {
        f.session
            .command(SessionCommand::StartShare(stale.clone(), a_share()));
        assert_eq!(unused(&f), Some(Unused::Share));
        f.session
            .command(SessionCommand::StartCamera(stale.clone(), None));
        assert_eq!(unused(&f), Some(Unused::Camera));
        f.session
            .command(SessionCommand::RetryMicrophone(stale.clone()));
        assert_eq!(unused(&f), Some(Unused::Microphone));
        f.session.command(SessionCommand::Unmute(stale, 0));
        assert_eq!(unused(&f), Some(Unused::Microphone));
        assert!(
            f.session.inner.shared().muted,
            "the microphone setting was opened for another seat"
        );
    }
    assert_eq!(
        f.session.engine.asked(),
        asked,
        "the engine was asked for something for another seat"
    );
    // The same actions for the seat that the session holds.
    f.session
        .command(SessionCommand::StartShare(held.clone(), a_share()));
    f.session
        .command(SessionCommand::StartCamera(held.clone(), None));
    f.session
        .command(SessionCommand::RetryMicrophone(held.clone()));
    // An open that was asked for before the last close of the person opens nothing.
    let calls = Calls {
        inner: f.session.inner.clone(),
    };
    let asked_at = f.session.inner.shared().closes;
    calls.mute();
    f.session
        .command(SessionCommand::Unmute(held.clone(), asked_at));
    assert!(
        f.session.inner.shared().muted && calls.view().muted,
        "an open that was on its way undid the close"
    );
    assert_eq!(f.session.engine.asked().len(), asked.len() + 3);
    let asked_at = f.session.inner.shared().closes;
    f.session.command(SessionCommand::Unmute(held, asked_at));
    assert_eq!(unused(&f), None);
    assert!(!f.session.inner.shared().muted);
    let after = f.session.engine.asked();
    assert_eq!(
        after[asked.len()..after.len() - 1],
        [Asked::Share, Asked::Camera, Asked::Microphone]
    );
    assert!(matches!(
        after.last(),
        Some(Asked::Controls(controls)) if !controls.muted
    ));
    f.session.engine.close().await;
}

#[tokio::test]
async fn an_unmute_without_a_call_is_for_no_call() {
    let f = fixture();
    let calls = Calls {
        inner: f.session.inner.clone(),
    };
    calls.mute();
    // The control was drawn without a call, and no call is active: the setting opens.
    calls.unmute_in(None);
    assert!(!calls.view().muted);
    calls.mute();
    // A call began after the control was drawn.
    let (commands, _taken) = mpsc::unbounded_channel();
    f.session.inner.shared().active = Some(ActiveHandle {
        key: 1,
        target: f.session.target,
        commands,
        gate: Arc::default(),
    });
    calls.unmute_in(None);
    assert!(calls.view().muted && f.session.inner.shared().muted);
    assert_eq!(unused(&f), Some(Unused::Microphone));
    // The call for which the control was drawn has ended.
    f.session.inner.shared().active = None;
    let seat = Seat {
        target: f.session.target,
        identity: "seat".into(),
    };
    calls.unmute_in(Some(&seat));
    assert!(calls.view().muted);
    assert_eq!(unused(&f), None);
    f.session.engine.close().await;
}

// A seat that a request for a new token gave, before the events up to its answer are
// read (ADR 0785); a removal by the media server (ADR 0734); a microphone on another
// device (ADR 0794).

#[tokio::test]
async fn a_seat_given_by_a_renewal_sends_nothing_before_the_events_reach_its_answer() {
    for removed in [false, true] {
        let mut f = fixture();
        renewed_at_another_seat(&mut f, 43);
        // A state lists the seat before the session has read the events up to 43.
        f.model.cursor.position = 44;
        f.model.own_call.as_mut().unwrap().identity = "seat-2".into();
        f.call().participants[0].identity = "seat-2".into();
        f.session.read_blocks(&f.model);
        assert!(
            !f.session.admitted,
            "a seat that can still be void was admitted"
        );
        let held = f.session.controls;
        assert!(held.microphone_blocked && held.sound_blocked && held.camera_held);
        if removed {
            // The events tell that a moderator removed the person before the answer.
            tell(&mut f, vec![(41, ended(Some(LeaveReason::Disconnected)))]);
            f.session.read_blocks(&f.model);
            assert!(!f.session.admitted && f.session.controls.camera_held);
            assert!(f.session.place.void().is_some());
        } else {
            // The events pass the answer: the seat is the one of the person.
            let (call_id, place) = (f.call().id, f.session.target.place);
            let participant = f.call().participants[0].clone();
            tell(
                &mut f,
                vec![(
                    44,
                    Event::CallParticipantJoined {
                        call_id,
                        place,
                        participant,
                    },
                )],
            );
            f.session.read_blocks(&f.model);
            assert!(f.session.admitted);
            let open = f.session.controls;
            assert!(!open.microphone_blocked && !open.sound_blocked && !open.camera_held);
        }
        f.session.engine.close().await;
    }
}

#[tokio::test]
async fn a_removal_by_the_media_server_waits_for_a_new_state_before_a_token() {
    for (lost, asks) in [(Lost::Failed, true), (Lost::Removed, false)] {
        let mut f = fixture();
        f.session.rejoining = false;
        f.session.fate = f.session.read_state(StreamStatus::Live, Some(&f.model));
        assert_eq!(f.session.fate, Fate::Held);
        let mut state = f.session.engine.state().borrow().clone();
        state.connection = Connection::Lost(lost);
        f.session.engine_changed(&state);
        assert_eq!(
            asks_for_a_token(&mut f),
            asks,
            "after {lost:?} with a state from before the loss"
        );
        // A state from after the loss is asked for, and nothing else is.
        assert_eq!(f.session.refresh.is_some(), !asks);
        f.session.refresh = None;
        if lost == Lost::Removed {
            // One state per loss: a later change of the engine asks for no second one.
            f.session.rejoining = false;
            f.session.engine_changed(&state);
            assert!(f.session.refresh.is_none());
            // The state from after the loss shows the place: the token is asked for.
            let cursor = Cursor {
                epoch: f.model.cursor.epoch,
                position: 45,
            };
            f.model.cursor = cursor;
            f.session.reloaded_at(cursor);
            f.session.fate = f.session.read_state(StreamStatus::Live, Some(&f.model));
            assert_eq!(f.session.fate, Fate::Held);
            assert!(asks_for_a_token(&mut f));
            // The token is there, and the engine tells the same loss once more before it
            // takes the token: no further state is asked for.
            f.session.rejoining = false;
            f.session.retry = None;
            f.session.engine_changed(&state);
            assert!(
                f.session.refresh.is_none(),
                "the same loss asked for a second state"
            );
            // A removal after another connection is another loss.
            state.connection = Connection::Connected;
            state.attempt += 1;
            f.session.engine_changed(&state);
            state.connection = Connection::Lost(Lost::Removed);
            f.session.engine_changed(&state);
            assert!(f.session.refresh.is_some());
            f.session.refresh = None;
        }
        f.session.engine.close().await;
    }
}

/// The session sees the last state of the engine only. When the engine takes the token, its
/// next connection is made and the media server removes it again before the session reads
/// a state in between, the second removal is another loss: it asks for a state of its own
/// before the next token.
#[tokio::test]
async fn a_removal_of_the_next_media_connection_asks_for_a_state_of_its_own() {
    let mut f = fixture();
    f.session.rejoining = false;
    f.session.fate = f.session.read_state(StreamStatus::Live, Some(&f.model));
    let mut state = f.session.engine.state().borrow().clone();
    state.connection = Connection::Lost(Lost::Removed);
    f.session.engine_changed(&state);
    assert!(f.session.refresh.is_some());
    f.session.refresh = None;
    // The state from after the loss shows the place: the token is asked for.
    let cursor = Cursor {
        epoch: f.model.cursor.epoch,
        position: 45,
    };
    f.model.cursor = cursor;
    f.session.reloaded_at(cursor);
    f.session.fate = f.session.read_state(StreamStatus::Live, Some(&f.model));
    assert_eq!(f.session.fate, Fate::Held);
    assert!(asks_for_a_token(&mut f));
    // The engine connects with the token, and the media server removes that connection.
    f.session.rejoining = false;
    f.session.retry = None;
    state.attempt += 1;
    f.session.engine_changed(&state);
    assert!(
        f.session.refresh.is_some(),
        "the removal of the next connection was taken as the one before"
    );
    assert!(!asks_for_a_token(&mut f));
    f.session.refresh = None;
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_removal_that_the_events_tell_ends_the_call_with_its_reason_while_a_state_is_loaded() {
    use junto_client_core::flow::access::AccessState;
    let removed = Some(LeaveReason::Disconnected);
    // The media server removed the device, and the session asks for a new state.
    let lose = |f: &mut Fixture| {
        f.session.rejoining = false;
        f.session.fate = f.session.read_state(StreamStatus::Live, Some(&f.model));
        let mut state = f.session.engine.state().borrow().clone();
        state.connection = Connection::Lost(Lost::Removed);
        f.session.engine_changed(&state);
        assert!(f.session.refresh.is_some());
    };

    // The event of the removal is read while the state is loaded: the call ends at once.
    let mut f = fixture();
    lose(&mut f);
    tell(&mut f, vec![(41, ended(removed))]);
    let logged_in = AccessState::LoggedIn {
        account_id: f.model.account.id,
        device_id: own_device(&f),
        session_expires_at: OffsetDateTime::UNIX_EPOCH,
        recoveries: Vec::new(),
        previous_account: None,
    };
    assert_eq!(
        f.session.read_as(None, &logged_in),
        Fate::End(EndReason::Server(removed)),
        "the end that the events told waits for the new state"
    );
    f.session.refresh = None;
    f.session.engine.close().await;

    // The new state arrives after the event: the reason of the end is the one of the event.
    let mut f = fixture();
    lose(&mut f);
    f.session.place.note(41, &ended(removed));
    let cursor = Cursor {
        epoch: f.model.cursor.epoch,
        position: 45,
    };
    f.session.reloaded_at(cursor);
    f.model.cursor = cursor;
    f.model.own_call = None;
    assert_eq!(
        f.session.read_state(StreamStatus::Live, Some(&f.model)),
        Fate::End(EndReason::Server(removed)),
        "the new state erased the reason of the end"
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_microphone_on_another_device_closes_the_setting_and_is_told() {
    let mut f = fixture();
    f.session.rejoining = false;
    f.session.apply_controls();
    let mut state = f.session.engine.state().borrow().clone();
    state.input_replaced = true;
    f.session.engine_changed(&state);
    assert!(f.session.inner.shared().muted && f.session.controls.muted);
    let view = f.session.inner.view.borrow().clone();
    assert!(view.muted);
    assert_eq!(view.closed, Some(MicrophoneClosed::DeviceChanged));
    f.session.engine.close().await;
}

#[test]
fn a_close_from_outside_the_engine_task_stops_the_frames_until_the_engine_has_taken_it() {
    let link = CaptureLink::default();
    link.set_open(true);
    assert!(link.is_open());
    // The person closes the microphone: the frames stop before the engine task runs.
    let closed_at = link.stop_now();
    assert!(!link.is_open());
    // The engine task is busy with an older command and opens its gate again.
    link.set_open(false);
    link.set_open(true);
    link.take_stops(closed_at - 1);
    assert!(
        !link.is_open(),
        "what the engine held before the close opened the gate"
    );
    // It takes the command of the close, and then a later open.
    link.set_open(false);
    link.take_stops(closed_at);
    assert!(!link.is_open());
    link.set_open(true);
    link.take_stops(link.stops());
    assert!(link.is_open());
}

#[test]
fn controls_that_open_do_not_open_after_a_close_that_followed_their_reading() {
    let link = CaptureLink::default();
    // The session reads "not muted" and the count of closes for its controls. The person
    // closes the microphone before the controls reach the engine.
    let read = link.stops();
    let closed_at = link.stop_now();
    let carried = link.count_for(false, read);
    // The engine task takes the controls of before the close and opens its gate.
    link.set_open(true);
    link.take_stops(carried);
    assert!(
        !link.is_open(),
        "controls from before a close opened the gate after it"
    );
    // The controls of the close itself carry its count, and later ones open.
    assert_eq!(link.count_for(true, read), closed_at + 1);
    link.take_stops(link.count_for(false, link.stops()));
    assert!(link.is_open());
}

#[tokio::test]
async fn closing_the_microphone_stops_the_gate_of_the_call_at_once() {
    let mut f = fixture();
    f.session.rejoining = false;
    let gate = f.session.engine.gate();
    let calls = Calls {
        inner: f.session.inner.clone(),
    };
    let (commands, _taken) = mpsc::unbounded_channel();
    f.session.inner.shared().active = Some(ActiveHandle {
        key: 1,
        target: f.session.target,
        commands,
        gate: gate.clone(),
    });
    let before = gate.stops();
    calls.mute();
    assert!(
        gate.stops() > before,
        "the close of the person waits for the session and the engine"
    );
    // A rule of the server that closes the microphone does not wait either.
    let before = gate.stops();
    f.session.inner.shared().muted = false;
    f.session
        .set_blocks((Some(a_block(AccountId::generate())), None));
    assert!(gate.stops() > before);
    // An open is the direction that waits for the engine.
    let before = gate.stops();
    f.session.set_blocks((None, None));
    assert_eq!(gate.stops(), before);
    // The release of the push-to-talk key is a close; the press is an open.
    calls.set_talking(true);
    assert_eq!(gate.stops(), before);
    calls.set_talking(false);
    assert!(
        gate.stops() > before,
        "the release of the key waits for the session and the engine"
    );
    f.session.inner.shared().active = None;
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_press_of_the_talk_key_does_not_open_after_its_release() {
    let mut f = fixture();
    f.session.rejoining = false;
    let gate = f.session.engine.gate();
    let calls = Calls {
        inner: f.session.inner.clone(),
    };
    let (commands, mut told) = mpsc::unbounded_channel();
    f.session.inner.shared().active = Some(ActiveHandle {
        key: 1,
        target: f.session.target,
        commands,
        gate: gate.clone(),
    });
    // The person presses the key and releases it before the session has taken the press.
    let before = gate.stops();
    calls.set_talking(true);
    calls.set_talking(false);
    let released = gate.stops();
    assert!(released > before);
    let from = f.session.engine.asked().len();
    while let Ok(command) = told.try_recv() {
        f.session.command(command);
    }
    // The press reaches the engine with the count from before the release, so the gate
    // that the engine opens for it stays closed. The release carries its own count.
    assert_eq!(
        f.session.engine.asked()[from..],
        [
            Asked::Talking(true, before),
            Asked::Talking(false, released)
        ],
        "the press carries a count that was read after the release"
    );
    gate.set_open(true);
    gate.take_stops(before);
    assert!(!gate.is_open());
    f.session.inner.shared().active = None;
    f.session.engine.close().await;
}

// The connection of a device to the room of the destination of a move (ADR 0511).

#[tokio::test]
async fn a_move_that_ended_starts_no_connection_with_a_late_token() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let connects = Arc::new(AtomicUsize::new(0));
    let counting = |connects: &Arc<AtomicUsize>| {
        let connects = connects.clone();
        async move |_token: &str| {
            connects.fetch_add(1, Ordering::SeqCst);
            Some(())
        }
    };

    // The request for the token waits. The move ends, and the answer arrives after it.
    let (stop, stopped) = oneshot::channel::<()>();
    let (answer, token) = oneshot::channel::<&str>();
    let work = tokio::spawn(probe(
        stopped,
        async move { token.await.ok() },
        counting(&connects),
        async |()| {},
    ));
    tokio::task::yield_now().await;
    drop(stop);
    let ended = tokio::time::timeout(Duration::from_secs(5), work).await;
    assert_eq!(
        ended.expect("the work waits for the token").unwrap(),
        Probed::Stopped
    );
    assert!(
        answer.send("token").is_err(),
        "the late answer is waited for"
    );
    assert_eq!(connects.load(Ordering::SeqCst), 0);

    // The answer is there in the moment in which the move ends.
    let (stop, stopped) = oneshot::channel::<()>();
    drop(stop);
    let result = probe(
        stopped,
        async { Some("token") },
        counting(&connects),
        async |()| {},
    )
    .await;
    assert_eq!(result, Probed::Stopped);
    assert_eq!(connects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn a_move_that_ends_while_its_connection_is_made_closes_that_connection() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let closed = Arc::new(AtomicUsize::new(0));
    let (stop, stopped) = oneshot::channel::<()>();
    let (started, connecting) = oneshot::channel::<()>();
    let (made, connected) = oneshot::channel::<()>();
    let closes = closed.clone();
    let work = tokio::spawn(probe(
        stopped,
        async { Some("token") },
        async move |_token: &str| {
            let _ = started.send(());
            connected.await.ok()
        },
        async move |()| {
            closes.fetch_add(1, Ordering::SeqCst);
        },
    ));
    connecting.await.unwrap();
    drop(stop);
    tokio::task::yield_now().await;
    assert!(
        !work.is_finished(),
        "a connection that is being made was cut off"
    );
    made.send(()).unwrap();
    assert_eq!(work.await.unwrap(), Probed::Closed);
    assert_eq!(closed.load(Ordering::SeqCst), 1);

    // No token and no connection are failures of the move.
    let (_stop, stopped) = oneshot::channel::<()>();
    let none = async { None::<&str> };
    let result = probe(stopped, none, async |_| Some(()), async |()| {}).await;
    assert_eq!(result, Probed::Failed);
    let (_stop, stopped) = oneshot::channel::<()>();
    let token = async { Some("token") };
    let result = probe(stopped, token, async |_| None::<()>, async |()| {}).await;
    assert_eq!(result, Probed::Failed);
}

// The loop of a session, with a state and notices that the test gives to the client core.

struct Looping {
    calls: Calls,
    live: junto_client_core::live::LiveServer,
    commands: mpsc::UnboundedSender<SessionCommand>,
    task: tokio::task::JoinHandle<()>,
    _directory: tempfile::TempDir,
}

/// Starts the loop of the session of a fixture. The device is logged in, and the event
/// stream is live with the state of the fixture.
fn looping(mut f: Fixture) -> Looping {
    use junto_client_core::flow::access::AccessState;
    f.session.server.set_access_for_test(AccessState::LoggedIn {
        account_id: f.model.account.id,
        device_id: own_device(&f),
        session_expires_at: OffsetDateTime::UNIX_EPOCH,
        recoveries: Vec::new(),
        previous_account: None,
    });
    let live = f.session.server.live();
    live.show_for_test(StreamStatus::Live, Some(f.model.clone()));
    let (commands, taken) = mpsc::unbounded_channel();
    f.session.inner.shared().active = Some(ActiveHandle {
        key: f.session.key,
        target: f.session.target,
        commands: commands.clone(),
        gate: f.session.engine.gate(),
    });
    f.session.review = true;
    let calls = Calls {
        inner: f.session.inner.clone(),
    };
    let task = f.session.inner.runtime.clone().spawn(f.session.run(taken));
    Looping {
        calls,
        live,
        commands,
        task,
        _directory: f._directory,
    }
}

impl Looping {
    async fn until(&self, what: &str, holds: impl FnMut(&CallsView) -> bool) -> CallsView {
        let mut watch = self.calls.watch();
        match tokio::time::timeout(Duration::from_secs(20), watch.wait_for(holds)).await {
            Ok(Ok(view)) => view.clone(),
            _ => panic!(
                "this did not hold: {what}; the view is {:?}",
                self.calls.view()
            ),
        }
    }

    async fn end(self) {
        let (done, ended) = oneshot::channel();
        let _ = self.commands.send(SessionCommand::End { done });
        let _ = ended.await;
        let _ = self.task.await;
    }
}

/// The end of a call waits for the work of a move to close the connection that it made to
/// the room of the destination, so that what follows the end can use the same identity
/// there, and it waits at most `CallTiming::move_end`.
#[tokio::test]
async fn the_end_of_a_call_waits_for_the_work_of_a_move_within_its_limit() {
    // Work that closes its connection, and work that does not end.
    for closes in [true, false] {
        let mut f = fixture();
        f.session.rejoining = false;
        let limit = if closes {
            Duration::from_secs(60)
        } else {
            Duration::from_millis(500)
        };
        f.session.timing.move_end = limit;
        let (close, closed) = oneshot::channel::<()>();
        let work = f.session.inner.runtime.spawn(async move {
            let _ = closed.await;
        });
        f.session.probes.push(work);
        let looping = looping(f);
        let started = Instant::now();
        let (done, mut ended) = oneshot::channel();
        looping.commands.send(SessionCommand::End { done }).unwrap();
        looping
            .until("the call has ended for the person", |view| {
                view.call.is_none()
            })
            .await;
        let waited = tokio::time::timeout(Duration::from_millis(200), &mut ended).await;
        assert!(
            waited.is_err(),
            "the end did not wait for the work of the move"
        );
        if closes {
            close.send(()).unwrap();
        }
        let identity = tokio::time::timeout(Duration::from_secs(20), ended)
            .await
            .unwrap_or_else(|_| panic!("the end waits after the work of the move has ended"))
            .unwrap();
        assert_eq!(identity, "seat");
        if !closes {
            assert!(started.elapsed() >= limit);
        }
        let _ = looping.task.await;
    }
}

#[tokio::test]
async fn the_loop_of_a_session_marks_the_events_before_its_first_notice_as_not_told() {
    // The answer of the join told the block of meeting mode at 45. The meeting ended at
    // 46, before the session read notices: the state of the stream is the one of 46.
    let mut f = fixture();
    f.session.rejoining = false;
    f.session.accepted.position = 45;
    f.session.grants =
        MeetingHistory::new(f.session.accepted, &f.answer.call, f.model.account.id, true);
    f.session.microphone_block = Some(MicrophoneBlock {
        by: AccountId::generate(),
        reason: BlockReason::Meeting,
    });
    f.model.cursor.position = 46;
    let looping = looping(f);
    let view = looping
        .until("the end of the block is taken as not read", |view| {
            view.closed.is_some()
        })
        .await;
    assert!(view.muted);
    assert_eq!(view.closed, Some(MicrophoneClosed::EndOfBlockNotRead));
    assert_eq!(view.call.unwrap().microphone_block, None);
    looping.end().await;
}

#[tokio::test]
async fn the_loop_of_a_session_takes_missed_notices_as_events_that_were_not_told() {
    let presenter = AccountId::generate();
    let mut f = blocked(presenter, BlockReason::Meeting);
    let (call_id, place) = (f.call().id, f.session.target.place);
    // The state after the block: no meeting and no block.
    f.call().meeting = None;
    f.call().participants[0].microphone_block = None;
    f.model.cursor.position = 44;
    let after = f.model.clone();
    // The loop starts with the state of 42, which shows the block.
    f.call().participants[0].microphone_block = Some(MicrophoneBlock {
        by: presenter,
        reason: BlockReason::Meeting,
    });
    f.model.cursor.position = 42;
    let looping = looping(f);
    // The loop runs and has its notices: it answers a command.
    let stale = Seat {
        target: CallTarget {
            server_id: ServerId::generate(),
            place,
        },
        identity: "seat".into(),
    };
    looping
        .commands
        .send(SessionCommand::StartShare(stale, a_share()))
        .unwrap();
    looping
        .until("the loop runs", |view| view.unused.is_some())
        .await;

    // The test holds the lock of what the calls share. The loop takes that lock when it
    // reads the state after a notice, so it reads at most one notice while the stream
    // gives the state of 44 and more notices than a reader may leave unread.
    let inner = looping.calls.inner.clone();
    let held = inner.shared();
    looping.live.show_for_test(StreamStatus::Live, Some(after));
    for id in 0..1_200 {
        looping.live.tell_for_test(Notice::Event {
            id: 1_000 + id,
            at: OffsetDateTime::UNIX_EPOCH,
            event: Arc::new(Event::CallSpeaking {
                call_id,
                place,
                account_ids: Vec::new(),
            }),
        });
    }
    drop(held);

    // A grant can be among the missed notices: the setting is closed, and the person is
    // told why.
    let view = looping
        .until("the missed notices are taken as lost events", |view| {
            view.closed.is_some()
        })
        .await;
    assert!(view.muted);
    assert_eq!(view.closed, Some(MicrophoneClosed::EndOfBlockNotRead));
    assert_eq!(view.call.unwrap().microphone_block, None);
    looping.end().await;
}

// The work of a move, the grants of meeting mode across a new history and a delayed
// model, the account of a session, an answer without a position, and the answer of a
// renewal as the membership of the person (ADR 0785).

#[tokio::test]
async fn renewal_waits_for_its_answer_when_the_old_seat_ends() {
    let f = fixture();
    let mut place = Place::new("seat".into(), Instant::now(), Duration::from_secs(10));
    assert_eq!(
        place.fate(
            Some((f.model.cursor, f.model.own_call.as_ref())),
            false,
            Instant::now()
        ),
        Fate::Held
    );
    place.note(
        41,
        &Event::OwnCallChanged {
            call: None,
            left: Some(LeaveReason::ConnectionLost),
        },
    );
    let ended = Cursor {
        epoch: f.model.cursor.epoch,
        position: 41,
    };
    // The answer to the request for a new token can name a new seat.
    assert_eq!(
        place.fate(Some((ended, None)), true, Instant::now()),
        Fate::Undecided
    );
    // Without such a request the end of the seat ends the call.
    assert_eq!(
        place.fate(Some((ended, None)), false, Instant::now()),
        Fate::End(EndReason::Server(Some(LeaveReason::ConnectionLost)))
    );
    f.session.engine.close().await;
}

fn a_move(f: &Fixture) -> CallMove {
    CallMove {
        id: CallMoveId::generate(),
        account_id: f.model.account.id,
        from: f.session.target.place,
        to: CallPlace::Channel {
            workspace_id: WorkspaceId::generate(),
            channel_id: ChannelId::generate(),
        },
        moved_by: AccountId::generate(),
        expires_at: OffsetDateTime::now_utc() + time::Duration::seconds(30),
    }
}

#[tokio::test]
async fn the_same_move_is_not_started_twice_and_an_expired_move_is_not_started() {
    let mut f = fixture();
    let call_move = a_move(&f);
    f.session.start_move(&call_move);
    let deadline = f.session.moving.as_ref().unwrap().deadline;
    tokio::time::sleep(Duration::from_millis(5)).await;
    f.session.start_move(&call_move);
    assert_eq!(
        f.session.moving.as_ref().unwrap().deadline,
        deadline,
        "the same move was replaced"
    );
    f.session.end_move();
    let mut expired = a_move(&f);
    expired.expires_at = OffsetDateTime::now_utc() - time::Duration::seconds(1);
    f.session.start_move(&expired);
    assert!(f.session.moving.is_none(), "an expired move was started");
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_replacement_move_is_started_in_the_same_review() {
    let mut f = fixture();
    let first = a_move(&f);
    f.session.start_move(&first);
    f.session.moving.as_mut().unwrap().reloaded = true;
    let second = a_move(&f);
    f.model.moves.push(second.clone());
    f.session.review_move(Some(&f.model));
    assert_eq!(
        f.session.moving.as_ref().map(|moving| moving.call_move.id),
        Some(second.id)
    );
    f.session.end_move();
    f.session.engine.close().await;
}

/// The events of a meeting in which the presenter blocks the person at `first` and lets
/// the person speak at `first + 2`.
fn a_grant(f: &mut Fixture, presenter: AccountId, first: u64) -> Vec<(u64, Event)> {
    use junto_client_core::protocol::call::Meeting;
    let call_id = f.call().id;
    let place = f.session.target.place;
    let mut blocked = f.call().participants[0].clone();
    blocked.microphone_block = Some(MicrophoneBlock {
        by: presenter,
        reason: BlockReason::Meeting,
    });
    let mut granted = blocked.clone();
    granted.microphone_block = None;
    vec![
        (
            first,
            Event::CallMeetingStarted {
                call_id,
                place,
                meeting: Meeting {
                    presenter,
                    started_at: OffsetDateTime::UNIX_EPOCH,
                },
            },
        ),
        (
            first + 1,
            Event::CallParticipantUpdated {
                call_id,
                place,
                participant: blocked,
            },
        ),
        (
            first + 2,
            Event::CallParticipantUpdated {
                call_id,
                place,
                participant: granted,
            },
        ),
    ]
}

fn tell(f: &mut Fixture, events: Vec<(u64, Event)>) {
    for (id, event) in events {
        f.session.note(&Notice::Event {
            id,
            at: OffsetDateTime::UNIX_EPOCH,
            event: Arc::new(event),
        });
    }
}

#[tokio::test]
async fn a_new_history_does_not_compare_its_grants_with_old_positions() {
    let mut f = fixture();
    let presenter = AccountId::generate();
    // The server starts another history, whose positions are lower than the ones before.
    f.session.note(&Notice::Reloaded {
        cursor: Cursor {
            epoch: StreamEpoch::generate(),
            position: 5,
        },
    });
    let events = a_grant(&mut f, presenter, 6);
    tell(&mut f, events);
    assert!(f.session.inner.shared().muted);
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_delayed_grant_does_not_undo_an_explicit_unmute_after_its_model() {
    use junto_client_core::protocol::call::Meeting;
    let mut f = fixture();
    let presenter = AccountId::generate();
    f.session.set_blocks((
        Some(MicrophoneBlock {
            by: presenter,
            reason: BlockReason::Meeting,
        }),
        None,
    ));
    // The model holds the grant already: the meeting runs and the block is gone.
    f.call().meeting = Some(Meeting {
        presenter,
        started_at: OffsetDateTime::UNIX_EPOCH,
    });
    f.model.cursor.position = 46;
    f.session.read_blocks(&f.model);
    assert!(f.session.inner.shared().muted);
    // The person opens the microphone, as `Calls::set_muted` does.
    f.session.inner.shared().muted = false;
    f.session.command(SessionCommand::Muted(false));
    assert!(!f.session.controls.muted);
    // The events of the same grant arrive after it.
    let events = a_grant(&mut f, presenter, 41);
    tell(&mut f, events);
    assert!(
        !f.session.inner.shared().muted,
        "the delayed grant closed the microphone that the person had opened"
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn losing_the_account_ends_a_call_even_while_a_refresh_is_pending() {
    let mut f = fixture();
    // The device of the fixture has no session on its server.
    f.session.refresh = Some(Box::pin(std::future::pending::<Cursor>()));
    assert_eq!(f.session.read_server(), Fate::End(EndReason::Server(None)));
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_hidden_call_meeting_block_supplies_the_grant_history() {
    let mut f = fixture();
    let presenter = AccountId::generate();
    // A call that hides who is inside tells its meeting to nobody outside (ADR 0517): the
    // session has no admission yet and has read no start of a meeting.
    f.call().hides_occupants = true;
    let own = f.call().participants[0].clone();
    f.call().participants.clear();
    f.session.read_blocks(&f.model);
    assert!(!f.session.admitted);
    let call_id = f.call().id;
    let place = f.session.target.place;
    let mut blocked = own.clone();
    blocked.microphone_block = Some(MicrophoneBlock {
        by: presenter,
        reason: BlockReason::Meeting,
    });
    tell(
        &mut f,
        vec![
            (
                46,
                Event::CallParticipantJoined {
                    call_id,
                    place,
                    participant: blocked,
                },
            ),
            (
                47,
                Event::CallParticipantUpdated {
                    call_id,
                    place,
                    participant: own,
                },
            ),
        ],
    );
    assert!(
        f.session.inner.shared().muted,
        "a hidden call need not send meeting_started before its admission snapshot"
    );
    assert!(!f.session.admitted);
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_new_seat_seen_before_its_answer_keeps_the_newer_blocks() {
    let mut f = fixture();
    // The stream tells the new seat, with a block, before the answer that gave it.
    f.call().participants[0].identity = "replacement".into();
    f.call().participants[0].microphone_block = Some(MicrophoneBlock {
        by: AccountId::generate(),
        reason: BlockReason::Moderator,
    });
    f.model.cursor.position = 50;
    f.session.read_blocks(&f.model);
    let (call_id, place) = (f.call().id, f.session.target.place);
    let participant = f.call().participants[0].clone();
    tell(
        &mut f,
        vec![(
            50,
            Event::CallParticipantJoined {
                call_id,
                place,
                participant,
            },
        )],
    );
    f.answer.identity = "replacement".into();
    let (taken, _) = oneshot::channel();
    f.session
        .finished(Done::Rejoined(Box::new(Ok(f.answer.clone())), taken));
    assert_eq!(f.session.place.identity(), "replacement");
    f.session.read_blocks(&f.model);
    assert!(f.session.microphone_block.is_some());
    assert!(
        f.session.controls.microphone_blocked,
        "the older answer removed the block of the newer model"
    );
    assert!(f.session.admitted);
    f.session.engine.close().await;
}

#[test]
fn a_session_rotation_is_not_a_logout_or_an_account_replacement() {
    use junto_client_core::flow::access::AccessState;
    let expected = AccountId::generate();
    let logged_in = |account_id| AccessState::LoggedIn {
        account_id,
        device_id: DeviceId::generate(),
        session_expires_at: OffsetDateTime::UNIX_EPOCH,
        recoveries: Vec::new(),
        previous_account: None,
    };
    assert!(
        !account_ended(None, &logged_in(expected), expected),
        "session rotation preserves the logged-in account while its token is absent"
    );
    assert!(!account_ended(
        Some(expected),
        &logged_in(expected),
        expected
    ));
    let other = AccountId::generate();
    assert!(account_ended(Some(other), &logged_in(expected), expected));
    assert!(account_ended(Some(other), &logged_in(other), expected));
    let logged_out = AccessState::LoggedOut {
        account_id: expected,
    };
    assert!(account_ended(None, &logged_out, expected));
}

#[tokio::test]
async fn an_answer_without_a_position_is_used_as_before_the_positions() {
    let mut f = fixture();
    let by = AccountId::generate();
    // A server from before ADR 0519: the answer names no position.
    f.answer.cursor = None;
    f.answer.microphone_block = Some(MicrophoneBlock {
        by,
        reason: BlockReason::Moderator,
    });
    let (taken, _) = oneshot::channel();
    f.session
        .finished(Done::Rejoined(Box::new(Ok(f.answer.clone())), taken));
    assert!(f.session.refresh.is_none());
    assert!(
        f.session.controls.microphone_blocked,
        "the block of the answer was not used"
    );
    assert!(f.session.microphone_block.is_some());
    // An answer without a position has no order with the state: the microphone and the
    // sound wait for a state that lists the seat.
    assert!(!f.session.admitted);
    assert!(f.session.controls.sound_blocked);
    // The state cannot be put in order with the answer: it replaces what the answer told.
    f.session.read_blocks(&f.model);
    assert!(f.session.microphone_block.is_none());
    assert!(f.session.admitted);
    // How the block ended cannot be read either: the setting is closed, and the person
    // is told (ADR 0785).
    assert!(f.session.inner.shared().muted);
    assert_eq!(
        f.session.inner.view.borrow().closed,
        Some(MicrophoneClosed::EndOfBlockNotRead)
    );
    // The person opens the microphone, and the next state changes nothing.
    f.session.inner.shared().muted = false;
    f.session.command(SessionCommand::Muted(false));
    f.model.cursor.position += 1;
    f.session.read_blocks(&f.model);
    assert!(!f.session.inner.shared().muted && !f.session.controls.muted);
    f.session.engine.close().await;
}

// The answer to a request for a new token for the seat that the session holds is applied by
// its position, and the admission stays as it was (ADR 0785).

fn a_block(by: AccountId) -> MicrophoneBlock {
    MicrophoneBlock {
        by,
        reason: BlockReason::Moderator,
    }
}

fn renewed(f: &mut Fixture) {
    let (taken, _) = oneshot::channel();
    f.session
        .finished(Done::Rejoined(Box::new(Ok(f.answer.clone())), taken));
}

#[tokio::test]
async fn an_answer_older_than_the_state_changes_nothing() {
    let mut f = fixture();
    f.session.accepted.position = 46;
    f.session
        .set_blocks((Some(a_block(AccountId::generate())), None));
    renewed(&mut f);
    assert!(f.session.microphone_block.is_some() && f.session.sound_block.is_none());
    assert_eq!(f.session.accepted.position, 46);
    assert!(f.session.admitted, "the older answer ended the admission");
    assert!(f.session.controls.microphone_blocked);
    assert!(
        !f.session.controls.sound_blocked,
        "the older answer closed the sound"
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_newer_answer_with_a_block_closes_the_microphone_until_the_end_of_the_block_is_told() {
    let mut f = fixture();
    f.answer.microphone_block = Some(a_block(AccountId::generate()));
    renewed(&mut f);
    assert!(f.session.controls.microphone_blocked);
    assert!(f.session.admitted, "the answer ended the admission");
    assert!(
        !f.session.controls.sound_blocked,
        "an answer without a sound block closed the sound"
    );
    // A state from before the answer shows no block.
    f.model.cursor.position = 44;
    f.session.read_blocks(&f.model);
    assert!(
        f.session.controls.microphone_blocked,
        "a state from before the answer opened the microphone"
    );
    f.model.cursor.position = 46;
    f.session.read_blocks(&f.model);
    // A state does not tell how a block ended (ADR 0785): the block holds until the event
    // of its end is read.
    assert!(f.session.microphone_block.is_some());
    assert!(f.session.controls.microphone_blocked);
    let (call_id, place) = (f.call().id, f.session.target.place);
    let participant = f.call().participants[0].clone();
    tell(
        &mut f,
        vec![(
            46,
            Event::CallParticipantUpdated {
                call_id,
                place,
                participant,
            },
        )],
    );
    f.session.read_blocks(&f.model);
    assert!(f.session.microphone_block.is_none());
    assert!(!f.session.controls.microphone_blocked);
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_newer_answer_without_a_block_removes_the_block_of_an_older_state() {
    let mut f = fixture();
    let by = AccountId::generate();
    f.session
        .set_blocks((Some(a_block(by)), Some(SoundBlock { by })));
    renewed(&mut f);
    assert!(f.session.microphone_block.is_none() && f.session.sound_block.is_none());
    assert!(f.session.admitted);
    assert!(
        !f.session.controls.microphone_blocked && !f.session.controls.sound_blocked,
        "the microphone and the sound waited for a state after the answer"
    );
    // The block of this test has no position among the events, so its end cannot be
    // read: the person's setting is closed, and the person is told (ADR 0785).
    assert!(f.session.inner.shared().muted && f.session.controls.muted);
    assert_eq!(
        f.session.inner.view.borrow().closed,
        Some(MicrophoneClosed::EndOfBlockNotRead)
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn an_event_at_or_before_a_newer_answer_changes_nothing() {
    let mut f = fixture();
    let by = AccountId::generate();
    f.answer.microphone_block = Some(a_block(by));
    f.answer.sound_block = Some(SoundBlock { by });
    renewed(&mut f);
    // The stream delivers a change of the person from before the answer: no block.
    let (call_id, place) = (f.call().id, f.session.target.place);
    let participant = f.call().participants[0].clone();
    f.model.cursor.position = 44;
    tell(
        &mut f,
        vec![(
            44,
            Event::CallParticipantUpdated {
                call_id,
                place,
                participant,
            },
        )],
    );
    f.session.read_blocks(&f.model);
    assert!(f.session.microphone_block.is_some() && f.session.sound_block.is_some());
    assert!(f.session.controls.microphone_blocked && f.session.controls.sound_blocked);
    assert_eq!(f.session.accepted.position, 45);
    assert!(f.session.admitted, "the answer ended the admission");
    assert!(!f.session.inner.shared().muted);
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_renewal_keeps_the_microphone_and_the_sound_open_while_the_stream_has_no_later_state() {
    let mut f = fixture();
    // The log of the server is at 45. Its events after 40 were for other accounts, so the
    // state of this device stays at 40 until the next checkpoint of the stream.
    renewed(&mut f);
    f.session.read_blocks(&f.model);
    assert_eq!(f.model.cursor.position, 40);
    assert!(f.session.admitted, "the renewal ended the admission");
    assert!(
        !f.session.controls.microphone_blocked && !f.session.controls.sound_blocked,
        "the microphone and the sound waited for a state after the answer"
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn an_answer_of_another_history_for_a_new_seat_is_used_with_its_seat() {
    let mut f = fixture();
    f.answer.identity = "new-seat".into();
    f.answer.cursor = Some(Cursor {
        epoch: StreamEpoch::generate(),
        position: 3,
    });
    renewed(&mut f);
    assert_eq!(
        f.session.place.identity(),
        "new-seat",
        "the seat that the server gave is used by nobody"
    );
    assert!(f.session.refresh.is_some());
    assert!(!f.session.admitted);
    assert!(f.session.controls.microphone_blocked && f.session.controls.sound_blocked);
    f.session.refresh = None;
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_state_that_tells_a_grant_again_does_not_undo_an_explicit_unmute() {
    use junto_client_core::protocol::call::Meeting;
    let mut f = fixture();
    let presenter = AccountId::generate();
    // A state shows the block of the meeting.
    f.call().meeting = Some(Meeting {
        presenter,
        started_at: OffsetDateTime::UNIX_EPOCH,
    });
    f.call().participants[0].microphone_block = Some(MicrophoneBlock {
        by: presenter,
        reason: BlockReason::Meeting,
    });
    f.session.read_blocks(&f.model);
    assert!(f.session.microphone_block.is_some());
    // The events tell the grant, and the person opens the microphone.
    let events = a_grant(&mut f, presenter, 41);
    tell(&mut f, events);
    assert!(f.session.inner.shared().muted);
    f.session.inner.shared().muted = false;
    f.session.command(SessionCommand::Muted(false));
    // The state that holds the same grant arrives after that.
    f.call().participants[0].microphone_block = None;
    f.model.cursor.position = 46;
    f.session.read_blocks(&f.model);
    assert!(f.session.microphone_block.is_none());
    assert!(
        !f.session.inner.shared().muted,
        "the state closed the microphone that the person had opened after the grant"
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_device_that_logs_in_again_keeps_the_call_and_its_gate() {
    use junto_client_core::flow::access::AccessState;
    let mut f = fixture();
    f.session.rejoining = false;
    f.session.apply_controls();
    let account_id = f.session.grants.account_id;
    // The client core logs in again: the device has its account and no session token.
    let logged_in = AccessState::LoggedIn {
        account_id,
        device_id: DeviceId::generate(),
        session_expires_at: OffsetDateTime::UNIX_EPOCH,
        recoveries: Vec::new(),
        previous_account: None,
    };
    assert!(!matches!(f.session.read_as(None, &logged_in), Fate::End(_)));
    assert!(f.session.admitted, "the login ended the admission");
    assert!(!f.session.controls.microphone_blocked && !f.session.controls.sound_blocked);
    // The server refused the login.
    assert_eq!(
        f.session
            .read_as(None, &AccessState::LoggedOut { account_id }),
        Fate::End(EndReason::Server(None))
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_grant_that_a_renewal_answer_tells_closes_the_microphone_setting_once() {
    use junto_client_core::protocol::call::Meeting;
    let mut f = fixture();
    let presenter = AccountId::generate();
    let meeting = Some(Meeting {
        presenter,
        started_at: OffsetDateTime::UNIX_EPOCH,
    });
    // A state shows the block of the meeting.
    f.call().meeting.clone_from(&meeting);
    f.call().participants[0].microphone_block = Some(MicrophoneBlock {
        by: presenter,
        reason: BlockReason::Meeting,
    });
    f.session.read_blocks(&f.model);
    assert!(f.session.microphone_block.is_some());
    // The answer was read after the presenter let the person speak.
    f.answer.call.meeting = meeting;
    renewed(&mut f);
    assert!(f.session.microphone_block.is_none());
    assert!(
        f.session.inner.shared().muted,
        "the grant of the answer did not close the microphone setting"
    );
    // The person opens the microphone, and the events of the same grant arrive after it.
    f.session.inner.shared().muted = false;
    f.session.command(SessionCommand::Muted(false));
    let events = a_grant(&mut f, presenter, 41);
    tell(&mut f, events);
    assert!(
        !f.session.inner.shared().muted,
        "the events of the grant closed the microphone that the person had opened"
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn an_answer_that_does_not_tell_the_blocks_keeps_them_and_ends_the_admission() {
    let mut f = fixture();
    let by = AccountId::generate();
    f.session.set_blocks((Some(a_block(by)), None));
    // A call that hides who is inside, and the server has not seen the device in the room.
    f.answer.call.hides_occupants = true;
    f.answer.call.participants.clear();
    renewed(&mut f);
    assert!(f.session.microphone_block.is_some());
    assert!(!f.session.admitted);
    assert!(f.session.controls.microphone_blocked && f.session.controls.sound_blocked);
    f.session.engine.close().await;
}

#[tokio::test]
async fn an_answer_of_another_history_for_the_held_seat_ends_the_admission() {
    let mut f = fixture();
    f.answer.cursor = Some(Cursor {
        epoch: StreamEpoch::generate(),
        position: 3,
    });
    renewed(&mut f);
    assert_eq!(f.session.place.identity(), "seat");
    assert!(f.session.refresh.is_some());
    assert!(!f.session.admitted);
    assert!(f.session.controls.microphone_blocked && f.session.controls.sound_blocked);
    f.session.refresh = None;
    f.session.engine.close().await;
}

// An answer that states the seat is the membership of the person at its cursor (ADR 0785).

fn ended(reason: Option<LeaveReason>) -> Event {
    Event::OwnCallChanged {
        call: None,
        left: reason,
    }
}

/// The media connection failed, and the time of the next request for a token has come.
/// Returns whether the session asked for a token.
fn asks_for_a_token(f: &mut Fixture) -> bool {
    let (done, _results) = mpsc::unbounded_channel();
    f.session.retry = Some(Instant::now());
    f.session.work(Instant::now(), &done);
    f.session.rejoining
}

#[tokio::test]
async fn a_token_is_requested_again_before_the_stream_reaches_the_answer_of_the_renewal() {
    let mut f = fixture();
    // The answer holds the events up to 45, and the state of the stream is at 40.
    renewed(&mut f);
    f.session.fate = f.session.read_state(StreamStatus::Live, Some(&f.model));
    assert_eq!(
        f.session.fate,
        Fate::Held,
        "the answer that stated the seat is not the membership"
    );
    assert!(
        asks_for_a_token(&mut f),
        "no token was requested after the media connection failed again"
    );
    f.session.engine.close().await;
}

#[tokio::test]
async fn an_interrupted_stream_decides_nothing_after_an_answer_that_stated_the_seat() {
    let mut f = fixture();
    renewed(&mut f);
    let interrupted = StreamStatus::Interrupted(junto_client_core::live::StreamFault::Silent);
    f.session.fate = f.session.read_state(interrupted, Some(&f.model));
    assert_eq!(f.session.fate, Fate::Undecided);
    assert!(!asks_for_a_token(&mut f));
    f.session.engine.close().await;
}

#[tokio::test]
async fn an_answer_from_before_the_end_of_the_seat_does_not_bring_the_seat_back() {
    // A removal by a moderator, a move, the end of the access, and the end of the call.
    for reason in [
        Some(LeaveReason::Disconnected),
        Some(LeaveReason::Moved),
        Some(LeaveReason::AccessEnded),
        None,
    ] {
        let mut f = fixture();
        // The state shows the seat, and the event 47 ends it.
        assert_eq!(
            f.session.read_state(StreamStatus::Live, Some(&f.model)),
            Fate::Held
        );
        tell(&mut f, vec![(47, ended(reason))]);
        // The answer was read at 45, before that event, and arrives after it.
        renewed(&mut f);
        f.model.cursor.position = 47;
        f.model.own_call = None;
        f.session.fate = f.session.read_state(StreamStatus::Live, Some(&f.model));
        assert_eq!(f.session.fate, Fate::End(EndReason::Server(reason)));
        assert!(!asks_for_a_token(&mut f), "a token was requested");
        f.session.engine.close().await;
    }
}

#[tokio::test]
async fn an_event_at_or_before_an_answer_that_stated_the_seat_does_not_end_the_seat() {
    let mut f = fixture();
    renewed(&mut f);
    // The stream delivers the end of an earlier seat of the account, from before the
    // answer, and its state is the one of that event.
    tell(&mut f, vec![(43, ended(Some(LeaveReason::Switched)))]);
    f.model.cursor.position = 43;
    f.model.own_call = None;
    assert_eq!(
        f.session.read_state(StreamStatus::Live, Some(&f.model)),
        Fate::Held
    );
    assert_eq!(f.session.place.identity(), "seat");
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_refusal_of_a_renewal_states_how_the_seat_ended() {
    use junto_client_core::api::Rejection;
    let refusal = |left: Option<LeaveReason>| {
        Err(ApiError::Rejected(Rejection {
            code: ErrorCode::SeatEnded,
            status: 409,
            retry_after: None,
            message: String::new(),
            left,
        }))
    };
    // The seat ended because no media connection used it: a place is asked for at once,
    // without naming the seat (ADR 0520).
    for left in [LeaveReason::ConnectionLost, LeaveReason::NeverConnected] {
        let mut f = fixture();
        let (taken, _) = oneshot::channel();
        f.session
            .finished(Done::Rejoined(Box::new(refusal(Some(left))), taken));
        assert!(f.session.renew_open, "{left:?}");
        assert!(f.session.retry.is_some());
        assert_eq!(f.session.fate, Fate::Held);
        // The request that is due names no seat. A later request, after a failure of
        // that one, names the seat again.
        let (done, _results) = mpsc::unbounded_channel();
        let due = f.session.retry.unwrap();
        f.session.work(due, &done);
        assert!(f.session.rejoining, "{left:?}");
        f.session.rejoining = false;
        f.session.retry = Some(due);
        f.session.work(due, &done);
        assert_eq!(
            f.session.renewals,
            [None, Some("seat".to_owned())],
            "{left:?}"
        );
        f.session.engine.close().await;
    }
    // Every other reason ends the call with it, and nothing is asked again.
    for (left, reason) in [
        (
            Some(LeaveReason::Disconnected),
            EndReason::Server(Some(LeaveReason::Disconnected)),
        ),
        (
            Some(LeaveReason::AccessEnded),
            EndReason::Server(Some(LeaveReason::AccessEnded)),
        ),
        (Some(LeaveReason::Switched), EndReason::Elsewhere),
        (None, EndReason::Server(None)),
    ] {
        let mut f = fixture();
        let (taken, _) = oneshot::channel();
        f.session
            .finished(Done::Rejoined(Box::new(refusal(left)), taken));
        assert_eq!(f.session.fate, Fate::End(reason), "{left:?}");
        assert!(!f.session.renew_open && f.session.retry.is_none());
        assert_eq!(f.session.renewal_seat().as_deref(), Some("seat"));
        f.session.engine.close().await;
    }
    // A move: its events decide, as without the request; nothing is asked again.
    let mut f = fixture();
    let (taken, _) = oneshot::channel();
    f.session.finished(Done::Rejoined(
        Box::new(refusal(Some(LeaveReason::Moved))),
        taken,
    ));
    assert_eq!(f.session.fate, Fate::Held);
    assert!(!f.session.renew_open && f.session.retry.is_none());
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_grant_that_a_read_of_the_own_call_states_closes_the_setting_once_after_lost_events() {
    let mut f = fixture();
    f.session.rejoining = false;
    f.session.apply_controls();
    let seat = f.session.place.identity().to_owned();
    // The events were all read: the event of the grant closes the setting, not the
    // statement.
    let stated_at = f.model.cursor.position;
    f.model.own_grant = Some((seat.clone(), stated_at));
    f.session.read_blocks(&f.model);
    assert!(!f.session.inner.shared().muted);

    // Notices were missed: the event of the grant can be among them. The statement closes
    // the setting once.
    f.session.missed();
    f.model.cursor.position += 1;
    f.session.read_blocks(&f.model);
    assert!(
        f.session.inner.shared().muted,
        "the stated grant did not close the setting"
    );
    // The person opens the microphone. A later read that still states the grant, before
    // the report of the open reached the server, closes nothing.
    f.session.inner.shared().muted = false;
    f.session.command(SessionCommand::Muted(false));
    f.model.cursor.position += 1;
    f.session.read_blocks(&f.model);
    assert!(
        !f.session.inner.shared().muted,
        "the same statement closed the setting twice"
    );
    f.session.engine.close().await;

    // A statement that the person answered by opening the microphone after it, and then
    // lost notices: the statement is older than the open, and closes nothing.
    let mut f = fixture();
    f.session.rejoining = false;
    f.session.apply_controls();
    let stated_at = f.model.cursor.position;
    f.model.own_grant = Some((seat.clone(), stated_at));
    f.session.read_blocks(&f.model);
    f.model.cursor.position += 1;
    f.session.read_blocks(&f.model);
    f.session.command(SessionCommand::Muted(false));
    f.session.missed();
    f.model.cursor.position += 1;
    f.session.read_blocks(&f.model);
    assert!(
        !f.session.inner.shared().muted,
        "a statement from before the open closed the setting"
    );

    // A read without the statement ends it; a later statement after lost events is a
    // grant again.
    f.model.own_grant = None;
    f.session.read_blocks(&f.model);
    f.session.missed();
    f.model.cursor.position += 1;
    f.model.own_grant = Some((seat, f.model.cursor.position));
    f.session.read_blocks(&f.model);
    assert!(f.session.inner.shared().muted);
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_join_answer_that_arrives_while_a_login_runs_starts_the_call() {
    use junto_client_core::flow::access::AccessState;
    let f = fixture();
    let (inner, server, target) = (
        f.session.inner.clone(),
        f.session.server.clone(),
        f.session.target,
    );
    let account_id = f.session.grants.account_id;
    // The access flow shows the account, and the device holds no session: a login after
    // an ended session runs (ADR 0787).
    server.set_access_for_test(AccessState::LoggedIn {
        account_id,
        device_id: DeviceId::generate(),
        session_expires_at: OffsetDateTime::UNIX_EPOCH,
        recoveries: Vec::new(),
        previous_account: None,
    });
    assert!(server.account().is_none());
    let key = {
        let mut shared = inner.shared();
        shared.next_key += 1;
        let key = shared.next_key;
        shared.joining = Some(Joining { key, target });
        key
    };
    assert!(
        start(&inner, key, server, target, f.answer.clone()),
        "the answer of the server started no call"
    );
    assert!(end_active(&inner).await.is_some());
    f.session.engine.close().await;
}

#[tokio::test]
async fn a_move_that_is_told_while_a_login_runs_is_followed() {
    use junto_client_core::protocol::call::CallMove;
    use junto_client_core::protocol::ids::CallMoveId;
    let mut f = fixture();
    // The event stream keeps its session while a login after an ended session runs
    // (ADR 0787), so the move can arrive while the device holds no session.
    assert!(f.session.server.account().is_none());
    let call_move = CallMove {
        id: CallMoveId::generate(),
        account_id: f.session.grants.account_id,
        from: f.session.target.place,
        to: CallPlace::Channel {
            workspace_id: WorkspaceId::generate(),
            channel_id: ChannelId::generate(),
        },
        moved_by: AccountId::generate(),
        expires_at: OffsetDateTime::now_utc() + time::Duration::seconds(60),
    };
    f.session.note_move(&Event::CallMoveRequested { call_move });
    assert!(
        f.session.moving.is_some(),
        "the move of the person was not followed"
    );
    f.session.end_move();
    f.session.engine.close().await;
}
