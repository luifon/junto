//! Files of the data directory (ADR 0710, ADR 0711).
//!
//! Every file is reached through a handle of its directory. A directory is opened once,
//! without following a link, and the opened object is checked: its kind, its owner and who
//! can access it. A file is then opened relative to that handle, again without following a
//! link, and the opened file is checked, never its path.

use std::cell::RefCell;
use std::fs::File;
use std::io::{self, Read, Write};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::DeserializeOwned;

#[cfg(unix)]
#[path = "storage/unix.rs"]
mod platform;
#[cfg(windows)]
#[path = "storage/windows.rs"]
mod platform;
#[cfg(windows)]
mod windows_acl;

#[cfg(not(any(unix, windows)))]
compile_error!("the client stores its files on macOS, Windows and Linux only (ADR 0705)");

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("cannot {action} {path}: {source}")]
    Io {
        action: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{path} is not a valid {kind} file: {detail}")]
    Corrupted {
        path: PathBuf,
        kind: &'static str,
        detail: String,
    },
    #[error("{path} has format version {found}, and this client reads versions up to {supported}")]
    NewerFormat {
        path: PathBuf,
        found: u32,
        supported: u32,
    },
    #[error("{path} can be accessed by other users")]
    NotPrivate { path: PathBuf },
    #[error("{path} belongs to another user")]
    NotOwned { path: PathBuf },
    #[error("{path} is not a regular file or directory of the expected kind")]
    UnexpectedKind { path: PathBuf },
    #[error("{path} is larger than {limit} bytes")]
    TooLarge { path: PathBuf, limit: u64 },
}

impl StorageError {
    pub(crate) fn io(action: &'static str, path: &Path, source: io::Error) -> Self {
        Self::Io {
            action,
            path: path.to_owned(),
            source,
        }
    }
}

/// The rules that decide whether an opened file or directory is accepted. They take what
/// the system reports about the opened object and have no access to the file system.
pub(crate) mod policy {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Refusal {
        Kind,
        Owner,
        Access,
    }

    /// Unix: the object has the expected kind and belongs to the user of the process. A
    /// private object has no permission bit for the group or for others.
    #[cfg_attr(not(unix), allow(dead_code))]
    pub(crate) fn unix(
        kind_fits: bool,
        owner: u32,
        mode: u32,
        user: u32,
        private: bool,
    ) -> Result<(), Refusal> {
        if !kind_fits {
            Err(Refusal::Kind)
        } else if owner != user {
            Err(Refusal::Owner)
        } else if private && mode & 0o077 != 0 {
            Err(Refusal::Access)
        } else {
            Ok(())
        }
    }

    /// Who owns an object on Windows. The owner can always change the access list.
    #[cfg_attr(not(windows), allow(dead_code))]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) enum Owner {
        User,
        /// The administrators group or the system account. Both can take any file anyway.
        Administration,
        Other,
    }

    /// One entry of an access list on Windows.
    #[cfg_attr(not(windows), allow(dead_code))]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Entry {
        /// False for every kind of entry other than "access allowed".
        pub(crate) allows: bool,
        pub(crate) for_user: bool,
    }

    /// Windows: the object has the expected kind and its owner is the user or the
    /// administration. A private object has an access list in which every entry allows
    /// access to the user of the process; an object without a list is open to everybody.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn windows(
        kind_fits: bool,
        owner: Owner,
        list: Option<&[Entry]>,
        private: bool,
    ) -> Result<(), Refusal> {
        if !kind_fits {
            return Err(Refusal::Kind);
        }
        if owner == Owner::Other {
            return Err(Refusal::Owner);
        }
        if !private {
            return Ok(());
        }
        match list {
            Some(entries) if entries.iter().all(|entry| entry.allows && entry.for_user) => Ok(()),
            _ => Err(Refusal::Access),
        }
    }
}

pub(crate) enum Fault {
    Io(io::Error),
    Refused(policy::Refusal),
}

