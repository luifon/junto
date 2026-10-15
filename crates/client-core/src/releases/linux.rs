//! Linux system tools for the shared installation decisions: the directory package and the
//! AppImage (ADR 0754).
use super::{Error, install::System};
use crate::updates::Version;
use std::path::{Path, PathBuf};

#[derive(Clone, Default)]
pub struct LinuxTools {
    /// The AppImage that this process runs from, with the version of this build. An AppImage
    /// states its version only to the program inside it.
    running: Option<(PathBuf, Version)>,
}

impl LinuxTools {
    pub fn for_running() -> Self {
        Self {
            running: std::env::current_exe()
                .ok()
                .and_then(|executable| running_appimage(&executable))
                .map(|appimage| (appimage, Version::THIS_BUILD)),
        }
    }

    /// Tools for a test that installs over an AppImage that runs a stated version.
    pub fn with_running(appimage: &Path, version: Version) -> Result<Self, Error> {
        Ok(Self {
            running: Some((appimage.canonicalize()?, version)),
        })
    }
}

/// The AppImage that runs `executable`: the runtime of an AppImage names its file in
/// `APPIMAGE` and the directory of its contents in `APPDIR`.
pub fn running_appimage(executable: &Path) -> Option<PathBuf> {
    let appimage = PathBuf::from(std::env::var_os("APPIMAGE")?);
    let contents = PathBuf::from(std::env::var_os("APPDIR")?);
    if !appimage.is_absolute()
        || !contents.is_absolute()
        || !executable.starts_with(&contents)
        || !super::install::is_appimage(&appimage)
    {
        return None;
    }
    appimage.canonicalize().ok()
}

impl System for LinuxTools {
    fn requirement(&self, _bundle: &Path) -> Result<String, Error> {
        Err(Error::Invalid(
            "a Linux package has no code-signing identity",
        ))
    }
    fn matches(&self, _bundle: &Path, _required: &str) -> Result<(), Error> {
        Err(Error::Invalid(
            "a Linux package has no code-signing identity",
        ))
    }
    fn version(&self, application: &Path) -> Result<Version, Error> {
        match &self.running {
            Some((appimage, version))
                if application
                    .canonicalize()
                    .is_ok_and(|path| path == *appimage) =>
            {
                Ok(*version)
            }
            _ => Err(Error::Invalid(
                "the version of an AppImage is known only to the program that it runs",
            )),
        }
    }
    fn exchange(&self, application: &Path, candidate: &Path) -> Result<(), Error> {
        exchange(application, candidate)
    }
}

/// Exchanges two paths in one step. A file system without that operation gets two renames
/// with a moment in between in which the application path does not exist.
pub fn exchange(application: &Path, candidate: &Path) -> Result<(), Error> {
    match rustix::fs::renameat_with(
        rustix::fs::CWD,
        application,
        rustix::fs::CWD,
        candidate,
        rustix::fs::RenameFlags::EXCHANGE,
    ) {
        Ok(()) => Ok(()),
        Err(rustix::io::Errno::INVAL | rustix::io::Errno::NOSYS | rustix::io::Errno::NOTSUP) => {
            exchange_by_renames(application, candidate)
        }
        Err(error) => Err(std::io::Error::from(error).into()),
    }
}

/// The exchange of a file system without the one-step exchange: the application moves beside
/// the candidate, then the candidate to the place of the application.
pub fn exchange_by_renames(application: &Path, candidate: &Path) -> Result<(), Error> {
    let mut held = candidate.as_os_str().to_owned();
    held.push(".replaced");
    let held = PathBuf::from(held);
    std::fs::rename(application, &held)?;
    if let Err(error) = std::fs::rename(candidate, application) {
        return Err(match std::fs::rename(&held, application) {
            Ok(()) => error.into(),
            Err(_) => Error::ExchangeIncomplete,
        });
    }
    Ok(())
}
