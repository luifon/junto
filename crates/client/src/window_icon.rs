//! The icon that the windows of the client give to an X11 window manager (ADR 0756). On
//! Wayland the compositor takes the icon from the menu entry of the application identifier,
//! on Windows gpui takes it from the icon group of the program, and on macOS the system takes
//! it from the bundle; the windows there get none.

use std::sync::Arc;

use image::RgbaImage;

/// The PNG rendering of the application icon that the build script writes, at
/// `junto_icon::WINDOW` pixels.
#[cfg(target_os = "linux")]
const PICTURE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/window-icon.png"));

/// The icon for the options of a new window: on Linux the rendering of the application icon,
/// decoded once; elsewhere none.
pub fn icon() -> Option<Arc<RgbaImage>> {
    #[cfg(target_os = "linux")]
    {
        static ICON: std::sync::OnceLock<Option<Arc<RgbaImage>>> = std::sync::OnceLock::new();
        ICON.get_or_init(|| match decoded(PICTURE) {
            Ok(picture) => Some(Arc::new(picture)),
            Err(error) => {
                tracing::warn!(%error, "the window icon was not decoded");
                None
            }
        })
        .clone()
    }
    #[cfg(not(target_os = "linux"))]
    None
}

#[cfg(target_os = "linux")]
fn decoded(bytes: &[u8]) -> Result<RgbaImage, image::ImageError> {
    image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map(|picture| picture.into_rgba8())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn the_window_icon_is_the_rendering_of_the_application_icon() {
        let picture = icon().unwrap();
        assert_eq!(picture.dimensions(), (64, 64));
        // The middle of the area that all three discs share is light, and the corner outside
        // the rounded square is transparent.
        assert_eq!(picture.get_pixel(32, 33).0, [0xfa, 0xf5, 0xea, 255]);
        assert_eq!(picture.get_pixel(0, 0).0[3], 0);
        assert!(Arc::ptr_eq(&picture, &icon().unwrap()));
    }
}