impl From<io::Error> for Fault {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

fn storage_error(fault: Fault, action: &'static str, path: &Path) -> StorageError {
    let path = path.to_owned();
    match fault {
        Fault::Io(source) => StorageError::Io {
            action,
            path,
            source,
        },
        Fault::Refused(policy::Refusal::Kind) => StorageError::UnexpectedKind { path },
        Fault::Refused(policy::Refusal::Owner) => StorageError::NotOwned { path },
        Fault::Refused(policy::Refusal::Access) => StorageError::NotPrivate { path },
    }
}

const TEMPORARY_SUFFIX: &str = ".tmp";
const LOCK_NAME: &str = "lock";

/// An opened directory that belongs to the user and that nobody else can access.
pub(crate) struct Dir {
    handle: platform::Handle,
    path: PathBuf,
    /// What the system uses to tell this directory from every other one, whatever path
    /// led to it.
    identity: Identity,
}

pub(crate) type Identity = (u64, u64);

impl std::fmt::Debug for Dir {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "Dir({})", self.path.display())
    }
}

impl Dir {
    /// Opens the directory at `path` and creates it first when it does not exist. The
    /// parents are created with the defaults of the system. A link at `path`, a directory
    /// of another user and a directory that others can access are refused.
    pub(crate) fn open(path: &Path) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| StorageError::io("create", parent, error))?;
        }
        let handle = platform::Handle::open_root(path)
            .map_err(|fault| storage_error(fault, "open", path))?;
        Self::opened(handle, path.to_owned())
    }

    /// Opens a directory inside this one under the same rules.
    pub(crate) fn subdirectory(&self, name: &str) -> Result<Self, StorageError> {
        self.entry(name, "open")?;
        let path = self.path.join(name);
        let handle = self
            .handle
            .open_directory(name)
            .map_err(|fault| storage_error(fault, "open", &path))?;
        Self::opened(handle, path)
    }

    fn opened(handle: platform::Handle, path: PathBuf) -> Result<Self, StorageError> {
        let identity = handle
            .identity()
            .map_err(|error| StorageError::io("inspect", &path, error))?;
        Ok(Self {
            handle,
            path,
            identity,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Checks the opened directory again: its owner and its access can change while the
    /// handle is held.
    pub(crate) fn check(&self) -> Result<(), StorageError> {
        self.handle
            .check()
            .map_err(|fault| storage_error(fault, "inspect", &self.path))
    }

    /// A name is one entry of this directory, never a path.
    fn entry(&self, name: &str, action: &'static str) -> Result<(), StorageError> {
        let plain =
            !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0']);
        if plain {
            Ok(())
        } else {
            Err(StorageError::io(
                action,
                &self.path.join(name),
                io::Error::new(io::ErrorKind::InvalidInput, "not a file name"),
            ))
        }
    }

    fn open_file(&self, name: &str, private: bool) -> Result<Option<File>, StorageError> {
        self.entry(name, "open")?;
        self.handle
            .open_file(name, private)
            .map_err(|fault| storage_error(fault, "open", &self.path.join(name)))
    }

    /// Reads a regular file of the user of at most `limit` bytes. `None` when the file
    /// does not exist.
    pub(crate) fn read(&self, name: &str, limit: u64) -> Result<Option<Vec<u8>>, StorageError> {
        let Some(file) = self.open_file(name, false)? else {
            return Ok(None);
        };
        let path = self.path.join(name);
        let mut bytes = Vec::new();
        file.take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| StorageError::io("read", &path, error))?;
        if bytes.len() as u64 > limit {
            return Err(StorageError::TooLarge { path, limit });
        }
        Ok(Some(bytes))
    }

    /// Reads a file that only the user can access into `buffer` and returns the number of
    /// bytes. The content passes through no other memory. `None` when the file does not
    /// exist; a file that holds more than the buffer is an error.
    pub(crate) fn read_private(
        &self,
        name: &str,
        buffer: &mut [u8],
    ) -> Result<Option<usize>, StorageError> {
        let Some(mut file) = self.open_file(name, true)? else {
            return Ok(None);
        };
        let path = self.path.join(name);
        let mut filled = 0;
        while filled < buffer.len() {
            match file.read(&mut buffer[filled..]) {
                Ok(0) => return Ok(Some(filled)),
                Ok(count) => filled += count,
                Err(error) => return Err(StorageError::io("read", &path, error)),
            }
        }
        let mut extra = [0u8; 1];
        match file.read(&mut extra) {
            Ok(0) => Ok(Some(filled)),
            Ok(_) => Err(StorageError::TooLarge {
                limit: buffer.len() as u64,
                path,
            }),
            Err(error) => Err(StorageError::io("read", &path, error)),
        }
    }

    /// Writes `bytes` to a new file that only the user can access and flushes it.
    fn write_temporary(&self, name: &str, bytes: &[u8]) -> io::Result<String> {
        let suffix = getrandom::u64().map_err(|_| io::Error::other("no random bytes"))?;
        let temporary = format!(".{name}.{suffix:016x}{TEMPORARY_SUFFIX}");
        let written = self.handle.create_file(&temporary).and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        });
        if let Err(error) = written {
            let _ = self.handle.remove(&temporary);
            return Err(error);
        }
        Ok(temporary)
    }

    /// Replaces a file in one step. The content is written to a new file that only the
    /// user can access, flushed to the disk, and renamed over the file; the directory is
    /// flushed too. A reader sees the old content or the new content, never a part of
    /// either.
    pub(crate) fn write(&self, name: &str, bytes: &[u8]) -> Result<(), StorageError> {
        self.entry(name, "write")?;
        let path = self.path.join(name);
        let replaced = self.write_temporary(name, bytes).and_then(|temporary| {
            self.handle
                .rename(&temporary, name)
                .and_then(|()| self.handle.sync())
                .inspect_err(|_| {
                    let _ = self.handle.remove(&temporary);
                })
        });
        replaced.map_err(|error| StorageError::io("write", &path, error))
    }

    /// Creates a file in one step and never replaces one. False when the name is taken.
    pub(crate) fn create(&self, name: &str, bytes: &[u8]) -> Result<bool, StorageError> {
        self.entry(name, "create")?;
        let path = self.path.join(name);
        let created = self.write_temporary(name, bytes).and_then(|temporary| {
            let named = self.handle.name_new(&temporary, name);
            let _ = self.handle.remove(&temporary);
            self.handle.sync()?;
            named
        });
        created.map_err(|error| StorageError::io("create", &path, error))
    }

    /// Deletes a file. False when it did not exist.
    pub(crate) fn remove(&self, name: &str) -> Result<bool, StorageError> {
        self.entry(name, "delete")?;
        self.handle
            .remove(name)
            .and_then(|removed| self.handle.sync().map(|()| removed))
            .map_err(|error| StorageError::io("delete", &self.path.join(name), error))
    }

    pub(crate) fn rename(&self, from: &str, to: &str) -> Result<(), StorageError> {
        self.entry(from, "rename")?;
        self.entry(to, "rename")?;
        self.handle
            .rename(from, to)
            .and_then(|()| self.handle.sync())
            .map_err(|error| StorageError::io("rename", &self.path.join(from), error))
    }

    /// Removes the temporary files that a write left behind when the process stopped
    /// before its rename. The caller holds the lock of the data directory.
    pub(crate) fn remove_leftovers(&self) -> Result<(), StorageError> {
        let names = self
            .handle
            .names()
            .map_err(|error| StorageError::io("list", &self.path, error))?;
        for name in names {
            let name = name.to_string_lossy();
            if name.starts_with('.') && name.ends_with(TEMPORARY_SUFFIX) {
                let _ = self.handle.remove(&name);
            }
        }
        Ok(())
    }

    /// Takes the exclusive lock of this directory. Every change of a stored file happens
    /// while it is held, so two processes that use one data directory do not lose each
    /// other's changes. A thread that holds the lock of a directory can take it again,
    /// through this handle or through another handle of the same directory: the lock is
    /// known by the identity of the directory, not by the path that was used to open it.
    pub(crate) fn lock(&self) -> Result<DirectoryLock, StorageError> {
        let path = self.path.join(LOCK_NAME);
        let held = HELD.with_borrow(|held| held.contains(&self.identity));
        let file = if held {
            None
        } else {
            let file = self
                .handle
                .open_lock(LOCK_NAME)
                .and_then(|file| file.lock().map(|()| file))
                .map_err(|error| StorageError::io("lock", &path, error))?;
            Some(file)
        };
        HELD.with_borrow_mut(|held| held.push(self.identity));
        Ok(DirectoryLock {
            directory: self.identity,
            _file: file,
            _thread: PhantomData,
        })
    }
}

