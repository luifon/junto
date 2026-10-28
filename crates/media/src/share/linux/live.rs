#![allow(clippy::unwrap_used)]

use std::sync::atomic::{AtomicUsize, Ordering};

use livekit::webrtc::video_frame::VideoBuffer;

use super::*;
use crate::share::{Resolution, ShareKind, ShareQuality};

#[derive(Default)]
struct Pictures {
    count: AtomicUsize,
    latest: Mutex<Option<crate::share::picture::PictureFrame>>,
    ended: Mutex<Vec<CaptureError>>,
}
impl CaptureSink for Pictures {
    fn picture(&self, frame: Arc<dyn VideoBuffer>) {
        let picture = crate::share::picture::convert(frame.as_ref(), (1920, 1080)).unwrap();
        *self.latest.lock().unwrap() = Some(picture);
        self.count.fetch_add(1, Ordering::Relaxed);
    }
    fn sound(&self, _: &[f32], _: Format) {
        panic!("sound was not selected");
    }
    fn ended(&self, error: CaptureError) {
        self.ended.lock().unwrap().push(error);
    }
}

fn background(colour: &str) {
    let output = std::process::Command::new("swaymsg")
        .args(["output", "HEADLESS-1", "bg", colour, "solid_color"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
}

async fn colour(pictures: &Pictures, expected: (u8, u8, u8)) {
    let started = Instant::now();
    loop {
        assert!(
            pictures.ended.lock().unwrap().is_empty(),
            "{:?}",
            pictures.ended.lock().unwrap()
        );
        let matches = pictures
            .latest
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|picture| {
                let pixel = picture
                    .pixel(picture.width / 2, picture.height / 2)
                    .unwrap();
                [pixel.0, pixel.1, pixel.2]
                    .into_iter()
                    .zip([expected.0, expected.1, expected.2])
                    .all(|(got, expected)| (i16::from(got) - i16::from(expected)).abs() <= 5)
            });
        if matches {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "the selected output colour did not arrive"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
#[ignore = "needs the private virtual portal: tools/linux/display.sh wayland tools/linux/portal.sh"]
async fn the_real_portal_captures_the_virtual_output_and_drop_stops_delivery() {
    assert_eq!(
        std::env::var("JUNTO_TEST_VIRTUAL_PORTAL").as_deref(),
        Ok("1")
    );
    let capture = SystemCapture;
    assert_eq!(capture.access(), ScreenAccess::AskOnStart);
    let sources = capture.sources().await.unwrap();
    assert_eq!(sources.portal, [ShareKind::Screen]);
    let pictures = Arc::new(Pictures::default());
    background("#b42040");
    let stream = capture
        .start(
            CaptureRequest {
                picture: PictureSource::Portal(ShareKind::Screen),
                sound: SoundChoice::None,
                quality: ShareQuality {
                    resolution: Resolution::P720,
                    frame_rate: 30,
                },
                dialog_parent: None,
            },
            pictures.clone(),
        )
        .await
        .unwrap();
    assert_eq!((stream.width, stream.height), (1280, 720));
    colour(&pictures, (180, 32, 64)).await;
    background("#2060b4");
    colour(&pictures, (32, 96, 180)).await;
    assert!(pictures.count.load(Ordering::Relaxed) >= 2);
    drop(stream);
    let stopped = pictures.count.load(Ordering::Relaxed);
    background("#ffffff");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(pictures.count.load(Ordering::Relaxed), stopped);
    assert!(pictures.ended.lock().unwrap().is_empty());
}
