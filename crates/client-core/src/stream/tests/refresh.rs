use super::*;

async fn requests(rig: &Rig, end: &str, count: usize) {
    soon("the request", async {
        while rig.http.count(end) < count {
            tokio::task::yield_now().await;
        }
    })
    .await;
}

#[tokio::test(start_paused = true)]
async fn an_existing_snapshot_and_an_earlier_barrier_cannot_satisfy_a_later_one() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let old = rig.http.hold("/snapshot");
    peer.event(41, Event::PermissionsChanged { workspace_id: None })
        .await;
    requests(&rig, "/snapshot", 2).await;
    let fresh = rig.http.hold("/snapshot");
    let first = tokio::spawn(rig.live.refresh());
    requests(&rig, "/snapshot", 3).await;
    let newest = rig.http.hold("/snapshot");
    let second = tokio::spawn(rig.live.refresh());
    requests(&rig, "/snapshot", 4).await;
    old.add_permits(1);
    fresh.add_permits(1);
    tokio::task::yield_now().await;
    assert!(!first.is_finished() && !second.is_finished());
    assert_eq!(rig.model().cursor.position, 41);
    rig.set_snapshot(60, |_| {});
    newest.add_permits(1);
    let first = soon("first barrier", first).await.unwrap().unwrap();
    let second = soon("second barrier", second).await.unwrap().unwrap();
    assert_eq!(first, second);
    assert_eq!(first.position, 60);
    assert_eq!(rig.model().cursor, second);
}

#[tokio::test(start_paused = true)]
async fn a_refresh_cancels_an_own_call_read_from_before_its_snapshot() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let held = rig.http.hold("/api/v1/call");
    let own = rig.scene.own_call(CallId::generate(), rig.scene.daily);
    peer.event(
        41,
        Event::OwnCallChanged {
            call: Some(own.clone()),
            left: None,
        },
    )
    .await;
    requests(&rig, "/api/v1/call", 1).await;
    rig.set_snapshot(60, |snapshot| snapshot.own_call = None);
    let cursor = soon("snapshot", rig.live.refresh()).await.unwrap();
    assert_eq!(cursor.position, 60);
    rig.http.routes.lock().unwrap().own_call = Some(OwnCallState {
        inside: None,
        own: own.clone(),
        call: Call {
            id: own.call_id,
            place: own.place,
            started_at: Some(time()),
            lock: None,
            meeting: None,
            hides_occupants: false,
            participants: vec![participant(rig.scene.account_id)],
            recording: None,
        },
        entry_requests: vec![],
    });
    held.add_permits(1);
    tokio::task::yield_now().await;
    assert!(rig.model().own_call.is_none());
    assert!(rig.model().call_at(&own.place).is_none());
}

