//! A picture of a call in the form that the window draws (ADR 0774): an image of gpui,
//! which the renderer copies into its atlas, or on macOS a pixel buffer of Core Video,
//! which the renderer draws without a copy.

use std::sync::Arc;

use gpui::{IntoElement, RenderImage, Styled, StyledImage};

#[derive(Clone)]
pub enum DrawnPicture {
    Image(Arc<RenderImage>),
    #[cfg(target_os = "macos")]
    Surface(core_video::pixel_buffer::CVPixelBuffer),
}

impl DrawnPicture {
    /// The image of gpui, for a picture that is drawn as one.
    pub(crate) fn image(&self) -> Option<&Arc<RenderImage>> {
        match self {
            Self::Image(image) => Some(image),
            #[cfg(target_os = "macos")]
            Self::Surface(_) => None,
        }
    }

    /// The width of the picture in pixels.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn width(&self) -> u32 {
        match self {
            Self::Image(image) => u32::try_from(image.size(0).width.0).unwrap_or_default(),
            #[cfg(target_os = "macos")]
            Self::Surface(buffer) => u32::try_from(buffer.get_width()).unwrap_or_default(),
        }
    }

    /// The picture over the whole of its parent, with its proportions kept.
    pub(crate) fn element(self) -> gpui::AnyElement {
        match self {
            Self::Image(image) => gpui::img(image)
                .size_full()
                .object_fit(gpui::ObjectFit::Contain)
                .into_any_element(),
            // The surface is painted here rather than by the surface element of gpui, so
            // that the tests see each paint of a pixel buffer (`take_painted_surfaces`).
            #[cfg(target_os = "macos")]
            Self::Surface(buffer) => gpui::canvas(
                |_, _, _| (),
                move |bounds, (), window, _| {
                    let size = gpui::size(buffer.get_width().into(), buffer.get_height().into());
                    let fitted = gpui::ObjectFit::Contain.get_bounds(bounds, size);
                    #[cfg(feature = "test-support")]
                    painted(PaintedSurface {
                        bounds: fitted,
                        size: (buffer.get_width(), buffer.get_height()),
                    });
                    window.paint_surface(fitted, buffer.clone());
                },
            )
            .size_full()
            .into_any_element(),
        }
    }
}

/// A pixel buffer that a window of this thread painted.
#[cfg(all(target_os = "macos", feature = "test-support"))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaintedSurface {
    /// Where the picture was painted, in the pixels of the window.
    pub bounds: gpui::Bounds<gpui::Pixels>,
    /// The width and the height of the pixel buffer.
    pub size: (usize, usize),
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
thread_local! {
    /// The paints of pixel buffers of this thread while a [`PaintRecord`] lives. Without
    /// one a paint is not kept, so that a run of the window that measures its memory and
    /// its time, which is built with the support of the tests, keeps nothing (ADR 0774).
    static PAINTED: std::cell::RefCell<Option<Vec<PaintedSurface>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(all(target_os = "macos", feature = "test-support"))]
fn painted(surface: PaintedSurface) {
    PAINTED.with_borrow_mut(|painted| {
        if let Some(painted) = painted {
            painted.push(surface);
        }
    });
}

/// Keeps the paints of pixel buffers of this thread until it is dropped.
#[cfg(all(target_os = "macos", feature = "test-support"))]
#[must_use]
pub struct PaintRecord(());

#[cfg(all(target_os = "macos", feature = "test-support"))]
impl Drop for PaintRecord {
    fn drop(&mut self) {
        PAINTED.set(None);
    }
}

/// Starts to keep the paints of pixel buffers of this thread.
#[cfg(all(target_os = "macos", feature = "test-support"))]
pub fn record_painted_surfaces() -> PaintRecord {
    PAINTED.set(Some(Vec::new()));
    PaintRecord(())
}

/// The pixel buffers that the windows of this thread painted since the last call while a
/// [`PaintRecord`] lives, in the order of their paint.
#[cfg(all(target_os = "macos", feature = "test-support"))]
pub fn take_painted_surfaces() -> Vec<PaintedSurface> {
    PAINTED.with_borrow_mut(|painted| painted.as_mut().map(std::mem::take).unwrap_or_default())
}

#[cfg(all(test, target_os = "macos", feature = "test-support"))]
mod tests {
    use super::*;

    #[test]
    fn a_paint_is_kept_only_while_a_record_lives() {
        let surface = PaintedSurface {
            bounds: gpui::Bounds::default(),
            size: (64, 36),
        };
        painted(surface);
        assert!(take_painted_surfaces().is_empty());
        let record = record_painted_surfaces();
        painted(surface);
        painted(surface);
        assert_eq!(take_painted_surfaces(), [surface, surface]);
        assert!(take_painted_surfaces().is_empty());
        painted(surface);
        drop(record);
        painted(surface);
        assert!(take_painted_surfaces().is_empty());
        assert!(PAINTED.with_borrow(Option::is_none));
    }
}
