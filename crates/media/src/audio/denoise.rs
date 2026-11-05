//! The noise suppressor of the microphone (ADR 0733): RNNoise, a recurrent network, through
//! its Rust port `nnnoiseless`, followed by a gate on the probability of voice that the
//! network gives for each frame. The network removes steady noise and lowers short noises;
//! the gate lowers a frame further while the network finds no voice, which takes typing and
//! clicks out of the pauses of speech.

use nnnoiseless::DenoiseState;

use super::FRAME_SAMPLES;

/// The probability of voice above which the gate opens.
const VOICE: f32 = 0.8;
/// How many frames of 10 ms the gate stays open after the last frame with voice.
const HOLD_FRAMES: usize = 8;
/// The gain of the closed gate: -25 dB.
const CLOSED: f32 = 0.056;
/// The part of the way to the target gain that one frame goes: fast to open, slow to close,
/// so that the start of a word is not cut and the end fades.
const OPENING: f32 = 0.5;
const CLOSING: f32 = 0.05;

pub struct Denoiser {
    network: Box<DenoiseState<'static>>,
    input: [f32; FRAME_SAMPLES],
    output: [f32; FRAME_SAMPLES],
    gain: f32,
    hold: usize,
}

impl Default for Denoiser {
    fn default() -> Self {
        Self {
            network: DenoiseState::new(),
            input: [0.0; FRAME_SAMPLES],
            output: [0.0; FRAME_SAMPLES],
            gain: 1.0,
            hold: 0,
        }
    }
}

impl Denoiser {
    /// Processes one mono frame of 10 ms at 48 kHz in place. The network delays the sound
    /// by one frame.
    pub fn process(&mut self, frame: &mut [i16; FRAME_SAMPLES]) {
        for (to, from) in self.input.iter_mut().zip(frame.iter()) {
            *to = f32::from(*from);
        }
        let voice = self.network.process_frame(&mut self.output, &self.input);
        if voice > VOICE {
            self.hold = HOLD_FRAMES;
        } else {
            self.hold = self.hold.saturating_sub(1);
        }
        let target = if self.hold > 0 { 1.0 } else { CLOSED };
        let step = if target > self.gain { OPENING } else { CLOSING };
        let start = self.gain;
        self.gain += (target - self.gain) * step;
        // The gain moves along the frame, so that a change makes no step in the sound.
        let per_sample = (self.gain - start) / FRAME_SAMPLES as f32;
        for (index, (to, from)) in frame.iter_mut().zip(self.output.iter()).enumerate() {
            let gain = start + per_sample * index as f32;
            *to = (from * gain)
                .round()
                .clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
        }
    }
}

#[cfg(test)]
mod tests {
    //! The clips of these tests are made here from a fixed seed: a voice of formant
    //! synthesis with pauses, and the noises of a call that the voice suppressor of the
    //! call library does not remove: typing, clicks of a mouse, and a fan with hum.

    use super::*;

    const RATE: f64 = 48_000.0;

    /// A generator of random numbers for the clips: xorshift.
    struct Random(u64);

    impl Random {
        fn next(&mut self) -> f64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }

        fn normal(&mut self) -> f64 {
            let (u, v) = (self.next().max(1e-12), self.next());
            (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
        }
    }

    /// A two-pole resonator at `frequency` with `bandwidth`, both in Hz.
    fn resonate(signal: &[f64], frequency: f64, bandwidth: f64) -> Vec<f64> {
        let r = (-std::f64::consts::PI * bandwidth / RATE).exp();
        let theta = 2.0 * std::f64::consts::PI * frequency / RATE;
        let (a1, a2) = (-2.0 * r * theta.cos(), r * r);
        let (mut y1, mut y2) = (0.0, 0.0);
        signal
            .iter()
            .map(|x| {
                let y = x - a1 * y1 - a2 * y2;
                y2 = y1;
                y1 = y;
                y
            })
            .collect()
    }

