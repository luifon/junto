//! Where the client keeps its files on each operating system (ADR 0710).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use junto_protocol::ids::ServerId;

use crate::storage::{Dir, DirectoryLock, StorageError};

/// Overrides the location of the data directory. Tests and the command-line tool use it.
pub const DATA_DIR_VARIABLE: &str = "JUNTO_DATA_DIR";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum System {
    MacOs,
    Windows,
    Linux,
}

impl System {
    pub const fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Linux
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LocationError {
    #[error("the environment variable {0} is not set to an absolute path")]
    NoBase(&'static str),
}

/// The data directory that `system` gives a user whose environment is `variable`.
///
/// - macOS: `$HOME/Library/Application Support/Junto`
/// - Windows: `%LOCALAPPDATA%\Junto`
/// - Linux: `$XDG_DATA_HOME/junto`, or `$HOME/.local/share/junto`
pub fn default_data_dir(
    system: System,
    variable: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf, LocationError> {
    let absolute = |name: &str| {
        variable(name)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| is_absolute(system, path))
    };
    if let Some(chosen) = absolute(DATA_DIR_VARIABLE) {
        return Ok(chosen);
    }
    match system {
        System::MacOs => absolute("HOME")
            .map(|home| {
                home.join("Library")
                    .join("Application Support")
                    .join("Junto")
            })
            .ok_or(LocationError::NoBase("HOME")),
        System::Windows => absolute("LOCALAPPDATA")
            .map(|local| local.join("Junto"))
            .ok_or(LocationError::NoBase("LOCALAPPDATA")),
        System::Linux => absolute("XDG_DATA_HOME")
            .map(|data| data.join("junto"))
            .or_else(|| {
                absolute("HOME").map(|home| home.join(".local").join("share").join("junto"))
            })
            .ok_or(LocationError::NoBase("HOME")),
    }
}

fn is_absolute(system: System, path: &Path) -> bool {
    if system == System::current() {
        return path.is_absolute();
    }
    let text = path.to_string_lossy();
    match system {
        System::Windows => {
            let bytes = text.as_bytes();
            text.starts_with(r"\\")
                || (bytes.len() >= 3
                    && bytes[0].is_ascii_alphabetic()
                    && bytes[1] == b':'
                    && (bytes[2] == b'\\' || bytes[2] == b'/'))
        }
        System::MacOs | System::Linux => text.starts_with('/'),
    }
}

/// The data directory of the current user on this system.
pub fn data_dir_of_this_user() -> Result<PathBuf, LocationError> {
    default_data_dir(System::current(), |name| std::env::var_os(name))
}

/// One opened data directory: the handles of the directory and of its key directory, and
/// the paths of the files in it.
#[derive(Debug, Clone)]
pub struct DataDir {
    handles: Arc<Handles>,
}

#[derive(Debug)]
struct Handles {
    root: Dir,
    keys: Dir,
}

impl DataDir {
    /// Opens the directory and its key directory and creates them when they do not exist.
    /// Both must belong to the user and be closed to everybody else; a link in place of
    /// either is refused. Removes what an interrupted write left behind.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let root = Dir::open(&root.into())?;
        let keys = root.subdirectory("keys")?;
        let data_dir = Self {
            handles: Arc::new(Handles { root, keys }),
        };
        let _lock = data_dir.lock()?;
        data_dir.handles.root.remove_leftovers()?;
        data_dir.handles.keys.remove_leftovers()?;
        Ok(data_dir)
    }

    pub fn root(&self) -> &Path {
        self.handles.root.path()
    }

    pub fn book_file(&self) -> PathBuf {
        self.root().join(BOOK_FILE)
    }

    pub fn profile_file(&self) -> PathBuf {
        self.root().join(PROFILE_FILE)
    }

    pub fn keys_dir(&self) -> PathBuf {
        self.handles.keys.path().to_owned()
    }

    /// The name of a key file is built from the server identifier and from nothing that a
    /// server sends as text.
    pub fn key_file(&self, server_id: ServerId) -> PathBuf {
        self.keys_dir().join(key_file_name(server_id))
    }

    pub(crate) fn files(&self) -> &Dir {
        &self.handles.root
    }

    pub(crate) fn keys(&self) -> &Dir {
        &self.handles.keys
    }

    pub(crate) fn lock(&self) -> Result<DirectoryLock, StorageError> {
        self.handles.root.lock()
    }
}

pub(crate) const BOOK_FILE: &str = "servers.json";
pub(crate) const PROFILE_FILE: &str = "profile.json";

pub(crate) fn key_file_name(server_id: ServerId) -> String {
    format!("{server_id}.key")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn environment(
        pairs: &'static [(&'static str, &'static str)],
    ) -> impl Fn(&str) -> Option<OsString> {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn macos_uses_application_support() {
        let path = default_data_dir(System::MacOs, environment(&[("HOME", "/Users/ada")])).unwrap();
        assert_eq!(
            path,
            PathBuf::from("/Users/ada/Library/Application Support/Junto")
        );
    }

    #[test]
    fn linux_prefers_the_xdg_data_directory() {
        let with_xdg = default_data_dir(
            System::Linux,
            environment(&[("HOME", "/home/ada"), ("XDG_DATA_HOME", "/data/ada")]),
        )
        .unwrap();
        assert_eq!(with_xdg, PathBuf::from("/data/ada/junto"));

        let without = default_data_dir(System::Linux, environment(&[("HOME", "/home/ada")]));
        assert_eq!(
            without.unwrap(),
            PathBuf::from("/home/ada/.local/share/junto")
        );

        let relative = default_data_dir(
            System::Linux,
            environment(&[("HOME", "/home/ada"), ("XDG_DATA_HOME", "relative/data")]),
        );
        assert_eq!(
            relative.unwrap(),
            PathBuf::from("/home/ada/.local/share/junto")
        );
    }

    #[test]
    fn windows_uses_the_local_application_data() {
        let path = default_data_dir(
            System::Windows,
            environment(&[
                ("LOCALAPPDATA", r"C:\Users\ada\AppData\Local"),
                ("APPDATA", r"C:\Users\ada\AppData\Roaming"),
            ]),
        )
        .unwrap();
        assert!(path.starts_with(r"C:\Users\ada\AppData\Local"));
        assert!(path.to_string_lossy().ends_with("Junto"));
        assert!(!path.to_string_lossy().contains("Roaming"));
    }

    #[test]
    fn the_override_wins_and_a_missing_base_is_an_error() {
        let chosen = default_data_dir(
            System::current(),
            environment(&[
                ("HOME", "/home/ada"),
                (DATA_DIR_VARIABLE, "/srv/junto-data"),
            ]),
        );
        if cfg!(unix) {
            assert_eq!(chosen.unwrap(), PathBuf::from("/srv/junto-data"));
        }
        assert_eq!(
            default_data_dir(System::MacOs, environment(&[])),
            Err(LocationError::NoBase("HOME"))
        );
        assert_eq!(
            default_data_dir(System::Linux, environment(&[("HOME", "")])),
            Err(LocationError::NoBase("HOME"))
        );
        assert_eq!(
            default_data_dir(System::Windows, environment(&[("HOME", "/home/ada")])),
            Err(LocationError::NoBase("LOCALAPPDATA"))
        );
        assert_eq!(
            default_data_dir(System::Windows, environment(&[("LOCALAPPDATA", "AppData")])),
            Err(LocationError::NoBase("LOCALAPPDATA"))
        );
    }

    #[test]
    fn every_stored_path_is_inside_the_data_directory() {
        let directory = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(directory.path().join("junto")).unwrap();
        let server_id = ServerId::generate();
        for path in [
            data_dir.book_file(),
            data_dir.profile_file(),
            data_dir.keys_dir(),
            data_dir.key_file(server_id),
        ] {
            assert!(path.starts_with(data_dir.root()), "{}", path.display());
            assert!(
                path.components()
                    .all(|part| part != std::path::Component::ParentDir)
            );
        }
        assert_eq!(
            data_dir.key_file(server_id).file_name().unwrap(),
            format!("{server_id}.key").as_str()
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_data_directory_that_others_can_enter_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("junto");
        std::fs::create_dir(&root).unwrap();
        for mode in [0o755, 0o750, 0o701] {
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(mode)).unwrap();
            assert!(
                matches!(DataDir::open(&root), Err(StorageError::NotPrivate { .. })),
                "{mode:o}"
            );
        }
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        DataDir::open(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_data_directory_or_key_directory_that_is_a_symbolic_link_is_refused() {
        let directory = tempfile::tempdir().unwrap();
        let elsewhere = directory.path().join("elsewhere");
        DataDir::open(&elsewhere).unwrap();
        let link = directory.path().join("junto");
        std::os::unix::fs::symlink(&elsewhere, &link).unwrap();
        assert!(DataDir::open(&link).is_err());

        let root = directory.path().join("other");
        let data_dir = DataDir::open(&root).unwrap();
        std::fs::remove_dir(data_dir.keys_dir()).unwrap();
        std::os::unix::fs::symlink(elsewhere.join("keys"), data_dir.keys_dir()).unwrap();
        assert!(DataDir::open(&root).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn the_data_directory_and_the_key_directory_are_created_private() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(directory.path().join("a").join("junto")).unwrap();
        for path in [data_dir.root().to_owned(), data_dir.keys_dir()] {
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", path.display());
        }
        DataDir::open(data_dir.root()).unwrap();
    }
}
