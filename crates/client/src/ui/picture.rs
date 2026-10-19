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
                    PAINTED.with_borrow_mut(|painted| {
                        painted.push(PaintedSurface {
                            bounds: fitted,
                            size: (buffer.get_width(), buffer.get_height()),
                        })
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
    static PAINTED: std::cell::RefCell<Vec<PaintedSurface>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// The pixel buffers that the windows of this thread painted since the last call, in the
/// order of their paint.
#[cfg(all(target_os = "macos", feature = "test-support"))]
pub fn take_painted_surfaces() -> Vec<PaintedSurface> {
    PAINTED.with_borrow_mut(std::mem::take)
}
