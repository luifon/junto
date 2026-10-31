//! Windows file storage below pinned directory handles (ADR 0606).
#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use windows_sys::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

use super::{Place, piece_number};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

fn component(name: &str) -> io::Result<&OsStr> {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].into_iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|number| {
                number.len() == 1 && matches!(number.as_bytes()[0], b'1'..=b'9')
            })
        });
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.ends_with('.')
        || device
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.-".contains(&byte))
    {
        return Err(invalid("an unsafe file-storage path component"));
    }
    Ok(OsStr::new(name))
}

fn reparse(metadata: &fs::Metadata) -> bool {
    metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

fn pin(path: &Path) -> io::Result<File> {
    let directory = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = directory.metadata()?;
    if reparse(&metadata) {
        return Err(invalid("a reparse point in the path of a stored file"));
    }
    if !metadata.is_dir() {
        return Err(io::ErrorKind::NotADirectory.into());
    }
    Ok(directory)
}

struct Directory {
    pins: Vec<File>,
    path: PathBuf,
}

impl Directory {
    fn root(root: &Path) -> io::Result<Self> {
        let path = fs::canonicalize(root)?;
        Ok(Self {
            pins: vec![pin(&path)?],
            path,
        })
    }

    fn enter(&mut self, name: &OsStr) -> io::Result<()> {
        let path = self.path.join(name);
        self.pins.push(pin(&path)?);
        self.path = path;
        Ok(())
    }

    fn child(&self, name: &OsStr) -> io::Result<Self> {
        let mut child = Self {
            pins: self
                .pins
                .iter()
                .map(File::try_clone)
                .collect::<io::Result<_>>()?,
            path: self.path.clone(),
        };
        child.enter(name)?;
        Ok(child)
    }

    fn names(&self) -> io::Result<Vec<OsString>> {
        fs::read_dir(&self.path)?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect()
    }
}

fn directory(root: &Path, parts: &[String]) -> io::Result<Directory> {
    let mut directory = Directory::root(root)?;
    for part in parts {
        directory.enter(component(part)?)?;
    }
    Ok(directory)
}

fn holder<'a>(root: &Path, place: &'a Place) -> io::Result<(Directory, &'a OsStr)> {
    let (name, above) = place
        .parts()
        .split_last()
        .ok_or_else(|| invalid("an empty file path"))?;
    Ok((directory(root, above)?, component(name)?))
}

pub fn open_file(root: &Path, place: &Place) -> io::Result<File> {
    let (parent, name) = holder(root, place)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(parent.path.join(name))?;
    let metadata = file.metadata()?;
    if reparse(&metadata) || !metadata.is_file() {
        return Err(invalid("not a regular stored file"));
    }
    Ok(file)
}

pub fn pieces(root: &Path, place: &Place) -> io::Result<Vec<String>> {
    let directory = directory(root, place.parts())?;
    let mut numbered = Vec::new();
    for name in directory.names()? {
        let Some((number, name)) = name
            .to_str()
            .and_then(|name| Some((piece_number(name)?, name)))
        else {
            continue;
        };
        let metadata = fs::symlink_metadata(directory.path.join(name))?;
        if metadata.is_file() && !reparse(&metadata) {
            numbered.push((number, name.to_owned()));
        }
    }
    numbered.sort();
    Ok(numbered.into_iter().map(|(_, name)| name).collect())
}

pub fn listed(
    root: &Path,
    place: &Place,
    wanted: impl Fn(&str) -> bool,
) -> io::Result<Vec<String>> {
    Ok(directory(root, place.parts())?
        .names()?
        .into_iter()
        .filter_map(|name| name.into_string().ok())
        .filter(|name| wanted(name))
        .collect())
}

