//! Validated packed pixels, copied before the buffer of the capture returns to its owner.

use livekit::webrtc::native::yuv_helper::{abgr_to_i420, argb_to_i420};
use livekit::webrtc::video_frame::I420Buffer;

use crate::share::{CaptureError, Resolution};

/// The byte order of a pixel. The portal of Linux gives either order; Windows gives blue first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) enum Format {
    Bgra,
    Bgrx,
    Rgba,
    Rgbx,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Crop {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The cropped rows of a captured picture without their padding, in the byte order of the
/// capture.
pub(crate) struct Frame {
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

    /// The picture of a shared application (ADR 0738): a black picture of `size` pixels with
    /// each layer drawn at its place, in the order of the list, so that a later layer covers
    /// an earlier one. A layer with transparency is drawn over what lies under it by its
    /// alpha, with premultiplied colours; a fully transparent pixel leaves what lies under
    /// it. The parts of a layer outside the picture are left out.
    #[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
    pub fn compose(size: (u32, u32), layers: &[(&Self, (i32, i32))]) -> Result<Self, CaptureError> {
        let (width, height) = size;
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 7680 * 4320 {
            return Err(CaptureError::Failed(
                "the picture of the application has an invalid size".into(),
            ));
        }
        let mut packed = [0, 0, 0, 255].repeat(width as usize * height as usize);
        for (layer, (left, top)) in layers {
            // The rows and columns of the layer that lie inside the picture.
            let columns = (
                i64::from(*left).max(0),
                (i64::from(*left) + i64::from(layer.width)).min(i64::from(width)),
            );
            let rows = (
                i64::from(*top).max(0),
                (i64::from(*top) + i64::from(layer.height)).min(i64::from(height)),
            );
            if columns.0 >= columns.1 {
                continue;
            }
            let count = (columns.1 - columns.0) as usize * 4;
            for y in rows.0..rows.1 {
                let from = ((y - i64::from(*top)) as usize * layer.width as usize
                    + (columns.0 - i64::from(*left)) as usize)
                    * 4;
                let to = (y as usize * width as usize + columns.0 as usize) * 4;
                draw_row(
                    &mut packed[to..to + count],
                    &layer.packed[from..from + count],
                    layer.format,
                );
            }
        }
        Ok(Self {
            width,
            height,
            format: Format::Bgrx,
            packed,
        })
    }

    /// The picture in the form of a preview of the dialog of a share, in the byte order
    /// blue, green, red with an alpha byte of 255. `source` is the size of what it shows.
    #[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
    pub fn into_picture(mut self, source: (u32, u32)) -> crate::share::picture::PictureFrame {
        let swapped = matches!(self.format, Format::Rgba | Format::Rgbx);
        for pixel in self.packed.as_chunks_mut::<4>().0 {
            if swapped {
                pixel.swap(0, 2);
            }
            pixel[3] = 255;
        }
        crate::share::picture::PictureFrame {
            width: self.width,
            height: self.height,
            bgra: self.packed,
            source,
        }
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

/// Draws one row of a layer of `format` over one row of the picture of an application, in
/// the order blue, green, red with an alpha byte of 255. A run of opaque pixels is copied
/// whole; only a pixel with partial transparency is blended. The colours of a layer are
/// premultiplied by its alpha, as Windows Graphics Capture gives them: the picture under a
/// pixel keeps the part that the pixel leaves, and a fully transparent pixel leaves it all.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
fn draw_row(target: &mut [u8], source: &[u8], format: Format) {
    let target = target.as_chunks_mut::<4>().0;
    let source = source.as_chunks::<4>().0;
    let swapped = matches!(format, Format::Rgba | Format::Rgbx);
    let transparent = matches!(format, Format::Bgra | Format::Rgba);
    let mut x = 0;
    while x < source.len() {
        let start = x;
        while x < source.len() && (!transparent || source[x][3] == 255) {
            x += 1;
        }
        let (opaque, from) = (&mut target[start..x], &source[start..x]);
        if swapped {
            for (pixel, over) in opaque.iter_mut().zip(from) {
                *pixel = [over[2], over[1], over[0], 255];
            }
        } else {
            opaque.copy_from_slice(from);
            if !transparent {
                for pixel in opaque {
                    pixel[3] = 255;
                }
            }
        }
        while x < source.len() && source[x][3] != 255 {
            let over = source[x];
            let alpha = over[3];
            if alpha != 0 {
                let (blue, red) = if swapped {
                    (over[2], over[0])
                } else {
                    (over[0], over[2])
                };
                let under = &mut target[x];
                let blend = |over: u8, under: u8| {
                    let kept = u16::from(under) * u16::from(255 - alpha) / 255;
                    (u16::from(over) + kept).min(255) as u8
                };
                *under = [
                    blend(blue, under[0]),
                    blend(over[1], under[1]),
                    blend(red, under[2]),
                    255,
                ];
            }
            x += 1;
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
    fn the_picture_of_an_application_draws_its_windows_at_their_places_from_the_back() {
        let solid = |width: u32, height: u32, pixel: [u8; 4], format| {
            let pixels = pixel.repeat((width * height) as usize);
            Frame::copy(&pixels, (width, height), (width * 4) as i32, None, format).unwrap()
        };
        let back = solid(3, 2, [10, 20, 30, 255], Format::Bgra);
        // A window in the other byte order, with one transparent corner, over the first.
        let mut front = solid(2, 2, [90, 80, 70, 255], Format::Rgba);
        front.packed[..4].copy_from_slice(&[1, 2, 3, 0]);
        // A window that lies across the left edge and below the picture.
        let edge = solid(2, 3, [5, 5, 5, 255], Format::Bgrx);
        let picture = Frame::compose(
            (4, 3),
            &[(&back, (0, 0)), (&front, (2, 1)), (&edge, (-1, 2))],
        )
        .unwrap();
        assert_eq!(
            (picture.width, picture.height, picture.format),
            (4, 3, Format::Bgrx)
        );
        let at = |x: usize, y: usize| -> [u8; 4] {
            let index = (y * 4 + x) * 4;
            picture.packed[index..index + 4].try_into().unwrap()
        };
        assert_eq!(at(0, 0), [10, 20, 30, 255]);
        // The ground is black where no window is.
        assert_eq!(at(3, 0), [0, 0, 0, 255]);
        // The transparent corner shows the window behind it.
        assert_eq!(at(2, 1), [10, 20, 30, 255]);
        // The front window is drawn in the order blue, green, red.
        assert_eq!(at(3, 1), [70, 80, 90, 255]);
        assert_eq!(at(3, 2), [70, 80, 90, 255]);
        assert_eq!(at(0, 2), [5, 5, 5, 255]);
        assert_eq!(at(1, 2), [0, 0, 0, 255]);
        let preview = Frame::compose((1, 1), &[(&front, (-1, 0))])
            .unwrap()
            .into_picture((10, 10));
        assert_eq!(
            (preview.bgra.as_slice(), preview.source),
            (&[70, 80, 90, 255][..], (10, 10))
        );
        assert!(Frame::compose((0, 3), &[]).is_err());
        // A layer wholly outside the picture changes nothing.
        let empty = Frame::compose((2, 2), &[(&back, (5, 5)), (&back, (-9, 0))]).unwrap();
        assert_eq!(empty.packed, [0, 0, 0, 255].repeat(4));
        // A shadow with an alpha of 51 (premultiplied black) over the back window keeps
        // four fifths of it, and over the black ground stays black.
        let shadow = solid(2, 1, [0, 0, 0, 51], Format::Bgra);
        let shaded = Frame::compose((4, 1), &[(&back, (0, 0)), (&shadow, (2, 0))]).unwrap();
        assert_eq!(&shaded.packed[8..12], &[8, 16, 24, 255]);
        assert_eq!(&shaded.packed[12..16], &[0, 0, 0, 255]);
        // A glow of a premultiplied light colour adds to what lies under it.
        let glow = solid(1, 1, [40, 40, 40, 102], Format::Bgra);
        let glowing = Frame::compose((1, 1), &[(&back, (0, 0)), (&glow, (0, 0))]).unwrap();
        assert_eq!(&glowing.packed[..], &[46, 52, 58, 255]);
    }

    /// The rule of one pixel of the picture of an application, as the composition states it.
    fn drawn_pixel(under: [u8; 4], over: [u8; 4], format: Format) -> [u8; 4] {
        let over = match format {
            Format::Bgra => over,
            Format::Bgrx => [over[0], over[1], over[2], 255],
            Format::Rgba => [over[2], over[1], over[0], over[3]],
            Format::Rgbx => [over[2], over[1], over[0], 255],
        };
        if over[3] == 0 {
            return under;
        }
        let alpha = u16::from(over[3]);
        let blend = |over: u8, under: u8| {
            (u16::from(over) + u16::from(under) * (255 - alpha) / 255).min(255) as u8
        };
        [
            blend(over[0], under[0]),
            blend(over[1], under[1]),
            blend(over[2], under[2]),
            255,
        ]
    }

    #[test]
    fn a_row_is_drawn_as_each_pixel_by_its_own_rule() {
        // Runs of opaque, transparent and partly transparent pixels of every alpha value, in
        // a fixed order of numbers that are not related to each other.
        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut source = Vec::new();
        for alpha in 0..=255_u8 {
            for _ in 0..(next() % 5 + 1) {
                let value = next().to_le_bytes();
                let run = next() % 3;
                let alpha = match run {
                    0 => 255,
                    1 => 0,
                    _ => alpha,
                };
                // Premultiplied colours are at most the alpha; the rule also holds above it.
                source.extend_from_slice(&[value[0], value[1], value[2], alpha]);
            }
        }
        // The picture under a layer is opaque: the black ground or layers drawn before.
        let under: Vec<u8> = (0..source.len() / 4)
            .flat_map(|_| {
                let value = next().to_le_bytes();
                [value[0], value[1], value[2], 255]
            })
            .collect();
        for format in [Format::Bgra, Format::Bgrx, Format::Rgba, Format::Rgbx] {
            let mut drawn = under.clone();
            draw_row(&mut drawn, &source, format);
            let expected: Vec<u8> = under
                .as_chunks::<4>()
                .0
                .iter()
                .zip(source.as_chunks::<4>().0)
                .flat_map(|(under, over)| drawn_pixel(*under, *over, format))
                .collect();
            assert_eq!(drawn, expected, "{format:?}");
        }
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
