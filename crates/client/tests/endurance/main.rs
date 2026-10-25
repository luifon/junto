//! The memory, the open files, the threads and the frames of the application over a long
//! use (ADR 0902). Two native windows of the application, A and B, and a third person C
//! without a window are in a call on the server of the tests with its media server. A and
//! B share the synthetic screen, and all three turn on the synthetic camera, so window A
//! draws two people with video beside its own camera and both shares. The call of A is in
//! its separate window; in the main window A reads a long text channel, scrolls it, opens
//! and closes threads and changes the channel, again and again.
//!
//! The program reads the process once in each cycle of the use: its memory as `footprint`
//! states it, graphics included, the bytes that its Rust allocations hold, its processor time, its open files and its threads. After a time of
//! warming, it fails when one of them grows at a rate per minute that a fixed set of
//! windows, pictures and messages does not reach, or when one piece of work of the
//! application holds the thread of the windows for longer than the bound of ADR 0730: the
//! work of one event with the work that its handlers start, or one frame of the display
//! with the work around it.
//!
//! It runs on the main thread, which the windows of the system need, so it is a test
//! without the test runner. It does nothing on other systems than macOS, and nothing
//! unless `JUNTO_TEST_ENDURANCE` is `1` and the variables of the media server of the tests
//! are set. `JUNTO_TEST_ENDURANCE_MINUTES` sets the time of the measured use (default 10).
//!
//! It measures an optimized build: without optimization the layout of gpui alone holds
//! the thread for hundreds of milliseconds. The harness finds the elements of a window by
//! the names that gpui keeps only with debug assertions, so the build is the release
//! profile with debug assertions:
//!
//! ```sh
//! export CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true
//! cargo build --workspace --release
//! JUNTO_TEST_ENDURANCE=1 tools/with-test-postgres.sh tools/with-test-livekit.sh \
//!     tools/serial.sh cargo test --release -p junto-client --test endurance
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
#[path = "../rehearsal/growth.rs"]
mod growth;
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
#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../app/rehearsal.rs"]
mod rehearsal;

#[cfg(target_os = "macos")]
use harness::Window;

/// The allocator of the program counts the bytes that its Rust allocations hold: the
/// footprint varies by tens of MiB with the free pages that the allocator of the system
/// keeps, the count only with what the program holds.
#[cfg(target_os = "macos")]
#[global_allocator]
static ALLOCATOR: junto_alloc_count::Counting = junto_alloc_count::Counting::new();
#[cfg(target_os = "macos")]
use junto_client_core::ServerHandle;
#[cfg(target_os = "macos")]
use junto_client_core::protocol::ids::{ChannelId, MessageId};
#[cfg(target_os = "macos")]
use process::{Frames, Sample};
#[cfg(target_os = "macos")]
use rehearsal::{Deployment, Rehearsal};

