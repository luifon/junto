//! The settings of the interface that belong to one installation and to no server: the
//! interface scale, the theme, the form of the server book (ADR 0703, ADR 0710) and the key
//! bindings (ADR 0777).

use serde::{Deserialize, Serialize};

use crate::keybinds::Keybinds;
use crate::paths::{DataDir, SETTINGS_FILE};
use crate::storage::{StorageError, Versioned};

pub const SETTINGS_FORMAT_VERSION: u32 = 2;

/// The narrowest and the widest interface scale, in percent (ADR 0703).
pub const SCALE_MIN_PERCENT: u32 = 90;
pub const SCALE_MAX_PERCENT: u32 = 150;

const SETTINGS: Versioned = Versioned {
    name: SETTINGS_FILE,
    kind: "settings",
    version: SETTINGS_FORMAT_VERSION,
    migrations: &[key_bindings],
    limit: 64 * 1024,
};

/// Version 1 had no key bindings: every action has its default.
fn key_bindings(mut value: serde_json::Value) -> Result<serde_json::Value, String> {
    value
        .as_object_mut()
        .ok_or("the settings are not an object")?
        .entry("keybinds")
        .or_insert(serde_json::json!({}));
    Ok(value)
}

/// The theme that the person chose. `System` follows the light or dark appearance of the
/// system.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeChoice {
    #[default]
    Light,
    Dark,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceSettings {
    /// The interface scale in percent, from 90 to 150.
    pub scale_percent: u32,
    /// The server book shows icons only.
    pub server_book_collapsed: bool,
    pub theme: ThemeChoice,
}

