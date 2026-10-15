//! An uncompressed archive of regular files. Links and special files are not supported.

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use super::{Error, MAX_ARCHIVE};

const MAGIC: &[u8] = b"JUNTO-ARCHIVE-1\n";
const MAX_FILES: usize = 10_000;

fn valid_name(name: &str) -> bool {
    name.len() <= 1024 && !name.is_empty() && name.split('/').all(super::filename)
}

pub fn pack(source: &Path, output: &mut impl Write) -> Result<(), Error> {
    if !fs::symlink_metadata(source)?.is_dir() {
        return Err(Error::Invalid("archive root"));
    }
    output.write_all(MAGIC)?;
    let mut budget = (0usize, 0u64);
    write_directory(source, source, output, &mut budget)?;
    output.write_all(&0u16.to_be_bytes())?;
    Ok(())
}

fn write_directory(
    root: &Path,
    directory: &Path,
    output: &mut impl Write,
    budget: &mut (usize, u64),
) -> Result<(), Error> {
    let mut children = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    children.sort_by_key(|entry| entry.file_name());
    for entry in children {
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| Error::Invalid("archive path"))?;
        let name = relative
            .to_str()
            .ok_or(Error::Invalid("archive path"))?
            .replace(std::path::MAIN_SEPARATOR, "/");
        if !valid_name(&name) {
            return Err(Error::Invalid("archive path"));
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.is_dir() {
            write_directory(root, &path, output, budget)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(Error::Invalid("archive file type"));
        }
        budget.0 += 1;
        budget.1 = budget
            .1
            .checked_add(metadata.len())
            .ok_or(Error::Invalid("archive size"))?;
        if budget.0 > MAX_FILES || budget.1 > MAX_ARCHIVE {
            return Err(Error::Invalid("archive size"));
        }
        output.write_all(&(name.len() as u16).to_be_bytes())?;
        output.write_all(name.as_bytes())?;
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            u8::from(metadata.permissions().mode() & 0o111 != 0)
        };
        #[cfg(not(unix))]
        let executable = 0;
        output.write_all(&[executable])?;
        output.write_all(&metadata.len().to_be_bytes())?;
        if std::io::copy(&mut File::open(path)?.take(metadata.len() + 1), output)? != metadata.len()
        {
            return Err(Error::Invalid("archive input changed"));
        }
    }
    Ok(())
}

/// Extract into a new directory under a caller-controlled parent. Errors remove it.
pub fn unpack(input: &mut impl Read, destination: &Path) -> Result<(), Error> {
    super::create_private_directory(destination)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(destination, fs::Permissions::from_mode(0o700))?;
    }
    let result = extract(input, destination);
    if result.is_err() {
        let _ = fs::remove_dir_all(destination);
    }
    result
}

/// One file of an archive, after the checks of its name, mode and size.
struct Entry {
    name: String,
    // Windows has no executable mode; the file name decides.
    #[cfg_attr(not(unix), allow(dead_code))]
    executable: bool,
    size: u64,
}

/// Reads the entries of an archive in order. Each entry's content must be read or skipped
/// before the next header.
struct Entries {
    names: HashSet<String>,
    total: u64,
}

impl Entries {
    fn start(input: &mut impl Read) -> Result<Self, Error> {
        let mut magic = vec![0; MAGIC.len()];
        input.read_exact(&mut magic)?;
        if magic != MAGIC {
            return Err(Error::Invalid("archive format"));
        }
        Ok(Self {
            names: HashSet::new(),
            total: 0,
        })
    }

    fn next(&mut self, input: &mut impl Read) -> Result<Option<Entry>, Error> {
        let mut length = [0; 2];
        input.read_exact(&mut length)?;
        let length = usize::from(u16::from_be_bytes(length));
        if length == 0 {
            let mut trailing = [0];
            if input.read(&mut trailing)? != 0 {
                return Err(Error::Invalid("trailing archive bytes"));
            }
            return Ok(None);
        }
        if length > 1024 {
            return Err(Error::Invalid("archive path"));
        }
        let mut name = vec![0; length];
        input.read_exact(&mut name)?;
        let name = String::from_utf8(name).map_err(|_| Error::Invalid("archive path"))?;
        if !valid_name(&name)
            || !self.names.insert(name.to_ascii_lowercase())
            || self.names.len() > MAX_FILES
        {
            return Err(Error::Invalid("archive path"));
        }
        let mut executable = [0];
        input.read_exact(&mut executable)?;
        if executable[0] > 1 {
            return Err(Error::Invalid("archive mode"));
        }
        let mut size = [0; 8];
        input.read_exact(&mut size)?;
        let size = u64::from_be_bytes(size);
        self.total = self
            .total
            .checked_add(size)
            .ok_or(Error::Invalid("archive size"))?;
        if self.total > MAX_ARCHIVE {
            return Err(Error::Invalid("archive size"));
        }
        Ok(Some(Entry {
            name,
            executable: executable[0] == 1,
            size,
        }))
    }
}

/// Writes the one file `name` of an archive to the new file `destination`, with the mode
/// of an executable on Unix when the archive marks it so. The rest of the archive is read
/// and checked as an extraction would check it.
pub fn extract_file(input: &mut impl Read, name: &str, destination: &Path) -> Result<(), Error> {
    let mut entries = Entries::start(input)?;
    let mut found = false;
    let result = (|| {
        while let Some(entry) = entries.next(input)? {
            if entry.name != name {
                if std::io::copy(&mut (&mut *input).take(entry.size), &mut std::io::sink())?
                    != entry.size
                {
                    return Err(Error::Invalid("truncated archive"));
                }
                continue;
            }
            found = true;
            write_file(input, &entry, destination)?;
        }
        if !found {
            return Err(Error::Invalid("archive file missing"));
        }
        Ok(())
    })();
    if result.is_err() && found {
        let _ = fs::remove_file(destination);
    }
    result
}

fn write_file(input: &mut impl Read, entry: &Entry, path: &Path) -> Result<(), Error> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(path)?;
    if std::io::copy(&mut (&mut *input).take(entry.size), &mut output)? != entry.size {
        return Err(Error::Invalid("truncated archive"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        output.set_permissions(fs::Permissions::from_mode(if entry.executable {
            0o755
        } else {
            0o644
        }))?;
    }
    output.sync_all()?;
    Ok(())
}

fn extract(input: &mut impl Read, destination: &Path) -> Result<(), Error> {
    let mut entries = Entries::start(input)?;
    while let Some(entry) = entries.next(input)? {
        let path = destination.join(&entry.name);
        let parent = path.parent().ok_or(Error::Invalid("archive path"))?;
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for directory in parent
                .ancestors()
                .take_while(|directory| *directory != destination)
            {
                fs::set_permissions(directory, fs::Permissions::from_mode(0o755))?;
            }
        }
        write_file(input, &entry, &path)?;
    }
    Ok(())
}

/// Reject links and unsafe permissions before a candidate can be installed.
pub fn verify_modes(directory: &Path) -> Result<(), Error> {
    let metadata = fs::symlink_metadata(directory)?;
    if !metadata.is_dir() && !metadata.is_file() {
        return Err(Error::Invalid("bundle file type"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = metadata.permissions().mode() & 0o7777;
        if if metadata.is_dir() {
            mode != 0o755
        } else {
            mode != 0o644 && mode != 0o755
        } {
            return Err(Error::Invalid("bundle permissions"));
        }
    }
    if metadata.is_dir() {
        for entry in fs::read_dir(directory)? {
            verify_modes(&entry?.path())?;
        }
    }
    Ok(())
}
