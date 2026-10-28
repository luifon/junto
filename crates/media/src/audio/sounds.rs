//! The short sounds of the events of the application (ADR 0707): messages, calls, shares,
//! requests and recordings. Plain sines at a low level, with a rise and a fall of 10 ms, so
//! that the device does not click.
//!
//! A sound opens the output device on the thread of the player, plays once, and closes
//! the device, as the tone of an incoming call does (ADR 0733). A request only sends the
//! sound to that thread and returns at once.

use super::{AudioHost, DeviceId, Format};
use junto_client_core::voice::SoundKind;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, SyncSender, channel, sync_channel};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    /// A new message for the person in a place that is not in front: a message of a place
    /// with the level "all messages", or a reply in a followed thread.
    Message,
    /// A message for the person alone: a mention, a direct message, a missed call.
    Mention,
    /// The person of this device entered a call.
    CallJoined,
    /// The person of this device is no longer in the call.
    CallLeft,
    /// A moderator moved the person of this device into another voice channel.
    Moved,
    /// The connection of the call of this device was lost, and the device connects again.
    ConnectionLost,
    /// Another person entered the call of this device.
    PersonJoined,
    /// Another person left the call of this device.
    PersonLeft,
    MicrophoneOff,
    MicrophoneOn,
    SoundOff,
    SoundOn,
    /// The key of push-to-talk is pressed: the microphone is sent.
    TalkStarts,
    /// The key of push-to-talk is released.
    TalkEnds,
    /// A share started in the call of this device.
    ShareStarted,
    /// A share ended in the call of this device.
    ShareEnded,
    /// A person started to watch the share of this device.
    ViewerJoined,
    /// A person stopped watching the share of this device.
    ViewerLeft,
    /// An invitation into a voice channel arrived.
    Invitation,
    /// A request waits for the person: a person asks to enter the call, or to join the
    /// server.
    Request,
    /// A recording of the call of this device started.
    RecordingStarted,
    /// The recording of the call of this device stopped.
    RecordingStopped,
    /// A person raised a hand in the call of this device.
    HandRaised,
}

impl Sound {
    pub const ALL: [Sound; 23] = [
        Sound::Message,
        Sound::Mention,
        Sound::CallJoined,
        Sound::CallLeft,
        Sound::Moved,
        Sound::ConnectionLost,
        Sound::PersonJoined,
        Sound::PersonLeft,
        Sound::MicrophoneOff,
        Sound::MicrophoneOn,
        Sound::SoundOff,
        Sound::SoundOn,
        Sound::TalkStarts,
        Sound::TalkEnds,
        Sound::ShareStarted,
        Sound::ShareEnded,
        Sound::ViewerJoined,
        Sound::ViewerLeft,
        Sound::Invitation,
        Sound::Request,
        Sound::RecordingStarted,
        Sound::RecordingStopped,
        Sound::HandRaised,
    ];

