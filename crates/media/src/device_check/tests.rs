#![allow(clippy::unwrap_used)]
use super::*;
use crate::audio::{
    DeviceId, Processing,
    synthetic::{SyntheticHost, Tone, tone_level},
};
use crate::camera::{CameraCapture, CameraDevice, synthetic::SyntheticCamera};
use crate::permission::FixedPermission;
use std::time::Duration;

async fn until(what: &str, ready: impl Fn() -> bool) {
    tokio::time::timeout(Duration::from_secs(20), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what}"));
}

fn options(host: Arc<dyn crate::audio::AudioHost>, camera: Arc<dyn CameraCapture>) -> MediaOptions {
    MediaOptions {
        host,
        camera,
        permission: Arc::new(FixedPermission::allowed()),
        capture: Arc::new(crate::share::NoCapture),
        timing: Default::default(),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_microphone_check_plays_the_chosen_input_on_the_chosen_output() {
    let host = SyntheticHost::with_tone(440.0);
    host.add_input("headset", "Headset", Tone::new(880.0));
    host.add_output("headphones", "Headphones");
    let levels = watch::Sender::new(Levels::default());
    let checks = DeviceChecks::new(
        options(Arc::new(host.clone()), Arc::new(SyntheticCamera::new())),
        tokio::runtime::Handle::current(),
        levels.clone(),
    );
    checks.start_audio(
        AudioCheckKind::Microphone,
        VoiceConfig {
            input_device: Some(DeviceId("headset".into())),
            output_device: Some(DeviceId("headphones".into())),
            processing: Processing::OFF,
            ..Default::default()
        },
    );
    until("the chosen microphone is heard", || {
        let (samples, rate) = host.played(Duration::from_millis(100));
        tone_level(&samples, rate, 880.0) > -35.0 && levels.borrow().input > 0.0
    })
    .await;
    assert_eq!(host.opened_inputs(), vec![Some(DeviceId("headset".into()))]);
    assert_eq!(
        host.opened_outputs(),
        vec![Some(DeviceId("headphones".into()))]
    );
    assert_eq!(
        checks.view().audio,
        AudioCheckState::Running(AudioCheckKind::Microphone)
    );
    checks.stop_audio();
    until("the stopped microphone meter clears", || {
        levels.borrow().input == 0.0
    })
    .await;
    assert_eq!(checks.view().audio, AudioCheckState::Off);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_output_check_plays_a_tone_without_opening_a_microphone_and_reports_failures() {
    let host = SyntheticHost::with_tone(440.0);
    let checks = DeviceChecks::new(
        options(Arc::new(host.clone()), Arc::new(SyntheticCamera::new())),
        tokio::runtime::Handle::current(),
        watch::Sender::new(Levels::default()),
    );
    checks.start_audio(AudioCheckKind::Output, VoiceConfig::default());
    until("the output test is heard", || {
        let (samples, rate) = host.played(Duration::from_millis(100));
        tone_level(&samples, rate, 660.0) > -45.0
    })
    .await;
    assert!(host.opened_inputs().is_empty());
    host.break_output(AudioError::Refused);
    until("the output failure is visible", || {
        checks.view().audio == AudioCheckState::Failed(AudioError::Refused)
    })
    .await;
    checks.start_audio(AudioCheckKind::Output, VoiceConfig::default());
    until("the output can retry", || {
        checks.view().audio == AudioCheckState::Running(AudioCheckKind::Output)
    })
    .await;
    checks.stop();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_camera_preview_is_local_mirrored_and_ends_when_the_device_is_removed() {
    let camera = SyntheticCamera::new();
    camera.set_access(CameraAccess::NotAsked);
    camera.set_devices(vec![CameraDevice {
        id: "desk".into(),
        name: "Desk camera".into(),
    }]);
    let checks = DeviceChecks::new(
        options(
            Arc::new(SyntheticHost::with_tone(440.0)),
            Arc::new(camera.clone()),
        ),
        tokio::runtime::Handle::current(),
        watch::Sender::new(Levels::default()),
    );
    let pictures = checks.picture();
    drop(checks.start_camera(Some("desk".into())));
    until("local camera preview arrives", || {
        pictures.borrow().is_some() && checks.view().camera == CameraState::Running
    })
    .await;
    assert!(pictures.borrow().as_ref().unwrap().mirrored);
    assert_eq!(camera.active(), 1);
    camera.set_devices(Vec::new());
    until("removed camera is reported", || {
        checks.view().camera == CameraState::Failed(CameraError::DeviceGone)
            && pictures.borrow().is_none()
    })
    .await;
    until("failed capture closes", || camera.active() == 0).await;
    camera.set_devices(vec![CameraDevice {
        id: "desk".into(),
        name: "Desk camera".into(),
    }]);
    drop(checks.start_camera(None));
    until("camera retry arrives", || pictures.borrow().is_some()).await;
    checks.stop();
    until("preview closes", || camera.active() == 0).await;
    assert!(pictures.borrow().is_none());
    assert_eq!(checks.view(), DeviceCheckView::default());
}

struct WaitingCamera {
    camera: SyntheticCamera,
    asked: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    answer: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}
impl CameraCapture for WaitingCamera {
    fn access(&self) -> CameraAccess {
        CameraAccess::NotAsked
    }
    fn request_access(&self) -> crate::share::Pending<CameraAccess> {
        let asked = self.asked.lock().unwrap().take().unwrap();
        let answer = self.answer.lock().unwrap().take().unwrap();
        Box::pin(async move {
            let _ = asked.send(());
            let _ = answer.await;
            CameraAccess::Allowed
        })
    }
    fn devices(&self) -> Result<Vec<CameraDevice>, CameraError> {
        self.camera.devices()
    }
    fn start(
        &self,
        device: Option<String>,
        sink: Arc<dyn CameraSink>,
    ) -> crate::share::Pending<Result<CaptureStream, CameraError>> {
        self.camera.start(device, sink)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_permission_answer_after_close_never_opens_a_preview() {
    let camera = SyntheticCamera::new();
    let (asked, question) = tokio::sync::oneshot::channel();
    let (release, answer) = tokio::sync::oneshot::channel();
    let checks = DeviceChecks::new(
        options(
            Arc::new(SyntheticHost::with_tone(440.0)),
            Arc::new(WaitingCamera {
                camera: camera.clone(),
                asked: Mutex::new(Some(asked)),
                answer: Mutex::new(Some(answer)),
            }),
        ),
        tokio::runtime::Handle::current(),
        watch::Sender::new(Levels::default()),
    );
    let pending = checks.start_camera(None);
    tokio::time::timeout(Duration::from_secs(20), question)
        .await
        .unwrap()
        .unwrap();
    checks.stop();
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(20), pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(camera.started(), 0);
    assert_eq!(checks.view().camera, CameraState::Off);
    assert!(checks.picture().borrow().is_none());
}

struct WaitingOutput {
    output: crate::audio::test_output::TestOutput,
    opening: tokio::sync::mpsc::UnboundedSender<()>,
}
impl crate::audio::AudioHost for WaitingOutput {
    fn devices(&self) -> Result<crate::audio::Devices, AudioError> {
        self.output.devices()
    }
    fn open_input(
        &self,
        device: Option<&DeviceId>,
        deliver: crate::audio::host::Deliver,
        failed: crate::audio::host::Failed,
    ) -> Result<crate::audio::OpenStream, AudioError> {
        self.output.open_input(device, deliver, failed)
    }
    fn open_output(
        &self,
        device: Option<&DeviceId>,
        fill: crate::audio::host::Fill,
        failed: crate::audio::host::Failed,
    ) -> Result<crate::audio::OpenStream, AudioError> {
        let _ = self.opening.send(());
        let (filled, taken) = std::sync::mpsc::sync_channel(1);
        let mut fill = fill;
        let stream = self.output.open_output(
            device,
            Box::new(move |samples, format| {
                fill(samples, format);
                let _ = filled.try_send(());
            }),
            failed,
        )?;
        taken
            .recv_timeout(Duration::from_secs(20))
            .map_err(|error| AudioError::Failed(error.to_string()))?;
        Ok(stream)
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_output_that_opens_after_close_is_silent_and_is_released() {
    let host = SyntheticHost::with_tone(440.0);
    let (release, held) = std::sync::mpsc::channel();
    let (closed, ended) = std::sync::mpsc::channel();
    let (opening, mut started) = tokio::sync::mpsc::unbounded_channel();
    let checks = DeviceChecks::new(
        options(
            Arc::new(WaitingOutput {
                output: crate::audio::test_output::TestOutput {
                    host: host.clone(),
                    release: Some(Mutex::new(held)),
                    closed,
                },
                opening,
            }),
            Arc::new(SyntheticCamera::new()),
        ),
        tokio::runtime::Handle::current(),
        watch::Sender::new(Levels::default()),
    );
    checks.start_audio(AudioCheckKind::Output, VoiceConfig::default());
    tokio::time::timeout(Duration::from_secs(20), started.recv())
        .await
        .unwrap()
        .unwrap();
    checks.stop();
    release.send(()).unwrap();
    tokio::task::spawn_blocking(move || ended.recv_timeout(Duration::from_secs(20)))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(checks.view(), DeviceCheckView::default());
    let (samples, _) = host.played_since_clear();
    assert!(!samples.is_empty(), "the late output callback actually ran");
    assert!(samples.iter().all(|sample| sample.abs() < 0.0001));
}
