//! Keeping a window above the windows of other programs (ADR 0507).
//!
//! The detached call window asks for this when it opens and when the person changes the
//! choice. The request changes only the place of the window in the stack: it does not give
//! the window the focus, and nothing of the call depends on it.

use gpui::{App, Window};

#[cfg(target_os = "linux")]
mod x11;

/// What the system did with the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeepAbove {
    /// The window manager was asked, and it states that it keeps windows above others.
    Applied,
    /// The system has no such request for an open window: Wayland has none at all, and on
    /// the other systems the window is opened again with another kind (ADR 0507).
    Unsupported,
    /// The request could not be made.
    Failed(String),
}

/// Whether the choice "keep on top" can be offered in this session.
pub fn offered(cx: &App) -> bool {
    #[cfg(target_os = "linux")]
    {
        cx.compositor_name() == "X11"
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = cx;
        true
    }
}

/// Asks the system to keep the window above other windows, or to stop doing so. Nothing
/// of the call depends on the answer, which is written to the log.
///
/// On X11 the request opens a connection to the display and waits for several answers of
/// the display server. It runs on a thread of its own, so the thread of the window does not
/// wait for them (ADR 0730), and the requests are sent in the order in which they were
/// asked for.
pub fn keep_above(window: &mut Window, above: bool, cx: &mut App) {
    #[cfg(target_os = "linux")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let identifier = match HasWindowHandle::window_handle(&*window) {
            Ok(handle) => match handle.as_raw() {
                RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
                _ => None,
            },
            Err(_) => None,
        };
        let Some(identifier) = identifier else {
            told(&KeepAbove::Unsupported);
            return;
        };
        // gpui can be connected to another display than the one of the process environment.
        let display = cx
            .graphical_environment()
            .and_then(|environment| environment.x11_display)
            .and_then(|display| display.into_string().ok());
        let request = Request {
            display,
            window: identifier,
            above,
        };
        match requests(cx) {
            Some(requests) if requests.send(request).is_ok() => {}
            _ => told(&KeepAbove::Failed(
                "the thread of the requests did not start".to_owned(),
            )),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (window, above, cx);
        told(&KeepAbove::Unsupported);
    }
}

fn told(outcome: &KeepAbove) {
    match outcome {
        KeepAbove::Applied => {}
        KeepAbove::Unsupported => {
            tracing::warn!("the window system keeps no window above the others");
        }
        KeepAbove::Failed(failure) => {
            tracing::warn!(%failure, "the call window was not kept above the others");
        }
    }
}

/// One request for the state "above" of a window.
#[cfg(target_os = "linux")]
struct Request {
    display: Option<String>,
    window: u32,
    above: bool,
}

/// The queue of the thread that sends the requests to the display server.
#[cfg(target_os = "linux")]
struct Requests(std::sync::mpsc::Sender<Request>);

#[cfg(target_os = "linux")]
impl gpui::Global for Requests {}

/// The queue of the requests. The thread starts with the first request of the process.
#[cfg(target_os = "linux")]
fn requests(cx: &mut App) -> Option<std::sync::mpsc::Sender<Request>> {
    if let Some(held) = cx.try_global::<Requests>() {
        return Some(held.0.clone());
    }
    let (sender, receiver) = std::sync::mpsc::channel::<Request>();
    let started = std::thread::Builder::new()
        .name("window-level".into())
        .spawn(move || {
            for request in receiver {
                let outcome =
                    match x11::set_above(request.display.as_deref(), request.window, request.above)
                    {
                        Ok(true) => KeepAbove::Applied,
                        Ok(false) => KeepAbove::Unsupported,
                        Err(failure) => KeepAbove::Failed(failure),
                    };
                told(&outcome);
            }
        });
    if let Err(error) = started {
        tracing::warn!(%error, "the thread of the window level did not start");
        return None;
    }
    cx.set_global(Requests(sender.clone()));
    Some(sender)
}
