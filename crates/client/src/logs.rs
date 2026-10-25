//! The log file of the application in its data directory (ADR 0757): the `tracing` output,
//! without colour codes, in `logs/junto.log`. When the file would pass [`FILE_LIMIT`] bytes
//! it becomes `junto.log.1`, the earlier `junto.log.1` becomes `junto.log.2`, and the earlier
//! `junto.log.2` is removed, so the log holds at most three files.
//!
//! The thread that logs gives each line to a thread of the log, which writes the file: the
//! thread of the window does not wait for the file system (ADR 0730). When the thread of the
//! log is behind by [`QUEUED_LINES`] lines, a new line is dropped and counted, and the file
//! states the count.
//!
//! One process writes the log of a data directory: it holds the lock of `logs/lock` while it
//! runs. Another instance on the same directory writes no log file. The lines given before
//! the file is opened, at most [`EARLY_LINES`], are written first when it opens.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex, PoisonError};

use tracing_subscriber::fmt::MakeWriter;

/// The directory of the log inside a data directory.
pub const LOGS_DIRECTORY: &str = "logs";
/// The name of the file that the application writes.
pub const LOG_FILE: &str = "junto.log";
/// The size at which the file is replaced by a new one.
pub const FILE_LIMIT: u64 = 4 * 1024 * 1024;
/// The earlier files that are kept.
pub const KEPT_FILES: u32 = 2;
/// The lines that wait for the thread of the log at most.
pub const QUEUED_LINES: usize = 4096;
/// The lines given before the file is opened that are kept for it.
pub const EARLY_LINES: usize = 256;
/// The lock of the log of a data directory, inside its directory of the log.
pub const LOCK_FILE: &str = "lock";

enum Message {
    Line(Vec<u8>),
    Written(std::sync::mpsc::Sender<()>),
}

/// The log file. It writes nothing until [`LogFile::open`], because the application knows
/// its data directory, and holds its lock, only after the log has started.
#[derive(Clone, Default)]
pub struct LogFile {
    sender: Arc<Mutex<Writer>>,
    dropped: Arc<AtomicUsize>,
    thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
}

enum Writer {
    /// Before [`LogFile::open`]: the first lines.
    Early(Vec<Vec<u8>>),
    Open(SyncSender<Message>),
    /// No file: another instance holds the log, or the file could not be opened.
    None,
}

impl Default for Writer {
    fn default() -> Self {
        Self::Early(Vec::new())
    }
}

impl LogFile {
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes from now on into `logs/junto.log` of the data directory. The file is opened
    /// here, at the start of the application; the thread of the log writes it.
    pub fn open(&self, data_dir: &Path) -> io::Result<PathBuf> {
        self.open_with_limit(data_dir, FILE_LIMIT)
    }

    // The start of the application opens the file, before the window (ADR 0730).
    #[allow(clippy::disallowed_methods)]
    fn open_with_limit(&self, data_dir: &Path, limit: u64) -> io::Result<PathBuf> {
        let opened = open_file(data_dir, limit);
        let mut writer = self.sender.lock().unwrap_or_else(PoisonError::into_inner);
        let (open, lock) = match opened {
            Ok(opened) => opened,
            Err(error) => {
                *writer = Writer::None;
                return Err(error);
            }
        };
        let path = open.path.clone();
        let (sender, receiver) = sync_channel(QUEUED_LINES);
        if let Writer::Early(lines) = std::mem::replace(&mut *writer, Writer::None) {
            for line in lines {
                let _ = sender.try_send(Message::Line(line));
            }
        }
        let dropped = self.dropped.clone();
        let thread = std::thread::Builder::new()
            .name("junto-log".to_owned())
            .spawn(move || {
                // The lock lasts as long as the thread of the log, which is the life of the
                // process for the log of the application.
                let _lock = lock;
                write_lines(open, &receiver, &dropped);
            })?;
        *writer = Writer::Open(sender);
        *self.thread.lock().unwrap_or_else(PoisonError::into_inner) = Some(thread);
        Ok(path)
    }

    /// Waits until the thread of the log has written the lines given before.
    pub fn written(&self) {
        let (done, wait) = std::sync::mpsc::channel();
        let sender = match &*self.sender.lock().unwrap_or_else(PoisonError::into_inner) {
            Writer::Open(sender) => Some(sender.clone()),
            _ => None,
        };
        if let Some(sender) = sender
            && sender.send(Message::Written(done)).is_ok()
        {
            let _ = wait.recv();
        }
    }

    /// Ends the thread of the log after the lines given before, and releases the lock.
    #[cfg(test)]
    fn close(&self) {
        *self.sender.lock().unwrap_or_else(PoisonError::into_inner) = Writer::None;
        let thread = self
            .thread
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(thread) = thread {
            let _ = thread.join();
        }
    }

    fn give(&self, line: Vec<u8>) {
        let mut writer = self.sender.lock().unwrap_or_else(PoisonError::into_inner);
        match &mut *writer {
            Writer::Early(lines) => {
                if lines.len() < EARLY_LINES {
                    lines.push(line);
                }
            }
            Writer::Open(sender) => {
                if let Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) =
                    sender.try_send(Message::Line(line))
                {
                    self.dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            Writer::None => {}
        }
    }
}

/// Takes the lock of the log of the data directory and opens the file.
// The start of the application opens the file, before the window (ADR 0730).
#[allow(clippy::disallowed_methods)]
fn open_file(data_dir: &Path, limit: u64) -> io::Result<(Open, File)> {
    let directory = data_dir.join(LOGS_DIRECTORY);
    std::fs::create_dir_all(&directory)?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(directory.join(LOCK_FILE))?;
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(io::Error::other(
                "another instance of Junto writes the log of this data directory",
            ));
        }
        Err(std::fs::TryLockError::Error(error)) => return Err(error),
    }
    let path = directory.join(LOG_FILE);
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let size = file.metadata()?.len();
    Ok((
        Open {
            path,
            file,
            size,
            limit,
            rotate_at: limit,
        },
        lock,
    ))
}

