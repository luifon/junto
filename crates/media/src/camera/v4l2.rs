//! The decisions of V4L2 capture that need no device, so that every system tests them.
use super::CameraError;
use super::matrix::Tag;

/// A buffer that the driver returned, as `junto_camera_next` reports it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Dequeued {
    pub index: u32,
    pub used: u32,
    pub flags: u32,
    /// The count of mapped buffers and the length of each.
    pub count: u32,
    pub length: u32,
    /// The time stamp of a monotonic clock in nanoseconds; 0 when the driver states none.
    pub timestamp: u64,
}

const BUFFER_ERROR: u32 = 0x40;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Buffer {
    /// A complete picture.
    Copy,
    /// A picture that the driver marked as damaged or did not complete; capture goes on.
    Discard,
    /// A buffer that names no mapping of the capture: the driver does not follow the API.
    Invalid,
}

/// What to do with a returned buffer whose picture needs `needed` bytes.
pub(super) fn buffer(dequeued: &Dequeued, needed: usize) -> Buffer {
    if dequeued.index >= dequeued.count || dequeued.used > dequeued.length {
        Buffer::Invalid
    } else if dequeued.flags & BUFFER_ERROR != 0 || (dequeued.used as usize) < needed {
        Buffer::Discard
    } else {
        Buffer::Copy
    }
}

/// The matrix of a V4L2 Y'CbCr encoding, after the driver default was resolved. The
/// extended-gamut encodings xvYCC 601 and 709 and sYCC use the matrix of their name.
pub(super) fn matrix(encoding: u32) -> Tag {
    match encoding {
        0 => Tag::Missing,
        1 | 3 | 5 => Tag::Bt601,
        2 | 4 => Tag::Bt709,
        6 => Tag::Bt2020,
        8 => Tag::Smpte240m,
        _ => Tag::Other,
    }
}

/// The failure of a refused frame-rate request that ends the start; `None` when the camera
/// keeps the rate of its driver.
pub(super) fn rate_failure(code: i32) -> Option<CameraError> {
    const NO_DEVICE: i32 = -19;
    const BUSY: i32 = -16;
    match code {
        NO_DEVICE => Some(CameraError::DeviceGone),
        BUSY => Some(CameraError::Failed(
            std::io::Error::from_raw_os_error(-code).to_string(),
        )),
        _ => None,
    }
}

/// A frame size or a frame interval of the driver, as `junto_camera_size` and
/// `junto_camera_interval` report it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Range {
    /// 1 discrete, 2 continuous, 3 stepwise, as in V4L2.
    pub kind: u32,
    /// Discrete: width and height, or numerator and denominator of the interval. Otherwise
    /// the minimum, the maximum and the step of the first value, then of the second.
    pub values: [u32; 6],
}

/// Whether a frame size of the driver includes `size`.
pub(super) fn fits(range: &Range, (width, height): (u32, u32)) -> bool {
    let v = range.values;
    let within = |value: u32, minimum: u32, maximum: u32, step: u32| {
        (minimum..=maximum).contains(&value) && (value - minimum).is_multiple_of(step.max(1))
    };
    match range.kind {
        1 => (v[0], v[1]) == (width, height),
        2 | 3 => within(width, v[0], v[1], v[2]) && within(height, v[3], v[4], v[5]),
        _ => false,
    }
}

