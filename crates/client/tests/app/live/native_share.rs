//! A share of a real screen through the window (ADR 0913). The application has the screen
//! capture of the system: the window lists what the system offers and starts the share as
//! a person does, on the executor of gpui, and the administrator receives its pictures.
//! The share has no sound. The test needs the screen permission of the application that
//! runs it, and runs only as a native stage:
//!
//! ```sh
//! JUNTO_TEST_NATIVE_DEVICES=1 tools/native-devices.sh w1
//! ```

use gpui::TestAppContext;
use junto_client::state::view::OwnShare;
use junto_client_core::protocol::call::CallPlace;
use junto_media::calls::{CallTarget, Phase};
use junto_media::share::ScreenAccess;

use super::{ROOM, enter, own_share, room};
use crate::calls::voice_channel;
use crate::harness::{World, member_with_capture};

#[gpui::test]
#[ignore = "a native stage: run through tools/native-devices.sh w1"]
fn a_share_of_a_real_screen_starts_through_the_window(cx: &mut TestAppContext) {
    assert_eq!(
        std::env::var("JUNTO_TEST_NATIVE_DEVICES").as_deref(),
        Ok("1"),
        "NOT RUN: set JUNTO_TEST_NATIVE_DEVICES=1 for an operator-supervised run"
    );
    let capture = junto_media::share::system();
    // Reading the permission shows no question. The stage asks nothing of the system.
    assert_eq!(
        capture.access(),
        ScreenAccess::Allowed,
        "the application that runs the test may not record the screen: give it the \
         permission through tools/native-devices.sh request-screen first"
    );
    let world = World::new();
    let server = world.server_with_media("Acme Studio");
    let admin = world.owner(&server);
    let caller = admin.caller(&world);
    let (_directory, mut window) =
        member_with_capture(cx, &world, &server.address(), &admin, "Luca", capture);
    voice_channel(&world, &admin, &mut window, ROOM);
    let workspace_id = window.view().workspace().unwrap().id;
    let voice = room(&mut window);
    enter(
        &world,
        &caller,
        CallTarget {
            server_id: admin.handle.id(),
            place: CallPlace::Channel {
                workspace_id,
                channel_id: voice.id,
            },
        },
    );
    window.click_channel(ROOM);
    window.click("join-call");
    window.until("the call is connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == Phase::Connected)
    });

    // The dialog reads the list of the system on the executor of the window.
    window.click("call-share");
    let display = window.until_value("the screens of the system are listed", |window| {
        window.in_shell(|shell, _, _| {
            shell
                .share_picker_sources()
                .and_then(|sources| sources.displays.first().map(|display| display.id))
        })
    });
    window.click_named(format!("share-display:{display}"));
    window.click("share-sound-0");
    window.click("start-share");
    window.until("the share of the screen runs", |window| {
        own_share(window) == OwnShare::Running
    });
    window.until(
        "the administrator receives the pictures of the screen",
        |_| {
            caller
                .calls
                .main_picture()
                .borrow()
                .as_ref()
                .is_some_and(|picture| picture.frame.width > 0 && picture.frame.height > 0)
        },
    );

    // The button of the strip ends the share, and the capture with it.
    window.click("call-share");
    window.until("the share has ended", |window| {
        own_share(window) == OwnShare::Off
    });
    window.click("leave-call");
    window.until("the window left the call", |window| {
        window.view().call.is_none()
    });
}
