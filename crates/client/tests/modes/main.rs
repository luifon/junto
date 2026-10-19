//! The memory, the processor time and the threads of the client in each mode of its use
//! (ADR 0914). Each mode runs in a process of its own: the program starts itself once per
//! mode, and the started process opens one native window of the application on a new
//! profile, brings it into the mode, holds the mode, reads the process at a fixed interval
//! and prints its readings. The first process prints the table of all modes. It fails only
//! when a mode was not reached or not held: these are measurements, not limits.
//!
//! The window is the only client in its process. The server of the tests runs in its own
//! process, and the other people of a call are processes of the test participant of
//! `spikes/test-participant`, which join the call with a token of the server, so the
//! readings hold the work of one client and of the program that drives its window. The
//! window is driven through the state of the application and through events of the
//! pointer, without the names of elements, which gpui keeps only with debug assertions:
//! the measured build is the release profile as it is shipped.
//!
//! It runs on the main thread, which the windows of the system need, so it is a test
//! without the test runner. It does nothing on other systems than macOS, and nothing
//! unless `JUNTO_TEST_MODES` is `1` and the variables of the media server of the tests are
//! set. `JUNTO_TEST_MODES_ONLY` names the modes to run, separated by commas (default all).
//! `JUNTO_TEST_MODES_SECONDS` sets the measured time of each mode (default 120).
//! `JUNTO_TEST_MODES_PROFILE` names a directory: in the middle of the measured time of each
//! mode the program writes there what uses the memory and the processor of the process
//! (`footprint -v`, `vmmap -summary`, `heap`, a `sample` of the threads, and with
//! `MallocStackLogging=1` the live allocations by the stacks that made them), for the
//! analysis of a mode.
//!
//! ```sh
//! (cd spikes/test-participant && ../../tools/serial.sh cargo build --release)
//! cargo build --release -p junto-server -p junto-client --tests
//! JUNTO_TEST_MODES=1 tools/with-test-postgres.sh tools/with-test-livekit.sh \
//!     tools/serial.sh cargo test --release -p junto-client --test modes
//! ```

#![allow(clippy::unwrap_used, clippy::expect_used)]
// A test waits for its files by design; the refusals of the file system in `clippy.toml`
// are for the code of the application (ADR 0730).
#![allow(clippy::disallowed_methods)]

use std::process::ExitCode;

// The visual test context of gpui exists only on macOS, so everything below except the
// `main` of other systems is compiled there only (ADR 0902).
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/recording_server.rs"]
mod recording_server;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/support.rs"]
mod support;

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../app/harness.rs"]
mod harness;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../../../client-core/tests/e2e/limit.rs"]
mod limit;
#[cfg(target_os = "macos")]
#[path = "../rehearsal/native.rs"]
mod native;
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../rehearsal/process.rs"]
mod process;

/// The allocator of the program counts the bytes that its Rust allocations hold.
#[cfg(target_os = "macos")]
#[global_allocator]
static ALLOCATOR: junto_alloc_count::Counting = junto_alloc_count::Counting::new();

#[cfg(target_os = "macos")]
use harness::Window;
#[cfg(target_os = "macos")]
use junto_client_core::ServerHandle;
#[cfg(target_os = "macos")]
use junto_client_core::protocol::ids::{AccountId, ChannelId, WorkspaceId};
#[cfg(target_os = "macos")]
use process::Sample;

/// The variable that makes a started process run one mode.
#[cfg(target_os = "macos")]
const CHILD: &str = "JUNTO_TEST_MODES_CHILD";
/// The time after the mode is reached in which the window draws it without a reading: the
/// first pictures of a call, the first pages of a channel and the caches of the GPU are
/// made in it.
#[cfg(target_os = "macos")]
const SETTLING: Duration = Duration::from_secs(20);
/// The time between two readings. The highest processor share of the mode is the share
/// over one such interval.
#[cfg(target_os = "macos")]
const INTERVAL: Duration = Duration::from_secs(5);
/// The messages of the channel that the idle and the text modes show.
#[cfg(target_os = "macos")]
const MESSAGES: usize = 400;
/// The limit of the writes of the long channel, which wait for the rate of writes of the
/// server.
#[cfg(target_os = "macos")]
const FILL_LIMIT: Duration = Duration::from_secs(1800);
/// The distance that the wheel of the pointer moves the message list in each frame of the
/// text mode, and how long it moves in one direction before it turns.
#[cfg(target_os = "macos")]
const WHEEL_PER_FRAME: f32 = 30.0;
#[cfg(target_os = "macos")]
const WHEEL_TURN: Duration = Duration::from_secs(20);
#[cfg(target_os = "macos")]
const MIB: u64 = 1024 * 1024;

