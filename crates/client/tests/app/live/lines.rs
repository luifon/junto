//! The lines of a direct call in the conversation of its two accounts (ADR 0518), in a
//! window that starts after the call: the conversation, its unread sign, and the lines. The
//! lines of the call of a voice channel in the chat of the channel (ADR 0501).

use gpui::TestAppContext;
use junto_client::state::feed::{FeedRow, SystemKind};
use junto_client_core::protocol::call::{CallPlace, UpdateCallSettings};
use junto_media::calls::{CallTarget, Phase};
use junto_media::share::{CaptureRequest, PictureSource, ShareQuality, SoundChoice};

use super::{ROOM, enter, room};
use crate::calls::voice_channel;
use crate::harness::{PATIENCE, Window, World, member, open};
use crate::support::logged_in;

/// The system lines of the shown list, as sentences with their kind, in their order.
fn system_lines(window: &mut Window) -> Vec<(String, SystemKind)> {
    window
        .feed()
        .map(|feed| {
            feed.rows
                .iter()
                .filter_map(|row| match row {
                    FeedRow::System(row) => Some((
                        row.parts.iter().map(|part| part.text.as_str()).collect(),
                        row.kind,
                    )),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn a_window_that_starts_after_a_call_that_was_not_answered_shows_its_lines_and_an_unread_sign(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let ada = world.owner(&server);
    let ada_id = logged_in(&ada.handle);
    let (directory, window) = member(cx, &world, &server.address(), &ada, "Bruno");
    let bruno = logged_in(
        &window
            .state
            .read_with(&window.cx, |state, _| state.client().servers())
            .remove(0),
    );
    // The application of Bruno does not run while Ada calls and hangs up.
    window.close();
    world.run(async {
        let api = ada.handle.api();
        api.join_direct_call(bruno).await.unwrap();
        api.leave_call().await.unwrap();
    });

    // Bruno starts the application afterwards: the conversation with Ada is in the
    // channel column with one unread message. The row has no name until the window has
    // read the profile of the other account, which it does for every row of this list.
    let mut window = open(cx, directory.path());
    window.until_ready();
    let mut conversation = None;
    window.until(
        "the conversation of the call with its unread sign and the name of the caller",
        |window| {
            conversation = window.view().lists().and_then(|lists| {
                lists
                    .directs
                    .iter()
                    .find(|row| {
                        row.account_id == ada_id
                            && row.unread.messages == 1
                            && row.name.as_deref() == Some("Ada")
                    })
                    .cloned()
            });
            conversation.is_some()
        },
    );
    let conversation = conversation.unwrap();
    assert!(conversation.unread.any());

    // The conversation holds the two lines of the call, as sentences.
    window.click_named(format!("direct-row:{}", conversation.id));
    let lines = |window: &mut crate::harness::Window| -> Vec<String> {
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
    };
    window.until("the lines of the call", |window| {
        lines(window) == ["Ada started a call", "Bruno did not answer the call of Ada"]
    });
}

/// What happens in the call of a voice channel is told in the chat of the channel, with the
/// names of the people, and each line is drawn.
#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn the_chat_of_a_voice_channel_tells_what_happened_in_its_call(cx: &mut TestAppContext) {
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let ada = world.owner(&server);
    let caller = ada.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &ada, "Luca");
    voice_channel(&world, &ada, &mut window, ROOM);
    let voice = room(&mut window).id;
    let target = CallTarget {
        server_id: ada.handle.id(),
        place: CallPlace::Channel {
            workspace_id: window.view().workspace().unwrap().id,
            channel_id: voice,
        },
    };
    let luca = window.view().person.account_id.unwrap();
    let api = ada.handle.api();

    // Ada starts the call, and Luca joins it in the window.
    enter(&world, &caller, target);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("the call is connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    // Ada closes the microphone of Luca and opens it again, shares her screen, changes the
    // settings of the room, and turns meeting mode on and off.
    let in_call = |window: &mut Window| {
        room(window).voice.is_some_and(|voice| {
            voice
                .occupants
                .iter()
                .any(|occupant| occupant.account_id == luca)
        })
    };
    window.until("the server lists Luca", in_call);
    world.run(api.block_microphone(voice, luca)).unwrap();
    world.run(api.unblock_microphone(voice, luca)).unwrap();
    caller.calls.start_share(CaptureRequest {
        picture: PictureSource::Display(1),
        sound: SoundChoice::None,
        quality: ShareQuality::default(),
    });
    window.until("the share of Ada arrives", |window| {
        window
            .state
            .read_with(&window.cx, |state, _| state.calls().view().call)
            .is_some_and(|call| !call.shares.is_empty())
    });
    let settings = UpdateCallSettings {
        user_limit: Some(Some(5)),
        hides_occupants: None,
    };
    world
        .run(api.update_call_settings(voice, &settings))
        .unwrap();
    // The server turns meeting mode on when its admission check has seen the share, a
    // moment after the report of the share.
    let started = std::time::Instant::now();
    while let Err(error) = world.run(api.start_meeting(voice)) {
        assert!(
            started.elapsed() < PATIENCE,
            "meeting mode did not start: {error}"
        );
        window.run_tasks();
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    world.run(api.end_meeting(voice)).unwrap();
    caller.calls.stop_share();
    world.run(api.lock_call(voice)).unwrap();
    world.run(api.unlock_call(voice)).unwrap();

    // Luca leaves, then Ada: the call ends.
    window.click("leave-call");
    window.until("the server no longer lists Luca", |window| !in_call(window));
    world.run(caller.calls.leave());
    let expected = [
        ("Ada created this channel", SystemKind::Plain),
        ("Call started by Ada", SystemKind::Start),
        ("Luca joined the call", SystemKind::Plain),
        ("Ada closed the microphone of Luca", SystemKind::Plain),
        (
            "Ada removed the microphone block of Luca",
            SystemKind::Plain,
        ),
        ("Ada started sharing", SystemKind::Start),
        ("Ada changed the settings of the room", SystemKind::Plain),
        ("Ada turned meeting mode on", SystemKind::Start),
        ("Ada turned meeting mode off", SystemKind::Plain),
        ("Ada stopped sharing", SystemKind::Plain),
        ("Ada locked the room", SystemKind::Plain),
        ("Ada unlocked the room", SystemKind::Plain),
        // Luca leaves first, while Ada is inside; the departure of Ada ends the call, whose
        // line states its length.
        ("Luca left the call", SystemKind::Plain),
        ("Call ended after …", SystemKind::Plain),
    ]
    .map(|(text, kind)| (text.to_owned(), kind));
    let mut seen = Vec::new();
    window.until(
        "the chat of the channel holds the lines of the call",
        |window| {
            // The length of the call is measured: the test reads that it is stated.
            let now: Vec<(String, SystemKind)> = system_lines(window)
                .into_iter()
                .map(
                    |(text, kind)| match text.strip_prefix("Call ended after ") {
                        Some(length)
                            if length.ends_with("seconds") || length.ends_with("second") =>
                        {
                            ("Call ended after …".to_owned(), kind)
                        }
                        _ => (text, kind),
                    },
                )
                .collect();
            if now != seen {
                eprintln!("the lines of the chat: {now:?}");
                seen = now;
            }
            seen == expected
        },
    );
    let feed = window.feed().unwrap();
    for row in feed.rows.iter() {
        if let FeedRow::System(row) = row {
            assert!(
                window.shows_named(format!("system:{}", row.message.id)),
                "the line {:?} is not drawn",
                row.parts
            );
        }
    }
}
