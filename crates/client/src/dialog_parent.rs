//! The window over which the desktop portal of Linux shows its dialog of a share (ADR 0751),
//! as a window identifier of the portal. An X11 window gives its XID. A Wayland window
//! exports its surface with xdg-foreign; the export lasts as long as the [`DialogParent`].

use std::future::Future;
use std::pin::Pin;

use gpui::Window;

/// A window identifier of the portal, and on Wayland the export that it names.
pub struct DialogParent {
    #[cfg(target_os = "linux")]
    identifier: ashpd::WindowIdentifier,
}

/// The identifier of a window: at once, or after the answer of the Wayland compositor.
pub enum Pending {
    Ready(Option<DialogParent>),
    Waiting(Pin<Box<dyn Future<Output = Option<DialogParent>>>>),
}

impl DialogParent {
    /// The identifier in the form of the portal, such as `x11:2a00004`.
    pub fn identifier(&self) -> String {
        #[cfg(target_os = "linux")]
        {
            self.identifier.to_string()
        }
        #[cfg(not(target_os = "linux"))]
        {
            String::new()
        }
    }

    /// The identifier of `window`. A window without a handle of the desktop, such as a
    /// window of the tests, and a system other than Linux have none.
    #[cfg(target_os = "linux")]
    pub fn of(window: &Window) -> Pending {
        use raw_window_handle::{HasDisplayHandle as _, HasWindowHandle as _};
        let (Ok(handle), Ok(display)) = (window.window_handle(), window.display_handle()) else {
            return Pending::Ready(None);
        };
        let (handle, display) = (handle.as_raw(), display.as_raw());
        let mut identifier = Box::pin(async move {
            ashpd::WindowIdentifier::from_raw_handle(&handle, Some(&display))
                .await
                .map(|identifier| DialogParent { identifier })
        });
        // The first poll reads the surface of the window while the window surely exists;
        // the later polls only wait for the export of the compositor.
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        match identifier.as_mut().poll(&mut context) {
            std::task::Poll::Ready(identifier) => Pending::Ready(identifier),
            std::task::Poll::Pending => Pending::Waiting(identifier),
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn of(_window: &Window) -> Pending {
        Pending::Ready(None)
    }
}
