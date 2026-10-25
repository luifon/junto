//! The short tones of the events of a call (ADR 0707): plain sines at a low level, with a
//! rise and a fall of 10 ms, so that the device does not click.
//!
//! A sound opens the output device on the thread of the player, plays once, and closes
//! the device, as the tone of an incoming call does (ADR 0733). A request only sends the
//! sound to that thread and returns at once.

use super::{AudioHost, DeviceId, Format};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    /// Another person entered the call of this device.
    PersonJoined,
    /// Another person left the call of this device.
    PersonLeft,
    MicrophoneOff,
    MicrophoneOn,
    SoundOff,
    SoundOn,
    /// A share started in the call of this device.
    ShareStarted,
    /// The person of this device is no longer in the call.
    CallLeft,
    /// An invitation into a voice channel arrived.
    Invitation,
}

impl Sound {
    pub const ALL: [Sound; 9] = [
        Sound::PersonJoined,
        Sound::PersonLeft,
        Sound::MicrophoneOff,
        Sound::MicrophoneOn,
        Sound::SoundOff,
        Sound::SoundOn,
        Sound::ShareStarted,
        Sound::CallLeft,
        Sound::Invitation,
    ];

    /// The tones and pauses of the sound, in their order.
    pub fn parts(self) -> &'static [Part] {
        use Part::{Pause, Tone};
        match self {
            Sound::PersonJoined => &[Tone(392, 70), Pause(20), Tone(523, 70)],
            Sound::PersonLeft => &[Tone(523, 70), Pause(20), Tone(392, 70)],
            Sound::MicrophoneOff => &[Tone(392, 80)],
            Sound::MicrophoneOn => &[Tone(523, 80)],
            Sound::SoundOff => &[Tone(370, 150)],
            Sound::SoundOn => &[Tone(494, 150)],
            Sound::ShareStarted => &[Tone(440, 60), Pause(40), Tone(440, 60)],
            Sound::CallLeft => &[Tone(330, 200)],
            Sound::Invitation => &[Tone(587, 120)],
        }
    }

    pub fn length(self) -> Duration {
        Duration::from_millis(
            self.parts()
                .iter()
                .map(|part| u64::from(part.millis()))
                .sum(),
        )
    }
}

/// A part of a sound: a sine of a frequency in Hz for a number of milliseconds, or silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Tone(u32, u32),
    Pause(u32),
}

impl Part {
    fn millis(self) -> u32 {
        match self {
            Part::Tone(_, millis) | Part::Pause(millis) => millis,
        }
    }
}

/// The peak of every sound, as a part of full scale: about 30 dB below full scale, and
/// 8.5 dB below the tone of an incoming call (0.08).
pub const LEVEL: f32 = 0.03;
/// The time in which a tone rises from silence to its level, and falls back at its end.
const FADE: f64 = 0.010;
/// The silence that follows a sound before the device is closed, so that the device has
/// played the last samples that it was given.
const TAIL: f64 = 0.150;
/// How long the player waits for the end of a sound beyond its length, for a device that
/// takes no samples.
const WAIT: Duration = Duration::from_secs(2);
/// The most sounds that the player plays of the requests that arrived while it played.
const MOST_WAITING: usize = 4;

/// The sample of a sound at a time from its start, or `None` after its end.
fn sample_at(parts: &[Part], at: f64) -> Option<f32> {
    let mut start = 0.0;
    for part in parts {
        let end = start + f64::from(part.millis()) / 1000.0;
        if at < end {
            let Part::Tone(hz, _) = *part else {
                return Some(0.0);
            };
            let into = at - start;
            let edge = into.min(end - at);
            let gain = if edge < FADE {
                0.5 - 0.5 * (std::f64::consts::PI * edge / FADE).cos()
            } else {
                1.0
            };
            let wave = (into * f64::from(hz) * std::f64::consts::TAU).sin();
            return Some((wave * gain) as f32 * LEVEL);
        }
        start = end;
    }
    None
}

/// The samples of one channel of a sound at a rate.
pub fn render(sound: Sound, sample_rate: u32) -> Vec<f32> {
    let step = 1.0 / f64::from(sample_rate.max(1));
    let mut samples = Vec::new();
    let mut at = 0.0;
    while let Some(sample) = sample_at(sound.parts(), at) {
        samples.push(sample);
        at += step;
    }
    samples
}

/// A sound, its device, and the generation of the player when it was asked for.
type Request = (Sound, Option<DeviceId>, u64);

pub struct SoundPlayer {
    thread: Option<Sender<Request>>,
    /// Raised by [`SoundPlayer::cancel`]: a request of an earlier generation is not played,
    /// and a sound that plays goes silent.
    generation: Arc<AtomicU64>,
}

