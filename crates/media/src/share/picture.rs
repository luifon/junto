//! The pictures of a share as an interface draws them: BGRA bytes at the size at which the
//! picture is drawn (ADR 0737).

use std::sync::Arc;

use livekit::webrtc::video_frame::native::VideoFrameBufferExt;
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer, VideoFormatType};

/// The largest size with the proportions of `source` that fits into `bound`. A source that
/// fits is not made larger. A side is at least one pixel.
pub fn fit(source: (u32, u32), bound: (u32, u32)) -> (u32, u32) {
    let (width, height) = (u64::from(source.0.max(1)), u64::from(source.1.max(1)));
    let (bound_width, bound_height) = (u64::from(bound.0.max(1)), u64::from(bound.1.max(1)));
    if width <= bound_width && height <= bound_height {
        return (width as u32, height as u32);
    }
    // The side that reaches its bound first decides.
    if width * bound_height >= height * bound_width {
        (
            bound_width as u32,
            (height * bound_width / width).max(1) as u32,
        )
    } else {
        (
            (width * bound_height / height).max(1) as u32,
            bound_height as u32,
        )
    }
}

/// One picture for an interface.
#[derive(Clone, PartialEq, Eq)]
pub struct PictureFrame {
    pub width: u32,
    pub height: u32,
    /// The pixels in rows from the top, four bytes each: blue, green, red, and an alpha
    /// byte of 255.
    pub bgra: Arc<[u8]>,
    /// The size of the picture as the share sends it.
    pub source: (u32, u32),
}

impl std::fmt::Debug for PictureFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PictureFrame")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

impl PictureFrame {
    /// The colour of the pixel at a place, as red, green and blue.
    pub fn pixel(&self, x: u32, y: u32) -> Option<(u8, u8, u8)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let at = ((y * self.width + x) * 4) as usize;
        let pixel = self.bgra.get(at..at + 4)?;
        Some((pixel[2], pixel[1], pixel[0]))
    }
}

/// The picture of a buffer in planes of brightness and colour, whatever form the buffer
/// has: the decoder of a system gives its own form.
fn planes(buffer: &dyn VideoBuffer) -> Option<I420Buffer> {
    if let Some(buffer) = buffer.as_i420() {
        return Some(buffer.to_i420());
    }
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(buffer) = buffer.as_native() {
        return Some(buffer.to_i420());
    }
    if let Some(buffer) = buffer.as_nv12() {
        return Some(buffer.to_i420());
    }
    if let Some(buffer) = buffer.as_i420a() {
        return Some(buffer.to_i420());
    }
    if let Some(buffer) = buffer.as_i422() {
        return Some(buffer.to_i420());
    }
    if let Some(buffer) = buffer.as_i444() {
        return Some(buffer.to_i420());
    }
    buffer.as_i010().map(VideoFrameBufferExt::to_i420)
}

/// Converts a picture of a share for an interface that draws it inside `bound` pixels.
/// Returns nothing for a buffer without a size, or of a form that this build cannot read.
pub fn convert(buffer: &dyn VideoBuffer, bound: (u32, u32)) -> Option<PictureFrame> {
    let source = (buffer.width(), buffer.height());
    if source.0 == 0 || source.1 == 0 {
        return None;
    }
    let (width, height) = fit(source, bound);
    let mut planes = planes(buffer)?;
    if (width, height) != source {
        planes = planes.scale(width as i32, height as i32);
    }
    let mut bgra = vec![0u8; (width * height * 4) as usize];
    // The call library names a format by its bytes from the highest to the lowest of a
    // 32-bit word: its "ARGB" is blue, green, red, alpha in memory.
    planes.to_argb(
        VideoFormatType::ARGB,
        &mut bgra,
        width * 4,
        width as i32,
        height as i32,
    );
    Some(PictureFrame {
        width,
        height,
        bgra: bgra.into(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture of one colour, given as brightness and the two colour values.
    fn plain(width: u32, height: u32, yuv: (u8, u8, u8)) -> I420Buffer {
        let mut buffer = I420Buffer::new(width, height);
        let (y, u, v) = buffer.data_mut();
        y.fill(yuv.0);
        u.fill(yuv.1);
        v.fill(yuv.2);
        buffer
    }

    fn near(one: (u8, u8, u8), other: (u8, u8, u8)) -> bool {
        let far = |a: u8, b: u8| a.abs_diff(b) > 12;
        !(far(one.0, other.0) || far(one.1, other.1) || far(one.2, other.2))
    }

    #[test]
    fn a_size_is_fitted_into_a_bound_without_a_change_of_its_proportions() {
        assert_eq!(fit((1920, 1080), (960, 960)), (960, 540));
        assert_eq!(fit((1080, 1920), (960, 960)), (540, 960));
        assert_eq!(fit((640, 360), (1920, 1080)), (640, 360));
        assert_eq!(fit((1920, 1080), (176, 110)), (176, 99));
        assert_eq!(fit((10_000, 1), (100, 100)), (100, 1));
        assert_eq!(fit((0, 0), (0, 0)), (1, 1));
    }

    #[test]
    fn a_picture_is_converted_to_blue_green_red_at_the_size_of_its_bound() {
        // Red in the colour space of video: low brightness, high second colour value.
        let red = plain(64, 36, (81, 90, 240));
        let frame = convert(&red, (640, 360)).unwrap();
        assert_eq!(
            (frame.width, frame.height, frame.source),
            (64, 36, (64, 36))
        );
        let (r, g, b) = frame.pixel(10, 10).unwrap();
        assert!(near((r, g, b), (255, 0, 0)), "{r} {g} {b}");
        assert_eq!(frame.bgra[3], 255);
        assert_eq!(frame.pixel(64, 0), None);

        let blue = plain(640, 360, (41, 240, 110));
        let small = convert(&blue, (160, 100)).unwrap();
        assert_eq!((small.width, small.height), (160, 90));
        assert_eq!(small.source, (640, 360));
        assert_eq!(small.bgra.len(), 160 * 90 * 4);
        let (r, g, b) = small.pixel(159, 89).unwrap();
        assert!(near((r, g, b), (0, 0, 255)), "{r} {g} {b}");
    }
}
