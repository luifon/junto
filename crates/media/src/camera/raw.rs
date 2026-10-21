//! Bounds and colour conversion for camera planes owned by the operating system.

use livekit::webrtc::video_frame::I420Buffer;

use super::CameraError;
use super::matrix::{self, Matrix, Tag};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pixel {
    Nv12,
    I420,
    Yuy2,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Layout {
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub pixel: Pixel,
    pub matrix: Tag,
}

impl Layout {
    pub fn length(self) -> Option<usize> {
        let (w, h) = (self.width as usize, self.height as usize);
        if w == 0 || h == 0 || w > 1280 || h > 720 || !w.is_multiple_of(2) || !h.is_multiple_of(2) {
            return None;
        }
        let row = if self.pixel == Pixel::Yuy2 { w * 2 } else { w };
        if self.stride < row || self.stride > 16384 || !self.stride.is_multiple_of(2) {
            return None;
        }
        let luma = self.stride.checked_mul(h)?;
        if self.pixel == Pixel::Yuy2 {
            Some(luma)
        } else {
            luma.checked_add(luma / 2)
        }
    }

    pub fn convert(self, bytes: &[u8]) -> Result<I420Buffer, CameraError> {
        let length = self.length().ok_or(CameraError::Unsupported)?;
        if bytes.len() < length {
            return Err(CameraError::Failed(
                "camera buffer is shorter than its planes".into(),
            ));
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let mut output = I420Buffer::new(self.width, self.height);
        let (sy, su, sv) = output.strides();
        let (y, u, v) = output.data_mut();
        for row in 0..h {
            for col in 0..w {
                let x = if self.pixel == Pixel::Yuy2 {
                    col * 2
                } else {
                    col
                };
                y[row * sy as usize + col] = bytes[row * self.stride + x];
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
                    Pixel::Yuy2 => {
                        let p = row * 2 * self.stride + col * 4;
                        let average = |offset| {
                            ((u16::from(bytes[p + offset])
                                + u16::from(bytes[p + self.stride + offset]))
                            .div_ceil(2)) as u8
                        };
                        (average(1), average(3))
                    }
                };
                u[row * su as usize + col] = cb;
                v[row * sv as usize + col] = cr;
            }
        }
        let from = matrix::source(self.matrix, self.height);
        Ok(if from == Matrix::BT601 {
            output
        } else {
            matrix::to_601(&output, from)
        })
    }
}

#[cfg(test)]
mod tests;