/// The time after the start of the video in which the caches of the windows, the GPU and
/// the call library fill. It is not measured. In a run of the debug build the footprint
/// grew by about 9 MiB in the first 5 minutes of the use and stayed level after that.
#[cfg(target_os = "macos")]
const WARMING: Duration = Duration::from_secs(5 * 60);
/// The steps of one cycle of the use ([`Use::act`]). The process is read once a cycle,
/// after its last step: the main window then always shows the end of the same channel
/// with no thread open, so two readings compare the same state of the windows. A reading
/// at a fixed interval falls on different steps, where the windows hold other messages.
#[cfg(target_os = "macos")]
const CYCLE: usize = 8;
/// The medians of the first and of the last minute of the measured use are shown.
#[cfg(target_os = "macos")]
const WINDOW: Duration = Duration::from_secs(60);
/// How fast each reading may grow over the measured use, per minute, as the slope of all
/// its readings ([`growth::slope`]). The windows hold a fixed number of pictures (ADR
/// 0774) and a bounded part of a channel (ADR 0747), so a use that repeats the same
/// actions keeps its memory.
///
/// The memory of the Rust allocations is the count of the allocator of the program: it
/// changes only when the program allocates or frees. A run that held 1 MiB a minute more
/// measured 1.02 MiB a minute and failed; the run of the committed code measured 0.01.
///
/// The footprint also holds the memory of the native libraries (Metal, the media engine)
/// and changes with the free pages that the allocator of the system keeps: runs without a
/// leak measured -2.93 to 1.25 MiB a minute. Its bound only fails a leak of native memory of
/// several MiB a minute. A leak of one file or thread at each thread that is opened (about
/// 4 a minute) or at each channel change (about 13 a minute) is above the bounds of files
/// and threads.
#[cfg(target_os = "macos")]
const MOST_FOOTPRINT_PER_MINUTE: f64 = 3.0 * MIB;
#[cfg(target_os = "macos")]
const MOST_RUST_PER_MINUTE: f64 = 0.5 * MIB;
#[cfg(target_os = "macos")]
const MOST_GRAPHICS_PER_MINUTE: f64 = 0.5 * MIB;
#[cfg(target_os = "macos")]
const MOST_FILES_PER_MINUTE: f64 = 2.0;
#[cfg(target_os = "macos")]
const MOST_THREADS_PER_MINUTE: f64 = 2.0;
#[cfg(target_os = "macos")]
const MIB: f64 = 1024.0 * 1024.0;
/// The longest time that one piece of work of the application may hold the thread of the
/// windows after the warming, and the share of the pieces that may take longer than
/// [`harness::Held::SLOW`] (ADR 0730).
#[cfg(target_os = "macos")]
const LONGEST_HOLD: Duration = Duration::from_millis(150);
#[cfg(target_os = "macos")]
const PIECES_PER_SLOW_PIECE: usize = 4000;
/// The messages of the long channel, how many of the newest are roots of a thread, and the
/// answers in each thread.
#[cfg(target_os = "macos")]
const MESSAGES: usize = 400;
/// The limit of the writes of the long channel, which wait for the rate of writes of the
/// server.
#[cfg(target_os = "macos")]
const FILL_LIMIT: Duration = Duration::from_secs(1800);
#[cfg(target_os = "macos")]
const THREADS: usize = 4;
#[cfg(target_os = "macos")]
const ANSWERS: usize = 10;

