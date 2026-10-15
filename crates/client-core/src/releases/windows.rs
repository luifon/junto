//! Windows system tools for the shared installation decisions (ADR 0755).
use super::{Error, install::System};
use crate::updates::Version;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Default)]
pub struct WindowsTools;

impl WindowsTools {
    pub fn for_running() -> Self {
        Self
    }
}

impl System for WindowsTools {
    fn requirement(&self, _bundle: &Path) -> Result<String, Error> {
        Err(Error::Invalid(
            "a Windows package has no code-signing identity",
        ))
    }
    fn matches(&self, _bundle: &Path, _required: &str) -> Result<(), Error> {
        Err(Error::Invalid(
            "a Windows package has no code-signing identity",
        ))
    }
    fn version(&self, _application: &Path) -> Result<Version, Error> {
        Err(Error::Invalid(
            "a Windows package states its version in its description",
        ))
    }
    fn exchange(&self, application: &Path, candidate: &Path) -> Result<(), Error> {
        exchange_entries(application, candidate)
    }
}

fn names(directory: &Path) -> Result<Vec<std::ffi::OsString>, Error> {
    let mut names = fs::read_dir(directory)?
        .map(|entry| entry.map(|entry| entry.file_name()))
        .collect::<Result<Vec<_>, _>>()?;
    names.sort();
    Ok(names)
}

/// Moves each entry back from `from` to `to`, also after a failed move. Returns whether
/// every move succeeded.
fn move_back(moved: &[std::ffi::OsString], from: &Path, to: &Path) -> bool {
    let mut every = true;
    for name in moved.iter().rev() {
        if fs::rename(from.join(name), to.join(name)).is_err() {
            every = false;
        }
    }
    every
}

/// Replaces the entries of the application directory with those of the candidate, one by one.
/// Windows refuses to rename a directory that holds a running program, and lets a running
/// program be renamed. The replaced entries go to a directory beside the candidate.
pub fn exchange_entries(application: &Path, candidate: &Path) -> Result<(), Error> {
    let replaced: PathBuf = {
        let mut name = candidate.as_os_str().to_owned();
        name.push(".replaced");
        name.into()
    };
    // Both lists are read before the first move: a failure after it must move entries back.
    let old = names(application)?;
    let new = names(candidate)?;
    fs::create_dir(&replaced)?;
    let mut moved_out = Vec::new();
    for name in &old {
        if let Err(error) = fs::rename(application.join(name), replaced.join(name)) {
            return Err(if move_back(&moved_out, &replaced, application) {
                error.into()
            } else {
                Error::ExchangeIncomplete
            });
        }
        moved_out.push(name.clone());
    }
    let mut moved_in = Vec::new();
    for name in &new {
        if let Err(error) = fs::rename(candidate.join(name), application.join(name)) {
            let new_back = move_back(&moved_in, application, candidate);
            let old_back = move_back(&moved_out, &replaced, application);
            return Err(if new_back && old_back {
                error.into()
            } else {
                Error::ExchangeIncomplete
            });
        }
        moved_in.push(name.clone());
    }
    Ok(())
}
