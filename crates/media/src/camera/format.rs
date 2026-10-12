//! Capture sizes, in order of preference, bounded by the camera publication limit.
pub(super) fn choose(mut accepts: impl FnMut((u32, u32)) -> bool) -> Option<(u32, u32)> {
    [(1280, 720), (640, 480), (352, 288)]
        .into_iter()
        .find(|size| accepts(*size))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_camera_uses_a_supported_smaller_capture_preset() {
        assert_eq!(choose(|_| true), Some((1280, 720)));
        assert_eq!(choose(|size| size == (640, 480)), Some((640, 480)));
        assert_eq!(choose(|size| size == (352, 288)), Some((352, 288)));
        assert_eq!(choose(|_| false), None);
    }
}
