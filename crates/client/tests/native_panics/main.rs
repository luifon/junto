//! A panic in code that the platform of the system calls (ADR 0757). The application runs
//! on the run loop of the system with the main window of an empty data directory, as the
//! binary does, and:
//!
//! 1. a task on the thread of the windows panics; the platform runs that task from its own
//!    callback (a function of the dispatch queue on macOS, the window procedure on
//!    Windows);
//! 2. on macOS, a press of the left button is posted to the application through AppKit,
//!    and the handler of the end of the press panics inside the method of the view of
//!    gpui, which AppKit calls;
//! 3. on macOS, a second press reaches the handler without a panic.
//!
//! After each panic the application must continue, the report must be in the data
//! directory, and the window must paint the notice with the count of the panics in a frame
//! that ends: the paint is recorded during the draw, and the next frame starts only after the
//! draw and its presentation. Before ADR 0757 the process ended at the first panic.
//!
//! It runs on the main thread, which the windows of the system need, so it is a test
//! without the test runner. It does nothing unless `JUNTO_TEST_NATIVE_PANICS` is `1`:
//!
//! ```sh
//! JUNTO_TEST_NATIVE_PANICS=1 cargo test -p junto-client --test native_panics
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]
// A test reads its files by design; the refusals of the file system in `clippy.toml` are for
// the code of the application (ADR 0730).
#![allow(clippy::disallowed_methods)]

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{AsyncApp, Entity, WindowHandle};
use gpui_base::Root;
use junto_client::app::{self, WindowStart};
use junto_client::assets::Assets;
use junto_client::panics::{self, test_support};
use junto_client::state::AppState;
use junto_client_core::{Client, ClientOptions};
use junto_media::calls::{CallTiming, MediaOptions};

/// How long a step may wait for the application.
const PATIENCE: Duration = Duration::from_secs(20);

fn main() -> ExitCode {
    if std::env::var("JUNTO_TEST_NATIVE_PANICS").as_deref() != Ok("1") {
        println!("native_panics: not run, JUNTO_TEST_NATIVE_PANICS is not 1");
        return ExitCode::SUCCESS;
    }
    let data_dir = tempfile::tempdir().unwrap();
    let outcome: Rc<RefCell<Option<Result<(), String>>>> = Rc::default();
    let result = outcome.clone();
    // The client creates its data directory itself, private to the account.
    let directory = data_dir.path().join("junto");
    gpui_platform::application()
        .with_assets(Assets)
        .run(move |cx| {
            app::init(cx).unwrap();
            let client = Client::open(ClientOptions::new(directory.clone())).unwrap();
            let start = WindowStart {
                activate: false,
                ..WindowStart::default()
            };
            let state = app::state_with_media(client, start, synthetic_media(), cx);
            let window = app::open_main_window(state.clone(), start, cx).unwrap();
            cx.spawn(async move |cx| {
                let checked = stages(&state, window, &directory, cx).await;
                *result.borrow_mut() = Some(checked);
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    let outcome = outcome.borrow_mut().take();
    match outcome {
        Some(Ok(())) => {
            println!("native_panics: passed");
            ExitCode::SUCCESS
        }
        Some(Err(failure)) => {
            eprintln!("native_panics: failed: {failure}");
            ExitCode::FAILURE
        }
        None => {
            eprintln!("native_panics: failed: the application ended before the test did");
            ExitCode::FAILURE
        }
    }
}

fn synthetic_media() -> MediaOptions {
    use junto_media::audio::synthetic::SyntheticHost;
    use junto_media::camera::synthetic::SyntheticCamera;
    use junto_media::permission::FixedPermission;
    use junto_media::share::synthetic::SyntheticCapture;
    use std::sync::Arc;
    MediaOptions {
        camera: Arc::new(SyntheticCamera::new()),
        host: Arc::new(SyntheticHost::with_tone(440.0)),
        permission: Arc::new(FixedPermission::allowed()),
        capture: Arc::new(SyntheticCapture::new()),
        timing: CallTiming::default(),
    }
}

async fn stages(
    state: &Entity<AppState>,
    window: WindowHandle<Root>,
    data_dir: &Path,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    let at_start = panics::panics_in_this_process();
    let notices = |cx: &mut AsyncApp| {
        state.read_with(&*cx, |state, _| {
            state.panic_notice().map(|notice| notice.count)
        })
    };
    wait_for_a_frame(window, cx).await?;

    println!("native_panics: a task on the thread of the windows panics");
    cx.spawn(async move |_| {
        panic!("{}", test_support::MESSAGE);
    })
    .detach();
    until("the notice of the panic of the task", cx, |cx| {
        notices(cx) == Some(1)
    })
    .await?;
    check_reports(data_dir, 1)?;
    wait_for_a_drawn_notice(window, 1, cx).await?;

    #[cfg(target_os = "macos")]
    {
        println!("native_panics: the end of a press, posted through AppKit, panics");
        test_support::panic_in_the_next(test_support::Handler::PressEnd);
        appkit::post_press()?;
        until("the notice of the panic of the press", cx, |cx| {
            notices(cx) == Some(2)
        })
        .await?;
        check_reports(data_dir, 2)?;
        wait_for_a_drawn_notice(window, 2, cx).await?;

        println!("native_panics: a second press, without a panic");
        let press_ends = test_support::press_ends();
        appkit::post_press()?;
        until("the second press reaches its handler", cx, |_| {
            test_support::press_ends() > press_ends
        })
        .await?;
    }

    let expected = if cfg!(target_os = "macos") { 2 } else { 1 };
    wait_for_a_drawn_notice(window, expected, cx).await?;
    let panics = panics::panics_in_this_process() - at_start;
    if panics != expected {
        return Err(format!(
            "{panics} panics happened, {expected} were expected"
        ));
    }
    Ok(())
}

/// Waits until the window has drawn a frame after this call.
async fn wait_for_a_frame(window: WindowHandle<Root>, cx: &mut AsyncApp) -> Result<(), String> {
    let drawn = Rc::new(Cell::new(false));
    let seen = drawn.clone();
    window
        .update(cx, |_, window, _| {
            window.on_next_frame(move |_, _| seen.set(true));
            window.refresh();
        })
        .map_err(|error| error.to_string())?;
    until("a frame of the window", cx, |_| drawn.get()).await
}

/// Waits until a frame of the window painted the notice of `panics` panics and the frame
/// after it started. The first frame callback runs before the draw of its frame and the
/// second before the draw of the next frame, which starts after the first frame was drawn
/// and presented; the count of paints of the notice must grow between the two.
async fn wait_for_a_drawn_notice(
    window: WindowHandle<Root>,
    panics: usize,
    cx: &mut AsyncApp,
) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let proved = Rc::new(Cell::new(None::<bool>));
        let seen = proved.clone();
        window
            .update(cx, |_, window, _| {
                window.on_next_frame(move |window, _| {
                    let before = test_support::notice_paints().0;
                    window.on_next_frame(move |_, _| {
                        let (paints, stated) = test_support::notice_paints();
                        seen.set(Some(paints > before && stated == panics));
                    });
                    window.refresh();
                });
                window.refresh();
            })
            .map_err(|error| error.to_string())?;
        until("two frames of the window", cx, |_| proved.get().is_some()).await?;
        if proved.get() == Some(true) {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!(
                "after {PATIENCE:?} no frame painted the notice of {panics} panics: {:?}",
                test_support::notice_paints()
            ));
        }
    }
}

async fn until(
    what: &str,
    cx: &mut AsyncApp,
    mut holds: impl FnMut(&mut AsyncApp) -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if holds(cx) {
            return Ok(());
        }
        if Instant::now() > deadline {
            return Err(format!("after {PATIENCE:?} this did not hold: {what}"));
        }
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    }
}

