use junto_media::audio::cpal_host::CpalHost;
use junto_media::audio::{AudioHost, DeviceId};
use junto_media::permission::{MicrophoneAccess, MicrophonePermission, SystemPermission};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use super::{guard::Guard, route};

pub fn host() -> Result<Arc<Guard>, Box<dyn std::error::Error>> {
    Ok(Arc::new(Guard::new(
        Arc::new(CpalHost::new()),
        route::verify,
    )?))
}

pub fn devices() -> Result<(), Box<dyn std::error::Error>> {
    let started = Instant::now();
    let host = host()?;
    let (output, output_seen) = mpsc::sync_channel(16);
    let stream = host.open_output(
        Some(host.id()),
        Box::new(move |samples, format| {
            samples.fill(0.0);
            let _ = output.try_send((samples.len(), format));
        }),
        Box::new(|error| eprintln!("output failed: {error}")),
    )?;
    let observed = output_seen.recv_timeout(Duration::from_secs(10))?;
    println!(
        "D0 output id={} samples={} format={:?} elapsed={:?}",
        stream.device.id,
        observed.0,
        observed.1,
        started.elapsed()
    );
    if SystemPermission.state() != MicrophoneAccess::Allowed {
        return Err("microphone permission must already be Allowed; no request was made".into());
    }
    let (input, input_seen) = mpsc::sync_channel(16);
    let capture = host.open_input(
        Some(host.id()),
        Box::new(move |samples, format| {
            let peak = samples.iter().copied().map(f32::abs).fold(0.0f32, f32::max);
            let _ = input.try_send((samples.len(), format, peak));
        }),
        Box::new(|error| eprintln!("input failed: {error}")),
    )?;
    let observed = input_seen.recv_timeout(Duration::from_secs(10))?;
    println!(
        "D0 input id={} samples={} format={:?} peak={} elapsed={:?}",
        capture.device.id,
        observed.0,
        observed.1,
        observed.2,
        started.elapsed()
    );
    if host.failed() {
        return Err("a native stream failed".into());
    }
    drop(capture);
    drop(stream);
    assert_eq!(host.active(), 0);
    let refused = host.open_output(
        Some(&DeviceId("deliberately-missing".into())),
        Box::new(|_, _| panic!("refused output called producer")),
        Box::new(|_| {}),
    );
    assert!(refused.is_err());
    assert_eq!(host.active(), 0);
    println!(
        "D0 passed: explicit silent streams and refusal, elapsed={:?}",
        started.elapsed()
    );
    Ok(())
}
