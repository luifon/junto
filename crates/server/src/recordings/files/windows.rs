//! Windows file storage below pinned directory handles (ADR 0606).
#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::path::Path;
use std::time::UNIX_EPOCH;

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, SYNCHRONIZE,
};
#[path = "windows/native.rs"]
mod native;
const DIRECTORY_ACCESS: u32 = FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | FILE_TRAVERSE;
const SHARING: u32 = FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE;

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
        .access_mode(DIRECTORY_ACCESS | SYNCHRONIZE)
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
    file: File,
}

impl Directory {
    fn root(root: &Path) -> io::Result<Self> {
        Ok(Self {
            pins: Vec::new(),
            file: pin(&fs::canonicalize(root)?)?,
        })
    }
    fn validate(&self) -> io::Result<()> {
        for file in self.pins.iter().chain([&self.file]) {
            let metadata = file.metadata()?;
            if reparse(&metadata) || !metadata.is_dir() {
                return Err(invalid("a changed stored directory"));
            }
        }
        Ok(())
    }
    fn open(
        &self,
        name: &OsStr,
        access: u32,
        disposition: u32,
        options: u32,
        sharing: u32,
    ) -> io::Result<File> {
        self.validate()?;
        native::open(&self.file, name, access, disposition, options, sharing)
    }
    fn enter(&mut self, name: &OsStr) -> io::Result<()> {
        let next = self.open(
            name,
            DIRECTORY_ACCESS,
            FILE_OPEN,
            FILE_DIRECTORY_FILE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )?;
        let metadata = next.metadata()?;
        if reparse(&metadata) || !metadata.is_dir() {
            return Err(invalid("not a regular stored directory"));
        }
        self.pins.push(std::mem::replace(&mut self.file, next));
        Ok(())
    }
    fn child(&self, name: &OsStr) -> io::Result<Self> {
        let mut child = Self {
            pins: self
                .pins
                .iter()
                .map(File::try_clone)
                .collect::<io::Result<_>>()?,
            file: self.file.try_clone()?,
        };
        child.enter(name)?;
        Ok(child)
    }
    fn names(&self) -> io::Result<Vec<OsString>> {
        self.validate()?;
        native::names(&self.file)
    }
    fn metadata(&self, name: &OsStr) -> io::Result<fs::Metadata> {
        self.open(name, FILE_READ_ATTRIBUTES, FILE_OPEN, 0, SHARING)?
            .metadata()
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
    let file = parent.open(
        name,
        FILE_GENERIC_READ,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE,
        SHARING,
    )?;
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
        let metadata = directory.metadata(OsStr::new(name))?;
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
    native::for_each_name(&directory.file, |entry| {
        let Ok(name) = entry.into_string() else {
            return;
        };
        if wanted(&name) && after.is_none_or(|after| name.as_str() > after) {
            first.insert(name);
            if first.len() > most {
                first.pop_last();
            }
        }
    })?;
    let mut entries = Vec::with_capacity(first.len());
    for name in first {
        match directory.metadata(OsStr::new(&name)) {
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
    let metadata = parent.metadata(name)?;
    if !reparse(&metadata) && metadata.is_dir() {
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
    }
    native::remove(&parent.open(name, DELETE, FILE_OPEN, 0, SHARING)?)
}

pub fn remove(root: &Path, place: &Place) -> io::Result<()> {
    let (parent, name) = holder(root, place)?;
    remove_entry(&parent, name, 0)
}

pub fn finish(root: &Path, from: &Place, to: &Place) -> io::Result<()> {
    let (source, from) = holder(root, from)?;
    let (destination, to) = holder(root, to)?;
    let file = source.open(
        from,
        DELETE | FILE_GENERIC_READ | FILE_GENERIC_WRITE,
        FILE_OPEN,
        FILE_NON_DIRECTORY_FILE,
        SHARING,
    )?;
    file.sync_all()?;
    destination.validate()?;
    match native::rename(&file, &destination.file, to, false) {
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => native::remove(&file),
        moved => {
            moved?;
            file.sync_all()
        }
    }
}

pub fn create_directory(root: &Path, place: &Place) -> io::Result<()> {
    let mut directory = Directory::root(root)?;
    for part in place.parts() {
        let name = component(part)?;
        let created = directory.open(
            name,
            DIRECTORY_ACCESS,
            FILE_OPEN_IF,
            FILE_DIRECTORY_FILE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
        )?;
        let metadata = created.metadata()?;
        if reparse(&metadata) || !metadata.is_dir() {
            return Err(invalid("not a regular stored directory"));
        }
        directory
            .pins
            .push(std::mem::replace(&mut directory.file, created));
    }
    Ok(())
}

pub fn rename(root: &Path, from: &Place, to: &Place) -> io::Result<()> {
    let (source, from) = holder(root, from)?;
    let (destination, to) = holder(root, to)?;
    let file = source.open(from, DELETE, FILE_OPEN, 0, SHARING)?;
    destination.validate()?;
    native::rename(&file, &destination.file, to, true)
}

pub fn write_new(root: &Path, place: &Place, bytes: &[u8]) -> io::Result<()> {
    let mut file = create_file(root, place)?;
    file.write_all(bytes)?;
    file.sync_all()
}

pub fn create_file(root: &Path, place: &Place) -> io::Result<File> {
    let (parent, name) = holder(root, place)?;
    parent.open(
        name,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE,
        FILE_CREATE,
        FILE_NON_DIRECTORY_FILE,
        SHARING,
    )
}

pub fn remove_if_empty(root: &Path, place: &Place) -> io::Result<()> {
    let (parent, name) = holder(root, place)?;
    let file = parent.open(name, DELETE, FILE_OPEN, FILE_DIRECTORY_FILE, SHARING)?;
    match native::remove(&file) {
        Err(error) if error.kind() == io::ErrorKind::DirectoryNotEmpty => Ok(()),
        removed => removed,
    }
}

#[cfg(test)]
#[path = "windows/tests.rs"]
mod tests;