    /// The tones, glides and pauses of the sound, in their order. A kind of event has a
    /// pattern of its own: the microphone two short tones in the upper register, the sound
    /// of the person three short tones in the lower register, people who come and go one
    /// glide, a share two equal tones; "off" and "leaves" fall in pitch, "on" and "joins"
    /// rise.
    pub fn parts(self) -> &'static [Part] {
        use Part::{Glide, Pause, Tone};
        match self {
            Sound::Message => &[Tone(740, 90)],
            Sound::Mention => &[Tone(659, 70), Pause(20), Tone(880, 110)],
            Sound::CallJoined => &[Glide(330, 494, 110), Pause(20), Tone(659, 90)],
            Sound::CallLeft => &[Tone(440, 70), Pause(20), Glide(392, 262, 140)],
            Sound::Moved => &[Glide(392, 523, 90), Pause(30), Glide(392, 523, 90)],
            Sound::ConnectionLost => &[Tone(330, 120), Pause(60), Tone(330, 120)],
            Sound::PersonJoined => &[Glide(392, 587, 150)],
            Sound::PersonLeft => &[Glide(587, 392, 150)],
            Sound::MicrophoneOff => &[Tone(659, 55), Pause(25), Tone(523, 55)],
            Sound::MicrophoneOn => &[Tone(523, 55), Pause(25), Tone(659, 55)],
            Sound::SoundOff => &[
                Tone(440, 45),
                Pause(20),
                Tone(370, 45),
                Pause(20),
                Tone(294, 45),
            ],
            Sound::SoundOn => &[
                Tone(294, 45),
                Pause(20),
                Tone(370, 45),
                Pause(20),
                Tone(440, 45),
            ],
            Sound::TalkStarts => &[Tone(880, 40)],
            Sound::TalkEnds => &[Tone(587, 40)],
            Sound::ShareStarted => &[Tone(494, 60), Pause(40), Tone(494, 60)],
            Sound::ShareEnded => &[Tone(392, 60), Pause(40), Tone(392, 60)],
            Sound::ViewerJoined => &[Glide(523, 659, 80)],
            Sound::ViewerLeft => &[Glide(659, 523, 80)],
            Sound::Invitation => &[
                Tone(523, 80),
                Pause(20),
                Tone(659, 80),
                Pause(20),
                Tone(784, 110),
            ],
            Sound::Request => &[
                Tone(659, 60),
                Pause(60),
                Tone(659, 60),
                Pause(60),
                Tone(659, 60),
            ],
            Sound::RecordingStarted => &[
                Tone(587, 80),
                Pause(40),
                Tone(587, 80),
                Pause(40),
                Tone(784, 120),
            ],
            Sound::RecordingStopped => &[
                Tone(587, 80),
                Pause(40),
                Tone(587, 80),
                Pause(40),
                Tone(440, 120),
            ],
            Sound::HandRaised => &[Glide(523, 784, 120)],
        }
    }

    /// The kind of the sound, whose switch turns it on and off.
    pub fn kind(self) -> SoundKind {
        match self {
            Sound::Message => SoundKind::Messages,
            Sound::Mention => SoundKind::Mentions,
            Sound::CallJoined => SoundKind::CallJoined,
            Sound::CallLeft => SoundKind::CallLeft,
            Sound::Moved => SoundKind::Moved,
            Sound::ConnectionLost => SoundKind::ConnectionLost,
            Sound::PersonJoined => SoundKind::PersonJoined,
            Sound::PersonLeft => SoundKind::PersonLeft,
            Sound::MicrophoneOff => SoundKind::MicrophoneOff,
            Sound::MicrophoneOn => SoundKind::MicrophoneOn,
            Sound::SoundOff => SoundKind::SoundOff,
            Sound::SoundOn => SoundKind::SoundOn,
            Sound::TalkStarts => SoundKind::TalkStarts,
            Sound::TalkEnds => SoundKind::TalkEnds,
            Sound::ShareStarted => SoundKind::ShareStarted,
            Sound::ShareEnded => SoundKind::ShareEnded,
            Sound::ViewerJoined => SoundKind::ViewerJoined,
            Sound::ViewerLeft => SoundKind::ViewerLeft,
            Sound::Invitation => SoundKind::Invitation,
            Sound::Request => SoundKind::Request,
            Sound::RecordingStarted => SoundKind::RecordingStarted,
            Sound::RecordingStopped => SoundKind::RecordingStopped,
            Sound::HandRaised => SoundKind::HandRaised,
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

/// A part of a sound: a sine of a frequency in Hz for a number of milliseconds, a sine whose
/// frequency moves evenly from one value to another, or silence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Tone(u32, u32),
    Glide(u32, u32, u32),
    Pause(u32),
}

impl Part {
    fn millis(self) -> u32 {
        match self {
            Part::Tone(_, millis) | Part::Glide(_, _, millis) | Part::Pause(millis) => millis,
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
            let into = at - start;
            // The phase of a glide is the integral of its frequency, so the wave stays
            // continuous while the frequency moves.
            let cycles = match *part {
                Part::Pause(_) => return Some(0.0),
                Part::Tone(hz, _) => into * f64::from(hz),
                Part::Glide(from, to, millis) => {
                    let span = f64::from(millis) / 1000.0;
                    let (from, to) = (f64::from(from), f64::from(to));
                    into * from + (to - from) * into * into / (2.0 * span)
                }
            };
            let edge = into.min(end - at);
            let gain = if edge < FADE {
                0.5 - 0.5 * (std::f64::consts::PI * edge / FADE).cos()
            } else {
                1.0
            };
            let wave = (cycles * std::f64::consts::TAU).sin();
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

/// What a person hears in samples of a sound, for the tests of the sounds and of the
/// window: the tones of the samples, and the sound that they are.
#[cfg(any(test, feature = "test-support"))]
pub mod heard {
    use super::{Sound, render};

    /// A sample below this is silence.
    const SILENT: f32 = 0.0005;

    /// One tone or glide of a sound: the pitch of its first half and of its second half,
    /// from the crossings of zero, and its length.
    #[derive(Debug, Clone, Copy)]
    pub struct Tone {
        pub first: f32,
        pub second: f32,
        pub millis: f32,
    }

    impl Tone {
        pub fn pitch(self) -> f32 {
            (self.first + self.second) / 2.0
        }
    }

    /// The tones of samples: the runs of sound between pauses of at least 3 ms.
    pub fn tones(samples: &[f32], rate: u32) -> Vec<Tone> {
        let gap = (rate / 1000 * 3) as usize;
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for (index, _) in samples
            .iter()
            .enumerate()
            .filter(|(_, sample)| sample.abs() > SILENT)
        {
            match runs.last_mut() {
                Some((_, end)) if index - *end <= gap => *end = index,
                _ => runs.push((index, index)),
            }
        }
        let pitch = |part: &[f32]| {
            let crossings = part
                .windows(2)
                .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
                .count();
            crossings as f32 / 2.0 / (part.len() as f32 / rate as f32)
        };
        runs.into_iter()
            .map(|(start, end)| {
                let run = &samples[start..=end];
                let half = run.len() / 2;
                Tone {
                    first: pitch(&run[..half]),
                    second: pitch(&run[half..]),
                    millis: run.len() as f32 * 1000.0 / rate as f32,
                }
            })
            .collect()
    }

    /// Two sounds are alike when they have as many tones and each tone has nearly the same
    /// pitch in each half and nearly the same length.
    pub fn alike(one: &[Tone], other: &[Tone]) -> bool {
        let near = |a: f32, b: f32| (a - b).abs() <= a.max(b) * 0.03;
        one.len() == other.len()
            && one.iter().zip(other).all(|(a, b)| {
                near(a.first, b.first)
                    && near(a.second, b.second)
                    && (a.millis - b.millis).abs() < 10.0
            })
    }

    /// The sound that samples are, when they are one of the sounds.
    pub fn recognise(samples: &[f32], rate: u32) -> Option<Sound> {
        let heard = tones(samples, rate);
        Sound::ALL
            .into_iter()
            .find(|sound| alike(&heard, &tones(&render(*sound, rate), rate)))
    }
}

#[cfg(test)]
mod tests {
    use super::heard::{Tone, alike, recognise, tones};
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
    fn every_sound_can_be_switched_off_without_changing_another() {
        for sound in Sound::ALL {
            let mut settings = junto_client_core::voice::Sounds::default();
            settings.set(sound.kind(), false);
            for other in Sound::ALL {
                assert_eq!(
                    settings.plays(other.kind()),
                    other != sound,
                    "{sound:?} and {other:?}"
                );
            }
        }
    }

    #[test]
    fn every_sound_is_short_and_quiet_and_starts_and_ends_in_silence() {
        for sound in Sound::ALL {
            let samples = render(sound, 48_000);
            let length = heard_length(&samples, 48_000);
            assert!(
                (Duration::from_millis(30)..=Duration::from_millis(380)).contains(&length),
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
    fn no_two_events_have_sounds_that_are_alike() {
        let heard: Vec<(Sound, Vec<Tone>)> = Sound::ALL
            .iter()
            .map(|sound| (*sound, tones(&render(*sound, 48_000), 48_000)))
            .collect();
        for (index, (sound, tones)) in heard.iter().enumerate() {
            assert!(!tones.is_empty(), "{sound:?} is silent");
            for (other, other_tones) in &heard[index + 1..] {
                assert!(
                    !alike(tones, other_tones),
                    "{sound:?} {tones:?} and {other:?} {other_tones:?}"
                );
            }
        }
    }

    /// The four sounds of the microphone and of the sound of the person can be told apart
    /// without looking: the microphone two tones high, the sound three tones low, "off"
    /// falling and "on" rising.
    #[test]
    fn the_four_sounds_of_the_microphone_and_the_sound_differ_in_count_register_and_direction() {
        let of = |sound| tones(&render(sound, 48_000), 48_000);
        let falls = |tones: &[Tone]| {
            tones
                .windows(2)
                .all(|pair| pair[1].pitch() < pair[0].pitch() * 0.9)
        };
        let rises = |tones: &[Tone]| {
            tones
                .windows(2)
                .all(|pair| pair[1].pitch() > pair[0].pitch() * 1.1)
        };
        let (microphone_off, microphone_on) = (of(Sound::MicrophoneOff), of(Sound::MicrophoneOn));
        let (sound_off, sound_on) = (of(Sound::SoundOff), of(Sound::SoundOn));
        assert_eq!((microphone_off.len(), microphone_on.len()), (2, 2));
        assert_eq!((sound_off.len(), sound_on.len()), (3, 3));
        assert!(
            falls(&microphone_off) && rises(&microphone_on),
            "{microphone_off:?} {microphone_on:?}"
        );
        assert!(
            falls(&sound_off) && rises(&sound_on),
            "{sound_off:?} {sound_on:?}"
        );
        let lowest_of_microphone = microphone_off
            .iter()
            .chain(&microphone_on)
            .map(|tone| tone.pitch())
            .fold(f32::MAX, f32::min);
        let highest_of_sound = sound_off
            .iter()
            .chain(&sound_on)
            .map(|tone| tone.pitch())
            .fold(0.0, f32::max);
        assert!(
            lowest_of_microphone > highest_of_sound * 1.1,
            "the microphone at {lowest_of_microphone} Hz is not above the sound at {highest_of_sound} Hz"
        );
        for (index, one) in [&microphone_off, &microphone_on, &sound_off, &sound_on]
            .iter()
            .enumerate()
        {
            for other in &[&microphone_off, &microphone_on, &sound_off, &sound_on][index + 1..] {
                assert!(!alike(one, other));
            }
        }
    }

    /// A glide is heard moving in its direction: a person who joins rises, one who leaves
    /// falls.
    #[test]
    fn a_glide_moves_in_pitch_from_its_start_to_its_end() {
        let joined = tones(&render(Sound::PersonJoined, 48_000), 48_000);
        let left = tones(&render(Sound::PersonLeft, 48_000), 48_000);
        assert_eq!((joined.len(), left.len()), (1, 1));
        assert!(joined[0].second > joined[0].first * 1.2, "{joined:?}");
        assert!(left[0].second < left[0].first / 1.2, "{left:?}");
    }

    #[test]
    fn every_sound_is_recognised_at_the_rates_of_devices() {
        for rate in [44_100, 48_000] {
            for sound in Sound::ALL {
                assert_eq!(recognise(&render(sound, rate), rate), Some(sound), "{rate}");
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
            (Duration::from_millis(140)..=Duration::from_millis(160)).contains(&length),
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
