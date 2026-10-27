//! The local profile preset: the name and the avatar that a new server account starts from
//! (ADR 0105, ADR 0710).

use std::fs::File;
use std::io;
use std::path::Path;

use junto_protocol::account::ImageType;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::paths::{DataDir, PROFILE_FILE};
use crate::storage::{StorageError, Versioned};

pub const PROFILE_FORMAT_VERSION: u32 = 2;

/// The longest name of an account or a device, in characters. The server has the same rule.
pub const NAME_MAX_CHARS: usize = 64;

/// The largest avatar that a server stores (ADR 0119). The preset has the same bound, so
/// that its avatar can be sent to every server.
pub const AVATAR_MAX_BYTES: usize = 2 * 1024 * 1024;
/// The largest file that a person can choose as a picture. The window cuts it to a square
/// and scales it to at most [`AVATAR_MAX_BYTES`] before a server sees it (ADR 0119).
pub const CHOSEN_PICTURE_MAX_BYTES: usize = 10 * 1024 * 1024;

const PROFILE: Versioned = Versioned {
    name: PROFILE_FILE,
    kind: "profile",
    version: PROFILE_FORMAT_VERSION,
    migrations: &[media_type_of_the_avatar],
    limit: 64 * 1024,
};

/// Version 1 named the format of the avatar `png`, `jpeg` or `webp`. Version 2 stores its
/// media type.
fn media_type_of_the_avatar(mut value: serde_json::Value) -> Result<serde_json::Value, String> {
    let profile = value
        .as_object_mut()
        .ok_or("the profile is not an object")?;
    let media_type = match profile.get("avatar") {
        None | Some(serde_json::Value::Null) => return Ok(value),
        Some(format) => match format.as_str() {
            Some("png") => ImageType::Png,
            Some("jpeg") => ImageType::Jpeg,
            Some("webp") => ImageType::Webp,
            _ => return Err("the format of the avatar is not known".to_owned()),
        },
    };
    profile.insert("avatar".to_owned(), media_type.media_type().into());
    Ok(value)
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidName {
    #[error("the name is empty")]
    Empty,
    #[error("the name is longer than 64 characters")]
    TooLong,
    #[error("the name contains a control character")]
    ControlCharacter,
}

/// A name of 1 to 64 characters without control characters, with outer whitespace removed.
pub fn account_name(text: &str) -> Result<String, InvalidName> {
    let name = device_name(text)?;
    if name.is_empty() {
        return Err(InvalidName::Empty);
    }
    Ok(name)
}

/// Like [`account_name`], and an empty name is allowed.
pub fn device_name(text: &str) -> Result<String, InvalidName> {
    let trimmed = text.trim();
    if trimmed.chars().count() > NAME_MAX_CHARS {
        return Err(InvalidName::TooLong);
    }
    if trimmed.chars().any(char::is_control) {
        return Err(InvalidName::ControlCharacter);
    }
    Ok(trimmed.to_owned())
}

/// Why bytes are not an avatar that a server stores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum InvalidAvatar {
    #[error("the avatar is larger than 2 MiB")]
    TooLarge,
    #[error("the avatar is not a PNG, JPEG or WebP image")]
    Unsupported,
}

/// The format that the first bytes of an image state, when the image has at most the size
/// of an avatar. The size in pixels is not read here; the server checks it (ADR 0119).
pub fn check_avatar(image: &[u8]) -> Result<ImageType, InvalidAvatar> {
    if image.len() > AVATAR_MAX_BYTES {
        return Err(InvalidAvatar::TooLarge);
    }
    if image.starts_with(b"\x89PNG\r\n\x1a\n") {
        Ok(ImageType::Png)
    } else if image.starts_with(b"\xff\xd8\xff") {
        Ok(ImageType::Jpeg)
    } else if image.len() >= 12 && &image[..4] == b"RIFF" && &image[8..12] == b"WEBP" {
        Ok(ImageType::Webp)
    } else {
        Err(InvalidAvatar::Unsupported)
    }
}

/// Opens a file that a person chose in a window. A link at the end of the path is not
/// followed, and the open of a pipe or of a device does not wait for its other side.
#[cfg(unix)]
fn open_chosen(path: &Path) -> io::Result<File> {
    use rustix::fs::{Mode, OFlags};
    let flags = OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC;
    Ok(File::from(rustix::fs::open(path, flags, Mode::empty())?))
}

