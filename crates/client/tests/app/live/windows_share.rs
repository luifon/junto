//! A share of a window of another program through the window, with the capture of Windows
//! (ADR 0706). The window lists what the system offers on the executor of gpui, starts the
//! share with the sound of the application of the window as a person does, and the
//! administrator receives its pictures; closing the shared window ends the share. The
//! shared window is the one of the fixture program of the media crate, which the build of
//! every target of the workspace makes beside this test (`junto-share-fixture.exe`). The
//! fixture plays no sound.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

use gpui::TestAppContext;
use junto_client::state::view::OwnShare;
use junto_client_core::protocol::call::CallPlace;
use junto_media::calls::{CallTarget, Phase};
use junto_media::share::ScreenAccess;

use super::{ROOM, enter, own_share, room};
use crate::calls::voice_channel;
use crate::harness::{World, member_with_capture};

/// The window of the fixture program; its process ends when the value is dropped.
struct Fixture {
    child: Child,
    _adopted: crate::limit::Adopted,
    input: ChildStdin,
    title: String,
}

impl Fixture {
    fn open() -> Self {
        let program = std::env::current_exe()
            .unwrap()
            .parent()
            .and_then(|deps| deps.parent())
            .unwrap()
            .join("junto-share-fixture.exe");
        assert!(
            program.exists(),
            "{} is missing: build every target of the workspace first",
            program.display()
        );
        let title = format!("Junto window share {}", std::process::id());
        let mut child = Command::new(program)
            .arg(&title)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let adopted = crate::limit::adopt(child.id());
        let input = child.stdin.take().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert!(line.starts_with("ready "), "the fixture wrote {line:?}");
        Self {
            child,
            _adopted: adopted,
            input,
            title,
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn near(got: (u8, u8, u8), expected: (u8, u8, u8)) -> bool {
    let close = |a: u8, b: u8| a.abs_diff(b) <= 30;
    close(got.0, expected.0) && close(got.1, expected.1) && close(got.2, expected.2)
}

#[gpui::test]
#[ignore = "needs a LiveKit server and a Windows desktop session"]
fn a_share_of_a_window_of_another_program_runs_through_the_window_and_ends_with_it(
    cx: &mut TestAppContext,
) {
    let mut fixture = Fixture::open();
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
    let title = fixture.title.clone();
    let shared = window.until_value("the window of the fixture is listed", |window| {
        window.in_shell(|shell, _, _| {
            shell.share_picker_sources().and_then(|sources| {
                sources
                    .windows
                    .iter()
                    .find(|listed| listed.title == title)
                    .map(|listed| listed.id)
            })
        })
    });
    window.click_named(format!("share-window:{shared}"));
    // No sound, all system sound, this application.
    window.click("share-sound-2");
    window.click("start-share");
    window.until("the share of the window runs", |window| {
        own_share(window) == OwnShare::Running
    });
    // The inner area of the fixture fills the lower part of the picture: its bottom left
    // quarter is blue and its bottom right quarter is yellow.
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
                    let at = |x: f64, y: f64| {
                        frame.pixel(
                            (f64::from(frame.width) * x) as u32,
                            (f64::from(frame.height) * y) as u32,
                        )
                    };
                    at(0.25, 0.8).is_some_and(|pixel| near(pixel, (40, 60, 200)))
                        && at(0.75, 0.8).is_some_and(|pixel| near(pixel, (220, 200, 40)))
                })
        },
    );

    // The person closes the shared window: the share ends, and nothing starts again.
    writeln!(fixture.input, "close").unwrap();
    fixture.input.flush().unwrap();
    window.until("the share ended with the window", |window| {
        own_share(window) == OwnShare::Off
    });
    window.click("leave-call");
    window.until("the window left the call", |window| {
        window.view().call.is_none()
    });
}
