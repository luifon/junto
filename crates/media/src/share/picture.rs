//! The pictures of a call as an interface draws them, at the size at which they are drawn
//! (ADR 0737): BGRA bytes, or planes of brightness and colour from which a window makes
//! the form that it draws (ADR 0774).

use livekit::webrtc::video_frame::native::VideoFrameBufferExt;
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer, VideoFormatType};

#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod surface;
mod video;

pub use video::VideoPicture;

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
    pub bgra: Vec<u8>,
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
pub(crate) fn planes(buffer: &dyn VideoBuffer) -> Option<I420Buffer> {
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
    let bgra = match buffer.as_i420() {
        // Planes at the size of the drawing are read where they are.
        Some(planes) if (width, height) == source => argb(planes, width, height),
        _ => {
            let mut planes = planes(buffer)?;
            if (width, height) != source {
                planes = planes.scale(width as i32, height as i32);
            }
            argb(&planes, width, height)
        }
    };
    Some(PictureFrame {
        width,
        height,
        bgra,
        source,
    })
}

/// The BGRA pixels of planes of `width` × `height`.
fn argb(planes: &I420Buffer, width: u32, height: u32) -> Vec<u8> {
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
    bgra
}

/// The picture with its columns in the opposite order: the preview of the own camera.
/// Transmitted pictures keep the orientation of the camera.
pub fn mirror(mut frame: PictureFrame) -> PictureFrame {
    let width = frame.width as usize;
    if width > 0 {
        for row in frame.bgra.chunks_exact_mut(width * 4) {
            for x in 0..width / 2 {
                let other = width - 1 - x;
                for channel in 0..4 {
                    row.swap(x * 4 + channel, other * 4 + channel);
                }
            }
        }
    }
    frame
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

    #[test]
    fn a_received_frame_in_planes_at_the_size_of_the_drawing_is_held_without_a_copy() {
        use livekit::webrtc::video_frame::{BoxVideoFrame, VideoFrame, VideoRotation};
        use std::sync::Arc;
        let frame = |width, height| -> Arc<BoxVideoFrame> {
            let buffer: Box<dyn VideoBuffer> = Box::new(plain(width, height, (81, 90, 240)));
            Arc::new(VideoFrame::new(VideoRotation::VideoRotation0, buffer))
        };
        let fitting = frame(64, 36);
        let received = fitting
            .buffer
            .as_i420()
            .map(|planes| planes.data().0.as_ptr());
        let picture = VideoPicture::of_frame(&fitting, false, (64, 36)).unwrap();
        assert_eq!(picture.brightness(), received);
        assert_eq!(
            (picture.width, picture.height, picture.source),
            (64, 36, (64, 36))
        );
        let larger = frame(128, 72);
        let received = larger
            .buffer
            .as_i420()
            .map(|planes| planes.data().0.as_ptr());
        let picture = VideoPicture::of_frame(&larger, false, (64, 36)).unwrap();
        assert_ne!(picture.brightness(), received);
        assert_eq!(
            (picture.width, picture.height, picture.source),
            (64, 36, (128, 72))
        );
    }

    /// The pixel buffer that the window draws on macOS gives the colours of the BGRA
    /// picture, when its values are read with the matrix of the surface shader of gpui
    /// (full-range BT.601), also at an odd size, where the last column and row of colour
    /// cover one pixel. Every pixel is compared. A mirror of an odd width puts two pixels of
    /// two colour columns of the source into one colour column of the buffer: that column
    /// holds the mean of the two, and its pixels are compared where the two are equal.
    #[cfg(target_os = "macos")]
    #[test]
    #[allow(unsafe_code)]
    fn the_pixel_buffer_has_the_colours_of_the_bgra_picture_at_an_odd_size() {
        fn shader(y: u8, cb: u8, cr: u8) -> (u8, u8, u8) {
            let (y, cb, cr) = (
                f32::from(y) / 255.0,
                f32::from(cb) / 255.0,
                f32::from(cr) / 255.0,
            );
            let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            (
                channel(y + 1.402 * cr - 0.701),
                channel(y - 0.3441 * cb - 0.7141 * cr + 0.5291),
                channel(y + 1.772 * cb - 0.886),
            )
        }
        let (width, height) = (63_usize, 35_usize);
        let pixels = crate::share::synthetic::two_colours(
            width as u32,
            height as u32,
            (200, 60, 30),
            (40, 90, 210),
        );
        let (y_plane, u_plane, v_plane) = pixels.data();
        let (ys, us, vs) = pixels.strides();
        let (ys, us, vs) = (ys as usize, us as usize, vs as usize);
        for mirrored in [false, true] {
            let picture = VideoPicture::new(&pixels, mirrored, (63, 35)).unwrap();
            let buffer = picture.surface().unwrap();
            assert_eq!((buffer.get_width(), buffer.get_height()), (width, height));
            assert_eq!(buffer.lock_base_address(0), 0);
            let (luma, chroma, luma_stride, chroma_stride) = unsafe {
                (
                    buffer.get_base_address_of_plane(0).cast::<u8>(),
                    buffer.get_base_address_of_plane(1).cast::<u8>(),
                    buffer.get_bytes_per_row_of_plane(0),
                    buffer.get_bytes_per_row_of_plane(1),
                )
            };
            for y in 0..height {
                for x in 0..width {
                    let source = if mirrored { width - 1 - x } else { x };
                    let held = unsafe { *luma.add(y * luma_stride + x) };
                    assert_eq!(
                        held,
                        surface::luma(y_plane[y * ys + source]),
                        "mirrored {mirrored}, brightness of ({x}, {y})"
                    );
                    let at = y / 2 * chroma_stride + x / 2 * 2;
                    let (cb, cr) = unsafe { (*chroma.add(at), *chroma.add(at + 1)) };
                    // The source pixels of the two pixels of this colour column.
                    let pair: Vec<usize> = [x / 2 * 2, x / 2 * 2 + 1]
                        .into_iter()
                        .filter(|column| *column < width)
                        .map(|column| if mirrored { width - 1 - column } else { column })
                        .collect();
                    let colour = |plane: &[u8], stride: usize, column: usize| {
                        plane[y / 2 * stride + column / 2]
                    };
                    let mean = |plane: &[u8], stride: usize| {
                        let sum: u16 = pair
                            .iter()
                            .map(|column| u16::from(colour(plane, stride, *column)))
                            .sum();
                        sum.div_ceil(pair.len() as u16) as u8
                    };
                    assert_eq!(
                        (cb, cr),
                        (
                            surface::chroma(mean(u_plane, us)),
                            surface::chroma(mean(v_plane, vs))
                        ),
                        "mirrored {mirrored}, colour of ({x}, {y})"
                    );
                    let equal = pair.iter().all(|column| {
                        colour(u_plane, us, *column) == colour(u_plane, us, pair[0])
                            && colour(v_plane, vs, *column) == colour(v_plane, vs, pair[0])
                    });
                    if !equal {
                        continue;
                    }
                    let shown = shader(held, cb, cr);
                    let bgra = picture.pixel(x as u32, y as u32).unwrap();
                    let far = |a: u8, b: u8| a.abs_diff(b) > 4;
                    assert!(
                        !(far(shown.0, bgra.0) || far(shown.1, bgra.1) || far(shown.2, bgra.2)),
                        "mirrored {mirrored}, ({x}, {y}): pixel buffer {shown:?}, BGRA {bgra:?}"
                    );
                }
            }
            assert_eq!(buffer.unlock_base_address(0), 0);
        }
    }
}