/// One mode of the use of the client.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// One server; the window shows the end of a text channel and nothing happens.
    Idle,
    /// The same channel; the wheel of the pointer moves the list up its history and back.
    Text,
    /// A call with one other person who sends a microphone.
    Voice,
    /// The call of `Voice` with the camera of this device and the camera of the other
    /// person.
    Camera,
    /// The call of `Voice`; this device shares its screen and watches the share of the
    /// other person.
    Share,
    /// A call with three other people who each send a microphone and a camera, with the
    /// camera of this device.
    Cameras,
}

#[cfg(target_os = "macos")]
const MODES: [Mode; 6] = [
    Mode::Idle,
    Mode::Text,
    Mode::Voice,
    Mode::Camera,
    Mode::Share,
    Mode::Cameras,
];

#[cfg(target_os = "macos")]
impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Idle => "idle",
            Mode::Text => "text",
            Mode::Voice => "voice",
            Mode::Camera => "camera",
            Mode::Share => "share",
            Mode::Cameras => "cameras",
        }
    }

    fn named(name: &str) -> Option<Self> {
        MODES.into_iter().find(|mode| mode.name() == name)
    }

    /// The modes in which the window shows the text channel with its long history.
    fn reads_text(self) -> bool {
        matches!(self, Mode::Idle | Mode::Text)
    }

    /// The other people of the call of the mode, as the arguments of the test participant.
    /// A camera is sent as the camera of the application sends it: 1280 × 720 at 30
    /// pictures a second in simulcast layers. A share is sent as the share of the
    /// application with its default settings: 1920 × 1080 at 30 pictures a second.
    fn others(self) -> Vec<Vec<&'static str>> {
        const CAMERA: [&str; 6] = [
            "--camera",
            "--camera-size",
            "1280x720",
            "--camera-fps",
            "30",
            "--simulcast",
        ];
        const SCREEN: [&str; 5] = [
            "--screen",
            "--screen-size",
            "1920x1080",
            "--screen-fps",
            "30",
        ];
        let with = |microphone: &'static str, extra: &[&'static str]| {
            let mut arguments = vec!["--mic-hz", microphone];
            arguments.extend_from_slice(extra);
            arguments
        };
        match self {
            Mode::Idle | Mode::Text => Vec::new(),
            Mode::Voice => vec![with("440", &[])],
            Mode::Camera => vec![with("440", &CAMERA)],
            Mode::Share => vec![with("440", &SCREEN)],
            Mode::Cameras => vec![
                with("440", &CAMERA),
                with("550", &CAMERA),
                with("770", &CAMERA),
            ],
        }
    }
}

/// What one mode used: the highest reading of each kind over the measured time, and the
/// processor share over the whole time and over the busiest interval.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, Default)]
struct Measured {
    footprint: u64,
    graphics: u64,
    /// The pixel buffers of Core Video (the category "IOSurface"): the cameras and the
    /// watched share that the window draws as surfaces. They are in the footprint and not
    /// in the graphics memory.
    surfaces: u64,
    rust: u64,
    processor_average: f64,
    processor_peak: f64,
    threads: usize,
    files: usize,
    /// The drawings of the window over the measured time. The frame loop of the program is
    /// paced by time and a native window waits for the display, so two runs can draw a
    /// different number of times; the processor time per drawing compares them.
    draws: usize,
    /// The time from the first to the last reading.
    seconds: f64,
}

#[cfg(target_os = "macos")]
impl Measured {
    fn of(samples: &[Sample], draws: usize) -> Self {
        let (first, last) = (samples[0], samples[samples.len() - 1]);
        let share = |from: &Sample, to: &Sample| {
            let time = (to.at - from.at).as_secs_f64();
            100.0 * to.processor.saturating_sub(from.processor).as_secs_f64() / time.max(0.001)
        };
        Self {
            footprint: samples
                .iter()
                .map(|s| s.footprint)
                .max()
                .unwrap_or_default(),
            graphics: samples.iter().map(|s| s.graphics).max().unwrap_or_default(),
            surfaces: samples.iter().map(|s| s.surfaces).max().unwrap_or_default(),
            rust: samples.iter().map(|s| s.rust).max().unwrap_or_default(),
            processor_average: share(&first, &last),
            processor_peak: samples
                .windows(2)
                .map(|pair| share(&pair[0], &pair[1]))
                .fold(0.0, f64::max),
            threads: samples.iter().map(|s| s.threads).max().unwrap_or_default(),
            files: samples.iter().map(|s| s.files).max().unwrap_or_default(),
            draws,
            seconds: (last.at - first.at).as_secs_f64(),
        }
    }

