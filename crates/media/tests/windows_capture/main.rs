//! The capture of Windows against a window of a separate program (ADR 0706). The tests run
//! in the Windows job of CI, in the desktop session of its account: the window of the
//! fixture appears above the other windows for a few seconds and takes no focus. No test
//! plays sound.

#![cfg(target_os = "windows")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

/// The window of the fixture, which the test program also shows as a window of its own.
#[path = "../share_fixture/window.rs"]
#[allow(unsafe_code, dead_code)]
mod window;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
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
    output: BufReader<ChildStdout>,
    title: String,
    window: u32,
    screen: u32,
    /// The inner area in pixels of its screen: left, top, right, bottom.
    area: (i32, i32, i32, i32),
}

impl Fixture {
    fn open() -> Self {
        Self::open_with(&[])
    }

    /// Opens the fixture with further arguments after its title.
    fn open_with(arguments: &[&str]) -> Self {
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
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("the fixture program starts");
        let input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        let words: Vec<&str> = line.split_whitespace().collect();
        assert_eq!(words.first(), Some(&"ready"), "the fixture wrote {line:?}");
        let number = |at: usize| -> i64 { words[at].parse().unwrap() };
        Self {
            _screen: screen,
            child,
            input,
            output,
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

    /// The next line that the fixture writes, without its end.
    fn line(&mut self) -> String {
        let mut line = String::new();
        self.output.read_line(&mut line).unwrap();
        line.trim_end().to_owned()
    }

    /// Changes the size of the inner area and gives the inner area that results.
    fn resize(&mut self, (width, height): (i32, i32)) -> (i32, i32, i32, i32) {
        self.send(&format!("resize {width} {height}"));
        let line = self.line();
        let words: Vec<i32> = line
            .strip_prefix("resized ")
            .unwrap_or_else(|| panic!("the fixture wrote {line:?}"))
            .split_whitespace()
            .map(|word| word.parse().unwrap())
            .collect();
        self.area = (words[1], words[2], words[3], words[4]);
        self.area
    }
}

/// A window of the test program itself, which is the client in these tests. It closes when
/// the value is dropped.
struct OwnWindow {
    commands: std::sync::mpsc::Sender<String>,
    thread: Option<std::thread::JoinHandle<i32>>,
    number: u32,
}

impl OwnWindow {
    fn open(title: &str) -> Self {
        let (commands, received) = std::sync::mpsc::channel::<String>();
        let (said, lines) = std::sync::mpsc::channel::<String>();
        let title = title.to_owned();
        let thread = std::thread::spawn(move || {
            window::run(&title, received.into_iter(), move |line| {
                let _ = said.send(line);
            })
        });
        let line = lines.recv_timeout(PATIENCE).expect("the own window opens");
        let number = line
            .strip_prefix("ready ")
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|number| number.parse().ok())
            .unwrap_or_else(|| panic!("the own window wrote {line:?}"));
        Self {
            commands,
            thread: Some(thread),
            number,
        }
    }
}

impl Drop for OwnWindow {
    fn drop(&mut self) {
        let _ = self.commands.send("close".to_owned());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
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

/// The middles of the quarters of the fixture in a picture of its window. The picture of a
/// window holds its frame: the inner area is its lower part, as wide as the picture without
/// the borders. A picture of another size than the window gives other places.
fn window_places(area: (i32, i32, i32, i32)) -> impl Fn(&PictureFrame) -> [(f64, f64); 4] {
    move |picture: &PictureFrame| {
        let (width, height) = (f64::from(picture.width), f64::from(picture.height));
        let inner = (area.2 - area.0, area.3 - area.1);
        let border = (width - f64::from(inner.0)) / 2.0;
        quarters((
            border,
            height - border - f64::from(inner.1),
            width - border,
            height - border,
        ))
    }
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
    // A window of the test program, which is the client here.
    let own_window = OwnWindow::open(&format!("Junto share test window {}", std::process::id()));
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
    assert!(sources.started_the_client.is_empty(), "{sources:?}");
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
            .all(|window| window.id != own_window.number),
        "the window of the client is listed"
    );
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
                dialog_parent: None,
            },
            pictures.clone(),
        ))
        .unwrap();
    assert!(stream.width > 640 && stream.height > 400, "{stream:?}");
    until_colours(
        &pictures,
        0,
        window_places(fixture.area),
        "the window share",
    );
    fixture.send("turn");
    until_colours(
        &pictures,
        1,
        window_places(fixture.area),
        "the window share after a turn",
    );
    // A larger and a smaller window: the pictures have the new size, and the share goes on.
    for size in [(900, 560), (420, 280)] {
        let area = fixture.resize(size);
        assert_eq!((area.2 - area.0, area.3 - area.1), size);
        until_colours(
            &pictures,
            1,
            window_places(area),
            &format!("the window share at {size:?}"),
        );
    }
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
                dialog_parent: None,
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
                    dialog_parent: None,
                },
                pictures.clone(),
            ))
            .unwrap();
        // The delivery gives a block of silence every 10 ms also without any packet: the
        // count of the frames that the system gave shows that the capture itself reads.
        let captured = junto_media::share::windows::captured_sound_frames();
        let started = Instant::now();
        while junto_media::share::windows::captured_sound_frames() - captured < 24_000
            || pictures.sound.lock().unwrap().len() < 50
        {
            assert!(
                started.elapsed() < PATIENCE,
                "{sound:?}: {} frames captured, {} blocks delivered",
                junto_media::share::windows::captured_sound_frames() - captured,
                pictures.sound.lock().unwrap().len()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // The stop wakes the threads that wait for the system; none waits for a time limit,
        // so a missing wake would hold the drop.
        let dropped = Instant::now();
        let dropping = std::thread::spawn(move || drop(stream));
        while !dropping.is_finished() {
            assert!(
                dropped.elapsed() < PATIENCE,
                "{sound:?}: the drop did not return"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        eprintln!("{sound:?}: the drop took {:?}", dropped.elapsed());
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

/// The window of the client lists the sound devices on its background threads, which are
/// threads of the default pool of the system. A listing must leave them outside the
/// single-threaded apartment of COM: the activation of the sound of a share runs on a thread of
/// that pool, and fails with "Cannot change thread mode after it is set" on a thread in that
/// apartment (ADR 0706, ADR 0733).
#[test]
fn a_share_with_sound_starts_after_the_sound_devices_were_listed_on_the_threads_of_the_system_pool()
{
    use junto_media::audio::AudioHost;
    use junto_media::audio::cpal_host::CpalHost;
    use junto_media::com::apartment;

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
    static HOST: std::sync::OnceLock<CpalHost> = std::sync::OnceLock::new();
    let after = pool::run(128, || {
        let _ = HOST.get_or_init(CpalHost::new).devices();
        apartment()
    });
    let mut failed = Vec::new();
    for sound in [SoundChoice::System, SoundChoice::Application(application)] {
        let started = runtime.block_on(capture.start(
            CaptureRequest {
                picture: PictureSource::Window(fixture.window),
                sound: sound.clone(),
                quality: ShareQuality::default(),
                dialog_parent: None,
            },
            Arc::new(Pictures::default()),
        ));
        if let Err(error) = started {
            failed.push(format!("{sound:?}: {error:?}"));
        }
    }
    let entered: Vec<_> = after
        .iter()
        .filter(|apartment| apartment.entered())
        .collect();
    assert!(
        failed.is_empty() && entered.is_empty(),
        "shares that did not start: {failed:?}; pool threads left in an apartment: {} of {}, {:?}",
        entered.len(),
        after.len(),
        entered.first()
    );
}

/// Work on the threads of the default pool of the system, which gpui uses for the background
/// tasks of the window on Windows.
#[allow(unsafe_code)]
mod pool {
    use std::ffi::c_void;
    use std::sync::Mutex;
    use std::sync::mpsc::{Sender, channel};
    use std::time::Duration;

    use junto_media::com::Apartment;
    use windows::Win32::System::Threading::{PTP_CALLBACK_INSTANCE, TrySubmitThreadpoolCallback};

    struct Task {
        work: fn() -> Apartment,
        done: Sender<Apartment>,
    }

    unsafe extern "system" fn call(_: PTP_CALLBACK_INSTANCE, context: *mut c_void) {
        // SAFETY: `run` gave the box of a task, and each callback takes its own once.
        let task = unsafe { Box::from_raw(context.cast::<Task>()) };
        let apartment = (task.work)();
        // The thread stays busy for a moment, so that the pool runs the other tasks on
        // other threads.
        std::thread::sleep(Duration::from_millis(40));
        let _ = task.done.send(apartment);
    }

    /// Runs `work` `count` times on the pool and gives the apartment that each call returned.
    pub fn run(count: usize, work: fn() -> Apartment) -> Vec<Apartment> {
        static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());
        let _one = ONE_AT_A_TIME.lock().unwrap();
        let (done, results) = channel();
        for _ in 0..count {
            let task = Box::into_raw(Box::new(Task {
                work,
                done: done.clone(),
            }));
            // SAFETY: the callback takes the box back; when the submission fails, it is
            // freed here.
            unsafe {
                if let Err(error) = TrySubmitThreadpoolCallback(Some(call), Some(task.cast()), None)
                {
                    drop(Box::from_raw(task));
                    panic!("the pool took no task: {error}");
                }
            }
        }
        drop(done);
        results.iter().collect()
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
                dialog_parent: None,
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

#[test]
fn the_sound_of_an_application_that_started_the_client_is_not_offered_alone() {
    // The fixture starts this test program again, which then is the client below the fixture
    // in the process tree, and which runs `a_client_started_by_the_fixture` alone.
    let program = std::env::current_exe().unwrap();
    let mut fixture = Fixture::open_with(&[
        "--start",
        program.to_str().unwrap(),
        "--exact",
        "a_client_started_by_the_fixture",
        "--ignored",
        "--nocapture",
    ]);
    let line = fixture.line();
    assert_eq!(line, "ran 0", "the client below the fixture failed");
}

#[test]
#[ignore = "the fixture starts it in `the_sound_of_an_application_that_started_the_client_is_not_offered_alone`"]
fn a_client_started_by_the_fixture() {
    let window: u32 = std::env::var("JUNTO_FIXTURE_WINDOW")
        .expect("started by the fixture")
        .parse()
        .unwrap();
    let capture = junto_media::share::system();
    let runtime = runtime();
    let sources = runtime.block_on(capture.sources()).unwrap();
    let fixture = sources
        .windows
        .iter()
        .find(|listed| listed.id == window)
        .expect("the window of the fixture is listed: its picture can be shared")
        .application
        .clone();
    // Its sound would include the sound of the client. A process further up, such as a
    // terminal with a window, can be listed too.
    assert!(sources.started_the_client.contains(&fixture), "{sources:?}");
    assert!(
        sources
            .applications
            .iter()
            .all(|application| application.id != fixture),
        "{sources:?}"
    );
    let start = |sound: SoundChoice| {
        runtime.block_on(capture.start(
            CaptureRequest {
                picture: PictureSource::Window(window),
                sound,
                quality: ShareQuality::default(),
                dialog_parent: None,
            },
            Arc::new(Pictures::default()),
        ))
    };
    assert_eq!(
        start(SoundChoice::Application(fixture)).err(),
        Some(CaptureError::StartedTheClient)
    );
    // All system sound leaves out the tree of the client and starts.
    drop(start(SoundChoice::System).unwrap());
}

/// The process that created a program that wrote its report into a file, and the path of
/// the program of that process.
fn creator_of(report: &std::path::Path) -> (u32, String) {
    let started = Instant::now();
    loop {
        if let Ok(text) = std::fs::read_to_string(report)
            && let Some((process, path)) = text.split_once(' ')
        {
            let _ = std::fs::remove_file(report);
            return (process.parse().unwrap(), path.to_owned());
        }
        assert!(started.elapsed() < PATIENCE, "no report in {report:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_program_that_the_client_opens_is_started_by_the_shell_outside_its_tree() {
    let fixture = env!("CARGO_BIN_EXE_junto-share-fixture");
    let report = std::env::temp_dir().join(format!("junto-share-report-{}", std::process::id()));
    // What the client opens: the shell starts it, hidden here, and is its creator.
    junto_media::share::windows::open_through_the_shell(
        fixture.as_ref(),
        &["--report".as_ref(), report.as_os_str()],
        false,
    )
    .unwrap();
    let (creator, path) = creator_of(&report);
    assert_ne!(creator, std::process::id());
    assert!(
        path.to_lowercase().ends_with(r"\explorer.exe"),
        "the creator is {creator} {path:?}"
    );
    // A program that the client starts itself is in its tree, and a share with all system
    // sound would leave out its sound.
    let status = Command::new(fixture)
        .arg("--report")
        .arg(&report)
        .status()
        .unwrap();
    assert!(status.success());
    assert_eq!(creator_of(&report).0, std::process::id());
}

/// The start of the name of each scheme that a process of this test program registers.
const SCHEME_PREFIX: &str = "junto-test-open-";
/// The number of schemes that this process has registered.
static REGISTERED: AtomicUsize = AtomicUsize::new(0);

/// A scheme of links that only one registration of this process of the test program uses,
/// registered for the account with a command that starts the fixture. The registration is
/// removed when the value is dropped. A process that is killed drops nothing, so its name holds
/// the number and the creation time of the process and the number of the registration
/// (`junto-test-open-<number>-<creation time>-<registration>`), and each registration first
/// removes the schemes of processes that no longer run. The keys are volatile: Windows also
/// removes them when the account logs off.
struct Scheme {
    name: String,
    key: String,
}

impl Scheme {
    fn register(command: &str) -> Self {
        Self::remove_ended();
        let process = std::process::id();
        let created = registry::creation_time(process).unwrap();
        let registration = REGISTERED.fetch_add(1, Ordering::Relaxed);
        let name = format!("{SCHEME_PREFIX}{process}-{created}-{registration}");
        let key = format!(r"Software\Classes\{name}");
        registry::set(&key, None, "URL:Junto test");
        registry::set(&key, Some("URL Protocol"), "");
        registry::set(&format!(r"{key}\shell\open\command"), None, command);
        Self { name, key }
    }

    /// Removes the schemes of this test program whose process no longer runs: no process has
    /// the number, or it has another creation time. A name without a creation time is of an
    /// earlier version of this program, and is removed when no process has its number.
    fn remove_ended() {
        for name in registry::classes() {
            let Some(process) = name.strip_prefix(SCHEME_PREFIX) else {
                continue;
            };
            let mut parts = process.splitn(3, '-');
            let number = parts.next().unwrap_or_default();
            let created = parts.next().and_then(|created| created.parse::<u64>().ok());
            let Ok(number) = number.parse::<u32>() else {
                continue;
            };
            let running = registry::creation_time(number);
            let ended = match created {
                Some(created) => running != Some(created),
                None => running.is_none(),
            };
            if ended {
                registry::remove(&format!(r"Software\Classes\{name}"));
            }
        }
    }
}

impl Drop for Scheme {
    fn drop(&mut self) {
        registry::remove(&self.key);
    }
}

#[allow(unsafe_code)]
mod registry {
    use windows::Win32::System::Registry::{
        HKEY, HKEY_CURRENT_USER, KEY_ENUMERATE_SUB_KEYS, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
        REG_OPTION_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegEnumKeyExW,
        RegOpenKeyExW, RegSetValueExW,
    };
    use windows::core::{HSTRING, PCWSTR, PWSTR};

    /// The creation time of a running process as Windows stores it, or nothing when no
    /// process of that number runs.
    pub fn creation_time(process: u32) -> Option<u64> {
        use windows::Win32::Foundation::{CloseHandle, FILETIME, WAIT_TIMEOUT};
        use windows::Win32::System::Threading::{
            GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
            WaitForSingleObject,
        };
        // SAFETY: the handle is closed before the return; the call writes the four times into
        // the values.
        unsafe {
            let handle = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                process,
            )
            .ok()?;
            let (mut created, mut ended, mut kernel, mut user) = (
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
                FILETIME::default(),
            );
            // A process that has ended stays open while another process holds a handle of it.
            let running = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT
                && GetProcessTimes(handle, &mut created, &mut ended, &mut kernel, &mut user)
                    .is_ok();
            let _ = CloseHandle(handle);
            running
                .then(|| u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime))
        }
    }

    /// The names of the keys of `Software\Classes` of the account.
    pub fn classes() -> Vec<String> {
        let mut opened = HKEY::default();
        let mut names = Vec::new();
        // SAFETY: the strings and the buffer outlive the calls, and `length` states the length
        // of the buffer; the key opened here is closed before the return.
        unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(r"Software\Classes"),
                None,
                KEY_ENUMERATE_SUB_KEYS,
                &mut opened,
            )
            .ok()
            .unwrap();
            // A key name has at most 255 characters.
            let mut buffer = [0u16; 256];
            for index in 0.. {
                let mut length = buffer.len() as u32;
                let read = RegEnumKeyExW(
                    opened,
                    index,
                    Some(PWSTR(buffer.as_mut_ptr())),
                    &mut length,
                    None,
                    None,
                    None,
                    None,
                );
                if read.is_err() {
                    break;
                }
                names.push(String::from_utf16_lossy(&buffer[..length as usize]));
            }
            let _ = RegCloseKey(opened);
        }
        names
    }

    /// Whether a key of the account is volatile: below a volatile key Windows refuses to create
    /// a key that is not volatile (`ERROR_CHILD_MUST_BE_VOLATILE`).
    pub fn is_volatile(key: &str) -> bool {
        use windows::Win32::Foundation::ERROR_CHILD_MUST_BE_VOLATILE;
        let child = format!(r"{key}\junto-test-volatile");
        let mut opened = HKEY::default();
        // SAFETY: the string outlives the call; a key opened here is closed before the return.
        let created = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(child.as_str()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut opened,
                None,
            )
        };
        if created.is_ok() {
            // SAFETY: the key was opened by the call above.
            let _ = unsafe { RegCloseKey(opened) };
            remove(&child);
            return false;
        }
        assert_eq!(created, ERROR_CHILD_MUST_BE_VOLATILE, "{child}");
        true
    }

    /// Sets a text value of a volatile key of the account, the default value without a name.
    pub fn set(key: &str, name: Option<&str>, value: &str) {
        let mut opened = HKEY::default();
        let data: Vec<u8> = value
            .encode_utf16()
            .chain([0])
            .flat_map(u16::to_le_bytes)
            .collect();
        let name = name.map(HSTRING::from);
        // SAFETY: the strings and the buffer outlive the calls; the key opened here is
        // closed before the return.
        unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                &HSTRING::from(key),
                None,
                PCWSTR::null(),
                REG_OPTION_VOLATILE,
                KEY_SET_VALUE,
                None,
                &mut opened,
                None,
            )
            .ok()
            .unwrap();
            let written = RegSetValueExW(
                opened,
                name.as_ref()
                    .map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr())),
                None,
                REG_SZ,
                Some(&data),
            );
            let _ = RegCloseKey(opened);
            written.ok().unwrap();
        }
    }

    /// Removes a key of the account with its keys and values.
    pub fn remove(key: &str) {
        // SAFETY: the string outlives the call.
        let _ = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key)) };
    }

    /// The program that the system starts for the verb "open" of a scheme, as the shell finds
    /// it, or nothing when the shell finds none.
    pub fn program_of(scheme: &str) -> Option<String> {
        use windows::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_EXECUTABLE, AssocQueryStringW};
        use windows::core::{PWSTR, w};
        let mut buffer = vec![0u16; 32_768];
        let mut size = buffer.len() as u32;
        // SAFETY: the strings and the buffer outlive the call, and `size` states the length
        // of the buffer.
        unsafe {
            AssocQueryStringW(
                ASSOCF_NONE,
                ASSOCSTR_EXECUTABLE,
                &HSTRING::from(scheme),
                w!("open"),
                Some(PWSTR(buffer.as_mut_ptr())),
                &mut size,
            )
            .ok()
            .ok()?;
        }
        let length = buffer.iter().position(|unit| *unit == 0)?;
        Some(String::from_utf16_lossy(&buffer[..length]))
    }

    /// Closes the message windows of the desktop whose title starts with `title`, and
    /// returns their number.
    pub fn close_messages(title: &str) -> usize {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetClassNameW, GetWindowTextW, PostMessageW, WM_CLOSE,
        };
        use windows::core::BOOL;
        struct Search {
            title: String,
            found: Vec<HWND>,
        }
        unsafe extern "system" fn visit(window: HWND, search: LPARAM) -> BOOL {
            // SAFETY: `search` is the value that `close_messages` passed, and the
            // enumeration calls this function on its thread during that call.
            let search = unsafe { &mut *(search.0 as *mut Search) };
            let (mut class, mut text) = ([0u16; 64], [0u16; 512]);
            // SAFETY: each call writes at most the length of its buffer.
            let (class_length, text_length) = unsafe {
                (
                    GetClassNameW(window, &mut class),
                    GetWindowTextW(window, &mut text),
                )
            };
            let class = String::from_utf16_lossy(&class[..class_length.max(0) as usize]);
            let text = String::from_utf16_lossy(&text[..text_length.max(0) as usize]);
            if class == "#32770" && text.starts_with(&search.title) {
                search.found.push(window);
            }
            true.into()
        }
        let mut search = Search {
            title: title.to_owned(),
            found: Vec::new(),
        };
        // SAFETY: `search` outlives the enumeration, which calls `visit` during the call.
        unsafe {
            let _ = EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize));
            for window in &search.found {
                let _ = PostMessageW(Some(*window), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        search.found.len()
    }
}

