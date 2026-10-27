//! The settings of the interface that belong to one installation and to no server: the
//! interface scale, the theme, the form of the server book and the widths of the panels
//! (ADR 0703, ADR 0710).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::paths::{DataDir, SETTINGS_FILE};
use crate::storage::{StorageError, Versioned};

pub const SETTINGS_FORMAT_VERSION: u32 = 1;

/// The narrowest and the widest interface scale, in percent (ADR 0703).
pub const SCALE_MIN_PERCENT: u32 = 90;
pub const SCALE_MAX_PERCENT: u32 = 150;

const SETTINGS: Versioned = Versioned {
    name: SETTINGS_FILE,
    kind: "settings",
    version: SETTINGS_FORMAT_VERSION,
    migrations: &[],
    limit: 64 * 1024,
};

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

/// A panel beside the main area whose width the person sets by dragging its edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Panel {
    ServerBook,
    Channels,
    Thread,
    Pins,
    Notices,
    Search,
    Members,
    Transcript,
    /// The panel of the call beside its stage in the main window.
    Call,
    /// The panel of the call in the separate call window.
    CallWindow,
}

impl Panel {
    pub const ALL: [Self; 10] = [
        Self::ServerBook,
        Self::Channels,
        Self::Thread,
        Self::Pins,
        Self::Notices,
        Self::Search,
        Self::Members,
        Self::Transcript,
        Self::Call,
        Self::CallWindow,
    ];

    /// The narrowest and the widest width of the panel, in pixels of the design.
    pub const fn bounds(self) -> (u16, u16) {
        match self {
            Self::ServerBook => (160, 360),
            Self::Channels => (200, 420),
            Self::Call | Self::CallWindow => (280, 720),
            Self::Thread
            | Self::Pins
            | Self::Notices
            | Self::Search
            | Self::Members
            | Self::Transcript => (280, 720),
        }
    }

    const fn index(self) -> usize {
        self as usize
    }
}

/// The widths that the person gave the panels, in pixels of the design. A panel without
/// one has the width of the design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PanelWidths([Option<u16>; Panel::ALL.len()]);

impl PanelWidths {
    pub fn get(&self, panel: Panel) -> Option<u16> {
        self.0[panel.index()]
    }

    /// Keeps `width` for `panel`, within the bounds of the panel.
    pub fn set(&mut self, panel: Panel, width: u16) {
        let (least, most) = panel.bounds();
        self.0[panel.index()] = Some(width.clamp(least, most));
    }

    fn of(stored: &BTreeMap<Panel, u16>) -> Self {
        let mut widths = Self::default();
        for (panel, width) in stored {
            widths.set(*panel, *width);
        }
        widths
    }

    fn stored(&self) -> BTreeMap<Panel, u16> {
        Panel::ALL
            .into_iter()
            .filter_map(|panel| self.get(panel).map(|width| (panel, width)))
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceSettings {
    /// The interface scale in percent, from 90 to 150.
    pub scale_percent: u32,
    /// The server book shows icons only.
    pub server_book_collapsed: bool,
    pub theme: ThemeChoice,
    pub panel_widths: PanelWidths,
}

impl Default for InterfaceSettings {
    fn default() -> Self {
        Self {
            scale_percent: 100,
            server_book_collapsed: false,
            theme: ThemeChoice::default(),
            panel_widths: PanelWidths::default(),
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
    /// A panel that this client does not know is left out of the file at the next write.
    #[serde(default, deserialize_with = "known_panels")]
    panel_widths: BTreeMap<Panel, u16>,
}

/// The widths of the panels that this client knows; a panel of a later client is skipped.
fn known_panels<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<Panel, u16>, D::Error> {
    let all = BTreeMap::<String, u16>::deserialize(deserializer)?;
    Ok(all
        .into_iter()
        .filter_map(|(name, width)| {
            serde_json::from_value::<Panel>(serde_json::Value::String(name))
                .ok()
                .map(|panel| (panel, width))
        })
        .collect())
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
        let Some(file) = SETTINGS.load::<SettingsFile>(self.data_dir.files())? else {
            return Ok(InterfaceSettings::default());
        };
        Ok(InterfaceSettings {
            scale_percent: file
                .scale_percent
                .clamp(SCALE_MIN_PERCENT, SCALE_MAX_PERCENT),
            server_book_collapsed: file.server_book_collapsed,
            theme: file.theme,
            panel_widths: PanelWidths::of(&file.panel_widths),
        })
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
        let _lock = self.data_dir.lock()?;
        let mut settings = match self.load() {
            Ok(settings) => settings,
            Err(StorageError::Corrupted { .. }) => InterfaceSettings::default(),
            Err(other) => return Err(other),
        };
        change(&mut settings);
        settings.scale_percent = settings
            .scale_percent
            .clamp(SCALE_MIN_PERCENT, SCALE_MAX_PERCENT);
        self.write(settings)?;
        Ok(settings)
    }

    fn write(&self, settings: InterfaceSettings) -> Result<(), StorageError> {
        SETTINGS.store(
            self.data_dir.files(),
            &SettingsFile {
                format_version: SETTINGS_FORMAT_VERSION,
                scale_percent: settings
                    .scale_percent
                    .clamp(SCALE_MIN_PERCENT, SCALE_MAX_PERCENT),
                server_book_collapsed: settings.server_book_collapsed,
                theme: settings.theme,
                panel_widths: settings.panel_widths.stored(),
            },
        )
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

        let mut panel_widths = PanelWidths::default();
        panel_widths.set(Panel::Call, 420);
        panel_widths.set(Panel::ServerBook, 900);
        let chosen = InterfaceSettings {
            scale_percent: 125,
            server_book_collapsed: true,
            theme: ThemeChoice::Dark,
            panel_widths,
        };
        store.save(chosen).unwrap();
        assert_eq!(store.load().unwrap(), chosen);
        let loaded = store.load().unwrap().panel_widths;
        assert_eq!(loaded.get(Panel::Call), Some(420));
        // A width beyond the bounds of its panel is kept as the nearest bound.
        assert_eq!(
            loaded.get(Panel::ServerBook),
            Some(Panel::ServerBook.bounds().1)
        );
        assert_eq!(loaded.get(Panel::Thread), None);
        let text = fs::read_to_string(data_dir.root().join(SETTINGS_FILE)).unwrap();
        assert!(text.contains("\"format_version\": 1"), "{text}");
    }

    #[test]
    fn the_width_of_a_panel_that_this_client_does_not_know_is_skipped() {
        let (_directory, data_dir, store) = store();
        fs::write(
            data_dir.root().join(SETTINGS_FILE),
            r#"{"format_version":1,"scale_percent":100,"panel_widths":{"call":400,"later_panel":300,"thread":10}}"#,
        )
        .unwrap();
        let widths = store.load().unwrap().panel_widths;
        assert_eq!(widths.get(Panel::Call), Some(400));
        assert_eq!(widths.get(Panel::Thread), Some(Panel::Thread.bounds().0));
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
        let newer = r#"{"format_version":2,"scale_percent":110,"later":true}"#;
        fs::write(&file, newer).unwrap();
        assert!(matches!(
            store.load(),
            Err(StorageError::NewerFormat {
                found: 2,
                supported: 1,
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
            r#"{{"format_version":2,"scale_percent":110,"later":"{}"}}"#,
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
            panel_widths: PanelWidths::default(),
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
