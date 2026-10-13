//! Validated packed pixels, copied before the PipeWire buffer returns to its owner.

use livekit::webrtc::native::yuv_helper::{abgr_to_i420, argb_to_i420};
use livekit::webrtc::video_frame::I420Buffer;

use crate::share::{CaptureError, Resolution};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Format {
    Bgra,
    Bgrx,
    Rgba,
    Rgbx,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Crop {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The cropped rows of a captured picture without their padding, in the byte order of the
/// capture.
pub(super) struct Frame {
    pub width: u32,
    pub height: u32,
    format: Format,
    packed: Vec<u8>,
}

impl Frame {
    pub fn copy(
        bytes: &[u8],
        dimensions: (u32, u32),
        stride: i32,
        crop: Option<Crop>,
        format: Format,
    ) -> Result<Self, CaptureError> {
        let invalid = || {
            CaptureError::Failed(
                "the captured pixel buffer has invalid dimensions or boundaries".into(),
            )
        };
        let (width, height) = dimensions;
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 7680 * 4320 {
            return Err(invalid());
        }
        let stride = usize::try_from(stride).map_err(|_| invalid())?;
        let row = (width as usize).checked_mul(4).ok_or_else(invalid)?;
        if stride < row {
            return Err(invalid());
        }
        let end = (height as usize - 1)
            .checked_mul(stride)
            .and_then(|last| last.checked_add(row))
            .ok_or_else(invalid)?;
        if end > bytes.len() {
            return Err(invalid());
        }
        let crop = crop.unwrap_or(Crop {
            x: 0,
            y: 0,
            width,
            height,
        });
        let x = u32::try_from(crop.x).map_err(|_| invalid())?;
        let y = u32::try_from(crop.y).map_err(|_| invalid())?;
        if crop.width == 0
            || crop.height == 0
            || x.checked_add(crop.width).is_none_or(|end| end > width)
            || y.checked_add(crop.height).is_none_or(|end| end > height)
        {
            return Err(invalid());
        }
        // This runs on the thread of the capture: rows are copied whole, and every step
        // for a pixel is left to `convert`.
        let mut packed = Vec::with_capacity(crop.width as usize * crop.height as usize * 4);
        for row in y..y + crop.height {
            let start = row as usize * stride + x as usize * 4;
            packed.extend_from_slice(&bytes[start..start + crop.width as usize * 4]);
        }
        Ok(Self {
            width: crop.width,
            height: crop.height,
            format,
            packed,
        })
    }

    /// Do not transmit hidden RGB in a fully transparent window pixel.
    fn hide_transparent(&mut self) {
        if matches!(self.format, Format::Bgrx | Format::Rgbx) {
            return;
        }
        for pixel in self.packed.as_chunks_mut::<4>().0 {
            if pixel[3] == 0 {
                *pixel = [0, 0, 0, 255];
            }
        }
    }

    pub fn convert(mut self, resolution: Resolution) -> I420Buffer {
        self.hide_transparent();
        let mut buffer = I420Buffer::new(self.width, self.height);
        let (stride_y, stride_u, stride_v) = buffer.strides();
        let (y, u, v) = buffer.data_mut();
        // The call library names a format by its bytes from the highest to the lowest.
        let to_i420 = match self.format {
            Format::Bgra | Format::Bgrx => argb_to_i420,
            Format::Rgba | Format::Rgbx => abgr_to_i420,
        };
        to_i420(
            &self.packed,
            self.width * 4,
            y,
            stride_y,
            u,
            stride_u,
            v,
            stride_v,
            self.width as i32,
            self.height as i32,
        );
        let size = resolution.fit(self.width, self.height);
        if size != (self.width, self.height) {
            buffer.scale(size.0 as i32, size.1 as i32)
        } else {
            buffer
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn padding_and_pixels_outside_the_crop_are_not_shared() {
        let bytes = [
            1, 2, 3, 255, 10, 20, 30, 255, 99, 99, 99, 99, 4, 5, 6, 255, 40, 50, 60, 255,
        ];
        let crop = Crop {
            x: 1,
            y: 0,
            width: 1,
            height: 2,
        };
        let frame = Frame::copy(&bytes, (2, 2), 12, Some(crop), Format::Rgba).unwrap();
        assert_eq!((frame.width, frame.height), (1, 2));
        assert_eq!(frame.format, Format::Rgba);
        assert_eq!(frame.packed, [10, 20, 30, 255, 40, 50, 60, 255]);
    }

    #[test]
    fn malformed_boundaries_fail_before_any_copy() {
        for (size, stride, crop) in [
            ((0, 2), 8, None),
            ((2, 2), -8, None),
            ((2, 2), 7, None),
            ((2, 2), 12, None),
            ((u32::MAX, u32::MAX), i32::MAX, None),
            (
                (2, 2),
                8,
                Some(Crop {
                    x: -1,
                    y: 0,
                    width: 1,
                    height: 1,
                }),
            ),
            (
                (2, 2),
                8,
                Some(Crop {
                    x: 1,
                    y: 1,
                    width: 2,
                    height: 2,
                }),
            ),
        ] {
            assert!(Frame::copy(&[0; 16], size, stride, crop, Format::Bgrx).is_err());
        }
    }

    #[test]
    fn a_transparent_pixel_cannot_disclose_its_hidden_colour() {
        for format in [Format::Bgra, Format::Rgba] {
            let mut frame = Frame::copy(&[99, 98, 97, 0], (1, 1), 4, None, format).unwrap();
            frame.hide_transparent();
            assert_eq!(frame.packed, [0, 0, 0, 255]);
        }
        // The fourth byte of a format without transparency is padding.
        let mut frame = Frame::copy(&[99, 98, 97, 0], (1, 1), 4, None, Format::Rgbx).unwrap();
        frame.hide_transparent();
        assert_eq!(frame.packed, [99, 98, 97, 0]);
        // The conversion applies the step: a transparent red pixel arrives black.
        let pixels = [0, 0, 200, 0].repeat(16 * 16);
        let frame = Frame::copy(&pixels, (16, 16), 64, None, Format::Bgra).unwrap();
        let buffer = frame.convert(Resolution::P720);
        let picture = crate::share::picture::convert(&buffer, (16, 16)).unwrap();
        let (red, green, blue) = picture.pixel(8, 8).unwrap();
        assert!(red < 4 && green < 4 && blue < 4, "{red} {green} {blue}");
    }

    #[test]
    fn both_byte_orders_keep_their_colours() {
        for (format, pixel) in [
            (Format::Bgrx, [20, 60, 180, 0]),
            (Format::Bgra, [20, 60, 180, 255]),
            (Format::Rgbx, [180, 60, 20, 0]),
            (Format::Rgba, [180, 60, 20, 255]),
        ] {
            let pixels = pixel.repeat(64 * 64);
            let frame = Frame::copy(&pixels, (64, 64), 64 * 4, None, format).unwrap();
            let buffer = frame.convert(Resolution::P720);
            let picture = crate::share::picture::convert(&buffer, (64, 64)).unwrap();
            let (red, green, blue) = picture.pixel(32, 32).unwrap();
            assert!((i16::from(red) - 180).abs() < 4, "{format:?}: red {red}");
            assert!(
                (i16::from(green) - 60).abs() < 4,
                "{format:?}: green {green}"
            );
            assert!((i16::from(blue) - 20).abs() < 4, "{format:?}: blue {blue}");
        }
    }

    #[test]
    fn the_copy_on_the_capture_thread_costs_about_one_plain_copy_of_the_picture() {
        // 3840 by 2160 with padded rows, as a compositor can give them.
        let (width, height, stride) = (3840_usize, 2160_usize, 3840 * 4 + 64);
        let source = vec![7_u8; stride * height];
        let measure = |work: &dyn Fn() -> usize| {
            (0..5)
                .map(|_| {
                    let started = std::time::Instant::now();
                    assert_eq!(work(), width * height * 4);
                    started.elapsed()
                })
                .min()
                .unwrap()
        };
        let plain = measure(&|| source[..width * height * 4].to_vec().len());
        let copied = measure(&|| {
            Frame::copy(
                &source,
                (width as u32, height as u32),
                stride as i32,
                None,
                Format::Bgra,
            )
            .unwrap()
            .packed
            .len()
        });
        eprintln!("3840 by 2160: plain copy {plain:?}, copy of the capture {copied:?}");
        assert!(
            copied < plain * 4 + std::time::Duration::from_millis(5),
            "plain copy {plain:?}, copy of the capture {copied:?}"
        );
    }

    #[test]
    fn converted_pixels_keep_the_selected_resolution_and_colour() {
        let pixels = [20, 60, 180, 255].repeat(128 * 72);
        let frame = Frame::copy(&pixels, (128, 72), 128 * 4, None, Format::Bgrx).unwrap();
        let buffer = frame.convert(Resolution::P720);
        let picture = crate::share::picture::convert(&buffer, (128, 72)).unwrap();
        let (red, green, blue) = picture.pixel(40, 40).unwrap();
        assert!((i16::from(red) - 180).abs() < 4);
        assert!((i16::from(green) - 60).abs() < 4);
        assert!((i16::from(blue) - 20).abs() < 4);
        assert_eq!((picture.width, picture.height), (128, 72));
    }
}
