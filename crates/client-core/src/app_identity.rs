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
    mod tests {
        use super::*;

        fn modified(path: &Path) -> std::time::SystemTime {
            std::fs::metadata(path).unwrap().modified().unwrap()
        }

        #[test]
        fn the_key_names_the_application_and_its_picture_and_is_removed() {
            let account_parent = || windows_registry::CURRENT_USER.open(ACCOUNT.base).is_ok();
            let parent_before = account_parent();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let base = format!(
                r"Software\Classes\junto-test-aumid-{}-{nanos}",
                std::process::id()
            );
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
