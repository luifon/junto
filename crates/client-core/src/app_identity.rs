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

    const IDENTIFIERS: &str = r"Software\Classes\AppUserModelId";
    /// `HRESULT_FROM_WIN32(ERROR_FILE_NOT_FOUND)`.
    const NOT_FOUND: i32 = 0x8007_0002_u32 as i32;

    /// What a notification shows of the application.
    pub struct Registration<'a> {
        pub identifier: &'a str,
        pub name: &'a str,
        /// The picture file; `register` writes it.
        pub picture: &'a Path,
    }

    fn key_path(identifier: &str) -> String {
        format!(r"{IDENTIFIERS}\{identifier}")
    }

    fn error(error: windows_registry::Result<()>) -> std::io::Result<()> {
        error.map_err(|error| std::io::Error::other(error.message()))
    }

    /// Writes the picture when the file differs from it, then the key of the identifier.
    pub fn register(registration: &Registration<'_>, picture: &[u8]) -> std::io::Result<()> {
        if std::fs::read(registration.picture).ok().as_deref() != Some(picture) {
            std::fs::write(registration.picture, picture)?;
        }
        let key = windows_registry::CURRENT_USER
            .create(key_path(registration.identifier))
            .map_err(|error| std::io::Error::other(error.message()))?;
        write(&key, registration)
    }

    pub(super) fn write(
        key: &windows_registry::Key,
        registration: &Registration<'_>,
    ) -> std::io::Result<()> {
        let picture = registration.picture.to_str().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "the path of the picture is not Unicode",
            )
        })?;
        error(key.set_string("DisplayName", registration.name))?;
        error(key.set_string("IconUri", picture))
    }

    /// Removes the key of the identifier. Returns whether it existed.
    pub fn unregister(identifier: &str) -> std::io::Result<bool> {
        match windows_registry::CURRENT_USER.remove_tree(key_path(identifier)) {
            Ok(()) => Ok(true),
            Err(error) if error.code().0 == NOT_FOUND => Ok(false),
            Err(error) => Err(std::io::Error::other(error.message())),
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_key_names_the_application_and_its_picture_and_is_removed() {
            let directory = tempfile::tempdir().unwrap();
            let picture = directory
                .path()
                .join(crate::app_identity::NOTIFICATION_PICTURE);
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let identifier = format!("chat.junto.Test.{}.{nanos}", std::process::id());
            // A volatile key, which Windows also removes at the logoff of the account, so a
            // killed test leaves none behind (ADR 0915).
            let key = windows_registry::CURRENT_USER
                .options()
                .read()
                .write()
                .create()
                .volatile()
                .open(key_path(&identifier))
                .unwrap();
            let registration = Registration {
                identifier: &identifier,
                name: "Junto",
                picture: &picture,
            };
            write(&key, &registration).unwrap();
            let read = windows_registry::CURRENT_USER
                .open(key_path(&identifier))
                .unwrap();
            assert_eq!(read.get_string("DisplayName").unwrap(), "Junto");
            assert_eq!(
                read.get_string("IconUri").unwrap(),
                picture.to_str().unwrap()
            );
            drop((key, read));
            assert!(unregister(&identifier).unwrap());
            assert!(!unregister(&identifier).unwrap());
            assert!(
                windows_registry::CURRENT_USER
                    .open(key_path(&identifier))
                    .is_err()
            );
        }
    }
}
