use std::f64::consts::TAU;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use livekit::webrtc::audio_frame::AudioFrame;
use livekit::webrtc::audio_source::native::NativeAudioSource;
use serde_json::json;
use tokio::sync::watch;
use tokio::time::{interval, Instant, MissedTickBehavior};

use crate::events::{emit, unix_ms};

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

/// The samples of a WAV file at the rate of the track, and the signal that starts them.
#[derive(Clone)]
pub struct FileAudio {
    pub samples: Arc<Vec<i16>>,
    pub play: watch::Receiver<bool>,
}

/// Reads a WAV file with 16-bit PCM and one channel and converts it to the rate of the
/// track by linear interpolation.
pub fn read_wav(path: &Path) -> Result<Arc<Vec<i16>>> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("{} is not a WAV file", path.display());
    }
    let mut format: Option<(u16, u16, u32, u16)> = None;
    let mut data: Option<&[u8]> = None;
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into()?) as usize;
        let end = (at + 8 + size).min(bytes.len());
        let body = &bytes[at + 8..end];
        match &bytes[at..at + 4] {
            b"fmt " if body.len() >= 16 => {
                format = Some((
                    u16::from_le_bytes(body[0..2].try_into()?),
                    u16::from_le_bytes(body[2..4].try_into()?),
                    u32::from_le_bytes(body[4..8].try_into()?),
                    u16::from_le_bytes(body[14..16].try_into()?),
                ));
            }
            b"data" => data = Some(body),
            _ => {}
        }
        at = end + size % 2;
    }
    let (encoding, channels, rate, bits) = format.ok_or_else(|| anyhow!("the WAV file has no format"))?;
    if encoding != 1 || channels != 1 || bits != 16 || rate == 0 {
        bail!("the WAV file must be 16-bit PCM with one channel");
    }
    let source: Vec<i16> = data
        .ok_or_else(|| anyhow!("the WAV file has no data"))?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| i16::from_le_bytes(*pair))
        .collect();
    if source.is_empty() {
        bail!("the WAV file has no samples");
    }
    let length = (source.len() as u64 * SAMPLE_RATE as u64 / rate as u64) as usize;
    let samples = (0..length)
        .map(|i| {
            let position = i as f64 * rate as f64 / SAMPLE_RATE as f64;
            let first = (position as usize).min(source.len() - 1);
            let second = (first + 1).min(source.len() - 1);
            let part = position - first as f64;
            (source[first] as f64 * (1.0 - part) + source[second] as f64 * part) as i16
        })
        .collect();
    Ok(Arc::new(samples))
}

/// Feeds sound into the source in real time, in 10 ms frames: the sine tone, and from the
/// start signal on the samples of the file, followed by silence. A tone of 0 Hz is silence.
pub async fn run_tone(source: NativeAudioSource, name: String, spec: ToneSpec, file: Option<FileAudio>) {
    let started = Instant::now();
    let wall_start_us = unix_ms() as f64 * 1000.0;
    let step = TAU * spec.hz / SAMPLE_RATE as f64;
    let peak = if spec.hz > 0.0 { spec.level.clamp(0.0, 1.0) * i16::MAX as f64 } else { 0.0 };
    let mut phase = 0.0f64;
    let mut frames_sent: u64 = 0;
    let mut file_position: Option<usize> = None;
    let mut file_ended = false;
    let mut ticker = interval(Duration::from_millis(5));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let frames_due = (started.elapsed().as_micros() / 10_000) as u64;
        while frames_sent < frames_due {
            if let Some(file) = &file {
                if file_position.is_none() && *file.play.borrow() {
                    file_position = Some(0);
                    let seconds = file.samples.len() as f64 / SAMPLE_RATE as f64;
                    emit("file_started", json!({ "name": name, "seconds": seconds }));
                }
            }
            let mut frame = AudioFrame::new(SAMPLE_RATE, 1, FRAME_SAMPLES as u32);
            let data = frame.data.to_mut();
            if let (Some(file), Some(position)) = (&file, file_position.as_mut()) {
                for sample in data.iter_mut() {
                    *sample = file.samples.get(*position).copied().unwrap_or(0);
                    *position += 1;
                }
                if !file_ended && *position >= file.samples.len() {
                    file_ended = true;
                    emit("file_ended", json!({ "name": name }));
                }
            } else {
                for (i, sample) in data.iter_mut().enumerate() {
                    let index = frames_sent * FRAME_SAMPLES as u64 + i as u64;
                    let wall_us = wall_start_us + index as f64 * 1_000_000.0 / SAMPLE_RATE as f64;
                    let audible = !spec.marker.is_some_and(|m| m.is_active((wall_us / 1000.0) as u64));
                    *sample = if audible { (phase.sin() * peak) as i16 } else { 0 };
                    phase = (phase + step) % TAU;
                }
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
    fn a_wav_file_is_read_at_the_rate_of_the_track() {
        let source: Vec<i16> = (0..1600).map(|i| (i * 10) as i16).collect();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + source.len() as u32 * 2).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&16_000u32.to_le_bytes());
        bytes.extend_from_slice(&32_000u32.to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&(source.len() as u32 * 2).to_le_bytes());
        for sample in &source {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let path = std::env::temp_dir().join(format!("test-participant-{}.wav", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let samples = read_wav(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(samples.len(), 4800);
        assert_eq!(samples[0], 0);
        assert_eq!(samples[3], 10);
        assert_eq!(samples[4], 13);

        std::fs::write(&path, &bytes[..40]).unwrap();
        assert!(read_wav(&path).is_err());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn silence_has_no_tones() {
        assert!(detect_tones(&vec![0i16; 9600], SAMPLE_RATE).is_empty());
    }
}
