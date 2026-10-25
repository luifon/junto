//! The identity of the application for the system (ADR 0701, ADR 0742).

/// The identifier of the application: the window class and the desktop entry on Linux, and
/// the application user model identifier of the notifications and the taskbar on Windows.
pub const APP_ID: &str = "chat.junto.Junto";

/// The name of the file of the picture that a notification on Windows shows, in the data
/// directory.
pub const NOTIFICATION_PICTURE: &str = "notification-icon.png";

#[cfg(windows)]
pub use self::windows::{Registration, register, unregister};

/// Windows shows a notification of a program that it did not install from a package under an
/// application user model identifier, with the name and the picture of the key of that
/// identifier below `Software\Classes\AppUserModelId` of the account. The program writes the
/// key at each start; `junto-recover unregister` removes it.
#[cfg(windows)]
mod windows {
    use std::path::Path;

    /// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`.
    const NOT_FOUND: i32 = 0x8007_0002_u32 as i32;

    /// What a notification shows of the application.
    pub struct Registration<'a> {
        pub identifier: &'a str,
        pub name: &'a str,
        /// The picture file; `register` writes it.
        pub picture: &'a Path,
    }

    /// Where the keys of the identifiers are. A test uses a volatile key of its own below
    /// `Software\Classes`: a volatile key that it created at the place of the real parent would
    /// refuse every key that is not volatile below it until the logoff of the account.
    pub(super) struct Place<'a> {
        pub base: &'a str,
        pub volatile: bool,
    }

    const ACCOUNT: Place<'static> = Place {
        base: r"Software\Classes\AppUserModelId",
        volatile: false,
    };

    fn error(error: impl std::fmt::Display) -> std::io::Error {
        std::io::Error::other(error.to_string())
    }

    /// Writes the picture when the file differs from it, then the key of the identifier.
    pub fn register(registration: &Registration<'_>, picture: &[u8]) -> std::io::Result<()> {
        register_at(&ACCOUNT, registration, picture)
    }

    pub(super) fn register_at(
        place: &Place<'_>,
        registration: &Registration<'_>,
        picture: &[u8],
    ) -> std::io::Result<()> {
        let path = registration.picture.to_str().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the path of the picture is not Unicode",
            )
        })?;
        if std::fs::read(registration.picture).ok().as_deref() != Some(picture) {
            std::fs::write(registration.picture, picture)?;
        }
        let mut options = windows_registry::CURRENT_USER.options();
        options.read().write().create();
        if place.volatile {
            options.volatile();
        }
        let key = options
            .open(format!(r"{}\{}", place.base, registration.identifier))
            .map_err(error)?;
        key.set_string("DisplayName", registration.name)
            .map_err(error)?;
        key.set_string("IconUri", path).map_err(error)
    }

    /// Removes the key of the identifier. Returns whether it existed.
    pub fn unregister(identifier: &str) -> std::io::Result<bool> {
        unregister_at(&ACCOUNT, identifier)
    }

    pub(super) fn unregister_at(place: &Place<'_>, identifier: &str) -> std::io::Result<bool> {
        match windows_registry::CURRENT_USER.remove_tree(format!(r"{}\{identifier}", place.base)) {
            Ok(()) => Ok(true),
            Err(error) if error.code().0 == NOT_FOUND => Ok(false),
            Err(other) => Err(error(other)),
        }
    }

    #[cfg(test)]
    #[allow(unsafe_code)]
    mod tests {
        use super::*;

        const TEST_KEYS: &str = "junto-test-aumid-";

        /// The creation time of the running process with this number, or nothing when no process
        /// has it.
        fn started(process: u32) -> Option<u64> {
            use windows::Win32::Foundation::{CloseHandle, FILETIME};
            use windows::Win32::System::Threading::{
                GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
            };
            let mut times = [FILETIME::default(); 4];
            // SAFETY: the handle is closed after the read; the times are written into the array.
            unsafe {
                let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process).ok()?;
                let [created, exited, kernel, user] = &mut times;
                let read = GetProcessTimes(handle, created, exited, kernel, user);
                let _ = CloseHandle(handle);
                read.ok()?;
            }
            Some((u64::from(times[0].dwHighDateTime) << 32) | u64::from(times[0].dwLowDateTime))
        }

        /// Removes the test keys of processes that no longer run: the name holds the number and
        /// the creation time of the process of the test (ADR 0915).
        fn remove_ended_test_keys() {
            let classes = windows_registry::CURRENT_USER
                .open(r"Software\Classes")
                .unwrap();
            let names: Vec<String> = classes
                .keys()
                .unwrap()
                .filter(|name| name.starts_with(TEST_KEYS))
                .collect();
            for name in names {
                let mut parts = name[TEST_KEYS.len()..].splitn(2, '-');
                let process = parts.next().and_then(|part| part.parse::<u32>().ok());
                let time = parts.next().and_then(|part| part.parse::<u64>().ok());
                let running = process
                    .zip(time)
                    .is_some_and(|(process, time)| started(process) == Some(time));
                if !running {
                    let _ = windows_registry::CURRENT_USER
                        .remove_tree(format!(r"Software\Classes\{name}"));
                }
            }
        }

        fn modified(path: &Path) -> std::time::SystemTime {
            std::fs::metadata(path).unwrap().modified().unwrap()
        }

        #[test]
        fn the_key_names_the_application_and_its_picture_and_is_removed() {
            let account_parent = || windows_registry::CURRENT_USER.open(ACCOUNT.base).is_ok();
            let parent_before = account_parent();
            // A key of an ended process goes, and so does one of a name of another form; the key
            // of a running process stays.
            let ended = format!(r"Software\Classes\{TEST_KEYS}4294967295-1");
            let other_form = format!(r"Software\Classes\{TEST_KEYS}form");
            let process = std::process::id();
            let base = format!(
                r"Software\Classes\{TEST_KEYS}{process}-{}",
                started(process).unwrap()
            );
            for name in [&ended, &other_form] {
                windows_registry::CURRENT_USER
                    .options()
                    .read()
                    .write()
                    .create()
                    .volatile()
                    .open(name)
                    .unwrap();
            }
            remove_ended_test_keys();
            assert!(windows_registry::CURRENT_USER.open(&ended).is_err());
            assert!(windows_registry::CURRENT_USER.open(&other_form).is_err());
            // Volatile: Windows also removes the keys of a killed test at the logoff of the
            // account (ADR 0915).
            let place = Place {
                base: &base,
                volatile: true,
            };
            let directory = tempfile::tempdir().unwrap();
            let picture = directory
                .path()
                .join(crate::app_identity::NOTIFICATION_PICTURE);
            let registration = Registration {
                identifier: "chat.junto.Test",
                name: "Junto",
                picture: &picture,
            };
            // A missing picture is written.
            register_at(&place, &registration, b"first").unwrap();
            assert_eq!(std::fs::read(&picture).unwrap(), b"first");
            let key = windows_registry::CURRENT_USER
                .open(format!(r"{base}\chat.junto.Test"))
                .unwrap();
            assert_eq!(key.get_string("DisplayName").unwrap(), "Junto");
            assert_eq!(
                key.get_string("IconUri").unwrap(),
                picture.to_str().unwrap()
            );
            drop(key);
            // An equal picture is not written again; another one is.
            let earlier = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
            std::fs::File::options()
                .write(true)
                .open(&picture)
                .unwrap()
                .set_modified(earlier)
                .unwrap();
            register_at(&place, &registration, b"first").unwrap();
            assert_eq!(modified(&picture), earlier);
            register_at(&place, &registration, b"second").unwrap();
            assert_eq!(std::fs::read(&picture).unwrap(), b"second");
            assert!(unregister_at(&place, "chat.junto.Test").unwrap());
            assert!(!unregister_at(&place, "chat.junto.Test").unwrap());
            windows_registry::CURRENT_USER.remove_tree(&base).unwrap();
            assert!(windows_registry::CURRENT_USER.open(&base).is_err());
            // The test creates no key at the place of the identifiers of the account.
            assert_eq!(account_parent(), parent_before);
        }
    }
}
