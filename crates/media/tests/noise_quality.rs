//! The measurement of the noise suppressor of the microphone through the capture chain of the
//! client (ADR 0733): the processing module with the echo canceller, the high-pass filter and
//! the gain control, then the suppressor, frame by frame, as `Processor::capture` runs them.
//! It prints figures and checks nothing; it runs only when it is named, with the clips of
//! `tools/noise_clips.py`:
//!
//!   JUNTO_NOISE_CLIPS=<folder> cargo test -p junto-media --release --test noise_quality \
//!       -- --ignored --nocapture
//!
//! Each figure compares the output of a variant with the output of the same chain without a
//! suppressor, so the gain control, which runs before the suppressor, acts on the same sound
//! in both:
//! - noise: the energy left in the frames where the speech is silent, in dB;
//! - speech: the energy of the frames where the speech is active, in dB;
//! - start: the energy of the first 30 ms and of the first 50 ms of each phrase that follows
//!   at least 150 ms of silence, in dB: the median and the worst phrase.
//!
//! The inputs: the speech at its level (-26 dBFS) and 20 dB lower, with the gain control on and
//! off, and the speech of a microphone at 16 kHz and at 8 kHz, converted to 48 kHz by the
//! resampler of the call library as the client converts it.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;

use junto_media::audio::FRAME_SAMPLES;
use junto_media::audio::denoise::Denoiser;
use junto_media::audio::processing::{Processing, Processor};
use livekit::webrtc::native::audio_resampler::AudioResampler;

/// The gate that the client does not run, which the unit tests of the media crate run as well.
#[path = "../src/audio/denoise/gate.rs"]
mod gate;

use gate::{GATE, GateSettings, Gated};

/// The samples of a WAV file of 16-bit mono sound at 48 kHz.
fn read(path: &Path) -> Vec<i16> {
    let bytes = std::fs::read(path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        if id == b"fmt " {
            let channels = u16::from_le_bytes(bytes[at + 10..at + 12].try_into().unwrap());
            let rate = u32::from_le_bytes(bytes[at + 12..at + 16].try_into().unwrap());
            let bits = u16::from_le_bytes(bytes[at + 22..at + 24].try_into().unwrap());
            assert_eq!(
                (channels, rate, bits),
                (1, 48_000, 16),
                "{}",
                path.display()
            );
        }
        if id == b"data" {
            return bytes[at + 8..at + 8 + size]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| i16::from_le_bytes(*pair))
                .collect();
        }
        at += 8 + size + size % 2;
    }
    panic!("{} has no data", path.display());
}

/// The sound of a microphone at `rate`: converted from 48 kHz to that rate and back, each step
/// of 10 ms by the resampler of the call library.
fn through_rate(samples: &[i16], rate: u32) -> Vec<i16> {
    let mut down = AudioResampler::default();
    let mut up = AudioResampler::default();
    let device = rate as usize / 100;
    let mut output = Vec::with_capacity(samples.len());
    for frame in samples.as_chunks::<FRAME_SAMPLES>().0 {
        let low = down
            .remix_and_resample(frame, FRAME_SAMPLES as u32, 1, 48_000, 1, rate)
            .to_vec();
        let mut low = low;
        low.resize(device, 0);
        let high = up.remix_and_resample(&low, device as u32, 1, rate, 1, 48_000);
        let mut high = high.to_vec();
        high.resize(FRAME_SAMPLES, 0);
        output.extend_from_slice(&high);
    }
    output
}

fn scaled(samples: &[i16], db: f64) -> Vec<i16> {
    let gain = 10f64.powf(db / 20.0);
    samples
        .iter()
        .map(|sample| (f64::from(*sample) * gain).round() as i16)
        .collect()
}

