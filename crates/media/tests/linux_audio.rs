//! The production sound host against isolated PipeWire null devices.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use junto_media::audio::DeviceInfo;
use junto_media::audio::synthetic::tone_level;
use junto_media::calls::MediaOptions;

#[allow(dead_code)]
#[path = "../../client-core/tests/e2e/limit.rs"]
mod limit;

fn pactl(arguments: &[&str]) -> String {
    assert_eq!(
        std::env::var("JUNTO_TEST_VIRTUAL_AUDIO").as_deref(),
        Ok("1")
    );
    let output = limit::output_of(Command::new("pactl").args(arguments)).unwrap();
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

struct Sink(String);

impl Sink {
    fn new(name: &str) -> Self {
        Self(pactl(&[
            "load-module",
            "module-null-sink",
            &format!("sink_name={name}"),
            &format!("sink_properties=device.description={name}"),
        ]))
    }
}

impl Drop for Sink {
    fn drop(&mut self) {
        if let Err(error) = limit::output_of(Command::new("pactl").args(["unload-module", &self.0]))
        {
            eprintln!("could not remove the virtual sound device: {error}");
        }
    }
}

fn until(reason: &str, mut ready: impl FnMut() -> bool) {
    limit::step(reason);
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < Duration::from_secs(15), "{reason}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn named(devices: &[DeviceInfo], name: &str) -> Option<DeviceInfo> {
    devices.iter().find(|device| device.name == name).cloned()
}

#[test]
#[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
fn the_production_host_captures_its_playback_through_a_null_device() {
    let _limit = limit::watch(limit::TEST_LIMIT);
    let name = "junto-test-roundtrip";
    let _sink = Sink::new(name);
    let host = MediaOptions::system().host;
    let devices = host.devices().unwrap();
    let input = named(&devices.inputs, name).expect("the null sink has a monitor input");
    let output = named(&devices.outputs, name).expect("the null sink is an output");
    assert!(input.id.0.to_lowercase().contains("pipewire"));
    assert!(output.id.0.to_lowercase().contains("pipewire"));
    let link = Arc::new(junto_media::audio::capture::CaptureLink::default());
    let (frames, taken) = tokio::sync::mpsc::channel(256);
    let mut chain = junto_media::audio::capture::CaptureChain::new(
        link.clone(),
        Arc::new(junto_media::audio::processing::Processor::new(
            junto_media::audio::processing::Processing::OFF,
        )),
        frames,
    );
    let samples = Arc::new(Mutex::new((Vec::new(), 0)));
    let captured = samples.clone();
    let faults = Arc::new(Mutex::new(Vec::new()));
    let input_faults = faults.clone();
    let _input = host
        .open_input(
            Some(&input.id),
            Box::new(move |data, format| {
                chain.deliver(data, format);
                let mut captured = captured.lock().unwrap();
                captured.1 = format.sample_rate;
                captured.0.extend(
                    data.chunks_exact(usize::from(format.channels))
                        .map(|frame| frame[0]),
                );
                let stale = captured
                    .0
                    .len()
                    .saturating_sub(format.sample_rate as usize * 2);
                captured.0.drain(..stale);
            }),
            Box::new(move |error| input_faults.lock().unwrap().push(error)),
        )
        .unwrap();
    std::thread::sleep(Duration::from_secs(5));
    let output_faults = faults.clone();
    let mut phase = 0.0_f32;
    let _output = host
        .open_output(
            Some(&output.id),
            Box::new(move |data, format| {
                for frame in data.chunks_exact_mut(usize::from(format.channels)) {
                    frame.fill(0.2 * phase.sin());
                    phase = (phase + std::f32::consts::TAU * 440.0 / format.sample_rate as f32)
                        % std::f32::consts::TAU;
                }
            }),
            Box::new(move |error| output_faults.lock().unwrap().push(error)),
        )
        .unwrap();
    until("a captured second of the 440 Hz tone", || {
        let captured = samples.lock().unwrap();
        captured.1 != 0
            && captured.0.len() >= captured.1 as usize
            && tone_level(&captured.0, captured.1, 440.0) > -20.0
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let mut queue = junto_media::audio::capture::FrameQueue::new(link.clone(), taken);
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            for _ in 0..300 {
                let frame = queue.next().await.unwrap();
                assert!(frame.iter().all(|sample| *sample == 0));
                queue.taken();
            }
        })
        .await
        .unwrap();
    });
    assert!(link.input.read() > -20.0);
    assert!(!link.is_open() && !link.speaking());
    let captured = samples.lock().unwrap();
    assert!(tone_level(&captured.0, captured.1, 1_700.0) < -50.0);
    assert!(
        faults.lock().unwrap().is_empty(),
        "{:?}",
        faults.lock().unwrap()
    );
}

