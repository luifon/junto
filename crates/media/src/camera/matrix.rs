//! The YCbCr matrix of a camera buffer, and the conversion of limited-range planes in another
//! matrix to the limited-range 601 matrix of the publication path (ADR 0770).
use livekit::webrtc::video_frame::{I420Buffer, VideoBuffer};

/// The luma coefficients Kr and Kb of a YCbCr matrix, as ITU-T H.273 defines them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Matrix {
    kr: f32,
    kb: f32,
}

impl Matrix {
    pub(super) const BT601: Self = Self {
        kr: 0.299,
        kb: 0.114,
    };
    pub(super) const BT709: Self = Self {
        kr: 0.2126,
        kb: 0.0722,
    };
    pub(super) const SMPTE_240M: Self = Self {
        kr: 0.212,
        kb: 0.087,
    };
    /// The non-constant-luminance matrix of ITU-R BT.2020.
    pub(super) const BT2020: Self = Self {
        kr: 0.2627,
        kb: 0.0593,
    };
}

/// The matrix attachment of a camera buffer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tag {
    Missing,
    Bt601,
    Bt709,
    Smpte240m,
    Bt2020,
    /// A tag that names no matrix of `Matrix`, such as the deprecated P3 tags of Core Video.
    Other,
}

/// The largest picture height of standard-definition video.
const STANDARD_DEFINITION_HEIGHT: u32 = 576;

/// The matrix of a camera buffer. A buffer without a known matrix tag uses the usual rule of
/// video decoders: 601 up to standard-definition height, 709 above it.
pub(super) fn source(tag: Tag, height: u32) -> Matrix {
    match tag {
        Tag::Bt601 => Matrix::BT601,
        Tag::Bt709 => Matrix::BT709,
        Tag::Smpte240m => Matrix::SMPTE_240M,
        Tag::Bt2020 => Matrix::BT2020,
        Tag::Missing | Tag::Other if height <= STANDARD_DEFINITION_HEIGHT => Matrix::BT601,
        Tag::Missing | Tag::Other => Matrix::BT709,
    }
}

