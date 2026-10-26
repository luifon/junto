//! The Windows setup program (ADR 0728): its file, which is the program `junto-installer`
//! followed by the archive of the Windows folder, and the installation and the removal of
//! Junto for one account without administrator rights. The decisions are portable; the
//! operations of Windows are behind [`SetupSystem`].

use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::{Error, archive, install};
use crate::updates::Version;

/// The end of a setup program: the length of the archive (eight bytes, big-endian), its
/// SHA-256, and this mark.
const MARK: &[u8; 16] = b"JUNTO-SETUP-1\0\0\0";
const TRAILER: u64 = 8 + 32 + 16;

/// The name of the installed folder and of the program in it (ADR 0755).
pub const FOLDER: &str = "Junto";
pub const PROGRAM: &str = "junto.exe";
/// The uninstall program of the package: `junto-installer` without an archive.
pub const UNINSTALLER: &str = "uninstall.exe";
/// The Start menu entry.
pub const SHORTCUT: &str = "Junto.lnk";

/// Writes a setup program: `installer`, then the archive of the folder `Junto` of
/// `package`, then the trailer. `output` must not exist.
pub fn write(installer: &Path, package: &Path, output: &Path) -> Result<(), Error> {
    let stub = fs::read(installer)?;
    if find_trailer(&stub).is_some() {
        return Err(Error::Invalid("the installer already holds an archive"));
    }
    install::verify_source(
        &super::Target::new("windows", "directory", "x86_64")?,
        package,
        described(&package.join(FOLDER))?,
    )?;
    let mut archive = Vec::new();
    archive::pack(package, &mut archive)?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    let written = (|| -> std::io::Result<()> {
        file.write_all(&stub)?;
        file.write_all(&archive)?;
        file.write_all(&(archive.len() as u64).to_be_bytes())?;
        file.write_all(&Sha256::digest(&archive))?;
        file.write_all(MARK)?;
        file.sync_all()
    })();
    if let Err(error) = written {
        drop(file);
        let _ = fs::remove_file(output);
        return Err(error.into());
    }
    Ok(())
}

fn find_trailer(bytes: &[u8]) -> Option<(u64, [u8; 32])> {
    let end = bytes.len().checked_sub(TRAILER as usize)?;
    let trailer = &bytes[end..];
    if &trailer[40..] != MARK {
        return None;
    }
    let mut length = [0; 8];
    length.copy_from_slice(&trailer[..8]);
    let mut digest = [0; 32];
    digest.copy_from_slice(&trailer[8..40]);
    Some((u64::from_be_bytes(length), digest))
}

/// The archive that a setup program holds, after the check of its length and SHA-256: a
/// download that ended early or changed is refused before anything is written.
pub fn payload(setup: &Path) -> Result<Vec<u8>, Error> {
    let mut file = File::open(setup)?;
    let size = file.metadata()?.len();
    if size < TRAILER {
        return Err(Error::Invalid("the setup program holds no archive"));
    }
    file.seek(SeekFrom::Start(size - TRAILER))?;
    let mut trailer = vec![0; TRAILER as usize];
    file.read_exact(&mut trailer)?;
    let (length, digest) =
        find_trailer(&trailer).ok_or(Error::Invalid("the setup program holds no archive"))?;
    if length > super::MAX_ARCHIVE || length > size - TRAILER {
        return Err(Error::Invalid("archive size"));
    }
    file.seek(SeekFrom::Start(size - TRAILER - length))?;
    let mut archive = vec![0; length as usize];
    file.read_exact(&mut archive)?;
    if Sha256::digest(&archive).as_slice() != digest {
        return Err(Error::Invalid("the archive of the setup program is damaged"));
    }
    Ok(archive)
}

/// Whether a program file is a setup program, which holds an archive.
pub fn holds_payload(program: &Path) -> bool {
    let read = || -> std::io::Result<bool> {
        let mut file = File::open(program)?;
        let size = file.metadata()?.len();
        if size < TRAILER {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(size - TRAILER))?;
        let mut trailer = vec![0; TRAILER as usize];
        file.read_exact(&mut trailer)?;
        Ok(find_trailer(&trailer).is_some())
    };
    read().unwrap_or(false)
}