thread_local! {
    static HELD: RefCell<Vec<Identity>> = const { RefCell::new(Vec::new()) };
}

/// The lock of one data directory. It stays with the thread that took it.
pub(crate) struct DirectoryLock {
    directory: Identity,
    _file: Option<File>,
    _thread: PhantomData<*const ()>,
}

impl Drop for DirectoryLock {
    fn drop(&mut self) {
        HELD.with_borrow_mut(|held| {
            if let Some(index) = held.iter().rposition(|held| *held == self.directory) {
                held.remove(index);
            }
        });
    }
}

/// One step that turns the content of format version `n` into version `n + 1`.
pub(crate) type Migration = fn(serde_json::Value) -> Result<serde_json::Value, String>;

/// A JSON file whose top-level object has the field `format_version`.
pub(crate) struct Versioned {
    pub(crate) name: &'static str,
    pub(crate) kind: &'static str,
    /// The version that this client writes.
    pub(crate) version: u32,
    /// `migrations[n - 1]` reads version `n`. Its length is `version - 1`.
    pub(crate) migrations: &'static [Migration],
    pub(crate) limit: u64,
}

#[derive(Deserialize)]
struct VersionProbe {
    format_version: u32,
}

impl Versioned {
    pub(crate) fn corrupted(&self, directory: &Dir, detail: String) -> StorageError {
        StorageError::Corrupted {
            path: directory.path().join(self.name),
            kind: self.kind,
            detail,
        }
    }

