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
            .lists()
            .is_some_and(|lists| lists.conversation.is_some())
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
            .lists()
            .is_some_and(|lists| lists.conversation.is_some())
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
            .lists()
            .is_some_and(|lists| lists.conversation.is_some())
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

/// The server finds the ring unanswered before the deadline of this device runs out: the
/// call ends with the outcome "no answer" as it does at the deadline of the device.
#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn an_outgoing_call_that_the_server_finds_unanswered_first_ends_with_no_answer(
    cx: &mut TestAppContext,
) {
    use sqlx::Connection;
    let world = World::new();
    let server = world.server_with_media("Unanswered on the server");
    let admin = world.owner(&server);
    let _recipient = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    let server_id = admin.handle.id();
    let other = admin.handle.account().unwrap();
    window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation(server_id, other, cx)
    });
    window.until("conversation ready", |window| {
        window.shows("start-direct-call")
    });
    window.click("start-direct-call");
    window.until("outgoing call connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    let connected = std::time::Instant::now();
    // Move the deadline of the ring into the past; the maintenance of the server resolves
    // it long before the sixty seconds of this device.
    world.run(async {
        let mut database = sqlx::PgConnection::connect(&server.database_url)
            .await
            .unwrap();
        sqlx::query("update call_invitations set expires_at = now() - interval '1 second'")
            .execute(&mut database)
            .await
            .unwrap();
    });
    window.until("the unanswered call ends", |window| {
        window.view().call.is_none()
    });
    // The resolution of the server ended the call, and not the deadline of this device.
    assert!(connected.elapsed() < std::time::Duration::from_secs(30));
    assert!(window.shows("direct-call-history"));
    assert_eq!(
        window.state.read_with(&window.cx, |state, _| state
            .direct_calls()
            .finished
            .get(&(server_id, other))
            .copied()),
        Some(junto_client::state::direct::RingOutcome::Missed)
    );
}

/// The other person declines while the answer of the server to the join is on its way:
/// the device learns of the decline before it has the call. The call ends as declined
/// when it arrives.
#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_decline_before_the_answer_to_the_join_ends_the_outgoing_call(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Declined before the answer");
    let admin = world.owner(&server);
    let _caller = admin.caller(&world);
    let proxy = world.run(crate::support::FaultProxy::start(&server));
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Member");
    let server_id = admin.handle.id();
    let other = admin.handle.account().unwrap();
    window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation(server_id, other, cx)
    });
    window.until("conversation ready", |window| {
        window.shows("start-direct-call")
    });
    proxy.hold_answer_ending("POST", &format!("/direct-calls/{other}/join"));
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
    window.until("the window holds the decline", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .direct_calls()
                .rings
                .get(&(server_id, invitation))
                .is_some_and(|ring| {
                    ring.outcome == junto_client::state::direct::RingOutcome::Declined
                })
        })
    });
    assert_eq!(proxy.answers_waiting(), 1);
    assert!(window.view().call.is_none());
    proxy.release();
    window.until("the call ends as declined when it arrives", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .direct_calls()
                .finished
                .get(&(server_id, other))
                .copied()
                == Some(junto_client::state::direct::RingOutcome::Declined)
        })
    });
    window.until("the call is left", |window| window.view().call.is_none());
    assert!(window.shows("direct-call-history"));
}

/// The panel of the direct-message call that the window shows: one person of the call.
fn direct_panel(window: &mut Window) -> Option<junto_client::state::view::CallPanel> {
    window
        .view()
        .lists()
        .and_then(|lists| lists.call.clone())
        .filter(|panel| panel.direct().is_some())
}

