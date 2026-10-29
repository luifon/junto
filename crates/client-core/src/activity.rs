//! Inactivity in the desktop session, read away from the window thread.
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU8, Ordering},
};
use std::time::{Duration, Instant};

pub const IDLE_AFTER: Duration = Duration::from_secs(5 * 60);
const READ_EVERY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityState {
    Starting,
    Active,
    Idle,
    Unavailable,
}

struct Shared {
    state: AtomicU8,
    stop: AtomicBool,
}
impl Shared {
    fn set(&self, idle: Option<bool>) {
        self.state.store(
            match idle {
                Some(false) => 1,
                Some(true) => 2,
                None => 3,
            },
            Ordering::SeqCst,
        );
    }
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

pub struct SystemActivity {
    shared: Arc<Shared>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl Default for SystemActivity {
    fn default() -> Self {
        Self::new()
    }
}
impl SystemActivity {
    pub fn new() -> Self {
        let shared = Arc::new(Shared {
            state: AtomicU8::new(0),
            stop: AtomicBool::new(false),
        });
        let work = shared.clone();
        let worker = std::thread::Builder::new()
            .name("junto-activity".into())
            .spawn(move || run(work))
            .ok();
        if worker.is_none() {
            shared.set(None);
        }
        Self { shared, worker }
    }
    pub fn state(&self) -> ActivityState {
        match self.shared.state.load(Ordering::SeqCst) {
            0 => ActivityState::Starting,
            1 => ActivityState::Active,
            2 => ActivityState::Idle,
            _ => ActivityState::Unavailable,
        }
    }
}
impl Drop for SystemActivity {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
        }
    }
}

fn run(shared: Arc<Shared>) {
    #[cfg(target_os = "linux")]
    let mut reader = linux::Reader::new();
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if wayland::run(&shared) || shared.stopped() {
            return;
        }
    }
    while !shared.stopped() {
        #[cfg(target_os = "linux")]
        let idle = reader.read();
        #[cfg(target_os = "macos")]
        let idle = macos::read();
        #[cfg(windows)]
        let idle = windows::read();
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        let idle = None;
        shared.set(idle);
        let until = Instant::now() + READ_EVERY;
        while !shared.stopped() && Instant::now() < until {
            std::thread::park_timeout(until.saturating_duration_since(Instant::now()));
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn idle_seconds(seconds: f64) -> Option<bool> {
    (seconds.is_finite() && seconds >= 0.).then(|| seconds >= IDLE_AFTER.as_secs_f64())
}
#[cfg(any(windows, test))]
fn tick_elapsed(last: u32, now: u32) -> Duration {
    Duration::from_millis(u64::from(now.wrapping_sub(last)))
}

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "linux")]
mod wayland;
#[cfg(windows)]
mod windows;

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    #[test]
    fn the_native_reader_starts_and_releases_its_worker() {
        if std::env::var_os("JUNTO_TEST_ACTIVITY").is_none() {
            return;
        }
        let activity = SystemActivity::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while activity.state() == ActivityState::Starting && Instant::now() < deadline {
            std::thread::park_timeout(Duration::from_millis(10));
        }
        assert_ne!(activity.state(), ActivityState::Starting);
        if std::env::var_os("DISPLAY").is_some() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            assert!(matches!(
                activity.state(),
                ActivityState::Active | ActivityState::Idle
            ));
        }
        let released = Arc::downgrade(&activity.shared);
        drop(activity);
        let deadline = Instant::now() + Duration::from_secs(3);
        while released.upgrade().is_some() && Instant::now() < deadline {
            std::thread::park_timeout(Duration::from_millis(10));
        }
        assert!(
            released.upgrade().is_none(),
            "the native activity worker did not stop"
        );
    }

    #[test]
    fn idle_uses_session_time_and_windows_tick_wrap_is_not_activity() {
        assert_eq!(idle_seconds(299.9), Some(false));
        assert_eq!(idle_seconds(300.), Some(true));
        for invalid in [-1., f64::NAN, f64::INFINITY] {
            assert_eq!(idle_seconds(invalid), None);
        }
        assert_eq!(tick_elapsed(u32::MAX - 19, 30), Duration::from_millis(50));
        assert_eq!(tick_elapsed(10, 300_010), IDLE_AFTER);
    }
}