    /// `None` when the file does not exist. A file of a newer version is refused and never
    /// overwritten; a file of an older version is migrated in memory.
    pub(crate) fn load<T: DeserializeOwned>(
        &self,
        directory: &Dir,
    ) -> Result<Option<T>, StorageError> {
        let Some(bytes) = directory.read(self.name, self.limit)? else {
            return Ok(None);
        };
        let corrupted = |detail: String| self.corrupted(directory, detail);
        let probe: VersionProbe =
            serde_json::from_slice(&bytes).map_err(|error| corrupted(shape_of(&error)))?;
        if probe.format_version > self.version {
            return Err(StorageError::NewerFormat {
                path: directory.path().join(self.name),
                found: probe.format_version,
                supported: self.version,
            });
        }
        if probe.format_version == 0 {
            return Err(corrupted("format version 0 does not exist".to_owned()));
        }
        let mut value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| corrupted(shape_of(&error)))?;
        for from in probe.format_version..self.version {
            let migration = usize::try_from(from - 1)
                .ok()
                .and_then(|index| self.migrations.get(index))
                .ok_or_else(|| corrupted(format!("no migration from format version {from}")))?;
            value = migration(value).map_err(corrupted)?;
            if let Some(object) = value.as_object_mut() {
                object.insert("format_version".to_owned(), (from + 1).into());
            }
        }
        serde_json::from_value(value)
            .map(Some)
            .map_err(|error| corrupted(shape_of(&error)))
    }

    pub(crate) fn store<T: serde::Serialize>(
        &self,
        directory: &Dir,
        content: &T,
    ) -> Result<(), StorageError> {
        let mut bytes = serde_json::to_vec_pretty(content).map_err(|error| {
            StorageError::io(
                "encode",
                &directory.path().join(self.name),
                io::Error::other(error.to_string()),
            )
        })?;
        bytes.push(b'\n');
        directory.write(self.name, &bytes)
    }
}