fn described(folder: &Path) -> Result<Version, Error> {
    #[derive(serde::Deserialize)]
    struct Description {
        format: u32,
        version: String,
        system: String,
        package: String,
        architecture: String,
    }
    let bytes = fs::read(folder.join(install::DESCRIPTION))?;
    let description: Description = serde_json::from_slice(&bytes)?;
    if description.format != 1
        || description.system != "windows"
        || description.package != "directory"
        || description.architecture != "x86_64"
    {
        return Err(Error::Invalid("package description"));
    }
    Version::parse(&description.version).map_err(|_| Error::Invalid("package description"))
}

/// The places of an installation for one account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Places {
    /// The folder of the program: `%LOCALAPPDATA%\Programs\Junto` for the setup program.
    pub folder: PathBuf,
    /// The data directory, `%LOCALAPPDATA%\Junto` (ADR 0710).
    pub data: PathBuf,
    /// `%APPDATA%\Microsoft\Windows\Start Menu\Programs`.
    pub start_menu: PathBuf,
}

impl Places {
    /// The places of the account from `LOCALAPPDATA` and `APPDATA`.
    pub fn of_account(local: &Path, roaming: &Path) -> Self {
        Self::of_folder(&local.join("Programs").join(FOLDER), local, roaming)
    }

    /// The places of the installation in `folder`, where a person extracted the ZIP file or
    /// the setup program installed it.
    pub fn of_folder(folder: &Path, local: &Path, roaming: &Path) -> Self {
        Self {
            folder: folder.to_owned(),
            data: local.join("Junto"),
            start_menu: ["Microsoft", "Windows", "Start Menu", "Programs"]
                .iter()
                .fold(roaming.to_owned(), |path, part| path.join(part)),
        }
    }

    pub fn folder(&self) -> PathBuf {
        self.folder.clone()
    }

    /// The folder that holds the folder of the program.
    pub fn programs(&self) -> PathBuf {
        self.folder
            .parent()
            .map_or_else(|| self.folder.clone(), Path::to_owned)
    }

    pub fn program(&self) -> PathBuf {
        self.folder.join(PROGRAM)
    }

    /// The directory of the updates of the folder: `.<name>.junto-update` beside it
    /// (ADR 0726).
    pub fn updates(&self) -> PathBuf {
        let name = self
            .folder
            .file_name()
            .map_or_else(|| FOLDER.into(), |name| name.to_string_lossy().into_owned());
        self.programs()
            .join(format!(".{name}{}", install::UPDATE_DIRECTORY))
    }

    pub fn shortcut(&self) -> PathBuf {
        self.start_menu.join(SHORTCUT)
    }

    /// The installed version, when the folder holds a description of the Windows folder.
    pub fn installed(&self) -> Option<Version> {
        described(&self.folder()).ok()
    }
}

/// The entry of the list of installed programs of the account, below
/// `Software\Microsoft\Windows\CurrentVersion\Uninstall\Junto` of `HKEY_CURRENT_USER`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UninstallEntry {
    pub strings: Vec<(&'static str, String)>,
    pub numbers: Vec<(&'static str, u32)>,
}

/// The key of the entry below `HKEY_CURRENT_USER`.
pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Junto";

/// The values of the entry of an installation of `version` whose files take `bytes`.
pub fn uninstall_entry(places: &Places, version: Version, bytes: u64) -> UninstallEntry {
    let folder = places.folder();
    let uninstaller = folder.join(UNINSTALLER);
    UninstallEntry {
        strings: vec![
            ("DisplayName", "Junto".to_owned()),
            ("DisplayVersion", version.to_string()),
            ("Publisher", "Junto".to_owned()),
            (
                "DisplayIcon",
                format!("{},0", folder.join(PROGRAM).display()),
            ),
            ("InstallLocation", folder.display().to_string()),
            ("UninstallString", format!("\"{}\"", uninstaller.display())),
            (
                "QuietUninstallString",
                format!("\"{}\" --quiet", uninstaller.display()),
            ),
        ],
        numbers: vec![
            ("NoModify", 1),
            ("NoRepair", 1),
            (
                "EstimatedSize",
                u32::try_from(bytes.div_ceil(1024)).unwrap_or(u32::MAX),
            ),
        ],
    }
}

/// The operations of Windows that an installation and a removal use.
pub trait SetupSystem {
    fn write_uninstall_entry(&self, entry: &UninstallEntry) -> std::io::Result<()>;
    fn remove_uninstall_entry(&self) -> std::io::Result<()>;
    /// A shortcut at `link` that starts `target` and names the application identifier of
    /// the notifications and the taskbar (ADR 0755).
    fn create_shortcut(&self, link: &Path, target: &Path) -> std::io::Result<()>;
    /// Removes the key of the identity of the notifications (ADR 0742).
    fn unregister_notifications(&self) -> std::io::Result<()>;
}

/// What an installation did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub version: Version,
    /// The version that it replaced.
    pub replaced: Option<Version>,
}