#[cfg(not(target_os = "macos"))]
fn main() -> ExitCode {
    println!("endurance: not run, the native windows run only on macOS");
    ExitCode::SUCCESS
}

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    if std::env::var("JUNTO_TEST_ENDURANCE").as_deref() != Ok("1") {
        println!("endurance: not run, JUNTO_TEST_ENDURANCE is not 1");
        return ExitCode::SUCCESS;
    }
    if std::env::var("JUNTO_TEST_LIVEKIT_URL").is_err() {
        println!("endurance: not run, JUNTO_TEST_LIVEKIT_URL is not set");
        return ExitCode::SUCCESS;
    }
    if env!("JUNTO_OPT_LEVEL") == "0" || !cfg!(debug_assertions) {
        println!(
            "endurance: not run: it measures the release profile with debug assertions; build it with --release and CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true"
        );
        return ExitCode::FAILURE;
    }
    let minutes = std::env::var("JUNTO_TEST_ENDURANCE_MINUTES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(10);
    let measured = Duration::from_secs(minutes * 60);
    assert!(
        measured >= 2 * WINDOW,
        "the measured use must be at least {:?}",
        2 * WINDOW
    );

    let mut native = native::Native::start();
    let world = harness::World::new();
    let server = world.server_with_media("Endurance");
    let profiles = tempfile::tempdir().unwrap();
    let deployment = Deployment {
        address: server.address(),
        owner_code: server.owner_code().unwrap(),
    };
    let mut run = Rehearsal::new(
        &mut native.cx,
        profiles.path(),
        deployment,
        true,
        None,
        "endurance",
    );
    run.first_owner();
    run.invitation();
    let place = Place::of(&world, &mut run.a);
    let third = third_person(&world, &server.address(), &place.owner);
    let roots = place.fill(&world, &[place.owner.clone(), third.handle.clone()]);
    run.call();
    run.video();
    world
        .run(third.calls.join(place.voice_target(&third)))
        .unwrap();
    until(&world, "the third person is connected", || {
        third
            .calls
            .view()
            .call
            .is_some_and(|call| call.phase == junto_media::calls::Phase::Connected)
    });
    third.calls.start_camera(None);
    until(&world, "the camera of the third person runs", || {
        third
            .calls
            .view()
            .call
            .is_some_and(|call| call.camera == junto_media::camera::CameraState::Running)
    });
    run.a
        .until("A draws the camera of the third person", |window| {
            window.state.read_with(&window.cx, |state, _| {
                state.calls().camera_pictures().borrow().len() == 3
            })
        });
    run.a.click("detach-call");
    run.a.until("the call is in its own window", |window| {
        window.shows("call-in-separate-window")
    });
    run.a.click_channel("general");

    let start = Instant::now();
    let mut use_of = Use {
        roots,
        threads_opened: 0,
        changes: 0,
        scrolls: 0,
    };
    let mut samples: Vec<Sample> = Vec::new();
    let mut frames = Frames::default();
    let mut warming_frames = Frames::default();
    let mut warming_held = None;
    let mut step = 0usize;
    run.a.held = harness::Held::default();
    while start.elapsed() < WARMING + measured {
        let warm = start.elapsed() >= WARMING;
        if warm && warming_held.is_none() {
            warming_held = Some(std::mem::take(&mut run.a.held));
        }
        let given = use_of.act(&mut run.a, step);
        if warm {
            frames.add(given);
        } else {
            warming_frames.add(given);
        }
        step += 1;
        if warm && step.is_multiple_of(CYCLE) {
            let reading = process::sample(start, ALLOCATOR.held());
            println!(
                "endurance: reading {:>4}s footprint {} KiB, Rust allocations {} KiB, graphics {} KiB, processor {:.1}s, {} files, {} threads",
                reading.at.as_secs(),
                reading.footprint >> 10,
                reading.rust >> 10,
                reading.graphics >> 10,
                reading.processor.as_secs_f64(),
                reading.files,
                reading.threads
            );
            samples.push(reading);
        }
    }
    let held = run.a.held;
    let still_drawn = run.a.state.read_with(&run.a.cx, |state, _| {
        state.calls().camera_pictures().borrow().len()
    });

    let mut failures = judge(&samples, frames, held, &use_of, still_drawn);
    let warming_held = warming_held.unwrap_or_default();
    println!(
        "endurance: while warming {} frames, the longest {:?}; {} pieces of work, the longest {:?}, {} longer than {:?}",
        warming_frames.count,
        warming_frames.longest,
        warming_held.count,
        warming_held.longest,
        warming_held.slow,
        harness::Held::SLOW
    );
    drop(third);
    drop(run);
    drop(native);
    // A panic in a handler or a task stops in gpui and no longer ends the program
    // (ADR 0757).
    let panics = junto_client::panics::panics_in_this_process();
    if panics > 0 {
        failures.push(format!(
            "{panics} panics happened in the process: {:?}",
            junto_client::panics::messages_on_this_thread()
        ));
    }
    if failures.is_empty() {
        println!("endurance: passed");
        ExitCode::SUCCESS
    } else {
        for failure in &failures {
            println!("endurance: failed: {failure}");
        }
        ExitCode::FAILURE
    }
}

/// Waits at most `PATIENCE` for a state of the person without a window.
#[cfg(target_os = "macos")]
#[track_caller]
fn until(world: &harness::World, what: &str, holds: impl Fn() -> bool) {
    let deadline = Instant::now() + harness::PATIENCE;
    while !holds() {
        assert!(
            Instant::now() < deadline,
            "after {:?}: {what}",
            harness::PATIENCE
        );
        world.run(async { tokio::time::sleep(Duration::from_millis(20)).await });
    }
}