/// The chain of the client over a clip: the processing module, then the suppressor when one is
/// given. The output is aligned with the input.
fn chain(
    samples: &[i16],
    automatic_gain: bool,
    suppressor: Option<Option<GateSettings>>,
) -> Vec<f64> {
    let processor = Processor::new(Processing {
        echo_cancellation: true,
        noise_suppression: false,
        automatic_gain,
    });
    let mut denoiser = suppressor.map(|gate| (Denoiser::default(), gate.map(Gated::new)));
    let delay = denoiser.as_ref().map_or(0, |(network, gated)| {
        network.delay() + gated.as_ref().map_or(0, Gated::delay)
    });
    let mut output = Vec::with_capacity(samples.len() + delay);
    for chunk in samples.as_chunks::<FRAME_SAMPLES>().0 {
        let mut frame = *chunk;
        processor.capture(&mut frame);
        if let Some((network, gated)) = &mut denoiser {
            let voice = network.process(&mut frame);
            if let Some(gated) = gated {
                gated.process(&mut frame, voice);
            }
        }
        output.extend(frame.iter().map(|sample| f64::from(*sample) / 32768.0));
    }
    output.drain(..delay.min(output.len()));
    output.resize(samples.len() / FRAME_SAMPLES * FRAME_SAMPLES, 0.0);
    output
}

fn energy(frames: &[&[f64]]) -> f64 {
    frames
        .iter()
        .flat_map(|frame| frame.iter())
        .map(|x| x * x)
        .sum::<f64>()
        .max(1e-20)
}

fn db(ratio: f64) -> f64 {
    10.0 * ratio.log10()
}

/// The frames of the clean speech: active, silent, the first frame of each phrase after at
/// least 150 ms of silence, and the first frame of each word after at least 100 ms below the
/// speech.
struct Marks {
    active: Vec<usize>,
    silent: Vec<usize>,
    starts: Vec<usize>,
    words: Vec<usize>,
}

fn marks(speech: &[i16]) -> Marks {
    let levels: Vec<f64> = speech
        .as_chunks::<FRAME_SAMPLES>()
        .0
        .iter()
        .map(|frame| {
            let mean = frame
                .iter()
                .map(|sample| (f64::from(*sample) / 32768.0).powi(2))
                .sum::<f64>()
                / FRAME_SAMPLES as f64;
            10.0 * mean.max(1e-20).log10()
        })
        .collect();
    let active: Vec<usize> = (0..levels.len()).filter(|i| levels[*i] > -45.0).collect();
    let silent: Vec<usize> = (0..levels.len()).filter(|i| levels[*i] < -60.0).collect();
    let after = |quiet: usize, below: f64| -> Vec<usize> {
        (quiet..levels.len().saturating_sub(5))
            .filter(|i| {
                levels[*i] > -45.0 && levels[i - quiet..*i].iter().all(|level| *level < below)
            })
            .collect()
    };
    Marks {
        active,
        silent,
        starts: after(15, -55.0),
        words: after(10, -50.0),
    }
}

/// The figures of one variant against the chain without a suppressor: (noise, speech, start
/// 30 ms median and worst, start 50 ms median and worst, the same for the starts of words).
fn figures(output: &[f64], reference: &[f64], marks: &Marks) -> (f64, f64, [f64; 8]) {
    let frames = |signal: &'_ [f64]| -> Vec<Vec<f64>> {
        signal
            .as_chunks::<FRAME_SAMPLES>()
            .0
            .iter()
            .map(|frame| frame.to_vec())
            .collect()
    };
    let (out, base) = (frames(output), frames(reference));
    let pick = |all: &[Vec<f64>], which: &[usize]| -> f64 {
        let chosen: Vec<&[f64]> = which.iter().map(|i| all[*i].as_slice()).collect();
        energy(&chosen)
    };
    let noise = db(pick(&out, &marks.silent) / pick(&base, &marks.silent));
    let speech = db(pick(&out, &marks.active) / pick(&base, &marks.active));
    let mut start = [0.0; 8];
    for (slot, length, firsts) in [
        (0, 3, &marks.starts),
        (2, 5, &marks.starts),
        (4, 3, &marks.words),
        (6, 5, &marks.words),
    ] {
        let mut losses: Vec<f64> = firsts
            .iter()
            .map(|first| {
                let which: Vec<usize> = (*first..first + length).collect();
                db(pick(&out, &which) / pick(&base, &which))
            })
            .collect();
        losses.sort_by(f64::total_cmp);
        start[slot] = losses[losses.len() / 2];
        start[slot + 1] = losses[0];
    }
    (noise, speech, start)
}

