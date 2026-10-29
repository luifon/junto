//! Starting the installed desktop application when the account logs in.

use std::io;
use std::sync::{Arc, Mutex, PoisonError};

use crate::paths::{DataDir, data_dir_of_this_user};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    CustomProfile,
    NotInstalled,
    SystemVersion,
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
}

#[derive(Clone, Default)]
pub struct KeptStartup(Arc<Mutex<Status>>);

impl KeptStartup {
    pub fn show(&self, status: Status) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = status;
    }
}

impl Startup for KeptStartup {
    fn status(&self) -> io::Result<Status> {
        Ok(*self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
    fn set(&self, enabled: bool) -> io::Result<Status> {
        self.show(if enabled { Status::On } else { Status::Off });
        self.status()
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
        let service: Arc<dyn Startup> = {
            #[cfg(target_os = "linux")]
            {
                Arc::new(linux::running()?)
            }
            #[cfg(target_os = "macos")]
            {
                Arc::new(macos::Login)
            }
            #[cfg(windows)]
            {
                Arc::new(windows::running()?)
            }
        };
        *held = Some(service.clone());
        Ok(service)
    }

    fn available(&self) -> io::Result<bool> {
        let default = data_dir_of_this_user().map_err(io::Error::other)?;
        match default.canonicalize() {
            Ok(default) => Ok(default == self.directory.root().canonicalize()?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error),
        }
    }
}

impl Startup for SystemStartup {
    fn status(&self) -> io::Result<Status> {
        if !self.available()? {
            return Ok(Status::Unavailable(Unavailable::CustomProfile));
        }
        self.native()?.status()
    }
    fn set(&self, enabled: bool) -> io::Result<Status> {
        if !self.available()? {
            return Ok(Status::Unavailable(Unavailable::CustomProfile));
        }
        self.native()?.set(enabled)
    }
    fn open_approval(&self) -> io::Result<()> {
        self.native()?.open_approval()
    }
}