/// The position and the category of a JSON error, without the text that serde quotes from
/// the file.
fn shape_of(error: &serde_json::Error) -> String {
    let category = match error.classify() {
        serde_json::error::Category::Io => "read error",
        serde_json::error::Category::Syntax => "not JSON",
        serde_json::error::Category::Data => "unexpected content",
        serde_json::error::Category::Eof => "ends early",
    };
    format!(
        "{category} at line {} column {}",
        error.line(),
        error.column()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;
    use std::ffi::OsString;
    use std::fs;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Sample {
        format_version: u32,
        label: String,
    }

    const SAMPLE: Versioned = Versioned {
        name: "sample.json",
        kind: "sample",
        version: 2,
        migrations: &[rename_title_to_label],
        limit: 4096,
    };

    fn rename_title_to_label(mut value: serde_json::Value) -> Result<serde_json::Value, String> {
        let object = value.as_object_mut().ok_or("not an object")?;
        let title = object.remove("title").ok_or("no title")?;
        object.insert("label".to_owned(), title);
        Ok(value)
    }

    fn private() -> (tempfile::TempDir, Dir) {
        let directory = tempfile::tempdir().unwrap();
        let private = Dir::open(&directory.path().join("private")).unwrap();
        (directory, private)
    }

    fn names(directory: &Dir) -> Vec<OsString> {
        let mut names: Vec<_> = fs::read_dir(directory.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn atomic_write_replaces_the_content_and_leaves_no_temporary_file() {
        let (_directory, private) = private();
        private.write("data.json", b"first").unwrap();
        private.write("data.json", b"second").unwrap();
        assert_eq!(
            fs::read(private.path().join("data.json")).unwrap(),
            b"second"
        );
        assert_eq!(names(&private), ["data.json"]);
    }

    #[test]
    fn a_failed_write_keeps_the_old_content_and_removes_its_temporary_file() {
        let (_directory, private) = private();
        private.write("data.json", b"kept").unwrap();
        assert!(matches!(
            private.write("missing/data.json", b"lost"),
            Err(StorageError::Io { .. })
        ));
        fs::create_dir(private.path().join("target")).unwrap();
        fs::write(private.path().join("target").join("inside"), b"x").unwrap();
        assert!(private.write("target", b"lost").is_err());
        assert_eq!(fs::read(private.path().join("data.json")).unwrap(), b"kept");
        assert_eq!(names(&private), ["data.json", "target"]);
    }

    #[test]
    fn a_reader_never_sees_a_partial_file_while_a_writer_replaces_it() {
        let (_directory, private) = private();
        let small = vec![b'a'; 64];
        let large = vec![b'b'; 256 * 1024];
        private.write("data.json", &small).unwrap();
        let path = private.path().join("data.json");
        let writer = {
            let (small, large) = (small.clone(), large.clone());
            std::thread::spawn(move || {
                for round in 0..40 {
                    let content = if round % 2 == 0 { &large } else { &small };
                    private.write("data.json", content).unwrap();
                }
            })
        };
        while !writer.is_finished() {
            let seen = fs::read(&path).unwrap();
            assert!(seen == small || seen == large, "{} bytes", seen.len());
        }
        writer.join().unwrap();
    }

    #[test]
    fn a_file_is_created_once_and_never_replaced() {
        let (_directory, private) = private();
        assert!(private.create("device.key", b"first").unwrap());
        assert!(!private.create("device.key", b"second").unwrap());
        assert_eq!(
            fs::read(private.path().join("device.key")).unwrap(),
            b"first"
        );
        assert_eq!(names(&private), ["device.key"]);
        assert!(private.remove("device.key").unwrap());
        assert!(!private.remove("device.key").unwrap());
        assert!(names(&private).is_empty());
    }

    /// While the creation gave the file a second name, the write failed with access denied in
    /// about two rounds of a thousand (ADR 0711), so the test repeats the sequence.
    #[cfg(windows)]
    #[test]
    fn a_created_file_is_replaced_at_once() {
        let (_directory, private) = private();
        for round in 0..3000 {
            let name = format!("{round}.key");
            assert!(private.create(&name, b"created").unwrap());
            private
                .write(&name, b"replaced")
                .unwrap_or_else(|error| panic!("round {round}: {error}"));
            assert_eq!(fs::read(private.path().join(&name)).unwrap(), b"replaced");
        }
    }

    #[test]
    fn leftovers_of_an_interrupted_write_are_removed() {
        let (_directory, private) = private();
        let leftover = private.path().join(".servers.json.00000000000000aa.tmp");
        fs::write(&leftover, b"half").unwrap();
        fs::write(private.path().join("servers.json"), b"{}").unwrap();
        fs::write(private.path().join("notes.tmp"), b"not ours").unwrap();
        private.remove_leftovers().unwrap();
        assert!(!leftover.exists());
        assert!(private.path().join("servers.json").exists());
        assert!(private.path().join("notes.tmp").exists());
    }

    #[cfg(unix)]
    #[test]
    fn written_files_and_created_directories_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let private = Dir::open(&directory.path().join("nested").join("private")).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(private.path()), 0o700);
        let inner = private.subdirectory("keys").unwrap();
        assert_eq!(mode(inner.path()), 0o700);

        let path = private.path().join("data.json");
        private.write("data.json", b"{}").unwrap();
        assert_eq!(mode(&path), 0o600);
        private.write("data.json", b"{\"again\":true}").unwrap();
        assert_eq!(mode(&path), 0o600);
        inner.create("device.key", b"secret").unwrap();
        assert_eq!(mode(&inner.path().join("device.key")), 0o600);
        let mut buffer = [0u8; 64];
        let read = private.read_private("data.json", &mut buffer).unwrap();
        assert_eq!(&buffer[..read.unwrap()], b"{\"again\":true}");
    }

    #[cfg(unix)]
    #[test]
    fn a_file_or_directory_that_others_can_access_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let (_directory, private) = private();
        let path = private.path().join("secret");
        private.write("secret", b"secret").unwrap();
        let mut buffer = [0u8; 64];

        for mode in [0o640, 0o604, 0o660, 0o644] {
            fs::set_permissions(&path, fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                matches!(
                    private.read_private("secret", &mut buffer),
                    Err(StorageError::NotPrivate { .. })
                ),
                "{mode:o}"
            );
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(
            private
                .read_private("secret", &mut buffer)
                .unwrap()
                .is_some()
        );

        let inner = private.subdirectory("keys").unwrap();
        for mode in [0o750, 0o705, 0o755] {
            fs::set_permissions(inner.path(), fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                matches!(
                    private.subdirectory("keys"),
                    Err(StorageError::NotPrivate { .. })
                ),
                "{mode:o}"
            );
            fs::set_permissions(private.path(), fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                matches!(
                    Dir::open(private.path()),
                    Err(StorageError::NotPrivate { .. })
                ),
                "{mode:o}"
            );
            fs::set_permissions(private.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_symbolic_link_in_place_of_a_file_or_a_directory_is_refused() {
        let (directory, private) = private();
        private.write("real", b"secret").unwrap();
        std::os::unix::fs::symlink(private.path().join("real"), private.path().join("link"))
            .unwrap();
        let mut buffer = [0u8; 64];
        assert!(matches!(
            private.read_private("link", &mut buffer),
            Err(StorageError::UnexpectedKind { .. })
        ));
        assert!(matches!(
            private.read("link", 64),
            Err(StorageError::UnexpectedKind { .. })
        ));
        assert!(
            private
                .read_private("absent", &mut buffer)
                .unwrap()
                .is_none()
        );

        let elsewhere = Dir::open(&directory.path().join("elsewhere")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), private.path().join("keys")).unwrap();
        assert!(matches!(
            private.subdirectory("keys"),
            Err(StorageError::UnexpectedKind { .. })
        ));
        std::os::unix::fs::symlink(elsewhere.path(), directory.path().join("root")).unwrap();
        assert!(matches!(
            Dir::open(&directory.path().join("root")),
            Err(StorageError::UnexpectedKind { .. })
        ));
        fs::create_dir(private.path().join("directory")).unwrap();
        assert!(matches!(
            private.read("directory", 64),
            Err(StorageError::UnexpectedKind { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn a_write_does_not_follow_a_link_that_waits_at_the_name() {
        let (directory, private) = private();
        let outside = directory.path().join("outside");
        fs::write(&outside, b"untouched").unwrap();
        std::os::unix::fs::symlink(&outside, private.path().join("servers.json")).unwrap();
        private.write("servers.json", b"new").unwrap();
        assert_eq!(fs::read(&outside).unwrap(), b"untouched");
        assert_eq!(private.read("servers.json", 64).unwrap().unwrap(), b"new");

        std::os::unix::fs::symlink(&outside, private.path().join("device.key")).unwrap();
        assert!(!private.create("device.key", b"key").unwrap());
        assert_eq!(fs::read(&outside).unwrap(), b"untouched");
    }

    #[test]
    fn the_unix_rule_checks_the_kind_the_owner_and_the_access() {
        use policy::{Refusal, unix};
        assert_eq!(unix(true, 501, 0o700, 501, true), Ok(()));
        assert_eq!(unix(true, 501, 0o600, 501, true), Ok(()));
        assert_eq!(unix(false, 501, 0o600, 501, true), Err(Refusal::Kind));
        assert_eq!(unix(true, 502, 0o600, 501, true), Err(Refusal::Owner));
        assert_eq!(unix(true, 0, 0o700, 501, true), Err(Refusal::Owner));
        for mode in [0o610, 0o601, 0o640, 0o604, 0o755, 0o777] {
            assert_eq!(
                unix(true, 501, mode, 501, true),
                Err(Refusal::Access),
                "{mode:o}"
            );
        }
        assert_eq!(unix(true, 501, 0o644, 501, false), Ok(()));
        assert_eq!(unix(true, 502, 0o644, 501, false), Err(Refusal::Owner));
    }

    #[test]
    fn the_windows_rule_checks_the_kind_the_owner_and_every_entry_of_the_access_list() {
        use policy::{Entry, Owner, Refusal, windows};
        let user = Entry {
            allows: true,
            for_user: true,
        };
        let other = Entry {
            allows: true,
            for_user: false,
        };
        let denial = Entry {
            allows: false,
            for_user: true,
        };
        assert_eq!(windows(true, Owner::User, Some(&[user]), true), Ok(()));
        assert_eq!(
            windows(true, Owner::Administration, Some(&[user, user]), true),
            Ok(())
        );
        assert_eq!(
            windows(true, Owner::Other, Some(&[user]), true),
            Err(Refusal::Owner)
        );
        assert_eq!(
            windows(true, Owner::Other, Some(&[user]), false),
            Err(Refusal::Owner)
        );
        assert_eq!(
            windows(false, Owner::User, Some(&[user]), true),
            Err(Refusal::Kind)
        );
        for list in [&[user, other][..], &[other][..], &[denial][..]] {
            assert_eq!(
                windows(true, Owner::User, Some(list), true),
                Err(Refusal::Access)
            );
        }
        assert_eq!(windows(true, Owner::User, None, true), Err(Refusal::Access));
        assert_eq!(windows(true, Owner::User, Some(&[]), true), Ok(()));
        assert_eq!(windows(true, Owner::User, Some(&[other]), false), Ok(()));
    }

    #[test]
    fn a_file_over_the_limit_is_refused() {
        let (_directory, private) = private();
        private.write("big", &[0u8; 65]).unwrap();
        assert!(matches!(
            private.read("big", 64),
            Err(StorageError::TooLarge { limit: 64, .. })
        ));
        assert_eq!(private.read("big", 65).unwrap().unwrap().len(), 65);
        let mut buffer = [0u8; 64];
        assert!(matches!(
            private.read_private("big", &mut buffer),
            Err(StorageError::TooLarge { limit: 64, .. })
        ));
        let mut buffer = [0u8; 65];
        assert_eq!(private.read_private("big", &mut buffer).unwrap(), Some(65));
    }

    #[test]
    fn the_current_format_version_is_read() {
        let (_directory, private) = private();
        private
            .write("sample.json", br#"{"format_version":2,"label":"now"}"#)
            .unwrap();
        assert_eq!(
            SAMPLE.load::<Sample>(&private).unwrap().unwrap(),
            Sample {
                format_version: 2,
                label: "now".to_owned()
            }
        );
        private.remove("sample.json").unwrap();
        assert!(SAMPLE.load::<Sample>(&private).unwrap().is_none());
    }

    #[test]
    fn an_older_format_version_is_migrated() {
        let (_directory, private) = private();
        let old = br#"{"format_version":1,"title":"before"}"#;
        private.write("sample.json", old).unwrap();
        assert_eq!(
            SAMPLE.load::<Sample>(&private).unwrap().unwrap(),
            Sample {
                format_version: 2,
                label: "before".to_owned()
            }
        );
        assert_eq!(fs::read(private.path().join("sample.json")).unwrap(), old);
    }

    #[test]
    fn a_newer_format_version_is_refused() {
        let (_directory, private) = private();
        private
            .write(
                "sample.json",
                br#"{"format_version":3,"label":"later","more":1}"#,
            )
            .unwrap();
        assert!(matches!(
            SAMPLE.load::<Sample>(&private),
            Err(StorageError::NewerFormat {
                found: 3,
                supported: 2,
                ..
            })
        ));
    }

    #[test]
    fn a_corrupted_file_is_reported_without_its_content() {
        let (_directory, private) = private();
        let cases: [&[u8]; 7] = [
            b"",
            b"{",
            b"not json at all",
            br#"{"label":"no version"}"#,
            br#"{"format_version":0,"label":"zero"}"#,
            br#"{"format_version":2,"label":{"hidden":"text"}}"#,
            br#"{"format_version":1,"label":"no title to migrate"}"#,
        ];
        for content in cases {
            private.write("sample.json", content).unwrap();
            let error = SAMPLE.load::<Sample>(&private).unwrap_err();
            assert!(
                matches!(error, StorageError::Corrupted { kind: "sample", .. }),
                "{error}"
            );
            assert!(!error.to_string().contains("hidden"), "{error}");
        }
    }

    #[test]
    fn the_directory_lock_makes_a_second_holder_wait() {
        let (_directory, private) = private();
        let path = private.path().to_owned();
        let first = private.lock().unwrap();
        let (sender, receiver) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let other = Dir::open(&path).unwrap();
            let _second = other.lock().unwrap();
            sender.send(()).unwrap();
        });
        assert!(
            receiver
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err()
        );
        drop(first);
        receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        waiter.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_lock_of_a_directory_is_the_same_under_another_spelling_of_its_path() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("real")).unwrap();
        std::os::unix::fs::symlink(
            directory.path().join("real"),
            directory.path().join("alias"),
        )
        .unwrap();
        let direct = directory.path().join("real").join("junto");
        let aliased = directory.path().join("alias").join("junto");
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let first = Dir::open(&direct).unwrap();
            let second = Dir::open(&aliased).unwrap();
            let outer = first.lock().unwrap();
            let inner = second.lock().unwrap();
            drop(inner);
            drop(outer);
            drop(second.lock().unwrap());
            sender.send(()).unwrap();
        });
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the thread waits for the lock that it holds itself");
    }

    #[test]
    fn the_thread_that_holds_the_directory_lock_can_take_it_again() {
        let (_directory, private) = private();
        let path = private.path().to_owned();
        let outer = private.lock().unwrap();
        let inner = private.lock().unwrap();
        drop(inner);
        let (sender, receiver) = std::sync::mpsc::channel();
        let waiter = std::thread::spawn(move || {
            let other = Dir::open(&path).unwrap();
            let _lock = other.lock().unwrap();
            sender.send(()).unwrap();
        });
        assert!(
            receiver
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_err()
        );
        drop(outer);
        receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap();
        waiter.join().unwrap();
        drop(private.lock().unwrap());
    }
}
