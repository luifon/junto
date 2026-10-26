//! Installation of a verified release, the record of the installed and the previous release,
//! and rollback (ADR 0726). The decisions are shared by every package; the platform modules
//! supply the operations of their system.
use super::{
    Error, Target, TrustedKeys, VerifiedArchive, archive, verify_release, verify_version,
    write_atomic,
};
use crate::updates::Version;
use serde::Deserialize;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

/// The file that names the version and target of a directory package or an AppImage.
pub const DESCRIPTION: &str = "junto-package.json";
/// The end of the name of the directory beside the application that holds the lock, the
/// record and the staging directories of its updates: `.<application name>.junto-update`.
pub const UPDATE_DIRECTORY: &str = ".junto-update";

/// The form of an installed copy of Junto.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Package {
    /// `Junto.app` on macOS.
    Bundle,
    /// A directory with the program and its files: the tarball on Linux, the ZIP on Windows.
    Directory,
    /// One AppImage file on Linux.
    AppImage,
}

impl Package {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bundle => "bundle",
            Self::Directory => "directory",
            Self::AppImage => "appimage",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        [Self::Bundle, Self::Directory, Self::AppImage]
            .into_iter()
            .find(|package| package.name() == text)
    }

    /// The package of the installation that this process runs from, else the usual
    /// package of its system.
    pub fn running() -> Self {
        running()
            .map(|(package, _)| package)
            .unwrap_or(if cfg!(target_os = "macos") {
                Self::Bundle
            } else {
                Self::Directory
            })
    }
}

/// Where the files of a package are in its archive.
struct Layout {
    /// The entry that is installed: it replaces the application.
    application: &'static str,
    /// The recovery program, kept beside the record of a previous release.
    recovery: &'static str,
    /// The description file, for the packages whose system does not state a version.
    description: Option<&'static str>,
    /// Every top-level entry of the archive.
    entries: &'static [&'static str],
}

fn layout(target: &Target) -> Result<Layout, Error> {
    Ok(
        match (Package::parse(&target.package), target.system.as_str()) {
            (Some(Package::Bundle), "macos") => Layout {
                application: "Junto.app",
                recovery: "Junto.app/Contents/MacOS/junto-recover",
                description: None,
                entries: &["Junto.app"],
            },
            (Some(Package::Directory), "linux") => Layout {
                application: "junto",
                recovery: "junto/junto-recover",
                description: Some("junto/junto-package.json"),
                entries: &["junto"],
            },
            (Some(Package::Directory), "windows") => Layout {
                application: "Junto",
                recovery: "Junto/junto-recover.exe",
                description: Some("Junto/junto-package.json"),
                entries: &["Junto"],
            },
            (Some(Package::AppImage), "linux") => Layout {
                application: "Junto.AppImage",
                recovery: "junto-recover",
                description: Some("junto-package.json"),
                entries: &["Junto.AppImage", "junto-package.json", "junto-recover"],
            },
            _ => return Err(Error::Invalid("package of another system")),
        },
    )
}