    /// The processor time of the process per drawing of the window, in milliseconds.
    fn per_draw(&self) -> Option<f64> {
        (self.draws > 0)
            .then(|| self.processor_average / 100.0 * self.seconds * 1000.0 / self.draws as f64)
    }

    /// The line that a started process prints and the first process reads.
    fn line(&self, mode: Mode) -> String {
        format!(
            "modes: result {} footprint={} graphics={} surfaces={} rust={} processor_average={:.2} processor_peak={:.2} threads={} files={} draws={} seconds={:.1}",
            mode.name(),
            self.footprint,
            self.graphics,
            self.surfaces,
            self.rust,
            self.processor_average,
            self.processor_peak,
            self.threads,
            self.files,
            self.draws,
            self.seconds
        )
    }

    fn parse(line: &str) -> Option<(Mode, Self)> {
        let mut words = line.strip_prefix("modes: result ")?.split(' ');
        let mode = Mode::named(words.next()?)?;
        let mut measured = Self::default();
        for word in words {
            let (key, value) = word.split_once('=')?;
            match key {
                "footprint" => measured.footprint = value.parse().ok()?,
                "graphics" => measured.graphics = value.parse().ok()?,
                "surfaces" => measured.surfaces = value.parse().ok()?,
                "rust" => measured.rust = value.parse().ok()?,
                "processor_average" => measured.processor_average = value.parse().ok()?,
                "processor_peak" => measured.processor_peak = value.parse().ok()?,
                "threads" => measured.threads = value.parse().ok()?,
                "files" => measured.files = value.parse().ok()?,
                "draws" => measured.draws = value.parse().ok()?,
                "seconds" => measured.seconds = value.parse().ok()?,
                _ => return None,
            }
        }
        Some((mode, measured))
    }
}

#[cfg(target_os = "macos")]
fn mib(bytes: u64) -> f64 {
    bytes as f64 / MIB as f64
}

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    println!("modes: not run, the native windows run only on macOS");
    ExitCode::SUCCESS
}

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    if let Ok(name) = std::env::var(CHILD) {
        let mode = Mode::named(&name).unwrap_or_else(|| panic!("no mode is named {name}"));
        return run_mode(mode);
    }
    if std::env::var("JUNTO_TEST_MODES").as_deref() != Ok("1") {
        println!("modes: not run, JUNTO_TEST_MODES is not 1");
        return ExitCode::SUCCESS;
    }
    if std::env::var("JUNTO_TEST_LIVEKIT_URL").is_err() {
        println!("modes: not run, JUNTO_TEST_LIVEKIT_URL is not set");
        return ExitCode::SUCCESS;
    }
    if env!("JUNTO_OPT_LEVEL") == "0" || cfg!(debug_assertions) {
        println!(
            "modes: not run: it measures the release profile as it is shipped; build it with --release and without debug assertions"
        );
        return ExitCode::FAILURE;
    }
    let participant = participant_tool();
    if !std::path::Path::new(&participant).exists() {
        println!(
            "modes: {participant} is missing. Build it once: (cd spikes/test-participant && ../../tools/serial.sh cargo build --release)"
        );
        return ExitCode::FAILURE;
    }
    let chosen: Vec<Mode> = match std::env::var("JUNTO_TEST_MODES_ONLY") {
        Ok(names) => names
            .split(',')
            .map(|name| {
                Mode::named(name.trim()).unwrap_or_else(|| panic!("no mode is named {name}"))
            })
            .collect(),
        Err(_) => MODES.to_vec(),
    };

    let mut failures = Vec::new();
    let mut table = Vec::new();
    for mode in chosen {
        match measure_in_own_process(mode) {
            Ok(measured) => table.push((mode, measured)),
            Err(failure) => failures.push(format!("{}: {failure}", mode.name())),
        }
    }
    println!(
        "modes: {:<8} {:>10} {:>10} {:>10} {:>10} {:>9} {:>9} {:>8} {:>6} {:>8} {:>8}",
        "mode",
        "footprint",
        "graphics",
        "surfaces",
        "Rust",
        "processor",
        "peak",
        "threads",
        "files",
        "draws",
        "ms/draw"
    );
    for (mode, measured) in &table {
        println!(
            "modes: {:<8} {:>6.1} MiB {:>6.1} MiB {:>6.1} MiB {:>6.1} MiB {:>8.1}% {:>8.1}% {:>8} {:>6} {:>8} {:>8}",
            mode.name(),
            mib(measured.footprint),
            mib(measured.graphics),
            mib(measured.surfaces),
            mib(measured.rust),
            measured.processor_average,
            measured.processor_peak,
            measured.threads,
            measured.files,
            measured.draws,
            measured
                .per_draw()
                .map_or_else(|| "-".to_owned(), |ms| format!("{ms:.2}"))
        );
    }
    if failures.is_empty() {
        println!("modes: passed");
        ExitCode::SUCCESS
    } else {
        for failure in &failures {
            println!("modes: failed: {failure}");
        }
        ExitCode::FAILURE
    }
}

