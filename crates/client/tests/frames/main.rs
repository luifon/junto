//! The graphics memory of the windows while the pictures of a call arrive (ADR 0774). Two
//! native windows of the application join a call on the server of the tests with its media
//! server. Both share the synthetic screen and turn on the synthetic camera, and each
//! watches the share of the other, so the windows draw every kind of picture of a call:
//! the watched share, the small pictures of both shares, the own camera and the camera of
//! the other person. The program gives the windows frames of the display for a time, first
//! with the call in the main window of A and then in the separate call window of A, and
//! reads with `footprint` the memory of the pictures of the process: the graphics memory,
//! which holds the images of gpui in the atlas, and the memory of the pixel buffers of Core
//! Video, which hold the watched share and the cameras that the windows draw as surfaces.
//! Then the shares and the cameras end, and the program reads the memory again.
//!
//! It fails when the memory of the pictures grows with the number of drawn pictures, or
//! when it stays after the shares and the cameras have ended.
//!
//! It runs on the main thread, which the windows of the system need, so it is a test
//! without the test runner, and it reads no filter of test names. It does nothing on other
//! systems than macOS, and nothing unless `JUNTO_TEST_FRAMES` is `1` and the variables of
//! the media server of the tests are set:
//!
//! ```sh
//! cargo build --workspace
//! JUNTO_TEST_FRAMES=1 tools/with-test-postgres.sh tools/with-test-livekit.sh \
//!     tools/serial.sh cargo test -p junto-client --test frames
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]
// A test waits for its files by design; the refusals of the file system in `clippy.toml`
// are for the code of the application (ADR 0730).
#![allow(clippy::disallowed_methods)]

use std::process::ExitCode;

// The visual test context of gpui exists only on macOS, so everything below except the
// `main` of other systems is compiled there only (ADR 0902).
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/recording_server.rs"]
mod recording_server;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/support.rs"]
mod support;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../app/harness.rs"]
mod harness;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/limit.rs"]
mod limit;
#[cfg(target_os = "macos")]
#[path = "../rehearsal/native.rs"]
mod native;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../rehearsal/process.rs"]
mod process;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../app/rehearsal.rs"]
mod rehearsal;

#[cfg(target_os = "macos")]
use rehearsal::{Deployment, Rehearsal};

/// How long the windows draw the pictures of the call in each place of the call.
#[cfg(target_os = "macos")]
const DRAWING: Duration = Duration::from_secs(20);
/// The most memory that the pictures of the call may add while the windows draw them for
/// `DRAWING`, after the first frames. A window holds a constant number of
/// pictures; the memory that one picture of the watched share takes is 3.5 MiB.
#[cfg(target_os = "macos")]
const MOST_GROWTH: u64 = 48 << 20;

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    println!("frames: not run, the native windows run only on macOS");
    ExitCode::SUCCESS
}

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    if std::env::var("JUNTO_TEST_FRAMES").as_deref() != Ok("1") {
        println!("frames: not run, JUNTO_TEST_FRAMES is not 1");
        return ExitCode::SUCCESS;
    }
    if std::env::var("JUNTO_TEST_LIVEKIT_URL").is_err() {
        println!("frames: not run, JUNTO_TEST_LIVEKIT_URL is not set");
        return ExitCode::SUCCESS;
    }
    let mut native = native::Native::start();
    let world = harness::World::new();
    let server = world.server_with_media("Frames");
    let profiles = tempfile::tempdir().unwrap();
    let deployment = Deployment {
        address: server.address(),
        owner_code: server.owner_code().unwrap(),
    };
    let mut run = Rehearsal::new(
        &mut native.cx,
        profiles.path(),
        deployment,
        true,
        None,
        "frames",
    );
    run.first_owner();
    run.invitation();
    run.call();
    let before = picture_memory();
    run.video();

    let mut failures = Vec::new();
    let mut check = |place: &str, measured: Measured| {
        println!(
            "frames: {place}: {} pictures drawn in {DRAWING:?}, graphics memory {} MiB to {} MiB, pixel buffers {} MiB to {} MiB",
            measured.pictures,
            measured.start.graphics >> 20,
            measured.end.graphics >> 20,
            measured.start.surfaces >> 20,
            measured.end.surfaces >> 20
        );
        if measured.pictures < 100 {
            failures.push(format!(
                "{place}: only {} pictures were drawn",
                measured.pictures
            ));
        }
        let grown = measured.end.total().saturating_sub(measured.start.total());
        if grown > MOST_GROWTH {
            failures.push(format!(
                "{place}: the memory of the pictures grew by {} MiB for {} pictures",
                grown >> 20,
                measured.pictures
            ));
        }
    };
    check("the main window", measure(&mut native.cx, &mut run));
    run.a.click("detach-call");
    run.a.until("the call is in its own window", |window| {
        window.shows("call-in-separate-window")
    });
    check("the call window", measure(&mut native.cx, &mut run));

    // The shares and the cameras end. The memory of their pictures is released.
    for window in [&mut run.a, run.b.as_mut().unwrap()] {
        let calls = window
            .state
            .read_with(&window.cx, |state, _| state.calls().clone());
        calls.stop_share(None);
        calls.stop_camera();
    }
    for window in [&mut run.a, run.b.as_mut().unwrap()] {
        window.until("no picture of the call is held", |window| {
            window.state.read_with(&window.cx, |state, _| {
                let calls = state.calls();
                calls.main_picture().borrow().is_none()
                    && calls.small_pictures().borrow().is_empty()
                    && calls.camera_pictures().borrow().is_empty()
            })
        });
    }
    give_frames(&mut run, Duration::from_secs(1));
    let after = picture_memory();
    println!(
        "frames: graphics memory {} MiB before the shares and the cameras, {} MiB after them; pixel buffers {} MiB before, {} MiB after",
        before.graphics >> 20,
        after.graphics >> 20,
        before.surfaces >> 20,
        after.surfaces >> 20
    );
    if after.total().saturating_sub(before.total()) > MOST_GROWTH {
        failures.push(format!(
            "the memory of the pictures is {} MiB higher after the end of the shares and the cameras",
            after.total().saturating_sub(before.total()) >> 20
        ));
    }
    drop(run);
    drop(native);
    // A panic in a handler or a task stops in gpui and no longer ends the program
    // (ADR 0757).
    let panics = junto_client::panics::panics_in_this_process();
    if panics > 0 {
        failures.push(format!(
            "{panics} panics happened in the process: {:?}",
            junto_client::panics::messages_on_this_thread()
        ));
    }
    if failures.is_empty() {
        println!("frames: passed");
        ExitCode::SUCCESS
    } else {
        for failure in &failures {
            println!("frames: failed: {failure}");
        }
        ExitCode::FAILURE
    }
}

