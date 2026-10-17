//! The rate at which a reading of the process grows over a long use (ADR 0902).

/// The slope of `points`, given as (minutes, value), in value per minute: the median of the
/// slopes between every two points at different times (the Theil–Sen estimate). Fewer than
/// half of the points far from the others, such as the readings around one collection of
/// an allocator, do not move it. `None` when no two points are at different times.
pub fn slope(points: &[(f64, f64)]) -> Option<f64> {
    let mut slopes = Vec::with_capacity(points.len() * points.len().saturating_sub(1) / 2);
    for (index, (at, value)) in points.iter().enumerate() {
        for (later_at, later_value) in points.iter().skip(index + 1) {
            let time = later_at - at;
            if time != 0.0 {
                slopes.push((later_value - value) / time);
            }
        }
    }
    if slopes.is_empty() {
        return None;
    }
    slopes.sort_unstable_by(f64::total_cmp);
    let middle = slopes.len() / 2;
    Some(if slopes.len() % 2 == 0 {
        (slopes[middle - 1] + slopes[middle]) / 2.0
    } else {
        slopes[middle]
    })
}