fn check_reports(data_dir: &Path, count: usize) -> Result<(), String> {
    let mut reports: Vec<PathBuf> = std::fs::read_dir(panics::reports_directory(data_dir))
        .map_err(|error| format!("the reports cannot be read: {error}"))?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .collect();
    reports.sort();
    if reports.len() != count {
        return Err(format!("{count} reports were expected: {reports:?}"));
    }
    for report in reports {
        let text = std::fs::read_to_string(&report).map_err(|error| error.to_string())?;
        let message = format!("message: {}", test_support::MESSAGE);
        if !text.contains(&message) || !text.contains("backtrace:") {
            return Err(format!(
                "{} does not hold the panic:\n{text}",
                report.display()
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod appkit {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSEvent, NSEventModifierFlags, NSEventType};
    use objc2_foundation::NSPoint;

    /// Posts a press of the left button at the middle of the window of the application to
    /// the event queue of the application. AppKit delivers it as it delivers a press of a
    /// person, through the methods of the view of gpui.
    pub fn post_press() -> Result<(), String> {
        let main_thread =
            MainThreadMarker::new().ok_or("the test does not run on the main thread")?;
        let application = NSApplication::sharedApplication(main_thread);
        let window = application
            .windows()
            .iter()
            .find(|window| window.isVisible())
            .ok_or("the application has no visible window")?;
        let content = window
            .contentView()
            .ok_or("the window has no content view")?
            .frame();
        let middle = NSPoint::new(content.size.width / 2.0, content.size.height / 2.0);
        for kind in [NSEventType::LeftMouseDown, NSEventType::LeftMouseUp] {
            let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                kind,
                middle,
                NSEventModifierFlags::empty(),
                0.0,
                window.windowNumber(),
                None,
                0,
                1,
                1.0,
            )
            .ok_or("AppKit made no event")?;
            application.postEvent_atStart(&event, false);
        }
        Ok(())
    }
}