/// Starts this program for one mode, passes its output on, and reads its result line.
#[cfg(target_os = "macos")]
fn measure_in_own_process(mode: Mode) -> Result<Measured, String> {
    use std::io::{BufRead, BufReader};
    println!("modes: {} starts", mode.name());
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env(CHILD, mode.name())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("the process of the mode did not start: {error}"))?;
    let mut result = None;
    for line in BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map_while(Result::ok)
    {
        println!("{line}");
        if let Some((named, measured)) = Measured::parse(&line)
            && named == mode
        {
            result = Some(measured);
        }
    }
    let status = child.wait().unwrap();
    match (status.success(), result) {
        (true, Some(measured)) => Ok(measured),
        (true, None) => Err("the process of the mode printed no result".to_owned()),
        (false, _) => Err(format!("the process of the mode ended with {status}")),
    }
}

/// The measured time of each mode in seconds: `JUNTO_TEST_MODES_SECONDS`, or 120.
#[cfg(target_os = "macos")]
fn measured_seconds() -> f64 {
    std::env::var("JUNTO_TEST_MODES_SECONDS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(120) as f64
}

/// Brings one window into the mode, holds the mode and prints the readings.
#[cfg(target_os = "macos")]
fn run_mode(mode: Mode) -> ExitCode {
    let measured_time = Duration::from_secs_f64(measured_seconds());
    let mut native = native::Native::start();
    let world = harness::World::new();
    let server = world.server_with_media("Modes");
    let profile = tempfile::tempdir().unwrap();
    let place = Place::prepare(&world, &server, profile.path(), mode);
    let mut window = harness::open(&mut native.cx, profile.path());
    window.until_ready();
    let mut others = Vec::new();
    if mode.reads_text() {
        window.until("the end of the long channel is drawn", |window| {
            window
                .view()
                .workspace()
                .and_then(|workspace| workspace.feed.as_ref())
                .is_some_and(|feed| feed.loaded)
                && window.in_shell(|shell, _, _| {
                    shell.feed_bounds(junto_client::shell::Pane::Main).is_some()
                })
        });
    } else {
        others = enter_call(&world, &mut window, &place, mode);
    }
    let mut text = Wheel::new();
    hold(&mut window, mode, &mut text, SETTLING);
    let draws_before = draws(&mut window);
    let mut samples = Vec::new();
    let start = Instant::now();
    samples.push(process::sample(start, ALLOCATOR.held()));
    let profile_at = measured_time / 2;
    let mut profiled = std::env::var_os("JUNTO_TEST_MODES_PROFILE").is_none();
    while start.elapsed() < measured_time {
        hold(&mut window, mode, &mut text, INTERVAL);
        if !profiled && start.elapsed() >= profile_at {
            profiled = true;
            write_profile(&mut window, mode, &mut text);
        }
        let reading = process::sample(start, ALLOCATOR.held());
        println!(
            "modes: {} reading {:>4}s footprint {:.1} MiB, graphics {:.1} MiB, pixel buffers {:.1} MiB, Rust allocations {:.1} MiB, processor {:.2}s, {} threads, {} files",
            mode.name(),
            reading.at.as_secs(),
            mib(reading.footprint),
            mib(reading.graphics),
            mib(reading.surfaces),
            mib(reading.rust),
            reading.processor.as_secs_f64(),
            reading.threads,
            reading.files
        );
        samples.push(reading);
    }
    let (all_draws, picture_draws) = draws(&mut window);
    let drawn = (all_draws - draws_before.0, picture_draws - draws_before.1);
    let mut failures = check_mode(&mut window, mode, &text, drawn);
    for other in &mut others {
        if let Some(status) = other.child.try_wait().unwrap() {
            failures.push(format!("a test participant ended with {status}"));
        }
    }
    println!(
        "modes: {}: the window was drawn {} times, {} of them for pictures of the call; the wheel turned {} times",
        mode.name(),
        drawn.0,
        drawn.1,
        text.turns
    );
    drop(others);
    drop(window);
    drop(native);
    if !failures.is_empty() {
        for failure in &failures {
            println!("modes: {}: not in the mode: {failure}", mode.name());
        }
        return ExitCode::FAILURE;
    }
    println!("{}", Measured::of(&samples, drawn.0).line(mode));
    ExitCode::SUCCESS
}

/// The checks after the measured time that the window was in the mode all the time.
#[cfg(target_os = "macos")]
fn check_mode(
    window: &mut Window,
    mode: Mode,
    wheel: &Wheel,
    drawn: (usize, usize),
) -> Vec<String> {
    let mut failures = Vec::new();
    let call = window
        .state
        .read_with(&window.cx, |state, _| state.calls().view().call);
    let cameras = window.state.read_with(&window.cx, |state, _| {
        state.calls().camera_pictures().borrow().len()
    });
    match mode {
        Mode::Idle | Mode::Text => {
            if call.is_some() {
                failures.push("the window is in a call".to_owned());
            }
        }
        _ => {
            if !call
                .as_ref()
                .is_some_and(|call| call.phase == junto_media::calls::Phase::Connected)
            {
                failures.push(format!("the call is not connected: {call:?}"));
            }
        }
    }
    if mode == Mode::Text && (wheel.turns == 0 || wheel.missed > 0) {
        failures.push(format!(
            "the list left its end and came back {} times, and {} motions of the wheel did not reach their place",
            wheel.turns, wheel.missed
        ));
    }
    let wanted_cameras = match mode {
        Mode::Camera => 2,
        Mode::Cameras => 4,
        _ => 0,
    };
    if cameras != wanted_cameras {
        failures.push(format!("{cameras} cameras are drawn, not {wanted_cameras}"));
    }
    if mode == Mode::Share {
        let share = window.view().call.map(|call| call.share);
        if share != Some(junto_client::state::view::OwnShare::Running) {
            failures.push(format!("the own share is {share:?}"));
        }
        if !watches_a_share(window) {
            failures.push("the window does not watch the share of the other person".to_owned());
        }
    }
    // A video mode draws pictures of the call through the whole time.
    if matches!(mode, Mode::Camera | Mode::Share | Mode::Cameras) && drawn.1 < 100 {
        failures.push(format!("only {} drawings for pictures", drawn.1));
    }
    failures
}

/// The server, the workspace and the channels of a mode, and the people besides the window.
#[cfg(target_os = "macos")]
struct Place {
    workspace_id: WorkspaceId,
    voice: ChannelId,
    /// One device for each other person, logged in as a member. They are held so that
    /// their accounts stay logged in while the mode is measured.
    _devices: Vec<support::Device>,
    other_accounts: Vec<(ServerHandle, AccountId)>,
}

#[cfg(target_os = "macos")]
impl Place {
    /// Makes the profile of the window on `directory` before the window opens: the name of
    /// the first start, the server claimed with the owner code, and for the modes of text
    /// the long channel. Then the client of the profile is closed, so that the window
    /// starts on it as on a profile of a person who used the application before.
    fn prepare(
        world: &harness::World,
        server: &support::TestServer,
        directory: &std::path::Path,
        mode: Mode,
    ) -> Self {
        let client = support::Device::open(
            directory,
            junto_client_core::version::SupportedVersions::THIS_BUILD,
        );
        client.profile().set_name("Ada").unwrap();
        let owner = world.run(async {
            let handle = support::add(&client, &server.address()).await;
            handle
                .dispatch(support::claim(server, "Ada"))
                .await
                .unwrap();
            support::logged_in(&handle);
            handle
        });
        let workspace_id = world.run(owner.api().workspaces()).unwrap()[0].id;
        let channels = world
            .run(owner.api().channels(workspace_id))
            .unwrap()
            .channels;
        let named = |name: &str| {
            channels
                .iter()
                .find(|channel| channel.name == name)
                .unwrap_or_else(|| panic!("no channel is named {name}"))
                .id
        };
        let (general, voice) = (named("general"), named("voice"));
        let people = if mode.reads_text() {
            1
        } else {
            mode.others().len()
        };
        let names = ["Bea", "Cyd", "Dov"];
        let others: Vec<support::Device> = names[..people]
            .iter()
            .map(|name| member(world, &server.address(), &owner, name))
            .collect();
        let other_accounts = others
            .iter()
            .map(|device| {
                let handle = device.client.servers().into_iter().next().unwrap();
                let account = support::logged_in(&handle);
                (handle, account)
            })
            .collect::<Vec<_>>();
        if mode.reads_text() {
            let writers = [owner.clone(), other_accounts[0].0.clone()];
            fill(world, general, &writers);
        }
        drop(owner);
        drop(client);
        Self {
            workspace_id,
            voice,
            _devices: others,
            other_accounts,
        }
    }
}

/// A member of the server on a device of its own, which joins with an invite of the owner.
#[cfg(target_os = "macos")]
fn member(
    world: &harness::World,
    address: &str,
    owner: &ServerHandle,
    name: &str,
) -> support::Device {
    let code = world
        .run(
            owner
                .api()
                .create_invite(&junto_client_core::protocol::invite::CreateInvite {
                    max_uses: Some(1),
                    ..Default::default()
                }),
        )
        .unwrap()
        .code;
    world.run(async {
        let device = support::Device::new();
        let handle = device.add(address).await;
        handle.dispatch(support::join(&code, name)).await.unwrap();
        support::logged_in(&handle);
        device
    })
}

/// Writes the long channel: `MESSAGES` messages, the writers in turn. A writer that the
/// server refuses for its rate of writes waits for the time that the server states (ADR
/// 0205).
#[cfg(target_os = "macos")]
fn fill(world: &harness::World, channel: ChannelId, writers: &[ServerHandle]) {
    use junto_client_core::protocol::message::{Block, Document, Inline, SendMessage};
    let place = junto_client_core::model::PlaceId::Channel(channel);
    world.run_for(FILL_LIMIT, async {
        for number in 0..MESSAGES {
            let api = writers[number % writers.len()].api();
            let message = SendMessage {
                document: Document {
                    blocks: vec![Block::Paragraph {
                        content: vec![Inline::Text {
                            text: format!(
                                "Line {number} of the long channel, with enough words to fill a row of the list"
                            ),
                            marks: Vec::new(),
                        }],
                    }],
                },
                reply_to: None,
                attachments: Vec::new(),
            };
            loop {
                match api.send_message(place, &message).await {
                    Ok(_) => break,
                    Err(error) => match error.retry_after() {
                        Some(wait) => tokio::time::sleep(wait).await,
                        None => panic!("the server refused a message: {error}"),
                    },
                }
            }
        }
    });
}

/// A process of the test participant in the call.
#[cfg(target_os = "macos")]
struct Other {
    child: std::process::Child,
}

#[cfg(target_os = "macos")]
impl Drop for Other {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(target_os = "macos")]
fn participant_tool() -> String {
    std::env::var("JUNTO_TEST_PARTICIPANT").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../spikes/test-participant/target/release/test-participant"
        )
        .to_owned()
    })
}