/// The lines that a program wrote into a file, once the file exists.
fn arguments_in(file: &std::path::Path) -> Vec<String> {
    let started = Instant::now();
    loop {
        if let Ok(text) = std::fs::read_to_string(file) {
            let _ = std::fs::remove_file(file);
            return text.split('\n').map(str::to_owned).collect();
        }
        assert!(started.elapsed() < PATIENCE, "no arguments in {file:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The keys of `Software\Classes` that a test created, removed when the value is dropped.
struct Keys(Vec<String>);

impl Drop for Keys {
    fn drop(&mut self) {
        for name in &self.0 {
            registry::remove(&format!(r"Software\Classes\{name}"));
        }
    }
}

#[test]
fn a_registration_removes_the_schemes_of_test_processes_that_no_longer_run() {
    // A process that runs until its input ends, and one that has ended. The handle of the
    // ended one stays open, so Windows gives its number to no other process.
    let mut running = Command::new("findstr")
        .arg("x")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .unwrap();
    let mut ended = Command::new("cmd")
        .args(["/d", "/c", "exit"])
        .spawn()
        .unwrap();
    ended.wait().unwrap();
    let number = running.id();
    let created = registry::creation_time(number).unwrap();
    let kept = [
        format!("{SCHEME_PREFIX}{number}-{created}"),
        format!("{SCHEME_PREFIX}{number}"),
        format!("{SCHEME_PREFIX}other"),
    ];
    let removed = [
        format!("{SCHEME_PREFIX}{number}-{}-0", created - 1),
        format!("{SCHEME_PREFIX}{}-{created}-0", ended.id()),
        format!("{SCHEME_PREFIX}{}", ended.id()),
    ];
    let _keys = Keys(kept.iter().chain(&removed).cloned().collect());
    for name in kept.iter().chain(&removed) {
        registry::set(&format!(r"Software\Classes\{name}"), None, "URL:Junto test");
    }
    let scheme = Scheme::register("unused");
    let classes = registry::classes();
    for name in kept.iter().chain([&scheme.name]) {
        assert!(classes.contains(name), "{name} was removed");
    }
    for name in &removed {
        assert!(!classes.contains(name), "{name} was not removed");
    }
    // Windows removes the keys of the scheme also at a logoff of the account.
    for key in [
        scheme.key.clone(),
        format!(r"{}\shell\open\command", scheme.key),
    ] {
        assert!(registry::is_volatile(&key), "{key} is not volatile");
    }
    let name = scheme.name.clone();
    drop(scheme);
    assert!(
        !registry::classes().contains(&name),
        "{name} stayed after the drop"
    );
    drop(running.stdin.take());
    running.wait().unwrap();
}

#[test]
fn a_link_with_quotes_and_spaces_reaches_its_application_as_one_argument() {
    use junto_client_core::link::Link;
    use junto_media::share::windows::{Opened, Target, open};
    // The shell finds the program of a registered command only by a path with backslashes;
    // the target directory of the build can be named with slashes.
    let fixture = std::path::absolute(env!("CARGO_BIN_EXE_junto-share-fixture")).unwrap();
    let process = std::process::id();
    let file =
        std::path::absolute(std::env::temp_dir().join(format!("junto-share-arguments-{process}")))
            .unwrap();
    let _ = std::fs::remove_file(&file);
    // The usual form of the command of an application for a scheme.
    let scheme = Scheme::register(&format!(
        r#""{}" --arguments "{}" "%1""#,
        fixture.display(),
        file.display()
    ));
    let name = &scheme.name;
    // An address that a server could send: with `%1` replaced as it is, the fixture would
    // receive `--report` and `C:\a b` as arguments of their own.
    let link = Link::of_any_scheme(&format!(r#"{name}:x" --report "C:\a b" y"#)).unwrap();
    // When the shell finds no program for the scheme, it shows a message on the desktop and
    // waits for a person to close it: the link is opened only when the shell finds the
    // fixture.
    let program = registry::program_of(name).map(std::path::PathBuf::from);
    assert_eq!(program.as_deref(), Some(fixture.as_path()));
    // The shell answers when it has started the program. A message that it shows all the
    // same is closed here, so that nothing of the test stays on the desktop.
    let (answer, answered) = std::sync::mpsc::channel();
    std::thread::spawn({
        let target = Target::Link(link.clone());
        move || answer.send(open(&target))
    });
    let opened = answered.recv_timeout(PATIENCE);
    if opened.is_err() {
        let closed = registry::close_messages(name);
        panic!("the shell did not answer; {closed} message windows of the shell were closed");
    }
    assert_eq!(opened.ok(), Some(Ok(Opened::ByTheShell)));
    assert_eq!(arguments_in(&file), [link.as_str()]);
    assert!(!link.as_str().contains(['"', ' ']), "{link}");
}

#[test]
fn the_graphics_memory_of_a_window_share_does_not_grow_with_its_pictures() {
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
                dialog_parent: None,
            },
            pictures.clone(),
        ))
        .unwrap();
    // Each turn changes every quarter: the next picture that shows it went through the copy
    // on the graphics card and the read into memory.
    let mut turns = 0;
    let mut turn = |fixture: &mut Fixture, count: usize| {
        for _ in 0..count {
            fixture.send("turn");
            turns += 1;
            until_colours(&pictures, turns, window_places(fixture.area), "a turn");
        }
    };
    turn(&mut fixture, 8);
    let before = junto_media::share::windows::graphics_memory_in_use().unwrap();
    turn(&mut fixture, 120);
    let after = junto_media::share::windows::graphics_memory_in_use().unwrap();
    drop(stream);
    // A texture of the window is about 1.2 MB; one more texture for each picture would add
    // about 140 MB.
    let grown = after.saturating_sub(before);
    eprintln!("graphics memory: {before} bytes before, {after} bytes after 120 pictures");
    assert!(
        grown < 16 << 20,
        "the graphics memory grew by {grown} bytes"
    );
}