/// The operations of the system that the shared decisions call.
pub trait System {
    /// The designated requirement of a macOS bundle. Packages of other systems have no
    /// code-signing identity, and the installer does not ask for one.
    fn requirement(&self, bundle: &Path) -> Result<String, Error>;
    fn matches(&self, bundle: &Path, required: &str) -> Result<(), Error>;
    /// The version of a macOS bundle, or of the AppImage that this process runs from.
    fn version(&self, application: &Path) -> Result<Version, Error>;
    /// Puts `candidate` at the path of `application`. The replaced application is then
    /// inside the directory that held the candidate. A failure that leaves neither in
    /// place returns `Error::ExchangeIncomplete`.
    fn exchange(&self, application: &Path, candidate: &Path) -> Result<(), Error>;
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Description {
    format: u32,
    version: String,
    system: String,
    package: String,
    architecture: String,
}

/// Reads a description file: its version, which must be for `target`.
fn described_version(path: &Path, target: &Target) -> Result<Version, Error> {
    let description: Description = serde_json::from_slice(&information(path)?)?;
    if description.format != 1
        || description.system != target.system
        || description.package != target.package
        || description.architecture != target.architecture
    {
        return Err(Error::Invalid("package description"));
    }
    Version::parse(&description.version).map_err(|_| Error::Invalid("package description"))
}

/// Checks the directory that a release is packed from: it holds exactly the entries of the
/// package and, where the package has a description, states `version` for `target`.
pub fn verify_source(target: &Target, source: &Path, version: Version) -> Result<(), Error> {
    let layout = layout(target)?;
    let mut names = fs::read_dir(source)?
        .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
        .collect::<Result<Vec<_>, Error>>()?;
    names.sort();
    let mut expected = layout.entries.to_vec();
    expected.sort_unstable();
    if names != expected {
        return Err(Error::Invalid("package contents"));
    }
    if let Some(description) = layout.description
        && described_version(&source.join(description), target)? != version
    {
        return Err(Error::Invalid("package and release versions differ"));
    }
    Ok(())
}

/// Whether a file starts as an AppImage of type 2: an ELF program with the AppImage mark.
pub fn is_appimage(path: &Path) -> bool {
    let mut start = [0; 11];
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
        && File::open(path)
            .and_then(|mut file| file.read_exact(&mut start))
            .is_ok()
        && start[..4] == *b"\x7fELF"
        && start[8..] == *b"AI\x02"
}

/// The installation that this process runs from.
pub fn running() -> Result<(Package, PathBuf), Error> {
    let executable = std::env::current_exe()?;
    #[cfg(target_os = "macos")]
    {
        Ok((Package::Bundle, application_for_executable(&executable)?))
    }
    #[cfg(not(target_os = "macos"))]
    {
        #[cfg(target_os = "linux")]
        if let Some(appimage) = super::linux::running_appimage(&executable) {
            return Ok((Package::AppImage, appimage));
        }
        let directory = executable.parent().ok_or(Error::NotInstalled)?;
        if fs::symlink_metadata(directory.join(DESCRIPTION)).is_ok_and(|file| file.is_file()) {
            return Ok((Package::Directory, directory.to_owned()));
        }
        Err(Error::NotInstalled)
    }
}

/// The package name that a Debian package writes into the description of its directory
/// (ADR 0728). The system package manager installs and updates it; the client does not.
pub const SYSTEM_PACKAGE: &str = "deb";

/// Whether the description file of a directory names the Debian package.
pub fn is_system_package(directory: &Path) -> bool {
    #[derive(Deserialize)]
    struct Named {
        package: String,
    }
    information(&directory.join(DESCRIPTION))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Named>(&bytes).ok())
        .is_some_and(|named| named.package == SYSTEM_PACKAGE)
}

/// Whether this process runs from a directory that the Debian package installed.
pub fn running_from_system_package() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|executable| executable.parent().map(Path::to_owned))
        .is_some_and(|directory| is_system_package(&directory))
}

/// The package of the installation at a path that a person names.
pub fn package_at(application: &Path) -> Result<Package, Error> {
    let metadata = fs::symlink_metadata(application)?;
    if metadata.is_dir()
        && application.file_name().and_then(|name| name.to_str()) == Some("Junto.app")
    {
        Ok(Package::Bundle)
    } else if metadata.is_dir()
        && fs::symlink_metadata(application.join(DESCRIPTION)).is_ok_and(|file| file.is_file())
    {
        Ok(Package::Directory)
    } else if is_appimage(application) {
        Ok(Package::AppImage)
    } else {
        Err(Error::NotInstalled)
    }
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

/// Whether an update can write beside the application. Nothing is written.
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
    // The read-only attribute of a Windows directory does not stop writes into it. The first
    // write of an update reports a location that the account cannot write.
    #[cfg(not(unix))]
    let _ = parent;
    Ok(())
}

/// What happened to the release that the update replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Previous {
    /// It is kept as an archive; the recovery program can restore it.
    Kept(Version),
    /// It is not kept, for the stated reason. The update was still installed.
    Missing(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub version: Version,
    pub previous: Previous,
}

/// The release of the installed version as the source gave it for an update, when the
/// record beside the application does not hold it.
pub enum Fetched {
    NotNeeded,
    Archive(Box<VerifiedArchive>),
    Unavailable(String),
}

pub struct Installer<S> {
    application: PathBuf,
    package: Package,
    target: Target,
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
            return Err(Error::Invalid("update directory is not a directory"));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            super::create_private_directory(path)?;
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

fn random_name(prefix: &str) -> Result<String, Error> {
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|_| Error::Invalid("random source"))?;
    Ok(format!("{prefix}{}", super::hex(&random)))
}

