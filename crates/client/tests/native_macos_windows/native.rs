use gpui::{App, AsyncApp, Entity, WindowBounds};
use junto_client::{
    app::{self, WindowStart},
    assets::Assets,
    shell::detached,
    state::AppState,
};
use junto_client_core::{Client, ClientOptions, window_placement::WindowPlacement};
use junto_media::{
    audio::synthetic::SyntheticHost,
    calls::{CallTiming, MediaOptions},
    camera::synthetic::SyntheticCamera,
    permission::FixedPermission,
    share::synthetic::SyntheticCapture,
};
use objc2::{MainThreadMarker, rc::Retained};
use objc2_app_kit::{
    NSApplication, NSApplicationDelegate, NSWindow, NSWindowButton, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSSize};
use std::{
    cell::RefCell,
    path::Path,
    process::{Command, ExitCode},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

pub fn run() -> ExitCode {
    if std::env::var("JUNTO_TEST_NATIVE_MACOS_WINDOWS").as_deref() != Ok("1") {
        println!("native_macos_windows: not run, its variable is not 1");
        return ExitCode::SUCCESS;
    }
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = if args.first().is_some_and(|arg| arg == "--child") && args.len() == 3 {
        child(args[1] == "restore", Path::new(&args[2]))
    } else {
        let directory = tempfile::tempdir().unwrap();
        ["save", "restore"].into_iter().try_for_each(|phase| {
            let mut child = Command::new(std::env::current_exe().unwrap())
                .args([
                    std::ffi::OsStr::new("--child"),
                    std::ffi::OsStr::new(phase),
                    directory.path().as_os_str(),
                ])
                .spawn()
                .map_err(|error| error.to_string())?;
            let deadline = Instant::now() + Duration::from_secs(90);
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        return if status.success() {
                            Ok(())
                        } else {
                            Err(format!("{phase}: {status}"))
                        };
                    }
                    Ok(None) => {}
                    Err(error) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(error.to_string());
                    }
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{phase}: the native child exceeded 90 seconds"));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        })
    };
    match result {
        Ok(()) => {
            println!("native_macos_windows: passed");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("native_macos_windows: {error}");
            ExitCode::FAILURE
        }
    }
}

fn windows() -> Vec<Retained<NSWindow>> {
    NSApplication::sharedApplication(MainThreadMarker::new().unwrap())
        .windows()
        .iter()
        .filter(|window| window.isVisible())
        .collect()
}

/// The main window draws its content up to its top edge, and its headers act as the title bar
/// (ADR 0703): the client asks gpui for a transparent title bar, for the window controls at
/// `MAIN_WINDOW_CONTROLS` and for the move of the window by the application (the options that
/// `open_main_window` gave gpui, `app::OpenedTitleBar`); the native window
/// has a content view of its full size with a transparent title bar, its close button at that
/// place, and the content of the window reaches the band of the title bar.
///
/// That the content view takes the presses in the title bar (`app_owns_titlebar_drag`) is
/// answered by a private method of AppKit (`_opaqueRectForWindowMoveWhenInTitlebar`), which
/// only an untyped message, unsafe code, can call; the test reads that setting in the options
/// that `open_main_window` gave gpui.
fn title_bar(window: &NSWindow, opened: app::OpenedTitleBar, reaches: bool) -> Result<(), String> {
    let controls = app::MAIN_WINDOW_CONTROLS;
    if !opened.app_owns_titlebar_drag
        || !opened.appears_transparent
        || opened.traffic_light_position
            != Some(gpui::point(gpui::px(controls.0), gpui::px(controls.1)))
    {
        return Err(format!(
            "the main window was opened without its own title bar: {opened:?}"
        ));
    }
    if !window
        .styleMask()
        .contains(NSWindowStyleMask::FullSizeContentView)
    {
        return Err("the content view of the main window is not of its full size".into());
    }
    if !window.titlebarAppearsTransparent() {
        return Err("the title bar of the main window is not transparent".into());
    }
    let content = window
        .contentView()
        .ok_or("the main window has no content view")?;
    let layout = window.contentLayoutRect();
    if content.frame().size.height <= layout.size.height + 1.0 {
        return Err(format!(
            "the content of the main window ({:?}) does not reach above its layout area ({layout:?})",
            content.frame()
        ));
    }
    let close = window
        .standardWindowButton(NSWindowButton::CloseButton)
        .ok_or("the main window has no close button")?;
    let place = close.frame().origin;
    if (place.x - f64::from(controls.0)).abs() > 1.0
        || (place.y - f64::from(controls.1)).abs() > 1.0
    {
        return Err(format!(
            "the close button of the main window is at {place:?}, not at {controls:?}"
        ));
    }
    if !reaches {
        return Err(
            "the client does not treat the headers of the main window as its title bar".into(),
        );
    }
    Ok(())
}

async fn until(
    cx: &mut AsyncApp,
    description: &str,
    mut ready: impl FnMut(&mut App) -> bool,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if cx.update(|cx| ready(cx)) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("timed out: {description}"));
        }
        cx.background_executor()
            .timer(Duration::from_millis(20))
            .await;
    }
}