/// This system has no such flags of an open. The kind of the opened file is still checked.
#[cfg(not(unix))]
fn open_chosen(path: &Path) -> io::Result<File> {
    File::open(path)
}

/// Reads a file that a person chose as an avatar in a window. Only a regular file is
/// read, and its kind is read from the opened file, so the file cannot be exchanged
/// between the check and the read. One byte more than an avatar can have is read at
/// most, so that a larger file is refused by [`check_avatar`] and not held in memory.
pub fn read_avatar_file(path: &Path) -> io::Result<Vec<u8>> {
    read_chosen_file(path, AVATAR_MAX_BYTES)
}

/// Reads a file that a person chose as a picture, as [`read_avatar_file`] does, with one
/// byte more than `most` at most: a larger file is refused by its caller and not held in
/// memory.
pub fn read_chosen_file(path: &Path, most: usize) -> io::Result<Vec<u8>> {
    use std::io::Read as _;
    let file = open_chosen(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the file is not a regular file",
        ));
    }
    let mut image = Vec::new();
    file.take(most as u64 + 1).read_to_end(&mut image)?;
    Ok(image)
}

/// The SHA-256 hash of an image in lower-case hexadecimal, as the server describes an
/// avatar (ADR 0119).
pub fn avatar_hash(image: &[u8]) -> String {
    use std::fmt::Write;
    let digest = Sha256::digest(image);
    let mut text = String::with_capacity(digest.len() * 2);
    for byte in digest {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

const fn avatar_file_name(kind: ImageType) -> &'static str {
    match kind {
        ImageType::Png => "avatar.png",
        ImageType::Jpeg => "avatar.jpg",
        ImageType::Webp => "avatar.webp",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProfilePreset {
    /// Empty until the user has chosen a name.
    pub name: String,
    pub avatar: Option<ImageType>,
}

#[derive(Serialize, Deserialize)]
struct ProfileFile {
    format_version: u32,
    name: String,
    /// The media type of the stored image.
    avatar: Option<ImageType>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Name(#[from] InvalidName),
    #[error(transparent)]
    Avatar(#[from] InvalidAvatar),
}

#[derive(Debug, Clone)]
pub struct ProfileStore {
    data_dir: DataDir,
}

impl ProfileStore {
    pub fn new(data_dir: DataDir) -> Self {
        Self { data_dir }
    }

    pub fn load(&self) -> Result<ProfilePreset, StorageError> {
        let files = self.data_dir.files();
        let Some(file) = PROFILE.load::<ProfileFile>(files)? else {
            return Ok(ProfilePreset::default());
        };
        if !file.name.is_empty() && account_name(&file.name).as_deref() != Ok(file.name.as_str()) {
            return Err(PROFILE.corrupted(files, "the name is not valid".to_owned()));
        }
        Ok(ProfilePreset {
            name: file.name,
            avatar: file.avatar,
        })
    }

    pub fn set_name(&self, name: &str) -> Result<ProfilePreset, ProfileError> {
        let name = account_name(name)?;
        let _lock = self.data_dir.lock()?;
        let mut preset = self.load()?;
        preset.name = name;
        self.write(&preset)?;
        Ok(preset)
    }

    /// Stores the image, then the reference to it, then removes an image of another format.
    pub fn set_avatar(&self, image: &[u8]) -> Result<ProfilePreset, ProfileError> {
        let format = check_avatar(image)?;
        let _lock = self.data_dir.lock()?;
        let mut preset = self.load()?;
        self.data_dir
            .files()
            .write(avatar_file_name(format), image)?;
        preset.avatar = Some(format);
        self.write(&preset)?;
        self.remove_images_except(Some(format));
        Ok(preset)
    }

    pub fn clear_avatar(&self) -> Result<ProfilePreset, ProfileError> {
        let _lock = self.data_dir.lock()?;
        let mut preset = self.load()?;
        preset.avatar = None;
        self.write(&preset)?;
        self.remove_images_except(None);
        Ok(preset)
    }

    pub fn avatar(&self) -> Result<Option<(ImageType, Vec<u8>)>, StorageError> {
        let Some(format) = self.load()?.avatar else {
            return Ok(None);
        };
        let image = self
            .data_dir
            .files()
            .read(avatar_file_name(format), AVATAR_MAX_BYTES as u64)?;
        Ok(image.map(|image| (format, image)))
    }

    fn write(&self, preset: &ProfilePreset) -> Result<(), StorageError> {
        let file = ProfileFile {
            format_version: PROFILE_FORMAT_VERSION,
            name: preset.name.clone(),
            avatar: preset.avatar,
        };
        PROFILE.store(self.data_dir.files(), &file)
    }

    fn remove_images_except(&self, kept: Option<ImageType>) {
        for format in ImageType::ALL {
            if Some(*format) != kept {
                let _ = self.data_dir.files().remove(avatar_file_name(*format));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    const PNG: &[u8] = b"\x89PNG\r\n\x1a\n-image-";
    const JPEG: &[u8] = b"\xff\xd8\xff\xe0-image-";

    fn store() -> (tempfile::TempDir, DataDir, ProfileStore) {
        let directory = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(directory.path().join("junto")).unwrap();
        let store = ProfileStore::new(data_dir.clone());
        (directory, data_dir, store)
    }

    #[test]
    fn names_follow_the_rule_of_the_server() {
        assert_eq!(account_name("  Ada  ").unwrap(), "Ada");
        assert_eq!(account_name(&"é".repeat(64)).unwrap().chars().count(), 64);
        assert_eq!(account_name(&"a".repeat(65)), Err(InvalidName::TooLong));
        assert_eq!(account_name("   "), Err(InvalidName::Empty));
        assert_eq!(account_name("a\nb"), Err(InvalidName::ControlCharacter));
        assert_eq!(device_name("  ").unwrap(), "");
        assert_eq!(device_name("tab\there"), Err(InvalidName::ControlCharacter));
    }

    #[test]
    fn the_preset_starts_empty_and_keeps_a_name_and_an_avatar() {
        let (_directory, data_dir, store) = store();
        assert_eq!(store.load().unwrap(), ProfilePreset::default());
        assert!(store.avatar().unwrap().is_none());

        store.set_name(" Ada ").unwrap();
        let preset = store.set_avatar(PNG).unwrap();
        assert_eq!(preset.name, "Ada");
        assert_eq!(preset.avatar, Some(ImageType::Png));
        assert_eq!(store.load().unwrap(), preset);
        assert_eq!(
            store.avatar().unwrap().unwrap(),
            (ImageType::Png, PNG.to_vec())
        );

        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(data_dir.profile_file()).unwrap()).unwrap();
        assert_eq!(stored["format_version"], 2);
        assert_eq!(stored["name"], "Ada");
        assert_eq!(stored["avatar"], "image/png");
    }

    #[test]
    fn a_new_avatar_replaces_the_old_file() {
        let (_directory, data_dir, store) = store();
        store.set_avatar(PNG).unwrap();
        store.set_avatar(JPEG).unwrap();
        assert!(!data_dir.root().join("avatar.png").exists());
        assert_eq!(
            store.avatar().unwrap().unwrap(),
            (ImageType::Jpeg, JPEG.to_vec())
        );
        store.clear_avatar().unwrap();
        assert!(!data_dir.root().join("avatar.jpg").exists());
        assert!(store.avatar().unwrap().is_none());
    }

    #[test]
    fn other_content_and_large_images_are_refused() {
        let (_directory, _data_dir, store) = store();
        assert!(matches!(
            store.set_avatar(b"<svg></svg>"),
            Err(ProfileError::Avatar(InvalidAvatar::Unsupported))
        ));
        let mut large = PNG.to_vec();
        large.resize(AVATAR_MAX_BYTES, 0);
        assert_eq!(check_avatar(&large), Ok(ImageType::Png));
        large.push(0);
        assert!(matches!(
            store.set_avatar(&large),
            Err(ProfileError::Avatar(InvalidAvatar::TooLarge))
        ));
        assert!(store.avatar().unwrap().is_none());
        assert!(matches!(
            store.set_name(" "),
            Err(ProfileError::Name(InvalidName::Empty))
        ));
        assert_eq!(check_avatar(b"RIFF\x10\0\0\0WEBPVP8 "), Ok(ImageType::Webp));
        assert_eq!(check_avatar(JPEG), Ok(ImageType::Jpeg));
        assert_eq!(check_avatar(b""), Err(InvalidAvatar::Unsupported));
        assert_eq!(
            check_avatar(b"RIFF\x10\0\0\0WAVE"),
            Err(InvalidAvatar::Unsupported)
        );
    }

    #[test]
    fn the_hash_of_an_avatar_is_written_as_the_server_writes_it() {
        assert_eq!(
            avatar_hash(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            avatar_hash(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn a_profile_of_format_version_1_is_read_and_written_in_the_current_version() {
        for (format, kind, file_name) in [
            ("png", ImageType::Png, "avatar.png"),
            ("jpeg", ImageType::Jpeg, "avatar.jpg"),
            ("webp", ImageType::Webp, "avatar.webp"),
        ] {
            let (_directory, data_dir, store) = store();
            let written = format!(r#"{{"format_version":1,"name":"Ada","avatar":"{format}"}}"#);
            fs::write(data_dir.profile_file(), written).unwrap();
            fs::write(data_dir.root().join(file_name), PNG).unwrap();
            let preset = store.load().unwrap();
            assert_eq!(preset.name, "Ada");
            assert_eq!(preset.avatar, Some(kind));
            assert_eq!(store.avatar().unwrap().unwrap(), (kind, PNG.to_vec()));

            store.set_name("Ada L.").unwrap();
            let stored: serde_json::Value =
                serde_json::from_slice(&fs::read(data_dir.profile_file()).unwrap()).unwrap();
            assert_eq!(stored["format_version"], 2);
            assert_eq!(stored["avatar"], kind.media_type());
            assert_eq!(store.load().unwrap().avatar, Some(kind));
        }

        let (_directory, data_dir, store) = store();
        fs::write(
            data_dir.profile_file(),
            br#"{"format_version":1,"name":"Ada","avatar":null}"#,
        )
        .unwrap();
        assert_eq!(store.load().unwrap().avatar, None);
        fs::write(
            data_dir.profile_file(),
            br#"{"format_version":1,"name":"Ada","avatar":"gif"}"#,
        )
        .unwrap();
        assert!(matches!(store.load(), Err(StorageError::Corrupted { .. })));
        // The spelling of version 1 is not a value of version 2.
        fs::write(
            data_dir.profile_file(),
            br#"{"format_version":2,"name":"Ada","avatar":"png"}"#,
        )
        .unwrap();
        assert!(matches!(store.load(), Err(StorageError::Corrupted { .. })));
    }

    #[test]
    fn a_corrupted_or_newer_profile_is_reported() {
        let (_directory, data_dir, store) = store();
        fs::write(data_dir.profile_file(), b"{\"format_version\":2,").unwrap();
        assert!(matches!(store.load(), Err(StorageError::Corrupted { .. })));
        assert!(matches!(
            store.set_name("Ada"),
            Err(ProfileError::Storage(StorageError::Corrupted { .. }))
        ));
        fs::write(
            data_dir.profile_file(),
            br#"{"format_version":2,"name":"a\nb","avatar":null}"#,
        )
        .unwrap();
        assert!(matches!(store.load(), Err(StorageError::Corrupted { .. })));
        fs::write(
            data_dir.profile_file(),
            br#"{"format_version":9,"name":"Ada","avatar":null}"#,
        )
        .unwrap();
        assert!(matches!(
            store.load(),
            Err(StorageError::NewerFormat { found: 9, .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn an_avatar_file_is_a_regular_file_and_is_read_up_to_one_byte_beyond_the_bound() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("picture.png");
        fs::write(&file, PNG).unwrap();
        assert_eq!(read_avatar_file(&file).unwrap(), PNG);
        let mut large = PNG.to_vec();
        large.resize(4 * AVATAR_MAX_BYTES, 0);
        fs::write(&file, &large).unwrap();
        assert_eq!(read_avatar_file(&file).unwrap().len(), AVATAR_MAX_BYTES + 1);

        // A link is not followed, also when it names a regular file.
        let link = directory.path().join("link.png");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(read_avatar_file(&link).is_err());
        assert!(read_avatar_file(directory.path()).is_err());
        assert!(read_avatar_file(&directory.path().join("absent.png")).is_err());

        // A pipe that nobody writes to is refused, and its open does not wait.
        let pipe = directory.path().join("pipe.png");
        let made = std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()
            .unwrap();
        assert!(made.success());
        let (tell, told) = std::sync::mpsc::channel();
        let reader = {
            let pipe = pipe.clone();
            std::thread::spawn(move || {
                let _ = tell.send(read_avatar_file(&pipe).map_err(|error| error.kind()));
            })
        };
        let read = told.recv_timeout(std::time::Duration::from_secs(5)).ok();
        if read.is_none() {
            // The open of the reader waits for a writer: this one lets the thread end.
            drop(fs::OpenOptions::new().write(true).open(&pipe));
        }
        reader.join().unwrap();
        assert_eq!(read, Some(Err(io::ErrorKind::InvalidInput)));
    }
}
