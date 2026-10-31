//! Standard-library storage for the Windows window-test server (ADR 0912).
#![forbid(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const FILE_SHARE_READ: u32 = 1;
const FILE_SHARE_WRITE: u32 = 2;

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
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
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

    fn validate(&self) -> io::Result<()> {
        for pin in &self.pins {
            let metadata = pin.metadata()?;
            if reparse(&metadata) || !metadata.is_dir() {
                return Err(invalid("a changed stored directory"));
            }
        }
        Ok(())
    }

    fn child_path(&self, name: &OsStr) -> io::Result<PathBuf> {
        self.validate()?;
        Ok(self.path.join(name))
    }

    fn enter(&mut self, name: &OsStr) -> io::Result<()> {
        let path = self.child_path(name)?;
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
        self.validate()?;
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

fn regular_file(path: &Path, writable: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(writable)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    if reparse(&metadata) || !metadata.is_file() {
        return Err(invalid("not a regular stored file"));
    }
    Ok(file)
}

pub fn open_file(root: &Path, place: &Place) -> io::Result<File> {
    let (parent, name) = holder(root, place)?;
    regular_file(&parent.child_path(name)?, false)
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
        let metadata = fs::symlink_metadata(directory.child_path(OsStr::new(name))?)?;
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
    directory.validate()?;
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
        match fs::symlink_metadata(directory.child_path(OsStr::new(&name))?) {
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
    let path = parent.child_path(name)?;
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

pub fn finish(root: &Path, from: &Place, to: &Place) -> io::Result<()> {
    let (source, from) = holder(root, from)?;
    let (destination, to) = holder(root, to)?;
    let (from, to) = (source.child_path(from)?, destination.child_path(to)?);
    let file = regular_file(&from, true)?;
    file.sync_all()?;
    match fs::hard_link(&from, to) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    fs::remove_file(from)
}

pub fn create_directory(root: &Path, place: &Place) -> io::Result<()> {
    let mut directory = Directory::root(root)?;
    for part in place.parts() {
        let name = component(part)?;
        match fs::create_dir(directory.child_path(name)?) {
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
    fs::rename(source.child_path(from)?, destination.child_path(to)?)
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
        .open(parent.child_path(name)?)
}

pub fn remove_if_empty(root: &Path, place: &Place) -> io::Result<()> {
    let (parent, name) = holder(root, place)?;
    match fs::remove_dir(parent.child_path(name)?) {
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(()),
        removed => removed,
    }
}

#[cfg(test)]
#[path = "windows/tests.rs"]
mod tests;
