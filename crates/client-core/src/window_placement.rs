//! Placement of the main and detached call windows, kept independently per installation.

use crate::paths::DataDir;
use crate::storage::{StorageError, Versioned};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowPlacement {
    pub display: Option<uuid::Uuid>,
    pub display_origin: Option<(i32, i32)>,
    pub maximized: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub width: u32,
    pub height: u32,
    pub keep_on_top: bool,
}
impl Default for WindowPlacement {
    fn default() -> Self {
        Self {
            display: None,
            display_origin: None,
            maximized: false,
            x: None,
            y: None,
            width: 680,
            height: 470,
            keep_on_top: false,
        }
    }
}

const CALL_FILE: Versioned = Versioned {
    name: "call-window.json",
    kind: "call window settings",
    version: 1,
    migrations: &[],
    limit: 16 * 1024,
};

const MAIN_FILE: Versioned = Versioned {
    name: "main-window.json",
    kind: "main window settings",
    version: 1,
    migrations: &[],
    limit: 16 * 1024,
};

impl WindowPlacement {
    pub fn main_window() -> Self {
        Self {
            width: 1440,
            height: 900,
            ..Self::default()
        }
    }
    fn bounded(mut self, main: bool) -> Self {
        self.width = self.width.clamp(if main { 720 } else { 480 }, 7680);
        self.height = self.height.clamp(if main { 480 } else { 320 }, 4320);
        self.display_origin = self
            .display_origin
            .map(|(x, y)| (x.clamp(-100_000, 100_000), y.clamp(-100_000, 100_000)));
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
    placement: WindowPlacement,
}

#[derive(Clone)]
pub struct WindowStore {
    data_dir: DataDir,
    main: bool,
}
impl WindowStore {
    pub fn new(data_dir: DataDir) -> Self {
        Self {
            data_dir,
            main: false,
        }
    }
    pub fn main_window(data_dir: DataDir) -> Self {
        Self {
            data_dir,
            main: true,
        }
    }
    fn file(&self) -> &'static Versioned {
        if self.main { &MAIN_FILE } else { &CALL_FILE }
    }
    pub fn load(&self) -> Result<WindowPlacement, StorageError> {
        Ok(self
            .file()
            .load::<Stored>(self.data_dir.files())?
            .map(|file| file.placement.bounded(self.main))
            .unwrap_or_else(|| {
                if self.main {
                    WindowPlacement::main_window()
                } else {
                    WindowPlacement::default()
                }
            }))
    }
    pub fn save(&self, settings: WindowPlacement) -> Result<(), StorageError> {
        let _lock = self.data_dir.lock()?;
        match self.load() {
            Ok(_) | Err(StorageError::Corrupted { .. }) => {}
            Err(error) => return Err(error),
        }
        self.file().store(
            self.data_dir.files(),
            &Stored {
                format_version: 1,
                placement: settings.bounded(self.main),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn main_and_call_windows_keep_separate_files_and_their_normal_restore_sizes() {
        let temp = tempfile::tempdir().unwrap();
        let directory = DataDir::open(temp.path().join("data")).unwrap();
        let main = WindowStore::main_window(directory.clone());
        let call = WindowStore::new(directory.clone());
        assert_eq!(main.load().unwrap(), WindowPlacement::main_window());
        assert_eq!(call.load().unwrap(), WindowPlacement::default());
        let choice = WindowPlacement {
            display: Some(uuid::Uuid::new_v4()),
            display_origin: Some((-1920, 0)),
            x: Some(-1800),
            y: Some(60),
            width: 1100,
            height: 700,
            maximized: true,
            ..WindowPlacement::main_window()
        };
        main.save(choice).unwrap();
        call.save(WindowPlacement {
            width: 800,
            ..WindowPlacement::default()
        })
        .unwrap();
        assert_eq!(main.load().unwrap(), choice);
        assert_eq!(call.load().unwrap().width, 800);
        let file = directory.root().join("main-window.json");
        let newer = r#"{"format_version":2,"future":"kept"}"#;
        std::fs::write(&file, newer).unwrap();
        assert!(matches!(
            main.save(choice),
            Err(StorageError::NewerFormat { .. })
        ));
        assert_eq!(std::fs::read_to_string(file).unwrap(), newer);
    }

    #[test]
    fn call_window_settings_survive_restart() {
        let temp = tempfile::tempdir().unwrap();
        let dir = DataDir::open(temp.path().join("data")).unwrap();
        let store = WindowStore::new(dir.clone());
        assert_eq!(store.load().unwrap(), WindowPlacement::default());
        let choice = WindowPlacement {
            display: None,
            x: Some(-700),
            y: Some(40),
            width: 960,
            height: 600,
            keep_on_top: true,
            display_origin: None,
            maximized: true,
        };
        store.save(choice).unwrap();
        assert_eq!(WindowStore::new(dir).load().unwrap(), choice);
    }
    #[test]
    fn call_window_settings_keep_the_display_with_its_relative_position() {
        let display = uuid::Uuid::new_v4();
        let choice: WindowPlacement = serde_json::from_value(serde_json::json!({
            "display": display, "x": 80, "y": 40, "width": 960, "height": 600,
            "keep_on_top": true
        }))
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let dir = DataDir::open(temp.path().join("data")).unwrap();
        let store = WindowStore::new(dir);
        store.save(choice).unwrap();
        let saved = serde_json::to_value(store.load().unwrap()).unwrap();
        assert_eq!(saved["display"], serde_json::json!(display));
        assert_eq!(saved["x"], 80);
        assert_eq!(saved["y"], 40);
    }

    #[test]
    fn call_window_settings_bound_sizes_and_preserve_newer_files() {
        let temp = tempfile::tempdir().unwrap();
        let dir = DataDir::open(temp.path().join("data")).unwrap();
        let store = WindowStore::new(dir.clone());
        store
            .save(WindowPlacement {
                width: 0,
                height: u32::MAX,
                ..WindowPlacement::default()
            })
            .unwrap();
        let held = store.load().unwrap();
        assert_eq!((held.width, held.height), (480, 4320));
        let file = dir.root().join("call-window.json");
        let newer = r#"{"format_version":2,"later":true}"#;
        std::fs::write(&file, newer).unwrap();
        assert!(matches!(
            store.save(WindowPlacement::default()),
            Err(StorageError::NewerFormat { .. })
        ));
        assert_eq!(std::fs::read_to_string(file).unwrap(), newer);
    }
}