fn write_lines(mut open: Open, receiver: &Receiver<Message>, dropped: &AtomicUsize) {
    while let Ok(message) = receiver.recv() {
        match message {
            Message::Line(line) => {
                let lost = dropped.swap(0, Ordering::Relaxed);
                if lost > 0 {
                    let note = format!("{lost} lines of the log were dropped here\n");
                    let _ = open.write(note.as_bytes());
                }
                // A line that cannot be written is lost and counted; the application
                // continues.
                if open.write(&line).is_err() {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            }
            Message::Written(done) => {
                let _ = done.send(());
            }
        }
    }
}

struct Open {
    path: PathBuf,
    file: File,
    size: u64,
    limit: u64,
    /// The size at which the next rotation is tried: one limit after the last failed one.
    rotate_at: u64,
}

// These run on the thread of the log only.
#[allow(clippy::disallowed_methods)]
impl Open {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.size > 0 && self.size + bytes.len() as u64 > self.rotate_at {
            // A rotation that fails, for example while another program holds an earlier file
            // open, is tried again after a further limit; the line goes into the current file.
            self.rotate_at = match self.rotate() {
                Ok(()) => self.limit,
                Err(_) => self.size + self.limit,
            };
        }
        self.file.write_all(bytes)?;
        self.size += bytes.len() as u64;
        Ok(())
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        for number in (1..KEPT_FILES).rev() {
            let from = numbered(&self.path, number);
            if from.exists() {
                std::fs::rename(&from, numbered(&self.path, number + 1))?;
            }
        }
        std::fs::rename(&self.path, numbered(&self.path, 1))?;
        self.file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        self.size = 0;
        Ok(())
    }
}

fn numbered(path: &Path, number: u32) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(format!(".{number}"));
    PathBuf::from(name)
}

/// One event of the log, given whole to the thread of the log when it ends.
pub struct Line {
    log: LogFile,
    buffer: Vec<u8>,
}

impl Write for Line {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        if !self.buffer.is_empty() {
            self.log.give(std::mem::take(&mut self.buffer));
        }
    }
}

impl<'a> MakeWriter<'a> for LogFile {
    type Writer = Line;

    fn make_writer(&'a self) -> Self::Writer {
        Line {
            log: self.clone(),
            buffer: Vec::new(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn write_line(log: &LogFile, text: &str) {
        let mut line = log.make_writer();
        line.write_all(text.as_bytes()).unwrap();
    }

    #[test]
    fn the_lines_before_the_file_opens_are_written_first_with_a_bound() {
        let data_dir = tempfile::tempdir().unwrap();
        let log = LogFile::new();
        for number in 0..EARLY_LINES + 10 {
            write_line(&log, &format!("before {number}\n"));
        }
        let path = log.open(data_dir.path()).unwrap();
        write_line(&log, "after\n");
        log.written();
        assert_eq!(path, data_dir.path().join("logs").join("junto.log"));
        let text = std::fs::read_to_string(path).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), EARLY_LINES + 1);
        assert_eq!(lines[0], "before 0");
        assert_eq!(
            lines[EARLY_LINES - 1],
            format!("before {}", EARLY_LINES - 1)
        );
        assert_eq!(lines[EARLY_LINES], "after");
    }

    #[test]
    fn a_second_log_of_the_same_data_directory_writes_no_file() {
        let data_dir = tempfile::tempdir().unwrap();
        let first = LogFile::new();
        let path = first.open_with_limit(data_dir.path(), 16).unwrap();
        let second = LogFile::new();
        assert!(second.open_with_limit(data_dir.path(), 16).is_err());
        for number in 0..20 {
            write_line(&first, &format!("first {number:02}\n"));
            write_line(&second, &format!("second {number:02}\n"));
        }
        first.written();
        let directory = path.parent().unwrap();
        let mut names: Vec<String> = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["junto.log", "junto.log.1", "junto.log.2", "lock"]);
        for name in &names[..3] {
            let text = std::fs::read_to_string(directory.join(name)).unwrap();
            assert!(!text.contains("second"), "{name}: {text}");
            assert!(text.len() <= 16, "{name}: {text}");
        }
    }

    #[test]
    fn the_log_keeps_at_most_three_files_of_the_limit() {
        let data_dir = tempfile::tempdir().unwrap();
        let log = LogFile::new();
        let path = log.open_with_limit(data_dir.path(), 10).unwrap();
        for number in 0..7 {
            write_line(&log, &format!("line {number}\n"));
        }
        log.written();
        let read = |path: &Path| std::fs::read_to_string(path).unwrap();
        assert_eq!(read(&path), "line 6\n");
        assert_eq!(read(&numbered(&path, 1)), "line 5\n");
        assert_eq!(read(&numbered(&path, 2)), "line 4\n");
        assert!(!numbered(&path, 3).exists());
        let names = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(names, 4, "three files of the log and the lock");
    }

    #[test]
    fn a_new_start_appends_to_the_file_and_counts_its_size() {
        let data_dir = tempfile::tempdir().unwrap();
        let first = LogFile::new();
        let path = first.open_with_limit(data_dir.path(), 16).unwrap();
        write_line(&first, "first start\n");
        first.written();
        first.close();
        let second = LogFile::new();
        second.open_with_limit(data_dir.path(), 16).unwrap();
        write_line(&second, "second\n");
        second.written();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second\n");
        assert_eq!(
            std::fs::read_to_string(numbered(&path, 1)).unwrap(),
            "first start\n"
        );
    }
}