/// A direct-message call is drawn as a call: its panel has the conversation, the volume of
/// the other person, the choice of the microphone, the speaker and the camera, and the
/// controls of a share (ADR 0735, ADR 0738).
#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn a_direct_call_has_the_chat_the_volume_the_devices_and_the_share_controls_of_a_call(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server_with_media("Direct call panel");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (directory, mut window) = member(cx, &world, &server.address(), &admin, "Member");
    turn_processing_off(&mut window);
    let server_id = admin.handle.id();
    let other = admin.handle.account().unwrap();
    let own = window.view().person.account_id.unwrap();
    window.state.update(&mut window.cx, |state, cx| {
        state.open_conversation(server_id, other, cx)
    });
    window.until("conversation shown", |window| {
        window.shows("start-direct-call")
    });

    // While it rings, the stage names the person, and the members list the person outside
    // the call.
    window.click("start-direct-call");
    window.until("the call rings in the page of a call", |window| {
        window.shows("outgoing-direct-call")
            && window.shows("direct-call-ringing")
            && window.shows("call-panel")
    });
    window.click("call-tab-members");
    window.until("the other person is listed outside the call", |window| {
        window.shows_named(format!("call-member:{other}"))
            && !window.shows_named(format!("volume:{other}"))
    });
    window.until("the other account receives a ring", |_| {
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
    window.until("the other person is in the call", |window| {
        window.shows("active-direct-call")
            && !window.shows("direct-call-ringing")
            && direct_panel(window).is_some_and(|panel| panel.people.len() == 2)
    });
    window.until("the window hears the other person", |window| {
        heard(&window.sound, ADMIN_TONE_HZ) > HEARD_DBFS
    });

    // The volume of the other person.
    let slider = crate::harness::selector(format!("volume:{other}"));
    window.until("the volume control is drawn", |window| window.shows(slider));
    window.click(slider);
    window.press(&["left"; 20].join(" "));
    window.until("the other person is not played", |window| {
        direct_panel(window).is_some_and(|panel| {
            panel
                .people
                .iter()
                .any(|person| person.account_id == other && person.volume == 0)
        }) && heard(&window.sound, ADMIN_TONE_HZ) < ABSENT_DBFS
    });
    window.press(&["right"; 10].join(" "));
    window.until("the other person is played again", |window| {
        heard(&window.sound, ADMIN_TONE_HZ) > HEARD_DBFS
    });

    // The chat of the call is the conversation.
    window.click("call-tab-chat");
    assert!(window.shows("call-chat"));
    let api = admin.handle.api();
    let conversation = world.run(api.open_conversation(own)).unwrap();
    world
        .run(api.send_message(
            junto_client_core::model::PlaceId::Direct(conversation.id),
            &junto_client_core::protocol::message::SendMessage {
                document: crate::messages::document("Can you hear me"),
                reply_to: None,
                attachments: Vec::new(),
            },
        ))
        .unwrap();
    window.until_message("Can you hear me");
    window.write("Yes, in the call");
    window.until_message("Yes, in the call");

    // The microphone, the speaker and the camera.
    window.click("call-tab-settings");
    window.until("the devices are listed", |window| {
        direct_panel(window)
            .is_some_and(|panel| panel.audio.inputs.len() == 2 && panel.audio.outputs.len() == 2)
    });
    window.click("input-device");
    window.click("device-choice-1");
    window.click("output-device");
    window.click("device-choice-1");
    window.until("the microphone and the speaker are chosen", |window| {
        direct_panel(window)
            .is_some_and(|panel| panel.audio.inputs[1].selected && panel.audio.outputs[1].selected)
    });
    window.click("camera-device");
    window.click("device-choice-1");
    window.until("the choices are stored", |_| {
        std::fs::read_to_string(voice_file(directory.path()))
            .is_ok_and(|stored| stored.contains("\"camera_device\": \"generated-camera\""))
    });
    window.click("call-camera");
    window.until("the other person receives the camera", |_| {
        !caller.calls.camera_pictures().borrow().is_empty()
    });
    window.click("call-camera");

    // A share: what is shared, and its sound.
    window.click("call-share");
    window.until("the screens and windows are listed", |window| {
        window.shows("share-sources")
    });
    window.click("share-display:1");
    window.click("share-sound-1");
    window.click("start-share");
    window.until("the own share runs with its sound", |window| {
        own_share(window) == OwnShare::Running
            && window.shows("share-header")
            && window.shows("share-sound")
            && direct_panel(window)
                .and_then(|panel| panel.stage)
                .is_some_and(|stage| stage.shares[0].sound == Some(ShareAudio::System))
    });
    window.until("the other person hears the share", |_| {
        heard(&caller.sound, SYSTEM_TONE_HZ) > SHARE_HEARD_DBFS
    });
    // The other person shares a window with the sound of its application: the window
    // chooses the share that it watches.
    caller.calls.start_share(CaptureRequest {
        picture: PictureSource::Window(11),
        sound: SoundChoice::Application(SyntheticCapture::player()),
        quality: ShareQuality::default(),
    });
    window.until("two shares are shown", |window| {
        window.shows("shares-bar")
            && direct_panel(window)
                .and_then(|panel| panel.stage)
                .is_some_and(|stage| stage.shares.len() == 2)
    });
    window.click_named(format!("share-card:{other}"));
    window.until(
        "the share of the other person is watched and heard",
        |window| {
            direct_panel(window)
                .and_then(|panel| panel.stage)
                .and_then(|stage| stage.watched().map(|share| share.id))
                == Some(ShareId::Of(other))
                && heard(&window.sound, PLAYER_TONE_HZ) > SHARE_HEARD_DBFS
        },
    );
    // The volume of the watched share, and the quality of the own share.
    window.click("share-volume");
    window.press(&["left"; 20].join(" "));
    window.until("the share is not played", |window| {
        direct_panel(window)
            .and_then(|panel| panel.stage)
            .is_some_and(|stage| stage.volume == 0)
            && heard(&window.sound, PLAYER_TONE_HZ) < ABSENT_DBFS
    });
    window.click("share-resolution-0");
    window.click("share-frame-rate-0");
    window.until("the settings of the share are stored", |_| {
        std::fs::read_to_string(voice_file(directory.path())).is_ok_and(|stored| {
            stored.contains("\"share_resolution\": \"p720\"")
                && stored.contains("\"share_frame_rate\": 15")
                && stored.contains("\"share_volume\": 0")
        })
    });

    window.click("leave-call");
    window.until("the call has ended", |window| {
        window.view().call.is_none() && window.shows("start-direct-call")
    });
    world.run(caller.calls.leave());
}
