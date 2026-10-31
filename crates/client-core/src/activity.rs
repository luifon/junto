//! Inactivity in the desktop session, read away from the window thread.
use std::sync::{
    Arc, Mutex,
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
    idle_since: Mutex<Option<Instant>>,
    readers: Mutex<Vec<tokio::sync::oneshot::Sender<Option<Duration>>>>,
}
impl Shared {
    fn set(&self, idle: Option<bool>) {
        let mut since = self
            .idle_since
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match idle {
            Some(true) => {
                since.get_or_insert_with(|| Instant::now() - IDLE_AFTER);
            }
            _ => *since = None,
        }
        drop(since);
        self.state.store(
            match idle {
                Some(false) => 1,
                Some(true) => 2,
                None => 3,
            },
            Ordering::SeqCst,
        );
    }
    fn set_duration(&self, idle: Option<Duration>) {
        self.set(idle.map(|duration| duration >= IDLE_AFTER));
        *self
            .idle_since
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            idle.and_then(|duration| Instant::now().checked_sub(duration));
    }
    fn idle_duration(&self) -> Option<Duration> {
        if !matches!(self.state.load(Ordering::SeqCst), 1 | 2) {
            return None;
        }
        Some(
            self.idle_since
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .map_or(Duration::ZERO, |since| since.elapsed()),
        )
    }
    fn take_readers(&self) -> Vec<tokio::sync::oneshot::Sender<Option<Duration>>> {
        std::mem::take(
            &mut *self
                .readers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
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
            idle_since: Mutex::new(None),
            readers: Mutex::new(Vec::new()),
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
    pub fn idle_duration(&self) -> Option<Duration> {
        self.shared.idle_duration()
    }
    /// Requests a native observation on the activity worker before resolving.
    pub fn read_idle_duration(&self) -> impl Future<Output = Option<Duration>> + use<> {
        let (send, receive) = tokio::sync::oneshot::channel();
        if let Some(worker) = &self.worker {
            self.shared
                .readers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(send);
            worker.thread().unpark();
        }
        async move { receive.await.ok().flatten() }
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
    if std::env::var_os("WAYLAND_DISPLAY").is_some() && (wayland::run(&shared) || shared.stopped())
    {
        return;
    }
    run_polling(&shared, || {
        #[cfg(target_os = "linux")]
        {
            reader.read()
        }
        #[cfg(target_os = "macos")]
        {
            macos::read()
        }
        #[cfg(windows)]
        {
            windows::read()
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            None
        }
    });
}

fn run_polling(shared: &Shared, mut read: impl FnMut() -> Option<Duration>) {
    while !shared.stopped() {
        let readers = shared.take_readers();
        let idle = read();
        shared.set_duration(idle);
        for reader in readers {
            let _ = reader.send(idle);
        }
        std::thread::park_timeout(READ_EVERY);
    }
    for reader in shared.take_readers() {
        let _ = reader.send(None);
    }
}

#[cfg(any(target_os = "macos", test))]
fn idle_seconds(seconds: f64) -> Option<Duration> {
    Duration::try_from_secs_f64(seconds).ok()
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
    #[tokio::test]
    async fn the_native_reader_starts_and_releases_its_worker() {
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
            assert!(activity.idle_duration().is_some());
            assert!(
                tokio::time::timeout(Duration::from_secs(2), activity.read_idle_duration())
                    .await
                    .unwrap()
                    .is_some()
            );
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
    fn duration_survives_the_presence_threshold_and_unknown_activity_never_qualifies() {
        let activity = SystemActivity {
            shared: Arc::new(Shared {
                state: AtomicU8::new(0),
                stop: AtomicBool::new(false),
                idle_since: Mutex::new(None),
                readers: Mutex::new(Vec::new()),
            }),
            worker: None,
        };
        activity.shared.set_duration(Some(Duration::from_secs(900)));
        assert_eq!(activity.state(), ActivityState::Idle);
        assert!(activity.idle_duration().unwrap() >= Duration::from_secs(900));
        activity.shared.set_duration(Some(Duration::from_secs(12)));
        assert_eq!(activity.state(), ActivityState::Active);
        assert!(activity.idle_duration().unwrap() < IDLE_AFTER);
        activity.shared.set_duration(None);
        assert_eq!(activity.state(), ActivityState::Unavailable);
        assert_eq!(activity.idle_duration(), None);
    }

    #[tokio::test]
    async fn input_between_the_cached_sample_and_an_idle_report_requires_a_fresh_read() {
        use std::sync::atomic::AtomicU64;
        let shared = Arc::new(Shared {
            state: AtomicU8::new(0),
            stop: AtomicBool::new(false),
            idle_since: Mutex::new(None),
            readers: Mutex::new(Vec::new()),
        });
        let native_idle = Arc::new(AtomicU64::new(900));
        let (sampled, first_sample) = tokio::sync::oneshot::channel();
        let work = shared.clone();
        let native = native_idle.clone();
        let worker = std::thread::spawn(move || {
            let mut sampled = Some(sampled);
            run_polling(&work, || {
                let idle = Duration::from_secs(native.load(Ordering::SeqCst));
                if let Some(sampled) = sampled.take() {
                    work.set_duration(Some(idle));
                    let _ = sampled.send(());
                }
                Some(idle)
            });
        });
        let activity = SystemActivity {
            shared,
            worker: Some(worker),
        };
        tokio::time::timeout(Duration::from_secs(2), first_sample)
            .await
            .unwrap()
            .unwrap();
        assert!(activity.idle_duration().unwrap() >= Duration::from_secs(900));
        native_idle.store(0, Ordering::SeqCst);
        let fresh = tokio::time::timeout(Duration::from_secs(2), activity.read_idle_duration())
            .await
            .unwrap();
        assert_eq!(
            fresh,
            Some(Duration::ZERO),
            "recent input must prevent an idle report"
        );
        native_idle.store(900, Ordering::SeqCst);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), activity.read_idle_duration())
                .await
                .unwrap(),
            Some(Duration::from_secs(900))
        );
    }

    #[test]
    fn idle_uses_session_time_and_windows_tick_wrap_is_not_activity() {
        assert_eq!(idle_seconds(299.9).unwrap().as_secs(), 299);
        assert_eq!(idle_seconds(300.), Some(IDLE_AFTER));
        for invalid in [-1., f64::NAN, f64::INFINITY] {
            assert_eq!(idle_seconds(invalid), None);
        }
        assert_eq!(tick_elapsed(u32::MAX - 19, 30), Duration::from_millis(50));
        assert_eq!(tick_elapsed(10, 300_010), IDLE_AFTER);
    }
}
