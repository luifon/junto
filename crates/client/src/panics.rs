//! The panic hook of the application: each panic is written to a report file in the data
//! directory, and the windows of that directory are told (ADR 0757). Nothing is sent over
//! the network.
//!
//! The hook runs on the thread that panics, before the panic unwinds. A panic in a callback
//! of the platform then stops in gpui and the application continues (ADR 0757); a panic
//! elsewhere ends its thread or the process as before, after the report is written.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::io::Write as _;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, Once, PoisonError};

use futures_channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};

/// The directory of the reports inside a data directory.
pub const REPORTS_DIRECTORY: &str = "panics";
/// The reports that a data directory keeps; a new report removes the oldest beyond it.
pub const KEPT_REPORTS: usize = 20;
/// The reports that one process writes. Later panics are counted and not written, so that a
/// panic that repeats does not fill the disk or hold the processor.
pub const WRITTEN_PER_PROCESS: usize = 10;

/// One panic that the hook saw, as the window of its data directory shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The report file. Absent when the report could not be written, or when this process
    /// has written [`WRITTEN_PER_PROCESS`] reports.
    pub file: Option<PathBuf>,
    pub message: String,
    pub location: String,
}

struct Listener {
    data_dir: PathBuf,
    sender: UnboundedSender<Report>,
}

static PROCESS_DATA_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());
static WRITTEN: AtomicUsize = AtomicUsize::new(0);
static SEQUENCE: AtomicUsize = AtomicUsize::new(0);

/// The latest messages of the panics of a thread that [`messages_on_this_thread`] gives.
const KEPT_MESSAGES: usize = 8;

thread_local! {
    static THREAD_DATA_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
    static PANICS_ON_THIS_THREAD: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static MESSAGES: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static HANDLED_BY_THE_CALLER: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Sets the panic hook of the process once. The hook calls the hook that was set before it,
/// which writes the panic to standard error.
pub fn install() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let handled = HANDLED_BY_THE_CALLER
                .try_with(|depth| depth.get() > 0)
                .unwrap_or(false);
            if !handled {
                let _ = PANICS_ON_THIS_THREAD.try_with(|count| count.set(count.get() + 1));
                let _ = MESSAGES.try_with(|messages| {
                    if let Ok(mut messages) = messages.try_borrow_mut() {
                        if messages.len() == KEPT_MESSAGES {
                            messages.remove(0);
                        }
                        messages.push(message_of(info));
                    }
                });
                record(info);
            }
            previous(info);
        }));
    });
}

/// The data directory of the panics of every thread that has none of its own.
pub fn record_into(data_dir: &Path) {
    install();
    *PROCESS_DATA_DIR
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(data_dir.to_owned());
}

/// The data directory of the panics of this thread, and the reports written into it from
/// now on. The application calls it on the thread of its windows.
pub fn watch(data_dir: &Path) -> UnboundedReceiver<Report> {
    install();
    THREAD_DATA_DIR.with_borrow_mut(|directory| *directory = Some(data_dir.to_owned()));
    let (sender, receiver) = unbounded();
    let mut listeners = LISTENERS.lock().unwrap_or_else(PoisonError::into_inner);
    listeners.retain(|listener| !listener.sender.is_closed());
    listeners.push(Listener {
        data_dir: data_dir.to_owned(),
        sender,
    });
    receiver
}

/// The count of the panics on this thread since it started. A window test requires it to be
/// unchanged by a test that expects no panic, because a panic in a handler no longer reaches
/// the test.
pub fn panics_on_this_thread() -> usize {
    PANICS_ON_THIS_THREAD.with(std::cell::Cell::get)
}

/// The messages of the latest panics on this thread, at most eight.
pub fn messages_on_this_thread() -> Vec<String> {
    MESSAGES.with_borrow(Clone::clone)
}

/// Runs `body`, which stops the panics of the code that it calls itself, such as a fault of
/// a library that costs one result. Such a panic is written to standard error only: no
/// report, no notice and no count.
pub fn handled_by_the_caller<R>(body: impl FnOnce() -> R) -> R {
    struct Depth;
    impl Drop for Depth {
        fn drop(&mut self) {
            HANDLED_BY_THE_CALLER.with(|depth| depth.set(depth.get() - 1));
        }
    }
    HANDLED_BY_THE_CALLER.with(|depth| depth.set(depth.get() + 1));
    let _depth = Depth;
    body()
}