    /// A voice: syllables of a pulse train with a moving pitch through three formants, in
    /// phrases with pauses of 1.2 s, after 1 s of silence.
    fn voice(random: &mut Random, phrases: usize) -> Vec<f64> {
        const VOWELS: [(f64, f64, f64); 5] = [
            (730.0, 1090.0, 2440.0),
            (270.0, 2290.0, 3010.0),
            (300.0, 870.0, 2240.0),
            (530.0, 1840.0, 2480.0),
            (570.0, 840.0, 2410.0),
        ];
        let mut voice = vec![0.0; RATE as usize];
        for _ in 0..phrases {
            for _ in 0..(5 + (random.next() * 4.0) as usize) {
                let length = ((0.15 + random.next() * 0.1) * RATE) as usize;
                let mut phase = 0.0;
                let mut pulses = vec![0.0; length];
                for (index, pulse) in pulses.iter_mut().enumerate() {
                    let along = index as f64 / length as f64;
                    let pitch = 120.0 + 30.0 * (std::f64::consts::PI * along).sin();
                    let before = phase;
                    phase += pitch / RATE;
                    if phase.floor() > before.floor() {
                        *pulse = 1.0;
                    }
                }
                let mut source = vec![0.0; length];
                for (index, pulse) in pulses.iter().enumerate() {
                    if *pulse > 0.0 {
                        for (offset, value) in source[index..].iter_mut().take(200).enumerate() {
                            *value += (-(offset as f64) / 30.0).exp();
                        }
                    }
                }
                let (f1, f2, f3) = VOWELS[(random.next() * 5.0) as usize % 5];
                let parts = [
                    resonate(&source, f1, 80.0 + f1 / 20.0),
                    resonate(&source, f2, 80.0 + f2 / 20.0),
                    resonate(&source, f3, 80.0 + f3 / 20.0),
                ];
                voice.extend((0..length).map(|index| {
                    let envelope = (std::f64::consts::PI * index as f64 / length as f64)
                        .sin()
                        .sqrt();
                    (parts[0][index] + parts[1][index] + parts[2][index]) * envelope
                }));
                let gap = ((0.02 + random.next() * 0.04) * RATE) as usize;
                voice.extend(std::iter::repeat_n(0.0, gap));
            }
            voice.extend(std::iter::repeat_n(0.0, (1.2 * RATE) as usize));
        }
        voice
    }

    /// Short bursts of noise above `low` Hz with a fast attack and a decay of `decay_ms`, at
    /// `per_second` on average: a press and a release for each.
    fn bursts(
        random: &mut Random,
        length: usize,
        per_second: f64,
        decay_ms: f64,
        low: f64,
    ) -> Vec<f64> {
        let mut signal = vec![0.0; length];
        let burst = (decay_ms * 5.0 / 1000.0 * RATE) as usize;
        let mut time = 0.0;
        loop {
            time += -(random.next().max(1e-12)).ln() / per_second;
            let start = (time * RATE) as usize;
            if start + burst + 6000 >= length {
                return signal;
            }
            let release = start + ((0.06 + random.next() * 0.06) * RATE) as usize;
            let level = 0.6 + random.next() * 0.4;
            for at in [start, release] {
                // A first-order high-pass of white noise, decaying.
                let alpha = 1.0 / (1.0 + 2.0 * std::f64::consts::PI * low / RATE);
                let (mut previous_in, mut previous_out) = (0.0, 0.0);
                for index in 0..burst {
                    let white = random.normal();
                    let high = alpha * (previous_out + white - previous_in);
                    previous_in = white;
                    previous_out = high;
                    let envelope = (-(index as f64) / (decay_ms / 1000.0 * RATE)).exp();
                    signal[at + index] += high * envelope * level;
                }
            }
        }
    }

    /// A fan: low-passed noise and a hum of 100 Hz with two harmonics.
    fn fan(random: &mut Random, length: usize) -> Vec<f64> {
        let mut low = 0.0;
        (0..length)
            .map(|index| {
                low += (random.normal() - low) * 0.05;
                let t = index as f64 / RATE;
                let hum = (1..=3)
                    .map(|k| (2.0 * std::f64::consts::PI * 100.0 * k as f64 * t).sin() / k as f64)
                    .sum::<f64>();
                low * 4.0 + hum * 0.3
            })
            .collect()
    }

    fn rms(signal: &[f64]) -> f64 {
        (signal.iter().map(|x| x * x).sum::<f64>() / signal.len().max(1) as f64).sqrt()
    }

