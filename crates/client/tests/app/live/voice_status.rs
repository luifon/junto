use super::*;

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn a_connected_member_can_set_status_and_other_viewers_receive_it(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Example Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Rin");
    voice_channel(&world, &admin, &mut window, "Daily");
    let channel = window
        .channels()
        .into_iter()
        .find(|row| row.name == "Daily")
        .unwrap()
        .id;
    world.run(async {
        admin.device.client.start_event_streams().unwrap();
        admin.device.client.select(Some(admin.handle.id()));
    });
    window.until(
        "the other viewer is subscribed before the status changes",
        |_| {
            let view = admin.handle.live().view();
            view.status == junto_client_core::live::StreamStatus::Live
                && view
                    .model
                    .as_ref()
                    .is_some_and(|model| model.channel(channel).is_some())
        },
    );
    window.right_click_named(format!("channel-row:{channel}"));
    assert!(!window.shows("voice-status-edit"));
    window.press("escape");
    window.click_channel("Daily");
    window.click("join-call");
    window.until("the member is connected", |window| {
        window
            .view()
            .call
            .as_ref()
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    let own = window.handle().owner().unwrap().account_id;
    window.until("the server has confirmed the connection", |window| {
        window.channels().iter().any(|row| {
            row.id == channel
                && row.voice.as_ref().is_some_and(|voice| {
                    voice
                        .occupants
                        .iter()
                        .any(|person| person.account_id == own)
                })
        })
    });
    window.right_click_named(format!("channel-row:{channel}"));
    window.click("voice-status-edit");
    window.type_into("voice-status-input", "Design review");
    window.click("voice-status-save");
    window.until("the status is shared with another viewer", |window| {
        !window.shows("voice-status-dialog")
            && window
                .channels()
                .iter()
                .any(|row| row.id == channel && row.voice_status == "Design review")
            && admin
                .handle
                .live()
                .view()
                .model
                .as_ref()
                .and_then(|model| model.channel(channel))
                .is_some_and(|row| row.channel.voice_status == "Design review")
    });
    world
        .run(
            admin
                .handle
                .api()
                .set_voice_channel_status(channel, "Updated remotely"),
        )
        .unwrap();
    window.until("a remote edit is drawn", |window| {
        window
            .channels()
            .iter()
            .any(|row| row.id == channel && row.voice_status == "Updated remotely")
    });
    window.right_click_named(format!("channel-row:{channel}"));
    window.click("voice-status-edit");
    world.run(window.handle().api().leave_call()).unwrap();
    window.until("leaving removes the member's status editor", |window| {
        !window.shows("voice-status-dialog")
    });
    assert!(
        window
            .channels()
            .iter()
            .any(|row| row.id == channel && row.voice_status == "Updated remotely")
    );
}
