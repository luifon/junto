use super::synthetic::SyntheticCamera;
use super::*;
use std::sync::{Mutex, PoisonError};

#[derive(Default)]
struct Sink {
    pictures: Mutex<Vec<crate::share::picture::PictureFrame>>,
    errors: Mutex<Vec<CameraError>>,
}
impl CameraSink for Sink {
    fn picture(&self, buffer: Arc<dyn VideoBuffer>) {
        if let Some(frame) = crate::share::picture::convert(buffer.as_ref(), (320, 180)) {
            self.pictures
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(frame);
        }
    }
    fn ended(&self, error: CameraError) {
        self.errors
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(error);
    }
}

#[test]
fn camera_choice_never_falls_back_to_another_device() {
    let devices = vec![
        CameraDevice {
            id: "first".into(),
            name: "First camera".into(),
        },
        CameraDevice {
            id: "second".into(),
            name: "Second camera".into(),
        },
    ];
    assert_eq!(choose(&devices, None).unwrap(), devices[0]);
    assert_eq!(choose(&devices, Some("second")).unwrap(), devices[1]);
    assert_eq!(choose(&devices, Some("gone")), Err(CameraError::DeviceGone));
    assert_eq!(choose(&[], None), Err(CameraError::DeviceGone));
}

#[tokio::test]
async fn camera_permission_states_are_explicit() {
    let camera = SyntheticCamera::new();
    for access in [
        CameraAccess::NotAsked,
        CameraAccess::Refused,
        CameraAccess::Restricted,
    ] {
        camera.set_access(access);
        assert_eq!(camera.access(), access);
        assert!(
            matches!(camera.start(None, Arc::new(Sink::default())).await, Err(CameraError::NoPermission(found)) if found == access)
        );
        assert_eq!(camera.access(), access);
    }
    camera.set_access(CameraAccess::NotAsked);
    assert_eq!(camera.request_access().await, CameraAccess::Allowed);
}

#[tokio::test]
async fn camera_starts_only_on_request_and_stop_closes_capture() {
    let camera = SyntheticCamera::new();
    assert_eq!(camera.active(), 0);
    assert_eq!(camera.devices().unwrap().len(), 1);
    assert_eq!(camera.active(), 0);
    let sink = Arc::new(Sink::default());
    let stream = camera.start(None, sink.clone()).await.unwrap();
    assert_eq!(camera.active(), 1);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while sink.pictures.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(stream);
    assert_eq!(camera.active(), 0);
    let pictures = sink.pictures.lock().unwrap();
    let frame = &pictures[0];
    let (left, right) = (frame.pixel(20, 20).unwrap(), frame.pixel(300, 20).unwrap());
    assert!(left.0 > 150 && left.2 < 70, "{left:?}");
    assert!(right.2 > 150 && right.0 < 70, "{right:?}");
}

#[tokio::test]
async fn camera_device_loss_is_visible() {
    let camera = SyntheticCamera::new();
    let sink = Arc::new(Sink::default());
    let stream = camera.start(None, sink.clone()).await.unwrap();
    camera.set_devices(vec![]);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while sink.errors.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*sink.errors.lock().unwrap(), [CameraError::DeviceGone]);
    drop(stream);
    assert_eq!(camera.active(), 0);
}

#[test]
fn only_own_camera_preview_is_mirrored() {
    let original = crate::share::picture::PictureFrame {
        width: 3,
        height: 2,
        bgra: vec![
            1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255, 13, 14, 15, 255, 16, 17, 18,
            255,
        ]
        .into(),
        source: (3, 2),
    };
    let preview = mirror(&original);
    assert_eq!(preview.pixel(0, 0), original.pixel(2, 0));
    assert_eq!(preview.pixel(2, 1), original.pixel(0, 1));
    assert_eq!(original.pixel(0, 0), Some((3, 2, 1)));
    assert_eq!(preview.source, original.source);
}

#[cfg(target_os = "macos")]
#[test]
fn camera_surface_uses_the_full_range_nv12_format_of_the_renderer() {
    let pixels = crate::share::synthetic::two_colours(64, 36, (200, 30, 30), (30, 30, 200));
    let frame = CameraFrame::new(&pixels, true, (64, 36)).unwrap();
    let surface = frame.surface().unwrap();
    assert_eq!(
        surface.get_pixel_format(),
        core_video::pixel_buffer::kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
    );
    assert_eq!((surface.get_width(), surface.get_height()), (64, 36));
}
