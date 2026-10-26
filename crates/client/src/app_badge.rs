//! The count of the messages for the person on the icon of the application (ADR 0741): the
//! badge of the Dock on macOS, the overlay of the taskbar button on Windows, and the count
//! of the launcher entry of a Linux desktop. The main window also writes it into its title.

#[cfg(target_os = "linux")]
mod linux;

/// The title of the main window with a count.
pub fn title(count: u64) -> String {
    match junto_client_core::app_badge::label(count) {
        Some(label) => format!("({label}) {}", crate::text::APP_NAME),
        None => crate::text::APP_NAME.to_owned(),
    }
}

/// Shows a count on the icon of the application. A start for the tests and for the tool
/// that draws the screens shows none on the system.
pub fn show(count: u64, window: &gpui::Window) {
    if cfg!(feature = "test-support") {
        return;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = window;
        junto_client_core::app_badge::set_dock(
            junto_client_core::app_badge::label(count).as_deref(),
        );
    }
    #[cfg(windows)]
    if let Some(handle) = crate::taskbar::handle(window)
        && let Err(error) = junto_client_core::app_badge::set_overlay(
            handle,
            count,
            &crate::text::for_person(count),
        )
    {
        tracing::warn!(%error, "the count of the taskbar button was not set");
    }
    #[cfg(target_os = "linux")]
    {
        let _ = window;
        linux::launcher().set(count);
    }
}
