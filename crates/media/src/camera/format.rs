//! Capture sizes and modes, in order of preference, bounded by the camera publication limit.

/// The capture sizes that capture asks for where a camera states a range of sizes or none: the
/// session presets of macOS, and the sizes inside a range of a V4L2 driver.
#[cfg(any(test, target_os = "macos", target_os = "linux"))]
pub(super) const PRESETS: [(u32, u32); 3] = [(1280, 720), (640, 480), (352, 288)];

#[cfg(any(test, target_os = "macos"))]
pub(super) fn choose(mut accepts: impl FnMut((u32, u32)) -> bool) -> Option<(u32, u32)> {
    PRESETS.into_iter().find(|size| accepts(*size))
}

#[cfg(any(test, target_os = "windows", target_os = "linux"))]
/// The order of frame rates: the highest at or below 30, then the lowest above 30.
pub(super) fn rate_order(a: (u32, u32), b: (u32, u32)) -> std::cmp::Ordering {
    let above = |(n, d): (u32, u32)| u64::from(n) > 30 * u64::from(d);
    above(a).cmp(&above(b)).then_with(|| {
        let order = (u64::from(a.0) * u64::from(b.1)).cmp(&(u64::from(b.0) * u64::from(a.1)));
        if above(a) { order } else { order.reverse() }
    })
}

#[cfg(any(test, target_os = "windows", target_os = "linux"))]
/// A capture mode that a camera offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Mode {
    pub width: u32,
    pub height: u32,
    /// Frames per second as a fraction; `None` when the driver states no rate.
    pub rate: Option<(u32, u32)>,
    /// Whether the pictures are compressed and need a decoder.
    pub compressed: bool,
}

#[cfg(any(test, target_os = "windows", target_os = "linux"))]
/// The lowest frame rate that is taken before a larger picture.
const MINIMUM_RATE: u32 = 24;

/// The largest published camera picture. A larger camera picture is scaled down to fit inside
/// it, with its proportions (ADR 0770).
#[cfg(any(test, target_os = "windows", target_os = "linux"))]
pub(super) const PUBLISHED: (u32, u32) = (1280, 720);

/// The largest camera picture that capture takes: 4096 pixels on a side and the pixels of
/// 3840 by 2160 in all.
#[cfg(any(test, target_os = "windows", target_os = "linux"))]
const LARGEST: (u32, u32) = (3840, 2160);

/// Whether capture takes pictures of `size`: both sides even and not zero, and within
/// `LARGEST`.
#[cfg(any(test, target_os = "windows", target_os = "linux"))]
pub(super) fn takes((width, height): (u32, u32)) -> bool {
    width > 0
        && height > 0
        && width.is_multiple_of(2)
        && height.is_multiple_of(2)
        && width <= 4096
        && height <= 4096
        && u64::from(width) * u64::from(height) <= u64::from(LARGEST.0) * u64::from(LARGEST.1)
}

/// The published size of a camera picture of `size`: the size itself when it fits inside
/// `PUBLISHED`, otherwise the largest size with the same proportions that fits, with even
/// sides.
#[cfg(any(test, target_os = "windows", target_os = "linux"))]
pub(super) fn fit((width, height): (u32, u32)) -> (u32, u32) {
    if width <= PUBLISHED.0 && height <= PUBLISHED.1 {
        return (width, height);
    }
    let (wide, high) = (u64::from(width), u64::from(height));
    let (limit_w, limit_h) = (u64::from(PUBLISHED.0), u64::from(PUBLISHED.1));
    let (fitted_w, fitted_h) = if wide * limit_h >= high * limit_w {
        (limit_w, high * limit_w / wide.max(1))
    } else {
        (wide * limit_h / high.max(1), limit_h)
    };
    let even = |side: u64| (u32::try_from(side).unwrap_or(0) & !1).max(2);
    (even(fitted_w), even(fitted_h))
}