/// Converts limited-range planes in `from` to owned limited-range 601 planes. The inverse
/// `from` matrix and the forward 601 matrix are combined without an intermediate RGB clip.
/// The chroma value of each 2x2 block corrects the luma of each pixel in the block.
pub(super) fn to_601(source: &I420Buffer, from: Matrix) -> I420Buffer {
    let (width, height) = (source.width() as usize, source.height() as usize);
    let (sy, su, sv) = source.strides();
    let (y, u, v) = source.data();
    let mut output = I420Buffer::new(width as u32, height as u32);
    let (dy, du, dv) = output.strides();
    let (oy, ou, ov) = output.data_mut();
    let byte = |value: f32| value.round().clamp(0.0, 255.0) as u8;
    let to = Matrix::BT601;
    for row in 0..height.div_ceil(2) {
        for col in 0..width.div_ceil(2) {
            let cb = f32::from(u[row * su as usize + col]) - 128.0;
            let cr = f32::from(v[row * sv as usize + col]) - 128.0;
            // R - Y, G - Y and B - Y of the source, on the 224-level chroma scale.
            let red = 2.0 * (1.0 - from.kr) * cr;
            let blue = 2.0 * (1.0 - from.kb) * cb;
            let green = -(from.kr * red + from.kb * blue) / (1.0 - from.kr - from.kb);
            let luma = to.kr * red + (1.0 - to.kr - to.kb) * green + to.kb * blue;
            ou[row * du as usize + col] = byte(128.0 + (blue - luma) / (2.0 * (1.0 - to.kb)));
            ov[row * dv as usize + col] = byte(128.0 + (red - luma) / (2.0 * (1.0 - to.kr)));
            for py in row * 2..(row * 2 + 2).min(height) {
                for px in col * 2..(col * 2 + 2).min(width) {
                    oy[py * dy as usize + px] =
                        byte(f32::from(y[py * sy as usize + px]) + luma * (219.0 / 224.0));
                }
            }
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tags_name_their_matrix_at_every_size() {
        for height in [4, 576, 577, 720, 1080] {
            assert_eq!(source(Tag::Bt601, height), Matrix::BT601);
            assert_eq!(source(Tag::Bt709, height), Matrix::BT709);
            assert_eq!(source(Tag::Smpte240m, height), Matrix::SMPTE_240M);
            assert_eq!(source(Tag::Bt2020, height), Matrix::BT2020);
        }
    }

    #[test]
    fn buffers_without_a_known_tag_use_601_up_to_standard_definition() {
        for tag in [Tag::Missing, Tag::Other] {
            for height in [4, 288, 480, 576] {
                assert_eq!(source(tag, height), Matrix::BT601, "{tag:?} {height}");
            }
            for height in [577, 720, 1080, 2160] {
                assert_eq!(source(tag, height), Matrix::BT709, "{tag:?} {height}");
            }
        }
    }

    /// An odd-sized buffer of one limited-range colour.
    fn planes((luma, cb, cr): (u8, u8, u8)) -> I420Buffer {
        let mut buffer = I420Buffer::new(5, 3);
        let (y, u, v) = buffer.data_mut();
        y.fill(luma);
        u.fill(cb);
        v.fill(cr);
        buffer
    }

    fn decoded(buffer: &I420Buffer) -> Vec<(u8, u8, u8)> {
        let frame = crate::share::picture::convert(buffer, (5, 3)).unwrap();
        (0..3)
            .flat_map(|y| (0..5).map(move |x| (x, y)))
            .map(|(x, y)| frame.pixel(x, y).unwrap())
            .collect()
    }

    /// The luma, Cb and Cr samples of each pixel, without the padding of the rows.
    fn samples(buffer: &I420Buffer) -> Vec<(u8, u8, u8)> {
        let (sy, su, sv) = buffer.strides();
        let (y, u, v) = buffer.data();
        (0..3)
            .flat_map(|py| (0..5).map(move |px| (px, py)))
            .map(|(px, py)| {
                let chroma = |plane: &[u8], stride: u32| plane[py / 2 * stride as usize + px / 2];
                (y[py * sy as usize + px], chroma(u, su), chroma(v, sv))
            })
            .collect()
    }

    #[test]
    fn each_matrix_converts_to_the_same_601_colour() {
        // The limited-range encodings of the colour (25, 220, 16) under each matrix. The exact
        // conversion of each encoding is at least 0.1 level from a rounding boundary of the 601
        // encoding (135, 67, 57), so a correct conversion gives these samples exactly. With the
        // coefficients of another matrix, a sample differs by 2 levels or more: the 240M
        // encoding converted with the 709 coefficients gives (132, 67, 58), and the 709
        // encoding converted with the 240M coefficients gives (137, 67, 56). Decoded with exact
        // arithmetic, these two differ from the colour by at most 4 levels, which the tolerance
        // of the decoded check accepts; the samples detect them.
        for (from, encoded) in [
            (Matrix::BT709, (157, 58, 51)),
            (Matrix::SMPTE_240M, (154, 58, 52)),
            (Matrix::BT2020, (151, 62, 50)),
        ] {
            let converted = to_601(&planes(encoded), from);
            for actual in samples(&converted) {
                assert_eq!(actual, (135, 67, 57), "{from:?}");
            }
            for actual in decoded(&converted) {
                for (a, expected) in [(actual.0, 25), (actual.1, 220), (actual.2, 16)] {
                    assert!(a.abs_diff(expected) <= 4, "{from:?} {actual:?}");
                }
            }
        }
    }

    #[test]
    fn converting_from_601_keeps_the_planes() {
        let source = planes((128, 76, 63));
        assert_eq!(to_601(&source, Matrix::BT601).data(), source.data());
    }
}