/// The window enters the voice channel and its call; the other people of the mode join
/// the call as test participants, and the window turns on its camera or its share as the
/// mode states. Returns when the window draws everything that the mode sends.
#[cfg(target_os = "macos")]
fn enter_call(
    world: &harness::World,
    window: &mut Window,
    place: &Place,
    mode: Mode,
) -> Vec<Other> {
    let server_id = window.view().selected.as_ref().unwrap().id;
    let (workspace_id, voice) = (place.workspace_id, place.voice);
    window.state.update(&mut window.cx, |state, cx| {
        state.select_channel(workspace_id, voice, cx);
        state.join_call(server_id, workspace_id, voice, cx);
    });
    window.until("the call is connected", |window| {
        window
            .view()
            .call
            .is_some_and(|call| call.phase == junto_media::calls::Phase::Connected)
    });
    let mut others = Vec::new();
    for ((handle, _), arguments) in place.other_accounts.iter().zip(mode.others()) {
        let join = world.run(handle.api().join_channel_call(voice)).unwrap();
        let child = std::process::Command::new(participant_tool())
            .arg("publish")
            .args(["--url", &join.media_url, "--token", join.token.expose()])
            .args(arguments)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        others.push(Other { child });
    }
    let people = 1 + others.len();
    window.until("every person of the mode is in the call", |window| {
        occupants(window) == people
    });
    window.until("the window hears the other people", |window| {
        harness::heard(&window.sound, 440.0) > harness::HEARD_DBFS
    });
    if matches!(mode, Mode::Camera | Mode::Cameras) {
        start_camera(window);
    }
    if mode == Mode::Share {
        let sharer = place.other_accounts[0].1;
        window.until("the share of the other person is listed", |window| {
            share_stage(window)
                .is_some_and(|stage| stage.shares.iter().any(|share| share.account_id == sharer))
        });
        window.state.update(&mut window.cx, |state, _| {
            state.watch_share(junto_media::engine::ShareId::Of(sharer));
        });
        let seat = window
            .state
            .read_with(&window.cx, |state, _| state.calls().view().call)
            .expect("the window is in the call")
            .seat();
        window.state.update(&mut window.cx, |state, _| {
            state.start_share(
                &seat,
                junto_media::share::PictureSource::Display(1),
                junto_media::share::SoundChoice::None,
            );
        });
        window.until("the own share runs", |window| {
            window.view().call.map(|call| call.share)
                == Some(junto_client::state::view::OwnShare::Running)
        });
        window.until(
            "the window draws the share of the other person",
            watches_a_share,
        );
    }
    let cameras = match mode {
        Mode::Camera => 2,
        Mode::Cameras => 4,
        _ => 0,
    };
    window.until("the window draws every camera of the mode", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state.calls().camera_pictures().borrow().len()
        }) == cameras
    });
    others
}