/// A mode, a type or a format of a camera for the log: its subtype, size and rate.
#[cfg(any(test, target_os = "windows"))]
pub(super) fn describe(
    subtype: &str,
    size: Option<(u32, u32)>,
    rate: Option<(u32, u32)>,
) -> String {
    let mut text = subtype.to_owned();
    if let Some((width, height)) = size {
        text.push_str(&format!(" {width}x{height}"));
    }
    if let Some((numerator, denominator)) = rate.filter(|(_, denominator)| *denominator > 0) {
        text.push_str(&format!(
            " {:.2} fps",
            f64::from(numerator) / f64::from(denominator)
        ));
    }
    text
}

/// The types that a camera offered, for the log: the description of each type, and
/// "(not read)" after a type that capture does not read.
#[cfg(any(test, target_os = "windows"))]
pub(super) fn offered(types: impl IntoIterator<Item = (String, bool)>) -> String {
    let listed: Vec<String> = types
        .into_iter()
        .map(|(text, read)| {
            if read {
                text
            } else {
                format!("{text} (not read)")
            }
        })
        .collect();
    if listed.is_empty() {
        "none".to_owned()
    } else {
        listed.join(", ")
    }
}

#[cfg(any(test, target_os = "windows", target_os = "linux"))]
fn reaches_minimum(mode: &Mode) -> bool {
    mode.rate
        .is_none_or(|(n, d)| u64::from(n) >= u64::from(MINIMUM_RATE) * u64::from(d))
}