/// The server, the workspace and the channels of the use.
#[cfg(target_os = "macos")]
struct Place {
    owner: ServerHandle,
    workspace_id: junto_client_core::protocol::ids::WorkspaceId,
    general: ChannelId,
    voice: ChannelId,
}

#[cfg(target_os = "macos")]
impl Place {
    fn of(world: &harness::World, window: &mut Window) -> Self {
        let owner = window
            .state
            .read_with(&window.cx, |state, _| state.client().servers())
            .into_iter()
            .next()
            .expect("window A knows its server");
        let workspace_id = window.view().workspace().expect("a workspace is shown").id;
        let channels = window.channels();
        let named = |name: &str| {
            channels
                .iter()
                .find(|row| row.name == name)
                .unwrap_or_else(|| panic!("no channel is named {name}"))
                .id
        };
        let (general, voice) = (named("general"), named("voice"));
        world
            .run(owner.api().create_channel(
                workspace_id,
                &junto_client_core::protocol::structure::CreateChannel {
                    name: "notes".to_owned(),
                    kind: junto_client_core::protocol::structure::ChannelKind::Text,
                    category_id: None,
                    position: None,
                },
            ))
            .unwrap();
        window.until("the second text channel is listed", |window| {
            window.channel_row("notes").is_some()
        });
        Self {
            owner,
            workspace_id,
            general,
            voice,
        }
    }

    /// Writes the long channel: `MESSAGES` messages, the newest `THREADS` of them with
    /// answers in their thread. The writers take turns; a writer that the server refuses
    /// for its rate of writes waits for the time that the server states.
    fn fill(&self, world: &harness::World, writers: &[ServerHandle]) -> Vec<MessageId> {
        let place = junto_client_core::model::PlaceId::Channel(self.general);
        world.run_for(FILL_LIMIT, async {
            let mut roots = Vec::new();
            let mut turn = 0usize;
            let mut next = || {
                turn += 1;
                writers[turn % writers.len()].api()
            };
            for number in 0..MESSAGES {
                let text = paragraph(&format!("Long channel line {number}"));
                let sent = written(|| {
                    let api = next();
                    let text = text.clone();
                    async move { api.send_message(place, &text).await }
                })
                .await;
                if number >= MESSAGES - THREADS {
                    for answer in 0..ANSWERS {
                        let text = paragraph(&format!("Answer {answer} to line {number}"));
                        written(|| {
                            let api = next();
                            let text = text.clone();
                            async move { api.reply_in_thread(sent.id, &text).await }
                        })
                        .await;
                    }
                    roots.push(sent.id);
                }
            }
            roots
        })
    }

    fn voice_target(&self, person: &harness::Caller) -> junto_media::calls::CallTarget {
        junto_media::calls::CallTarget {
            server_id: person.handle.id(),
            place: junto_client_core::protocol::call::CallPlace::Channel {
                workspace_id: self.workspace_id,
                channel_id: self.voice,
            },
        }
    }
}

/// Sends a write until the server takes it. A refusal for the rate of writes states when
/// to send again (ADR 0205).
#[cfg(target_os = "macos")]
async fn written<F, W>(mut write: W) -> junto_client_core::protocol::message::Message
where
    W: FnMut() -> F,
    F: std::future::Future<
            Output = Result<
                junto_client_core::protocol::message::Message,
                junto_client_core::api::ApiError,
            >,
        >,
{
    loop {
        match write().await {
            Ok(message) => return message,
            Err(error) => match error.retry_after() {
                Some(wait) => tokio::time::sleep(wait).await,
                None => panic!("the server refused a message: {error}"),
            },
        }
    }
}

#[cfg(target_os = "macos")]
fn paragraph(text: &str) -> junto_client_core::protocol::message::SendMessage {
    use junto_client_core::protocol::message::{Block, Document, Inline, SendMessage};
    SendMessage {
        document: Document {
            blocks: vec![Block::Paragraph {
                content: vec![Inline::Text {
                    text: text.to_owned(),
                    marks: Vec::new(),
                }],
            }],
        },
        reply_to: None,
        attachments: Vec::new(),
    }
}

