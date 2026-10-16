//! Sound devices that exist only in the program: a tone as the microphone and a sink that
//! measures what would be played. The tests of the engine and of the window use them; no
//! test opens a real microphone or plays a sound (ADR 0902).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use super::host::{
    AudioError, AudioHost, Deliver, DeviceId, DeviceInfo, Devices, Failed, Fill, Format, OpenStream,
};

/// What the sink keeps of the sound that was played last.
const KEPT_SECONDS: usize = 12;

/// One synthetic microphone: a sine tone.
#[derive(Debug, Clone, PartialEq)]
pub struct Tone {
    pub hz: f32,
    /// The peak of the tone, from 0 to 1.
    pub amplitude: f32,
    pub format: Format,
}

impl Tone {
    /// A tone as a usual microphone gives it: stereo at 44.1 kHz, so that the conversion
    /// of the pipeline takes part.
    pub fn new(hz: f32) -> Self {
        Self {
            hz,
            amplitude: 0.25,
            format: Format {
                sample_rate: 44_100,
                channels: 2,
            },
        }
    }
}

/// What a microphone gives in place of its tone: speech that a test queued, and a pause of
/// the tone. A stream that is open follows a change at once.
#[derive(Default)]
struct Voice {
    /// Mono samples at the rate of the device, delivered before the tone.
    queued: VecDeque<f32>,
    hushed: bool,
}

struct State {
    inputs: Vec<(DeviceInfo, Tone)>,
    /// The voice of each microphone that has one, by the identifier of the device.
    voices: Vec<(String, Arc<Mutex<Voice>>)>,
    outputs: Vec<DeviceInfo>,
    /// The devices that were opened, in their order: the identifier, or the default.
    opened_inputs: Vec<Option<DeviceId>>,
    opened_outputs: Vec<Option<DeviceId>>,
    /// The next open of an input fails with this.
    input_fault: Option<AudioError>,
    played: VecDeque<f32>,
    played_format: Option<Format>,
    /// Reports a failure of the open input stream, as a disconnected device does.
    input_failed: Option<Failed>,
    output_failed: Option<Failed>,
}

/// A host with a fake device list. Every clone shares one state.
#[derive(Clone)]
pub struct SyntheticHost {
    state: Arc<Mutex<State>>,
}

fn device(id: &str, name: &str, is_default: bool) -> DeviceInfo {
    DeviceInfo {
        id: DeviceId(id.to_owned()),
        name: name.to_owned(),
        is_default,
    }
}

