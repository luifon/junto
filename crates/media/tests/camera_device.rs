//! Opt-in Windows/Linux device check; listing never starts capture or requests permission.

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    use junto_media::camera::{CameraAccess, CameraError, CameraSink, native_for_test};
    use livekit::webrtc::video_frame::VideoBuffer;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    enum Event {
        Frame(u32, u32),
        Ended(CameraError),
    }
    struct Sink(tokio::sync::mpsc::UnboundedSender<Event>, Mutex<usize>);
    impl CameraSink for Sink {
        fn picture(&self, frame: Arc<dyn VideoBuffer>) {
            *self
                .1
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) += 1;
            let _ = self.0.send(Event::Frame(frame.width(), frame.height()));
        }
        fn ended(&self, error: CameraError) {
            let _ = self.0.send(Event::Ended(error));
        }
    }
    let camera = native_for_test();
    let devices = camera.devices()?;
    println!(
        "Camera devices: {}; access: {:?}",
        devices.len(),
        camera.access()
    );
    if std::env::args().any(|arg| arg == "--list") {
        return Ok(());
    }
    if std::env::var("JUNTO_TEST_CAMERA_DEVICE").as_deref() != Ok("1") {
        return Err("set JUNTO_TEST_CAMERA_DEVICE=1 to authorize this device check".into());
    }
    if devices.is_empty() {
        return Err("no camera exists; native capture was not tested".into());
    }
    if camera.access() != CameraAccess::Allowed {
        return Err("camera permission is not already granted; no request was made".into());
    }
    let index: usize = std::env::var("JUNTO_TEST_CAMERA_INDEX")
        .unwrap_or_else(|_| "0".into())
        .parse()?;
    let chosen = devices
        .get(index)
        .ok_or("camera index is absent")?
        .id
        .clone();
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let (send, mut receive) = tokio::sync::mpsc::unbounded_channel();
        let sink = Arc::new(Sink(send, Mutex::new(0)));
        assert!(matches!(
            camera
                .start(Some("absent-test-camera".into()), sink.clone())
                .await,
            Err(CameraError::DeviceGone)
        ));
        let stream = camera.start(Some(chosen), sink.clone()).await?;
        let size = (stream.width, stream.height);
        tokio::time::timeout(Duration::from_secs(10), async {
            for _ in 0..30 {
                match receive.recv().await.ok_or("camera delivery closed")? {
                    Event::Frame(w, h) if (w, h) == size => {}
                    Event::Frame(_, _) => return Err("frame size differs from capture size".into()),
                    Event::Ended(error) => return Err(Box::<dyn std::error::Error>::from(error)),
                }
            }
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .await??;
        drop(stream);
        println!(
            "Received 30 frames at {} by {}; capture stopped",
            size.0, size.1
        );
        Ok::<_, Box<dyn std::error::Error>>(())
    })
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn main() {
    eprintln!("This device check is for Windows and Linux; use the native camera stage on macOS.");
}
