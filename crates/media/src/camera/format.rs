//! Capture sizes, in order of preference, bounded by the camera publication limit.
pub(super) fn choose(mut accepts: impl FnMut((u32, u32)) -> bool) -> Option<(u32, u32)> {
    [(1280, 720), (640, 480), (352, 288)]
        .into_iter()
        .find(|size| accepts(*size))
}

#[cfg(any(test, target_os = "windows"))]
pub(super) fn rate_order(a: (u32, u32), b: (u32, u32)) -> std::cmp::Ordering {
    let above = |(n, d): (u32, u32)| u64::from(n) > 30 * u64::from(d);
    above(a).cmp(&above(b)).then_with(|| {
        let order = (u64::from(a.0) * u64::from(b.1)).cmp(&(u64::from(b.0) * u64::from(a.1)));
        if above(a) { order } else { order.reverse() }
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
}