fn placements(state: &Entity<AppState>, cx: &App) -> [WindowPlacement; 2] {
    [
        state.read(cx).main_window_settings(),
        state.read(cx).call_window_settings(),
    ]
}

fn same_geometry(a: WindowPlacement, b: WindowPlacement) -> bool {
    let near = |a: i64, b: i64| (a - b).abs() <= 2;
    near(a.width.into(), b.width.into())
        && near(a.height.into(), b.height.into())
        && a.x.zip(b.x).is_some_and(|(a, b)| near(a.into(), b.into()))
        && a.y.zip(b.y).is_some_and(|(a, b)| near(a.into(), b.into()))
}

fn child(restoring: bool, directory: &Path) -> Result<(), String> {
    junto_client::panics::install();
    let outcome: Rc<RefCell<Option<Result<(), String>>>> = Rc::default();
    let answer = outcome.clone();
    let directory = directory.to_owned();
    let application = gpui_platform::application().with_assets(Assets);
    application.on_reopen(|cx| junto_client::notify::bring_to_front(None, cx));
    let ended = outcome.clone();
    application.run(move |cx| {
        app::init(cx).unwrap();
        // `quit` ends the process inside AppKit with exit code 0, and the client quits when its
        // last window closes: an end before the result is a failure.
        cx.on_app_quit(move |_| {
            if ended.borrow().is_none() {
                eprintln!("native_macos_windows: the application ended before the native checks");
                std::process::exit(1);
            }
            async {}
        })
        .detach();
        let client = Client::open(ClientOptions::new(directory.join("profile"))).unwrap();
        junto_client::panics::record_into(client.data_dir().root());
        let start = WindowStart {
            activate: false,
            ..WindowStart::default()
        };
        let media = MediaOptions {
            host: Arc::new(SyntheticHost::with_tone(440.0)),
            camera: Arc::new(SyntheticCamera::new()),
            permission: Arc::new(FixedPermission::allowed()),
            capture: Arc::new(SyntheticCapture::new()),
            timing: CallTiming::default(),
        };
        let state = app::state_with_media(client, start, media, cx);
        let main = app::open_main_window(state.clone(), start, cx).unwrap();
        cx.spawn(async move |cx| {
            let result = async {
                until(cx, "the native main window appears", |_| {
                    windows().len() == 1
                })
                .await?;
                let main_native = windows().pop().unwrap();
                if !restoring {
                    let (reaches, opened) = cx.update(|cx| {
                        let reaches = main
                            .update(cx, |_, window, cx| {
                                junto_client::shell::title_bar::reaches_title_bar(window, cx)
                            })
                            .unwrap();
                        (reaches, *cx.global::<app::OpenedTitleBar>())
                    });
                    title_bar(&main_native, opened, reaches)?;
                }
                cx.update(|cx| detached::open(state.clone(), cx));
                until(cx, "the detached call window appears", |_| {
                    windows().len() == 2
                })
                .await?;
                let call_native = windows()
                    .into_iter()
                    .find(|window| window.windowNumber() != main_native.windowNumber())
                    .unwrap();
                let call = cx.update(|cx| {
                    cx.windows()
                        .into_iter()
                        .find(|handle| *handle != main.into())
                        .unwrap()
                });
                let expected_file = directory.join("normal.json");
                if !restoring {
                    main_native.setContentSize(NSSize::new(900.0, 600.0));
                    call_native.setContentSize(NSSize::new(600.0, 400.0));
                    until(cx, "both normal placements are observed", |cx| {
                        let saved = placements(&state, cx);
                        saved[0].width == 900
                            && saved[1].width == 600
                            && saved.iter().all(|saved| !saved.maximized)
                    })
                    .await?;
                    let normal = cx.update(|cx| placements(&state, cx));
                    std::fs::write(&expected_file, serde_json::to_vec(&normal).unwrap())
                        .map_err(|error| error.to_string())?;
                    main_native.performZoom(None);
                    call_native.performZoom(None);
                    until(
                        cx,
                        "both native zooms reach the saved maximized flags",
                        |cx| {
                            main_native.isZoomed()
                                && call_native.isZoomed()
                                && placements(&state, cx).iter().all(|saved| saved.maximized)
                        },
                    )
                    .await?;
                    let maximized = cx.update(|cx| placements(&state, cx));
                    if !normal
                        .into_iter()
                        .zip(maximized)
                        .all(|(normal, saved)| same_geometry(normal, saved))
                    {
                        return Err(format!(
                            "maximizing replaced the normal placement: {normal:?} before, \
                             {maximized:?} after"
                        ));
                    }
                } else {
                    let normal: [WindowPlacement; 2] = serde_json::from_slice(
                        &std::fs::read(&expected_file).map_err(|error| error.to_string())?,
                    )
                    .map_err(|error| error.to_string())?;
                    until(cx, "restart restores maximization of both windows", |cx| {
                        main_native.isZoomed()
                            && call_native.isZoomed()
                            && placements(&state, cx).iter().all(|saved| saved.maximized)
                    })
                    .await?;
                    let both_maximized = cx.update(|cx| {
                        main.update(cx, |_, window, _| {
                            matches!(window.window_bounds(), WindowBounds::Maximized(_))
                        })
                        .unwrap()
                            && call
                                .update(cx, |_, window, _| {
                                    matches!(window.window_bounds(), WindowBounds::Maximized(_))
                                })
                                .unwrap()
                    });
                    if !both_maximized {
                        return Err("native maximization is not reported by gpui".into());
                    }
                    main_native.performZoom(None);
                    call_native.performZoom(None);
                    until(
                        cx,
                        "unzoom restores the independent normal placements",
                        |cx| {
                            let saved = placements(&state, cx);
                            !main_native.isZoomed()
                                && !call_native.isZoomed()
                                && saved.iter().all(|saved| !saved.maximized)
                                && normal
                                    .into_iter()
                                    .zip(saved)
                                    .all(|(normal, saved)| same_geometry(normal, saved))
                        },
                    )
                    .await?;
                    // A zoomed window that is moved, then zoomed and unzoomed again: both
                    // keep its size and only move it, so AppKit sends no resize. The window
                    // is then not reported as maximized, and a resize is saved as its normal
                    // placement (review L1, ADR 0757).
                    main_native.performZoom(None);
                    until(cx, "the main window is zoomed again", |cx| {
                        main_native.isZoomed() && placements(&state, cx)[0].maximized
                    })
                    .await?;
                    let frame = main_native.frame();
                    main_native
                        .setFrameOrigin(NSPoint::new(frame.origin.x + 40.0, frame.origin.y - 40.0));
                    until(cx, "the moved window is no longer zoomed", |_| {
                        !main_native.isZoomed()
                    })
                    .await?;
                    main_native.performZoom(None);
                    until(cx, "the moved window is zoomed by a move", |_| {
                        main_native.isZoomed()
                    })
                    .await?;
                    main_native.performZoom(None);
                    until(
                        cx,
                        "a window unzoomed by a move is not reported as maximized",
                        |cx| {
                            !main_native.isZoomed()
                                && !placements(&state, cx)[0].maximized
                                && main
                                    .update(cx, |_, window, _| {
                                        !matches!(
                                            window.window_bounds(),
                                            WindowBounds::Maximized(_)
                                        )
                                    })
                                    .unwrap_or(false)
                        },
                    )
                    .await?;
                    main_native.setContentSize(NSSize::new(820.0, 560.0));
                    until(
                        cx,
                        "a resize after it is saved as the normal placement",
                        |cx| {
                            let saved = placements(&state, cx);
                            saved[0].width == 820 && !saved[0].maximized
                        },
                    )
                    .await?;
                    main_native.performClose(None);
                    until(cx, "only the detached call window remains", |cx| {
                        cx.windows().len() == 1 && call_native.isVisible()
                    })
                    .await?;
                    let application =
                        NSApplication::sharedApplication(MainThreadMarker::new().unwrap());
                    let delegate = application.delegate().ok_or("no application delegate")?;
                    if !delegate.applicationShouldHandleReopen_hasVisibleWindows(&application, true)
                    {
                        return Err("the Dock reopen request was refused".into());
                    }
                    until(
                        cx,
                        "Dock reopening restores and activates main while call stays visible",
                        |cx| {
                            cx.windows().len() == 2
                                && call_native.isVisible()
                                && state.read(cx).is_call_detached()
                                && windows().into_iter().any(|window| {
                                    window.windowNumber() != call_native.windowNumber()
                                        && window.isKeyWindow()
                                })
                        },
                    )
                    .await?;
                }
                Ok(())
            }
            .await
            .and_then(|()| {
                if junto_client::panics::panics_in_this_process() == 0 {
                    Ok(())
                } else {
                    Err("a native window callback panicked".to_owned())
                }
            });
            // `quit` ends the process inside AppKit with exit code 0 and `run` does not
            // return, so a failure is stated here and ends the process with its own code.
            if let Err(error) = &result {
                eprintln!("native_macos_windows: {error}");
                std::process::exit(1);
            }
            *answer.borrow_mut() = Some(result);
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
    let result = outcome.borrow_mut().take();
    result.unwrap_or_else(|| Err("the application ended before the native checks".into()))
}
