//! The log file of the application in its data directory (ADR 0757): the `tracing` output,
//! without colour codes, in `logs/junto.log`. When the file would pass [`FILE_LIMIT`] bytes
//! it becomes `junto.log.1`, the earlier `junto.log.1` becomes `junto.log.2`, and the earlier
//! `junto.log.2` is removed, so the log holds at most three files.
//!
//! The thread that logs gives each line to a thread of the log, which writes the file: the
//! thread of the window does not wait for the file system (ADR 0730). When the thread of the
//! log is behind by [`QUEUED_LINES`] lines, a new line is dropped and counted, and the file
//! states the count.

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

enum Message {
    Line(Vec<u8>),
    Written(std::sync::mpsc::Sender<()>),
}

/// The log file. It writes nothing until [`LogFile::open`], because the application knows
/// its data directory, and holds its lock, only after the log has started.
#[derive(Clone, Default)]
pub struct LogFile {
    sender: Arc<Mutex<Option<SyncSender<Message>>>>,
    dropped: Arc<AtomicUsize>,
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
        let directory = data_dir.join(LOGS_DIRECTORY);
        std::fs::create_dir_all(&directory)?;
        let path = directory.join(LOG_FILE);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        let size = file.metadata()?.len();
        let open = Open {
            path: path.clone(),
            file,
            size,
            limit,
        };
        let (sender, receiver) = sync_channel(QUEUED_LINES);
        let dropped = self.dropped.clone();
        std::thread::Builder::new()
            .name("junto-log".to_owned())
            .spawn(move || write_lines(open, &receiver, &dropped))?;
        *self.sender.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender);
        Ok(path)
    }

    /// Waits until the thread of the log has written the lines given before.
    pub fn written(&self) {
        let (done, wait) = std::sync::mpsc::channel();
        let sender = self
            .sender
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(sender) = sender
            && sender.send(Message::Written(done)).is_ok()
        {
            let _ = wait.recv();
        }
    }

    fn give(&self, line: Vec<u8>) {
        let sender = self.sender.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(sender) = sender.as_ref() else {
            return;
        };
        if let Err(TrySendError::Full(_)) = sender.try_send(Message::Line(line)) {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }
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
                // A line that cannot be written is lost; the application continues.
                let _ = open.write(&line);
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
}

// These run on the thread of the log only.
#[allow(clippy::disallowed_methods)]
impl Open {
    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.size > 0 && self.size + bytes.len() as u64 > self.limit {
            self.rotate()?;
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
    fn nothing_is_written_before_the_file_is_opened() {
        let data_dir = tempfile::tempdir().unwrap();
        let log = LogFile::new();
        write_line(&log, "before\n");
        let path = log.open(data_dir.path()).unwrap();
        write_line(&log, "after\n");
        log.written();
        assert_eq!(path, data_dir.path().join("logs").join("junto.log"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "after\n");
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
        assert_eq!(names, 3);
    }

    #[test]
    fn a_new_start_appends_to_the_file_and_counts_its_size() {
        let data_dir = tempfile::tempdir().unwrap();
        let first = LogFile::new();
        let path = first.open_with_limit(data_dir.path(), 16).unwrap();
        write_line(&first, "first start\n");
        first.written();
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
