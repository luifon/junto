//! A picture of a camera or of the watched share: planes of brightness and colour at the
//! size at which it is drawn. The form that a window draws is made from the planes when the
//! window asks for it (ADR 0774).
use super::{PictureFrame, convert, fit, mirror, planes};
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer};
use std::sync::OnceLock;

pub struct VideoPicture {
    pub width: u32,
    pub height: u32,
    /// The picture is drawn with its columns in the opposite order: the preview of the own
    /// camera.
    pub mirrored: bool,
    /// The size of the picture as its track sends it.
    pub source: (u32, u32),
    planes: I420Buffer,
    pixels: OnceLock<Option<PictureFrame>>,
    #[cfg(target_os = "macos")]
    surface: OnceLock<Option<Surface>>,
}
impl std::fmt::Debug for VideoPicture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VideoPicture")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("mirrored", &self.mirrored)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}
impl VideoPicture {
    /// The picture of a buffer inside `bound` pixels. Absent for a buffer without a size,
    /// or of a form that this build cannot read.
    pub fn new(buffer: &dyn VideoBuffer, mirrored: bool, bound: (u32, u32)) -> Option<Self> {
        let source = (buffer.width(), buffer.height());
        if source.0 == 0 || source.1 == 0 {
            return None;
        }
        let mut planes = planes(buffer)?;
        let (width, height) = fit(source, bound);
        if (width, height) != source {
            planes = planes.scale(width as i32, height as i32);
        }
        Some(Self {
            width,
            height,
            mirrored,
            source,
            planes,
            pixels: OnceLock::new(),
            #[cfg(target_os = "macos")]
            surface: OnceLock::new(),
        })
    }

    /// The picture as BGRA pixels, made at the first call and kept, to read its pixels.
    pub fn picture(&self) -> Option<&PictureFrame> {
        self.pixels.get_or_init(|| self.bgra()).as_ref()
    }

    /// The picture as new BGRA pixels that the caller owns: a window moves them into its
    /// image without a copy.
    pub fn bgra(&self) -> Option<PictureFrame> {
        let mut picture = convert(&self.planes, (self.width, self.height))?;
        picture.source = self.source;
        Some(if self.mirrored {
            mirror(picture)
        } else {
            picture
        })
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<(u8, u8, u8)> {
        self.picture()?.pixel(x, y)
    }

    /// The picture as a pixel buffer of Core Video, made at the first call and kept. The
    /// renderer of the window draws it without a copy into its atlas.
    #[cfg(target_os = "macos")]
    pub fn surface(&self) -> Option<core_video::pixel_buffer::CVPixelBuffer> {
        self.surface
            .get_or_init(|| super::surface::convert(&self.planes, self.mirrored).map(Surface))
            .as_ref()
            .map(|surface| surface.0.clone())
    }
}

#[cfg(target_os = "macos")]
struct Surface(core_video::pixel_buffer::CVPixelBuffer);
// The buffer owns its planes and is never written after conversion. Core Video retains
// and releases it atomically; readers only hand it to the renderer.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
unsafe impl Send for Surface {}
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
unsafe impl Sync for Surface {}
