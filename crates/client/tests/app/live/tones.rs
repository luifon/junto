//! The sounds of call events in the window (ADR 0707).

use std::time::Duration;

use gpui::TestAppContext;
use junto_client_core::protocol::call::CallPlace;
use junto_media::audio::sounds::{Sound, heard};
use junto_media::calls::{CallTarget, Phase};

use super::{ROOM, enter, occupants, room};
use crate::calls::voice_channel;
use crate::harness::{Window, World, member};

/// Waits until the output of the sounds was opened `count` times and the last sound that it
/// played is `sound`.
fn until_sound(window: &mut Window, what: &str, count: usize, sound: Sound) {
    window.until(what, |window| {
        let (played, rate) = window.tones.played_since_clear();
        window.tones.opened_outputs().len() == count
            && heard::recognise(&played, rate) == Some(sound)
    });
}

#[gpui::test]
#[ignore = "needs a LiveKit server: run through tools/with-test-livekit.sh"]
fn the_person_in_the_call_hears_a_tone_for_each_event_and_none_for_the_people_present(
    cx: &mut TestAppContext,
) {
    let world = World::new();
    let server = world.server_with_media("Tones");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Luca");
    voice_channel(&world, &admin, &mut window, ROOM);
    let workspace_id = window.view().workspace().unwrap().id;
    let target = CallTarget {
        server_id: admin.handle.id(),
        place: CallPlace::Channel {
            workspace_id,
            channel_id: room(&mut window).id,
        },
    };

    // The administrator is in the call before the person: the entry of the person plays
    // the sound of the own entry and none for the person present.
    enter(&world, &caller, target);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("both people are in the call", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
            && occupants(window).len() == 2
    });
    until_sound(&mut window, "the sound of the own entry", 1, Sound::CallJoined);
    world.run(async { tokio::time::sleep(Duration::from_millis(500)).await });
    window.settle();
    assert_eq!(window.tones.opened_outputs().len(), 1);

    window.click("microphone");
    until_sound(&mut window, "the sound of a closed microphone", 2, Sound::MicrophoneOff);
    world.run(caller.calls.leave());
    until_sound(&mut window, "the sound of a person who leaves", 3, Sound::PersonLeft);
    enter(&world, &caller, target);
    until_sound(&mut window, "the sound of a person who joins", 4, Sound::PersonJoined);
    window.click("leave-call");
    until_sound(&mut window, "the sound of the end of the call", 5, Sound::CallLeft);
    world.run(caller.calls.leave());
}
