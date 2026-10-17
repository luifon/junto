//! The capture of Windows against a window of a separate program (ADR 0706). The tests run
//! in the Windows job of CI, in the desktop session of its account: the window of the
//! fixture appears above the other windows for a few seconds and takes no focus. No test
//! plays sound.

#![cfg(target_os = "windows")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use junto_media::audio::Format;
use junto_media::share::picture::{PictureFrame, convert};
use junto_media::share::{
    CaptureError, CaptureRequest, CaptureSink, PictureSource, Resolution, ScreenAccess,
    ShareQuality, SoundChoice,
};
use livekit::webrtc::video_frame::VideoBuffer;

/// The colours of the quarters of the fixture: top left, top right, bottom left, bottom
/// right. After `turn` was sent `k` times, quarter `q` shows colour `(q + k) % 4`.
const COLOURS: [(u8, u8, u8); 4] = [(200, 40, 40), (40, 180, 60), (40, 60, 200), (220, 200, 40)];
/// The difference of each colour value that the conversion to video and back may make.
const TOLERANCE: u8 = 12;
const PATIENCE: Duration = Duration::from_secs(20);

/// One fixture at a time: each test expects its window above the others.
static SCREEN: Mutex<()> = Mutex::new(());
static OPENED: AtomicUsize = AtomicUsize::new(0);

/// The window of the fixture program. Its process ends when the value is dropped.
struct Fixture {
    _screen: MutexGuard<'static, ()>,
    child: Child,
    input: ChildStdin,
    title: String,
    window: u32,
    screen: u32,
    /// The inner area in pixels of its screen: left, top, right, bottom.
    area: (i32, i32, i32, i32),
}

impl Fixture {
    fn open() -> Self {
        let screen = SCREEN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let title = format!(
            "Junto share fixture {} {}",
            std::process::id(),
            OPENED.fetch_add(1, Ordering::Relaxed)
        );
        let mut child = Command::new(env!("CARGO_BIN_EXE_junto-share-fixture"))
            .arg(&title)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("the fixture program starts");
        let input = child.stdin.take().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        let words: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(words.first(), Some(&"ready"), "the fixture wrote {line:?}");
        let number = |at: usize| -> i64 { words[at].parse().unwrap() };
        Self {
            _screen: screen,
            child,
            input,
            title,
            window: number(1) as u32,
            screen: number(2) as u32,
            area: (
                number(3) as i32,
                number(4) as i32,
                number(5) as i32,
                number(6) as i32,
            ),
        }
    }

    fn send(&mut self, command: &str) {
        writeln!(self.input, "{command}").unwrap();
        self.input.flush().unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Default)]
struct Pictures {
    latest: Mutex<Option<PictureFrame>>,
    count: Mutex<usize>,
    sound: Mutex<Vec<(usize, Format, f32)>>,
    ended: Mutex<Vec<CaptureError>>,
}

impl CaptureSink for Pictures {
    fn picture(&self, buffer: Arc<dyn VideoBuffer>) {
        let size = (buffer.width(), buffer.height());
        *self.latest.lock().unwrap() = convert(buffer.as_ref(), size);
        *self.count.lock().unwrap() += 1;
    }

    fn sound(&self, samples: &[f32], format: Format) {
        let peak = samples
            .iter()
            .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
        self.sound
            .lock()
            .unwrap()
            .push((samples.len(), format, peak));
    }

