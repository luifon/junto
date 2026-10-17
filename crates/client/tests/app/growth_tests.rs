//! The rate of growth of `tests/rehearsal/growth.rs`, which the native target `endurance`
//! uses and which runs without the test runner.

use crate::growth::slope;

const MIB: f64 = 1024.0 * 1024.0;

/// A reading every 5 seconds for 9 minutes: 372 MiB, a growth per minute, and a
/// variation of 4 MiB that repeats every 7 readings.
fn readings(per_minute: f64) -> Vec<(f64, f64)> {
    (0..108)
        .map(|number| {
            let minutes = f64::from(number) / 12.0;
            let variation = [0.0, 3.0, -1.0, 2.0, -2.0, 1.0, 4.0][number as usize % 7];
            (minutes, (372.0 + variation + per_minute * minutes) * MIB)
        })
        .collect()
}

#[test]
fn a_steady_reading_has_no_slope_and_a_leak_of_two_mib_a_minute_has_its_own() {
    let steady = slope(&readings(0.0)).unwrap() / MIB;
    assert!(steady.abs() < 0.25, "{steady}");
    let leak = slope(&readings(2.0)).unwrap() / MIB;
    assert!((leak - 2.0).abs() < 0.25, "{leak}");
}

#[test]
fn a_few_readings_far_from_the_others_do_not_move_the_slope() {
    let mut points = readings(0.0);
    for point in points.iter_mut().skip(90).take(6) {
        point.1 += 200.0 * MIB;
    }
    let shown = slope(&points).unwrap() / MIB;
    assert!(shown.abs() < 0.25, "{shown}");
}

#[test]
fn a_count_that_rises_once_has_a_small_slope_and_one_that_rises_at_each_action_a_large_one() {
    let once: Vec<(f64, f64)> = (0..108)
        .map(|number| {
            (
                f64::from(number) / 12.0,
                if number < 54 { 40.0 } else { 41.0 },
            )
        })
        .collect();
    assert!(slope(&once).unwrap() < 0.25);
    // One more thread at each of 84 channel changes in 9 minutes.
    let leak: Vec<(f64, f64)> = (0..108)
        .map(|number| {
            (
                f64::from(number) / 12.0,
                40.0 + f64::from(number * 84 / 108),
            )
        })
        .collect();
    assert!(slope(&leak).unwrap() > 8.0);
}

#[test]
fn points_at_one_time_have_no_slope() {
    assert_eq!(slope(&[]), None);
    assert_eq!(slope(&[(1.0, 2.0), (1.0, 3.0)]), None);
}