pub fn entries_after(
    root: &Path,
    place: &Place,
    after: Option<&str>,
    most: usize,
    wanted: impl Fn(&str) -> bool,
) -> io::Result<Vec<(String, i64)>> {
    let directory = directory(root, place.parts())?;
    let mut first = std::collections::BTreeSet::new();
    for entry in fs::read_dir(&directory.path)? {
        let Ok(name) = entry?.file_name().into_string() else {
            continue;
        };
        if wanted(&name) && after.is_none_or(|after| name.as_str() > after) {
            first.insert(name);
            if first.len() > most {
                first.pop_last();
            }
        }
    }
    let mut entries = Vec::with_capacity(first.len());
    for name in first {
        match fs::symlink_metadata(directory.path.join(&name)) {
            Ok(metadata) => {
                let changed = match metadata.modified()?.duration_since(UNIX_EPOCH) {
                    Ok(age) => i64::try_from(age.as_secs()).unwrap_or(i64::MAX),
                    Err(before) => {
                        let age = before.duration();
                        let seconds = age
                            .as_secs()
                            .saturating_add(u64::from(age.subsec_nanos() != 0));
                        -i64::try_from(seconds).unwrap_or(i64::MAX)
                    }
                };
                entries.push((name, changed));
            }
            Err(error) if super::is_absent(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(entries)
}

fn remove_entry(parent: &Directory, name: &OsStr, depth: usize) -> io::Result<()> {
    let path = parent.path.join(name);
    let metadata = fs::symlink_metadata(&path)?;
    if reparse(&metadata) {
        return if metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0 {
            fs::remove_dir(path)
        } else {
            fs::remove_file(path)
        };
    }
    if !metadata.is_dir() {
        return fs::remove_file(path);
    }
    if depth >= 16 {
        return Err(io::Error::other("the directory is deeper than a recording"));
    }
    let child = parent.child(name)?;
    for name in child.names()? {
        match remove_entry(&child, &name, depth + 1) {
            Err(error) if super::is_absent(&error) => {}
            removed => removed?,
        }
    }
    drop(child);
    fs::remove_dir(path)
}

pub fn remove(root: &Path, place: &Place) -> io::Result<()> {
    let (parent, name) = holder(root, place)?;
    remove_entry(&parent, name, 0)
}

fn wide(path: &Path) -> io::Result<Vec<u16>> {
    let mut text: Vec<u16> = path.as_os_str().encode_wide().collect();
    if text.contains(&0) {
        return Err(invalid("a zero in a file path"));
    }
    text.push(0);
    Ok(text)
}

pub fn finish(root: &Path, from: &Place, to: &Place) -> io::Result<()> {
    let (source, from) = holder(root, from)?;
    let (destination, to) = holder(root, to)?;
    let (from, to) = (source.path.join(from), destination.path.join(to));
    let (from_text, to_text) = (wide(&from)?, wide(&to)?);
    // SAFETY: both paths are terminated, and their parent directories remain pinned.
    if unsafe { MoveFileExW(from_text.as_ptr(), to_text.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::AlreadyExists {
            return fs::remove_file(from);
        }
        return Err(error);
    }
    Ok(())
}

pub fn create_directory(root: &Path, place: &Place) -> io::Result<()> {
    let mut directory = Directory::root(root)?;
    for part in place.parts() {
        let name = component(part)?;
        match fs::create_dir(directory.path.join(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        directory.enter(name)?;
    }
    Ok(())
}

pub fn rename(root: &Path, from: &Place, to: &Place) -> io::Result<()> {
    let (source, from) = holder(root, from)?;
    let (destination, to) = holder(root, to)?;
    fs::rename(source.path.join(from), destination.path.join(to))
}

pub fn write_new(root: &Path, place: &Place, bytes: &[u8]) -> io::Result<()> {
    let mut file = create_file(root, place)?;
    file.write_all(bytes)?;
    file.sync_all()
}

pub fn create_file(root: &Path, place: &Place) -> io::Result<File> {
    let (parent, name) = holder(root, place)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(parent.path.join(name))
}

pub fn remove_if_empty(root: &Path, place: &Place) -> io::Result<()> {
    let (parent, name) = holder(root, place)?;
    match fs::remove_dir(parent.path.join(name)) {
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(()),
        removed => removed,
    }
}

#[cfg(test)]
#[path = "windows/tests.rs"]
mod tests;