#[cfg(target_os = "macos")]
fn start_camera(window: &mut Window) {
    let call = window
        .state
        .read_with(&window.cx, |state, _| state.calls().view().call)
        .expect("the window is in the call");
    let action = junto_client::state::CameraAction::drawn_for(&call);
    window
        .state
        .update(&mut window.cx, |state, cx| state.use_camera(&action, cx));
    window.until("the camera of the window runs", |window| {
        window.state.read_with(&window.cx, |state, _| {
            state
                .calls()
                .view()
                .call
                .is_some_and(|call| call.camera == junto_media::camera::CameraState::Running)
        })
    });
}

/// The people that the window lists in the call of the voice channel.
#[cfg(target_os = "macos")]
fn occupants(window: &mut Window) -> usize {
    window
        .channels()
        .into_iter()
        .find(|row| row.kind == junto_client_core::protocol::structure::ChannelKind::Voice)
        .and_then(|row| row.voice)
        .map_or(0, |voice| voice.occupants.len())
}

#[cfg(target_os = "macos")]
fn share_stage(window: &mut Window) -> Option<junto_client::state::view::ShareStage> {
    window
        .view()
        .workspace()
        .and_then(|workspace| workspace.call.clone())
        .and_then(|panel| panel.stage)
}

/// The window watches a share of another person and holds a picture of it.
#[cfg(target_os = "macos")]
fn watches_a_share(window: &mut Window) -> bool {
    let watched = share_stage(window).is_some_and(|stage| {
        stage.watched().is_some_and(|share| {
            share.size.is_some() && Some(share.account_id) != window_account(window)
        })
    });
    watched
        && window.state.read_with(&window.cx, |state, _| {
            state.calls().main_picture().borrow().is_some()
        })
}

