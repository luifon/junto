use crate::common::{AnyResult, Options, CHANNELS, RATE};

const FLOOR_DB: f64 = -160.0;

fn decibels(amplitude: f64) -> f64 {
    if amplitude <= 0.0 {
        FLOOR_DB
    } else {
        (20.0 * amplitude.log10()).max(FLOOR_DB)
    }
}

fn hann(index: usize, length: usize) -> f64 {
    0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / length as f64).cos()
}

fn tone_amplitude(samples: &[f64], frequency: f64) -> f64 {
    let length = samples.len();
    if length == 0 {
        return 0.0;
    }
    let omega = std::f64::consts::TAU * frequency / RATE as f64;
    let (mut real, mut imaginary, mut window_sum) = (0.0, 0.0, 0.0);
    for (index, sample) in samples.iter().enumerate() {
        let weight = hann(index, length);
        let angle = omega * index as f64;
        real += sample * weight * angle.cos();
        imaginary -= sample * weight * angle.sin();
        window_sum += weight;
    }
    2.0 * (real * real + imaginary * imaginary).sqrt() / window_sum
}

fn rms(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f64>() / samples.len() as f64).sqrt()
}

fn print_row(label: &str, samples: &[f64], frequencies: &[f64]) {
    let mut row = format!("{label:>13}  rms {:8.1}", decibels(rms(samples)));
    for frequency in frequencies {
        row.push_str(&format!("  {frequency:.0}Hz {:8.1}", decibels(tone_amplitude(samples, *frequency))));
    }
    println!("{row}");
}

pub fn run(options: &Options) -> AnyResult<()> {
    let path = options.required("in")?;
    let window_seconds: f64 = options.number("window", 0.5)?;
    let frequencies: Vec<f64> = options
        .required("freqs")?
        .split(',')
        .map(|text| text.parse::<f64>())
        .collect::<Result<_, _>>()?;

    let bytes = std::fs::read(path)?;
    let mono: Vec<f64> = bytes
        .chunks_exact(4 * CHANNELS)
        .map(|frame| {
            let left = f32::from_le_bytes([frame[0], frame[1], frame[2], frame[3]]) as f64;
            let right = f32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]) as f64;
            (left + right) / 2.0
        })
        .collect();
    let seconds = mono.len() as f64 / RATE as f64;
    println!("file {path}: {} frames, {seconds:.3} s, values in dBFS (amplitude of a full-scale sine = 0)", mono.len());

    if let Some(ranges) = options.text("ranges") {
        for range in ranges.split(',') {
            let (from, to) = range.split_once('-').ok_or("range must be FROM-TO")?;
            let start = ((from.parse::<f64>()? * RATE as f64) as usize).min(mono.len());
            let end = ((to.parse::<f64>()? * RATE as f64) as usize).min(mono.len());
            print_row(&format!("{from}-{to}s"), &mono[start..end], &frequencies);
        }
        return Ok(());
    }

    let window = (window_seconds * RATE as f64) as usize;
    for (index, chunk) in mono.chunks(window).enumerate() {
        if chunk.len() < window / 2 {
            break;
        }
        print_row(&format!("{:.1}s", index as f64 * window_seconds), chunk, &frequencies);
    }
    Ok(())
}