#[tokio::test(start_paused = true)]
async fn an_off_account_or_session_change_cancels_the_barrier() {
    for change in 0..3 {
        let mut rig = rig();
        rig.want(ConnectionLevel::Selected, "jst_one");
        let (_peer, _, _) = rig.served().await;
        let _held = rig.http.hold("/snapshot");
        let refresh = tokio::spawn(rig.live.refresh());
        requests(&rig, "/snapshot", 2).await;
        match change {
            0 => rig.off(),
            1 => rig.want(ConnectionLevel::Selected, "jst_two"),
            _ => {
                let target = rig.target("jst_two", AccountId::generate());
                rig.desired.send_replace(Desired {
                    level: ConnectionLevel::Selected,
                    target: Some(target),
                });
            }
        }
        assert!(matches!(
            soon("cancelled barrier", refresh).await.unwrap(),
            Err(LoadError::Off)
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn a_failed_snapshot_does_not_acknowledge_the_barrier() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (_peer, _, _) = rig.served().await;
    let gate = rig.http.hold("/snapshot");
    rig.http.fail("/snapshot", 500, "internal");
    rig.set_snapshot(60, |_| {});
    let refresh = tokio::spawn(rig.live.refresh());
    requests(&rig, "/snapshot", 2).await;
    gate.add_permits(1);
    rig.when("failed read", |view| {
        matches!(view.status, StreamStatus::Interrupted(_))
    })
    .await;
    assert!(!refresh.is_finished());
    assert_ne!(rig.model().cursor.position, 60);
    gate.add_permits(1);
    assert_eq!(soon("retry", refresh).await.unwrap().unwrap().position, 60);
}

#[tokio::test(start_paused = true)]
async fn an_own_call_read_keeps_its_blocks_over_older_and_later_delivery_of_included_events() {
    use junto_protocol::call::{BlockReason, Meeting, MicrophoneBlock, SoundBlock};
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let call_id = CallId::generate();
    let own = rig.scene.own_call(call_id, rig.scene.daily);
    let place = own.place;
    let before = participant(rig.scene.account_id);
    let mut blocked = before.clone();
    blocked.microphone_block = Some(MicrophoneBlock {
        by: AccountId::generate(),
        reason: BlockReason::Meeting,
    });
    blocked.sound_block = Some(SoundBlock {
        by: AccountId::generate(),
    });
    blocked.hand_raised = true;
    let meeting = Meeting {
        presenter: AccountId::generate(),
        started_at: time(),
    };
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.own_call = Some(OwnCallState {
            inside: None,
            own: own.clone(),
            call: Call {
                id: call_id,
                place,
                started_at: Some(time()),
                lock: None,
                meeting: Some(meeting.clone()),
                hides_occupants: false,
                participants: vec![blocked.clone()],
                recording: None,
            },
            entry_requests: vec![],
        });
        routes.own_call_cursor = Some(Cursor {
            epoch: rig.epoch(),
            position: 45,
        });
    }
    let gate = rig.http.hold("/api/v1/call");
    peer.event(
        41,
        Event::OwnCallChanged {
            call: Some(own),
            left: None,
        },
    )
    .await;
    requests(&rig, "/api/v1/call", 1).await;
    peer.event(
        42,
        Event::CallParticipantUpdated {
            call_id,
            place,
            participant: before.clone(),
        },
    )
    .await;
    rig.model_when("older event", |model| model.cursor.position == 42)
        .await;
    gate.add_permits(1);
    let model = rig
        .model_when("HTTP read", |model| {
            model
                .call_at(&place)
                .is_some_and(|call| call.meeting.is_some())
        })
        .await;
    assert_eq!(
        model
            .call_at(&place)
            .unwrap()
            .participant(blocked.account_id),
        Some(&blocked)
    );
    // The call is known up to the position of the read, the rest up to the last event.
    assert_eq!(model.cursor.position, 42);
    assert_eq!(model.call_cursor(&place).position, 45);
    peer.event(
        43,
        Event::CallParticipantUpdated {
            call_id,
            place,
            participant: before.clone(),
        },
    )
    .await;
    peer.event(
        44,
        Event::CallMeetingEnded {
            call_id,
            place,
            reason: junto_protocol::call::MeetingEnd::TurnedOff,
        },
    )
    .await;
    let model = rig
        .model_when("included events", |model| model.cursor.position == 44)
        .await;
    assert_eq!(
        model
            .call_at(&place)
            .unwrap()
            .participant(blocked.account_id),
        Some(&blocked)
    );
    assert_eq!(model.call_at(&place).unwrap().meeting, Some(meeting));
    peer.event(
        46,
        Event::CallParticipantUpdated {
            call_id,
            place,
            participant: before.clone(),
        },
    )
    .await;
    let model = rig
        .model_when("later event", |model| model.cursor.position == 46)
        .await;
    assert_eq!(model.call_cursor(&place), model.cursor);
    assert_eq!(
        model
            .call_at(&place)
            .unwrap()
            .participant(before.account_id),
        Some(&before)
    );
}

/// ADR 0517 with the positions of ADR 0785: what an answer of the own call says of the seat
/// holds at the position of that answer. An event that the answer holds does not end it;
/// an event after it that lists the account does.
#[tokio::test(start_paused = true)]
async fn what_an_own_call_answer_says_of_the_seat_is_ordered_with_the_events() {
    use crate::model::SeatStated;
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let call_id = CallId::generate();
    let own = rig.scene.own_call(call_id, rig.scene.daily);
    let place = own.place;
    let mut seat = participant(rig.scene.account_id);
    seat.identity.clone_from(&own.identity);
    let answer =
        |inside: bool, participants: Vec<junto_protocol::call::CallParticipant>| OwnCallState {
            inside: Some(inside),
            own: own.clone(),
            call: Call {
                id: call_id,
                place,
                started_at: Some(time()),
                lock: None,
                meeting: None,
                hides_occupants: false,
                participants,
                recording: None,
            },
            entry_requests: vec![],
        };
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.own_call = Some(answer(false, vec![]));
        routes.own_call_cursor = Some(Cursor {
            epoch: rig.epoch(),
            position: 45,
        });
    }
    peer.event(
        41,
        Event::OwnCallChanged {
            call: Some(own.clone()),
            left: None,
        },
    )
    .await;
    let model = rig
        .model_when("the answer", |model| model.own_seat.is_some())
        .await;
    assert!(model.seat_stated_outside(&own.identity));

    // The answer was read at position 45 and holds this event. The read that the event
    // asks for gets no answer yet.
    let gate = rig.http.hold("/api/v1/call");
    peer.event(
        44,
        Event::CallParticipantJoined {
            call_id,
            place,
            participant: seat.clone(),
        },
    )
    .await;
    requests(&rig, "/api/v1/call", 2).await;
    let model = rig
        .model_when("included event", |model| model.cursor.position == 44)
        .await;
    assert!(model.seat_stated_outside(&own.identity));
    assert_eq!(model.call_at(&place).unwrap().participants, vec![]);

    // The server sees the seat after that answer. The event of the entry ends the
    // statement before the next answer arrives.
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.own_call = Some(answer(true, vec![seat.clone()]));
        routes.own_call_cursor = Some(Cursor {
            epoch: rig.epoch(),
            position: 46,
        });
    }
    peer.event(
        46,
        Event::CallParticipantJoined {
            call_id,
            place,
            participant: seat.clone(),
        },
    )
    .await;
    let model = rig
        .model_when("later event", |model| model.cursor.position == 46)
        .await;
    assert_eq!(model.own_seat, None);
    assert!(!model.seat_stated_outside(&own.identity));
    assert_eq!(model.call_at(&place).unwrap().participants, vec![seat]);
    requests(&rig, "/api/v1/call", 3).await;
    gate.add_permits(2);
    let model = rig
        .model_when("the answer after the entry", |model| {
            model.own_seat.is_some()
        })
        .await;
    assert_eq!(
        model.own_seat,
        Some(SeatStated {
            identity: own.identity,
            inside: true
        })
    );
}

