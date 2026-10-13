#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command};
use std::sync::atomic::AtomicUsize;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::audio::synthetic::tone_level;
use crate::calls::MediaOptions;
use crate::share::linux::{SoundClock, prepared};

use super::*;

/// The tests share one sound graph: a system capture of one test would hear the players of another.
static GRAPH: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
const SINK: &str = "junto-shared-sound-test";

fn pactl(arguments: &[&str]) -> String {
    let output = Command::new("pactl").args(arguments).output().unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().into()
}
struct Sink(String);
impl Drop for Sink {
    fn drop(&mut self) {
        let _ = Command::new("pactl")
            .args(["unload-module", &self.0])
            .output();
    }
}
struct Player(Child);
impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn player(program: &str, file: &Path) -> Player {
    let mut command = Command::new(program);
    if program == "pw-play" {
        command
            .arg(format!("--target={SINK}"))
            .arg("--properties={ node.dont-fallback=true node.dont-reconnect=true }");
    } else {
        command.arg(format!("--device={SINK}"));
    }
    Player(command.arg(file).spawn().unwrap())
}

/// A private null sink and a directory for the tone files.
fn scene() -> (Sink, tempfile::TempDir) {
    assert_eq!(
        std::env::var("JUNTO_TEST_VIRTUAL_AUDIO").as_deref(),
        Ok("1")
    );
    let sink = Sink(pactl(&[
        "load-module",
        "module-null-sink",
        &format!("sink_name={SINK}"),
        "sink_properties=device.description=Junto-Shared-Sound-Test",
    ]));
    // The session restores the mute of an earlier sink of this name, and the monitor of a
    // muted sink is silent.
    pactl(&["set-sink-mute", SINK, "0"]);
    let data = tempfile::Builder::new()
        .prefix("junto-share-")
        .tempdir_in("/tmp")
        .unwrap();
    (sink, data)
}

/// The level of a tone at the monitor of the test sink, in dBFS.
fn monitor_level(frequency: f32) -> f64 {
    let mut recorder = Command::new("parec")
        .arg(format!("--device={SINK}.monitor"))
        .args([
            "--raw",
            "--format=float32le",
            "--rate=48000",
            "--channels=1",
        ])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = recorder.stdout.take().unwrap();
    let mut bytes = vec![0; 4 * 48_000 * 2];
    std::io::Read::read_exact(&mut output, &mut bytes).unwrap();
    recorder.kill().unwrap();
    recorder.wait().unwrap();
    let samples: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|sample| f32::from_le_bytes(*sample))
        .collect();
    f64::from(tone_level(&samples[48_000..], 48_000, frequency))
}

fn tone(file: &Path, frequency: f32) {
    let frames = 48_000 * 60;
    let bytes = frames * 4_u32;
    let mut wav = std::fs::File::create(file).unwrap();
    wav.write_all(b"RIFF").unwrap();
    wav.write_all(&(bytes + 36).to_le_bytes()).unwrap();
    wav.write_all(b"WAVEfmt ").unwrap();
    wav.write_all(&16_u32.to_le_bytes()).unwrap();
    wav.write_all(&1_u16.to_le_bytes()).unwrap();
    wav.write_all(&2_u16.to_le_bytes()).unwrap();
    wav.write_all(&48_000_u32.to_le_bytes()).unwrap();
    wav.write_all(&192_000_u32.to_le_bytes()).unwrap();
    wav.write_all(&4_u16.to_le_bytes()).unwrap();
    wav.write_all(&16_u16.to_le_bytes()).unwrap();
    wav.write_all(b"data").unwrap();
    wav.write_all(&bytes.to_le_bytes()).unwrap();
    let mut data = Vec::with_capacity(bytes as usize);
    for frame in 0..frames {
        let sample = (0.2
            * (f64::from(frame) * std::f64::consts::TAU * f64::from(frequency) / 48_000.0).sin()
            * f64::from(i16::MAX)) as i16;
        data.extend_from_slice(&sample.to_le_bytes());
        data.extend_from_slice(&sample.to_le_bytes());
    }
    wav.write_all(&data).unwrap();
}