#[cfg(target_os = "macos")]
struct Measured {
    /// The pictures that the windows took and drew in the time.
    pictures: usize,
    start: PictureMemory,
    end: PictureMemory,
}

/// The memory of the pictures of the process, in bytes.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
struct PictureMemory {
    /// The dirty memory of the category "IOAccelerator (graphics)": the GPU textures of the
    /// windows, among them the images of gpui in the atlas.
    graphics: u64,
    /// The dirty memory of the category "IOSurface": the pixel buffers of Core Video.
    surfaces: u64,
}

#[cfg(target_os = "macos")]
impl PictureMemory {
    fn total(self) -> u64 {
        self.graphics + self.surfaces
    }
}

/// Gives the windows frames for one second, then for `DRAWING`, and reads the memory of
/// the pictures at the start and at the end of `DRAWING`.
#[cfg(target_os = "macos")]
fn measure(cx: &mut gpui::TestAppContext, run: &mut Rehearsal) -> Measured {
    give_frames(run, Duration::from_secs(1));
    let (drawn, start) = (picture_draws(cx), picture_memory());
    give_frames(run, DRAWING);
    Measured {
        pictures: picture_draws(cx) - drawn,
        start,
        end: picture_memory(),
    }
}

/// Gives the windows frames of the display for this time (`process::give_frames`).
#[cfg(target_os = "macos")]
fn give_frames(run: &mut Rehearsal, time: Duration) {
    process::give_frames(&mut run.a, time);
    run.a.run_tasks();
}

/// The drawings that the pictures of the calls caused, in all windows.
#[cfg(target_os = "macos")]
fn picture_draws(cx: &mut gpui::TestAppContext) -> usize {
    use gpui::AppContext as _;
    let mut draws = 0;
    for handle in cx.windows() {
        draws += cx
            .update_window(handle, |root, _, cx| {
                let root = root.downcast::<gpui_base::Root>().ok()?;
                let shell = root
                    .read(cx)
                    .view()
                    .clone()
                    .downcast::<junto_client::shell::Shell>()
                    .ok()?;
                Some(shell.read(cx).draws().1)
            })
            .ok()
            .flatten()
            .unwrap_or_default();
    }
    draws
}

/// The memory of the pictures of this process, as `footprint` states it.
#[cfg(target_os = "macos")]
fn picture_memory() -> PictureMemory {
    let (graphics, surfaces) = process::picture_memory();
    PictureMemory { graphics, surfaces }
}
