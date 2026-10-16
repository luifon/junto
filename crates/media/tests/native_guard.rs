#![allow(clippy::unwrap_used)]

#[path = "native/guard.rs"]
mod guard;

use guard::Guard;
use junto_media::audio::host::{Deliver, Failed, Fill};
use junto_media::audio::{
    AudioError, AudioHost, DeviceId, DeviceInfo, Devices, Format, OpenStream,
};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Fake {
    opens: Mutex<Vec<Option<DeviceId>>>,
    wrong: bool,
    during_open: Mutex<Vec<f32>>,
    fill: Mutex<Option<Fill>>,
    failure: Mutex<Option<Failed>>,
}

fn id() -> DeviceId {
    DeviceId("verified-virtual-device".into())
}
fn info(id: DeviceId) -> DeviceInfo {
    DeviceInfo {
        id,
        name: "Virtual".into(),
        is_default: false,
    }
}

impl AudioHost for Fake {
    fn devices(&self) -> Result<Devices, AudioError> {
        Ok(Devices {
            inputs: vec![info(id())],
            outputs: vec![info(id())],
        })
    }
    fn open_input(
        &self,
        chosen: Option<&DeviceId>,
        _: Deliver,
        _: Failed,
    ) -> Result<OpenStream, AudioError> {
        self.opens.lock().unwrap().push(chosen.cloned());
        Ok(OpenStream::new(info(id()), ()))
    }
    fn open_output(
        &self,
        chosen: Option<&DeviceId>,
        mut fill: Fill,
        failed: Failed,
    ) -> Result<OpenStream, AudioError> {
        self.opens.lock().unwrap().push(chosen.cloned());
        let mut data = vec![0.; 4];
        fill(
            &mut data,
            Format {
                sample_rate: 48_000,
                channels: 2,
            },
        );
        *self.during_open.lock().unwrap() = data;
        *self.fill.lock().unwrap() = Some(fill);
        *self.failure.lock().unwrap() = Some(failed);
        Ok(OpenStream::new(
            info(if self.wrong {
                DeviceId("other".into())
            } else {
                id()
            }),
            (),
        ))
    }
}

#[test]
fn default_missing_and_other_devices_never_reach_the_host() {
    for chosen in [
        None,
        Some(DeviceId("gone".into())),
        Some(DeviceId("speakers".into())),
    ] {
        let host = Arc::new(Fake::default());
        let guarded = Guard::new(host.clone(), || Ok(id())).unwrap();
        assert!(
            guarded
                .open_output(
                    chosen.as_ref(),
                    Box::new(|data, _| data.fill(0.5)),
                    Box::new(|_| {})
                )
                .is_err()
        );
        let input_guard = Guard::new(host.clone(), || Ok(id())).unwrap();
        assert!(
            input_guard
                .open_input(chosen.as_ref(), Box::new(|_, _| {}), Box::new(|_| {}))
                .is_err()
        );
        assert!(host.opens.lock().unwrap().is_empty());
    }
}

#[test]
fn output_is_zero_until_the_returned_route_is_verified() {
    let host = Arc::new(Fake::default());
    let guarded = Guard::new(host.clone(), || Ok(id())).unwrap();
    assert_eq!(guarded.id(), &id());
    assert_eq!(guarded.active(), 0);
    let _stream = guarded
        .open_output(
            Some(&id()),
            Box::new(|data, _| data.fill(0.5)),
            Box::new(|_| {}),
        )
        .unwrap();
    assert_eq!(guarded.active(), 1);
    assert_eq!(*host.during_open.lock().unwrap(), vec![0.; 4]);
    drop(_stream);
    assert_eq!(guarded.active(), 0);
}

#[test]
fn a_stream_failure_disarms_output_and_refuses_recovery() {
    let host = Arc::new(Fake::default());
    let guarded = Guard::new(host.clone(), || Ok(id())).unwrap();
    let _stream = guarded
        .open_output(
            Some(&id()),
            Box::new(|data, _| data.fill(0.5)),
            Box::new(|_| {}),
        )
        .unwrap();
    let format = Format {
        sample_rate: 48_000,
        channels: 2,
    };
    let mut samples = [0.; 4];
    host.fill.lock().unwrap().as_mut().unwrap()(&mut samples, format);
    assert_eq!(samples, [0.5; 4]);
    host.failure.lock().unwrap().as_mut().unwrap()(AudioError::NoDevice);
    assert!(guarded.failed());
    host.fill.lock().unwrap().as_mut().unwrap()(&mut samples, format);
    assert_eq!(samples, [0.; 4]);
    assert!(
        guarded
            .open_output(Some(&id()), Box::new(|_, _| {}), Box::new(|_| {}))
            .is_err()
    );
    assert_eq!(host.opens.lock().unwrap().len(), 1);
}

#[test]
fn a_changed_or_disappeared_identity_refuses_reopen() {
    for changed in [
        Err(AudioError::NoDevice),
        Ok(DeviceId("replacement".into())),
    ] {
        let current = Arc::new(Mutex::new(Ok(id())));
        let verify = current.clone();
        let host = Arc::new(Fake::default());
        let guarded = Guard::new(host.clone(), move || verify.lock().unwrap().clone()).unwrap();
        drop(
            guarded
                .open_input(Some(&id()), Box::new(|_, _| {}), Box::new(|_| {}))
                .unwrap(),
        );
        *current.lock().unwrap() = changed;
        assert!(
            guarded
                .open_output(Some(&id()), Box::new(|_, _| {}), Box::new(|_| {}))
                .is_err()
        );
        assert_eq!(host.opens.lock().unwrap().len(), 1);
        *current.lock().unwrap() = Ok(id());
        assert!(
            guarded
                .open_input(Some(&id()), Box::new(|_, _| {}), Box::new(|_| {}))
                .is_err()
        );
        assert_eq!(host.opens.lock().unwrap().len(), 1);
    }
}

#[test]
fn a_wrong_returned_route_fails_with_only_silence_generated() {
    let host = Arc::new(Fake {
        wrong: true,
        ..Fake::default()
    });
    let guarded = Guard::new(host.clone(), || Ok(id())).unwrap();
    assert!(
        guarded
            .open_output(
                Some(&id()),
                Box::new(|data, _| data.fill(0.5)),
                Box::new(|_| {})
            )
            .is_err()
    );
    assert_eq!(*host.during_open.lock().unwrap(), vec![0.; 4]);
}