impl Default for InterfaceSettings {
    fn default() -> Self {
        Self {
            scale_percent: 100,
            server_book_collapsed: false,
            theme: ThemeChoice::default(),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SettingsFile {
    format_version: u32,
    scale_percent: u32,
    #[serde(default)]
    server_book_collapsed: bool,
    #[serde(default)]
    theme: ThemeChoice,
    #[serde(default)]
    keybinds: Keybinds,
}

#[derive(Debug, Clone)]
pub struct SettingsStore {
    data_dir: DataDir,
}

impl SettingsStore {
    pub fn new(data_dir: DataDir) -> Self {
        Self { data_dir }
    }

    /// The stored settings, or the defaults when there is no file. A scale outside its
    /// range is read as the nearest bound.
    pub fn load(&self) -> Result<InterfaceSettings, StorageError> {
        Ok(self.load_all()?.0)
    }

    /// The stored key bindings, or the defaults when there is no file.
    pub fn load_keybinds(&self) -> Result<Keybinds, StorageError> {
        Ok(self.load_all()?.1)
    }

    fn load_all(&self) -> Result<(InterfaceSettings, Keybinds), StorageError> {
        let Some(file) = SETTINGS.load::<SettingsFile>(self.data_dir.files())? else {
            return Ok((InterfaceSettings::default(), Keybinds::default()));
        };
        let settings = InterfaceSettings {
            scale_percent: file
                .scale_percent
                .clamp(SCALE_MIN_PERCENT, SCALE_MAX_PERCENT),
            server_book_collapsed: file.server_book_collapsed,
            theme: file.theme,
        };
        Ok((settings, file.keybinds.checked()))
    }

    /// Replaces the stored settings. See [`SettingsStore::update`] for a file that cannot
    /// be read.
    pub fn save(&self, settings: InterfaceSettings) -> Result<(), StorageError> {
        self.update(|stored| *stored = settings).map(|_| ())
    }

    /// Changes the stored settings in one step under the lock of the data directory: the
    /// file is read again, changed and written, so that a change of one setting keeps what
    /// another program wrote of the others meanwhile. Returns what was written.
    ///
    /// A file that is not a settings file is replaced with the defaults and the change:
    /// the settings hold nothing that a person cannot set again. Every other file that
    /// cannot be read stays as it is, and the change fails: a file of a newer format, a
    /// file that is larger than this client reads, a file that cannot be opened.
    pub fn update(
        &self,
        change: impl FnOnce(&mut InterfaceSettings),
    ) -> Result<InterfaceSettings, StorageError> {
        Ok(self.update_all(|settings, _| change(settings))?.0)
    }

    /// As [`SettingsStore::update`], for the key bindings.
    pub fn update_keybinds(
        &self,
        change: impl FnOnce(&mut Keybinds),
    ) -> Result<Keybinds, StorageError> {
        Ok(self.update_all(|_, keybinds| change(keybinds))?.1)
    }

    fn update_all(
        &self,
        change: impl FnOnce(&mut InterfaceSettings, &mut Keybinds),
    ) -> Result<(InterfaceSettings, Keybinds), StorageError> {
        let _lock = self.data_dir.lock()?;
        let (mut settings, mut keybinds) = match self.load_all() {
            Ok(stored) => stored,
            Err(StorageError::Corrupted { .. }) => Default::default(),
            Err(other) => return Err(other),
        };
        change(&mut settings, &mut keybinds);
        settings.scale_percent = settings
            .scale_percent
            .clamp(SCALE_MIN_PERCENT, SCALE_MAX_PERCENT);
        let keybinds = keybinds.checked();
        SETTINGS.store(
            self.data_dir.files(),
            &SettingsFile {
                format_version: SETTINGS_FORMAT_VERSION,
                scale_percent: settings.scale_percent,
                server_book_collapsed: settings.server_book_collapsed,
                theme: settings.theme,
                keybinds: keybinds.clone(),
            },
        )?;
        Ok((settings, keybinds))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn store() -> (tempfile::TempDir, DataDir, SettingsStore) {
        let directory = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(directory.path().join("junto")).unwrap();
        let store = SettingsStore::new(data_dir.clone());
        (directory, data_dir, store)
    }

    #[test]
    fn the_settings_start_with_their_defaults_and_keep_what_is_saved() {
        let (_directory, data_dir, store) = store();
        assert_eq!(store.load().unwrap(), InterfaceSettings::default());
        assert!(!data_dir.root().join(SETTINGS_FILE).exists());

        let chosen = InterfaceSettings {
            scale_percent: 125,
            server_book_collapsed: true,
            theme: ThemeChoice::Dark,
        };
        store.save(chosen).unwrap();
        assert_eq!(store.load().unwrap(), chosen);
        let text = fs::read_to_string(data_dir.root().join(SETTINGS_FILE)).unwrap();
        assert!(text.contains("\"format_version\": 1"), "{text}");
    }

    #[test]
    fn a_scale_outside_its_range_becomes_the_nearest_bound() {
        let (_directory, data_dir, store) = store();
        store
            .save(InterfaceSettings {
                scale_percent: 400,
                ..InterfaceSettings::default()
            })
            .unwrap();
        assert_eq!(store.load().unwrap().scale_percent, SCALE_MAX_PERCENT);
        fs::write(
            data_dir.root().join(SETTINGS_FILE),
            r#"{"format_version":1,"scale_percent":10}"#,
        )
        .unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded.scale_percent, SCALE_MIN_PERCENT);
        assert!(!loaded.server_book_collapsed);
        // A file of a client without themes has the light theme.
        assert_eq!(loaded.theme, ThemeChoice::Light);
    }

    #[test]
    fn a_file_of_a_newer_format_stays_and_an_unreadable_file_is_replaced() {
        let (_directory, data_dir, store) = store();
        let file = data_dir.root().join(SETTINGS_FILE);
        let newer = r#"{"format_version":3,"scale_percent":110,"later":true}"#;
        fs::write(&file, newer).unwrap();
        assert!(matches!(
            store.load(),
            Err(StorageError::NewerFormat {
                found: 3,
                supported: 2,
                ..
            })
        ));
        assert!(matches!(
            store.save(InterfaceSettings::default()),
            Err(StorageError::NewerFormat { .. })
        ));
        assert_eq!(fs::read_to_string(&file).unwrap(), newer);

        fs::write(&file, "not json").unwrap();
        assert!(matches!(store.load(), Err(StorageError::Corrupted { .. })));
        store.save(InterfaceSettings::default()).unwrap();
        assert_eq!(store.load().unwrap(), InterfaceSettings::default());
    }

    #[test]
    fn a_file_whose_format_cannot_be_read_from_its_start_is_not_replaced() {
        let (_directory, data_dir, store) = store();
        let file = data_dir.root().join(SETTINGS_FILE);
        // A file of a later format that holds more than this client reads of a settings file.
        let later = format!(
            r#"{{"format_version":3,"scale_percent":110,"later":"{}"}}"#,
            "x".repeat(70 * 1024)
        );
        fs::write(&file, &later).unwrap();
        assert!(matches!(store.load(), Err(StorageError::TooLarge { .. })));
        assert!(matches!(
            store.save(InterfaceSettings::default()),
            Err(StorageError::TooLarge { .. })
        ));
        assert_eq!(fs::read_to_string(&file).unwrap(), later);
    }

    /// A file of version 1 has no key bindings: every action has its default. A change of
    /// the bindings keeps the interface settings, and the reverse (ADR 0777).
    #[test]
    fn a_file_of_version_1_has_the_default_bindings_and_each_change_keeps_the_other_part() {
        use crate::keybinds::KeyAction;
        let (_directory, data_dir, store) = store();
        let file = data_dir.root().join(SETTINGS_FILE);
        fs::write(
            &file,
            r#"{"format_version":1,"scale_percent":120,"server_book_collapsed":true,"theme":"dark"}"#,
        )
        .unwrap();
        assert_eq!(store.load_keybinds().unwrap(), Keybinds::default());
        let keys: crate::global_keys::KeyCombo = "control+alt+c".parse().unwrap();
        store
            .update_keybinds(|keybinds| {
                keybinds
                    .set(KeyAction::ToggleCamera, Some(keys.clone()))
                    .unwrap();
            })
            .unwrap();
        let text = fs::read_to_string(&file).unwrap();
        assert!(text.contains("\"format_version\": 2"), "{text}");
        assert!(text.contains("\"toggle_camera\""), "{text}");
        let settings = store.load().unwrap();
        assert_eq!(
            (
                settings.scale_percent,
                settings.server_book_collapsed,
                settings.theme
            ),
            (120, true, ThemeChoice::Dark)
        );
        store
            .update(|settings| settings.scale_percent = 100)
            .unwrap();
        assert_eq!(
            store
                .load_keybinds()
                .unwrap()
                .get(KeyAction::ToggleCamera)
                .keys,
            Some(keys)
        );
    }

    #[test]
    fn a_change_of_one_setting_keeps_what_another_program_wrote_of_the_others() {
        let (_directory, data_dir, first) = store();
        let second = SettingsStore::new(data_dir);
        // Both programs read the defaults. The first collapses the server book, and then the
        // second, which still holds the defaults, changes the scale.
        let (held_by_first, held_by_second) = (first.load().unwrap(), second.load().unwrap());
        assert_eq!(held_by_first, held_by_second);
        first
            .update(|settings| settings.server_book_collapsed = true)
            .unwrap();
        let written = second
            .update(|settings| settings.scale_percent = 125)
            .unwrap();
        let both = InterfaceSettings {
            scale_percent: 125,
            server_book_collapsed: true,
            theme: ThemeChoice::Light,
        };
        assert_eq!(written, both);
        assert_eq!(first.load().unwrap(), both);
        // A scale outside its range is written as the nearest bound.
        assert_eq!(
            first
                .update(|settings| settings.scale_percent = 10)
                .unwrap()
                .scale_percent,
            SCALE_MIN_PERCENT
        );
    }
}
