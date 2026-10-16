use crate::scene::{Person, Room, account};
use junto_media::camera::{
    CameraAccess, CameraCapture, CameraDevice, CameraError, CameraSink, CameraState,
};
use junto_media::share::{CaptureStream, Pending};
use livekit::webrtc::video_frame::VideoBuffer;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

#[derive(Clone, Copy, Debug, Default)]
struct Progress {
    frames: usize,
    stopped: usize,
    invalid: bool,
    dimensions: (u32, u32),
    configured: (u32, u32),
}

struct Observed {
    native: Arc<dyn CameraCapture>,
    progress: watch::Sender<Progress>,
}
struct Sink {
    inner: Arc<dyn CameraSink>,
    progress: watch::Sender<Progress>,
}
impl CameraSink for Sink {
    fn picture(&self, buffer: Arc<dyn VideoBuffer>) {
        let dimensions = (buffer.width(), buffer.height());
        let valid = junto_media::share::picture::convert(&*buffer, (32, 24)).is_some();
        self.progress.send_modify(|p| {
            p.frames += 1;
            p.dimensions = dimensions;
            p.invalid |= !valid || dimensions.0 == 0 || dimensions.1 == 0;
        });
        self.inner.picture(buffer);
    }
    fn ended(&self, error: CameraError) {
        self.inner.ended(error);
    }
}
impl Drop for Sink {
    fn drop(&mut self) {
        self.progress.send_modify(|p| p.stopped += 1);
    }
}
impl CameraCapture for Observed {
    fn access(&self) -> CameraAccess {
        self.native.access()
    }
    fn request_access(&self) -> Pending<CameraAccess> {
        // The native stage never prompts, including after permission changes.
        let state = self.native.access();
        Box::pin(async move { state })
    }
    fn devices(&self) -> Result<Vec<CameraDevice>, CameraError> {
        self.native.devices()
    }
    fn start(
        &self,
        device: Option<String>,
        sink: Arc<dyn CameraSink>,
    ) -> Pending<Result<CaptureStream, CameraError>> {
        let result = self.native.start(
            device,
            Arc::new(Sink {
                inner: sink,
                progress: self.progress.clone(),
            }),
        );
        let progress = self.progress.clone();
        Box::pin(async move {
            let stream = result.await?;
            progress.send_modify(|p| p.configured = (stream.width, stream.height));
            Ok(stream)
        })
    }
}

pub async fn capture() {
    let started = Instant::now();
    let native = junto_media::camera::native_for_test();
    assert_eq!(
        native.access(),
        CameraAccess::Allowed,
        "camera permission must already be granted"
    );
    let devices = native.devices().unwrap();
    let selected = if let Ok(id) = std::env::var("JUNTO_TEST_CAMERA_ID") {
        devices
            .iter()
            .find(|device| device.id == id)
            .expect("selected camera exists")
    } else {
        assert_eq!(
            devices.len(),
            1,
            "set JUNTO_TEST_CAMERA_ID when there is more than one camera"
        );
        &devices[0]
    };
    println!("C1 selected id={} name={}", selected.id, selected.name);
    let (progress, mut observed) = watch::channel(Progress::default());
    let mut local = Person::new(0.0);
    local.calls = junto_media::Calls::new(
        local.device.client.clone(),
        junto_media::MediaOptions {
            host: Arc::new(local.host.clone()),
            permission: Arc::new(junto_media::permission::FixedPermission::allowed()),
            camera: Arc::new(Observed { native, progress }),
            capture: Arc::new(junto_media::share::NoCapture),
            timing: Default::default(),
        },
    );
    local.calls.set_voice(crate::scene::plain_voice());
    let peer = Person::new(0.0);
    let (room, owner) = Room::new("Native camera", &local, "Native").await;
    room.member(&owner, &peer, "Peer").await;
    local.enter(room.target).await;
    peer.enter(room.target).await;
    let remote_id = junto_media::engine::ShareId::Of(account(&owner));
    peer.calls.set_camera_view(remote_id, (640, 480));
    for session in 1..=2 {
        let before = *observed.borrow();
        local.calls.start_camera(Some(selected.id.clone()));
        super::calls::state(&local, |v| {
            v.call
                .as_ref()
                .is_some_and(|c| c.camera == CameraState::Running)
        })
        .await;
        let mut pictures = peer.calls.camera_pictures();
        let mut received = 0;
        tokio::time::timeout(Duration::from_secs(10), async {
            while received < 10 {
                pictures.changed().await.unwrap();
                let pictures = pictures.borrow_and_update();
                if let Some(frame) = pictures.get(&remote_id) {
                    assert!(frame.width > 0 && frame.height > 0);
                    assert!(!frame.mirrored);
                    assert!(frame.picture().is_some(), "remote native frame converts");
                    received += 1;
                }
            }
        })
        .await
        .expect("ten remote camera frames within ten seconds");
        let during = *observed.borrow();
        assert!(during.frames >= before.frames + 10);
        assert!(!during.invalid);
        assert_eq!(
            during.dimensions, during.configured,
            "camera frames must match the declared capture dimensions"
        );
        local.calls.stop_camera();
        tokio::time::timeout(
            Duration::from_secs(2),
            observed.wait_for(|p| p.stopped == session),
        )
        .await
        .expect("native camera shutdown within two seconds")
        .unwrap();
        println!(
            "C1 session={session} native_frames={} remote_frames={received} dimensions={:?} elapsed={:?}",
            during.frames - before.frames,
            during.dimensions,
            started.elapsed()
        );
    }
    local.calls.leave().await;
    peer.calls.leave().await;
    println!(
        "C1 passed: native publication, conversion, remote progression and completed shutdown; C2 NOT RUN: assisted target unavailable"
    );
}
