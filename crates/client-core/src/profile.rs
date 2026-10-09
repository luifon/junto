//! The local profile preset: the name and the avatar that a new server account starts from
//! (ADR 0105, ADR 0710).

use serde::{Deserialize, Serialize};

use crate::paths::{DataDir, PROFILE_FILE};
use crate::storage::{StorageError, Versioned};

pub const PROFILE_FORMAT_VERSION: u32 = 1;

/// The longest name of an account or a device, in characters. The server has the same rule.
pub const NAME_MAX_CHARS: usize = 64;

pub const AVATAR_MAX_BYTES: u64 = 1024 * 1024;

const PROFILE: Versioned = Versioned {
    name: PROFILE_FILE,
    kind: "profile",
    version: PROFILE_FORMAT_VERSION,
    migrations: &[],
    limit: 64 * 1024,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AvatarFormat {
    Png,
    Jpeg,
    Webp,
}

impl AvatarFormat {
    const ALL: [Self; 3] = [Self::Png, Self::Jpeg, Self::Webp];

    const fn file_name(self) -> &'static str {
        match self {
            Self::Png => "avatar.png",
            Self::Jpeg => "avatar.jpg",
            Self::Webp => "avatar.webp",
        }
    }

    pub const fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
        }
    }

    /// The format that the first bytes of an image state.
    pub fn of(image: &[u8]) -> Option<Self> {
        if image.starts_with(b"\x89PNG\r\n\x1a\n") {
            Some(Self::Png)
        } else if image.starts_with(b"\xff\xd8\xff") {
            Some(Self::Jpeg)
        } else if image.len() >= 12 && &image[..4] == b"RIFF" && &image[8..12] == b"WEBP" {
            Some(Self::Webp)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProfilePreset {
    /// Empty until the user has chosen a name.
    pub name: String,
    pub avatar: Option<AvatarFormat>,
}

#[derive(Serialize, Deserialize)]
struct ProfileFile {
    format_version: u32,
    name: String,
    avatar: Option<AvatarFormat>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Name(#[from] InvalidName),
    #[error("the avatar is not a PNG, JPEG or WebP image")]
    UnknownImage,
    #[error("the avatar is larger than 1 MiB")]
    ImageTooLarge,
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
        if image.len() as u64 > AVATAR_MAX_BYTES {
            return Err(ProfileError::ImageTooLarge);
        }
        let format = AvatarFormat::of(image).ok_or(ProfileError::UnknownImage)?;
        let _lock = self.data_dir.lock()?;
        let mut preset = self.load()?;
        self.data_dir.files().write(format.file_name(), image)?;
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

    pub fn avatar(&self) -> Result<Option<(AvatarFormat, Vec<u8>)>, StorageError> {
        let Some(format) = self.load()?.avatar else {
            return Ok(None);
        };
        let image = self
            .data_dir
            .files()
            .read(format.file_name(), AVATAR_MAX_BYTES)?;
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

    fn remove_images_except(&self, kept: Option<AvatarFormat>) {
        for format in AvatarFormat::ALL {
            if Some(format) != kept {
                let _ = self.data_dir.files().remove(format.file_name());
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
        assert_eq!(preset.avatar, Some(AvatarFormat::Png));
        assert_eq!(store.load().unwrap(), preset);
        assert_eq!(
            store.avatar().unwrap().unwrap(),
            (AvatarFormat::Png, PNG.to_vec())
        );

        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(data_dir.profile_file()).unwrap()).unwrap();
        assert_eq!(stored["format_version"], 1);
        assert_eq!(stored["name"], "Ada");
        assert_eq!(stored["avatar"], "png");
    }

    #[test]
    fn a_new_avatar_replaces_the_old_file() {
        let (_directory, data_dir, store) = store();
        store.set_avatar(PNG).unwrap();
        store.set_avatar(JPEG).unwrap();
        assert!(!data_dir.root().join("avatar.png").exists());
        assert_eq!(
            store.avatar().unwrap().unwrap(),
            (AvatarFormat::Jpeg, JPEG.to_vec())
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
            Err(ProfileError::UnknownImage)
        ));
        let mut large = PNG.to_vec();
        large.resize(AVATAR_MAX_BYTES as usize + 1, 0);
        assert!(matches!(
            store.set_avatar(&large),
            Err(ProfileError::ImageTooLarge)
        ));
        assert!(matches!(
            store.set_name(" "),
            Err(ProfileError::Name(InvalidName::Empty))
        ));
        assert_eq!(
            AvatarFormat::of(b"RIFF\x10\0\0\0WEBPVP8 "),
            Some(AvatarFormat::Webp)
        );
        assert_eq!(AvatarFormat::Webp.media_type(), "image/webp");
    }

    #[test]
    fn a_corrupted_or_newer_profile_is_reported() {
        let (_directory, data_dir, store) = store();
        fs::write(data_dir.profile_file(), b"{\"format_version\":1,").unwrap();
        assert!(matches!(store.load(), Err(StorageError::Corrupted { .. })));
        assert!(matches!(
            store.set_name("Ada"),
            Err(ProfileError::Storage(StorageError::Corrupted { .. }))
        ));
        fs::write(
            data_dir.profile_file(),
            br#"{"format_version":1,"name":"a\nb","avatar":null}"#,
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
}
