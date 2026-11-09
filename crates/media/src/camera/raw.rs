//! Bounds and colour conversion for camera planes owned by the operating system.

use livekit::webrtc::video_frame::I420Buffer;

use super::CameraError;
use super::format;
use super::matrix::{self, Matrix, Tag};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pixel {
    Nv12,
    I420,
    /// Packed 4:2:2 in the order Y, U, Y, V.
    Yuy2,
    /// Packed 4:2:2 in the order U, Y, V, Y. Only DirectShow on Windows gives it.
    #[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
    Uyvy,
    /// Packed blue, green and red bytes (RGB24 of Windows). `bottom_up`: the first row in
    /// memory is the last row of the picture.
    #[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
    Bgr24 {
        bottom_up: bool,
    },
    /// Packed blue, green and red bytes and one unused byte (RGB32 and ARGB32 of Windows).
    #[cfg_attr(not(any(test, target_os = "windows")), allow(dead_code))]
    Bgrx {
        bottom_up: bool,
    },
}

impl Pixel {
    /// The bytes of one row of `width` pixels without padding.
    pub fn row(self, width: usize) -> Option<usize> {
        width.checked_mul(match self {
            Pixel::Nv12 | Pixel::I420 => 1,
            Pixel::Yuy2 | Pixel::Uyvy => 2,
            Pixel::Bgr24 { .. } => 3,
            Pixel::Bgrx { .. } => 4,
        })
    }

    fn rgb(self) -> bool {
        matches!(self, Pixel::Bgr24 { .. } | Pixel::Bgrx { .. })
    }
}

/// The largest row of a picture in bytes.
const LARGEST_STRIDE: usize = 16384;

#[derive(Clone, Copy, Debug)]
pub(super) struct Layout {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub pixel: Pixel,
    /// The YCbCr matrix of the pictures; RGB pictures do not use it.
    pub matrix: Tag,
}

