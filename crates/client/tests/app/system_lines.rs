//! The lines of a channel that the chat draws (ADR 0408, ADR 0740): a pin and a rename, with
//! the names of the channel; the answer to a request to enter a locked call; the end of a
//! transcript; and repeated lines of a call, which are one line, also after a reconnect. The
//! calls of the voice channels are made by the server of this checkout inside the test
//! process, with its media server in memory. The lines that need a real media server are in
//! `live::lines`.

use gpui::TestAppContext;
use junto_client::state::feed::FeedRow;
use junto_client::state::view::ServerStatus;
use junto_client_core::flow::access::AccessAction;
use junto_client_core::protocol::ids::ChannelId;
use junto_client_core::protocol::secret::OwnerCode;
use junto_client_core::protocol::structure::{ChannelKind, UpdateChannel};

use crate::calls::voice_channel;
use crate::harness::{Admin, Window, World, member};
use crate::messages::document;
use crate::recording_server::{RecordingServer, Spoken};
use crate::support::{Device, FaultProxy, join, logged_in};

/// The text of each system line of the shown list, in its order.
fn line_texts(window: &mut Window) -> Vec<String> {
    window
        .feed()
        .map(|feed| {
            feed.rows
                .iter()
                .filter_map(|row| match row {
                    FeedRow::System(row) => {
                        Some(row.parts.iter().map(|part| part.text.as_str()).collect())
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Waits until the shown list has exactly these system lines, and requires that the window
/// draws the text of each.
#[track_caller]
fn until_lines(window: &mut Window, what: &str, expected: &[&str]) {
    window.until_read(what, line_texts, |lines| lines == expected);
    assert_lines_drawn(window, |_| true);
}

/// The window draws the text of each system line of the shown list that `checked` accepts.
#[track_caller]
fn assert_lines_drawn(window: &mut Window, checked: impl Fn(&str) -> bool) {
    let feed = window.feed().unwrap();
    for row in feed.rows.iter() {
        if let FeedRow::System(row) = row {
            let text: String = row.parts.iter().map(|part| part.text.as_str()).collect();
            if !checked(&text) {
                continue;
            }
            assert!(
                window.shows_named(format!("system-text:{}", row.message.id)),
                "the line {:?} is not drawn",
                row.parts
            );
        }
    }
}

/// The owner of a server inside the test process, as a client without a window.
fn owner(world: &World, server: &RecordingServer) -> Admin {
    world.run(async {
        let device = Device::new();
        let handle = device.add(&server.address()).await;
        let claim = AccessAction::ClaimOwner {
            code: OwnerCode::new(server.owner_code()),
            name: "Ada".to_owned(),
            device_name: "First device".to_owned(),
        };
        handle.dispatch(claim).await.unwrap();
        logged_in(&handle);
        Admin { device, handle }
    })
}

/// Another member of the server, as a client without a window.
fn person(world: &World, server: &RecordingServer, admin: &Admin, name: &str) -> Admin {
    let code = admin.invite(world, 1);
    world.run(async {
        let device = Device::new();
        let handle = device.add(&server.address()).await;
        handle.dispatch(join(&code, name)).await.unwrap();
        logged_in(&handle);
        Admin { device, handle }
    })
}

/// A new voice channel, once the window lists it.
fn voice(world: &World, admin: &Admin, window: &mut Window, name: &str) -> ChannelId {
    voice_channel(world, admin, window, name);
    window
        .channels()
        .into_iter()
        .find(|row| row.kind == ChannelKind::Voice && row.name == name)
        .unwrap()
        .id
}

#[gpui::test]
fn a_pin_and_a_rename_are_lines_of_the_chat_and_renames_in_a_row_are_one_line(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server("Acme Studio");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let channel = window.selected_channel();
    let before = window.channel_names()[0].clone();
    let kept = admin.send(&world, channel, document("keep this"));
    let api = admin.handle.api();
    world.run(api.pin_message(kept.id)).unwrap();
    for name in ["plans", "roadmap"] {
        world
            .run(api.update_channel(
                channel,
                &UpdateChannel {
                    name: Some(name.to_owned()),
                    ..UpdateChannel::default()
                },
            ))
            .unwrap();
    }
    let expected = [
        "Ada pinned a message".to_owned(),
        format!("Ada renamed this channel from {before} to roadmap"),
    ];
    window.until("the chat draws the pin and one rename", |window| {
        let lines = line_texts(window);
        lines.ends_with(&expected)
    });
}

#[gpui::test]
fn the_answer_to_a_request_to_enter_a_locked_call_is_a_line_with_who_answered_and_who_asked(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.run(RecordingServer::start("Acme Studio"));
    let admin = owner(&world, &server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let daily = voice(&world, &admin, &mut window, "Daily");
    let cara = person(&world, &server, &admin, "Cara");
    window.click_channel("Daily");

    let _seat = world.run(server.enter(daily, &admin.handle, false));
    let api = admin.handle.api();
    world.run(api.lock_call(daily)).unwrap();
    for accept in [false, true] {
        let request = world.run(cara.handle.api().request_entry(daily)).unwrap();
        world
            .run(api.answer_entry_request(request.id, accept))
            .unwrap();
    }
    until_lines(
        &mut window,
        "the chat draws the answers to the requests of Cara",
        &[
            "Ada created this channel",
            "Call started by Ada",
            "Ada locked the room",
            "Cara asked to join",
            "Ada did not let Cara in",
            "Cara asked to join",
            "Ada let Cara in",
        ],
    );
}

#[gpui::test]
fn the_end_of_a_transcript_is_a_line_that_states_whether_it_is_ready_or_failed(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.run(RecordingServer::start("Acme Studio"));
    let admin = owner(&world, &server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    let luca = window.handle();
    let ada = logged_in(&admin.handle);
    let transcript = |line: &str| line.starts_with("The transcript of the recording");
    let transcript_lines = |window: &mut Window| -> Vec<String> {
        line_texts(window)
            .into_iter()
            .filter(|line| transcript(line))
            .collect()
    };

    let daily = voice(&world, &admin, &mut window, "Daily");
    let recorded = world.run(server.record(daily, &[&admin.handle, &luca]));
    let said = [Spoken {
        account_id: ada,
        speaker: None,
        start_ms: 100,
        end_ms: 120,
        text: "Good morning.".to_owned(),
    }];
    world.run(server.transcribe(recorded, &said));
    window.click_channel("Daily");
    window.until_read(
        "the chat of Daily draws the line of the ready transcript",
        transcript_lines,
        |lines| lines == &["The transcript of the recording is ready"],
    );
    assert_lines_drawn(&mut window, transcript);

    let weekly = voice(&world, &admin, &mut window, "Weekly");
    let recorded = world.run(server.record(weekly, &[&admin.handle, &luca]));
    world.run(server.refuse_transcript(recorded));
    window.click_channel("Weekly");
    window.until_read(
        "the chat of Weekly draws the line of the failed transcript",
        transcript_lines,
        |lines| lines == &["The transcript of the recording failed"],
    );
    assert_lines_drawn(&mut window, transcript);
}

/// Joins and departures of several people in a row are one line, and a person who leaves
/// and joins again at once is one line. The event stream of the window is cut while lines
/// are written: after the reconnect, the chat draws each line once, and the lines that were
/// written meanwhile join the lines before them.
#[gpui::test]
fn repeated_lines_of_a_call_are_one_line_and_a_reconnect_neither_repeats_nor_loses_a_line(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.run(RecordingServer::start("Acme Studio"));
    let (listener, port) = server.take_proxy();
    let proxy = world.run(FaultProxy::start_on(listener, port));
    let admin = owner(&world, &server);
    let (_directory, mut window) = member(cx, &world, &proxy.address(), &admin, "Luca");
    let daily = voice(&world, &admin, &mut window, "Daily");
    let bruno = person(&world, &server, &admin, "Bruno");
    let cara = person(&world, &server, &admin, "Cara");
    window.click_channel("Daily");

    let _ada = world.run(server.enter(daily, &admin.handle, false));
    let seat = world.run(server.enter(daily, &bruno.handle, false));
    until_lines(
        &mut window,
        "the chat draws the start of the call and the join of Bruno",
        &[
            "Ada created this channel",
            "Call started by Ada",
            "Bruno joined the call",
        ],
    );

    proxy.block_streams(true);
    proxy.cut_streams();
    window.until("the interruption is shown", |window| {
        window.status() == Some(ServerStatus::Reconnecting)
    });
    let cara_seat = world.run(server.enter(daily, &cara.handle, false));
    world.run(server.leave(&[(&cara.handle, cara_seat), (&bruno.handle, seat)]));
    proxy.block_streams(false);
    window.until_ready();
    until_lines(
        &mut window,
        "after the reconnect the chat draws each line once, joined",
        &[
            "Ada created this channel",
            "Call started by Ada",
            "Bruno and Cara joined the call",
            "Cara and Bruno left the call",
        ],
    );

    let seat = world.run(server.enter(daily, &bruno.handle, false));
    world.run(server.leave(&[(&bruno.handle, seat)]));
    let _seat = world.run(server.enter(daily, &bruno.handle, false));
    until_lines(
        &mut window,
        "the chat draws a quick return as one line",
        &[
            "Ada created this channel",
            "Call started by Ada",
            "Bruno and Cara joined the call",
            "Cara and Bruno left the call",
            "Bruno joined the call",
            "Bruno left the call and joined again",
        ],
    );
}
