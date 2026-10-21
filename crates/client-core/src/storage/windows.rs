//! Directory handles on Windows. A directory is opened without following a reparse point
//! and held open without the right to delete or rename it, so its path cannot be given to
//! another object while the handle exists. A file is opened inside such a directory, again
//! without following a reparse point, and each check reads the opened handle.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use super::{Fault, policy, windows_acl};

const FILE_SHARE_READ: u32 = 0x1;
const FILE_SHARE_WRITE: u32 = 0x2;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

pub(super) struct Handle {
    /// Keeps the directory in place. Its sharing mode excludes deletion and renaming.
    directory: File,
    path: PathBuf,
}

/// Checks the opened handle: its kind, that it is no reparse point, its owner and its
/// access list.
fn check(file: &File, directory: bool, private: bool) -> Result<(), Fault> {
    let metadata = file.metadata()?;
    let plain = metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT == 0;
    let security = windows_acl::security_of(file)?;
    policy::windows(
        plain && metadata.is_dir() == directory,
        security.owner,
        security.entries.as_deref(),
        private,
    )
    .map_err(Fault::Refused)
}

fn open_directory(path: &Path) -> Result<Handle, Fault> {
    let directory = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    check(&directory, true, true)?;
    Ok(Handle {
        directory,
        path: path.to_owned(),
    })
}

fn created(result: io::Result<()>) -> io::Result<()> {
    match result {
        Err(error) if error.kind() != io::ErrorKind::AlreadyExists => Err(error),
        _ => Ok(()),
    }
}

impl Handle {
    pub(super) fn open_root(path: &Path) -> Result<Self, Fault> {
        created(windows_acl::create_private_dir(path))?;
        open_directory(path)
    }

    pub(super) fn open_directory(&self, name: &str) -> Result<Self, Fault> {
        let path = self.path.join(name);
        created(windows_acl::create_private_dir(&path))?;
        open_directory(&path)
    }

    /// The volume and the file number of the opened directory.
    pub(super) fn identity(&self) -> io::Result<(u64, u64)> {
        windows_acl::identity_of(&self.directory)
    }

    pub(super) fn check(&self) -> Result<(), Fault> {
        check(&self.directory, true, true)
    }

    pub(super) fn open_file(&self, name: &str, private: bool) -> Result<Option<File>, Fault> {
        let opened = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(self.path.join(name));
        let file = match opened {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        check(&file, false, private)?;
        Ok(Some(file))
    }

    /// A new file, which takes the access list of its directory. Fails when the name is
    /// taken.
    pub(super) fn create_file(&self, name: &str) -> io::Result<File> {
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(self.path.join(name))
    }

    pub(super) fn open_lock(&self, name: &str) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(self.path.join(name))
    }

    pub(super) fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        fs::rename(self.path.join(from), self.path.join(to))
    }

    /// Renames `from` to `to` when no file has the name `to`. False when `to` exists. A
    /// hard link is not used: after one, the system kept the file open for up to a second, and
    /// a rename over the file failed with access denied during that time (ADR 0711).
    pub(super) fn name_new(&self, from: &str, to: &str) -> io::Result<bool> {
        match windows_acl::rename_new(&self.path.join(from), &self.path.join(to)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(super) fn remove(&self, name: &str) -> io::Result<bool> {
        match fs::remove_file(self.path.join(name)) {
            Ok(()) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(super) fn names(&self) -> io::Result<Vec<OsString>> {
        fs::read_dir(&self.path)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect()
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        Ok(())
    }
}
