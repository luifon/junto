//! The files of recordings below the directory of the recordings (ADR 0606).
//!
//! The recorder writes into this directory. The server builds every path from identifiers
//! that it generated, never from a name that the recorder reports, and it opens a file one
//! path part at a time without following a link.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use junto_protocol::ids::{RecordingId, RecordingSegmentId};

/// The directory in which the start script of the recorder saves the files that a lost
/// recorder left, one directory for each job.
pub const RESCUED: &str = "rescued";
const TRANSCRIPT: &str = "transcript.json";
const JOB_DIRECTORY: &str = "transcription";
pub const JOB_FILE: &str = "job.json";
pub const JOB_TRANSCRIPT: &str = "transcript.json";
const TRACK_EXTENSION: &str = "ogg";
const VIDEO_EXTENSION: &str = "mp4";
/// The end of the name of a file that the server has not finished writing.
const UNFINISHED: &str = ".partial";

/// True for a text that is one safe path part: 1 to 128 letters, digits, `_` and `-`.
pub fn is_name(text: &str) -> bool {
    (1..=128).contains(&text.len())
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

/// A path below the directory of the recordings, as its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place(Vec<String>);

impl Place {
    fn of(parts: &[&dyn std::fmt::Display]) -> Self {
        Self(parts.iter().map(ToString::to_string).collect())
    }

    /// The directory of one attachment, using its server-generated identifier.
    pub fn attachment(attachment: junto_protocol::ids::AttachmentId) -> Self {
        Self::of(&[&attachment])
    }

    /// The directory of one export, below the directory of the exports.
    pub fn export(export: junto_protocol::ids::ExportId) -> Self {
        Self::of(&[&export])
    }

    /// The directory of a recording.
    pub fn recording(recording: RecordingId) -> Self {
        Self::of(&[&recording])
    }

    /// The file of an audio track, where the recorder writes it.
    pub fn track(recording: RecordingId, segment: RecordingSegmentId) -> Self {
        Self::of(&[&recording, &format_args!("{segment}.{TRACK_EXTENSION}")])
    }

    /// The directory of the pieces of a composite.
    pub fn pieces(recording: RecordingId, segment: RecordingSegmentId) -> Self {
        Self::of(&[&recording, &segment])
    }

    /// The video that the server joins from the pieces of a composite.
    pub fn video(recording: RecordingId, segment: RecordingSegmentId) -> Self {
        Self::of(&[&recording, &format_args!("{segment}.{VIDEO_EXTENSION}")])
    }

    /// The transcript of a recording as the worker wrote it.
    pub fn transcript(recording: RecordingId) -> Self {
        Self::of(&[&recording, &TRANSCRIPT])
    }

    /// The directory that holds the directories of the runs of the transcription worker.
    pub fn jobs(recording: RecordingId) -> Self {
        Self::of(&[&recording, &JOB_DIRECTORY])
    }

    /// A new directory for the run of the transcription worker that is the attempt
    /// `attempt` of its job. The name states the attempt, so that a run tells the
    /// directories of earlier attempts from the directory of a later one.
    pub fn job_run(recording: RecordingId, attempt: u32) -> Self {
        let run = uuid::Uuid::new_v4().simple();
        Self::of(&[&recording, &JOB_DIRECTORY, &format_args!("{attempt}-{run}")])
    }

    /// The directory of the files that were saved of a job of a lost recorder. `None` for a
    /// job identifier that is no safe path part.
    pub fn rescued(job_id: &str) -> Option<Self> {
        is_name(job_id).then(|| Self::of(&[&RESCUED, &job_id]))
    }

    /// The audio track of `segment` among the files that were saved of a job.
    pub fn rescued_track(job_id: &str, segment: RecordingSegmentId) -> Option<Self> {
        let mut place = Self::rescued(job_id)?;
        place.0.push(format!("{segment}.{TRACK_EXTENSION}"));
        Some(place)
    }

    /// A new name for a video while the server joins it. The video gets its name when it is
    /// whole.
    pub fn unfinished_video(recording: RecordingId, segment: RecordingSegmentId) -> Self {
        let unique = uuid::Uuid::new_v4().simple();
        Self::of(&[&recording, &format_args!("{segment}.{unique}{UNFINISHED}")])
    }

    /// A file or directory inside this directory.
    pub fn with(&self, name: &str) -> Self {
        let mut parts = self.0.clone();
        parts.push(name.to_owned());
        Self(parts)
    }

    pub fn parts(&self) -> &[String] {
        &self.0
    }

    /// The path as the queue of removals stores it.
    pub fn stored(&self) -> String {
        self.0.join("/")
    }

    /// Reads a stored path. Every part must be a safe name.
    pub fn from_stored(text: &str) -> Option<Self> {
        let parts: Vec<String> = text.split('/').map(str::to_owned).collect();
        parts
            .iter()
            .all(|part| is_name(part))
            .then_some(Self(parts))
    }

    pub fn under(&self, root: &Path) -> PathBuf {
        self.0
            .iter()
            .fold(root.to_path_buf(), |path, part| path.join(part))
    }
}

#[cfg(unix)]
mod system {
    use rustix::fs::{
        AtFlags, Dir, FileType, Mode, OFlags, fstat, linkat, mkdirat, open, openat, renameat,
        statat, unlinkat,
    };
    use std::ffi::CString;
    use std::fs::File;
    use std::io::{self, Write};
    use std::os::fd::OwnedFd;
    use std::path::Path;

    use super::Place;
    use crate::recorder::COMPOSITE_PIECE_PREFIX;

    /// The deepest tree that a removal deletes. The server creates at most three levels.
    const DEEPEST: usize = 16;
    const PIECE_EXTENSION: &str = "ts";

    /// The number of a piece of a composite, from its file name `part_<number>.ts`.
    fn piece_number(name: &str) -> Option<u64> {
        let stem = name.strip_suffix(PIECE_EXTENSION)?.strip_suffix('.')?;
        let digits = stem
            .strip_prefix(COMPOSITE_PIECE_PREFIX)?
            .strip_prefix('_')?;
        (!digits.is_empty()
            && digits.len() <= 12
            && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| digits.parse().ok())
        .flatten()
    }

    fn directory_flags() -> OFlags {
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC
    }

    /// Opens the directory `name` inside `directory`. A link is an error of its own, so
    /// that it is not taken for a directory that is not there.
    fn inner(directory: &OwnedFd, name: &str) -> io::Result<OwnedFd> {
        let found = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
        if FileType::from_raw_mode(found.st_mode) == FileType::Symlink {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a link in the path of a file of a recording",
            ));
        }
        let flags = directory_flags() | OFlags::NOFOLLOW;
        Ok(openat(directory, name, flags, Mode::empty())?)
    }

    /// Opens the directory at `parts` below `root`. No part may be a link.
    fn directory(root: &Path, parts: &[String]) -> io::Result<OwnedFd> {
        let mut directory = open(root, directory_flags(), Mode::empty())?;
        for part in parts {
            directory = inner(&directory, part)?;
        }
        Ok(directory)
    }

    /// The directory that holds `place`, and the name of `place` inside it.
    fn holder<'a>(root: &Path, place: &'a Place) -> io::Result<(OwnedFd, &'a str)> {
        let Some((name, above)) = place.parts().split_last() else {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        };
        Ok((directory(root, above)?, name.as_str()))
    }

    fn kind(directory: &OwnedFd, name: &std::ffi::CStr) -> io::Result<FileType> {
        let found = statat(directory, name, AtFlags::SYMLINK_NOFOLLOW)?;
        Ok(FileType::from_raw_mode(found.st_mode))
    }

    pub fn open_file(root: &Path, place: &Place) -> io::Result<File> {
        let (directory, name) = holder(root, place)?;
        let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
        let file = openat(&directory, name, flags, Mode::empty())?;
        if FileType::from_raw_mode(fstat(&file)?.st_mode) != FileType::RegularFile {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not a regular file",
            ));
        }
        Ok(File::from(file))
    }

    /// The names in a directory, without `.` and `..`.
    fn names(directory: &OwnedFd) -> io::Result<Vec<CString>> {
        let mut names = Vec::new();
        for entry in Dir::read_from(directory)? {
            let entry = entry?;
            let name = entry.file_name();
            if name != c"." && name != c".." {
                names.push(name.to_owned());
            }
        }
        Ok(names)
    }

    pub fn pieces(root: &Path, place: &Place) -> io::Result<Vec<String>> {
        let directory = directory(root, place.parts())?;
        let mut numbered = Vec::new();
        for name in names(&directory)? {
            let Some((number, text)) = name
                .to_str()
                .ok()
                .and_then(|text| Some((piece_number(text)?, text)))
            else {
                continue;
            };
            if kind(&directory, &name)? == FileType::RegularFile {
                numbered.push((number, text.to_owned()));
            }
        }
        numbered.sort();
        Ok(numbered.into_iter().map(|(_, name)| name).collect())
    }

    /// The names in a directory that `wanted` accepts.
    pub fn listed(
        root: &Path,
        place: &Place,
        wanted: impl Fn(&str) -> bool,
    ) -> io::Result<Vec<String>> {
        let directory = directory(root, place.parts())?;
        Ok(names(&directory)?
            .iter()
            .filter_map(|name| name.to_str().ok())
            .filter(|name| wanted(name))
            .map(str::to_owned)
            .collect())
    }

    /// The first `most` names of a directory that `wanted` accepts and that come after
    /// `after`, in the order of the names, each with the time of the last change of its
    /// entry in seconds since 1970. A link is not followed.
    pub fn entries_after(
        root: &Path,
        place: &Place,
        after: Option<&str>,
        most: usize,
        wanted: impl Fn(&str) -> bool,
    ) -> io::Result<Vec<(String, i64)>> {
        let directory = directory(root, place.parts())?;
        // The names are read one at a time and only the first `most` are kept, so the
        // memory of a pass does not grow with the directory.
        let mut first = std::collections::BTreeSet::new();
        for entry in Dir::read_from(&directory)? {
            let name = entry?.file_name().to_owned();
            if name.as_c_str() == c"." || name.as_c_str() == c".." {
                continue;
            }
            let Ok(name) = name.to_str() else {
                continue;
            };
            if !wanted(name) || after.is_some_and(|after| name <= after) {
                continue;
            }
            first.insert(name.to_owned());
            if first.len() > most {
                first.pop_last();
            }
        }
        let mut entries = Vec::with_capacity(first.len());
        for name in first {
            match statat(&directory, name.as_str(), AtFlags::SYMLINK_NOFOLLOW) {
                // The type of the field differs between systems.
                Ok(found) => {
                    let changed = i64::try_from(i128::from(found.st_mtime)).unwrap_or(i64::MAX);
                    entries.push((name, changed));
                }
                Err(rustix::io::Errno::NOENT) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Ok(entries)
    }

    fn remove_entry(directory: &OwnedFd, name: &std::ffi::CStr, depth: usize) -> io::Result<()> {
        if kind(directory, name)? != FileType::Directory {
            return Ok(unlinkat(directory, name, AtFlags::empty())?);
        }
        if depth >= DEEPEST {
            return Err(io::Error::other("the directory is deeper than a recording"));
        }
        let inner = openat(
            directory,
            name,
            directory_flags() | OFlags::NOFOLLOW,
            Mode::empty(),
        )?;
        // An entry that another process deleted in the meantime is gone, which is what the
        // removal wants. The directory itself is deleted only when it holds nothing.
        for inner_name in names(&inner)? {
            match remove_entry(&inner, &inner_name, depth + 1) {
                Err(error) if super::is_absent(&error) => {}
                removed => removed?,
            }
        }
        Ok(unlinkat(directory, name, AtFlags::REMOVEDIR)?)
    }

    pub fn remove(root: &Path, place: &Place) -> io::Result<()> {
        let (directory, name) = holder(root, place)?;
        let name = CString::new(name).map_err(io::Error::other)?;
        remove_entry(&directory, &name, 0)
    }

    pub fn finish(root: &Path, from: &Place, to: &Place) -> io::Result<()> {
        let (from_directory, from_name) = holder(root, from)?;
        let (to_directory, to_name) = holder(root, to)?;
        let linked = match linkat(
            &from_directory,
            from_name,
            &to_directory,
            to_name,
            AtFlags::empty(),
        ) {
            Err(rustix::io::Errno::EXIST) => Ok(()),
            linked => linked,
        };
        unlinkat(&from_directory, from_name, AtFlags::empty())?;
        linked?;
        rustix::fs::fsync(&to_directory)?;
        Ok(rustix::fs::fsync(&from_directory)?)
    }

    pub fn create_directory(root: &Path, place: &Place) -> io::Result<()> {
        let mut directory = open(root, directory_flags(), Mode::empty())?;
        for part in place.parts() {
            match mkdirat(&directory, part.as_str(), Mode::from_raw_mode(0o777)) {
                Ok(()) | Err(rustix::io::Errno::EXIST) => {}
                Err(error) => return Err(error.into()),
            }
            directory = inner(&directory, part)?;
        }
        Ok(())
    }

    pub fn rename(root: &Path, from: &Place, to: &Place) -> io::Result<()> {
        let (from_directory, from_name) = holder(root, from)?;
        let (to_directory, to_name) = holder(root, to)?;
        Ok(renameat(
            &from_directory,
            from_name,
            &to_directory,
            to_name,
        )?)
    }

    pub fn remove_if_empty(root: &Path, place: &Place) -> io::Result<()> {
        let (directory, name) = holder(root, place)?;
        match unlinkat(&directory, name, AtFlags::REMOVEDIR) {
            Err(rustix::io::Errno::NOTEMPTY | rustix::io::Errno::EXIST) => Ok(()),
            removed => Ok(removed?),
        }
    }

    pub fn create_file(root: &Path, place: &Place) -> io::Result<File> {
        let (directory, name) = holder(root, place)?;
        let flags =
            OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        Ok(File::from(openat(
            &directory,
            name,
            flags,
            Mode::from_raw_mode(0o600),
        )?))
    }

    pub fn write_new(root: &Path, place: &Place, bytes: &[u8]) -> io::Result<()> {
        let (directory, name) = holder(root, place)?;
        let flags =
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut file = File::from(openat(&directory, name, flags, Mode::from_raw_mode(0o666))?);
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(rustix::fs::fsync(&directory)?)
    }
}

#[cfg(any(not(unix), test))]
mod unsupported_system {
    use std::fs::File;
    use std::io;
    use std::path::Path;

    use super::Place;

    fn unsupported<T>() -> io::Result<T> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the files of recordings are read and written on Unix systems only",
        ))
    }

    pub fn open_file(_: &Path, _: &Place) -> io::Result<File> {
        unsupported()
    }

    pub fn pieces(_: &Path, _: &Place) -> io::Result<Vec<String>> {
        unsupported()
    }

    pub fn listed(_: &Path, _: &Place, _: impl Fn(&str) -> bool) -> io::Result<Vec<String>> {
        unsupported()
    }

    pub fn entries_after(
        _: &Path,
        _: &Place,
        _: Option<&str>,
        _: usize,
        _: impl Fn(&str) -> bool,
    ) -> io::Result<Vec<(String, i64)>> {
        unsupported()
    }

    pub fn remove(_: &Path, _: &Place) -> io::Result<()> {
        unsupported()
    }

    pub fn finish(_: &Path, _: &Place, _: &Place) -> io::Result<()> {
        unsupported()
    }

    pub fn create_directory(_: &Path, _: &Place) -> io::Result<()> {
        unsupported()
    }

    pub fn rename(_: &Path, _: &Place, _: &Place) -> io::Result<()> {
        unsupported()
    }

    pub fn write_new(_: &Path, _: &Place, _: &[u8]) -> io::Result<()> {
        unsupported()
    }

    pub fn create_file(_: &Path, _: &Place) -> io::Result<File> {
        unsupported()
    }

    pub fn remove_if_empty(_: &Path, _: &Place) -> io::Result<()> {
        unsupported()
    }
}

