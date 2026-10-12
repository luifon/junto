//! Camera frames retain video planes until the interface needs to draw them.
use crate::share::picture::{self, PictureFrame};
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer};
use std::sync::OnceLock;

pub struct CameraFrame {
    pub width: u32,
    pub height: u32,
    pub mirrored: bool,
    planes: I420Buffer,
    pixels: OnceLock<Option<PictureFrame>>,
    #[cfg(target_os = "macos")]
    surface: OnceLock<Option<Surface>>,
}
impl std::fmt::Debug for CameraFrame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CameraFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish()
    }
}
impl CameraFrame {
    pub fn new(buffer: &dyn VideoBuffer, mirrored: bool, bound: (u32, u32)) -> Option<Self> {
        if buffer.width() == 0 || buffer.height() == 0 {
            return None;
        }
        let mut planes = picture::planes(buffer)?;
        let (width, height) = picture::fit((buffer.width(), buffer.height()), bound);
        if (width, height) != (buffer.width(), buffer.height()) {
            planes = planes.scale(width as i32, height as i32);
        }
        Some(Self {
            width,
            height,
            mirrored,
            planes,
            pixels: OnceLock::new(),
            #[cfg(target_os = "macos")]
            surface: OnceLock::new(),
        })
    }
    pub fn picture(&self) -> Option<&PictureFrame> {
        self.pixels
            .get_or_init(|| {
                let picture = picture::convert(&self.planes, (self.width, self.height))?;
                Some(if self.mirrored {
                    super::mirror(&picture)
                } else {
                    picture
                })
            })
            .as_ref()
    }
    pub fn pixel(&self, x: u32, y: u32) -> Option<(u8, u8, u8)> {
        self.picture()?.pixel(x, y)
    }
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