#[test]
#[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
fn the_production_device_list_follows_a_device_appearing_and_disappearing() {
    let _limit = limit::watch(limit::TEST_LIMIT);
    let name = "junto-test-hotplug";
    let host = MediaOptions::system().host;
    assert!(named(&host.devices().unwrap().outputs, name).is_none());
    let sink = Sink::new(name);
    until("the existing host lists the new device", || {
        named(&host.devices().unwrap().outputs, name).is_some()
    });
    let chosen = named(&host.devices().unwrap().outputs, name).unwrap().id;
    let failed = Arc::new(Mutex::new(None));
    let reported = failed.clone();
    let played = Arc::new(AtomicUsize::new(0));
    let playing = played.clone();
    let stream = host
        .open_output(
            Some(&chosen),
            Box::new(move |samples, _| {
                samples.fill(0.0);
                playing.fetch_add(samples.len(), Ordering::Relaxed);
            }),
            Box::new(move |error| remember_failure(&reported, error)),
        )
        .unwrap();
    until("the device starts playing before it is removed", || {
        played.load(Ordering::Relaxed) > 0
    });
    drop(sink);
    until("the open stream reports the disconnected device", || {
        failed.lock().unwrap().is_some()
    });
    assert_eq!(
        *failed.lock().unwrap(),
        Some(junto_media::audio::AudioError::NoDevice)
    );
    drop(stream);
    until("the existing host removes the disconnected device", || {
        named(&host.devices().unwrap().outputs, name).is_none()
    });
    assert_eq!(
        host.open_output(Some(&chosen), Box::new(|_, _| {}), Box::new(|_| {}))
            .unwrap_err(),
        junto_media::audio::AudioError::NoDevice,
    );
}

fn remember_failure(
    reported: &Mutex<Option<junto_media::audio::AudioError>>,
    error: junto_media::audio::AudioError,
) {
    reported.lock().unwrap().get_or_insert(error);
}

#[test]
#[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
fn an_unavailable_pipewire_daemon_is_an_audio_error() {
    let _limit = limit::watch(limit::TEST_LIMIT);
    assert_eq!(
        std::env::var("JUNTO_TEST_VIRTUAL_AUDIO").as_deref(),
        Ok("1")
    );
    if std::env::var_os("JUNTO_TEST_MISSING_PIPEWIRE_CHILD").is_some() {
        assert!(MediaOptions::system().host.devices().is_err());
        return;
    }
    let output = limit::output_of(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "an_unavailable_pipewire_daemon_is_an_audio_error",
                "--nocapture",
            ])
            .env("PIPEWIRE_REMOTE", "junto-linux-unavailable")
            .env("JUNTO_TEST_MISSING_PIPEWIRE_CHILD", "1"),
    )
    .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
#[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
fn removing_a_monitor_ends_the_capture_without_choosing_another_device() {
    let _limit = limit::watch(limit::TEST_LIMIT);
    let name = "junto-test-capture-removal";
    let sink = Sink::new(name);
    let host = MediaOptions::system().host;
    let input = named(&host.devices().unwrap().inputs, name).unwrap().id;
    let captured = Arc::new(AtomicUsize::new(0));
    let received = captured.clone();
    let failed = Arc::new(Mutex::new(None));
    let reported = failed.clone();
    let stream = host
        .open_input(
            Some(&input),
            Box::new(move |samples, _| {
                received.fetch_add(samples.len(), Ordering::Relaxed);
            }),
            Box::new(move |error| remember_failure(&reported, error)),
        )
        .unwrap();
    until("the monitor starts capturing before removal", || {
        captured.load(Ordering::Relaxed) > 0
    });
    drop(sink);
    until("the monitor reports removal", || {
        failed.lock().unwrap().is_some()
    });
    assert_eq!(
        *failed.lock().unwrap(),
        Some(junto_media::audio::AudioError::NoDevice)
    );
    drop(stream);
    until(
        "the removed monitor leaves the refreshed device list",
        || named(&host.devices().unwrap().inputs, name).is_none(),
    );
    assert_eq!(
        host.open_input(Some(&input), Box::new(|_, _| {}), Box::new(|_| {}))
            .unwrap_err(),
        junto_media::audio::AudioError::NoDevice
    );
}

