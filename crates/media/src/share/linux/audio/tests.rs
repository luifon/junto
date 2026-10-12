#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::Path;
use std::process::{Child, Command};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::audio::synthetic::tone_level;
use crate::calls::MediaOptions;
use crate::share::linux::{SoundClock, prepared};

use super::*;

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
        command.args([
            "--target=junto-linux1-shared-sound-test",
            "--properties={ node.dont-fallback=true node.dont-reconnect=true }",
        ]);
    } else {
        command.arg("--device=junto-linux1-shared-sound-test");
    }
    Player(command.arg(file).spawn().unwrap())
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

fn levels(samples: &[f32]) -> [f64; 3] {
    [440.0, 1000.0, 2500.0].map(|hz| f64::from(tone_level(samples, 48_000, hz)))
}

#[tokio::test]
#[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
async fn selected_application_sound_and_system_sound_exclude_own_playback_and_ignore_sink_mute() {
    assert_eq!(
        std::env::var("JUNTO_TEST_VIRTUAL_AUDIO").as_deref(),
        Ok("1")
    );
    let _sink = Sink(pactl(&[
        "load-module",
        "module-null-sink",
        "sink_name=junto-linux1-shared-sound-test",
        "sink_properties=device.description=Junto-Shared-Sound-Test",
    ]));
    let data = tempfile::Builder::new()
        .prefix("junto-linux1-share-")
        .tempdir_in("/tmp")
        .unwrap();
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
    let _own = host
        .open_output(
            Some(&output.id),
            Box::new(move |samples, format| {
                for frame in samples.chunks_exact_mut(usize::from(format.channels)) {
                    frame.fill(0.2 * phase.sin());
                    phase = (phase + std::f32::consts::TAU * 2500.0 / format.sample_rate as f32)
                        % std::f32::consts::TAU;
                }
            }),
            Box::new(move |failure| reported.lock().unwrap().push(failure)),
        )
        .unwrap();
    pactl(&["set-sink-mute", "junto-linux1-shared-sound-test", "1"]);
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
    let samples = system.sample(4);
    let measured = levels(&samples[48_000..]);
    eprintln!("system levels at 440, 1000 and own 2500 Hz: {measured:?}");
    assert!(
        measured[0] > -22.0 && measured[1] > -22.0 && measured[2] < -50.0,
        "{measured:?}"
    );
    drop(system);
    let mut chosen = Recording::start(SoundChoice::Application(application)).await;
    let samples = chosen.sample(3);
    let measured = levels(&samples[48_000..]);
    eprintln!("application levels at 440, 1000 and own 2500 Hz: {measured:?}");
    assert!(
        measured[0] > -22.0 && measured[1] < -50.0 && measured[2] < -50.0,
        "{measured:?}"
    );
    drop(wanted);
    let pause = chosen.sample(2);
    assert!(pause[48_000..].iter().all(|value| value.abs() < 0.0001));
    let _again = player("pw-play", &wanted_file);
    let restarted = chosen.sample(3);
    let measured = levels(&restarted[48_000..]);
    eprintln!("restarted application levels: {measured:?}");
    assert!(
        faults.lock().unwrap().is_empty(),
        "{:?}",
        faults.lock().unwrap()
    );
    assert!(
        measured[0] > -22.0 && measured[1] < -50.0 && measured[2] < -50.0,
        "{measured:?}"
    );
}
