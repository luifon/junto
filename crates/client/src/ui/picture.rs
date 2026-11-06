//! A picture of a call in the form that the window draws (ADR 0774): an image of gpui,
//! which the renderer copies into its atlas, or on macOS a pixel buffer of Core Video,
//! which the renderer draws without a copy.

use std::sync::Arc;

use gpui::{IntoElement, RenderImage, Styled};

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

    /// The width and the height of the picture in pixels.
    fn size(&self) -> gpui::Size<gpui::DevicePixels> {
        match self {
            Self::Image(image) => image.size(0),
            #[cfg(target_os = "macos")]
            Self::Surface(buffer) => {
                gpui::size(buffer.get_width().into(), buffer.get_height().into())
            }
        }
    }

    /// The width of the picture in pixels.
    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn width(&self) -> u32 {
        u32::from(self.size().width)
    }

    /// The picture over the whole of its parent, with its proportions kept.
    ///
    /// The element is a canvas without a size of its own: it takes the size of its parent
    /// in each layout, and the picture is painted in the largest part of that area that
    /// has the proportions of the picture. An `img` of gpui is not used, because it gives
    /// its layout node the proportions of the picture, and taffy then makes the node at
    /// least as high as its width divided by those proportions, also when its height is
    /// set, so that a picture in a block parent is drawn below its area (ADR 0774).
    pub(crate) fn element(self) -> gpui::AnyElement {
        gpui::canvas(
            |_, _, _| (),
            move |area, (), window, _| self.paint(area, window),
        )
        .size_full()
        .into_any_element()
    }

    fn paint(&self, area: gpui::Bounds<gpui::Pixels>, window: &mut gpui::Window) {
        let size = self.size();
        let fitted = gpui::ObjectFit::Contain.get_bounds(area, size);
        #[cfg(feature = "test-support")]
        painted(PaintedPicture {
            bounds: fitted,
            size: (u32::from(size.width), u32::from(size.height)),
        });
        match self {
            Self::Image(image) => {
                if let Err(error) = window.paint_image(
                    area,
                    fitted,
                    gpui::Corners::default(),
                    image.clone(),
                    0,
                    false,
                ) {
                    tracing::warn!("a call picture could not be painted: {error:#}");
                }
            }
            #[cfg(target_os = "macos")]
            Self::Surface(buffer) => window.paint_surface(fitted, buffer.clone()),
        }
    }
}

/// A call picture that a window of this thread painted.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaintedPicture {
    /// Where the picture was painted, in the pixels of the window.
    pub bounds: gpui::Bounds<gpui::Pixels>,
    /// The width and the height of the picture in pixels.
    pub size: (u32, u32),
}

#[cfg(feature = "test-support")]
thread_local! {
    /// The paints of call pictures of this thread while a [`PaintRecord`] lives. Without
    /// one a paint is not kept, so that a run of the window that measures its memory and
    /// its time, which is built with the support of the tests, keeps nothing (ADR 0774).
    static PAINTED: std::cell::RefCell<Option<Vec<PaintedPicture>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(feature = "test-support")]
fn painted(picture: PaintedPicture) {
    PAINTED.with_borrow_mut(|painted| {
        if let Some(painted) = painted {
            painted.push(picture);
        }
    });
}

/// Keeps the paints of call pictures of this thread until it is dropped.
#[cfg(feature = "test-support")]
#[must_use]
pub struct PaintRecord(());

#[cfg(feature = "test-support")]
impl Drop for PaintRecord {
    fn drop(&mut self) {
        PAINTED.set(None);
    }
}

/// Starts to keep the paints of call pictures of this thread.
#[cfg(feature = "test-support")]
pub fn record_painted_pictures() -> PaintRecord {
    PAINTED.set(Some(Vec::new()));
    PaintRecord(())
}

/// The call pictures that the windows of this thread painted since the last call while a
/// [`PaintRecord`] lives, in the order of their paint.
#[cfg(feature = "test-support")]
pub fn take_painted_pictures() -> Vec<PaintedPicture> {
    PAINTED.with_borrow_mut(|painted| painted.as_mut().map(std::mem::take).unwrap_or_default())
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;

    #[test]
    fn a_paint_is_kept_only_while_a_record_lives() {
        let picture = PaintedPicture {
            bounds: gpui::Bounds::default(),
            size: (64, 36),
        };
        painted(picture);
        assert!(take_painted_pictures().is_empty());
        let record = record_painted_pictures();
        painted(picture);
        painted(picture);
        assert_eq!(take_painted_pictures(), [picture, picture]);
        assert!(take_painted_pictures().is_empty());
        painted(picture);
        drop(record);
        painted(picture);
        assert!(take_painted_pictures().is_empty());
        assert!(PAINTED.with_borrow(Option::is_none));
    }
}