struct Recording {
    life: Arc<Life>,
    thread: Option<JoinHandle<()>>,
    clock: SoundClock,
}
impl Recording {
    async fn start(choice: SoundChoice) -> Self {
        let life = Arc::new(Life::default());
        let (samples, received) = rtrb::RingBuffer::new(CAPACITY);
        let (ready, started) = tokio::sync::oneshot::channel();
        let thread = std::thread::spawn({
            let life = life.clone();
            move || run(life, choice, samples, ready)
        });
        let recording = Self {
            life,
            thread: Some(thread),
            clock: SoundClock::new(received),
        };
        prepared(started, &recording.life).await.unwrap();
        recording
    }
    fn sample(&mut self, seconds: u32) -> Vec<f32> {
        let mut collected = Vec::new();
        let mut next = Instant::now();
        for _ in 0..seconds * 100 {
            self.life.result().unwrap();
            let frame = self.clock.tick(Instant::now());
            collected.extend(frame.as_chunks::<2>().0.iter().map(|pair| pair[0]));
            next += Duration::from_millis(10);
            std::thread::sleep(next.saturating_duration_since(Instant::now()));
        }
        eprintln!("steps of the sound clock so far: {}", self.clock.breaks);
        collected
    }
}
impl Drop for Recording {
    fn drop(&mut self) {
        self.life.stop();
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

const TONES: [f32; 3] = [440.0, 1000.0, 2500.0];

/// The level of a tone in each half second. The delivery clock can change the pitch by
/// one frame in a block of 480 for a second or longer while it follows the clock of the
/// sound graph, so each half second is measured at the three possible pitches.
fn half_seconds(samples: &[f32], hz: f32) -> impl Iterator<Item = f64> + '_ {
    samples
        .as_chunks::<24_000>()
        .0
        .iter()
        .map(move |half_second| {
            [hz * 479.0 / 480.0, hz, hz * 481.0 / 480.0]
                .into_iter()
                .map(|shifted| f64::from(tone_level(half_second, 48_000, shifted)))
                .fold(f64::MIN, f64::max)
        })
}

/// For each tone: the level that at least half of the half seconds reach, for a tone that
/// must be there. One step of the delivery clock in the measured time does not hide the
/// tone; a tone that plays for a small part of the time does not pass.
fn usual(samples: &[f32]) -> [f64; 3] {
    TONES.map(|hz| {
        let mut levels: Vec<f64> = half_seconds(samples, hz).collect();
        levels.sort_by(f64::total_cmp);
        levels[(levels.len() - 1) / 2]
    })
}

/// For each tone: its level in the half second in which it is strongest, for a tone that
/// must never be there.
fn strongest(samples: &[f32]) -> [f64; 3] {
    TONES.map(|hz| half_seconds(samples, hz).fold(f64::MIN, f64::max))
}

#[tokio::test]
#[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
async fn selected_application_sound_and_system_sound_exclude_own_playback_and_ignore_sink_mute() {
    let _graph = GRAPH.lock().await;
    let (_sink, data) = scene();
    let wanted_file = data.path().join("wanted.wav");
    let other_file = data.path().join("other.wav");
    tone(&wanted_file, 440.0);
    tone(&other_file, 1000.0);
    let wanted = player("pw-play", &wanted_file);
    let _other = player("paplay", &other_file);
    let host = MediaOptions::system().host;
    let output = host
        .devices()
        .unwrap()
        .outputs
        .into_iter()
        .find(|device| device.name == "Junto-Shared-Sound-Test")
        .unwrap();
    let mut phase = 0.0_f32;
    let faults = Arc::new(std::sync::Mutex::new(Vec::new()));
    let reported = faults.clone();
    // Without these two controls the exclusion would also hold for a tone that never plays.
    let played = Arc::new(AtomicUsize::new(0));
    let filled = played.clone();
    let _own = host
        .open_output(
            Some(&output.id),
            Box::new(move |samples, format| {
                for frame in samples.chunks_exact_mut(usize::from(format.channels)) {
                    frame.fill(0.2 * phase.sin());
                    phase = (phase + std::f32::consts::TAU * 2500.0 / format.sample_rate as f32)
                        % std::f32::consts::TAU;
                }
                filled.fetch_add(
                    samples.len() / usize::from(format.channels),
                    Ordering::Relaxed,
                );
            }),
            Box::new(move |failure| reported.lock().unwrap().push(failure)),
        )
        .unwrap();
    let started = Instant::now();
    let own = loop {
        let own = monitor_level(2500.0);
        if own > -22.0 {
            break own;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the own tone is absent from the output: {own} dBFS"
        );
    };
    eprintln!("own 2500 Hz tone at the monitor of its output: {own} dBFS");
    let own_frames = |seconds: usize, before: usize| {
        let frames = played.load(Ordering::Relaxed) - before;
        assert!(
            frames > seconds * 48_000 / 2,
            "the own tone played only {frames} frames in {seconds} seconds"
        );
    };
    pactl(&["set-sink-mute", SINK, "1"]);
    let started = Instant::now();
    let application = loop {
        let (_, apps) = sources().unwrap();
        if let Some(app) = apps
            .iter()
            .find(|app| app.id.bundle_id.starts_with("native:pw-"))
        {
            break app.id.clone();
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the test player is absent: {apps:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut system = Recording::start(SoundChoice::System).await;
    let before = played.load(Ordering::Relaxed);
    let samples = system.sample(4);
    own_frames(4, before);
    let (there, at_most) = (usual(&samples[48_000..]), strongest(&samples[48_000..]));
    eprintln!("system levels at 440, 1000 and own 2500 Hz: {there:?}, at most {at_most:?}");
    assert!(
        there[0] > -22.0 && there[1] > -22.0 && at_most[2] < -50.0,
        "{there:?}, at most {at_most:?}"
    );
    drop(system);
    let mut chosen = Recording::start(SoundChoice::Application(application)).await;
    let before = played.load(Ordering::Relaxed);
    let samples = chosen.sample(3);
    own_frames(3, before);
    let (there, at_most) = (usual(&samples[48_000..]), strongest(&samples[48_000..]));
    eprintln!("application levels at 440, 1000 and own 2500 Hz: {there:?}, at most {at_most:?}");
    assert!(
        there[0] > -22.0 && at_most[1] < -50.0 && at_most[2] < -50.0,
        "{there:?}, at most {at_most:?}"
    );
    drop(wanted);
    let pause = chosen.sample(2);
    assert!(pause[48_000..].iter().all(|value| value.abs() < 0.0001));
    let _again = player("pw-play", &wanted_file);
    let before = played.load(Ordering::Relaxed);
    let restarted = chosen.sample(3);
    own_frames(3, before);
    let (there, at_most) = (usual(&restarted[48_000..]), strongest(&restarted[48_000..]));
    eprintln!("restarted application levels: {there:?}, at most {at_most:?}");
    assert!(
        faults.lock().unwrap().is_empty(),
        "{:?}",
        faults.lock().unwrap()
    );
    assert!(
        there[0] > -22.0 && at_most[1] < -50.0 && at_most[2] < -50.0,
        "{there:?}, at most {at_most:?}"
    );
}

#[tokio::test]
#[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
async fn a_player_that_ends_during_a_capture_releases_its_objects_and_the_capture_goes_on() {
    let _graph = GRAPH.lock().await;
    let (_sink, data) = scene();
    let file = data.path().join("tone.wav");
    tone(&file, 440.0);
    let first = player("pw-play", &file);
    let mut system = Recording::start(SoundChoice::System).await;
    let samples = system.sample(2);
    assert!(usual(&samples[48_000..])[0] > -22.0);
    // PipeWire removes the node, its ports and the client of the player.
    drop(first);
    let pause = system.sample(2);
    assert!(pause[48_000..].iter().all(|value| value.abs() < 0.0001));
    let _second = player("pw-play", &file);
    let samples = system.sample(3);
    assert!(usual(&samples[48_000..])[0] > -22.0);
}

#[tokio::test]
#[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
async fn players_that_start_and_end_all_the_time_do_not_end_a_system_capture() {
    let _graph = GRAPH.lock().await;
    let (_sink, data) = scene();
    let file = data.path().join("tone.wav");
    tone(&file, 440.0);
    let _steady = player("pw-play", &file);
    let mut system = Recording::start(SoundChoice::System).await;
    let started = Instant::now();
    let mut round = 0_u64;
    // A player that lives a few milliseconds can end between its announcement and its link.
    while started.elapsed() < Duration::from_secs(30) {
        let players: Vec<Player> = (0..3).map(|_| player("pw-play", &file)).collect();
        std::thread::sleep(Duration::from_millis(round * 37 % 250));
        drop(players);
        system
            .life
            .result()
            .unwrap_or_else(|failure| panic!("round {round}: {failure}"));
        round += 1;
    }
    eprintln!("{round} rounds of three players");
    let samples = system.sample(3);
    assert!(usual(&samples[48_000..])[0] > -22.0);
}

#[tokio::test]
#[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
async fn a_system_capture_starts_while_nothing_plays_and_hears_a_later_player() {
    let _graph = GRAPH.lock().await;
    let (_sink, data) = scene();
    let file = data.path().join("tone.wav");
    tone(&file, 440.0);
    // A stream without a link has no format yet; the start must not wait for one.
    let mut system = Recording::start(SoundChoice::System).await;
    let silence = system.sample(1);
    assert!(silence.iter().all(|value| value.abs() < 0.0001));
    let _player = player("pw-play", &file);
    let samples = system.sample(3);
    assert!(usual(&samples[48_000..])[0] > -22.0);
}
