//! The taskbar of Windows (ADR 0755): the main window states the identifier of the
//! application and what a pin made from its taskbar button starts, and takes them away before
//! it closes.

use gpui::Window;

fn handle(window: &Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => Some(handle.hwnd.get()),
        _ => None,
    }
}

/// Gives the main window the identifier and the relaunch properties of this program.
pub fn identify(window: &Window) {
    let Some(handle) = handle(window) else {
        return;
    };
    let program = match std::env::current_exe() {
        Ok(program) => program.display().to_string(),
        Err(error) => {
            tracing::warn!(%error, "the taskbar properties were not set: no program path");
            return;
        }
    };
    let relaunch = junto_client_core::taskbar::Relaunch {
        identifier: junto_client_core::app_identity::APP_ID,
        command: &format!("\"{program}\""),
        name: crate::text::APP_NAME,
        icon: &format!("{program},0"),
    };
    if let Err(error) = junto_client_core::taskbar::set(handle, &relaunch) {
        tracing::warn!(%error, "the taskbar properties were not set");
    }
    junto_client_core::taskbar::mark_pins_of_this_program(junto_client_core::app_identity::APP_ID);
}

/// Takes the properties of [`identify`] away, before the window is destroyed.
pub fn forget(window: &Window) {
    if let Some(handle) = handle(window)
        && let Err(error) = junto_client_core::taskbar::clear(handle)
    {
        tracing::warn!(%error, "the taskbar properties were not taken away");
    }
}
