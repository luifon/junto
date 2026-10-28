//! The panic hook of the application: each panic is written to a report file in the data
//! directory, and the windows of that directory are told (ADR 0757). Nothing is sent over
//! the network.
//!
//! The hook runs on the thread that panics, before the panic unwinds. A panic in a callback
//! of the platform then stops in gpui and the application continues (ADR 0757); a panic
//! elsewhere ends its thread or the process as before, after the report is written.

use std::cell::RefCell;
use std::collections::HashMap;
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
/// The reports that one process writes into one data directory. A later panic is counted
/// and shown without a file, and so is a panic at a location that has a report already, so
/// that a panic that repeats does not fill the disk or hold the processor with backtraces.
pub const WRITTEN_PER_PROCESS: usize = 10;

/// One panic that the hook saw, as the window of its data directory shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// The report file. Absent when the report could not be written, when this process has
    /// written [`WRITTEN_PER_PROCESS`] reports into the data directory, or when it has a
    /// report of the same location already.
    pub file: Option<PathBuf>,
    pub message: String,
    pub location: String,
    /// The panics at this location in this process and data directory, this one included.
    pub occurrence: usize,
}

struct Listener {
    data_dir: PathBuf,
    sender: UnboundedSender<Report>,
}

static PROCESS_DATA_DIR: Mutex<Option<PathBuf>> = Mutex::new(None);
static LISTENERS: Mutex<Vec<Listener>> = Mutex::new(Vec::new());
static SEQUENCE: AtomicUsize = AtomicUsize::new(0);
static PANICS_IN_THE_PROCESS: AtomicUsize = AtomicUsize::new(0);

/// What the hook has seen in each data directory: the reports written and the panics at
/// each location.
#[derive(Default)]
struct Seen {
    written: usize,
    locations: HashMap<String, usize>,
}

static SEEN: Mutex<Option<HashMap<PathBuf, Seen>>> = Mutex::new(None);

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
                PANICS_IN_THE_PROCESS.fetch_add(1, Ordering::Relaxed);
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

