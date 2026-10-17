//! The GPU copies of the images that the windows draw (ADR 0774).

use std::sync::Arc;

use gpui::{App, RenderImage};

/// Removes the GPU copies of an image from every window. Call it when no window draws the
/// image any more: the window copies an image into its GPU memory the first time that it
/// draws it, and keeps the copy until the image is released.
///
/// gpui removes the copies only from the windows that the application holds at the time of
/// the call. During the work of a window (a frame of the display, an event, a drawing) that
/// window is not held, and its copy would stay. The removal therefore runs after the current
/// work of the application, when every window is held.
#[allow(clippy::disallowed_methods)]
pub fn release(image: Arc<RenderImage>, cx: &mut App) {
    cx.defer(move |cx| cx.drop_image(image, None));
}
