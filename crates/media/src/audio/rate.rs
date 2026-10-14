//! The sample rates of devices that the pipeline converts (ADR 0733).

use super::{FRAME_SAMPLES, SAMPLE_RATE};

/// The most frames of 10 ms in one step of a conversion.
pub const MAX_SPAN_FRAMES: usize = 4;
const LOWEST_RATE: u32 = 8_000;
/// The resampler of the call library takes at most 10 ms at this rate in one step.
const HIGHEST_RATE: u32 = 192_000;

/// One step of the conversion between a device and the pipeline: the smallest number of
/// frames of 10 ms that are a whole number of samples of the device. At 48 kHz and at
/// 44.1 kHz that is one frame; 22.05 kHz needs two, and 11.025 kHz needs four. A step
/// converts exactly, so the time of the device and the time of the pipeline stay equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub frames: usize,
    /// The samples of one channel of the device in the step.
    pub device_samples: usize,
}

fn common_divisor(mut one: u32, mut two: u32) -> u32 {
    while two != 0 {
        (one, two) = (two, one % two);
    }
    one
}

impl Span {
    /// The step for a rate, or nothing for a rate that the pipeline does not convert.
    pub fn of(sample_rate: u32) -> Option<Self> {
        if !(LOWEST_RATE..=HIGHEST_RATE).contains(&sample_rate) {
            return None;
        }
        let frames = 100 / common_divisor(sample_rate, 100);
        let device_samples = u64::from(sample_rate) * u64::from(frames) / 100;
        let fits =
            frames as usize <= MAX_SPAN_FRAMES && device_samples * 100 <= u64::from(HIGHEST_RATE);
        fits.then_some(Self {
            frames: frames as usize,
            device_samples: device_samples as usize,
        })
    }

    /// The samples of one channel of the pipeline in the step.
    pub fn pipeline_samples(self) -> usize {
        FRAME_SAMPLES * self.frames
    }

    /// The resampler converts 10 ms from one rate to another. A step of several frames is
    /// given to it as 10 ms at rates that are that many times higher: the ratio of the two
    /// rates, which is all that the conversion uses, is the same.
    pub fn device_rate(self) -> u32 {
        self.device_samples as u32 * 100
    }

    pub fn pipeline_rate(self) -> u32 {
        SAMPLE_RATE * self.frames as u32
    }
}

/// Whether the pipeline converts the rate of a device.
pub fn converts(sample_rate: u32) -> bool {
    Span::of(sample_rate).is_some()
}

/// The rates that a device is asked for when the pipeline does not convert its default
/// rate, the best first.
pub const PREFERRED_RATES: [u32; 8] = [
    48_000, 44_100, 96_000, 32_000, 88_200, 24_000, 16_000, 192_000,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_is_the_fewest_frames_that_are_whole_samples_of_the_device() {
        let step = |rate| Span::of(rate).map(|span| (span.frames, span.device_samples));
        assert_eq!(step(48_000), Some((1, 480)));
        assert_eq!(step(44_100), Some((1, 441)));
        assert_eq!(step(16_000), Some((1, 160)));
        assert_eq!(step(192_000), Some((1, 1920)));
        assert_eq!(step(22_050), Some((2, 441)));
        assert_eq!(step(11_025), Some((4, 441)));
        // No rate of a real device, and no whole step of at most 40 ms.
        assert_eq!(step(44_101), None);
        // Above what the resampler takes in one step.
        assert_eq!(step(384_000), None);
        assert_eq!(step(96_025), None);
        assert_eq!(step(4_000), None);
        assert_eq!(step(0), None);
        let span = Span::of(11_025).unwrap();
        assert_eq!(
            (span.device_rate(), span.pipeline_rate()),
            (44_100, 192_000)
        );
        assert_eq!(span.pipeline_samples(), 1920);
        assert!(PREFERRED_RATES.iter().all(|rate| converts(*rate)));
    }
}
