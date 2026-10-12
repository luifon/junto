//! Validated packed pixels, copied before the PipeWire buffer returns to its owner.

use livekit::webrtc::native::yuv_helper::argb_to_i420;
use livekit::webrtc::video_frame::I420Buffer;

use crate::share::{CaptureError, Resolution};

#[derive(Clone, Copy)]
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

pub(super) struct Frame {
    pub width: u32,
    pub height: u32,
    bgra: Vec<u8>,
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
        let mut bgra = Vec::with_capacity(crop.width as usize * crop.height as usize * 4);
        for row in y..y + crop.height {
            let start = row as usize * stride + x as usize * 4;
            for pixel in bytes[start..start + crop.width as usize * 4]
                .as_chunks::<4>()
                .0
            {
                let (blue, green, red, alpha) = match format {
                    Format::Bgra => (pixel[0], pixel[1], pixel[2], pixel[3]),
                    Format::Bgrx => (pixel[0], pixel[1], pixel[2], 255),
                    Format::Rgba => (pixel[2], pixel[1], pixel[0], pixel[3]),
                    Format::Rgbx => (pixel[2], pixel[1], pixel[0], 255),
                };
                // Do not transmit hidden RGB in a fully transparent window pixel.
                if alpha == 0 {
                    bgra.extend_from_slice(&[0, 0, 0, 255]);
                } else {
                    bgra.extend_from_slice(&[blue, green, red, 255]);
                }
            }
        }
        Ok(Self {
            width: crop.width,
            height: crop.height,
            bgra,
        })
    }

    pub fn convert(self, resolution: Resolution) -> I420Buffer {
        let mut buffer = I420Buffer::new(self.width, self.height);
        let (stride_y, stride_u, stride_v) = buffer.strides();
        let (y, u, v) = buffer.data_mut();
        argb_to_i420(
            &self.bgra,
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
        assert_eq!(frame.bgra, [30, 20, 10, 255, 60, 50, 40, 255]);
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
        let frame = Frame::copy(&[99, 98, 97, 0], (1, 1), 4, None, Format::Bgra).unwrap();
        assert_eq!(frame.bgra, [0, 0, 0, 255]);
        let frame = Frame::copy(&[99, 98, 97, 0], (1, 1), 4, None, Format::Rgbx).unwrap();
        assert_eq!(frame.bgra, [97, 98, 99, 255]);
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
