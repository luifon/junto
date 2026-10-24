use gpui::TestAppContext;
use junto_media::calls::Phase;

use super::ROOM;
use crate::calls::voice_channel;
use crate::harness::{World, member};

fn selected_help(cx: &mut TestAppContext, picture: &'static str, sound: &'static str) -> String {
    let world = World::new();
    let server = world.server_with_media("Share sound");
    let admin = world.owner(&server);
    let (_directory, mut window) = member(cx, &world, &server.address(), &admin, "Ada");
    voice_channel(&world, &admin, &mut window, ROOM);
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("the call is connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });
    window.click("call-share");
    window.until("the share choices are listed", |window| {
        window.shows("share-sources")
    });
    window.click(picture);
    window.click(sound);
    if picture == "share-display:1" && sound == "share-sound-2" {
        window.click_named(format!(
            "share-application:{}",
            junto_media::share::synthetic::PLAYER
        ));
    }
    assert!(window.shows("share-sound-help") && window.shows("start-share"));
    window.in_shell(|shell, _, _| shell.share_picker_sound_help().unwrap().to_string())
}

#[gpui::test]
#[ignore = "needs a LiveKit server"]
fn selected_system_sound_warns_about_another_client(cx: &mut TestAppContext) {
    let shown = selected_help(cx, "share-display:1", "share-sound-1");
    assert!(shown.contains("another running copy of Junto"), "{shown}");
    assert!(!shown.contains("except the sound of Junto"), "{shown}");
}

#[gpui::test]
#[ignore = "needs a LiveKit server"]
fn selected_application_sound_warns_that_it_can_include_the_call(cx: &mut TestAppContext) {
    let shown = selected_help(cx, "share-display:1", "share-sound-2");
    assert!(
        shown.contains("may include the sound of this call"),
        "{shown}"
    );
    assert!(!shown.contains("no other one"), "{shown}");
}

#[gpui::test]
#[ignore = "needs a LiveKit server"]
fn selected_window_sound_warns_that_it_can_include_the_call(cx: &mut TestAppContext) {
    let shown = selected_help(cx, "share-window:11", "share-sound-2");
    assert!(
        shown.contains("may include the sound of this call"),
        "{shown}"
    );
    assert!(!shown.contains("no other one"), "{shown}");
}