/// A third person without a window: a device that joins with an invite of A, with the
/// synthetic devices of the tests.
#[cfg(target_os = "macos")]
fn third_person(world: &harness::World, address: &str, owner: &ServerHandle) -> harness::Caller {
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
    let admin = world.run(async {
        let device = support::Device::new();
        let handle = device.add(address).await;
        handle.dispatch(support::join(&code, "Cyd")).await.unwrap();
        support::logged_in(&handle);
        harness::Admin { device, handle }
    });
    admin.caller(world)
}

/// What window A does with its main window, one action at a time, with frames of the
/// display between the actions.
#[cfg(target_os = "macos")]
struct Use {
    roots: Vec<MessageId>,
    threads_opened: usize,
    changes: usize,
    scrolls: usize,
}

#[cfg(target_os = "macos")]
impl Use {
    /// One action, then two seconds of frames. Returns the frames that were given.
    fn act(&mut self, window: &mut Window, step: usize) -> Frames {
        let mut frames = Frames::default();
        match step % CYCLE {
            // Up the history of the channel and back to its end.
            0..=2 => {
                for _ in 0..6 {
                    scroll(window, 2000.0);
                    frames.add(process::give_frames(window, Duration::from_millis(150)));
                }
                self.scrolls += 1;
            }
            3 => {
                for _ in 0..30 {
                    scroll(window, -4000.0);
                }
                frames.add(process::give_frames(window, Duration::from_millis(300)));
            }
            // A thread of one of the newest messages, open for a moment.
            4 => {
                let root = self.roots[self.threads_opened % self.roots.len()];
                let row = harness::selector(format!("message:{root}"));
                if window.shows(row) {
                    window.right_click_named(format!("message:{root}"));
                    window.click("message-thread");
                    window.until("the thread is read", |window| {
                        window
                            .view()
                            .workspace()
                            .and_then(|workspace| workspace.thread.as_ref())
                            .is_some_and(|thread| {
                                thread.feed.as_ref().is_some_and(|feed| feed.loaded)
                            })
                    });
                    frames.add(process::give_frames(window, Duration::from_millis(500)));
                    window.click("close-thread");
                    self.threads_opened += 1;
                }
            }
            // The other channels and back.
            5 => {
                for name in ["notes", "voice", "general"] {
                    window.click_channel(name);
                    frames.add(process::give_frames(window, Duration::from_millis(200)));
                }
                self.changes += 3;
            }
            _ => {}
        }
        frames.add(process::give_frames(window, Duration::from_secs(2)));
        frames
    }
}

/// Turns the wheel of the pointer over the message list: a positive distance moves the
/// list towards older messages.
#[cfg(target_os = "macos")]
fn scroll(window: &mut Window, distance: f32) {
    use gpui::{ScrollDelta, ScrollWheelEvent, point, px};
    let Some(bounds) = window.cx.debug_bounds("feed") else {
        return;
    };
    window.dispatch(ScrollWheelEvent {
        position: bounds.center(),
        delta: ScrollDelta::Pixels(point(px(0.0), px(distance))),
        ..Default::default()
    });
}

/// One reading of the process and the most that it may grow in a minute.
#[cfg(target_os = "macos")]
struct Reading {
    name: &'static str,
    of: fn(&Sample) -> u64,
    most_per_minute: f64,
    bytes: bool,
}