/// The modes that capture takes (`takes`), best first: the modes of at least 24 frames per
/// second by published size (`fit`), largest first; then the others by rate, highest first,
/// then by published size. At one published size: the smaller picture first, because a larger
/// one costs a scale for each picture; then the best rate of `rate_order`, then a stated rate,
/// then an uncompressed mode, because decoding costs processor time for each picture. A mode
/// without a stated rate counts as reaching 24.
#[cfg(any(test, target_os = "windows", target_os = "linux"))]
pub(super) fn order(modes: &[Mode]) -> Vec<usize> {
    let published = |mode: &Mode| {
        let (width, height) = fit((mode.width, mode.height));
        u64::from(width) * u64::from(height)
    };
    let source = |mode: &Mode| u64::from(mode.width) * u64::from(mode.height);
    let within_size = |a: &Mode, b: &Mode| {
        a.rate
            .is_none()
            .cmp(&b.rate.is_none())
            .then_with(|| match (a.rate, b.rate) {
                (Some(a), Some(b)) => rate_order(a, b),
                _ => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.compressed.cmp(&b.compressed))
    };
    let mut indices: Vec<usize> = (0..modes.len())
        .filter(|index| takes((modes[*index].width, modes[*index].height)))
        .collect();
    indices.sort_by(|a, b| {
        let (a, b) = (&modes[*a], &modes[*b]);
        reaches_minimum(b)
            .cmp(&reaches_minimum(a))
            .then_with(|| {
                if reaches_minimum(a) {
                    published(b).cmp(&published(a))
                } else {
                    rate_order(a.rate.unwrap_or((0, 1)), b.rate.unwrap_or((0, 1)))
                        .then_with(|| published(b).cmp(&published(a)))
                }
            })
            .then_with(|| source(a).cmp(&source(b)))
            .then_with(|| within_size(a, b))
    });
    indices
}

/// The order of `order`, with every uncompressed mode before every compressed mode. DirectShow
/// takes this order: there a compressed mode needs a decoder filter between the source and the
/// sink, which decodes each picture on the processor (ADR 0770).
#[cfg(any(test, target_os = "windows"))]
pub(super) fn uncompressed_first(modes: &[Mode]) -> Vec<usize> {
    let mut indices = order(modes);
    indices.sort_by_key(|index| modes[*index].compressed);
    indices
}

/// The index of the mode to capture with: the first of `order`.
#[cfg(any(test, target_os = "linux"))]
pub(super) fn select(modes: &[Mode]) -> Option<usize> {
    order(modes).first().copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_frame_rates_prefer_nearest_below_the_publication_limit() {
        let mut rates = [(60, 1), (15, 1), (120, 1), (30, 1), (24, 1), (30000, 1001)];
        rates.sort_by(|a, b| rate_order(*a, *b));
        assert_eq!(
            rates,
            [(30, 1), (30000, 1001), (24, 1), (15, 1), (60, 1), (120, 1)]
        );
        assert_eq!(rate_order((30, 1), (60, 2)), std::cmp::Ordering::Equal);
    }
    #[test]
    fn a_camera_uses_a_supported_smaller_capture_preset() {
        assert_eq!(choose(|_| true), Some((1280, 720)));
        assert_eq!(choose(|size| size == (640, 480)), Some((640, 480)));
        assert_eq!(choose(|size| size == (352, 288)), Some((352, 288)));
        assert_eq!(choose(|_| false), None);
    }

    fn mode(width: u32, height: u32, rate: Option<(u32, u32)>, compressed: bool) -> Mode {
        Mode {
            width,
            height,
            rate,
            compressed,
        }
    }

    #[test]
    fn the_largest_size_that_reaches_24_fps_is_chosen() {
        // A common USB camera: uncompressed 720p only at 10 frames per second.
        let modes = [
            mode(1280, 720, Some((10, 1)), false),
            mode(640, 480, Some((30, 1)), false),
            mode(352, 288, Some((30, 1)), false),
        ];
        assert_eq!(select(&modes), Some(1));
        let modes = [
            mode(1280, 720, Some((10, 1)), false),
            mode(1280, 720, Some((24, 1)), false),
            mode(640, 480, Some((30, 1)), false),
        ];
        assert_eq!(select(&modes), Some(1));
    }

    #[test]
    fn a_compressed_mode_is_taken_for_its_rate_and_an_uncompressed_one_at_the_same_rate() {
        let modes = [
            mode(1280, 720, Some((10, 1)), false),
            mode(1280, 720, Some((30, 1)), true),
            mode(640, 480, Some((30, 1)), false),
        ];
        assert_eq!(select(&modes), Some(1));
        let modes = [
            mode(1280, 720, Some((30, 1)), true),
            mode(1280, 720, Some((30, 1)), false),
        ];
        assert_eq!(select(&modes), Some(1));
    }

    #[test]
    fn without_a_size_that_reaches_24_fps_the_highest_rate_is_chosen() {
        let modes = [
            mode(1280, 720, Some((5, 1)), false),
            mode(640, 480, Some((15, 2)), false),
            mode(352, 288, Some((15, 1)), false),
        ];
        assert_eq!(select(&modes), Some(2));
        let modes = [
            mode(1280, 720, Some((15, 1)), false),
            mode(640, 480, Some((15, 1)), false),
        ];
        assert_eq!(select(&modes), Some(0));
    }

    #[test]
    fn a_mode_without_a_stated_rate_counts_as_reaching_the_minimum_after_stated_ones() {
        let modes = [
            mode(1280, 720, None, false),
            mode(640, 480, Some((30, 1)), false),
        ];
        assert_eq!(select(&modes), Some(0));
        let modes = [
            mode(1280, 720, None, false),
            mode(1280, 720, Some((30, 1)), false),
        ];
        assert_eq!(select(&modes), Some(1));
    }

    #[test]
    fn every_mode_that_capture_takes_is_ordered_by_the_rule() {
        let modes = [
            mode(1280, 720, Some((30, 1)), true),
            mode(1280, 720, Some((10, 1)), false),
            mode(640, 480, Some((30, 1)), false),
            mode(1920, 1080, Some((30, 1)), false),
            mode(352, 288, Some((30, 1)), false),
            mode(640, 480, Some((15, 1)), false),
            mode(7680, 4320, Some((30, 1)), false),
        ];
        assert_eq!(order(&modes), [0, 3, 2, 4, 5, 1]);
        assert_eq!(select(&modes), Some(0));
    }

    /// A virtual camera offers one size, the size that its own application sets: OBS Studio
    /// its output size, NVIDIA Broadcast the size of its camera setting.
    #[test]
    fn a_camera_that_offers_only_another_size_is_taken_at_that_size() {
        for size in [
            (1920, 1080),
            (1092, 614),
            (1152, 720),
            (2560, 1440),
            (1080, 1920),
        ] {
            assert_eq!(
                select(&[mode(size.0, size.1, Some((60, 1)), false)]),
                Some(0),
                "{size:?}"
            );
        }
        assert_eq!(select(&[mode(1279, 720, Some((30, 1)), false)]), None);
        assert_eq!(select(&[mode(4096, 2304, Some((30, 1)), false)]), None);
        assert_eq!(select(&[mode(5120, 1440, Some((30, 1)), false)]), None);
        assert_eq!(select(&[]), None);
    }

    #[test]
    fn of_two_sizes_with_the_same_published_size_the_smaller_is_chosen() {
        let modes = [
            mode(1920, 1080, Some((30, 1)), false),
            mode(1280, 720, Some((30, 1)), false),
            mode(3840, 2160, Some((30, 1)), false),
        ];
        assert_eq!(order(&modes), [1, 0, 2]);
        // A size between the presets gives a larger published picture than 640 by 480.
        let modes = [
            mode(640, 480, Some((30, 1)), false),
            mode(1092, 614, Some((30, 1)), false),
        ];
        assert_eq!(select(&modes), Some(1));
    }

    #[test]
    fn a_larger_picture_is_published_inside_the_limit_with_its_proportions() {
        assert_eq!(fit((1280, 720)), (1280, 720));
        assert_eq!(fit((640, 480)), (640, 480));
        assert_eq!(fit((1092, 614)), (1092, 614));
        assert_eq!(fit((1920, 1080)), (1280, 720));
        assert_eq!(fit((3840, 2160)), (1280, 720));
        assert_eq!(fit((1440, 1080)), (960, 720));
        assert_eq!(fit((1920, 1200)), (1152, 720));
        assert_eq!(fit((2560, 1080)), (1280, 540));
        assert_eq!(fit((1080, 1920)), (404, 720));
        assert_eq!(fit((4096, 2)), (1280, 2));
    }

    #[test]
    fn every_uncompressed_mode_comes_before_every_compressed_mode_where_a_decoder_is_a_filter() {
        let modes = [
            mode(1280, 720, Some((30, 1)), true),
            mode(1280, 720, Some((10, 1)), false),
            mode(640, 480, Some((30, 1)), false),
            mode(640, 480, Some((30, 1)), true),
            mode(1920, 1080, Some((30, 1)), true),
            mode(7680, 4320, Some((30, 1)), false),
        ];
        assert_eq!(order(&modes), [0, 4, 2, 3, 1]);
        assert_eq!(uncompressed_first(&modes), [2, 1, 0, 4, 3]);
        // A camera that offers only compressed modes keeps the order of the rule.
        let modes = [
            mode(1920, 1080, Some((30, 1)), true),
            mode(1280, 720, Some((30, 1)), true),
            mode(640, 480, Some((30, 1)), true),
        ];
        assert_eq!(uncompressed_first(&modes), [1, 0, 2]);
        assert!(uncompressed_first(&[]).is_empty());
    }

    #[test]
    fn the_offered_types_name_each_type_and_mark_those_that_capture_does_not_read() {
        assert_eq!(
            offered([
                ("NV12 1920x1080 30.00 fps".to_owned(), true),
                ("H264 1920x1080 30.00 fps".to_owned(), false),
            ]),
            "NV12 1920x1080 30.00 fps, H264 1920x1080 30.00 fps (not read)"
        );
        assert_eq!(offered([]), "none");
    }

    #[test]
    fn a_mode_for_the_log_names_its_subtype_size_and_rate() {
        assert_eq!(
            describe("NV12", Some((1920, 1080)), Some((10_000_000, 166_666))),
            "NV12 1920x1080 60.00 fps"
        );
        assert_eq!(describe("MJPG", None, None), "MJPG");
        assert_eq!(
            describe("RGB24", Some((640, 480)), Some((30, 0))),
            "RGB24 640x480"
        );
    }
}
