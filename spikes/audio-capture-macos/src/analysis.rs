use std::f64::consts::PI;

pub const PRESENT_ABOVE_DBFS: f64 = -30.0;
pub const ABSENT_BELOW_DBFS: f64 = -50.0;

pub fn hann_goertzel_amplitude(samples: &[f32], sample_rate: f64, frequency: f64) -> f64 {
    let n = samples.len();
    if n < 2 {
        return 0.0;
    }
    let omega = 2.0 * PI * frequency / sample_rate;
    let coefficient = 2.0 * omega.cos();
    let mut previous = 0.0f64;
    let mut before_previous = 0.0f64;
    let mut window_sum = 0.0f64;
    for (index, sample) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (2.0 * PI * index as f64 / (n - 1) as f64).cos();
        window_sum += window;
        let current = f64::from(*sample) * window + coefficient * previous - before_previous;
        before_previous = previous;
        previous = current;
    }
    let power = previous * previous + before_previous * before_previous
        - coefficient * previous * before_previous;
    2.0 * power.max(0.0).sqrt() / window_sum
}

pub fn rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
    (sum / samples.len() as f64).sqrt()
}

pub fn dbfs(amplitude: f64) -> f64 {
    if amplitude <= 1e-10 {
        -200.0
    } else {
        20.0 * amplitude.log10()
    }
}

pub fn verdict(level_dbfs: f64) -> &'static str {
    if level_dbfs > PRESENT_ABOVE_DBFS {
        "PRESENT"
    } else if level_dbfs < ABSENT_BELOW_DBFS {
        "ABSENT"
    } else {
        "AMBIGUOUS"
    }
}

pub fn first_index_above(samples: &[f32], threshold: f32) -> Option<usize> {
    samples.iter().position(|s| s.abs() > threshold)
}
