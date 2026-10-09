use std::f64::consts::TAU;
use std::time::Duration;

use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use tokio::time::{interval, Instant, MissedTickBehavior};

use crate::events::unix_ms;

pub const SAMPLE_RATE: u32 = 48_000;
const FRAME_SAMPLES: usize = (SAMPLE_RATE / 100) as usize;

/// A time marker tied to the wall clock: active for `gap_ms` at every Unix-time
/// multiple of `period_ms`. Every participant on one clock marks the same moment.
#[derive(Clone, Copy)]
pub struct Marker {
    pub period_ms: u64,
    pub gap_ms: u64,
}

impl Marker {
    pub fn is_active(&self, unix_ms: u64) -> bool {
        unix_ms % self.period_ms.max(1) < self.gap_ms
    }
}

#[derive(Clone, Copy)]
pub struct ToneSpec {
    pub hz: f64,
    pub level: f64,
    pub marker: Option<Marker>,
}

/// Feeds a sine tone into the source in real time, in 10 ms frames.
pub async fn run_tone(source: NativeAudioSource, spec: ToneSpec) {
    let started = Instant::now();
    let wall_start_us = unix_ms() as f64 * 1000.0;
    let step = TAU * spec.hz / SAMPLE_RATE as f64;
    let peak = spec.level.clamp(0.0, 1.0) * i16::MAX as f64;
    let mut phase = 0.0f64;
    let mut frames_sent: u64 = 0;
    let mut ticker = interval(Duration::from_millis(5));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let frames_due = (started.elapsed().as_micros() / 10_000) as u64;
        while frames_sent < frames_due {
            let mut frame = AudioFrame::new(SAMPLE_RATE, 1, FRAME_SAMPLES as u32);
            let data = frame.data.to_mut();
            for (i, sample) in data.iter_mut().enumerate() {
                let index = frames_sent * FRAME_SAMPLES as u64 + i as u64;
                let wall_us = wall_start_us + index as f64 * 1_000_000.0 / SAMPLE_RATE as f64;
                let audible = !spec.marker.is_some_and(|m| m.is_active((wall_us / 1000.0) as u64));
                *sample = if audible { (phase.sin() * peak) as i16 } else { 0 };
                phase = (phase + step) % TAU;
            }
            if source.capture_frame(&frame).await.is_err() {
                return;
            }
            frames_sent += 1;
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct DetectedTone {
    pub hz: u32,
    pub dbfs: f64,
}

const SWEEP_START_HZ: u32 = 50;
const SWEEP_END_HZ: u32 = 4000;
const SWEEP_STEP_HZ: u32 = 10;
const FLOOR_DBFS: f64 = -50.0;
const BELOW_STRONGEST_DB: f64 = 25.0;

/// Finds the sine tones in a block of mono samples with a Goertzel sweep.
pub fn detect_tones(samples: &[i16], sample_rate: u32) -> Vec<DetectedTone> {
    let n = samples.len();
    if n < 2 {
        return Vec::new();
    }
    let windowed: Vec<f64> = samples
        .iter()
        .enumerate()
        .map(|(i, &s)| {
            let hann = 0.5 - 0.5 * (TAU * i as f64 / (n - 1) as f64).cos();
            hann * s as f64 / i16::MAX as f64
        })
        .collect();
    let levels: Vec<(u32, f64)> = (SWEEP_START_HZ..=SWEEP_END_HZ)
        .step_by(SWEEP_STEP_HZ as usize)
        .map(|hz| (hz, goertzel_dbfs(&windowed, hz as f64, sample_rate)))
        .collect();
    let strongest = levels.iter().map(|&(_, db)| db).fold(f64::MIN, f64::max);
    let mut tones = Vec::new();
    for i in 1..levels.len() - 1 {
        let (hz, db) = levels[i];
        let is_peak = db > levels[i - 1].1 && db >= levels[i + 1].1;
        if is_peak && db > FLOOR_DBFS && db > strongest - BELOW_STRONGEST_DB {
            tones.push(DetectedTone { hz, dbfs: (db * 10.0).round() / 10.0 });
        }
    }
    tones
}

fn goertzel_dbfs(windowed: &[f64], hz: f64, sample_rate: u32) -> f64 {
    let omega = TAU * hz / sample_rate as f64;
    let coeff = 2.0 * omega.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in windowed {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let power = s1 * s1 + s2 * s2 - coeff * s1 * s2;
    let amplitude = 4.0 * power.max(0.0).sqrt() / windowed.len() as f64;
    20.0 * amplitude.max(1e-9).log10()
}

pub fn rms_dbfs(samples: &[i16]) -> f64 {
    if samples.is_empty() {
        return -180.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64 / i16::MAX as f64).powi(2)).sum();
    let rms = (sum / samples.len() as f64).sqrt();
    (20.0 * rms.max(1e-9).log10() * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f64, level: f64, seconds: f64) -> Vec<i16> {
        (0..(seconds * SAMPLE_RATE as f64) as usize)
            .map(|i| ((TAU * hz * i as f64 / SAMPLE_RATE as f64).sin() * level * i16::MAX as f64) as i16)
            .collect()
    }

    #[test]
    fn finds_two_tones_with_their_levels() {
        let a = sine(440.0, 0.25, 0.2);
        let b = sine(1370.0, 0.25, 0.2);
        let mixed: Vec<i16> = a.iter().zip(&b).map(|(x, y)| x + y).collect();
        let tones = detect_tones(&mixed, SAMPLE_RATE);
        let found: Vec<u32> = tones.iter().map(|t| t.hz).collect();
        assert_eq!(found, vec![440, 1370]);
        assert!(tones.iter().all(|t| (t.dbfs - (-12.0)).abs() < 1.0));
    }

    #[test]
    fn silence_has_no_tones() {
        assert!(detect_tones(&vec![0i16; 9600], SAMPLE_RATE).is_empty());
    }
}