#[test]
#[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
fn muted_speech_ignores_playback_captured_by_the_microphone_with_processing_off() {
    use junto_media::audio::{
        capture::{CaptureChain, CaptureLink, FrameQueue},
        playout::{BLOCK_SAMPLES, Mixer, PlayoutChain},
        processing::{Processing, Processor},
    };
    use std::sync::atomic::AtomicBool;
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .try_init();
    let _limit = limit::watch(limit::TEST_LIMIT);
    let name = "junto-test-muted-echo";
    let _sink = Sink::new(name);
    let host = MediaOptions::system().host;
    let devices = host.devices().unwrap();
    let input = named(&devices.inputs, name).unwrap();
    let output = named(&devices.outputs, name).unwrap();
    let processor = Arc::new(Processor::new(Processing::OFF));
    let link = Arc::new(CaptureLink::default());
    let (frames, mut taken) = tokio::sync::mpsc::channel(2048);
    let mut capture = CaptureChain::new(link.clone(), processor.clone(), frames);
    let local = Arc::new(AtomicBool::new(false));
    let speaking = local.clone();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let levels = observed.clone();
    let captured = link.clone();
    let faults = Arc::new(Mutex::new(Vec::new()));
    let failed = faults.clone();
    let mut phase = 0.0_f32;
    let mut spoken = 0usize;
    let _input = host
        .open_input(
            Some(&input.id),
            Box::new(move |data, format| {
                let mut data = data.to_vec();
                // Local speech as syllables of 200 ms with pauses of 150 ms and a changed
                // pitch for each: the echo canceller suppresses a continuous single tone
                // after its onset while playback continues.
                if speaking.load(Ordering::Relaxed) {
                    let syllable = format.sample_rate as usize * 35 / 100;
                    let voiced = format.sample_rate as usize / 5;
                    for frame in data.chunks_exact_mut(usize::from(format.channels)) {
                        let pitch = [700.0_f32, 1100.0, 1700.0][(spoken / syllable) % 3];
                        if spoken % syllable < voiced {
                            for sample in frame {
                                *sample += 0.15 * phase.sin();
                            }
                        }
                        phase = (phase + std::f32::consts::TAU * pitch / format.sample_rate as f32)
                            % std::f32::consts::TAU;
                        spoken += 1;
                    }
                }
                capture.deliver(&data, format);
                levels
                    .lock()
                    .unwrap()
                    .push((captured.input.read(), captured.speech_input.read()));
            }),
            Box::new(move |error| failed.lock().unwrap().push(error)),
        )
        .unwrap();
    let mixer = Mixer::shared();
    let source = mixer.add(1.0);
    let mut playout = PlayoutChain::new(mixer.clone(), processor);
    let failed = faults.clone();
    let mut index = 0usize;
    let _output = host
        .open_output(
            Some(&output.id),
            Box::new(move |data, format| {
                while mixer.waiting(&source) < 10 {
                    let mut block = [0i16; BLOCK_SAMPLES];
                    for frame in block.as_chunks_mut::<2>().0 {
                        let phase = std::f32::consts::TAU * 440.0 * index as f32 / 48000.0;
                        frame.fill((phase.sin() * 0.2 * f32::from(i16::MAX)) as i16);
                        index += 1;
                    }
                    source.push(&block);
                }
                playout.fill(data, format);
            }),
            Box::new(move |error| failed.lock().unwrap().push(error)),
        )
        .unwrap();
    until("the virtual microphone receives loud playback", || {
        link.input.read() > -25.0
    });
    until(
        "the echo-filtered local signal rejects sustained speaker playback",
        || {
            let levels = observed.lock().unwrap();
            levels.len() >= 100
                && levels[levels.len() - 100..]
                    .iter()
                    .all(|(raw, speech)| *raw > -25.0 && *speech < -45.0)
        },
    );
    let observed_echo = observed.lock().unwrap();
    let peak = observed_echo
        .iter()
        .map(|(_, filtered)| *filtered)
        .fold(-96.0_f32, f32::max);
    eprintln!(
        "echo-only peak filtered level {peak}, last raw/filtered {:?}",
        observed_echo.last()
    );
    assert!(
        peak < -45.0,
        "speaker playback triggered detection during startup: {peak}"
    );
    drop(observed_echo);
    observed.lock().unwrap().clear();
    while taken.try_recv().is_ok() {}
    local.store(true, Ordering::Relaxed);
    until("local speech beside playback remains detectable", || {
        let levels = observed.lock().unwrap();
        levels.iter().filter(|(_, speech)| *speech > -40.0).count() >= 10
    });
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let mut queue = FrameQueue::new(link.clone(), taken);
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(5), async {
            for _ in 0..100 {
                assert!(
                    queue
                        .next()
                        .await
                        .unwrap()
                        .iter()
                        .all(|sample| *sample == 0)
                );
            }
        })
        .await
        .unwrap();
    });
    assert!(!link.is_open() && !link.speaking());
    assert!(faults.lock().unwrap().is_empty());
}
