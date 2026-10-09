use crate::common::{AnyResult, Options, Sine, CHANNELS, RATE};
use std::io::Write;

pub fn run(options: &Options) -> AnyResult<()> {
    let frequency: f64 = options.number("freq", 440.0)?;
    let seconds: f64 = options.number("seconds", 5.0)?;
    let amplitude: f32 = options.number("amp", 0.2)?;
    let path = options.required("out")?;

    let frames = (seconds * RATE as f64) as u32;
    let data_bytes = frames * CHANNELS as u32 * 2;
    let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
    file.write_all(b"RIFF")?;
    file.write_all(&(36 + data_bytes).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&(CHANNELS as u16).to_le_bytes())?;
    file.write_all(&RATE.to_le_bytes())?;
    file.write_all(&(RATE * CHANNELS as u32 * 2).to_le_bytes())?;
    file.write_all(&(CHANNELS as u16 * 2).to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_bytes.to_le_bytes())?;
    let mut sine = Sine::new(frequency, amplitude);
    for _ in 0..frames {
        let sample = (sine.next_sample() * 32767.0) as i16;
        for _ in 0..CHANNELS {
            file.write_all(&sample.to_le_bytes())?;
        }
    }
    file.flush()?;
    println!("wrote {path}: {frequency} Hz, {seconds} s, amplitude {amplitude}");
    Ok(())
}
