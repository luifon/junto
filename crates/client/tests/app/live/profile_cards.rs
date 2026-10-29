use super::*;

#[gpui::test]
#[ignore = "needs PostgreSQL and LiveKit"]
fn profile_call_targets_the_shown_person_and_carries_sound(cx: &mut TestAppContext) {
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
    window.click_named(format!("profile-name:{server_id}:{other}"));
    window.until("the profile card is ready", |window| {
        let own = window.shell.entity_id();
        window.state.read_with(&window.cx, |state, _| {
            state
                .profile_view(own)
                .is_some_and(|view| view.account == other && view.can_contact())
        })
    });
    assert!(window.view().call.is_none());
    window.click("profile-call");
    assert!(!window.shows("profile-card"));
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
    caller.calls.start_share(CaptureRequest {
        picture: PictureSource::Display(1),
        sound: SoundChoice::None,
        quality: ShareQuality::default(),
        dialog_parent: None,
    });
    window.until(
        "the screen share and its people strip are drawn",
        |window| stage(window).is_some(),
    );
    window.click_named(format!("tile-profile-name:{}", other.as_uuid().as_u128()));
    window.until(
        "the share strip opens the correct person's profile",
        |window| {
            let own = window.shell.entity_id();
            window.state.read_with(&window.cx, |state, _| {
                state
                    .profile_view(own)
                    .is_some_and(|view| view.account == other && view.can_contact())
            })
        },
    );
    window.press("escape");
    caller.calls.stop_share(None);
    world.run(caller.calls.leave());
    window.until(
        "the direct call ends when the other person leaves",
        |window| window.view().call.is_none(),
    );
    assert!(!window.shows("outgoing-direct-call"));
}
