//! The share of every window of one application on Windows (ADR 0738).
//!
//! Windows Graphics Capture captures one screen or one window. Each window of the shared
//! application that shows a part of the shared screen therefore has a capture of its own,
//! and the delivery draws the newest picture of each window at its place on the screen into
//! one picture of the size of the screen, from the back to the front, with the rest black.
//! The windows of the application are read again every [`FOLLOW`], so a window that opens,
//! closes, moves or changes its size is followed. This module calls the functions of the
//! other modules of the capture and has no unsafe code of its own.

use std::collections::HashSet;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::HWND;

use super::capture::{Ready, Thread, apartment, finish};
use super::graphics::{Gpu, Session, Target};
use super::rules::{self, Layer};
use super::sources;
use super::still::{self, Still};
use crate::share::delivery::pixels::{Format, Frame};
use crate::share::delivery::{Latest, Life};
use crate::share::listing::{Bounds, Own, application_pictures};
use crate::share::picture::PictureFrame;
use crate::share::{ApplicationId, CaptureError};

/// How often the windows of a shared application are read again. The wait of the message
/// loop of the picture thread, at most 100 ms, adds to it.
const FOLLOW: Duration = Duration::from_millis(250);

/// The application and the screen of a share of an application.
pub(super) struct Shared {
    application: (ApplicationId, String),
    screen: Bounds,
    /// The size of the picture in pixels: the size of the screen.
    pub size: (u32, u32),
}

/// The application of the process `process_id` and the screen of its share, as the dialog
/// of a share lists them now: a share starts only with an application that a person could
/// choose.
pub(super) fn resolve(own: &Own, process_id: i32) -> Result<Shared, CaptureError> {
    let placed = sources::placed_windows(own);
    let displays: Vec<_> = sources::displays()
        .into_iter()
        .map(|display| (display.source, display.bounds))
        .collect();
    let listed = application_pictures(&placed, &displays)
        .into_iter()
        .find(|listed| listed.application.process_id == process_id)
        .ok_or(CaptureError::SourceGone)?;
    let (_, screen) = displays
        .iter()
        .find(|(display, _)| display.id == listed.display)
        .ok_or(CaptureError::SourceGone)?;
    Ok(Shared {
        application: (listed.application, listed.name),
        screen: *screen,
        size: (listed.width, listed.height),
    })
}

/// The windows of the application on its screen, from the back to the front, each at its
/// place in the picture.
fn layers(own: &Own, shared: &Shared) -> Vec<Layer> {
    let front_to_back: Vec<(u32, Bounds)> = sources::application_windows(own, &shared.application)
        .into_iter()
        .map(|(window, bounds)| (window.id, bounds))
        .collect();
    rules::application_layers(&front_to_back, shared.screen)
}

fn handle(window: u32) -> HWND {
    HWND(rules::window_handle(window) as *mut c_void)
}

/// The pictures that the delivery draws: the newest picture of each window, at its place.
pub(super) type Drawn = Arc<Mutex<Vec<(Arc<Gpu>, (i32, i32))>>>;

/// One captured window of the shared application.
struct Captured {
    window: u32,
    life: Arc<Life>,
    session: Session,
}

/// The picture thread of a share of an application: it captures each window of the
/// application on the shared screen and follows them until the share stops. The capture
/// of one window that ends, as when the window is closed, ends that window only.
pub(super) fn picture(
    life: Arc<Life>,
    own: Own,
    shared: Shared,
    pictures: Arc<Latest<()>>,
    drawn: Drawn,
    ready: Ready<()>,
) {
    let mut ready = Some(ready);
    let result = (|| {
        apartment()?;
        let owner = Thread::current();
        let waker = owner.waker(&life)?;
        life.on_stop(move || {
            if let Err(error) = waker.wake() {
                tracing::warn!(%error, "the picture wake failed; the bounded wait will check cancellation");
            }
        });
        let mut captured: Vec<Captured> = Vec::new();
        // Windows whose capture did not start; they are tried again when they are listed
        // anew after they left the list.
        let mut refused: HashSet<u32> = HashSet::new();
        let mut next = Instant::now();
        while !life.stopped() {
            if Instant::now() >= next {
                follow(
                    &own,
                    &shared,
                    &life,
                    &pictures,
                    &drawn,
                    &mut captured,
                    &mut refused,
                );
                next = Instant::now() + FOLLOW;
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Ok(()));
                }
            }
            owner.pump()?;
        }
        for window in captured {
            end(window, &life);
        }
        Ok(())
    })();
    finish(&life, &mut ready, result);
}

/// Ends the capture of one window, and gives a failure of its cleanup to the share.
fn end(window: Captured, life: &Life) {
    window.life.stop();
    let Captured {
        life: window_life,
        session,
        ..
    } = window;
    drop(session);
    if let Err(failure) = window_life.cleanup_result() {
        life.cleanup_failed(failure);
    }
}

