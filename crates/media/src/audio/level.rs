//! The level of sound: what a meter shows, and whether a person speaks (ADR 0507,
//! ADR 0513).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// The level of digital silence, and the lowest level that a meter tells apart.
pub const SILENCE_DBFS: f32 = -96.0;
/// A meter shows nothing at this level and below.
const METER_FLOOR_DBFS: f32 = -60.0;
/// A meter is full at this level and above. Speech at a usual distance from a microphone
/// has about -30 to -15 dBFS.
const METER_CEILING_DBFS: f32 = -12.0;
/// A level falls by this much with each frame of 10 ms, so that a meter does not flicker
/// between the syllables of a word.
const RELEASE_DB_PER_FRAME: f32 = 0.6;
/// A person speaks while the level of the microphone is above this.
const SPEAKING_DBFS: f32 = -45.0;
/// A person still counts as speaking for this many frames after the level fell: the
/// pauses inside a sentence do not end the mark.
const SPEAKING_HOLD_FRAMES: u32 = 40;

/// The root mean square of a frame in dB relative to full scale.
pub fn dbfs(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return SILENCE_DBFS;
    }
    let sum: f64 = samples
        .iter()
        .map(|sample| {
            let value = f64::from(*sample) / f64::from(i16::MAX);
            value * value
        })
        .sum();
    let mean = sum / samples.len() as f64;
    if mean <= 0.0 {
        return SILENCE_DBFS;
    }
    ((10.0 * mean.log10()) as f32).max(SILENCE_DBFS)
}

/// The share of a meter that a level lights, from 0 to 1.
pub fn meter(level_dbfs: f32) -> f32 {
    ((level_dbfs - METER_FLOOR_DBFS) / (METER_CEILING_DBFS - METER_FLOOR_DBFS)).clamp(0.0, 1.0)
}

/// The lit segments of a meter with `segments` segments. A sound above the floor of the
/// meter lights at least one.
pub fn segments(value: f32, segments: usize) -> usize {
    if value <= 0.0 {
        return 0;
    }
    ((value * segments as f32).ceil() as usize).clamp(1, segments)
}

/// The level of one sound source, written by the thread that carries its sound and read
/// by the engine. A rise is taken at once and a fall is slowed.
#[derive(Debug)]
pub struct LevelCell {
    bits: AtomicU32,
}

impl Default for LevelCell {
    fn default() -> Self {
        Self {
            bits: AtomicU32::new(SILENCE_DBFS.to_bits()),
        }
    }
}

impl LevelCell {
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Takes the level of the next frame.
    pub fn frame(&self, samples: &[i16]) -> f32 {
        let now = dbfs(samples);
        let held = self.read();
        let next = now.max(held - RELEASE_DB_PER_FRAME).max(SILENCE_DBFS);
        self.bits.store(next.to_bits(), Ordering::Relaxed);
        next
    }

    pub fn read(&self) -> f32 {
        f32::from_bits(self.bits.load(Ordering::Relaxed))
    }

    pub fn silence(&self) {
        self.bits.store(SILENCE_DBFS.to_bits(), Ordering::Relaxed);
    }
}

/// Decides from the frames of a microphone whether its person speaks.
#[derive(Debug, Default)]
pub struct Speaking {
    held: u32,
}

impl Speaking {
    /// Takes the level of the next frame that is sent and returns the decision.
    pub fn frame(&mut self, level_dbfs: f32) -> bool {
        if level_dbfs > SPEAKING_DBFS {
            self.held = SPEAKING_HOLD_FRAMES;
        } else {
            self.held = self.held.saturating_sub(1);
        }
        self.held > 0
    }

    pub fn reset(&mut self) {
        self.held = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amplitude: f32) -> Vec<i16> {
        (0..480)
            .map(|index| {
                let phase = std::f32::consts::TAU * 440.0 * index as f32 / 48_000.0;
                (phase.sin() * amplitude * f32::from(i16::MAX)) as i16
            })
            .collect()
    }

    #[test]
    fn the_level_of_a_tone_follows_its_amplitude_and_silence_is_the_floor() {
        // A sine of amplitude a has the root mean square a / sqrt(2).
        let full = dbfs(&tone(1.0));
        assert!((full - -3.01).abs() < 0.1, "{full}");
        let quarter = dbfs(&tone(0.25));
        assert!((quarter - -15.05).abs() < 0.1, "{quarter}");
        assert_eq!(dbfs(&[0; 480]), SILENCE_DBFS);
        assert_eq!(dbfs(&[]), SILENCE_DBFS);
    }

    #[test]
    fn a_meter_is_empty_below_its_floor_and_full_above_its_ceiling() {
        assert_eq!(meter(SILENCE_DBFS), 0.0);
        assert_eq!(meter(-60.0), 0.0);
        assert_eq!(meter(-12.0), 1.0);
        assert_eq!(meter(0.0), 1.0);
        assert!((meter(-36.0) - 0.5).abs() < 0.001);
        assert_eq!(segments(0.0, 5), 0);
        assert_eq!(segments(0.01, 5), 1);
        assert_eq!(segments(0.5, 5), 3);
        assert_eq!(segments(1.0, 5), 5);
        assert_eq!(segments(0.26, 20), 6);
    }

    #[test]
    fn a_level_rises_at_once_and_falls_slowly() {
        let cell = LevelCell::default();
        assert_eq!(cell.read(), SILENCE_DBFS);
        let loud = cell.frame(&tone(0.5));
        assert!(loud > -10.0);
        let after_one = cell.frame(&[0; 480]);
        assert!((loud - after_one - RELEASE_DB_PER_FRAME).abs() < 0.001);
        for _ in 0..400 {
            cell.frame(&[0; 480]);
        }
        assert_eq!(cell.read(), SILENCE_DBFS);
        cell.frame(&tone(0.5));
        cell.silence();
        assert_eq!(cell.read(), SILENCE_DBFS);
    }

    #[test]
    fn a_person_speaks_above_the_threshold_and_for_a_hold_after_it() {
        let mut speaking = Speaking::default();
        assert!(!speaking.frame(SILENCE_DBFS));
        assert!(!speaking.frame(-50.0));
        assert!(speaking.frame(-30.0));
        for _ in 0..SPEAKING_HOLD_FRAMES - 1 {
            assert!(speaking.frame(SILENCE_DBFS));
        }
        assert!(!speaking.frame(SILENCE_DBFS));
        assert!(speaking.frame(-30.0));
        speaking.reset();
        assert!(!speaking.frame(SILENCE_DBFS));
    }
}