#[tokio::test(start_paused = true)]
async fn events_newer_than_a_held_own_call_answer_survive_its_installation() {
    use junto_protocol::call::{BlockReason, CallLock, Meeting, MicrophoneBlock};
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let call_id = CallId::generate();
    let own = rig.scene.own_call(call_id, rig.scene.daily);
    let place = own.place;
    let unblocked = participant(rig.scene.account_id);
    let mut blocked = unblocked.clone();
    blocked.microphone_block = Some(MicrophoneBlock {
        by: AccountId::generate(),
        reason: BlockReason::Moderator,
    });
    let locked = CallLock {
        by: AccountId::generate(),
        at: time(),
    };
    let meeting = Meeting {
        presenter: AccountId::generate(),
        started_at: time(),
    };
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.own_call_cursor = Some(Cursor {
            epoch: rig.epoch(),
            position: 42,
        });
        routes.own_call = Some(OwnCallState {
            inside: None,
            own: own.clone(),
            call: Call {
                id: call_id,
                place,
                started_at: Some(time()),
                lock: Some(locked.clone()),
                meeting: None,
                hides_occupants: false,
                participants: vec![blocked],
                recording: None,
            },
            entry_requests: vec![],
        });
    }
    let gate = rig.http.hold("/api/v1/call");
    peer.event(
        41,
        Event::OwnCallChanged {
            call: Some(own),
            left: None,
        },
    )
    .await;
    requests(&rig, "/api/v1/call", 1).await;
    peer.event(
        43,
        Event::CallMeetingStarted {
            call_id,
            place,
            meeting: meeting.clone(),
        },
    )
    .await;
    peer.event(
        44,
        Event::CallParticipantUpdated {
            call_id,
            place,
            participant: unblocked.clone(),
        },
    )
    .await;
    rig.model_when("later events", |model| model.cursor.position == 44)
        .await;
    gate.add_permits(1);
    let model = rig
        .model_when("the held read", |model| {
            model
                .call_at(&place)
                .is_some_and(|call| call.lock.is_some())
        })
        .await;
    let call = model.call_at(&place).unwrap();
    assert_eq!(call.lock, Some(locked));
    assert_eq!(call.meeting, Some(meeting));
    assert_eq!(call.participant(unblocked.account_id), Some(&unblocked));
}