    fn ended(&self, error: CaptureError) {
        self.ended.lock().unwrap().push(error);
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
}

fn near(got: (u8, u8, u8), expected: (u8, u8, u8)) -> bool {
    got.0.abs_diff(expected.0) <= TOLERANCE
        && got.1.abs_diff(expected.1) <= TOLERANCE
        && got.2.abs_diff(expected.2) <= TOLERANCE
}

/// The middle of each quarter of an area, as fractions of the picture of that area.
fn quarters(area: (f64, f64, f64, f64)) -> [(f64, f64); 4] {
    let (left, top, right, bottom) = area;
    let (width, height) = (right - left, bottom - top);
    [
        (left + width * 0.25, top + height * 0.25),
        (left + width * 0.75, top + height * 0.25),
        (left + width * 0.25, top + height * 0.75),
        (left + width * 0.75, top + height * 0.75),
    ]
}

/// Waits until the newest picture shows the colours of the quarters after `turns` turns at
/// the places that `places` gives for a picture of a size.
fn until_colours(
    pictures: &Pictures,
    turns: usize,
    places: impl Fn(&PictureFrame) -> [(f64, f64); 4],
    what: &str,
) {
    let started = Instant::now();
    let mut seen = Vec::new();
    loop {
        assert!(
            pictures.ended.lock().unwrap().is_empty(),
            "{:?}",
            pictures.ended.lock().unwrap()
        );
        if let Some(picture) = pictures.latest.lock().unwrap().as_ref() {
            seen = places(picture)
                .iter()
                .map(|(x, y)| picture.pixel(*x as u32, *y as u32).unwrap_or_default())
                .collect();
            let wanted: Vec<_> = (0..4)
                .map(|quarter| COLOURS[(quarter + turns) % 4])
                .collect();
            if seen
                .iter()
                .zip(&wanted)
                .all(|(got, expected)| near(*got, *expected))
            {
                return;
            }
        }
        assert!(
            started.elapsed() < PATIENCE,
            "{what}: the quarters show {seen:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn the_lists_name_the_screens_the_window_of_another_program_and_its_application() {
    let fixture = Fixture::open();
    let capture = junto_media::share::system();
    assert!(capture.supported());
    assert_eq!(capture.access(), ScreenAccess::Allowed);
    let sources = runtime().block_on(capture.sources()).unwrap();
    assert!(sources.system_sound);
    assert!(sources.portal.is_empty());
    let screen = sources
        .displays
        .iter()
        .find(|display| display.id == fixture.screen)
        .expect("the screen of the fixture is listed");
    assert!(screen.width >= 640 && screen.height >= 400, "{screen:?}");
    let window = sources
        .windows
        .iter()
        .find(|window| window.title == fixture.title)
        .expect("the window of the fixture is listed");
    assert_eq!(window.id, fixture.window);
    assert_eq!(window.application.process_id, fixture.child.id() as i32);
    assert!(
        window
            .application
            .bundle_id
            .ends_with(r"\junto-share-fixture.exe"),
        "{window:?}"
    );
    assert_eq!(window.application_name, "junto-share-fixture");
    // The visible frame holds the inner area of 640 by 400 pixels and the title bar.
    assert!(window.width >= 640 && window.height > 400, "{window:?}");
    assert!(
        sources
            .applications
            .iter()
            .any(|application| application.id == window.application)
    );
    // The program that runs the test is the client here: none of its windows and not its
    // own application is offered.
    let own = std::process::id() as i32;
    assert!(
        sources
            .windows
            .iter()
            .all(|window| window.application.process_id != own)
    );
    assert!(
        sources
            .applications
            .iter()
            .all(|application| application.id.process_id != own)
    );
}

#[test]
fn a_window_share_shows_its_colours_follows_a_change_and_ends_when_the_window_closes() {
    let mut fixture = Fixture::open();
    let capture = junto_media::share::system();
    let runtime = runtime();
    let pictures = Arc::new(Pictures::default());
    let stream = runtime
        .block_on(capture.start(
            CaptureRequest {
                picture: PictureSource::Window(fixture.window),
                sound: SoundChoice::None,
                quality: ShareQuality {
                    resolution: Resolution::Source,
                    frame_rate: 30,
                },
            },
            pictures.clone(),
        ))
        .unwrap();
    assert!(stream.width > 640 && stream.height > 400, "{stream:?}");
    // The picture of a window holds its frame: the inner area is its lower part, as wide as
    // the picture without the borders. The middles of the quarters are far from the edges.
    let area = fixture.area;
    let places = move |picture: &PictureFrame| {
        let (width, height) = (f64::from(picture.width), f64::from(picture.height));
        let inner = (area.2 - area.0, area.3 - area.1);
        let border = (width - f64::from(inner.0)) / 2.0;
        quarters((
            border,
            height - border - f64::from(inner.1),
            width - border,
            height - border,
        ))
    };
    until_colours(&pictures, 0, places, "the window share");
    fixture.send("turn");
    until_colours(&pictures, 1, places, "the window share after a turn");
    assert!(
        pictures.sound.lock().unwrap().is_empty(),
        "a share without sound gave sound"
    );

    fixture.send("close");
    let started = Instant::now();
    while pictures.ended.lock().unwrap().is_empty() {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "the share did not end with the window"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(*pictures.ended.lock().unwrap(), [CaptureError::SourceGone]);
    let count = *pictures.count.lock().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        *pictures.count.lock().unwrap(),
        count,
        "pictures came after the end"
    );
    drop(stream);
    assert_eq!(pictures.ended.lock().unwrap().len(), 1);
}

#[test]
fn a_screen_share_shows_the_window_where_it_is_and_stops_when_dropped() {
    let fixture = Fixture::open();
    let capture = junto_media::share::system();
    let runtime = runtime();
    let pictures = Arc::new(Pictures::default());
    let stream = runtime
        .block_on(capture.start(
            CaptureRequest {
                picture: PictureSource::Display(fixture.screen),
                sound: SoundChoice::None,
                quality: ShareQuality {
                    resolution: Resolution::Source,
                    frame_rate: 15,
                },
            },
            pictures.clone(),
        ))
        .unwrap();
    let area = fixture.area;
    until_colours(
        &pictures,
        0,
        |_| {
            quarters((
                f64::from(area.0),
                f64::from(area.1),
                f64::from(area.2),
                f64::from(area.3),
            ))
        },
        "the screen share",
    );
    drop(stream);
    let count = *pictures.count.lock().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        *pictures.count.lock().unwrap(),
        count,
        "pictures came after the drop"
    );
    // The owner stopped the share: the sink is not told an end.
    assert!(pictures.ended.lock().unwrap().is_empty());
}

#[test]
fn a_share_with_the_sound_of_one_application_or_of_the_system_starts_and_gives_stereo_at_48_khz() {
    let fixture = Fixture::open();
    let capture = junto_media::share::system();
    let runtime = runtime();
    let sources = runtime.block_on(capture.sources()).unwrap();
    let application = sources
        .windows
        .iter()
        .find(|window| window.id == fixture.window)
        .unwrap()
        .application
        .clone();
    for sound in [SoundChoice::Application(application), SoundChoice::System] {
        let pictures = Arc::new(Pictures::default());
        let stream = runtime
            .block_on(capture.start(
                CaptureRequest {
                    picture: PictureSource::Window(fixture.window),
                    sound: sound.clone(),
                    quality: ShareQuality::default(),
                },
                pictures.clone(),
            ))
            .unwrap();
        let started = Instant::now();
        while pictures.sound.lock().unwrap().len() < 50 {
            assert!(started.elapsed() < PATIENCE, "{sound:?}: no sound blocks");
            std::thread::sleep(Duration::from_millis(20));
        }
        drop(stream);
        let blocks = pictures.sound.lock().unwrap().clone();
        let stereo = Format {
            sample_rate: 48_000,
            channels: 2,
        };
        assert!(
            blocks
                .iter()
                .all(|(samples, format, _)| *samples == 960 && *format == stereo),
            "{sound:?}"
        );
        if matches!(sound, SoundChoice::Application(_)) {
            // The fixture plays nothing.
            assert!(blocks.iter().all(|(_, _, peak)| *peak == 0.0), "{sound:?}");
        }
        assert!(pictures.ended.lock().unwrap().is_empty(), "{sound:?}");
    }
}

#[test]
fn a_source_that_is_not_there_does_not_start() {
    let capture = junto_media::share::system();
    let runtime = runtime();
    let fixture = Fixture::open();
    let (window, screen) = (fixture.window, fixture.screen);
    let process = fixture.child.id() as i32;
    let bundle = runtime
        .block_on(capture.sources())
        .unwrap()
        .windows
        .into_iter()
        .find(|listed| listed.id == window)
        .unwrap()
        .application
        .bundle_id;
    drop(fixture);
    for (picture, sound) in [
        (PictureSource::Window(window), SoundChoice::None),
        (PictureSource::Display(9_999), SoundChoice::None),
        (
            PictureSource::Display(screen),
            SoundChoice::Application(junto_media::share::ApplicationId {
                process_id: process,
                bundle_id: bundle.clone(),
            }),
        ),
    ] {
        let started = runtime.block_on(capture.start(
            CaptureRequest {
                picture,
                sound: sound.clone(),
                quality: ShareQuality::default(),
            },
            Arc::new(Pictures::default()),
        ));
        assert_eq!(
            started.err(),
            Some(CaptureError::SourceGone),
            "{picture:?} {sound:?}"
        );
    }
}
