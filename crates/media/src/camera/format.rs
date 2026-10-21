//! Capture sizes, in order of preference, bounded by the camera publication limit.
#[cfg(any(test, target_os = "macos"))]
pub(super) fn choose(mut accepts: impl FnMut((u32, u32)) -> bool) -> Option<(u32, u32)> {
    [(1280, 720), (640, 480), (352, 288)]
        .into_iter()
        .find(|size| accepts(*size))
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

#[cfg(any(test, target_os = "windows", target_os = "linux"))]
fn reaches_minimum(mode: &Mode) -> bool {
    mode.rate
        .is_none_or(|(n, d)| u64::from(n) >= u64::from(MINIMUM_RATE) * u64::from(d))
}

#[cfg(any(test, target_os = "windows", target_os = "linux"))]
/// The index of the mode to capture with: the largest capture size with a mode of at least 24
/// frames per second; at that size the best rate of `rate_order`, then a stated rate, then an
/// uncompressed mode. When no size reaches 24, the highest rate, then the larger size. A mode
/// without a stated rate counts as reaching 24.
pub(super) fn select(modes: &[Mode]) -> Option<usize> {
    let presets = [(1280, 720), (640, 480), (352, 288)];
    let preset = |mode: &Mode| {
        presets
            .iter()
            .position(|size| *size == (mode.width, mode.height))
    };
    let within_size = |a: &Mode, b: &Mode| {
        b.rate
            .is_none()
            .cmp(&a.rate.is_none())
            .reverse()
            .then_with(|| match (a.rate, b.rate) {
                (Some(a), Some(b)) => rate_order(a, b),
                _ => std::cmp::Ordering::Equal,
            })
            .then_with(|| a.compressed.cmp(&b.compressed))
    };
    let candidates = || (0..modes.len()).filter(|index| preset(&modes[*index]).is_some());
    let size = candidates()
        .filter(|index| reaches_minimum(&modes[*index]))
        .filter_map(|index| preset(&modes[index]))
        .min();
    if let Some(size) = size {
        return candidates()
            .filter(|index| preset(&modes[*index]) == Some(size) && reaches_minimum(&modes[*index]))
            .min_by(|a, b| within_size(&modes[*a], &modes[*b]));
    }
    candidates().min_by(|a, b| {
        let rate = |index: &usize| modes[*index].rate.unwrap_or((0, 1));
        rate_order(rate(a), rate(b))
            .then_with(|| preset(&modes[*a]).cmp(&preset(&modes[*b])))
            .then_with(|| within_size(&modes[*a], &modes[*b]))
    })
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
    fn sizes_outside_the_presets_are_not_chosen() {
        assert_eq!(select(&[mode(1920, 1080, Some((30, 1)), false)]), None);
        assert_eq!(select(&[]), None);
    }
}
