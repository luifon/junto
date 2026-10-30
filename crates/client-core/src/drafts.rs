//! Private, bounded desktop draft snapshots (ADR 0710).
use crate::{
    paths::DataDir,
    storage::{StorageError, Versioned},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

const LIMIT: u64 = 16 * 1024 * 1024;
const FILE: Versioned = Versioned {
    name: "drafts.json",
    kind: "drafts",
    version: 1,
    migrations: &[],
    limit: LIMIT,
};
#[derive(Serialize, Deserialize)]
struct Stored<T> {
    format_version: u32,
    drafts: T,
}
#[derive(Clone)]
pub struct DraftStore {
    data: DataDir,
}
impl DraftStore {
    pub fn new(data: DataDir) -> Self {
        Self { data }
    }
    pub fn load<T: DeserializeOwned>(&self) -> Result<Vec<T>, StorageError> {
        Ok(FILE
            .load::<Stored<Vec<T>>>(self.data.files())?
            .map_or_else(Vec::new, |stored| stored.drafts))
    }
    pub fn save<T: Serialize>(&self, drafts: &[T]) -> Result<(), StorageError> {
        let _lock = self.data.lock()?;
        match FILE.load::<Stored<Vec<serde_json::Value>>>(self.data.files()) {
            Ok(_) | Err(StorageError::Corrupted { .. }) => {}
            Err(error) => return Err(error),
        }
        let stored = Stored {
            format_version: 1,
            drafts,
        };
        let length = serde_json::to_vec_pretty(&stored)
            .map_err(|error| {
                StorageError::io(
                    "encode",
                    &self.data.root().join(FILE.name),
                    std::io::Error::other(error),
                )
            })?
            .len();
        if drafts.len() > 1000 || length as u64 + 1 > LIMIT {
            return Err(StorageError::TooLarge {
                path: self.data.root().join(FILE.name),
                limit: LIMIT,
            });
        }
        FILE.store(self.data.files(), &stored)
    }
    pub fn tidy(&self, kept: &std::collections::HashSet<String>) -> Result<(), StorageError> {
        let _lock = self.data.lock()?;
        let directory = self.data.files().subdirectory("draft-pictures")?;
        directory.remove_leftovers()?;
        for name in directory.names()? {
            let Some(name) = name.to_str() else {
                continue;
            };
            if name.len() == 64
                && name.bytes().all(|byte| byte.is_ascii_hexdigit())
                && !kept.contains(name)
            {
                directory.remove(name)?;
            }
        }
        Ok(())
    }
    pub fn save_picture(&self, bytes: &[u8]) -> Result<String, StorageError> {
        if bytes.len() > 256 * 1024 * 1024 {
            return Err(StorageError::TooLarge {
                path: self.data.root().join("draft-pictures"),
                limit: 256 * 1024 * 1024,
            });
        }
        let hash: String = Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        let _lock = self.data.lock()?;
        let directory = self.data.files().subdirectory("draft-pictures")?;
        if !directory.contains_private(&hash)? {
            directory.create(&hash, bytes)?;
        }
        Ok(hash)
    }
    pub fn picture(&self, hash: &str) -> Result<Option<Vec<u8>>, StorageError> {
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let bytes = self
            .data
            .files()
            .subdirectory("draft-pictures")?
            .read(hash, 256 * 1024 * 1024)?;
        if let Some(bytes) = &bytes {
            let actual: String = Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            if actual != hash {
                return Err(StorageError::Corrupted {
                    path: self.data.root().join("draft-pictures").join(hash),
                    kind: "draft picture",
                    detail: "content differs from its hash".to_owned(),
                });
            }
        }
        Ok(bytes)
    }
    pub fn remove_picture(&self, hash: &str) -> Result<(), StorageError> {
        if hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            self.data
                .files()
                .subdirectory("draft-pictures")?
                .remove(hash)?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_are_atomic_bounded_and_leave_newer_files_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let data = DataDir::open(directory.path().join("data")).unwrap();
        let store = DraftStore::new(data.clone());
        store.save(&["日本語 draft", "thread draft"]).unwrap();
        assert_eq!(
            store.load::<String>().unwrap(),
            ["日本語 draft", "thread draft"]
        );
        assert!(store.save(&vec!["x"; 1001]).is_err());
        assert_eq!(store.load::<String>().unwrap().len(), 2);
        let newer = r#"{"format_version":2,"drafts":[]}"#;
        std::fs::write(data.root().join("drafts.json"), newer).unwrap();
        assert!(matches!(
            store.save::<String>(&[]),
            Err(StorageError::NewerFormat { .. })
        ));
        assert_eq!(
            std::fs::read_to_string(data.root().join("drafts.json")).unwrap(),
            newer
        );
    }
    #[test]
    fn pasted_pictures_use_private_content_names_and_reject_paths() {
        let directory = tempfile::tempdir().unwrap();
        let store = DraftStore::new(DataDir::open(directory.path().join("data")).unwrap());
        let hash = store.save_picture(b"picture bytes").unwrap();
        assert_eq!(store.picture(&hash).unwrap().unwrap(), b"picture bytes");
        assert!(store.picture("../keys").unwrap().is_none());
        std::fs::write(
            store.data.root().join("draft-pictures").join(&hash),
            b"changed content",
        )
        .unwrap();
        assert!(matches!(
            store.picture(&hash),
            Err(StorageError::Corrupted { .. })
        ));
        store.remove_picture(&hash).unwrap();
        assert!(store.picture(&hash).unwrap().is_none());
        let hash = store.save_picture(b"orphan picture").unwrap();
        store.tidy(&std::collections::HashSet::new()).unwrap();
        assert!(store.picture(&hash).unwrap().is_none());
    }
}
