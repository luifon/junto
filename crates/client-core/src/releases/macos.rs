//! macOS system tools for the shared installation decisions.
use super::{Error, install::System};
use crate::updates::Version;
use std::fs;
use std::path::Path;
use std::process::Command;

#[derive(Clone, Copy, Default)]
pub struct MacTools;

impl MacTools {
    pub fn for_running() -> Self {
        Self
    }
}

fn command(program: &str, arguments: &[&std::ffi::OsStr]) -> Result<String, Error> {
    let output = Command::new(program).args(arguments).output()?;
    if !output.status.success() {
        return Err(Error::Invalid("bundle verification failed"));
    }
    String::from_utf8([output.stdout, output.stderr].concat())
        .map_err(|_| Error::Invalid("bundle metadata"))
}

fn plist(bundle: &Path, key: &str) -> Result<String, Error> {
    Ok(command(
        "/usr/libexec/PlistBuddy",
        &[
            "-c".as_ref(),
            format!("Print :{key}").as_ref(),
            bundle.join("Contents/Info.plist").as_os_str(),
        ],
    )?
    .trim()
    .to_owned())
}

pub fn version(bundle: &Path) -> Result<Version, Error> {
    Version::parse(&plist(bundle, "CFBundleShortVersionString")?)
        .map_err(|_| Error::Invalid("bundle version"))
}

fn requirement(bundle: &Path) -> Result<String, Error> {
    if !fs::symlink_metadata(bundle)?.is_dir()
        || plist(bundle, "CFBundleIdentifier")? != "org.junto.app"
    {
        return Err(Error::Invalid("application bundle"));
    }
    command(
        "/usr/bin/codesign",
        &[
            "--verify".as_ref(),
            "--deep".as_ref(),
            "--strict".as_ref(),
            bundle.as_os_str(),
        ],
    )?;
    let text = command(
        "/usr/bin/codesign",
        &["-d".as_ref(), "-r-".as_ref(), bundle.as_os_str()],
    )?;
    text.lines()
        .find_map(|line| line.strip_prefix("designated => "))
        .map(str::to_owned)
        .ok_or(Error::Invalid("ad-hoc or missing certificate requirement"))
}

fn matches(bundle: &Path, required: &str) -> Result<(), Error> {
    if requirement(bundle)? != required {
        return Err(Error::Invalid("different application identity"));
    }
    command(
        "/usr/bin/codesign",
        &[
            "--verify".as_ref(),
            "--deep".as_ref(),
            "--strict".as_ref(),
            "-R".as_ref(),
            format!("={required}").as_ref(),
            bundle.as_os_str(),
        ],
    )?;
    Ok(())
}

impl System for MacTools {
    fn requirement(&self, bundle: &Path) -> Result<String, Error> {
        requirement(bundle)
    }
    fn matches(&self, bundle: &Path, required: &str) -> Result<(), Error> {
        matches(bundle, required)
    }
    fn version(&self, bundle: &Path) -> Result<Version, Error> {
        version(bundle)
    }
    fn exchange(&self, application: &Path, candidate: &Path) -> Result<(), Error> {
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            application,
            rustix::fs::CWD,
            candidate,
            rustix::fs::RenameFlags::EXCHANGE,
        )
        .map_err(std::io::Error::from)?;
        Ok(())
    }
}

pub fn verify_bundle(bundle: &Path) -> Result<Version, Error> {
    super::install::certificate_requirement(requirement(bundle)?)?;
    version(bundle)
}

pub fn verify_release_identity(bundle: &Path) -> Result<(), Error> {
    let expected = include_str!(concat!(env!("OUT_DIR"), "/certificate.sha1")).trim();
    crate::release_configuration::identity(&requirement(bundle)?, expected).map_err(Error::Invalid)
}