/// The frame rates, as fractions of frames per second, that the intervals of the driver
/// offer for one format and size.
pub(super) fn rates(ranges: &[Range]) -> Vec<(u32, u32)> {
    let mut rates = Vec::new();
    for range in ranges {
        let v = range.values;
        // An interval of n/d seconds is a rate of d/n frames per second.
        let mut add = |n: u32, d: u32| {
            if n != 0 && d != 0 && !rates.contains(&(d, n)) {
                rates.push((d, n));
            }
        };
        match range.kind {
            1 => add(v[0], v[1]),
            2 | 3 => {
                add(v[0], v[1]);
                add(v[2], v[3]);
                // 1/30 s within [minimum, maximum]: n_min/d_min <= 1/30 <= n_max/d_max.
                let (minimum, maximum) = (
                    (u64::from(v[0]), u64::from(v[1])),
                    (u64::from(v[2]), u64::from(v[3])),
                );
                if minimum.1 != 0
                    && maximum.1 != 0
                    && 30 * minimum.0 <= minimum.1
                    && maximum.1 <= 30 * maximum.0
                {
                    add(1, 30);
                }
            }
            _ => {}
        }
    }
    rates
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dequeued(index: u32, used: u32, flags: u32) -> Dequeued {
        Dequeued {
            index,
            used,
            flags,
            count: 4,
            length: 1000,
            timestamp: 0,
        }
    }

    #[test]
    fn damaged_and_short_buffers_are_discarded_and_foreign_ones_are_invalid() {
        assert_eq!(buffer(&dequeued(0, 900, 0), 900), Buffer::Copy);
        assert_eq!(buffer(&dequeued(3, 1000, 0), 900), Buffer::Copy);
        assert_eq!(buffer(&dequeued(0, 10, BUFFER_ERROR), 900), Buffer::Discard);
        assert_eq!(
            buffer(&dequeued(0, 900, BUFFER_ERROR), 900),
            Buffer::Discard
        );
        assert_eq!(buffer(&dequeued(0, 10, 0), 900), Buffer::Discard);
        assert_eq!(buffer(&dequeued(0, 0, 0), 900), Buffer::Discard);
        assert_eq!(buffer(&dequeued(4, 900, 0), 900), Buffer::Invalid);
        assert_eq!(buffer(&dequeued(4, 10, BUFFER_ERROR), 900), Buffer::Invalid);
        assert_eq!(buffer(&dequeued(0, 1001, 0), 900), Buffer::Invalid);
    }

    #[test]
    fn extended_gamut_encodings_use_their_matrix() {
        for (encoding, tag) in [
            (0, Tag::Missing),
            (1, Tag::Bt601),
            (2, Tag::Bt709),
            (3, Tag::Bt601),
            (4, Tag::Bt709),
            (5, Tag::Bt601),
            (6, Tag::Bt2020),
            (7, Tag::Other),
            (8, Tag::Smpte240m),
            (99, Tag::Other),
        ] {
            assert_eq!(matrix(encoding), tag, "{encoding}");
        }
    }

    #[test]
    fn only_a_removed_or_busy_device_ends_the_start_at_the_rate_request() {
        assert_eq!(rate_failure(-19), Some(CameraError::DeviceGone));
        assert!(matches!(rate_failure(-16), Some(CameraError::Failed(_))));
        for code in [-22, -25, -5, -95] {
            assert_eq!(rate_failure(code), None, "{code}");
        }
    }

    fn range(kind: u32, values: [u32; 6]) -> Range {
        Range { kind, values }
    }

    #[test]
    fn a_capture_size_fits_a_discrete_or_stepwise_frame_size() {
        assert!(fits(&range(1, [1280, 720, 0, 0, 0, 0]), (1280, 720)));
        assert!(!fits(&range(1, [1280, 720, 0, 0, 0, 0]), (640, 480)));
        let stepwise = range(3, [160, 1920, 16, 120, 1080, 8]);
        assert!(fits(&stepwise, (1280, 720)));
        assert!(fits(&stepwise, (352, 288)));
        assert!(!fits(&stepwise, (1290, 720)));
        assert!(!fits(&range(3, [160, 640, 16, 120, 480, 8]), (1280, 720)));
        assert!(fits(&range(2, [1, 4096, 1, 1, 2160, 1]), (640, 480)));
        assert!(fits(&range(3, [1, 4096, 0, 1, 2160, 0]), (640, 480)));
    }

    #[test]
    fn intervals_give_their_rates_and_thirty_when_a_range_includes_it() {
        assert_eq!(
            rates(&[
                range(1, [1, 30, 0, 0, 0, 0]),
                range(1, [1001, 30000, 0, 0, 0, 0]),
                range(1, [1, 10, 0, 0, 0, 0]),
                range(1, [0, 30, 0, 0, 0, 0]),
            ]),
            [(30, 1), (30000, 1001), (10, 1)]
        );
        assert_eq!(
            rates(&[range(3, [1, 60, 1, 5, 1, 60])]),
            [(60, 1), (5, 1), (30, 1)]
        );
        assert_eq!(rates(&[range(2, [1, 15, 1, 5, 1, 1])]), [(15, 1), (5, 1)]);
    }
}