impl SyntheticHost {
    /// One microphone with a tone of `hz`, and one output.
    pub fn with_tone(hz: f32) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                inputs: vec![(device("tone", "Tone microphone", true), Tone::new(hz))],
                outputs: vec![device("sink", "Measuring sink", true)],
                voices: Vec::new(),
                opened_inputs: Vec::new(),
                opened_outputs: Vec::new(),
                input_fault: None,
                played: VecDeque::new(),
                played_format: None,
                input_failed: None,
                output_failed: None,
            })),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds a second microphone with another tone.
    pub fn add_input(&self, id: &str, name: &str, tone: Tone) {
        self.lock().inputs.push((device(id, name, false), tone));
    }

    /// Changes the tone of a microphone. A stream that is opened after this delivers the
    /// new tone, so a test tells it from the sound of the stream before.
    pub fn set_tone(&self, id: &str, hz: f32) {
        for (held, tone) in &mut self.lock().inputs {
            if held.id.0 == id {
                tone.hz = hz;
            }
        }
    }

    /// The voice of a microphone, made at its first use.
    fn voice(state: &mut State, id: &str) -> Arc<Mutex<Voice>> {
        if let Some((_, voice)) = state.voices.iter().find(|(held, _)| held == id) {
            return voice.clone();
        }
        let voice = Arc::new(Mutex::new(Voice::default()));
        state.voices.push((id.to_owned(), voice.clone()));
        voice
    }

    /// The format of a microphone of the list.
    fn input_format(state: &State, id: &str) -> Option<Format> {
        state
            .inputs
            .iter()
            .find(|(held, _)| held.id.0 == id)
            .map(|(_, tone)| tone.format)
    }

    /// Queues mono speech for a microphone. Its stream delivers the speech in place of the
    /// tone, converted to the rate of the device, then the tone again. The sound of a call
    /// can carry spoken words this way, from a file that a test read.
    pub fn say(&self, id: &str, samples: &[f32], rate: u32) {
        let mut state = self.lock();
        let Some(format) = Self::input_format(&state, id) else {
            return;
        };
        let voice = Self::voice(&mut state, id);
        drop(state);
        let converted = convert_rate(samples, rate, format.sample_rate);
        voice
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .queued
            .extend(converted);
    }

    /// Pauses the tone of a microphone, so that it gives silence when no speech is queued,
    /// or lets it sound again.
    pub fn hush(&self, id: &str, hushed: bool) {
        let voice = Self::voice(&mut self.lock(), id);
        voice.lock().unwrap_or_else(PoisonError::into_inner).hushed = hushed;
    }

    /// How long the queued speech of a microphone still lasts.
    pub fn speaking(&self, id: &str) -> Duration {
        let mut state = self.lock();
        let Some(format) = Self::input_format(&state, id) else {
            return Duration::ZERO;
        };
        let voice = Self::voice(&mut state, id);
        drop(state);
        let queued = voice
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .queued
            .len();
        Duration::from_secs_f64(queued as f64 / f64::from(format.sample_rate))
    }

    pub fn add_output(&self, id: &str, name: &str) {
        self.lock().outputs.push(device(id, name, false));
    }

    /// Removes a device from the list, as disconnecting it does.
    pub fn remove_input(&self, id: &str) {
        self.lock().inputs.retain(|(held, _)| held.id.0 != id);
    }

    /// The next open of a microphone fails.
    pub fn fail_next_input(&self, error: AudioError) {
        self.lock().input_fault = Some(error);
    }

    /// The open microphone stops, as a disconnected device does.
    pub fn break_input(&self, error: AudioError) {
        let failed = self.lock().input_failed.take();
        if let Some(mut failed) = failed {
            failed(error);
        }
    }

    /// The open output stops, as a disconnected device does.
    pub fn break_output(&self, error: AudioError) {
        let failed = self.lock().output_failed.take();
        if let Some(mut failed) = failed {
            failed(error);
        }
    }

    pub fn opened_inputs(&self) -> Vec<Option<DeviceId>> {
        self.lock().opened_inputs.clone()
    }

    pub fn opened_outputs(&self) -> Vec<Option<DeviceId>> {
        self.lock().opened_outputs.clone()
    }

    /// Forgets what was played so far.
    pub fn clear_played(&self) {
        self.lock().played.clear();
    }

    /// The left channel of what was played in the last `duration`, with its rate. Empty
    /// when less than that was played since the last clear.
    pub fn played(&self, duration: Duration) -> (Vec<f32>, u32) {
        self.played_of(duration, 0)
    }

    /// The left channel of everything that was played since the last clear, or since the
    /// output was opened, with its rate. The sink keeps the last seconds only.
    pub fn played_since_clear(&self) -> (Vec<f32>, u32) {
        let state = self.lock();
        let Some(format) = state.played_format else {
            return (Vec::new(), 0);
        };
        let channels = usize::from(format.channels);
        let samples = state.played.iter().step_by(channels).copied().collect();
        (samples, format.sample_rate)
    }

    /// One channel of what was played in the last `duration`, with its rate: 0 is the
    /// left channel and 1 the right one.
    pub fn played_of(&self, duration: Duration, channel: usize) -> (Vec<f32>, u32) {
        let state = self.lock();
        let Some(format) = state.played_format else {
            return (Vec::new(), 0);
        };
        let channels = usize::from(format.channels);
        let wanted = (duration.as_secs_f32() * format.sample_rate as f32) as usize * channels;
        if state.played.len() < wanted {
            return (Vec::new(), format.sample_rate);
        }
        let samples = state
            .played
            .iter()
            .skip(state.played.len() - wanted + channel.min(channels - 1))
            .step_by(channels)
            .copied()
            .collect();
        (samples, format.sample_rate)
    }
}