#[cfg(target_os = "macos")]
fn window_account(window: &mut Window) -> Option<AccountId> {
    window.view().person.account_id
}

/// The motion of the wheel of the pointer in the text mode.
#[cfg(target_os = "macos")]
struct Wheel {
    /// The time in the current direction.
    moved: Duration,
    /// True while the wheel moves the list towards older messages.
    up: bool,
    /// The list left its end in the last motion up.
    left_end: bool,
    /// The times the list left its end and came back to it.
    turns: usize,
    /// The motions up that did not leave the end, and the motions down after one that left
    /// it that did not come back to the end.
    missed: usize,
}

#[cfg(target_os = "macos")]
impl Wheel {
    fn new() -> Self {
        Self {
            moved: Duration::ZERO,
            up: true,
            left_end: false,
            turns: 0,
            missed: 0,
        }
    }

    /// Turns the direction after `WHEEL_TURN`, and counts a motion up that left the end
    /// of the list followed by a motion down that came back to it.
    fn turn_if_due(&mut self, window: &mut Window) {
        if self.moved < WHEEL_TURN {
            return;
        }
        let at_end =
            window.in_shell(|shell, _, _| shell.feed_is_at_end(junto_client::shell::Pane::Main));
        if self.up {
            self.left_end = !at_end;
            self.missed += usize::from(at_end);
        } else if self.left_end {
            self.turns += usize::from(at_end);
            self.missed += usize::from(!at_end);
        }
        self.up = !self.up;
        self.moved = Duration::ZERO;
    }
}