/// The total size of the files below a directory.
fn size_of(directory: &Path) -> u64 {
    fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => size_of(&entry.path()),
            Ok(_) => entry.metadata().map(|data| data.len()).unwrap_or(0),
            Err(_) => 0,
        })
        .sum()
}

/// Installs the archive of a setup program for the account: the folder `Junto` in
/// `programs`, replaced entry by entry when it exists, as an update replaces it (ADR 0755),
/// the Start menu entry and the entry of the list of installed programs. A newer installed
/// version is kept unless `allow_older` is set.
pub fn install(
    archive: &[u8],
    places: &Places,
    allow_older: bool,
    system: &impl SetupSystem,
) -> Result<Installed, Error> {
    fs::create_dir_all(places.programs())?;
    let updates = places.updates();
    if !updates.is_dir() {
        super::create_private_directory(&updates)?;
    }
    let lock = updates_lock(&updates)?;
    let stage = updates.join(install::random_name("stage-")?);
    archive::unpack(&mut &archive[..], &stage)?;
    let result = install_from(&stage, places, allow_older, system);
    finish(&updates, &stage);
    drop(lock);
    result
}

fn install_from(
    stage: &Path,
    places: &Places,
    allow_older: bool,
    system: &impl SetupSystem,
) -> Result<Installed, Error> {
    let candidate = stage.join(FOLDER);
    let version = described(&candidate)?;
    let entries = fs::read_dir(stage)?.count();
    if entries != 1 {
        return Err(Error::Invalid("package contents"));
    }
    archive::verify_modes(&candidate)?;
    let folder = places.folder();
    let replaced = places.installed();
    if let Some(installed) = replaced
        && installed > version
        && !allow_older
    {
        return Err(Error::Invalid("a newer version of Junto is installed"));
    }
    if fs::symlink_metadata(&folder).is_ok() {
        super::windows::exchange_entries(&folder, &candidate)?;
    } else {
        fs::rename(&candidate, &folder)?;
    }
    system.create_shortcut(&places.shortcut(), &places.program())?;
    system.write_uninstall_entry(&uninstall_entry(places, version, size_of(&folder)))?;
    Ok(Installed { version, replaced })
}

fn updates_lock(updates: &Path) -> Result<File, Error> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(updates.join("lock"))
        .map_err(|_| Error::Locked)?;
    file.try_lock().map_err(|_| Error::Locked)?;
    Ok(file)
}

/// Removes a staging directory, as an update does: a running program that the setup
/// replaced keeps it, and a mark lets the next update remove it (ADR 0726).
fn finish(updates: &Path, stage: &Path) {
    let Some(name) = stage.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let mark = updates.join(format!("finished-{name}"));
    if fs::write(&mark, b"").is_ok() && fs::remove_dir_all(stage).is_ok() {
        let _ = fs::remove_file(mark);
    }
}

/// Removes Junto from the account: the registration of its notifications, the Start menu
/// entry, the entry of the list of installed programs, the folder and the directory of its
/// updates, and with `remove_data` the data directory. Each part is tried; the first
/// failure is returned after the others.
pub fn uninstall(
    places: &Places,
    remove_data: bool,
    system: &impl SetupSystem,
) -> std::io::Result<()> {
    let absent = |result: std::io::Result<()>| match result {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    };
    let mut results = vec![
        system.unregister_notifications(),
        absent(fs::remove_file(places.shortcut())),
        system.remove_uninstall_entry(),
        absent(fs::remove_dir_all(places.folder())),
        absent(fs::remove_dir_all(places.updates())),
    ];
    if remove_data {
        results.push(absent(fs::remove_dir_all(&places.data)));
    }
    results.into_iter().collect()
}