/// A thread that calls `tick` every 10 ms until the stream is dropped.
struct Ticker {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Ticker {
    fn start(mut tick: impl FnMut() + Send + 'static) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = stop.clone();
        let thread = std::thread::spawn(move || {
            let started = Instant::now();
            let mut done = 0u32;
            while !stopped.load(Ordering::Relaxed) {
                // The ticks follow the clock, so a late tick is made up and the rate of the
                // device holds over time.
                let due = (started.elapsed().as_millis() / 10) as u32;
                while done < due {
                    tick();
                    done += 1;
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl AudioHost for SyntheticHost {
    fn devices(&self) -> Result<Devices, AudioError> {
        let state = self.lock();
        Ok(Devices {
            inputs: state.inputs.iter().map(|(info, _)| info.clone()).collect(),
            outputs: state.outputs.clone(),
        })
    }

    fn open_input(
        &self,
        chosen: Option<&DeviceId>,
        mut deliver: Deliver,
        failed: Failed,
    ) -> Result<OpenStream, AudioError> {
        let mut state = self.lock();
        state.opened_inputs.push(chosen.cloned());
        if let Some(fault) = state.input_fault.take() {
            return Err(fault);
        }
        let (info, tone) = state
            .inputs
            .iter()
            .find(|(info, _)| match chosen {
                Some(id) => info.id == *id,
                None => info.is_default,
            })
            .cloned()
            .ok_or(AudioError::NoDevice)?;
        state.input_failed = Some(failed);
        let voice = Self::voice(&mut state, &info.id.0);
        drop(state);
        let rate = u64::from(tone.format.sample_rate);
        let channels = usize::from(tone.format.channels);
        let mut position = 0u64;
        let mut ticks = 0u64;
        let mut buffer: Vec<f32> = Vec::new();
        let ticker = Ticker::start(move || {
            // The samples that the device has made after this tick, so that a rate that
            // 10 ms do not divide holds over time.
            ticks += 1;
            let due = ticks * rate / 100;
            buffer.clear();
            let mut voice = voice.lock().unwrap_or_else(PoisonError::into_inner);
            while position < due {
                let sample = match voice.queued.pop_front() {
                    Some(spoken) => spoken,
                    None if voice.hushed => 0.0,
                    None => {
                        let phase = std::f64::consts::TAU * f64::from(tone.hz) * position as f64
                            / rate as f64;
                        phase.sin() as f32 * tone.amplitude
                    }
                };
                buffer.extend(std::iter::repeat_n(sample, channels));
                position += 1;
            }
            drop(voice);
            deliver(&buffer, tone.format);
        });
        Ok(OpenStream::new(info, ticker))
    }

    fn open_output(
        &self,
        chosen: Option<&DeviceId>,
        mut fill: Fill,
        failed: Failed,
    ) -> Result<OpenStream, AudioError> {
        let mut state = self.lock();
        state.opened_outputs.push(chosen.cloned());
        let info = state
            .outputs
            .iter()
            .find(|info| match chosen {
                Some(id) => info.id == *id,
                None => info.is_default,
            })
            .cloned()
            .ok_or(AudioError::NoDevice)?;
        let format = Format {
            sample_rate: 48_000,
            channels: 2,
        };
        state.played_format = Some(format);
        state.played.clear();
        state.output_failed = Some(failed);
        drop(state);
        let shared = self.state.clone();
        let mut buffer = vec![0.0f32; 480 * 2];
        let kept = KEPT_SECONDS * 48_000 * 2;
        let ticker = Ticker::start(move || {
            buffer.fill(0.0);
            fill(&mut buffer, format);
            let mut state = shared.lock().unwrap_or_else(PoisonError::into_inner);
            state.played.extend(buffer.iter().copied());
            let surplus = state.played.len().saturating_sub(kept);
            state.played.drain(..surplus);
        });
        Ok(OpenStream::new(info, ticker))
    }
}

/// Converts mono samples from one rate to another by linear interpolation. Speech for a
/// synthetic microphone needs no better conversion: the words are what is checked.
fn convert_rate(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() || from == 0 {
        return samples.to_vec();
    }
    let count = (samples.len() as u64 * u64::from(to) / u64::from(from)) as usize;
    let step = f64::from(from) / f64::from(to);
    (0..count)
        .map(|index| {
            let at = index as f64 * step;
            let before = at.floor() as usize;
            let after = (before + 1).min(samples.len() - 1);
            let part = (at - before as f64) as f32;
            samples[before] * (1.0 - part) + samples[after] * part
        })
        .collect()
}

/// The level of one frequency in a block of samples, in dB relative to full scale, with a
/// Hann window (the Goertzel filter).
pub fn tone_level(samples: &[f32], sample_rate: u32, hz: f32) -> f32 {
    let count = samples.len();
    if count < 2 || sample_rate == 0 {
        return -200.0;
    }
    let omega = std::f64::consts::TAU * f64::from(hz) / f64::from(sample_rate);
    let coefficient = 2.0 * omega.cos();
    let (mut one, mut two) = (0.0f64, 0.0f64);
    for (index, sample) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / (count - 1) as f64).cos();
        let next = window * f64::from(*sample) + coefficient * one - two;
        two = one;
        one = next;
    }
    let power = one * one + two * two - coefficient * one * two;
    let amplitude = 4.0 * power.max(0.0).sqrt() / count as f64;
    (20.0 * amplitude.max(1e-10).log10()) as f32
}

/// The root mean square of a block of samples, in dB relative to full scale.
pub fn level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -200.0;
    }
    let mean = samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum::<f64>()
        / samples.len() as f64;
    (10.0 * mean.max(1e-20).log10()) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, amplitude: f32, rate: u32, count: usize) -> Vec<f32> {
        (0..count)
            .map(|index| {
                (std::f32::consts::TAU * hz * index as f32 / rate as f32).sin() * amplitude
            })
            .collect()
    }

    #[test]
    fn the_filter_finds_a_tone_at_its_level_and_no_other_frequency() {
        let tone = sine(660.0, 0.25, 48_000, 9600);
        assert!((tone_level(&tone, 48_000, 660.0) - -12.04).abs() < 0.5);
        assert!(tone_level(&tone, 48_000, 990.0) < -60.0);
        assert!(tone_level(&vec![0.0; 9600], 48_000, 660.0) < -150.0);
        assert!((level(&tone) - -15.05).abs() < 0.2);
    }

    #[test]
    fn the_microphone_delivers_its_tone_and_the_sink_keeps_what_it_was_given() {
        let host = SyntheticHost::with_tone(440.0);
        let heard: Arc<Mutex<Vec<f32>>> = Arc::default();
        let into = heard.clone();
        let input = host
            .open_input(
                None,
                Box::new(move |samples, format| {
                    assert_eq!((format.sample_rate, format.channels), (44_100, 2));
                    let mut heard = into.lock().unwrap();
                    heard.extend(samples.iter().step_by(2));
                }),
                Box::new(|_| {}),
            )
            .unwrap();
        assert_eq!(input.device.name, "Tone microphone");
        let output = host
            .open_output(
                None,
                Box::new(|samples, _| samples.fill(0.5)),
                Box::new(|_| {}),
            )
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        while heard.lock().unwrap().len() < 22_050 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        drop((input, output));
        let heard = heard.lock().unwrap();
        assert!(heard.len() >= 22_050);
        assert!((tone_level(&heard[..22_050], 44_100, 440.0) - -12.04).abs() < 0.5);
        let (played, rate) = host.played(Duration::from_millis(200));
        assert_eq!(rate, 48_000);
        assert_eq!(played.len(), 9600);
        assert!(played.iter().all(|sample| *sample == 0.5));
        assert_eq!(host.opened_inputs(), [None]);
        assert_eq!(host.opened_outputs(), [None]);
    }

    /// The left channel of what one open microphone delivers until `count` samples.
    fn record(host: &SyntheticHost, count: usize, during: impl FnOnce()) -> Vec<f32> {
        let heard: Arc<Mutex<Vec<f32>>> = Arc::default();
        let into = heard.clone();
        let input = host
            .open_input(
                None,
                Box::new(move |samples, _| {
                    into.lock().unwrap().extend(samples.iter().step_by(2));
                }),
                Box::new(|_| {}),
            )
            .unwrap();
        during();
        let deadline = Instant::now() + Duration::from_secs(20);
        while heard.lock().unwrap().len() < count && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(input);
        let heard = heard.lock().unwrap();
        assert!(heard.len() >= count);
        heard[..count].to_vec()
    }

    #[test]
    fn queued_speech_takes_the_place_of_the_tone_and_a_hushed_microphone_is_silent() {
        let host = SyntheticHost::with_tone(440.0);
        // Speech at another rate is converted to the rate of the device. Here a tone of
        // 1000 Hz at 22.05 kHz stands for speech: the microphone delivers it at 44.1 kHz.
        let speech = sine(1000.0, 0.5, 22_050, 22_050);
        host.hush("tone", true);
        let heard = record(&host, 44_100 + 22_050, || {
            host.say("tone", &speech, 22_050);
        });
        assert!(host.speaking("tone") == Duration::ZERO);
        // The speech comes first, whole and at its pitch, and the tone does not sound.
        let (spoken, after) = heard.split_at(44_100);
        assert!((tone_level(&spoken[4_410..39_690], 44_100, 1000.0) - -6.02).abs() < 0.5);
        assert!(tone_level(spoken, 44_100, 440.0) < -40.0);
        assert!(level(after) < -150.0, "a hushed microphone gives silence");

        // Without the pause the tone returns when the speech has ended.
        host.hush("tone", false);
        let heard = record(&host, 22_050, || {});
        assert!((tone_level(&heard, 44_100, 440.0) - -12.04).abs() < 0.5);
    }

    #[test]
    fn a_device_that_is_not_in_the_list_cannot_be_opened() {
        let host = SyntheticHost::with_tone(440.0);
        host.add_input("desk", "Desk microphone", Tone::new(880.0));
        let desk = DeviceId("desk".to_owned());
        assert!(
            host.open_input(Some(&desk), Box::new(|_, _| {}), Box::new(|_| {}))
                .is_ok()
        );
        host.remove_input("desk");
        assert_eq!(
            host.open_input(Some(&desk), Box::new(|_, _| {}), Box::new(|_| {}))
                .unwrap_err(),
            AudioError::NoDevice
        );
        host.fail_next_input(AudioError::Refused);
        assert_eq!(
            host.open_input(None, Box::new(|_, _| {}), Box::new(|_| {}))
                .unwrap_err(),
            AudioError::Refused
        );
        assert_eq!(host.devices().unwrap().inputs.len(), 1);
    }
}
