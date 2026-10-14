use super::*;

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn direct_conversation_starts_outgoing_call_and_carries_sound(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Direct calls");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    turn_processing_off(&mut window);
    let server_id = admin.handle.id();
    let other = admin.handle.account().unwrap();
    let own = window.view().person.account_id.unwrap();
    window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation(server_id, other, cx)
    });
    window.until("conversation shown", |window| {
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.conversation.is_some())
    });
    window.click("start-direct-call");
    window.until("outgoing call shown", |window| {
        window.shows("outgoing-direct-call")
    });
    window.until("other account receives a ring", |_| {
        admin
            .handle
            .live()
            .view()
            .model
            .is_some_and(|model| model.incoming_invitations().next().is_some())
    });
    let invitation = admin
        .handle
        .live()
        .view()
        .model
        .unwrap()
        .incoming_invitations()
        .next()
        .unwrap()
        .clone();
    let target = CallTarget {
        server_id,
        place: CallPlace::direct(own, other),
    };
    world
        .run(caller.calls.join_invited(target, invitation.id))
        .unwrap();
    window.until("direct call accepted", |window| {
        window.shows("active-direct-call")
    });
    window.until("window hears caller", |window| {
        heard(&window.sound, ADMIN_TONE_HZ) > HEARD_DBFS
    });
    window.until("caller hears window", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });
    world.run(caller.calls.leave());
    window.until(
        "the direct call ends when the other person leaves",
        |window| window.view().call.is_none(),
    );
    assert!(!window.shows("outgoing-direct-call"));
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn incoming_direct_call_can_be_declined(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Incoming call");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::direct(
            admin.handle.account().unwrap(),
            window.view().person.account_id.unwrap(),
        ),
    };
    enter(&world, &caller, target);
    window.until("incoming ring view", |window| {
        window.shows("incoming-direct-call")
    });
    assert!(!window.shows("start-direct-call"));
    assert_eq!(window.view().invitations[0].by.as_deref(), Some("Ada"));
    window.click("direct-call-decline");
    window.until("ring dismissed", |window| {
        !window.shows("incoming-direct-call")
    });
    assert!(window.view().call.is_none());
    assert!(
        !window
            .state
            .read_with(&window.cx, |state, _| state.is_ringing())
    );
    world.run(caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn incoming_direct_call_accepts_and_stops_the_ring(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Accept call");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    turn_processing_off(&mut window);
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::direct(
            admin.handle.account().unwrap(),
            window.view().person.account_id.unwrap(),
        ),
    };
    enter(&world, &caller, target);
    window.until("incoming call", |window| {
        window.shows("incoming-direct-call")
    });
    window.until("ring reaches only the injected output", |window| {
        heard(&window.sound, 440.0) > -40.0
    });
    window.click("direct-call-accept");
    window.until("accepted", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    assert!(!window.shows("incoming-direct-call"));
    assert!(
        !window
            .state
            .read_with(&window.cx, |state, _| state.is_ringing())
    );
    window.until("accepted call carries sound to caller", |_| {
        heard(&caller.sound, WINDOW_TONE_HZ) > HEARD_DBFS
    });
    window.until("accepted call carries sound to callee", |window| {
        heard(&window.sound, ADMIN_TONE_HZ) > HEARD_DBFS
    });
    window.click("leave-call");
    world.run(caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn missed_direct_call_is_shown_in_conversation(cx: &mut TestAppContext) {
    use sqlx::Connection;
    let world = World::new();
    let server = world.server_with_media("Missed call");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let server_id = admin.handle.id();
    let other = admin.handle.account().unwrap();
    let own = window.view().person.account_id.unwrap();
    window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation(server_id, other, cx)
    });
    window.until("conversation shown", |window| {
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.conversation.is_some())
    });
    enter(
        &world,
        &caller,
        CallTarget {
            server_id,
            place: CallPlace::direct(own, other),
        },
    );
    window.until("incoming call", |window| {
        window.shows("incoming-direct-call")
    });
    // Move the fixture's deadline into the past; the server's real maintenance resolves it.
    world.run(async {
        let mut database = sqlx::PgConnection::connect(&server.database_url)
            .await
            .unwrap();
        sqlx::query("update call_invitations set expires_at = now() - interval '1 second'")
            .execute(&mut database)
            .await
            .unwrap();
    });
    window.until("missed outcome in conversation", |window| {
        window.shows("direct-call-history")
            && window.state.read_with(&window.cx, |state, _| {
                state
                    .direct_calls()
                    .rings
                    .values()
                    .any(|ring| ring.outcome == junto_client::state::direct::RingOutcome::Missed)
            })
    });
    assert!(!window.shows("incoming-direct-call"));
    world.run(caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn muted_direct_conversation_does_not_ring(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Muted call");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let server_id = admin.handle.id();
    let other = admin.handle.account().unwrap();
    let own = window.view().person.account_id.unwrap();
    let handle = window.state.read_with(&window.cx, |state, _| {
        state.client().server(server_id).unwrap()
    });
    world
        .run(handle.api().mute_direct_conversation(other, None))
        .unwrap();
    enter(
        &world,
        &caller,
        CallTarget {
            server_id,
            place: CallPlace::direct(own, other),
        },
    );
    window.until("muted direct call is visible without an invitation", |_| {
        handle
            .live()
            .view()
            .model
            .is_some_and(|model| !model.direct_calls.is_empty())
    });
    assert!(!window.shows("incoming-direct-call"));
    assert!(
        !window
            .state
            .read_with(&window.cx, |state, _| state.direct_calls().ringing())
    );
    assert!(heard(&window.sound, 440.0) < ABSENT_DBFS);
    world.run(caller.calls.leave());
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn outgoing_decline_ends_call_and_is_shown(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Declined outgoing call");
    let admin = world.owner(&server);
    let _caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let server_id = admin.handle.id();
    let other = admin.handle.account().unwrap();
    window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation(server_id, other, cx)
    });
    window.until("conversation shown", |window| {
        window
            .view()
            .workspace()
            .is_some_and(|workspace| workspace.conversation.is_some())
    });
    window.click("start-direct-call");
    window.until("ring arrives", |_| {
        admin
            .handle
            .live()
            .view()
            .model
            .is_some_and(|model| model.incoming_invitations().next().is_some())
    });
    let invitation = admin
        .handle
        .live()
        .view()
        .model
        .unwrap()
        .incoming_invitations()
        .next()
        .unwrap()
        .id;
    world
        .run(admin.handle.api().refuse_call_invitation(invitation))
        .unwrap();
    window.until("decline ends outgoing call", |window| {
        window.view().call.is_none() && window.shows("direct-call-history")
    });
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn direct_and_voice_calls_share_one_owner_and_refusal_preserves_voice(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Call ownership");
    let admin = world.owner(&server);
    let _caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    voice_channel(&world, &admin, &mut window, ROOM);
    let server_id = admin.handle.id();
    let other = admin.handle.account().unwrap();
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("voice connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    let voice = window.state.read_with(&window.cx, |state, _| {
        state.calls().view().call.unwrap().target
    });
    window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation(server_id, other, cx)
    });
    window.until("direct conversation ready", |window| {
        window.shows("start-direct-call")
    });
    window.click("start-direct-call");
    window.until("direct replaces voice", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .calls()
                .view()
                .call
                .is_some_and(|call| matches!(call.target.place, CallPlace::Direct { .. }))
        })
    });
    window.until("invitation received", |_| {
        admin
            .handle
            .live()
            .view()
            .model
            .is_some_and(|m| m.incoming_invitations().next().is_some())
    });
    let invitation = admin
        .handle
        .live()
        .view()
        .model
        .unwrap()
        .incoming_invitations()
        .next()
        .unwrap()
        .id;
    world
        .run(admin.handle.api().refuse_call_invitation(invitation))
        .unwrap();
    window.until("decline ends direct call", |window| {
        window.view().call.is_none()
    });
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("voice restored", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .calls()
                .view()
                .call
                .is_some_and(|call| call.target == voice && call.phase == Phase::Connected)
        })
    });
    let identity = window.state.read_with(&window.cx, |state, _| {
        state.calls().view().call.unwrap().identity
    });
    window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation(server_id, other, cx)
    });
    window.until("direct conversation ready", |window| {
        window.shows("start-direct-call")
    });
    window.click("start-direct-call");
    window.until("decline pause is shown", |window| {
        window.view().call_notice.is_some()
    });
    assert_eq!(
        identity,
        window.state.read_with(&window.cx, |state, _| state
            .calls()
            .view()
            .call
            .unwrap()
            .identity)
    );
    window.click("leave-call");
}

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn an_unanswered_outgoing_call_ends_even_when_the_recipient_muted_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Unanswered direct call");
    let admin = world.owner(&server);
    let _recipient = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let server_id = admin.handle.id();
    let other = admin.handle.account().unwrap();
    let own = window.view().person.account_id.unwrap();
    world
        .run(admin.handle.api().mute_direct_conversation(own, None))
        .unwrap();
    window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation(server_id, other, cx)
    });
    window.until("conversation ready", |window| {
        window.shows("start-direct-call")
    });
    window.click("start-direct-call");
    window.until("outgoing call connected without a ring", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    window.until("recipient snapshot is available", |_| {
        admin.handle.live().view().model.is_some()
    });
    assert!(
        admin
            .handle
            .live()
            .view()
            .model
            .unwrap()
            .incoming_invitations()
            .next()
            .is_none()
    );
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
    while window.view().call.is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "unanswered call outlived its sixty-second deadline"
        );
        window
            .cx
            .executor()
            .advance_clock(std::time::Duration::from_millis(20));
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(window.shows("direct-call-history"));
    assert_eq!(
        window.state.read_with(&window.cx, |state, _| state
            .direct_calls()
            .finished
            .get(&(server_id, other))
            .copied()),
        Some(junto_client::state::direct::RingOutcome::Missed)
    );
    assert!(
        admin
            .handle
            .live()
            .view()
            .model
            .unwrap()
            .incoming_invitations()
            .next()
            .is_none()
    );
}
