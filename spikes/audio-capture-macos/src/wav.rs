use std::f64::consts::PI;
use std::fs::File;
use std::io::{BufWriter, Result, Write};
use std::path::Path;

pub fn write_float32(path: &Path, sample_rate: u32, channels: &[Vec<f32>]) -> Result<()> {
    let channel_count = channels.len() as u16;
    let frames = channels.iter().map(Vec::len).min().unwrap_or(0);
    let data_bytes = (frames * channels.len() * 4) as u32;
    let mut out = BufWriter::new(File::create(path)?);
    out.write_all(b"RIFF")?;
    out.write_all(&(36 + data_bytes).to_le_bytes())?;
    out.write_all(b"WAVEfmt ")?;
    out.write_all(&16u32.to_le_bytes())?;
    out.write_all(&3u16.to_le_bytes())?;
    out.write_all(&channel_count.to_le_bytes())?;
    out.write_all(&sample_rate.to_le_bytes())?;
    out.write_all(&(sample_rate * u32::from(channel_count) * 4).to_le_bytes())?;
    out.write_all(&(channel_count * 4).to_le_bytes())?;
    out.write_all(&32u16.to_le_bytes())?;
    out.write_all(b"data")?;
    out.write_all(&data_bytes.to_le_bytes())?;
    for frame in 0..frames {
        for channel in channels {
            out.write_all(&channel[frame].to_le_bytes())?;
        }
    }
    out.flush()
}

pub fn write_sine(
    path: &Path,
    sample_rate: u32,
    frequency: f64,
    amplitude: f64,
    seconds: f64,
) -> Result<()> {
    let frames = (seconds * f64::from(sample_rate)) as usize;
    let mono: Vec<f32> = (0..frames)
        .map(|i| (amplitude * (2.0 * PI * frequency * i as f64 / f64::from(sample_rate)).sin()) as f32)
        .collect();
    write_float32(path, sample_rate, &[mono.clone(), mono])
}