impl SoundPlayer {
    pub fn new(host: Arc<dyn AudioHost>) -> Self {
        let (requests, received) = channel();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        let thread = std::thread::Builder::new()
            .name("junto-sounds".to_owned())
            .spawn(move || play_requests(host.as_ref(), &received, &current));
        let thread = match thread {
            Ok(_) => Some(requests),
            Err(error) => {
                tracing::warn!(%error, "the sounds of a call have no thread");
                None
            }
        };
        Self { thread, generation }
    }

    /// Plays a sound once on a device, or on the default device of the system. It returns
    /// at once; the sound follows the sounds that were asked for before it.
    pub fn play(&self, sound: Sound, device: Option<&DeviceId>) {
        if let Some(thread) = &self.thread {
            let generation = self.generation.load(Ordering::Acquire);
            let _ = thread.send((sound, device.cloned(), generation));
        }
    }

    /// Drops the sounds that wait and silences the one that plays: the person turned the
    /// sounds off.
    pub fn cancel(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

/// The thread of the player. It ends when the player is dropped.
fn play_requests(host: &dyn AudioHost, requests: &Receiver<Request>, current: &Arc<AtomicU64>) {
    while let Ok(first) = requests.recv() {
        let mut waiting = vec![first];
        while let Ok(next) = requests.try_recv() {
            if !waiting.iter().any(|(sound, _, _)| *sound == next.0) {
                waiting.push(next);
            }
        }
        for (sound, device, generation) in waiting.into_iter().take(MOST_WAITING) {
            if generation == current.load(Ordering::Acquire) {
                play_once(host, sound, device.as_ref(), current.clone(), generation);
            }
        }
    }
}

fn play_once(
    host: &dyn AudioHost,
    sound: Sound,
    device: Option<&DeviceId>,
    current: Arc<AtomicU64>,
    generation: u64,
) {
    let (finished, ended) = sync_channel(1);
    match host.open_output(
        device,
        Box::new(fill(sound, finished, current, generation)),
        Box::new(|error| tracing::debug!(%error, "a call sound stopped")),
    ) {
        Ok(stream) => {
            let _ = ended.recv_timeout(sound.length() + WAIT);
            drop(stream);
        }
        Err(error) => tracing::debug!(%error, "a call sound could not start"),
    }
}

/// The samples of the device: the sound, then silence. It tells when the sound and the
/// tail after it have been given to the device.
fn fill(
    sound: Sound,
    finished: SyncSender<()>,
    current: Arc<AtomicU64>,
    generation: u64,
) -> impl FnMut(&mut [f32], Format) + Send + 'static {
    let end = sound.length().as_secs_f64() + TAIL;
    let mut elapsed = 0.0_f64;
    let mut told = false;
    move |samples, format| {
        // A cancelled sound gives silence and ends at once.
        if current.load(Ordering::Acquire) != generation {
            samples.fill(0.0);
            elapsed = end;
        }
        let step = 1.0 / f64::from(format.sample_rate.max(1));
        for frame in samples.chunks_mut(usize::from(format.channels.max(1))) {
            frame.fill(sample_at(sound.parts(), elapsed).unwrap_or(0.0));
            elapsed += step;
        }
        if !told && elapsed >= end {
            told = true;
            let _ = finished.try_send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::synthetic::SyntheticHost;
    use crate::audio::test_output::TestOutput;
    use std::sync::Mutex;
    use std::time::Instant;

    const PATIENCE: Duration = Duration::from_secs(10);
    /// The peak of the tone of an incoming call.
    const RING_LEVEL: f32 = 0.08;
    /// A sample below this is silence.
    const SILENT: f32 = 0.0005;

    fn peak(samples: &[f32]) -> f32 {
        samples
            .iter()
            .fold(0.0, |peak, sample| sample.abs().max(peak))
    }

    /// The time from the first to the last sample that is not silence.
    fn heard_length(samples: &[f32], rate: u32) -> Duration {
        let first = samples.iter().position(|sample| sample.abs() > SILENT);
        let last = samples.iter().rposition(|sample| sample.abs() > SILENT);
        match (first, last) {
            (Some(first), Some(last)) => {
                Duration::from_secs_f64((last - first + 1) as f64 / f64::from(rate))
            }
            _ => Duration::ZERO,
        }
    }

    #[test]
    fn every_sound_is_short_and_quiet_and_starts_and_ends_in_silence() {
        for sound in Sound::ALL {
            let samples = render(sound, 48_000);
            let length = heard_length(&samples, 48_000);
            assert!(
                (Duration::from_millis(60)..=Duration::from_millis(210)).contains(&length),
                "{sound:?} lasts {length:?}"
            );
            let highest = peak(&samples);
            assert!(
                highest > LEVEL * 0.9 && highest <= LEVEL && highest < RING_LEVEL / 2.0,
                "{sound:?} has the peak {highest}"
            );
            // Without the rise and the fall, the first and the last millisecond reach
            // more than half of the level.
            for edge in [&samples[..48], &samples[samples.len() - 48..]] {
                assert!(
                    peak(edge) < LEVEL / 10.0,
                    "{sound:?} does not rise from silence or fall to it"
                );
            }
        }
    }

    #[test]
    fn no_two_events_have_the_same_sound() {
        for (index, sound) in Sound::ALL.iter().enumerate() {
            for other in &Sound::ALL[index + 1..] {
                assert_ne!(sound.parts(), other.parts(), "{sound:?} and {other:?}");
            }
        }
    }

    #[test]
    fn a_sound_reaches_the_output_once_and_the_output_closes_after_it() {
        let host = SyntheticHost::with_tone(700.0);
        let (closed, close) = channel();
        let player = SoundPlayer::new(Arc::new(TestOutput {
            host: host.clone(),
            release: None,
            closed,
        }));
        player.play(Sound::PersonJoined, None);
        close
            .recv_timeout(PATIENCE)
            .expect("the output closed after the sound");
        let (samples, rate) = host.played_since_clear();
        let length = heard_length(&samples, rate);
        assert!(
            (Duration::from_millis(150)..=Duration::from_millis(170)).contains(&length),
            "the sound lasted {length:?}"
        );
        let highest = peak(&samples);
        assert!(
            highest > LEVEL * 0.9 && highest <= LEVEL,
            "the peak is {highest}"
        );
        assert_eq!(host.opened_outputs(), vec![None]);
    }

    #[test]
    fn a_device_that_is_slow_to_open_does_not_hold_the_caller() {
        let host = SyntheticHost::with_tone(700.0);
        let (release, held) = channel();
        let (closed, close) = channel();
        let player = SoundPlayer::new(Arc::new(TestOutput {
            host: host.clone(),
            release: Some(Mutex::new(held)),
            closed,
        }));
        let (returned, asked) = channel();
        let caller = std::thread::spawn(move || {
            player.play(Sound::MicrophoneOff, None);
            let _ = returned.send(());
            player
        });
        let answer = asked.recv_timeout(PATIENCE);
        release.send(()).unwrap();
        assert!(
            answer.is_ok(),
            "the request for a sound waited for the device"
        );
        let _player = caller.join().unwrap();
        close
            .recv_timeout(PATIENCE)
            .expect("the output closed after the sound");
    }

    #[test]
    fn a_cancel_drops_the_waiting_sounds_and_silences_the_one_that_opens() {
        let host = SyntheticHost::with_tone(700.0);
        let (release, held) = channel();
        let (closed, close) = channel();
        let player = SoundPlayer::new(Arc::new(TestOutput {
            host: host.clone(),
            release: Some(Mutex::new(held)),
            closed,
        }));
        // The first sound waits for its device, the second waits behind it.
        player.play(Sound::PersonJoined, None);
        std::thread::sleep(Duration::from_millis(100));
        player.play(Sound::PersonLeft, None);
        player.cancel();
        release.send(()).unwrap();
        release.send(()).unwrap();
        close
            .recv_timeout(PATIENCE)
            .expect("the output of the first sound closed");
        assert!(
            close.recv_timeout(Duration::from_millis(500)).is_err(),
            "the waiting sound was played"
        );
        let (samples, _) = host.played_since_clear();
        assert!(peak(&samples) < SILENT, "a cancelled sound was heard");
        assert_eq!(host.opened_outputs().len(), 1);
    }

    #[test]
    fn a_device_that_does_not_open_does_not_stop_the_next_sound() {
        let host = SyntheticHost::with_tone(700.0);
        let player = SoundPlayer::new(Arc::new(host.clone()));
        player.play(Sound::SoundOff, Some(&DeviceId("unplugged".to_owned())));
        player.play(Sound::SoundOn, None);
        let until = Instant::now() + PATIENCE;
        loop {
            let (samples, rate) = host.played_since_clear();
            if heard_length(&samples, rate) >= Duration::from_millis(140) {
                break;
            }
            assert!(Instant::now() < until, "the second sound was not played");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            host.opened_outputs(),
            vec![Some(DeviceId("unplugged".to_owned())), None]
        );
    }
}
