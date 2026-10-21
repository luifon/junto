//! Directory handles on Unix: every open is relative to a directory descriptor and never
//! follows a link, and each check reads the opened object.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use rustix::fs::{AtFlags, FileType, Mode, OFlags};
use rustix::io::Errno;

use super::{Fault, policy};

pub(super) struct Handle {
    directory: OwnedFd,
}

fn directory_flags() -> OFlags {
    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC
}

fn file_mode() -> Mode {
    Mode::RUSR | Mode::WUSR
}

/// A link at the last part of the path, which the open did not follow.
fn is_link(error: Errno) -> bool {
    matches!(error, Errno::LOOP | Errno::NOTDIR | Errno::MLINK)
}

fn opened(result: rustix::io::Result<OwnedFd>) -> Result<OwnedFd, Fault> {
    result.map_err(|error| {
        if is_link(error) {
            Fault::Refused(policy::Refusal::Kind)
        } else {
            Fault::Io(error.into())
        }
    })
}

/// Checks the opened object itself: its kind, its owner and its permission bits.
#[allow(clippy::useless_conversion)] // st_mode is u16 on macOS and u32 on Linux.
fn check(descriptor: &OwnedFd, expected: FileType, private: bool) -> Result<(), Fault> {
    let status = rustix::fs::fstat(descriptor).map_err(io::Error::from)?;
    policy::unix(
        FileType::from_raw_mode(status.st_mode) == expected,
        status.st_uid,
        u32::from(status.st_mode) & 0o777,
        rustix::process::geteuid().as_raw(),
        private,
    )
    .map_err(Fault::Refused)
}

fn created(result: rustix::io::Result<()>) -> io::Result<()> {
    match result {
        Ok(()) | Err(Errno::EXIST) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

impl Handle {
    pub(super) fn open_root(path: &Path) -> Result<Self, Fault> {
        created(rustix::fs::mkdir(path, Mode::RWXU))?;
        let directory = opened(rustix::fs::open(path, directory_flags(), Mode::empty()))?;
        check(&directory, FileType::Directory, true)?;
        Ok(Self { directory })
    }

    pub(super) fn open_directory(&self, name: &str) -> Result<Self, Fault> {
        created(rustix::fs::mkdirat(&self.directory, name, Mode::RWXU))?;
        let directory = opened(rustix::fs::openat(
            &self.directory,
            name,
            directory_flags(),
            Mode::empty(),
        ))?;
        check(&directory, FileType::Directory, true)?;
        Ok(Self { directory })
    }

    /// The device and the file number of the opened directory.
    #[allow(clippy::unnecessary_cast)]
    pub(super) fn identity(&self) -> io::Result<(u64, u64)> {
        let status = rustix::fs::fstat(&self.directory)?;
        Ok((status.st_dev as u64, status.st_ino as u64))
    }

    pub(super) fn check(&self) -> Result<(), Fault> {
        check(&self.directory, FileType::Directory, true)
    }

    pub(super) fn open_file(&self, name: &str, private: bool) -> Result<Option<File>, Fault> {
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        let file = match rustix::fs::openat(&self.directory, name, flags, Mode::empty()) {
            Ok(file) => file,
            Err(Errno::NOENT) => return Ok(None),
            Err(error) => return opened(Err(error)).map(|_| None),
        };
        check(&file, FileType::RegularFile, private)?;
        Ok(Some(File::from(file)))
    }

    /// A new file that only the user can access. Fails when the name is taken.
    pub(super) fn create_file(&self, name: &str) -> io::Result<File> {
        let flags =
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        Ok(File::from(rustix::fs::openat(
            &self.directory,
            name,
            flags,
            file_mode(),
        )?))
    }

    pub(super) fn open_lock(&self, name: &str) -> io::Result<File> {
        let flags = OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        Ok(File::from(rustix::fs::openat(
            &self.directory,
            name,
            flags,
            file_mode(),
        )?))
    }

    pub(super) fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        Ok(rustix::fs::renameat(
            &self.directory,
            from,
            &self.directory,
            to,
        )?)
    }

    /// Gives `from` the second name `to`. False when `to` exists.
    pub(super) fn name_new(&self, from: &str, to: &str) -> io::Result<bool> {
        let linked =
            rustix::fs::linkat(&self.directory, from, &self.directory, to, AtFlags::empty());
        match linked {
            Ok(()) => Ok(true),
            Err(Errno::EXIST) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn remove(&self, name: &str) -> io::Result<bool> {
        match rustix::fs::unlinkat(&self.directory, name, AtFlags::empty()) {
            Ok(()) => Ok(true),
            Err(Errno::NOENT) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    pub(super) fn names(&self) -> io::Result<Vec<OsString>> {
        let mut names = Vec::new();
        for entry in rustix::fs::Dir::read_from(&self.directory)? {
            let entry = entry?;
            names.push(OsStr::from_bytes(entry.file_name().to_bytes()).to_owned());
        }
        Ok(names)
    }

    pub(super) fn sync(&self) -> io::Result<()> {
        Ok(rustix::fs::fsync(&self.directory)?)
    }
}
