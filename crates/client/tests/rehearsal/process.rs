//! What the process of a native window test uses: its memory as `footprint` states it, its
//! processor time, its open files and its threads. The native targets `frames` and
//! `endurance` read them while the windows draw (ADR 0902).

use std::time::{Duration, Instant};

/// One reading of the process.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// The time since the start of the measurement.
    pub at: Duration,
    /// The memory of the process that the system counts against it (`phys_footprint`),
    /// graphics memory included.
    pub footprint: u64,
    /// The dirty memory of the category "IOAccelerator (graphics)": the GPU textures of
    /// the windows are in it.
    pub graphics: u64,
    /// The dirty memory of the category "IOSurface": the pixel buffers of Core Video that
    /// the windows draw as surfaces (the cameras and the watched share) are in it.
    pub surfaces: u64,
    /// The bytes that the Rust allocations of the process hold, as the counting allocator
    /// of the program states them (`junto-alloc-count`).
    pub rust: u64,
    /// The processor time of all threads of the process since its start.
    pub processor: Duration,
    /// The open file descriptors: files, sockets, pipes and devices.
    pub files: usize,
    pub threads: usize,
}

pub fn sample(start: Instant, rust: usize) -> Sample {
    let (footprint, graphics, surfaces) = memory();
    Sample {
        at: start.elapsed(),
        footprint,
        graphics,
        surfaces,
        rust: rust as u64,
        processor: processor_time(),
        files: open_files(),
        threads: threads(),
    }
}

/// The memory of the pictures that the windows of this process draw, in bytes: the graphics
/// memory and the memory of the pixel buffers.
pub fn picture_memory() -> (u64, u64) {
    let (_, graphics, surfaces) = memory();
    (graphics, surfaces)
}

/// `phys_footprint`, the graphics memory and the memory of the pixel buffers of this
/// process, in bytes.
fn memory() -> (u64, u64, u64) {
    let output = std::process::Command::new("/usr/bin/footprint")
        .args(["-f", "bytes", &std::process::id().to_string()])
        .output()
        .expect("footprint runs");
    assert!(output.status.success(), "footprint failed: {output:?}");
    let text = String::from_utf8_lossy(&output.stdout);
    let first_number = |line: &str| line.split_whitespace().next()?.parse::<u64>().ok();
    // A process whose windows were drawn by Metal has this category. Without it the
    // graphics memory is unknown, and the bounds on it could not fail.
    let graphics = text
        .lines()
        .find(|line| line.trim_end().ends_with("IOAccelerator (graphics)"))
        .and_then(first_number)
        .unwrap_or_else(|| {
            panic!("footprint states no memory of the category IOAccelerator (graphics): {text}")
        });
    // The category exists only while the process holds a pixel buffer.
    let surfaces = text
        .lines()
        .find(|line| line.trim_end().ends_with("IOSurface"))
        .and_then(first_number)
        .unwrap_or_default();
    let footprint = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("phys_footprint:"))
        .and_then(first_number)
        .expect("footprint states phys_footprint");
    (footprint, graphics, surfaces)
}

/// The processor time of this process, as `ps` states it: `[[days-]hours:]minutes:seconds`.
fn processor_time() -> Duration {
    let output = std::process::Command::new("/bin/ps")
        .args(["-o", "time=", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs");
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days.parse::<f64>().unwrap_or_default(), clock),
        None => (0.0, text),
    };
    let seconds = clock.split(':').fold(0.0, |total, part| {
        total * 60.0 + part.parse::<f64>().unwrap_or_default()
    });
    Duration::from_secs_f64(days * 86_400.0 + seconds)
}

fn open_files() -> usize {
    // The listing itself holds one descriptor of the directory while it reads.
    std::fs::read_dir("/dev/fd")
        .expect("the descriptors of the process can be listed")
        .count()
        .saturating_sub(1)
}

fn threads() -> usize {
    let output = std::process::Command::new("/bin/ps")
        .args(["-M", "-p", &std::process::id().to_string()])
        .output()
        .expect("ps runs");
    // A line of titles, then one line for each thread.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .count()
        .saturating_sub(1)
}

/// The time of the frames that the windows were given.
#[derive(Debug, Default, Clone, Copy)]
pub struct Frames {
    pub count: usize,
    /// The longest time that one frame and the work of the application around it held
    /// the thread of the windows.
    pub longest: Duration,
}

impl Frames {
    pub fn add(&mut self, other: Frames) {
        self.count += other.count;
        self.longest = self.longest.max(other.longest);
    }
}

/// The time between two frames of the display that the programs give the windows.
pub const FRAME: Duration = Duration::from_millis(16);

/// Gives every window a frame of the display at the rate of a display, for this time,
/// and runs the work of the application between two frames. It returns how long each
/// frame with the work around it held the thread of the windows.
pub fn give_frames(window: &mut crate::harness::Window, time: Duration) -> Frames {
    let end = Instant::now() + time;
    let mut next = Instant::now();
    let mut frames = Frames::default();
    while Instant::now() < end {
        let started = Instant::now();
        window.holding(|window| {
            window.park();
            window.frame();
            window.park();
        });
        frames.count += 1;
        frames.longest = frames.longest.max(started.elapsed());
        next += FRAME;
        if let Some(rest) = next.checked_duration_since(Instant::now()) {
            std::thread::sleep(rest);
        }
    }
    window.holding(|window| window.cx.run_until_parked());
    frames
}