/// The count of the panics on every thread of the process since the hook was set, except
/// those that [`handled_by_the_caller`] stops. A native test requires it to be unchanged.
pub fn panics_in_this_process() -> usize {
    PANICS_IN_THE_PROCESS.load(Ordering::Relaxed)
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

/// True for 1, 10, 100 and so on.
pub fn is_a_power_of_ten(mut count: usize) -> bool {
    while count >= 10 && count.is_multiple_of(10) {
        count /= 10;
    }
    count == 1
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
    let (occurrence, write) = {
        let mut seen = SEEN.lock().unwrap_or_else(PoisonError::into_inner);
        let seen = seen
            .get_or_insert_with(HashMap::new)
            .entry(data_dir.clone())
            .or_default();
        let occurrence = seen.locations.entry(location.clone()).or_insert(0);
        *occurrence += 1;
        let occurrence = *occurrence;
        let write = occurrence == 1 && seen.written < WRITTEN_PER_PROCESS;
        seen.written += usize::from(write);
        (occurrence, write)
    };
    let file = if write {
        write_report(&data_dir, &message, &location).ok()
    } else {
        None
    };
    let report = Report {
        file,
        message,
        location,
        occurrence,
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
    // Only the directory of the reports is made: the client makes the data directory itself,
    // private to the account, and a report does not make it first.
    match std::fs::create_dir(&directory) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
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
        "panic-{stamp}-{:010}-{:06}.txt",
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
    // The header is on the disk before the backtrace is resolved, which can take long or
    // fail.
    let mut file = std::fs::File::create_new(&path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    let backtrace = format!("backtrace:\n{}\n", backtrace());
    file.write_all(backtrace.as_bytes())?;
    file.sync_all()?;
    remove_old_reports(&directory);
    Ok(path)
}

/// The backtrace with the names of its functions. On Windows also the address of each frame
/// with the base address of its module: the package carries no symbols, and the offsets are
/// resolved with the symbols file of the release (ADR 0757). On macOS and Linux the program
/// holds its symbol table, and the backtrace has the names.
fn backtrace() -> String {
    #[cfg_attr(not(windows), allow(unused_mut))]
    let mut text = std::backtrace::Backtrace::force_capture().to_string();
    #[cfg(windows)]
    frames(&mut text);
    text
}

#[cfg(windows)]
fn frames(text: &mut String) {
    text.push_str("\nframes (address, base of its module, offset):\n");
    backtrace::trace(|frame| {
        let address = frame.ip() as usize;
        let base = frame.module_base_address().map_or(0, |base| base as usize);
        let _ = writeln!(
            text,
            "  {address:#x} {base:#x} +{:#x}",
            address.wrapping_sub(base)
        );
        true
    });
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

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[inline(never)]
    fn a_function_whose_name_the_report_shows() {
        panic!("a panic of the report test");
    }

    /// Also a check of a release build (`cargo test --release`): the backtrace of a report
    /// names the functions, which needs the symbols of the program (ADR 0757).
    #[test]
    fn a_report_names_the_functions_of_its_backtrace() {
        let data_dir = tempfile::tempdir().unwrap();
        let _reports = watch(data_dir.path());
        assert!(std::panic::catch_unwind(a_function_whose_name_the_report_shows).is_err());
        let reports: Vec<_> = std::fs::read_dir(reports_directory(data_dir.path()))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(reports.len(), 1, "{reports:?}");
        let text = std::fs::read_to_string(&reports[0]).unwrap();
        // With `--nocapture` the report is shown, for a check of the resolution of its frames.
        println!("{text}");
        assert!(
            text.contains("message: a panic of the report test"),
            "{text}"
        );
        assert!(
            text.contains("a_function_whose_name_the_report_shows"),
            "the backtrace names no function:\n{text}"
        );
        assert_eq!(
            text.contains("frames (address, base of its module, offset):"),
            cfg!(windows)
        );
    }

    #[test]
    fn a_report_does_not_make_the_data_directory() {
        let parent = tempfile::tempdir().unwrap();
        let data_dir = parent.path().join("not made");
        let _reports = watch(&data_dir);
        assert!(std::panic::catch_unwind(|| panic!("a panic before the data directory")).is_err());
        assert!(!data_dir.exists());
    }

    #[test]
    fn a_location_has_one_report_in_a_process() {
        let data_dir = tempfile::tempdir().unwrap();
        let mut reports = watch(data_dir.path());
        for _ in 0..3 {
            assert!(std::panic::catch_unwind(|| panic!("a panic that repeats")).is_err());
        }
        let files = std::fs::read_dir(reports_directory(data_dir.path()))
            .unwrap()
            .count();
        assert_eq!(files, 1);
        let occurrences: Vec<usize> = std::iter::from_fn(|| reports.try_recv().ok())
            .map(|report| report.occurrence)
            .collect();
        assert_eq!(occurrences, [1, 2, 3]);
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
        /// The render of the shell, inside a draw of the window.
        Render,
        /// The observer of the state of the application in the shell, which runs at each
        /// change of the state.
        StateObserver,
    }

    thread_local! {
        /// The handler that panics, and whether it panics at every run.
        static NEXT: Cell<Option<(Handler, bool)>> = const { Cell::new(None) };
        /// The tasks of the state that paused after a panic in their update, in order.
        static PAUSED: std::cell::RefCell<Vec<&'static str>> =
            const { std::cell::RefCell::new(Vec::new()) };
        /// How deep this thread is in updates of tasks that follow a source.
        static FOLLOWING: Cell<usize> = const { Cell::new(0) };
        /// The panics of the armed handler outside such an update: in an update that runs
        /// once and ends with the panic.
        static ONCE: Cell<usize> = const { Cell::new(0) };
        static PRESS_ENDS: Cell<usize> = const { Cell::new(0) };
        static NOTICE_PAINTS: Cell<(usize, usize)> = const { Cell::new((0, 0)) };
    }

    /// How often the notice of a panic was painted on this thread, and the count of panics
    /// that the latest paint stated.
    pub fn notice_paints() -> (usize, usize) {
        NOTICE_PAINTS.with(Cell::get)
    }

    pub(crate) fn notice_painted(count: usize) {
        NOTICE_PAINTS.with(|paints| paints.set((paints.get().0 + 1, count)));
    }

    /// The next run of `handler` on this thread panics.
    pub fn panic_in_the_next(handler: Handler) {
        NEXT.with(|next| next.set(Some((handler, false))));
    }

    /// Every run of `handler` on this thread panics until [`stop_panicking`]. The list of
    /// the tasks that paused starts empty.
    pub fn panic_in_every(handler: Handler) {
        PAUSED.with_borrow_mut(Vec::clear);
        ONCE.with(|once| once.set(0));
        NEXT.with(|next| next.set(Some((handler, true))));
    }

    /// The tasks of the state that follow a source, such as the panic reports or the changes
    /// of the client core, that paused after a panic in their update since
    /// [`panic_in_every`], in order (ADR 0757).
    pub fn paused_tasks() -> Vec<&'static str> {
        PAUSED.with_borrow(Clone::clone)
    }

    pub(crate) fn task_paused(task: &'static str) {
        PAUSED.with_borrow_mut(|paused| paused.push(task));
    }

    /// The panics of the armed handler since [`panic_in_every`] in updates that run once,
    /// such as the answer of a read: such a task ends with its panic and does not pause.
    pub fn panics_in_single_updates() -> usize {
        ONCE.with(Cell::get)
    }

    /// Runs `update`, an update of a task that follows a source.
    pub(crate) fn following<R>(update: impl FnOnce() -> R) -> R {
        struct Left;
        impl Drop for Left {
            fn drop(&mut self) {
                FOLLOWING.with(|depth| depth.set(depth.get().saturating_sub(1)));
            }
        }
        FOLLOWING.with(|depth| depth.set(depth.get() + 1));
        let _left = Left;
        update()
    }

    pub fn stop_panicking() {
        NEXT.with(|next| next.set(None));
    }

    /// How often the handler of the end of a press ran on this thread.
    pub fn press_ends() -> usize {
        PRESS_ENDS.with(Cell::get)
    }

    pub(crate) fn reached(handler: Handler) {
        if handler == Handler::PressEnd {
            PRESS_ENDS.with(|count| count.set(count.get() + 1));
        }
        if let Some((armed, every)) = NEXT.with(Cell::get)
            && armed == handler
        {
            if !every {
                NEXT.with(|next| next.set(None));
            }
            if FOLLOWING.with(Cell::get) == 0 {
                ONCE.with(|once| once.set(once.get() + 1));
            }
            panic!("{MESSAGE}");
        }
    }
}
