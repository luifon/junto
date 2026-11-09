//! A share of a window of Junto through the dialog of the window, with the capture of Windows
//! (ADR 0502, ADR 0736, ADR 0738). The program of these tests is the client here; a second
//! start of the same program opens a real window, as another profile of Junto does. The
//! dialog lists that window among the windows of the client, offers no sound of its
//! application, and the share runs without sound: the administrator receives its pictures,
//! and the sharer sees the statement in place of the picture.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use gpui::{
    AppContext, Context, IntoElement, Render, Styled, TestAppContext, Window, WindowOptions, div,
    rgb,
};
use junto_client::state::view::OwnShare;
use junto_client_core::protocol::call::CallPlace;
use junto_media::calls::{CallTarget, Phase};
use junto_media::share::ScreenAccess;

use super::{ROOM, enter, own_share, room, stage};
use crate::calls::voice_channel;
use crate::harness::{World, member_with_capture};

/// The variable that makes the helper below open its window, with the title of the window.
const HELPER: &str = "JUNTO_TEST_OWN_WINDOW_TITLE";

struct Ground;

impl Render for Ground {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().bg(rgb(0xd8c028))
    }
}

/// Not a test of its own: the second start of this program, which opens one real window and
/// waits until it is ended. It does nothing without its variable.
#[test]
#[ignore = "a helper of windows_own_share"]
fn helper_opens_a_window_of_this_program() {
    let Ok(title) = std::env::var(HELPER) else {
        return;
    };
    gpui_platform::application().run(move |cx| {
        let options = WindowOptions {
            titlebar: Some(gpui::TitlebarOptions {
                title: Some(title.into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        cx.open_window(options, |_, cx| cx.new(|_| Ground)).unwrap();
        println!("ready");
    });
}

/// The second start of this program; it ends when the value is dropped.
struct SecondStart {
    child: Child,
    _adopted: crate::limit::Adopted,
    title: String,
}

impl SecondStart {
    fn open() -> Self {
        let title = format!("Junto own window {}", std::process::id());
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "live::windows_own_share::helper_opens_a_window_of_this_program",
                "--ignored",
                "--nocapture",
                "--test-threads",
                "1",
            ])
            .env(HELPER, &title)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let adopted = crate::limit::adopt(child.id());
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        while !line.starts_with("ready") {
            line.clear();
            assert!(
                output.read_line(&mut line).unwrap() > 0,
                "the second start ended before its window opened"
            );
        }
        Self {
            child,
            _adopted: adopted,
            title,
        }
    }
}

impl Drop for SecondStart {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[gpui::test]
#[ignore = "needs a LiveKit server and a Windows desktop session"]
fn a_window_of_junto_is_shared_through_the_window_without_its_sound(cx: &mut TestAppContext) {
    let second = SecondStart::open();
    let capture = junto_media::share::system();
    assert_eq!(capture.access(), ScreenAccess::Allowed);
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

    window.click("call-share");
    let title = second.title.clone();
    let (shared, application, clients, sounds) =
        window.until_value("the window of the second start is listed", |window| {
            window.in_shell(|shell, _, _| {
                let sources = shell.share_picker_sources()?;
                let listed = sources
                    .windows
                    .iter()
                    .find(|listed| listed.title == title)?;
                Some((
                    listed.id,
                    listed.application.clone(),
                    sources.clients.clone(),
                    sources.applications.clone(),
                ))
            })
        });
    // The window belongs to a start of the client: named among the clients, and its
    // application is not offered for its sound.
    assert!(clients.contains(&application), "{clients:?}");
    assert!(sounds.iter().all(|listed| listed.id != application));
    window.click("share-tab-window");
    window.click_named(format!("share-window:{shared}"));
    let help = window.in_shell(|shell, _, _| shell.share_picker_sound_help());
    assert_eq!(
        help.as_deref(),
        Some(junto_client::text::SOUND_OF_JUNTO_NOT_SHARED)
    );
    // "This application" is not offered: the sound offers no sound and all system sound.
    assert!(!window.shows("share-sound-2"));
    window.click("share-sound-0");
    window.click("start-share");
    window.until("the share of the window runs", |window| {
        own_share(window) == OwnShare::Running
    });
    window.until("the stage states that the share shows Junto", |window| {
        stage(window).is_some_and(|stage| {
            stage
                .shares
                .iter()
                .any(|share| share.shows_the_client && share.sound.is_some())
        }) && window.shows("share-shows-junto")
    });
    // The window of the second start is yellow.
    window.until(
        "the administrator receives the pictures of the window",
        |_| {
            caller
                .calls
                .main_picture()
                .borrow()
                .as_ref()
                .is_some_and(|picture| {
                    let frame = &picture.frame;
                    frame
                        .pixel(frame.width / 2, frame.height / 2)
                        .is_some_and(|(red, green, blue)| red > 150 && green > 150 && blue < 100)
                })
        },
    );
    window.click("leave-call");
    window.until("the window left the call", |window| {
        window.view().call.is_none()
    });
    drop(second);
}
