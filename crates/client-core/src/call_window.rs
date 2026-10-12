//! Placement of the detached call window, kept per installation.

use crate::paths::DataDir;
use crate::storage::{StorageError, Versioned};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CallWindowSettings {
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: u32,
    pub height: u32,
    pub keep_on_top: bool,
}
impl Default for CallWindowSettings {
    fn default() -> Self {
        Self {
            x: None,
            y: None,
            width: 680,
            height: 470,
            keep_on_top: false,
        }
    }
}

const FILE: Versioned = Versioned {
    name: "call-window.json",
    kind: "call window settings",
    version: 1,
    migrations: &[],
    limit: 16 * 1024,
};

impl CallWindowSettings {
    fn bounded(mut self) -> Self {
        self.width = self.width.clamp(480, 7680);
        self.height = self.height.clamp(320, 4320);
        match (self.x, self.y) {
            (Some(x), Some(y)) => {
                self.x = Some(x.clamp(-100_000, 100_000));
                self.y = Some(y.clamp(-100_000, 100_000));
            }
            _ => {
                self.x = None;
                self.y = None;
            }
        }
        self
    }
}

#[derive(Serialize, Deserialize)]
struct Stored {
    format_version: u32,
    #[serde(flatten)]
    placement: CallWindowSettings,
}

#[derive(Clone)]
pub struct CallWindowStore {
    data_dir: DataDir,
}
impl CallWindowStore {
    pub fn new(data_dir: DataDir) -> Self {
        Self { data_dir }
    }
    pub fn load(&self) -> Result<CallWindowSettings, StorageError> {
        Ok(FILE
            .load::<Stored>(self.data_dir.files())?
            .map(|file| file.placement.bounded())
            .unwrap_or_default())
    }
    pub fn save(&self, settings: CallWindowSettings) -> Result<(), StorageError> {
        let _lock = self.data_dir.lock()?;
        match self.load() {
            Ok(_) | Err(StorageError::Corrupted { .. }) => {}
            Err(error) => return Err(error),
        }
        FILE.store(
            self.data_dir.files(),
            &Stored {
                format_version: 1,
                placement: settings.bounded(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn call_window_settings_survive_restart() {
        let temp = tempfile::tempdir().unwrap();
        let dir = DataDir::open(temp.path().join("data")).unwrap();
        let store = CallWindowStore::new(dir.clone());
        assert_eq!(store.load().unwrap(), CallWindowSettings::default());
        let choice = CallWindowSettings {
            x: Some(-700),
            y: Some(40),
            width: 960,
            height: 600,
            keep_on_top: true,
        };
        store.save(choice).unwrap();
        assert_eq!(CallWindowStore::new(dir).load().unwrap(), choice);
    }
    #[test]
    fn call_window_settings_bound_sizes_and_preserve_newer_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir = DataDir::open(temp.path().join("data")).unwrap();
        let store = CallWindowStore::new(dir.clone());
        store
            .save(CallWindowSettings {
                width: 0,
                height: u32::MAX,
                ..CallWindowSettings::default()
            })
            .unwrap();
        let held = store.load().unwrap();
        assert_eq!((held.width, held.height), (480, 4320));
        let file = dir.root().join("call-window.json");
        let newer = r#"{"format_version":2,"later":true}"#;
        std::fs::write(&file, newer).unwrap();
        assert!(matches!(
            store.save(CallWindowSettings::default()),
            Err(StorageError::NewerFormat { .. })
        ));
        assert_eq!(std::fs::read_to_string(file).unwrap(), newer);
    }
}
