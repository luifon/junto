//! The layout of the pictures of a DirectShow video format block (`VIDEOINFOHEADER` and
//! `VIDEOINFOHEADER2`), from the fields that capture reads (ADR 0770).

use super::format::Mode;
use super::matrix::Tag;
use super::raw::{Layout, Pixel};

/// The uncompressed subtypes that capture reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Subtype {
    Nv12,
    /// I420 and IYUV.
    I420,
    Yuy2,
    Uyvy,
    Rgb24,
    /// RGB32 and ARGB32.
    Rgb32,
}

/// The kinds of DirectShow types that capture reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// An uncompressed type, which the sink takes directly.
    Uncompressed(Subtype),
    /// An MJPEG type, which the MJPEG decoder of DirectShow decodes between the source and the
    /// sink to an uncompressed picture of the same size.
    Mjpeg,
}

/// The capture mode of a type of `kind`: the size of the layout of an uncompressed type, or the
/// size of the bitmap header of an MJPEG type, and the rate of the frame time.
pub(super) fn mode_of(kind: Kind, video: &Video) -> Option<Mode> {
    let (width, height, compressed) = match kind {
        Kind::Uncompressed(subtype) => {
            let layout = layout_of(subtype, video)?;
            (layout.width, layout.height, false)
        }
        Kind::Mjpeg => (
            u32::try_from(video.width).ok().filter(|width| *width > 0)?,
            video.height.unsigned_abs(),
            true,
        ),
    };
    Some(Mode {
        width,
        height,
        rate: video.rate(),
        compressed,
    })
}

/// The fields of a video format block that capture reads.
#[derive(Clone, Copy, Debug)]
pub(super) struct Video {
    /// The width of the bitmap header: the stride in pixels.
    pub width: i32,
    /// The height of the bitmap header. For RGB, a positive height states that the first row
    /// in memory is the last row of the picture.
    pub height: i32,
    /// The width of the target rectangle: the visible width, or 0.
    pub visible: i32,
    /// The time of one picture in units of 100 nanoseconds.
    pub frame_time: i64,
    /// The control flags of `VIDEOINFOHEADER2`.
    pub flags: Option<u32>,
}

/// The control flags of a format block carry the colour of the pictures.
pub(super) const COLOR_INFO_PRESENT: u32 = 0x80;

/// The layout of uncompressed pictures of `subtype`. The width of the bitmap header is the
/// stride in pixels, and the target rectangle, when it is set, the visible width. The planes
/// of YUV formats run from the top whatever the sign of the height; RGB rows run from the
/// bottom for a positive height, and each row of RGB24 is padded to four bytes. A stated full
/// range of a YUV format is refused, as the Media Foundation path refuses it; RGB pictures are
/// full range and are converted with the 601 matrix.
pub(super) fn layout_of(subtype: Subtype, video: &Video) -> Option<Layout> {
    let stride_pixels = u32::try_from(video.width).ok().filter(|width| *width > 0)?;
    let width = match u32::try_from(video.visible) {
        Ok(visible) if visible > 0 && visible <= stride_pixels => visible,
        _ => stride_pixels,
    };
    let height = video.height.unsigned_abs();
    let bottom_up = video.height > 0;
    let pixel = match subtype {
        Subtype::Nv12 => Pixel::Nv12,
        Subtype::I420 => Pixel::I420,
        Subtype::Yuy2 => Pixel::Yuy2,
        Subtype::Uyvy => Pixel::Uyvy,
        Subtype::Rgb24 => Pixel::Bgr24 { bottom_up },
        Subtype::Rgb32 => Pixel::Bgrx { bottom_up },
    };
    let mut matrix = Tag::Missing;
    if matches!(subtype, Subtype::Rgb24 | Subtype::Rgb32) {
        matrix = Tag::Bt601;
    } else if let Some(flags) = video.flags.filter(|flags| flags & COLOR_INFO_PRESENT != 0) {
        if (flags >> 12) & 0x7 == 1 {
            return None;
        }
        matrix = match (flags >> 15) & 0x7 {
            0 => Tag::Missing,
            1 => Tag::Bt709,
            2 => Tag::Bt601,
            3 => Tag::Smpte240m,
            _ => Tag::Other,
        };
    }
    let row = pixel.row(stride_pixels as usize)?;
    let stride = if subtype == Subtype::Rgb24 {
        row.checked_add(3)? & !3
    } else {
        row
    };
    let layout = Layout {
        width,
        height,
        stride,
        pixel,
        matrix,
    };
    layout.length()?;
    Some(layout)
}

impl Video {
    /// The frame rate of the frame time, which is in units of 100 nanoseconds.
    pub(super) fn rate(&self) -> Option<(u32, u32)> {
        let frame_time = u32::try_from(self.frame_time)
            .ok()
            .filter(|time| *time > 0)?;
        Some((10_000_000, frame_time))
    }
}