#[cfg(not(unix))]
use unsupported_system as system;

/// True for the error of a file or directory that is not there.
pub fn is_absent(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

/// Opens a regular file below `root`. No part of its path may be a link.
pub fn open(root: &Path, place: &Place) -> io::Result<File> {
    system::open_file(root, place)
}

/// The size of the regular file at `place`. `None` when no file is there.
pub fn size(root: &Path, place: &Place) -> io::Result<Option<u64>> {
    match open(root, place) {
        Ok(file) => Ok(Some(file.metadata()?.len())),
        Err(error) if is_absent(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

/// The names of the pieces of a composite that are in its directory, in the order of their
/// numbers. Empty when the directory is not there.
pub fn pieces(root: &Path, place: &Place) -> io::Result<Vec<String>> {
    match system::pieces(root, place) {
        Err(error) if is_absent(&error) => Ok(Vec::new()),
        listed => listed,
    }
}

/// The names of the files in a directory that the server did not finish writing. Empty
/// when the directory is not there.
pub fn unfinished(root: &Path, place: &Place) -> io::Result<Vec<String>> {
    match system::listed(root, place, |name| name.ends_with(UNFINISHED)) {
        Err(error) if is_absent(&error) => Ok(Vec::new()),
        listed => listed,
    }
}

/// The first `most` entries of `rescued/` after the name `after`, in the order of their
/// names, each with the time of its last change in seconds since 1970. Only a name that
/// can be the identifier of a job is listed. Empty when the directory is not there.
pub fn rescued_entries(
    root: &Path,
    after: Option<&str>,
    most: usize,
) -> io::Result<Vec<(String, i64)>> {
    let rescued = Place(vec![RESCUED.to_owned()]);
    match system::entries_after(root, &rescued, after, most, is_name) {
        Err(error) if is_absent(&error) => Ok(Vec::new()),
        listed => listed,
    }
}

/// The number of the attempt in the name of the directory of a run of the worker.
fn run_attempt(name: &str) -> Option<u32> {
    let (attempt, run) = name.split_once('-')?;
    let plain = !attempt.is_empty() && attempt.bytes().all(|byte| byte.is_ascii_digit());
    (plain && is_name(run))
        .then(|| attempt.parse().ok())
        .flatten()
}

/// The directories of the runs of the worker for a recording that belong to an attempt
/// before `attempt`. Empty when the recording has no directory of runs.
pub fn earlier_runs(root: &Path, recording: RecordingId, attempt: u32) -> io::Result<Vec<Place>> {
    let runs = Place::jobs(recording);
    let earlier = |name: &str| run_attempt(name).is_some_and(|of_run| of_run < attempt);
    match system::listed(root, &runs, earlier) {
        Ok(names) => Ok(names.iter().map(|name| runs.with(name)).collect()),
        Err(error) if is_absent(&error) => Ok(Vec::new()),
        Err(error) => Err(error),
    }
}

/// Gives a finished file its name. A file that already has the name stays: `from` is
/// deleted in both cases. No directory above the two places may be a link.
pub fn finish(root: &Path, from: &Place, to: &Place) -> io::Result<()> {
    system::finish(root, from, to)
}

/// Creates the directory at `place` and the directories above it, as far as they are
/// missing. No part that exists may be a link.
pub fn create_directory(root: &Path, place: &Place) -> io::Result<()> {
    system::create_directory(root, place)
}

/// Moves the file that was saved of a lost job to its place in the directory of its
/// recording, which is created when the recorder did not create it. No directory above the
/// two places may be a link.
pub fn take_rescued(root: &Path, from: &Place, to: &Place) -> io::Result<()> {
    if let Some((_, above)) = to.parts().split_last() {
        create_directory(root, &Place(above.to_vec()))?;
    }
    system::rename(root, from, to)
}

/// Creates a new file that only the user of the server reads, opened for reading and
/// writing. A file or a link at the place is an error, and no directory above the place may
/// be a link.
pub fn create_file(root: &Path, place: &Place) -> io::Result<File> {
    system::create_file(root, place)
}

/// Writes a new file. A file or a link at the place is an error, and no directory above the
/// place may be a link.
pub fn write_new(root: &Path, place: &Place, bytes: &[u8]) -> io::Result<()> {
    system::write_new(root, place, bytes)
}

/// Deletes a directory that holds nothing. A directory that holds something stays, and
/// nothing at the place is no error.
pub fn remove_if_empty(root: &Path, place: &Place) -> io::Result<()> {
    match system::remove_if_empty(root, place) {
        Err(error) if is_absent(&error) => Ok(()),
        removed => removed,
    }
}

/// Deletes a file or a directory with what it holds. A link is deleted, not followed, and
/// no directory above the place may be a link. Nothing at the place is no error.
pub fn remove(root: &Path, place: &Place) -> io::Result<()> {
    match system::remove(root, place) {
        Err(error) if is_absent(&error) => Ok(()),
        removed => removed,
    }
}

#[cfg(test)]
#[cfg(unix)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("junto-files-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn places_are_built_from_identifiers() {
        let (recording, segment) = (RecordingId::generate(), RecordingSegmentId::generate());
        assert_eq!(
            Place::track(recording, segment).stored(),
            format!("{recording}/{segment}.ogg")
        );
        assert_eq!(
            Place::video(recording, segment).stored(),
            format!("{recording}/{segment}.mp4")
        );
        assert_eq!(
            Place::rescued_track("EG_abc", segment).unwrap().stored(),
            format!("rescued/EG_abc/{segment}.ogg")
        );
        for unsafe_job in ["", "..", "EG/1", "EG 1", "a.b", &"a".repeat(129)] {
            assert_eq!(Place::rescued(unsafe_job), None, "{unsafe_job:?}");
        }
        let stored = Place::recording(recording).stored();
        assert_eq!(
            Place::from_stored(&stored),
            Some(Place::recording(recording))
        );
        assert_eq!(Place::from_stored("../etc"), None);
        assert_eq!(Place::from_stored("a//b"), None);
        assert_eq!(Place::from_stored("a/b.c"), None);
    }

    #[test]
    fn a_file_is_opened_only_as_a_regular_file_without_a_link_in_its_path() {
        let scratch = Scratch::new();
        let root = &scratch.0;
        std::fs::create_dir_all(root.join("real")).unwrap();
        std::fs::write(root.join("real/file.ogg"), b"sound").unwrap();
        std::fs::write(root.join("outside"), b"secret").unwrap();
        std::os::unix::fs::symlink(root.join("outside"), root.join("real/link.ogg")).unwrap();
        std::os::unix::fs::symlink(root.join("real"), root.join("linked")).unwrap();

        let place = |parts: &[&str]| Place(parts.iter().map(|part| (*part).to_owned()).collect());
        assert_eq!(size(root, &place(&["real", "file.ogg"])).unwrap(), Some(5));
        assert_eq!(size(root, &place(&["real", "absent.ogg"])).unwrap(), None);
        assert_eq!(size(root, &place(&["absent", "file.ogg"])).unwrap(), None);
        assert!(open(root, &place(&["real", "link.ogg"])).is_err());
        assert!(open(root, &place(&["linked", "file.ogg"])).is_err());
        assert!(open(root, &place(&["real"])).is_err());
    }

    #[test]
    fn pieces_are_listed_in_the_order_of_their_numbers() {
        let scratch = Scratch::new();
        let root = &scratch.0;
        let directory = root.join("pieces");
        std::fs::create_dir_all(&directory).unwrap();
        for name in [
            "part_00010.ts",
            "part_00002.ts",
            "part_00001.ts",
            "playlist.m3u8",
            "part_x.ts",
            "other_00003.ts",
            "part_00004.mp4",
        ] {
            std::fs::write(directory.join(name), b"x").unwrap();
        }
        std::os::unix::fs::symlink(
            root.join("pieces/part_00001.ts"),
            directory.join("part_00003.ts"),
        )
        .unwrap();
        let place = Place(vec!["pieces".to_owned()]);
        assert_eq!(
            pieces(root, &place).unwrap(),
            ["part_00001.ts", "part_00002.ts", "part_00010.ts"]
        );
        assert!(
            pieces(root, &Place(vec!["absent".to_owned()]))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_finished_file_gets_its_name_and_never_replaces_another() {
        let scratch = Scratch::new();
        let root = &scratch.0;
        let place = |name: &str| Place(vec![name.to_owned()]);
        std::fs::write(root.join("a.partial"), b"first").unwrap();
        std::fs::write(root.join("b.partial"), b"second").unwrap();
        let mut waiting = unfinished(root, &Place(Vec::new())).unwrap();
        waiting.sort();
        assert_eq!(waiting, ["a.partial", "b.partial"]);

        finish(root, &place("a.partial"), &place("video.mp4")).unwrap();
        finish(root, &place("b.partial"), &place("video.mp4")).unwrap();
        assert_eq!(std::fs::read(root.join("video.mp4")).unwrap(), b"first");
        assert!(unfinished(root, &Place(Vec::new())).unwrap().is_empty());
        assert!(unfinished(root, &place("absent")).unwrap().is_empty());

        std::fs::create_dir_all(root.join("rescued/EG_1")).unwrap();
        std::fs::write(root.join("rescued/EG_1/track.ogg"), b"sound").unwrap();
        let from = Place(vec!["rescued".into(), "EG_1".into(), "track.ogg".into()]);
        let to = Place(vec!["recording".into(), "track.ogg".into()]);
        take_rescued(root, &from, &to).unwrap();
        assert_eq!(
            std::fs::read(root.join("recording/track.ogg")).unwrap(),
            b"sound"
        );
        assert!(!root.join("rescued/EG_1/track.ogg").exists());
    }

    /// A link above the place of a file leads out of the directory of the recordings. No
    /// removal, move or new name goes through it.
    #[test]
    fn no_change_goes_through_a_link_above_its_place() {
        let scratch = Scratch::new();
        let root = scratch.0.join("recordings");
        let outside = scratch.0.join("outside");
        std::fs::create_dir_all(outside.join("EG_1")).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(outside.join("EG_1/track.ogg"), b"kept").unwrap();
        std::fs::write(outside.join("a.partial"), b"video").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("rescued")).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("recording")).unwrap();
        let place = |parts: &[&str]| Place(parts.iter().map(|part| (*part).to_owned()).collect());

        assert!(remove(&root, &place(&["rescued", "EG_1"])).is_err());
        assert!(outside.join("EG_1/track.ogg").exists());
        let from = place(&["rescued", "EG_1", "track.ogg"]);
        assert!(take_rescued(&root, &from, &place(&["real", "track.ogg"])).is_err());
        assert!(outside.join("EG_1/track.ogg").exists());
        let through = place(&["recording", "a.partial"]);
        assert!(finish(&root, &through, &place(&["recording", "video.mp4"])).is_err());
        assert!(outside.join("a.partial").exists());
        assert!(!outside.join("video.mp4").exists());
        assert!(create_directory(&root, &place(&["recording", "transcription"])).is_err());
        assert!(!outside.join("transcription").exists());
        assert!(write_new(&root, &place(&["recording", "transcript.json"]), b"{}").is_err());
        assert!(!outside.join("transcript.json").exists());

        // The link itself is removed, and what it points to stays.
        remove(&root, &place(&["rescued"])).unwrap();
        assert!(std::fs::symlink_metadata(root.join("rescued")).is_err());
        assert!(outside.join("EG_1/track.ogg").exists());
    }

    #[test]
    fn the_saved_files_of_jobs_are_listed_in_the_order_of_their_names_and_in_parts() {
        let scratch = Scratch::new();
        let root = &scratch.0;
        assert!(rescued_entries(root, None, 10).unwrap().is_empty());
        for name in ["EG_c", "EG_a", "EG_d", "EG_b", "not a job", "a.b"] {
            std::fs::create_dir_all(root.join("rescued").join(name)).unwrap();
        }
        std::os::unix::fs::symlink(root, root.join("rescued/EG_e")).unwrap();
        let names = |after: Option<&str>, most| -> Vec<String> {
            let entries = rescued_entries(root, after, most).unwrap();
            entries.into_iter().map(|(name, _)| name).collect()
        };
        assert_eq!(names(None, 2), ["EG_a", "EG_b"]);
        assert_eq!(names(Some("EG_b"), 2), ["EG_c", "EG_d"]);
        assert_eq!(names(Some("EG_d"), 2), ["EG_e"]);
        assert_eq!(names(None, 10).len(), 5);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        for (name, changed) in rescued_entries(root, None, 10).unwrap() {
            assert!((now - changed).abs() < 3600, "{name} {changed} {now}");
        }
    }

    #[test]
    fn the_runs_of_earlier_attempts_are_told_from_later_ones() {
        let scratch = Scratch::new();
        let root = &scratch.0;
        let recording = RecordingId::generate();
        assert!(earlier_runs(root, recording, 5).unwrap().is_empty());
        let own = Place::job_run(recording, 2);
        let runs = Place::jobs(recording).under(root);
        for name in [
            "1-aaaa", "2-bbbb", "3-cccc", "10-dddd", "other", "-eeee", "1-", "x-ffff",
        ] {
            std::fs::create_dir_all(runs.join(name)).unwrap();
        }
        create_directory(root, &own).unwrap();
        let earlier = |attempt| {
            let mut names: Vec<String> = earlier_runs(root, recording, attempt)
                .unwrap()
                .iter()
                .map(|place| place.parts().last().cloned().unwrap())
                .collect();
            names.sort();
            names
        };
        assert!(earlier(1).is_empty());
        assert_eq!(earlier(2), ["1-aaaa"]);
        let own_name = own.parts().last().cloned().unwrap();
        assert!(own_name.starts_with("2-") && own_name.len() == 34);
        let mut before_ten = vec![
            "1-aaaa".to_owned(),
            "2-bbbb".to_owned(),
            "3-cccc".to_owned(),
        ];
        before_ten.push(own_name);
        before_ten.sort();
        assert_eq!(earlier(10), before_ten);
    }

    #[test]
    fn removal_deletes_a_directory_and_does_not_follow_a_link() {
        let scratch = Scratch::new();
        let root = &scratch.0;
        std::fs::create_dir_all(root.join("kept")).unwrap();
        std::fs::write(root.join("kept/file"), b"kept").unwrap();
        std::fs::create_dir_all(root.join("gone/inner")).unwrap();
        std::fs::write(root.join("gone/inner/file"), b"x").unwrap();
        std::os::unix::fs::symlink(root.join("kept"), root.join("link")).unwrap();

        remove(root, &Place(vec!["gone".to_owned()])).unwrap();
        assert!(!root.join("gone").exists());
        remove(root, &Place(vec!["link".to_owned()])).unwrap();
        assert!(root.join("kept/file").exists());
        assert!(std::fs::symlink_metadata(root.join("link")).is_err());
        remove(root, &Place(vec!["absent".to_owned()])).unwrap();
    }
}

#[cfg(test)]
mod unsupported_system_tests {
    use super::*;

    #[test]
    fn every_unsupported_storage_operation_refuses() {
        let root = Path::new("unused-storage-root");
        let place = Place::recording(RecordingId::generate());
        let errors = [
            unsupported_system::pieces(root, &place).unwrap_err(),
            unsupported_system::listed(root, &place, |_| true).unwrap_err(),
            unsupported_system::entries_after(root, &place, None, 1, |_| true).unwrap_err(),
            unsupported_system::finish(root, &place, &place).unwrap_err(),
            unsupported_system::create_directory(root, &place).unwrap_err(),
            unsupported_system::rename(root, &place, &place).unwrap_err(),
            unsupported_system::write_new(root, &place, b"").unwrap_err(),
            unsupported_system::remove_if_empty(root, &place).unwrap_err(),
        ];
        assert!(
            errors
                .iter()
                .all(|error| error.kind() == io::ErrorKind::Unsupported)
        );
    }

    #[test]
    fn recording_and_attachment_files_fail_closed_without_descriptor_operations() {
        let root = Path::new("unused-storage-root");
        for place in [
            Place::recording(RecordingId::generate()),
            Place::attachment(junto_protocol::ids::AttachmentId::generate()),
        ] {
            assert_eq!(
                unsupported_system::open_file(root, &place)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::Unsupported
            );
            assert_eq!(
                unsupported_system::create_file(root, &place)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::Unsupported
            );
            assert_eq!(
                unsupported_system::remove(root, &place).unwrap_err().kind(),
                io::ErrorKind::Unsupported
            );
        }
    }
}
