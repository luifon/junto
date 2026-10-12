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

fn pactl(arguments: &[&str]) -> String {
    assert_eq!(
        std::env::var("JUNTO_TEST_VIRTUAL_AUDIO").as_deref(),
        Ok("1")
    );
    let output = Command::new("pactl").args(arguments).output().unwrap();
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
        if let Err(error) = Command::new("pactl")
            .args(["unload-module", &self.0])
            .output()
        {
            eprintln!("could not remove the virtual sound device: {error}");
        }
    }
}

fn until(reason: &str, mut ready: impl FnMut() -> bool) {
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
    let name = "junto-test-roundtrip";
    let _sink = Sink::new(name);
    let host = MediaOptions::system().host;
    let devices = host.devices().unwrap();
    let input = named(&devices.inputs, name).expect("the null sink has a monitor input");
    let output = named(&devices.outputs, name).expect("the null sink is an output");
    assert!(input.id.0.to_lowercase().contains("pipewire"));
    assert!(output.id.0.to_lowercase().contains("pipewire"));
    let samples = Arc::new(Mutex::new((Vec::new(), 0)));
    let captured = samples.clone();
    let faults = Arc::new(Mutex::new(Vec::new()));
    let input_faults = faults.clone();
    let _input = host
        .open_input(
            Some(&input.id),
            Box::new(move |data, format| {
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
fn a_later_error_does_not_erase_the_device_disconnection() {
    use junto_media::audio::AudioError;
    let reported = Mutex::new(None);
    remember_failure(&reported, AudioError::NoDevice);
    remember_failure(
        &reported,
        AudioError::Failed("after the disconnection".to_owned()),
    );
    assert_eq!(*reported.lock().unwrap(), Some(AudioError::NoDevice));
}

#[test]
#[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
fn cleanup_does_not_panic_when_the_module_is_already_gone() {
    assert!(std::panic::catch_unwind(|| drop(Sink("4294967295".to_owned()))).is_ok());
}

#[test]
#[ignore = "needs isolated virtual PipeWire devices: tools/linux-container.sh"]
fn an_unavailable_pipewire_daemon_is_an_audio_error() {
    assert_eq!(
        std::env::var("JUNTO_TEST_VIRTUAL_AUDIO").as_deref(),
        Ok("1")
    );
    if std::env::var_os("JUNTO_TEST_MISSING_PIPEWIRE_CHILD").is_some() {
        assert!(MediaOptions::system().host.devices().is_err());
        return;
    }
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "an_unavailable_pipewire_daemon_is_an_audio_error",
            "--nocapture",
        ])
        .env("PIPEWIRE_REMOTE", "junto-linux1-unavailable")
        .env("JUNTO_TEST_MISSING_PIPEWIRE_CHILD", "1")
        .output()
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
