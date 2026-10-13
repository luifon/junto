#![allow(clippy::unwrap_used)]

use super::*;
use junto_client_core::ClientOptions;
use junto_client_core::address::ServerAddress;
use junto_client_core::book::{ConnectionMode, Membership, ProfileSync, ServerBook, ServerEntry};
use junto_client_core::paths::DataDir;
use junto_client_core::protocol::account::OwnAccount;
use junto_client_core::protocol::call::ChannelCall;
use junto_client_core::protocol::call::{BlockReason, CallParticipant};
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
        }],
        own_call: Some(OwnCall {
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
        blocks_untold: false,
        refresh: None,
        accepted: model.cursor,
        seat_told: None,
        grants: MeetingHistory::new(model.cursor, &call, account_id, false),
        admitted: true,
        connected_once: true,
        interrupted_at: None,
        retry: None,
        backoff: Backoff::new(timing.reconnect_first, timing.reconnect_longest),
        rejoining: true,
        state_report: Reporter::new(timing.state_spacing, None),
        speaking_report: Reporter::new(timing.speaking_spacing, Some(timing.speaking_renewal)),
        share_report: Reporter::holding(timing.share_spacing, None),
        moving: None,
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
    assert!(!f.session.controls.microphone_blocked && !f.session.controls.sound_blocked);
    assert!(f.session.admitted);
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
        assert_eq!(f.session.inner.shared().muted, still_presenting);
        assert!(!f.session.controls.microphone_blocked);
        assert_eq!(f.session.controls.muted, still_presenting);
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

// The tests from here to the end of the file were written again from the notes and the
// failure texts of the unit that wrote them first; the file that held them was lost.

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
async fn a_newer_answer_with_a_block_closes_the_microphone_until_a_later_state_opens_it() {
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
