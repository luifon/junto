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

/// Asks the system to keep the window above other windows, or to stop doing so.
pub fn keep_above(window: &mut Window, above: bool, cx: &mut App) -> KeepAbove {
    #[cfg(target_os = "linux")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        let identifier = match HasWindowHandle::window_handle(&*window) {
            Ok(handle) => match handle.as_raw() {
                RawWindowHandle::Xcb(handle) => handle.window.get(),
                _ => return KeepAbove::Unsupported,
            },
            Err(_) => return KeepAbove::Unsupported,
        };
        // gpui can be connected to another display than the one of the process environment.
        let display = cx
            .graphical_environment()
            .and_then(|environment| environment.x11_display)
            .and_then(|display| display.into_string().ok());
        match x11::set_above(display.as_deref(), identifier, above) {
            Ok(true) => KeepAbove::Applied,
            Ok(false) => KeepAbove::Unsupported,
            Err(failure) => KeepAbove::Failed(failure),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (window, above, cx);
        KeepAbove::Unsupported
    }
}