/// Holds the mode for this time: frames of the display at the rate of a display, and in
/// the text mode one turn of the wheel of the pointer before each frame.
#[cfg(target_os = "macos")]
fn hold(window: &mut Window, mode: Mode, wheel: &mut Wheel, time: Duration) {
    if mode != Mode::Text {
        process::give_frames(window, time);
        return;
    }
    let end = Instant::now() + time;
    let mut next = Instant::now();
    while Instant::now() < end {
        wheel.turn_if_due(window);
        scroll(
            window,
            if wheel.up {
                WHEEL_PER_FRAME
            } else {
                -WHEEL_PER_FRAME
            },
        );
        window.holding(|window| {
            window.park();
            window.frame();
            window.park();
        });
        wheel.moved += process::FRAME;
        next += process::FRAME;
        if let Some(rest) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(rest);
        }
    }
}

/// Turns the wheel of the pointer over the message list: a positive distance moves the
/// list towards older messages.
#[cfg(target_os = "macos")]
fn scroll(window: &mut Window, distance: f32) {
    use gpui::{ScrollDelta, ScrollWheelEvent, point, px};
    let Some(bounds) =
        window.in_shell(|shell, _, _| shell.feed_bounds(junto_client::shell::Pane::Main))
    else {
        return;
    };
    window.dispatch(ScrollWheelEvent {
        position: bounds.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(distance))),
        ..Default::default()
    });
}

/// How often the windows were drawn for any cause, and for pictures of a call.
#[cfg(target_os = "macos")]
fn draws(window: &mut Window) -> (usize, usize) {
    let (all, pictures, _) = window.in_shell(|shell, _, _| shell.draws());
    (all, pictures)
}

/// Writes what uses the memory and the processor of this process into the directory of
/// `JUNTO_TEST_MODES_PROFILE`: the regions of `footprint -v`, the summary of `vmmap`, a
/// sample of the stacks of all threads over a few seconds, and the count of the Rust
/// allocations. The window gets no frames while `footprint` and `vmmap` read it; the sample
/// reads the process while the window draws the mode.
#[cfg(target_os = "macos")]
fn write_profile(window: &mut Window, mode: Mode, wheel: &mut Wheel) {
    let Some(directory) = std::env::var_os("JUNTO_TEST_MODES_PROFILE") else {
        return;
    };
    let directory = std::path::PathBuf::from(directory);
    std::fs::create_dir_all(&directory).unwrap();
    let pid = std::process::id().to_string();
    let name = mode.name();
    let run = |tool: &str, arguments: &[&str], file: String| {
        let output = std::process::Command::new(tool)
            .args(arguments)
            .output()
            .unwrap_or_else(|error| panic!("{tool} did not start: {error}"));
        let mut text = output.stdout;
        text.extend_from_slice(&output.stderr);
        std::fs::write(directory.join(file), text).unwrap();
    };
    run(
        "/usr/bin/footprint",
        &["-v", "-f", "bytes", &pid],
        format!("{name}-footprint.txt"),
    );
    run(
        "/usr/bin/vmmap",
        &["-summary", &pid],
        format!("{name}-vmmap.txt"),
    );
    run("/usr/bin/heap", &["-s", &pid], format!("{name}-heap.txt"));
    // With `MallocStackLogging=1` in the environment the system records the stack of each
    // allocation, and the live ones grouped by their stacks show which code holds the memory.
    if std::env::var_os("MallocStackLogging").is_some() {
        run(
            "/usr/bin/malloc_history",
            &[&pid, "-allBySize"],
            format!("{name}-allocations.txt"),
        );
    }
    std::fs::write(
        directory.join(format!("{name}-rust.txt")),
        format!("{}\n", ALLOCATOR.held()),
    )
    .unwrap();
    // The sample runs while the windows draw, so it shows the work of the mode.
    let sample_file = directory.join(format!("{name}-sample.txt"));
    let mut sampler = std::process::Command::new("/usr/bin/sample")
        .args([pid.as_str(), "5", "-mayDie", "-file"])
        .arg(&sample_file)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    while sampler.try_wait().unwrap().is_none() {
        hold(window, mode, wheel, Duration::from_millis(250));
    }
}