#[tokio::test(start_paused = true)]
async fn an_own_call_answer_without_a_position_is_used_as_before_the_positions() {
    use junto_protocol::call::{BlockReason, MicrophoneBlock};
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let call_id = CallId::generate();
    let own = rig.scene.own_call(call_id, rig.scene.daily);
    let place = own.place;
    let unblocked = participant(rig.scene.account_id);
    let mut blocked = unblocked.clone();
    blocked.microphone_block = Some(MicrophoneBlock {
        by: AccountId::generate(),
        reason: BlockReason::Moderator,
    });
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.own_call_unplaced = true;
        routes.own_call = Some(OwnCallState {
            inside: None,
            own: own.clone(),
            call: Call {
                id: call_id,
                place,
                started_at: Some(time()),
                lock: None,
                meeting: None,
                hides_occupants: false,
                participants: vec![blocked],
                recording: None,
            },
            entry_requests: vec![],
        });
    }
    let gate = rig.http.hold("/api/v1/call");
    peer.event(
        41,
        Event::OwnCallChanged {
            call: Some(own),
            left: None,
        },
    )
    .await;
    requests(&rig, "/api/v1/call", 1).await;
    peer.event(
        42,
        Event::CallParticipantUpdated {
            call_id,
            place,
            participant: unblocked.clone(),
        },
    )
    .await;
    rig.model_when("the event", |model| model.cursor.position == 42)
        .await;
    gate.add_permits(1);
    // The answer cannot be put in order with the event: the event is applied again.
    let model = rig
        .model_when("the read", |model| {
            model
                .call_at(&place)
                .is_some_and(|call| call.started_at.is_some())
        })
        .await;
    let call = model.call_at(&place).unwrap();
    assert_eq!(call.participant(unblocked.account_id), Some(&unblocked));
    assert_eq!(model.call_cursor(&place), model.cursor);
}

#[tokio::test(start_paused = true)]
async fn an_included_leave_removes_the_speaking_mark_that_an_own_call_answer_kept() {
    let mut rig = rig();
    rig.want(ConnectionLevel::Selected, "jst_one");
    let (peer, _, _) = rig.served().await;
    let call_id = CallId::generate();
    let own = rig.scene.own_call(call_id, rig.scene.daily);
    let place = own.place;
    let other = participant(AccountId::generate());
    let gate = rig.http.hold("/api/v1/call");
    peer.event(
        41,
        Event::OwnCallChanged {
            call: Some(own.clone()),
            left: None,
        },
    )
    .await;
    requests(&rig, "/api/v1/call", 1).await;
    peer.event(
        42,
        Event::CallParticipantJoined {
            call_id,
            place,
            participant: other.clone(),
        },
    )
    .await;
    peer.event(
        43,
        Event::CallSpeaking {
            call_id,
            place,
            account_ids: vec![other.account_id],
        },
    )
    .await;
    let model = rig
        .model_when("the speaking mark", |model| model.cursor.position == 43)
        .await;
    assert_eq!(model.call_at(&place).unwrap().speaking, [other.account_id]);
    // The answer was read after the person had left: it holds the event 44.
    {
        let mut routes = rig.http.routes.lock().unwrap();
        routes.own_call = Some(OwnCallState {
            inside: None,
            own,
            call: Call {
                id: call_id,
                place,
                started_at: Some(time()),
                lock: None,
                meeting: None,
                hides_occupants: false,
                participants: vec![participant(rig.scene.account_id)],
                recording: None,
            },
            entry_requests: vec![],
        });
        routes.own_call_cursor = Some(Cursor {
            epoch: rig.epoch(),
            position: 45,
        });
    }
    gate.add_permits(1);
    rig.model_when("the answer", |model| {
        model.call_cursor(&place).position == 45
    })
    .await;
    peer.event(
        44,
        Event::CallParticipantLeft {
            call_id,
            place,
            account_id: other.account_id,
            reason: junto_protocol::call::LeaveReason::Left,
        },
    )
    .await;
    let model = rig
        .model_when("the included leave", |model| model.cursor.position == 44)
        .await;
    let call = model.call_at(&place).unwrap();
    assert!(call.participant(other.account_id).is_none());
    assert!(
        call.speaking.is_empty(),
        "a person who is not in the call has a speaking mark"
    );
}