/// Reads the windows of the application again, ends the captures of the windows that are
/// gone or no longer on the screen, starts the captures of new windows, and gives the
/// delivery the windows in their order and at their places.
fn follow(
    own: &Own,
    shared: &Shared,
    life: &Life,
    pictures: &Arc<Latest<()>>,
    drawn: &Drawn,
    captured: &mut Vec<Captured>,
    refused: &mut HashSet<u32>,
) {
    let layers = layers(own, shared);
    refused.retain(|window| layers.iter().any(|layer| layer.window == *window));
    let (kept, gone): (Vec<_>, Vec<_>) = std::mem::take(captured).into_iter().partition(|held| {
        !held.life.stopped() && layers.iter().any(|layer| layer.window == held.window)
    });
    *captured = kept;
    for window in gone {
        if let Err(failure) = window.life.result() {
            tracing::info!(%failure, window = window.window, "a window of the shared application ended");
        }
        end(window, life);
    }
    for layer in &layers {
        if captured.iter().any(|held| held.window == layer.window)
            || refused.contains(&layer.window)
        {
            continue;
        }
        let window_life = Arc::new(Life::default());
        match Session::start(
            Target::Window(handle(layer.window)),
            window_life.clone(),
            pictures.clone(),
        ) {
            Ok(session) => captured.push(Captured {
                window: layer.window,
                life: window_life,
                session,
            }),
            Err(error) => {
                tracing::warn!(%error, window = layer.window, "a window of the shared application could not be captured");
                refused.insert(layer.window);
            }
        }
    }
    let now: Vec<(Arc<Gpu>, (i32, i32))> = layers
        .iter()
        .filter_map(|layer| {
            captured
                .iter()
                .find(|held| held.window == layer.window)
                .map(|held| (held.session.gpu.clone(), layer.at))
        })
        .collect();
    let mut held = drawn.lock().unwrap_or_else(PoisonError::into_inner);
    let same = held.len() == now.len()
        && held
            .iter()
            .zip(&now)
            .all(|(one, other)| Arc::ptr_eq(&one.0, &other.0) && one.1 == other.1);
    if !same {
        *held = now;
        drop(held);
        // A window that moved sends no new picture of itself: the picture is drawn anew.
        pictures.put(());
    }
}

/// Draws the newest picture of each window into one picture of `size` pixels. A window
/// whose picture cannot be read is left out of this picture.
pub(super) fn draw(drawn: &Drawn, size: (u32, u32)) -> Result<Option<Frame>, CaptureError> {
    let layers = drawn.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let mut frames = Vec::with_capacity(layers.len());
    for (gpu, at) in &layers {
        match gpu.read() {
            Ok(Some(frame)) => frames.push((frame, *at)),
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, "a window of the shared application was not read"),
        }
    }
    let placed: Vec<(&Frame, (i32, i32))> = frames.iter().map(|(frame, at)| (frame, *at)).collect();
    Frame::compose(size, &placed).map(Some)
}

/// The preview of a shared application: a still picture of each of its windows on its
/// screen, drawn at its place into a picture of at most `bound` pixels with the
/// proportions of the screen.
pub(super) fn preview(
    own: &Own,
    process_id: i32,
    bound: (u32, u32),
) -> Result<PictureFrame, CaptureError> {
    let shared = resolve(own, process_id)?;
    let (width, height) = crate::share::picture::fit(shared.size, bound);
    let scale = f64::from(width) / f64::from(shared.size.0.max(1));
    let windows: std::collections::HashMap<u32, Bounds> =
        sources::application_windows(own, &shared.application)
            .into_iter()
            .map(|(window, bounds)| (window.id, bounds))
            .collect();
    let mut stills = Vec::new();
    for layer in layers(own, &shared) {
        let Some(bounds) = windows.get(&layer.window) else {
            continue;
        };
        let wanted = (
            ((bounds.width * scale).round() as u32).max(1),
            ((bounds.height * scale).round() as u32).max(1),
        );
        let Ok(picture) = still::picture(&Still::Window(handle(layer.window)), wanted) else {
            continue;
        };
        let at = (
            (f64::from(layer.at.0) * scale).round() as i32,
            (f64::from(layer.at.1) * scale).round() as i32,
        );
        let frame = Frame::copy(
            &picture.bgra,
            (picture.width, picture.height),
            (picture.width * 4) as i32,
            None,
            Format::Bgra,
        )?;
        stills.push((frame, at));
    }
    let placed: Vec<(&Frame, (i32, i32))> = stills.iter().map(|(frame, at)| (frame, *at)).collect();
    Ok(Frame::compose((width, height), &placed)?.into_picture(shared.size))
}