/// Gives `destination` the content of the archive `source` of the record. On Unix a second
/// name of the file, or a copy where a link fails. On Windows a copy: after a hard link the
/// system can keep the file open for up to a second, and while a file is open, a directory
/// that holds it cannot be renamed, which the update does next (ADR 0711, ADR 0726).
fn keep_archive(source: &Path, destination: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    if fs::hard_link(source, destination).is_ok() {
        return Ok(());
    }
    fs::copy(source, destination).map(|_| ())
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

impl<S: System> Installer<S> {
    pub fn with_system(application: &Path, package: Package, system: S) -> Result<Self, Error> {
        let metadata = fs::symlink_metadata(application)?;
        let shape = match package {
            Package::Bundle => {
                metadata.is_dir()
                    && application.file_name().and_then(|name| name.to_str()) == Some("Junto.app")
            }
            Package::Directory => metadata.is_dir() && application.join(DESCRIPTION).is_file(),
            Package::AppImage => is_appimage(application),
        };
        if !shape {
            return Err(Error::NotInstalled);
        }
        Ok(Self {
            application: application.canonicalize()?,
            package,
            target: Target::this_build(package),
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
    #[cfg(test)]
    fn with_target(mut self, target: Target) -> Self {
        self.target = target;
        self
    }
    pub fn target(&self) -> &Target {
        &self.target
    }
    pub fn application(&self) -> &Path {
        &self.application
    }
    pub fn location(&self) -> Result<(), Error> {
        install_location(&self.application)
    }
    fn parent(&self) -> Result<&Path, Error> {
        self.application
            .parent()
            .ok_or(Error::Invalid("application parent"))
    }
    /// The directory of the updates of this application: `.<name>.junto-update` beside it,
    /// so that two installations in one directory do not share it.
    pub fn directory(&self) -> Result<PathBuf, Error> {
        let name = self
            .application
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Error::Invalid("application name"))?;
        Ok(self.parent()?.join(format!(".{name}{UPDATE_DIRECTORY}")))
    }
    /// The record of the installed release and of the previous release.
    pub fn record(&self) -> Result<PathBuf, Error> {
        Ok(self.directory()?.join("record"))
    }
    fn prepare(&self) -> Result<PathBuf, Error> {
        let directory = self.directory()?;
        private_directory(&directory).map_err(|error| match error {
            Error::Io(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                Error::NotWritable
            }
            error => error,
        })?;
        Ok(directory)
    }
    /// A new path for a download beside the application, on the file system of the
    /// application, so that the installer can move the file into the record.
    pub fn download_path(&self) -> Result<PathBuf, Error> {
        Ok(self.prepare()?.join(random_name("download-")?))
    }
    fn stage(&self) -> Result<Directory, Error> {
        let path = self.directory()?.join(random_name("stage-")?);
        super::create_private_directory(&path)?;
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
            .open(self.directory()?.join("lock"))
            .map_err(|_| Error::Locked)?;
        file.try_lock().map_err(|_| Error::Locked)?;
        Ok(file)
    }
    /// Removes a staging directory after a finished operation. A file that the system does
    /// not let go (a running program on Windows) keeps it; a mark beside it lets the next
    /// operation remove it. Staging without a mark can hold the only recovery copy.
    fn finish(&self, mut stage: Directory) {
        stage.remove = false;
        let Some(name) = stage.path.file_name().and_then(|name| name.to_str()) else {
            return;
        };
        let Ok(directory) = self.directory() else {
            return;
        };
        let mark = directory.join(format!("finished-{name}"));
        if fs::write(&mark, b"").is_ok() && fs::remove_dir_all(&stage.path).is_ok() {
            let _ = fs::remove_file(mark);
        }
    }
    fn remove_finished(&self) -> Result<(), Error> {
        let directory = self.directory()?;
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(stage) = name
                .to_str()
                .and_then(|name| name.strip_prefix("finished-"))
            else {
                continue;
            };
            let removed = !stage.starts_with("stage-")
                || match fs::remove_dir_all(directory.join(stage)) {
                    Ok(()) => true,
                    Err(error) => error.kind() == std::io::ErrorKind::NotFound,
                };
            if removed {
                let _ = fs::remove_file(entry.path());
            }
        }
        Ok(())
    }

    /// The version of the installed application.
    pub fn installed_version(&self) -> Result<Version, Error> {
        match self.package {
            Package::Bundle | Package::AppImage => self.system.version(&self.application),
            Package::Directory => {
                described_version(&self.application.join(DESCRIPTION), &self.target)
            }
        }
    }

    /// The directory of the record that holds the release of `version`, when its release
    /// information and its archive verify. A damaged archive is then downloaded again.
    fn local_record(&self, version: Version) -> Option<PathBuf> {
        let record = self.record().ok()?;
        ["installed", "previous"]
            .into_iter()
            .map(|name| record.join(name))
            .find(|directory| {
                information(&directory.join("release.json"))
                    .and_then(|bytes| verify_version(&bytes, &self.keys, version, &self.target))
                    .and_then(|manifest| {
                        super::verify_archive(
                            &directory.join("archive.junto"),
                            &manifest,
                            &self.keys,
                        )
                    })
                    .is_ok()
            })
    }

    /// Whether the record beside the application holds the release of `version`, so that an
    /// update does not need to download it.
    pub fn has_record_of(&self, version: Version) -> bool {
        self.local_record(version).is_some()
    }

    fn recovery_name(&self) -> &'static str {
        if self.target.system == "windows" {
            "recover.exe"
        } else {
            "recover"
        }
    }

    /// The candidate in an extracted archive, after the checks of its shape, version, modes
    /// and, for a bundle, its code-signing identity: the designated requirement must be the
    /// product identifier with the compiled certificate pin, and the platform signature must
    /// verify. The installed bundle is not asked: a file added to it or a damaged signature
    /// does not stop an update.
    fn candidate(&self, unpacked: &Path, version: Version) -> Result<PathBuf, Error> {
        let layout = layout(&self.target)?;
        let mut names = fs::read_dir(unpacked)?
            .map(|entry| Ok(entry?.file_name().to_string_lossy().into_owned()))
            .collect::<Result<Vec<_>, Error>>()?;
        names.sort();
        let mut expected = layout.entries.to_vec();
        expected.sort_unstable();
        if names != expected {
            return Err(Error::Invalid("package contents"));
        }
        let candidate = unpacked.join(layout.application);
        let metadata = fs::symlink_metadata(&candidate)?;
        let shape = match self.package {
            Package::Bundle | Package::Directory => metadata.is_dir(),
            Package::AppImage => is_appimage(&candidate),
        };
        let stated = match layout.description {
            Some(description) => described_version(&unpacked.join(description), &self.target)?,
            None => self.system.version(&candidate)?,
        };
        if !shape || stated != version {
            return Err(Error::Invalid("package contents or version"));
        }
        for entry in layout.entries {
            archive::verify_modes(&unpacked.join(entry))?;
        }
        if self.package == Package::Bundle {
            let required = certificate_requirement(self.system.requirement(&candidate)?)?;
            crate::release_configuration::identity(&required, &self.pin)
                .map_err(|_| Error::Invalid("different application identity"))?;
            self.system.matches(&candidate, &required)?;
        }
        Ok(candidate)
    }

    /// Prepares the record of the release that the update replaces in `destination`: its
    /// archive, its release information and the recovery program from that archive.
    fn previous_record(
        &self,
        current: Version,
        fetched: Fetched,
        destination: &Path,
    ) -> Result<(), String> {
        let result = (|| {
            private_directory(destination)?;
            let archive_path = destination.join("archive.junto");
            let information_path = destination.join("release.json");
            if let Some(local) = self.local_record(current) {
                keep_archive(&local.join("archive.junto"), &archive_path)?;
                fs::copy(local.join("release.json"), &information_path)?;
            } else {
                match fetched {
                    Fetched::Archive(mut fetched) => {
                        write_atomic(&information_path, fetched.information())?;
                        fetched.keep(&archive_path)?;
                    }
                    Fetched::Unavailable(reason) => return Err(Error::Unavailable(reason)),
                    Fetched::NotNeeded => {
                        return Err(Error::Invalid("no release of the installed version"));
                    }
                }
            }
            let manifest = verify_version(
                &information(&information_path)?,
                &self.keys,
                current,
                &self.target,
            )?;
            super::verify_archive(&archive_path, &manifest, &self.keys)?;
            archive::extract_file(
                &mut File::open(&archive_path)?,
                layout(&self.target)?.recovery,
                &destination.join(self.recovery_name()),
            )
        })();
        result.map_err(|error| {
            let _ = fs::remove_dir_all(destination);
            error.to_string()
        })
    }

    /// Moves the new record in and the candidate to the place of the application. Each
    /// failure before the end restores what it moved.
    fn commit(&self, stage: &mut Directory, record: &Path, candidate: &Path) -> Result<(), Error> {
        let current = self.record()?;
        let obsolete = stage.path.join("obsolete");
        if exists(&current) {
            self.system.rename(&current, &obsolete)?;
        }
        if let Err(error) = self.system.rename(record, &current) {
            if exists(&obsolete)
                && let Err(restore) = self.system.rename(&obsolete, &current)
            {
                stage.remove = false;
                return Err(restore);
            }
            return Err(error);
        }
        if let Err(error) = self.system.exchange(&self.application, candidate) {
            if matches!(error, Error::ExchangeIncomplete) {
                stage.remove = false;
                return Err(error);
            }
            if let Err(restore) = self.system.rename(&current, record) {
                stage.remove = false;
                return Err(restore);
            }
            if exists(&obsolete)
                && let Err(restore) = self.system.rename(&obsolete, &current)
            {
                stage.remove = false;
                return Err(restore);
            }
            return Err(error);
        }
        Ok(())
    }

    /// Installs a verified release over the application. `fetched` is the release of the
    /// installed version from the source, for the case that the record does not hold it.
    pub fn install(
        &self,
        archive: &mut VerifiedArchive,
        fetched: Fetched,
    ) -> Result<Installed, Error> {
        self.prepare()?;
        let _lock = self.lock()?;
        self.remove_finished()?;
        self.target.check(&archive.manifest)?;
        let offered =
            Version::parse(&archive.manifest.version).map_err(|_| Error::Invalid("version"))?;
        let current = self.installed_version()?;
        if offered <= current {
            return Err(Error::NotNewer);
        }
        let mut stage = self.stage()?;
        let unpacked = stage.path.join("unpacked");
        archive.unpack(&unpacked)?;
        let candidate = self.candidate(&unpacked, offered)?;
        let record = stage.path.join("record");
        private_directory(&record)?;
        let installed = record.join("installed");
        private_directory(&installed)?;
        write_atomic(&installed.join("release.json"), &archive.wire)?;
        archive.keep(&installed.join("archive.junto"))?;
        let previous = match self.previous_record(current, fetched, &record.join("previous")) {
            Ok(()) => Previous::Kept(current),
            Err(reason) => Previous::Missing(reason),
        };
        self.commit(&mut stage, &record, &candidate)?;
        self.finish(stage);
        Ok(Installed {
            version: offered,
            previous,
        })
    }

    /// Restores the previous release from the record. The current application is neither
    /// run nor required to be intact.
    pub fn rollback(&self) -> Result<Version, Error> {
        self.prepare()?;
        let _lock = self.lock()?;
        self.remove_finished()?;
        let record = self.record()?;
        let old = verify_release(
            &information(&record.join("previous/release.json"))?,
            &self.keys,
        )?;
        let installed = verify_release(
            &information(&record.join("installed/release.json"))?,
            &self.keys,
        )?;
        self.target.check(&old)?;
        self.target.check(&installed)?;
        let old_version = Version::parse(&old.version).map_err(|_| Error::Invalid("version"))?;
        let installed_version =
            Version::parse(&installed.version).map_err(|_| Error::Invalid("version"))?;
        if old_version >= installed_version {
            return Err(Error::Invalid("no older rollback version"));
        }
        // A damaged application may not state its version; the checks below still require
        // an authentic older release (ADR 0726).
        if let Ok(current) = self.installed_version()
            && current != installed_version
        {
            return Err(Error::Invalid(
                "rollback does not describe the installed version",
            ));
        }
        let mut file = File::open(record.join("previous/archive.junto"))?;
        super::verify_contents(&mut file, &old, &self.keys)?;
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(0))?;
        let mut stage = self.stage()?;
        let unpacked = stage.path.join("unpacked");
        archive::unpack(&mut file, &unpacked)?;
        drop(file);
        let candidate = self.candidate(&unpacked, old_version)?;
        if let Err(error) = self.system.exchange(&self.application, &candidate) {
            if matches!(error, Error::ExchangeIncomplete) {
                stage.remove = false;
            }
            return Err(error);
        }
        // The record now describes what is installed: the restored release is the installed
        // one, and there is no older release to restore. The record of the replaced release
        // goes with the staging directory. When a move fails, the record keeps both releases
        // and the version check refuses a second rollback where the version can be read.
        if self
            .system
            .rename(&record.join("installed"), &stage.path.join("replaced"))
            .is_ok()
        {
            let _ = self
                .system
                .rename(&record.join("previous"), &record.join("installed"));
        }
        self.finish(stage);
        Ok(old_version)
    }
}

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
