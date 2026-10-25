//! The emoji that the fonts of this installation do not draw, as the window measured them
//! (ADR 0758). The key names what the result depends on; the window measures again when
//! its key differs.

use serde::{Deserialize, Serialize};

use crate::paths::DataDir;
use crate::storage::{StorageError, Versioned};

/// One measurement: the emoji of the table that are not drawn whole and those drawn by a
/// font without colour. Every other emoji of the table is drawn in colour.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmojiMeasurement {
    pub key: String,
    pub broken: Vec<String>,
    pub text: Vec<String>,
}

const FILE: Versioned = Versioned {
    name: "emoji-fonts.json",
    kind: "emoji measurement",
    version: 1,
    migrations: &[],
    limit: 512 * 1024,
};

#[derive(Serialize, Deserialize)]
struct Stored {
    format_version: u32,
    #[serde(flatten)]
    measurement: EmojiMeasurement,
}

#[derive(Clone)]
pub struct EmojiFontsStore {
    data_dir: DataDir,
}

impl EmojiFontsStore {
    pub fn new(data_dir: DataDir) -> Self {
        Self { data_dir }
    }

    /// The kept measurement; `None` when there is none.
    pub fn load(&self) -> Result<Option<EmojiMeasurement>, StorageError> {
        Ok(FILE
            .load::<Stored>(self.data_dir.files())?
            .map(|stored| stored.measurement))
    }

    /// Replaces the kept measurement. A file of a newer format is not replaced.
    pub fn save(&self, measurement: &EmojiMeasurement) -> Result<(), StorageError> {
        let _lock = self.data_dir.lock()?;
        match self.load() {
            Ok(_) | Err(StorageError::Corrupted { .. }) => {}
            Err(error) => return Err(error),
        }
        FILE.store(
            self.data_dir.files(),
            &Stored {
                format_version: 1,
                measurement: measurement.clone(),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_measurement_is_kept_for_the_next_start() {
        let temp = tempfile::tempdir().unwrap();
        let dir = DataDir::open(temp.path().join("data")).unwrap();
        let store = EmojiFontsStore::new(dir.clone());
        assert_eq!(store.load().unwrap(), None);
        let measurement = EmojiMeasurement {
            key: "0.1.0 3944 fonts 1a probe Colour".to_owned(),
            broken: vec!["\u{1fae9}".to_owned()],
            text: vec!["\u{1f634}".to_owned()],
        };
        store.save(&measurement).unwrap();
        assert_eq!(EmojiFontsStore::new(dir).load().unwrap(), Some(measurement));
    }
}
