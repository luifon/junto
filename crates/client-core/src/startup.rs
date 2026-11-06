//! Starting the installed desktop application when the account logs in.

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use sha2::{Digest, Sha256};

use crate::app_identity::APP_ID;
use crate::paths::{DataDir, normal_data_dir_of_this_user};

// The tests of the launch agent use paths that Windows does not allow (`<` and `>` in a
// name), so they run on macOS and Linux.
#[cfg(any(target_os = "macos", all(test, unix)))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
mod launch_agent;
#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    NotInstalled,
    SystemVersion,
}

/// The profile that an entry at login starts (ADR 0730). The normal profile has the entry of
/// the application; every other profile has an entry of its own, named by a key of its data
/// directory, that starts the application with that directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Profile {
    Normal,
    Other {
        /// Sixteen hexadecimal digits of the SHA-256 of the canonical path of the directory.
        key: String,
        /// The last part of the path of the directory, which the entry shows.
        name: String,
        directory: std::path::PathBuf,
    },
}

impl Profile {
    /// The profile of a data directory: the normal one when it is the directory of the
    /// user, otherwise another one.
    pub fn of(directory: &Path, normal: Option<&Path>) -> io::Result<Self> {
        let directory = directory.canonicalize()?;
        if normal
            .and_then(|normal| normal.canonicalize().ok())
            .as_ref()
            == Some(&directory)
        {
            return Ok(Self::Normal);
        }
        let digest = Sha256::digest(directory.as_os_str().as_encoded_bytes());
        let key = digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let name = directory
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| "profile".to_owned());
        Ok(Self::Other {
            key,
            name,
            directory,
        })
    }

    /// The name of the entry of the profile at login, as each system names its entries:
    /// the identifier of the application, with the key of a profile other than the normal
    /// one after `separator`.
    pub(crate) fn entry_name(&self, separator: &str) -> String {
        match self {
            Self::Normal => APP_ID.to_owned(),
            Self::Other { key, .. } => format!("{APP_ID}{separator}profile{separator}{key}"),
        }
    }

    /// The arguments after the program that start the application with this profile.
    #[cfg(any(target_os = "macos", test))]
    pub(crate) fn arguments(&self) -> Vec<String> {
        let mut arguments = vec!["--no-activate".to_owned()];
        if let Self::Other { directory, .. } = self {
            arguments.push("--data-dir".to_owned());
            arguments.push(directory.to_string_lossy().into_owned());
        }
        arguments
    }

    /// The name that an entry shows: "Junto", or "Junto (name)" for another profile. Only
    /// the desktop entry of Linux has a name.
    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn shown_name(&self) -> String {
        match self {
            Self::Normal => "Junto".to_owned(),
            Self::Other { name, .. } => format!("Junto ({name})"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Off,
    On,
    RequiresApproval,
    Unavailable(Unavailable),
}

impl Status {
    pub fn requested(self) -> bool {
        matches!(self, Self::On | Self::RequiresApproval)
    }
}

pub trait Startup: Send + Sync + 'static {
    fn status(&self) -> io::Result<Status>;
    fn set(&self, enabled: bool) -> io::Result<Status>;
    fn open_approval(&self) -> io::Result<()> {
        Ok(())
    }
    /// The name of the profile whose entry this is, when it is not the normal profile.
    fn profile_name(&self) -> Option<String> {
        None
    }
}

#[derive(Clone, Default)]
pub struct KeptStartup {
    status: Arc<Mutex<Status>>,
    profile: Option<String>,
}

impl KeptStartup {
    /// The entry of a profile other than the normal one, with its name.
    pub fn of_profile(name: impl Into<String>) -> Self {
        Self {
            profile: Some(name.into()),
            ..Self::default()
        }
    }

    pub fn show(&self, status: Status) {
        *self.status.lock().unwrap_or_else(PoisonError::into_inner) = status;
    }
}

impl Startup for KeptStartup {
    fn status(&self) -> io::Result<Status> {
        Ok(*self.status.lock().unwrap_or_else(PoisonError::into_inner))
    }
    fn set(&self, enabled: bool) -> io::Result<Status> {
        self.show(if enabled { Status::On } else { Status::Off });
        self.status()
    }
    fn profile_name(&self) -> Option<String> {
        self.profile.clone()
    }
}

pub struct SystemStartup {
    directory: DataDir,
    native: Mutex<Option<Arc<dyn Startup>>>,
}

#[cfg(windows)]
pub(crate) fn remove_windows_entry(program: &std::path::Path) -> io::Result<()> {
    windows::remove_for(program)
}

/// Removes the entries at login of this program on Linux: of the normal profile and of every
/// other profile (ADR 0754).
#[cfg(target_os = "linux")]
pub fn remove_linux_entry() -> io::Result<()> {
    linux::remove_every_entry_of_this_program()
}

impl SystemStartup {
    pub fn new(directory: DataDir) -> Self {
        Self {
            directory,
            native: Mutex::new(None),
        }
    }

    fn native(&self) -> io::Result<Arc<dyn Startup>> {
        let mut held = self.native.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(service) = held.as_ref() {
            return Ok(service.clone());
        }
        let normal = normal_data_dir_of_this_user().map_err(io::Error::other)?;
        let profile = Profile::of(self.directory.root(), Some(&normal))?;
        let service: Arc<dyn Startup> = {
            #[cfg(target_os = "linux")]
            {
                Arc::new(linux::running(profile)?)
            }
            #[cfg(target_os = "macos")]
            {
                macos::running(profile)?
            }
            #[cfg(windows)]
            {
                Arc::new(windows::running(profile)?)
            }
        };
        *held = Some(service.clone());
        Ok(service)
    }
}

impl Startup for SystemStartup {
    fn status(&self) -> io::Result<Status> {
        self.native()?.status()
    }
    fn set(&self, enabled: bool) -> io::Result<Status> {
        self.native()?.set(enabled)
    }
    fn open_approval(&self) -> io::Result<()> {
        self.native()?.open_approval()
    }
    fn profile_name(&self) -> Option<String> {
        let normal = normal_data_dir_of_this_user().ok();
        match Profile::of(self.directory.root(), normal.as_deref()) {
            Ok(Profile::Other { name, .. }) => Some(name),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_other_than_the_normal_one_has_an_entry_that_names_it_and_its_directory() {
        let root = tempfile::tempdir().unwrap();
        let normal = root.path().join("normal");
        let other = root.path().join("Work profile");
        std::fs::create_dir_all(&normal).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        assert_eq!(
            Profile::of(&normal, Some(&normal)).unwrap(),
            Profile::Normal
        );
        let profile = Profile::of(&other, Some(&normal)).unwrap();
        let Profile::Other {
            key,
            name,
            directory,
        } = &profile
        else {
            panic!("{profile:?}");
        };
        assert_eq!((key.len(), name.as_str()), (16, "Work profile"));
        assert_eq!(directory, &other.canonicalize().unwrap());
        // The key is the same at each start, and another directory has another key.
        assert_eq!(Profile::of(&other, Some(&normal)).unwrap(), profile);
        let third = root.path().join("third");
        std::fs::create_dir_all(&third).unwrap();
        assert_ne!(Profile::of(&third, Some(&normal)).unwrap(), profile);
        // A directory chosen with `JUNTO_DATA_DIR` is another profile: the normal directory
        // is the default of the system without that variable.
        // The paths are invented paths of Linux, read by the rules of Linux on every system.
        let variables = |name: &str| match name {
            "JUNTO_DATA_DIR" => Some(std::ffi::OsString::from("/srv/work-profile")),
            "HOME" => Some(std::ffi::OsString::from("/srv/accounts/a")),
            _ => None,
        };
        assert_eq!(
            crate::paths::normal_data_dir(crate::paths::System::Linux, variables).unwrap(),
            std::path::Path::new("/srv/accounts/a/.local/share/junto")
        );
        assert_eq!(
            crate::paths::default_data_dir(crate::paths::System::Linux, variables).unwrap(),
            std::path::Path::new("/srv/work-profile")
        );
        // Without a directory of the user every directory is another profile.
        assert!(matches!(
            Profile::of(&normal, None).unwrap(),
            Profile::Other { .. }
        ));
        assert_eq!(Profile::Normal.entry_name("."), APP_ID);
        assert_eq!(profile.entry_name("."), format!("{APP_ID}.profile.{key}"));
        assert_eq!(Profile::Normal.arguments(), ["--no-activate"]);
        assert_eq!(
            profile.arguments(),
            [
                "--no-activate".to_owned(),
                "--data-dir".to_owned(),
                other.canonicalize().unwrap().to_string_lossy().into_owned()
            ]
        );
        assert_eq!(profile.shown_name(), "Junto (Work profile)");
    }
}