/// The failures of the use: the growth of the readings per minute, the longest piece of
/// work on the thread of the windows, and actions that did not happen.
#[cfg(target_os = "macos")]
fn judge(
    samples: &[Sample],
    frames: Frames,
    held: harness::Held,
    use_of: &Use,
    still_drawn: usize,
) -> Vec<String> {
    let mut failures = Vec::new();
    let Some(last) = samples.last() else {
        return vec!["no reading of the process".to_owned()];
    };
    let first_at = samples[0].at;
    let early: Vec<&Sample> = samples
        .iter()
        .filter(|sample| sample.at < first_at + WINDOW)
        .collect();
    let late: Vec<&Sample> = samples
        .iter()
        .filter(|sample| sample.at + WINDOW > last.at)
        .collect();
    let median = |part: &[&Sample], of: fn(&Sample) -> u64| {
        let mut values: Vec<u64> = part.iter().map(|sample| of(sample)).collect();
        values.sort_unstable();
        values[values.len() / 2]
    };
    let minutes = (last.at - first_at).as_secs_f64() / 60.0;
    let processor = last.processor.saturating_sub(samples[0].processor);
    println!(
        "endurance: {} readings over {minutes:.1} minutes; processor {:.0}% of one core; {} frames, the longest {:?}; {} pieces of work, the longest {:?}, {} longer than {:?}; {} scrolls, {} threads opened, {} channel changes",
        samples.len(),
        100.0 * processor.as_secs_f64() / (last.at - first_at).as_secs_f64().max(1.0),
        frames.count,
        frames.longest,
        held.count,
        held.longest,
        held.slow,
        harness::Held::SLOW,
        use_of.scrolls,
        use_of.threads_opened,
        use_of.changes,
    );
    let readings = [
        Reading {
            name: "footprint",
            of: |sample| sample.footprint,
            most_per_minute: MOST_FOOTPRINT_PER_MINUTE,
            bytes: true,
        },
        Reading {
            name: "memory of the Rust allocations",
            of: |sample| sample.rust,
            most_per_minute: MOST_RUST_PER_MINUTE,
            bytes: true,
        },
        Reading {
            name: "graphics memory",
            of: |sample| sample.graphics,
            most_per_minute: MOST_GRAPHICS_PER_MINUTE,
            bytes: true,
        },
        Reading {
            name: "open files",
            of: |sample| sample.files as u64,
            most_per_minute: MOST_FILES_PER_MINUTE,
            bytes: false,
        },
        Reading {
            name: "threads",
            of: |sample| sample.threads as u64,
            most_per_minute: MOST_THREADS_PER_MINUTE,
            bytes: false,
        },
    ];
    for reading in readings {
        let (before, after) = (median(&early, reading.of), median(&late, reading.of));
        let points: Vec<(f64, f64)> = samples
            .iter()
            .map(|sample| {
                (
                    (sample.at - first_at).as_secs_f64() / 60.0,
                    (reading.of)(sample) as f64,
                )
            })
            .collect();
        let per_minute = growth::slope(&points).unwrap_or_default();
        let shown = |value: f64| {
            if reading.bytes {
                format!("{:.2} MiB", value / MIB)
            } else {
                format!("{value:.2}")
            }
        };
        println!(
            "endurance: {}: {} in the first minute, {} in the last; {} a minute (at most {})",
            reading.name,
            shown(before as f64),
            shown(after as f64),
            shown(per_minute),
            shown(reading.most_per_minute)
        );
        if per_minute > reading.most_per_minute {
            failures.push(format!(
                "the {} grew by {} a minute over {minutes:.1} minutes, from {} to {}",
                reading.name,
                shown(per_minute),
                shown(before as f64),
                shown(after as f64)
            ));
        }
    }
    if held.longest > LONGEST_HOLD {
        failures.push(format!(
            "the application held the thread of the windows for {:?} in one piece of work",
            held.longest
        ));
    }
    if held.slow * PIECES_PER_SLOW_PIECE > held.count {
        failures.push(format!(
            "{} of {} pieces of work took longer than {:?}",
            held.slow,
            held.count,
            harness::Held::SLOW
        ));
    }
    if use_of.threads_opened == 0 || use_of.changes == 0 || use_of.scrolls == 0 {
        failures.push("an action of the text use did not happen".to_owned());
    }
    if still_drawn != 3 {
        failures.push(format!(
            "window A drew {still_drawn} cameras at the end, not 3"
        ));
    }
    failures
}