/// The base of the subtypes that are named by a four-character code: the code is the first
/// field of the identifier.
const FOUR_CHARACTER_BASE: u128 = 0x0000_0000_0000_0010_8000_00aa_0038_9b71;

/// The name of a video subtype for the log: the name of a known RGB subtype of DirectShow or
/// Media Foundation, its four-character code, or the identifier.
pub(super) fn subtype_name(identifier: u128) -> String {
    const NAMED: [(u128, &str); 4] = [
        (0xe436eb7d_524f_11ce_9f53_0020af0ba770, "RGB24"),
        (0xe436eb7e_524f_11ce_9f53_0020af0ba770, "RGB32"),
        (0x773c9ac0_3274_11d0_b724_00aa006c1a01, "ARGB32"),
        (0xe436eb7c_524f_11ce_9f53_0020af0ba770, "RGB555"),
    ];
    // The RGB subtypes of Media Foundation have the base of the four-character codes and a
    // Direct3D format number as their first field.
    const DIRECT_3D: [(u32, &str); 7] = [
        (20, "RGB24"),
        (21, "ARGB32"),
        (22, "RGB32"),
        (23, "RGB565"),
        (24, "RGB555"),
        (31, "A2R10G10B10"),
        (41, "RGB8"),
    ];
    if let Some((_, name)) = NAMED.iter().find(|(known, _)| *known == identifier) {
        return (*name).to_owned();
    }
    if identifier & ((1 << 96) - 1) == FOUR_CHARACTER_BASE
        && let Some((_, name)) = DIRECT_3D
            .iter()
            .find(|(number, _)| u128::from(*number) == identifier >> 96)
    {
        return (*name).to_owned();
    }
    if identifier & ((1 << 96) - 1) == FOUR_CHARACTER_BASE {
        let code = ((identifier >> 96) as u32).to_le_bytes();
        if code.iter().all(|byte| byte.is_ascii_graphic()) {
            return String::from_utf8_lossy(&code).into_owned();
        }
    }
    format!("{identifier:032x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(width: i32, height: i32) -> Video {
        Video {
            width,
            height,
            visible: 0,
            frame_time: 333_333,
            flags: None,
        }
    }

    #[test]
    fn a_direct_show_block_gives_the_layout_of_the_media_foundation_path() {
        let yuy2 = layout_of(Subtype::Yuy2, &video(1280, 720)).unwrap();
        assert_eq!((yuy2.width, yuy2.height, yuy2.stride), (1280, 720, 2560));
        let nv12 = layout_of(Subtype::Nv12, &video(640, -480)).unwrap();
        assert_eq!(
            (nv12.height, nv12.stride, nv12.pixel),
            (480, 640, Pixel::Nv12)
        );
        let uyvy = layout_of(Subtype::Uyvy, &video(1920, 1080)).unwrap();
        assert_eq!(
            (uyvy.stride, uyvy.pixel, uyvy.published()),
            (3840, Pixel::Uyvy, (1280, 720))
        );
        let i420 = layout_of(Subtype::I420, &video(640, 480)).unwrap();
        assert_eq!((i420.stride, i420.pixel), (640, Pixel::I420));
        assert!(layout_of(Subtype::Nv12, &video(0, 480)).is_none());
        assert!(layout_of(Subtype::Nv12, &video(8192, 480)).is_none());
        assert_eq!(video(640, 480).rate(), Some((10_000_000, 333_333)));
        let unstated = Video {
            frame_time: 0,
            ..video(640, 480)
        };
        assert_eq!(unstated.rate(), None);
    }

    #[test]
    fn an_rgb_block_states_its_row_order_by_the_sign_of_its_height() {
        let up = layout_of(Subtype::Rgb24, &video(1918, 1080)).unwrap();
        assert_eq!(up.pixel, Pixel::Bgr24 { bottom_up: true });
        // Each row of RGB24 is padded to four bytes.
        assert_eq!(up.stride, 5756);
        let down = layout_of(Subtype::Rgb32, &video(1920, -1080)).unwrap();
        assert_eq!(
            (down.pixel, down.stride),
            (Pixel::Bgrx { bottom_up: false }, 7680)
        );
        // A full range flag does not refuse RGB, which is full range.
        let full = COLOR_INFO_PRESENT | (1 << 12);
        let flagged = Video {
            flags: Some(full),
            ..video(640, 480)
        };
        assert!(layout_of(Subtype::Rgb32, &flagged).is_some());
        assert!(layout_of(Subtype::Nv12, &flagged).is_none());
    }

    #[test]
    fn a_stated_full_range_is_refused_and_a_stated_matrix_is_kept() {
        let with = |flags| Video {
            flags,
            ..video(640, 480)
        };
        let matrix = |flags| layout_of(Subtype::Nv12, &with(flags)).map(|layout| layout.matrix);
        assert_eq!(matrix(None), Some(Tag::Missing));
        assert_eq!(
            matrix(Some(COLOR_INFO_PRESENT | (1 << 15) | (2 << 12))),
            Some(Tag::Bt709)
        );
        assert_eq!(
            matrix(Some(COLOR_INFO_PRESENT | (2 << 15) | (1 << 12))),
            None
        );
        // Without the flag the bits are not colour information.
        assert_eq!(matrix(Some((1 << 15) | (1 << 12))), Some(Tag::Missing));
    }

    #[test]
    fn a_visible_width_inside_the_stride_is_the_width() {
        let padded = Video {
            visible: 1272,
            ..video(1280, 720)
        };
        let layout = layout_of(Subtype::Nv12, &padded).unwrap();
        assert_eq!((layout.width, layout.stride), (1272, 1280));
    }

    #[test]
    fn a_subtype_is_named_by_its_code_or_its_rgb_name() {
        assert_eq!(subtype_name(0x3231564e_0000_0010_8000_00aa00389b71), "NV12");
        assert_eq!(subtype_name(0x47504a4d_0000_0010_8000_00aa00389b71), "MJPG");
        assert_eq!(
            subtype_name(0xe436eb7d_524f_11ce_9f53_0020af0ba770),
            "RGB24"
        );
        assert_eq!(
            subtype_name(0x773c9ac0_3274_11d0_b724_00aa006c1a01),
            "ARGB32"
        );
        // The RGB subtypes of Media Foundation.
        assert_eq!(
            subtype_name(0x00000016_0000_0010_8000_00aa00389b71),
            "RGB32"
        );
        assert_eq!(
            subtype_name(0x00000014_0000_0010_8000_00aa00389b71),
            "RGB24"
        );
        assert_eq!(
            subtype_name(0x00000015_0000_0010_8000_00aa00389b71),
            "ARGB32"
        );
        assert_eq!(
            subtype_name(0x00000099_0000_0010_8000_00aa00389b71),
            "0000009900000010800000aa00389b71"
        );
        assert_eq!(
            subtype_name(0x00000016_0000_0010_8000_00aa00389b72),
            "0000001600000010800000aa00389b72"
        );
    }

    #[test]
    fn an_mjpeg_type_gives_a_compressed_mode_of_its_header_size() {
        let mjpeg = mode_of(Kind::Mjpeg, &video(1920, 1080)).unwrap();
        assert_eq!(
            mjpeg,
            Mode {
                width: 1920,
                height: 1080,
                rate: Some((10_000_000, 333_333)),
                compressed: true,
            }
        );
        // A top-down header states a negative height.
        assert_eq!(
            mode_of(Kind::Mjpeg, &video(1280, -720)).map(|mode| (mode.width, mode.height)),
            Some((1280, 720))
        );
        assert!(mode_of(Kind::Mjpeg, &video(0, 720)).is_none());
        assert!(mode_of(Kind::Mjpeg, &video(-1280, 720)).is_none());
        let raw = mode_of(Kind::Uncompressed(Subtype::Yuy2), &video(1280, 720)).unwrap();
        assert!(!raw.compressed);
        assert_eq!((raw.width, raw.height), (1280, 720));
        // An uncompressed type needs a layout that capture takes.
        assert!(mode_of(Kind::Uncompressed(Subtype::Nv12), &video(8192, 480)).is_none());
    }

    /// The types of a capture pin, as fake format blocks, in the order of a DirectShow start:
    /// every uncompressed type that capture takes before every MJPEG type.
    #[test]
    fn a_direct_show_start_tries_the_uncompressed_types_and_then_the_mjpeg_types() {
        let at = |frame_time| Video {
            frame_time,
            ..video(1280, 720)
        };
        let types = [
            (Kind::Mjpeg, at(333_333)),
            (Kind::Uncompressed(Subtype::Yuy2), at(1_000_000)),
            (Kind::Mjpeg, video(1920, 1080)),
            (Kind::Uncompressed(Subtype::Nv12), video(640, 480)),
            (Kind::Mjpeg, video(640, 480)),
        ];
        let modes: Vec<Mode> = types
            .iter()
            .map(|(kind, video)| mode_of(*kind, video).unwrap())
            .collect();
        assert_eq!(
            super::super::format::uncompressed_first(&modes),
            [3, 1, 0, 2, 4]
        );
        // A camera with only MJPEG types, such as a capture device that offers no other, is
        // started through the decoder in the order of the rule.
        let modes: Vec<Mode> = [video(1920, 1080), video(1280, 720), video(4096, 4096)]
            .iter()
            .map(|video| mode_of(Kind::Mjpeg, video).unwrap())
            .collect();
        assert_eq!(super::super::format::uncompressed_first(&modes), [1, 0]);
    }
}