impl Layout {
    /// The bytes of one picture, when capture takes pictures of this layout: a size that
    /// `format::takes` accepts and a stride that holds a row.
    pub fn length(self) -> Option<usize> {
        if !format::takes((self.width, self.height)) {
            return None;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let row = self.pixel.row(w)?;
        if self.stride < row || self.stride > LARGEST_STRIDE || !self.stride.is_multiple_of(2) {
            return None;
        }
        let rows = self.stride.checked_mul(h)?;
        match self.pixel {
            Pixel::Nv12 | Pixel::I420 => rows.checked_add(rows / 2),
            Pixel::Yuy2 | Pixel::Uyvy | Pixel::Bgr24 { .. } | Pixel::Bgrx { .. } => Some(rows),
        }
    }

    /// The size of the published pictures (`format::fit`).
    pub fn published(self) -> (u32, u32) {
        format::fit((self.width, self.height))
    }

    /// The limited-range 601 picture of `bytes`, at the published size.
    pub fn convert(self, bytes: &[u8]) -> Result<I420Buffer, CameraError> {
        let length = self.length().ok_or(CameraError::Unsupported)?;
        if bytes.len() < length {
            return Err(CameraError::Failed(
                "camera buffer is shorter than its planes".into(),
            ));
        }
        let mut output = I420Buffer::new(self.width, self.height);
        if self.pixel.rgb() {
            self.rgb_planes(bytes, &mut output);
        } else {
            self.yuv_planes(bytes, &mut output);
        }
        let (width, height) = self.published();
        if (width, height) != (self.width, self.height) {
            output = output.scale(width as i32, height as i32);
        }
        // The matrix of the source follows from the height of the camera picture, not from
        // the published height.
        let from = if self.pixel.rgb() {
            Matrix::BT601
        } else {
            matrix::source(self.matrix, self.height)
        };
        Ok(if from == Matrix::BT601 {
            output
        } else {
            matrix::to_601(&output, from)
        })
    }

    fn yuv_planes(self, bytes: &[u8], output: &mut I420Buffer) {
        let (w, h) = (self.width as usize, self.height as usize);
        let (sy, su, sv) = output.strides();
        let (sy, su, sv) = (sy as usize, su as usize, sv as usize);
        let (y, u, v) = output.data_mut();
        for row in 0..h {
            let line = &bytes[row * self.stride..];
            let target = &mut y[row * sy..row * sy + w];
            match self.pixel {
                Pixel::Nv12 | Pixel::I420 => target.copy_from_slice(&line[..w]),
                Pixel::Yuy2 => {
                    for (col, luma) in target.iter_mut().enumerate() {
                        *luma = line[col * 2];
                    }
                }
                _ => {
                    for (col, luma) in target.iter_mut().enumerate() {
                        *luma = line[col * 2 + 1];
                    }
                }
            }
        }
        let uv = self.stride * h;
        for row in 0..h / 2 {
            for col in 0..w / 2 {
                let (cb, cr) = match self.pixel {
                    Pixel::Nv12 => {
                        let p = uv + row * self.stride + col * 2;
                        (bytes[p], bytes[p + 1])
                    }
                    Pixel::I420 => {
                        let p = uv + row * (self.stride / 2) + col;
                        (bytes[p], bytes[p + uv / 4])
                    }
                    _ => {
                        let p = row * 2 * self.stride + col * 4;
                        let average = |offset| {
                            ((u16::from(bytes[p + offset])
                                + u16::from(bytes[p + self.stride + offset]))
                            .div_ceil(2)) as u8
                        };
                        if self.pixel == Pixel::Yuy2 {
                            (average(1), average(3))
                        } else {
                            (average(0), average(2))
                        }
                    }
                };
                u[row * su + col] = cb;
                v[row * sv + col] = cr;
            }
        }
    }

    /// Converts RGB pixels with the limited-range 601 matrix, as the publication path reads
    /// them. The chroma of each block of two by two pixels is that of their average colour.
    fn rgb_planes(self, bytes: &[u8], output: &mut I420Buffer) {
        let (w, h) = (self.width as usize, self.height as usize);
        let (size, bottom_up) = match self.pixel {
            Pixel::Bgr24 { bottom_up } => (3, bottom_up),
            Pixel::Bgrx { bottom_up } => (4, bottom_up),
            _ => return,
        };
        let line = |row: usize| {
            let at = if bottom_up { h - 1 - row } else { row } * self.stride;
            &bytes[at..at + w * size]
        };
        let (sy, su, sv) = output.strides();
        let (sy, su, sv) = (sy as usize, su as usize, sv as usize);
        let (y, u, v) = output.data_mut();
        for row in 0..h {
            let source = line(row);
            for (col, luma) in y[row * sy..row * sy + w].iter_mut().enumerate() {
                let p = col * size;
                *luma = luma_of(
                    i32::from(source[p + 2]),
                    i32::from(source[p + 1]),
                    i32::from(source[p]),
                );
            }
        }
        for row in 0..h / 2 {
            let (upper, lower) = (line(row * 2), line(row * 2 + 1));
            for col in 0..w / 2 {
                let mut sum = [0i32; 3];
                for source in [upper, lower] {
                    for p in [col * 2 * size, (col * 2 + 1) * size] {
                        for (channel, total) in sum.iter_mut().enumerate() {
                            *total += i32::from(source[p + 2 - channel]);
                        }
                    }
                }
                let [r, g, b] = sum.map(|total| (total + 2) / 4);
                let (cb, cr) = chroma_of(r, g, b);
                u[row * su + col] = cb;
                v[row * sv + col] = cr;
            }
        }
    }
}

/// The limited-range 601 luma of red, green and blue values from 0 to 255.
fn luma_of(r: i32, g: i32, b: i32) -> u8 {
    (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8
}

/// The limited-range 601 chroma (Cb, Cr) of red, green and blue values from 0 to 255.
fn chroma_of(r: i32, g: i32, b: i32) -> (u8, u8) {
    (
        (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8,
        (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8,
    )
}

#[cfg(test)]
mod tests;