/// The variants that the measurement compares: the suppressor of the client, which is the
/// network alone; the first gate of the client (a threshold of 0.8, 80 ms of hangover, -25 dB);
/// a gate with a lower threshold, a longer hangover and a smaller attenuation; and the gate
/// with 50 ms of lookahead that keeps the starts of words.
fn variants() -> Vec<(String, Option<GateSettings>)> {
    let gate = |voice: f32, hangover: usize, closed_db: f32, lookahead: usize| GateSettings {
        voice,
        hangover_frames: hangover,
        closed: 10f32.powf(closed_db / 20.0),
        closing: 0.05,
        lookahead_frames: lookahead,
    };
    vec![
        (
            "the network alone, the suppressor of the client".to_owned(),
            None,
        ),
        (
            "the first gate: 0.8, 80 ms, -25 dB".to_owned(),
            Some(gate(0.8, 8, -25.0, 0)),
        ),
        (
            "a gate of 0.3, 200 ms, -12 dB".to_owned(),
            Some(gate(0.3, 20, -12.0, 0)),
        ),
        (
            "the gate of `GATE`: 0.6, 80 ms, -25 dB, 50 ms lookahead".to_owned(),
            Some(GATE),
        ),
    ]
}

#[test]
#[ignore = "a measurement: run it by name with JUNTO_NOISE_CLIPS"]
fn the_suppressor_through_the_capture_chain() {
    let folder = std::env::var_os("JUNTO_NOISE_CLIPS").expect("JUNTO_NOISE_CLIPS names the clips");
    let folder = Path::new(&folder);
    let speech = read(&folder.join("speech.wav"));
    let marks = marks(&speech);
    println!(
        "{} frames, {} active, {} silent, {} phrase starts, {} word starts",
        speech.len() / FRAME_SAMPLES,
        marks.active.len(),
        marks.silent.len(),
        marks.starts.len(),
        marks.words.len()
    );
    let inputs: Vec<(&str, Vec<i16>, bool)> = vec![
        ("speech, gain control on", speech.clone(), true),
        (
            "speech -20 dB, gain control on",
            scaled(&speech, -20.0),
            true,
        ),
        ("speech, gain control off", speech.clone(), false),
        (
            "speech -20 dB, gain control off",
            scaled(&speech, -20.0),
            false,
        ),
        (
            "speech of a 16 kHz microphone",
            through_rate(&speech, 16_000),
            true,
        ),
        (
            "speech of an 8 kHz microphone",
            through_rate(&speech, 8_000),
            true,
        ),
    ];
    let noises: Vec<(String, Vec<i16>)> = ["typing", "clicks", "fan", "voice"]
        .iter()
        .map(|name| {
            (
                name.to_string(),
                read(&folder.join(format!("mix-{name}.wav"))),
            )
        })
        .collect();
    for (name, settings) in variants() {
        println!("\n{name}");
        for (input, samples, gain) in &inputs {
            let reference = chain(samples, *gain, None);
            let output = chain(samples, *gain, Some(settings));
            let (_, level, start) = figures(&output, &reference, &marks);
            println!(
                "  {input:<34} speech {level:>5.1} dB  phrases 30 ms {:>5.1} / {:>5.1}  50 ms {:>5.1} / {:>5.1}  words 30 ms {:>5.1} / {:>5.1}  50 ms {:>5.1} / {:>5.1}",
                start[0], start[1], start[2], start[3], start[4], start[5], start[6], start[7]
            );
        }
        for (noise, samples) in &noises {
            let reference = chain(samples, true, None);
            let output = chain(samples, true, Some(settings));
            let (left, level, start) = figures(&output, &reference, &marks);
            println!(
                "  {noise:<34} noise {left:>5.1} dB  speech {level:>5.1} dB  words 50 ms {:>5.1} / {:>5.1}",
                start[6], start[7]
            );
        }
    }
}