    /// The voice at -26 dBFS over its active part, the noise `snr_db` below it.
    fn mixed(voice: &[f64], noise: &[f64], snr_db: f64) -> (Vec<f64>, Vec<f64>) {
        let active: Vec<f64> = voice.iter().copied().filter(|x| x.abs() > 1e-6).collect();
        let voice_gain = 10f64.powf(-26.0 / 20.0) / rms(&active);
        let voice: Vec<f64> = voice.iter().map(|x| x * voice_gain).collect();
        let noise_gain = rms(&active) * voice_gain / rms(noise) * 10f64.powf(-snr_db / 20.0);
        let mix = voice
            .iter()
            .zip(noise)
            .map(|(v, n)| v + n * noise_gain)
            .collect();
        (voice, mix)
    }

    /// Runs a clip through the suppressor, frame by frame, and gives the output one frame
    /// earlier, to align it with the input.
    fn suppressed(clip: &[f64]) -> Vec<f64> {
        let mut denoiser = Denoiser::default();
        let mut output = Vec::with_capacity(clip.len());
        for chunk in clip.as_chunks::<FRAME_SAMPLES>().0.iter() {
            let mut frame = [0i16; FRAME_SAMPLES];
            for (to, from) in frame.iter_mut().zip(chunk) {
                *to = (from * 32768.0).clamp(-32768.0, 32767.0) as i16;
            }
            denoiser.process(&mut frame);
            output.extend(frame.iter().map(|sample| f64::from(*sample) / 32768.0));
        }
        output.drain(..FRAME_SAMPLES);
        output
    }

    /// The level of each frame of 10 ms in dBFS.
    fn levels(signal: &[f64]) -> Vec<f64> {
        signal
            .as_chunks::<FRAME_SAMPLES>()
            .0
            .iter()
            .map(|frame| 20.0 * rms(frame).max(1e-10).log10())
            .collect()
    }

    /// The scores of a clip: the noise left in the frames where the voice is silent, in dB
    /// relative to the input there; the level of the voice frames relative to the clean
    /// voice, in dB; and the correlation of the output with the clean voice there.
    fn scores(voice: &[f64], mix: &[f64]) -> (f64, f64, f64) {
        let output = suppressed(mix);
        let frames = levels(voice);
        let pick = |signal: &[f64], keep: &dyn Fn(f64) -> bool| -> Vec<f64> {
            signal
                .as_chunks::<FRAME_SAMPLES>()
                .0
                .iter()
                .zip(&frames)
                .filter(|(_, level)| keep(**level))
                .flat_map(|(frame, _)| frame.iter().copied())
                .collect()
        };
        let silent = |level: f64| level < -60.0;
        let active = |level: f64| level > -45.0;
        let left = 20.0 * (rms(&pick(&output, &silent)) / rms(&pick(mix, &silent))).log10();
        let (out_voice, clean) = (pick(&output, &active), pick(voice, &active));
        let level = 20.0 * (rms(&out_voice) / rms(&clean)).log10();
        let dot: f64 = out_voice.iter().zip(&clean).map(|(a, b)| a * b).sum();
        let correlation = dot / (rms(&out_voice) * rms(&clean) * clean.len() as f64);
        (left, level, correlation)
    }

    #[test]
    fn the_noises_of_a_call_fall_and_the_voice_stays() {
        let mut random = Random(0x5eed_1234_abcd_0001);
        let voice = voice(&mut random, 3);
        let length = voice.len();
        let clips = [
            (
                "typing",
                bursts(&mut random, length, 7.0, 4.0, 800.0),
                10.0,
                -6.0,
            ),
            (
                "clicks",
                bursts(&mut random, length, 1.5, 1.5, 200.0),
                15.0,
                -6.0,
            ),
            ("fan", fan(&mut random, length), 10.0, -15.0),
        ];
        for (name, noise, snr_db, at_most) in clips {
            let (clean, mix) = mixed(&voice, &noise, snr_db);
            let (left, level, correlation) = scores(&clean, &mix);
            assert!(
                left <= at_most,
                "{name}: the noise fell by {left:.1} dB only"
            );
            assert!(
                level.abs() <= 3.0,
                "{name}: the voice changed by {level:.1} dB"
            );
            assert!(
                correlation >= 0.8,
                "{name}: the voice kept a correlation of {correlation:.2}"
            );
        }
        // The voice alone keeps its level.
        let (clean, _) = mixed(&voice, &vec![1e-9; length], 60.0);
        let (_, level, correlation) = scores(&clean, &clean);
        assert!(level.abs() <= 2.0, "the voice changed by {level:.1} dB");
        assert!(
            correlation >= 0.85,
            "the voice kept a correlation of {correlation:.2}"
        );
    }
}
