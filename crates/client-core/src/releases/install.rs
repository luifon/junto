//! Installation decisions shared by real platform operations and deterministic tests.
use super::{
    Error, Installer, TrustedKeys, VerifiedArchive, archive, verify_release, write_atomic,
};
use crate::updates::Version;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

pub trait System {
    fn requirement(&self, bundle: &Path) -> Result<String, Error>;
    fn matches(&self, bundle: &Path, required: &str) -> Result<(), Error>;
    fn version(&self, bundle: &Path) -> Result<Version, Error>;
    fn exchange(&self, first: &Path, second: &Path) -> Result<(), Error>;
    fn rename(&self, first: &Path, second: &Path) -> Result<(), Error> {
        Ok(fs::rename(first, second)?)
    }
}
pub fn certificate_requirement(required: String) -> Result<String, Error> {
    if !required.contains("certificate leaf =") {
        return Err(Error::Invalid("ad-hoc or missing certificate requirement"));
    }
    Ok(required)
}

pub struct BundleInstaller<S> {
    application: PathBuf,
    system: S,
    keys: TrustedKeys,
    pin: String,
}
struct Directory {
    path: PathBuf,
    remove: bool,
}
impl Drop for Directory {
    fn drop(&mut self) {
        if self.remove {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}
fn private_directory(path: &Path) -> Result<(), Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_dir() => {
            return Err(Error::Invalid("recovery directory is not a directory"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(path)?;
        }
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn information(path: &Path) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take(super::MAX_METADATA as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > super::MAX_METADATA {
        return Err(Error::Invalid("metadata size"));
    }
    Ok(bytes)
}
pub fn application_for_executable(executable: &Path) -> Result<PathBuf, Error> {
    let bundle = executable
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .ok_or(Error::Invalid("bundle path"))?;
    if bundle.file_name().and_then(|name| name.to_str()) != Some("Junto.app") {
        return Err(Error::Invalid("Junto.app is required"));
    }
    Ok(bundle.to_owned())
}
pub fn install_location(application: &Path) -> Result<(), Error> {
    if application
        .components()
        .any(|part| part.as_os_str() == "AppTranslocation")
    {
        return Err(Error::NotWritable);
    }
    let parent = application.parent().ok_or(Error::NotWritable)?;
    #[cfg(unix)]
    {
        rustix::fs::access(parent, rustix::fs::Access::WRITE_OK).map_err(|_| Error::NotWritable)?;
    }
    #[cfg(not(unix))]
    {
        if fs::metadata(parent)?.permissions().readonly() {
            return Err(Error::NotWritable);
        }
    }
    Ok(())
}
impl<S: System> BundleInstaller<S> {
    pub fn with_system(application: &Path, system: S) -> Result<Self, Error> {
        if application.file_name().and_then(|name| name.to_str()) != Some("Junto.app")
            || !fs::symlink_metadata(application)?.is_dir()
        {
            return Err(Error::Invalid("Junto.app is required"));
        }
        Ok(Self {
            application: application.canonicalize()?,
            system,
            keys: TrustedKeys::compiled()?,
            pin: include_str!(concat!(env!("OUT_DIR"), "/certificate.sha1"))
                .trim()
                .into(),
        })
    }
    pub fn with_trust(mut self, keys: TrustedKeys, pin: String) -> Self {
        self.keys = keys;
        self.pin = pin;
        self
    }
    pub fn location(&self) -> Result<(), Error> {
        install_location(&self.application)
    }
    fn parent(&self) -> Result<&Path, Error> {
        self.application
            .parent()
            .ok_or(Error::Invalid("bundle parent"))
    }
    pub fn previous(&self) -> Result<PathBuf, Error> {
        Ok(self.parent()?.join(".Junto.previous"))
    }
    fn receipt(&self, version: Version) -> Result<PathBuf, Error> {
        Ok(self
            .parent()?
            .join(".Junto.releases")
            .join(format!("{version}.json")))
    }
    fn stage(&self) -> Result<Directory, Error> {
        let mut random = [0; 16];
        getrandom::fill(&mut random).map_err(|_| Error::Invalid("random source"))?;
        let path = self
            .parent()?
            .join(format!(".Junto.stage-{}", super::hex(&random)));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path)?;
        Ok(Directory { path, remove: true })
    }
    fn lock(&self) -> Result<File, Error> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        }
        let file = options
            .open(self.parent()?.join(".Junto.update.lock"))
            .map_err(|_| Error::Locked)?;
        file.try_lock().map_err(|_| Error::Locked)?;
        Ok(file)
    }
    fn matches(&self, bundle: &Path, required: &str) -> Result<(), Error> {
        if certificate_requirement(self.system.requirement(bundle)?)? != required {
            return Err(Error::Invalid("different application identity"));
        }
        self.system.matches(bundle, required)
    }
    fn target(&self, manifest: &super::Manifest) -> Result<Version, Error> {
        if manifest.system != "macos" || manifest.architecture != std::env::consts::ARCH {
            return Err(Error::Invalid("release target"));
        }
        Version::parse(&manifest.version).map_err(|_| Error::Invalid("version"))
    }
    pub fn rollback(&self) -> Result<(), Error> {
        let _lock = self.lock()?;
        let previous = self.previous()?;
        let old_wire = information(&previous.join("release.json"))?;
        let old = verify_release(&old_wire, &self.keys)?;
        let installed =
            verify_release(&information(&previous.join("installed.json"))?, &self.keys)?;
        let old_version = self.target(&old)?;
        let installed_version = self.target(&installed)?;
        if old_version >= installed_version {
            return Err(Error::Invalid("no older rollback version"));
        }
        if let Ok(current) = self.system.version(&self.application)
            && current != installed_version
        {
            return Err(Error::Invalid(
                "rollback does not describe the installed version",
            ));
        }
        let mut file = File::open(previous.join("archive.junto"))?;
        super::verify_contents(&mut file, &old, &self.keys)?;
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(0))?;
        let stage = self.stage()?;
        archive::unpack(&mut file, &stage.path.join("unpacked"))?;
        let candidate = stage.path.join("unpacked/Junto.app");
        if fs::read_dir(stage.path.join("unpacked"))?.count() != 1
            || self.system.version(&candidate)? != old_version
        {
            return Err(Error::Invalid("rollback contents or version"));
        }
        archive::verify_modes(&candidate)?;
        let required = certificate_requirement(self.system.requirement(&candidate)?)?;
        crate::release_configuration::identity(&required, &self.pin).map_err(Error::Invalid)?;
        self.matches(&candidate, &required)?;
        self.system.exchange(&self.application, &candidate)
    }
}
impl<S: System> Installer for BundleInstaller<S> {
    fn install(&self, archive: &mut VerifiedArchive) -> Result<PathBuf, Error> {
        let _lock = self.lock()?;
        let mut stage = self.stage()?;
        archive.unpack(&stage.path.join("unpacked"))?;
        let required = certificate_requirement(self.system.requirement(&self.application)?)?;
        let offered = self.target(&archive.manifest)?;
        let current = self.system.version(&self.application)?;
        if offered <= current {
            return Err(Error::NotNewer);
        }
        let candidate = stage.path.join("unpacked/Junto.app");
        if fs::read_dir(stage.path.join("unpacked"))?.count() != 1
            || self.system.version(&candidate)? != offered
        {
            return Err(Error::Invalid("bundle contents or version"));
        }
        archive::verify_modes(&candidate)?;
        self.matches(&candidate, &required)?;
        let old_wire = information(&self.receipt(current)?).map_err(|_| {
            Error::Invalid(
                "installed release proof is missing; keep the complete installation folder",
            )
        })?;
        let old_manifest = verify_release(&old_wire, &archive.keys)?;
        if self.target(&old_manifest)? != current {
            return Err(Error::Invalid("installed release proof version"));
        }
        let backup = stage.path.join("backup");
        private_directory(&backup)?;
        let old_archive = backup.join("archive.junto");
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&old_archive)?;
        archive::pack_bundle(&self.application, &mut file)?;
        file.sync_all()?;
        super::verify_archive(&old_archive, &old_manifest, &archive.keys)?;
        write_atomic(&backup.join("release.json"), &old_wire)?;
        write_atomic(&backup.join("installed.json"), &archive.wire)?;
        fs::copy(
            self.application.join("Contents/MacOS/junto-release"),
            backup.join("recover"),
        )?;
        private_directory(&self.parent()?.join(".Junto.releases"))?;
        write_atomic(&self.receipt(offered)?, &archive.wire)?;
        let previous = self.previous()?;
        let obsolete = stage.path.join("obsolete");
        if fs::symlink_metadata(&previous).is_ok() {
            self.system.rename(&previous, &obsolete)?;
        }
        if let Err(error) = self.system.rename(&backup, &previous) {
            if fs::symlink_metadata(&obsolete).is_ok()
                && let Err(restore) = self.system.rename(&obsolete, &previous)
            {
                stage.remove = false;
                return Err(restore);
            }
            return Err(error);
        }
        if let Err(error) = self.system.exchange(&self.application, &candidate) {
            if let Err(restore) = self.system.rename(&previous, &backup) {
                stage.remove = false;
                return Err(restore);
            }
            if fs::symlink_metadata(&obsolete).is_ok()
                && let Err(restore) = self.system.rename(&obsolete, &previous)
            {
                stage.remove = false;
                return Err(restore);
            }
            return Err(error);
        }
        Ok(previous)
    }
}
#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
