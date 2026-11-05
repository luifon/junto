#![allow(unsafe_code)]

use super::{Profile, Startup, Status, Unavailable};
use objc2_service_management::{SMAppService, SMAppServiceStatus};
use std::io;

fn installed() -> bool {
    let bundle = objc2_foundation::NSBundle::mainBundle();
    bundle
        .bundleIdentifier()
        .is_some_and(|identifier| identifier.to_string() == "org.junto.app")
        && bundle.bundlePath().to_string().ends_with(".app")
}

fn status_of(status: SMAppServiceStatus) -> Status {
    match status {
        SMAppServiceStatus::Enabled => Status::On,
        SMAppServiceStatus::RequiresApproval => Status::RequiresApproval,
        SMAppServiceStatus::NotRegistered => Status::Off,
        _ => Status::Unavailable(Unavailable::NotInstalled),
    }
}

pub(super) fn status() -> Status {
    if objc2::runtime::AnyClass::get(c"SMAppService").is_none() {
        return Status::Unavailable(Unavailable::SystemVersion);
    }
    if !installed() {
        return Status::Unavailable(Unavailable::NotInstalled);
    }
    // SMAppService is not restricted to the main thread; its objects stay inside this call.
    unsafe { status_of(SMAppService::mainAppService().status()) }
}

pub(super) fn set(enabled: bool) -> io::Result<Status> {
    if objc2::runtime::AnyClass::get(c"SMAppService").is_none() {
        return Ok(Status::Unavailable(Unavailable::SystemVersion));
    }
    if !installed() {
        return Ok(Status::Unavailable(Unavailable::NotInstalled));
    }
    unsafe {
        let service = SMAppService::mainAppService();
        let before = status_of(service.status());
        if before.requested() == enabled {
            return Ok(before);
        }
        let result = if enabled {
            service.registerAndReturnError()
        } else {
            service.unregisterAndReturnError()
        };
        let status = status_of(service.status());
        if let Err(error) = result
            && !(enabled && status == Status::RequiresApproval)
        {
            return Err(io::Error::other(error.to_string()));
        }
        Ok(status)
    }
}

pub(super) fn open_approval() {
    if objc2::runtime::AnyClass::get(c"SMAppService").is_some() {
        unsafe {
            SMAppService::openSystemSettingsLoginItems();
        }
    }
}

/// The entry at login of a profile: the login service of the system for the normal profile,
/// a launch agent of the account for another one (`launch_agent`). Both need the installed
/// bundle.
pub(super) fn running(profile: Profile) -> io::Result<std::sync::Arc<dyn Startup>> {
    Ok(match profile {
        Profile::Normal => std::sync::Arc::new(Login),
        Profile::Other { .. } => {
            let path = super::launch_agent::directory()?
                .join(format!("{}.plist", profile.entry_name(".")));
            std::sync::Arc::new(Installed(super::launch_agent::Agent {
                path,
                program: std::env::current_exe()?,
                profile,
            }))
        }
    })
}

/// A launch agent of a profile, which only an installed bundle writes.
struct Installed(super::launch_agent::Agent);

impl Startup for Installed {
    fn status(&self) -> io::Result<Status> {
        if !installed() {
            return Ok(Status::Unavailable(Unavailable::NotInstalled));
        }
        self.0.status()
    }
    fn set(&self, enabled: bool) -> io::Result<Status> {
        if !installed() {
            return Ok(Status::Unavailable(Unavailable::NotInstalled));
        }
        self.0.set(enabled)
    }
}

pub(super) struct Login;
impl Startup for Login {
    fn status(&self) -> io::Result<Status> {
        Ok(status())
    }
    fn set(&self, enabled: bool) -> io::Result<Status> {
        set(enabled)
    }
    fn open_approval(&self) -> io::Result<()> {
        open_approval();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn the_system_status_keeps_the_approval_requirement() {
        assert_eq!(status_of(SMAppServiceStatus::Enabled), Status::On);
        assert_eq!(status_of(SMAppServiceStatus::NotRegistered), Status::Off);
        assert_eq!(
            status_of(SMAppServiceStatus::RequiresApproval),
            Status::RequiresApproval
        );
        assert_eq!(
            status_of(SMAppServiceStatus::NotFound),
            Status::Unavailable(Unavailable::NotInstalled)
        );
    }
}
