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
            #[cfg(target_os = "macos")]
            Self::Surface(buffer) => gpui::surface(buffer)
                .size_full()
                .object_fit(gpui::ObjectFit::Contain)
                .into_any_element(),
        }
    }
}
