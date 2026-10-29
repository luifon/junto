//! The bounded emoji choices of this installation (ADR 0710, ADR 0740).

use serde::{Deserialize, Serialize};

use crate::paths::DataDir;
use crate::storage::{StorageError, Versioned};

pub const MOST_RECENT: usize = 24;

const FILE: Versioned = Versioned {
    name: "emoji-history.json",
    kind: "recent emoji",
    version: 1,
    migrations: &[],
    limit: 8 * 1024,
};

#[derive(Serialize, Deserialize)]
struct Stored {
    format_version: u32,
    emoji: Vec<String>,
}

fn normalized(value: &str) -> Option<&'static str> {
    (value.len() <= 64)
        .then(|| emojis::get(value))
        .flatten()
        .map(emojis::Emoji::as_str)
}

fn bounded(values: &[String]) -> Vec<String> {
    let mut found = Vec::new();
    for emoji in values.iter().filter_map(|value| normalized(value)) {
        if !found.iter().any(|value| value == emoji) {
            found.push(emoji.to_owned());
            if found.len() == MOST_RECENT {
                break;
            }
        }
    }
    found
}

/// Records one choice, newest first; an identical first choice changes nothing.
pub fn remember(values: &mut Vec<String>, emoji: &str) -> bool {
    let Some(emoji) = normalized(emoji) else {
        return false;
    };
    if values.first().is_some_and(|value| value == emoji) {
        return false;
    }
    values.retain(|value| value != emoji);
    values.insert(0, emoji.to_owned());
    values.truncate(MOST_RECENT);
    true
}

#[derive(Clone)]
pub struct EmojiHistoryStore {
    data_dir: DataDir,
}

impl EmojiHistoryStore {
    pub fn new(data_dir: DataDir) -> Self {
        Self { data_dir }
    }

    pub fn load(&self) -> Result<Vec<String>, StorageError> {
        Ok(FILE
            .load::<Stored>(self.data_dir.files())?
            .map_or_else(Vec::new, |stored| bounded(&stored.emoji)))
    }

    pub fn save(&self, emoji: &[String]) -> Result<(), StorageError> {
        let _lock = self.data_dir.lock()?;
        match self.load() {
            Ok(_) | Err(StorageError::Corrupted { .. }) => {}
            Err(error) => return Err(error),
        }
        FILE.store(
            self.data_dir.files(),
            &Stored {
                format_version: 1,
                emoji: bounded(emoji),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_are_bounded_deduplicated_and_kept_for_the_next_start() {
        let directory = tempfile::tempdir().unwrap();
        let data = DataDir::open(directory.path().join("data")).unwrap();
        let store = EmojiHistoryStore::new(data.clone());
        assert!(store.load().unwrap().is_empty());
        let choices: Vec<_> = emojis::iter().take(32).map(emojis::Emoji::as_str).collect();
        let mut recent = Vec::new();
        for emoji in &choices {
            assert!(remember(&mut recent, emoji));
        }
        assert_eq!(recent.len(), MOST_RECENT);
        assert!(!remember(&mut recent, choices[31]));
        assert!(remember(&mut recent, choices[20]));
        assert_eq!(recent[0], choices[20]);
        assert_eq!(
            recent.iter().filter(|emoji| *emoji == choices[20]).count(),
            1
        );
        assert!(!remember(&mut recent, "ordinary text"));
        store.save(&recent).unwrap();
        assert_eq!(EmojiHistoryStore::new(data).load().unwrap(), recent);
    }

    #[test]
    fn a_newer_history_is_left_untouched_and_unknown_choices_are_not_loaded() {
        let directory = tempfile::tempdir().unwrap();
        let data = DataDir::open(directory.path().join("data")).unwrap();
        let store = EmojiHistoryStore::new(data.clone());
        store
            .save(&["unknown".to_owned(), "🎉".to_owned(), "🎉".to_owned()])
            .unwrap();
        assert_eq!(store.load().unwrap(), ["🎉"]);
        let path = data.root().join("emoji-history.json");
        let newer = r#"{"format_version":2,"emoji":["🎉"]}"#;
        std::fs::write(&path, newer).unwrap();
        assert!(matches!(
            store.save(&[]),
            Err(StorageError::NewerFormat { .. })
        ));
        assert_eq!(std::fs::read_to_string(path).unwrap(), newer);
    }
}