fn message_of(info: &PanicHookInfo<'_>) -> String {
    info.payload_as_str()
        .unwrap_or("a panic without a message")
        .to_owned()
}

/// The directory of the reports of a data directory.
pub fn reports_directory(data_dir: &Path) -> PathBuf {
    data_dir.join(REPORTS_DIRECTORY)
}

fn record(info: &PanicHookInfo<'_>) {
    let data_dir = THREAD_DATA_DIR
        .try_with(|directory| directory.try_borrow().ok().and_then(|held| held.clone()))
        .ok()
        .flatten()
        .or_else(|| {
            PROCESS_DATA_DIR
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone()
        });
    let Some(data_dir) = data_dir else {
        return;
    };
    let message = message_of(info);
    let location = info
        .location()
        .map(|location| location.to_string())
        .unwrap_or_default();
    let file = if WRITTEN.fetch_add(1, Ordering::Relaxed) < WRITTEN_PER_PROCESS {
        write_report(&data_dir, &message, &location).ok()
    } else {
        None
    };
    let report = Report {
        file,
        message,
        location,
    };
    let listeners = LISTENERS.lock().unwrap_or_else(PoisonError::into_inner);
    for listener in listeners
        .iter()
        .filter(|listener| listener.data_dir == data_dir)
    {
        let _ = listener.sender.unbounded_send(report.clone());
    }
}

// The hook writes on the thread that panics, also the thread of the window, before the panic
// can end the process. A panic is not a part of the work of the window (ADR 0730, ADR 0757).
#[allow(clippy::disallowed_methods)]
fn write_report(data_dir: &Path, message: &str, location: &str) -> std::io::Result<PathBuf> {
    let directory = reports_directory(data_dir);
    std::fs::create_dir_all(&directory)?;
    let now = time::OffsetDateTime::now_utc();
    let stamp = format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    let name = format!(
        "panic-{stamp}-{}-{}.txt",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let path = directory.join(name);
    let mut text = String::new();
    let thread = std::thread::current();
    let _ = writeln!(text, "Junto panic report");
    let _ = writeln!(
        text,
        "version: {}",
        junto_client_core::releases::build_label()
    );
    let _ = writeln!(
        text,
        "time: {}",
        now.format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    );
    let _ = writeln!(
        text,
        "system: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(
        text,
        "thread: {} ({:?})",
        thread.name().unwrap_or("unnamed"),
        thread.id()
    );
    let _ = writeln!(text, "location: {location}");
    let _ = writeln!(text, "message: {message}");
    let _ = writeln!(text);
    let _ = writeln!(text, "backtrace:");
    let _ = writeln!(text, "{}", std::backtrace::Backtrace::force_capture());
    let mut file = std::fs::File::create_new(&path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    remove_old_reports(&directory);
    Ok(path)
}

#[allow(clippy::disallowed_methods)]
fn remove_old_reports(directory: &Path) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| name.starts_with("panic-") && name.ends_with(".txt"))
        .collect();
    names.sort();
    let excess = names.len().saturating_sub(KEPT_REPORTS);
    for name in names.iter().take(excess) {
        let _ = std::fs::remove_file(directory.join(name));
    }
}

/// Test hooks: an event handler of the application that panics when a test asks for it.
#[cfg(feature = "test-support")]
pub mod test_support {
    use std::cell::Cell;

    /// The message of a panic that a test asked for.
    pub const MESSAGE: &str = "a panic that the test asked for";

    /// The handlers that a test can make panic.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Handler {
        /// The listener of a scroll of the message list, which runs inside the scroll of the
        /// list while the list holds its state.
        FeedScroll,
        /// The listener of the end of a press of the left button anywhere in the shell.
        PressEnd,
    }

    thread_local! {
        static NEXT: Cell<Option<Handler>> = const { Cell::new(None) };
        static PRESS_ENDS: Cell<usize> = const { Cell::new(0) };
    }

    /// The next run of `handler` on this thread panics.
    pub fn panic_in_the_next(handler: Handler) {
        NEXT.with(|next| next.set(Some(handler)));
    }

    /// How often the handler of the end of a press ran on this thread.
    pub fn press_ends() -> usize {
        PRESS_ENDS.with(Cell::get)
    }

    pub(crate) fn reached(handler: Handler) {
        if handler == Handler::PressEnd {
            PRESS_ENDS.with(|count| count.set(count.get() + 1));
        }
        if NEXT.with(|next| next.get()) == Some(handler) {
            NEXT.with(|next| next.set(None));
            panic!("{MESSAGE}");
        }
    }
}
