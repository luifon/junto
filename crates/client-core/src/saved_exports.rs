//! The exports of a server whose files this device saved, with the account that saved each
//! (ADR 0411, ADR 0710, ADR 0761). The administration counts the ready exports of the account
//! that the device has not saved; the file keeps that count correct after a restart.

use std::collections::{BTreeMap, BTreeSet};

use junto_protocol::ids::{AccountId, ExportId, ServerId};
use serde::{Deserialize, Serialize};

use crate::paths::{DataDir, SAVED_EXPORTS_FILE};
use crate::storage::{StorageError, Versioned};

pub const SAVED_EXPORTS_FORMAT_VERSION: u32 = 1;

/// The most exports that the file holds. A server keeps at most 10 exports of an account
/// (ADR 0411); the bound holds the exports of many servers and accounts.
pub const SAVED_EXPORTS_STORED: usize = 1024;

const SAVED_EXPORTS: Versioned = Versioned {
    name: SAVED_EXPORTS_FILE,
    kind: "saved exports",
    version: SAVED_EXPORTS_FORMAT_VERSION,
    migrations: &[],
    limit: 256 * 1024,
};

type Marks = BTreeMap<ServerId, BTreeMap<AccountId, BTreeSet<ExportId>>>;

/// One saved export: its server, the account that saved it, and the export.
pub type Mark = (ServerId, AccountId, ExportId);

/// The identifiers of the exports that this device saved, by server and account.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SavedExports {
    saved: Marks,
}

/// What the client knows about the exports of one server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    /// The client does not hold the server.
    Removed,
    /// No export list of the server is read now.
    Unknown,
    /// The export list of this account is read, with these exports.
    Loaded {
        account_id: AccountId,
        exports: Vec<ExportId>,
    },
}

impl SavedExports {
    pub fn contains(
        &self,
        server_id: ServerId,
        account_id: AccountId,
        export_id: ExportId,
    ) -> bool {
        self.saved
            .get(&server_id)
            .and_then(|accounts| accounts.get(&account_id))
            .is_some_and(|exports| exports.contains(&export_id))
    }

    /// Adds an export. Over [`SAVED_EXPORTS_STORED`] exports, the lowest identifiers are
    /// dropped: the identifier that a server gives an export starts with the time of its
    /// request.
    pub fn insert(&mut self, server_id: ServerId, account_id: AccountId, export_id: ExportId) {
        self.saved
            .entry(server_id)
            .or_default()
            .entry(account_id)
            .or_default()
            .insert(export_id);
        self.bound();
    }

    pub fn remove(&mut self, server_id: ServerId, account_id: AccountId, export_id: ExportId) {
        if let Some(accounts) = self.saved.get_mut(&server_id) {
            if let Some(exports) = accounts.get_mut(&account_id) {
                exports.remove(&export_id);
                if exports.is_empty() {
                    accounts.remove(&account_id);
                }
            }
            if accounts.is_empty() {
                self.saved.remove(&server_id);
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = Mark> + '_ {
        self.saved.iter().flat_map(|(server_id, accounts)| {
            accounts.iter().flat_map(move |(account_id, exports)| {
                exports
                    .iter()
                    .map(move |export_id| (*server_id, *account_id, *export_id))
            })
        })
    }

    pub fn len(&self) -> usize {
        self.saved
            .values()
            .flat_map(BTreeMap::values)
            .map(BTreeSet::len)
            .sum()
    }

    pub fn is_empty(&self) -> bool {
        self.saved.is_empty()
    }

    /// The marks that no longer name an export of the server: every mark of a server that
    /// the client does not hold, and each mark of an account whose loaded export list lacks
    /// the export. The marks of another account than the one whose list is loaded, and the
    /// marks of a server whose list is not read, stay. `listing` is asked once per server.
    pub fn stale(&self, listing: impl Fn(ServerId) -> Listing) -> Vec<Mark> {
        let mut stale = Vec::new();
        for (server_id, accounts) in &self.saved {
            let listing = listing(*server_id);
            for (account_id, exports) in accounts {
                for export_id in exports {
                    let gone = match &listing {
                        Listing::Removed => true,
                        Listing::Unknown => false,
                        Listing::Loaded {
                            account_id: listed_account,
                            exports: listed,
                        } => listed_account == account_id && !listed.contains(export_id),
                    };
                    if gone {
                        stale.push((*server_id, *account_id, *export_id));
                    }
                }
            }
        }
        stale
    }

    fn bound(&mut self) {
        for accounts in self.saved.values_mut() {
            accounts.retain(|_, exports| !exports.is_empty());
        }
        self.saved.retain(|_, accounts| !accounts.is_empty());
        let count = self.len();
        if count <= SAVED_EXPORTS_STORED {
            return;
        }
        let mut marks: Vec<(ExportId, ServerId, AccountId)> = self
            .iter()
            .map(|(server_id, account_id, export_id)| (export_id, server_id, account_id))
            .collect();
        marks.sort_unstable();
        self.saved.clear();
        for (export_id, server_id, account_id) in
            marks.into_iter().skip(count - SAVED_EXPORTS_STORED)
        {
            self.saved
                .entry(server_id)
                .or_default()
                .entry(account_id)
                .or_default()
                .insert(export_id);
        }
    }
}

#[derive(Serialize, Deserialize)]
struct SavedExportsFile {
    format_version: u32,
    saved: Marks,
}

#[derive(Debug, Clone)]
pub struct SavedExportsStore {
    data_dir: DataDir,
}

impl SavedExportsStore {
    pub fn new(data_dir: DataDir) -> Self {
        Self { data_dir }
    }

    /// The stored exports, or none when there is no file.
    pub fn load(&self) -> Result<SavedExports, StorageError> {
        let Some(file) = SAVED_EXPORTS.load::<SavedExportsFile>(self.data_dir.files())? else {
            return Ok(SavedExports::default());
        };
        let mut saved = SavedExports { saved: file.saved };
        saved.bound();
        Ok(saved)
    }

    /// Changes the stored exports in one step under the lock of the data directory, by the
    /// rules of the settings of the interface (ADR 0710). A file that is not a file of
    /// saved exports is replaced; a file of a newer format, or one that is larger than this
    /// client reads, stays, and the change fails. Returns what was written.
    pub fn update(
        &self,
        change: impl FnOnce(&mut SavedExports),
    ) -> Result<SavedExports, StorageError> {
        let _lock = self.data_dir.lock()?;
        let mut saved = match self.load() {
            Ok(saved) => saved,
            Err(StorageError::Corrupted { .. }) => SavedExports::default(),
            Err(other) => return Err(other),
        };
        change(&mut saved);
        saved.bound();
        SAVED_EXPORTS.store(
            self.data_dir.files(),
            &SavedExportsFile {
                format_version: SAVED_EXPORTS_FORMAT_VERSION,
                saved: saved.saved.clone(),
            },
        )?;
        Ok(saved)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    fn store() -> (tempfile::TempDir, DataDir, SavedExportsStore) {
        let directory = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(directory.path().join("junto")).unwrap();
        let store = SavedExportsStore::new(data_dir.clone());
        (directory, data_dir, store)
    }

    /// An identifier of an export in the order of the number, as the time of the request
    /// orders the identifiers that a server gives.
    fn export(number: u128) -> ExportId {
        ExportId::from_uuid(uuid::Uuid::from_u128(number))
    }

    #[test]
    fn saved_exports_are_written_and_read_back_by_another_store() {
        let (_directory, data_dir, store) = store();
        assert!(store.load().unwrap().is_empty());
        assert!(!data_dir.root().join(SAVED_EXPORTS_FILE).exists());

        let (harbor, acme) = (ServerId::generate(), ServerId::generate());
        let (ada, luca) = (AccountId::generate(), AccountId::generate());
        let (first, second, other) = (
            ExportId::generate(),
            ExportId::generate(),
            ExportId::generate(),
        );
        let written = store
            .update(|saved| {
                saved.insert(harbor, ada, first);
                saved.insert(harbor, luca, second);
                saved.insert(acme, ada, other);
            })
            .unwrap();
        assert_eq!(written.len(), 3);
        let read = SavedExportsStore::new(data_dir.clone()).load().unwrap();
        assert_eq!(read, written);
        assert!(read.contains(harbor, ada, first) && read.contains(harbor, luca, second));
        assert!(!read.contains(harbor, luca, first) && !read.contains(harbor, ada, second));
        assert!(read.contains(acme, ada, other) && !read.contains(acme, ada, first));
        let text = fs::read_to_string(data_dir.root().join(SAVED_EXPORTS_FILE)).unwrap();
        assert!(text.contains("\"format_version\": 1"), "{text}");
        let file: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(
            file["saved"][harbor.to_string()][ada.to_string()][0],
            first.to_string()
        );
    }

    #[test]
    fn a_removed_export_and_the_last_export_of_an_account_and_a_server_leave_the_file() {
        let (_directory, data_dir, store) = store();
        let (harbor, acme) = (ServerId::generate(), ServerId::generate());
        let (ada, luca) = (AccountId::generate(), AccountId::generate());
        let (kept, removed, of_luca, only) = (
            ExportId::generate(),
            ExportId::generate(),
            ExportId::generate(),
            ExportId::generate(),
        );
        store
            .update(|saved| {
                saved.insert(harbor, ada, kept);
                saved.insert(harbor, ada, removed);
                saved.insert(harbor, luca, of_luca);
                saved.insert(acme, ada, only);
            })
            .unwrap();
        let written = store
            .update(|saved| {
                saved.remove(harbor, ada, removed);
                saved.remove(harbor, luca, of_luca);
                saved.remove(acme, ada, only);
            })
            .unwrap();
        assert_eq!(
            written.iter().collect::<Vec<_>>(),
            [(harbor, ada, kept)],
            "{written:?}"
        );
        assert_eq!(store.load().unwrap(), written);
        let text = fs::read_to_string(data_dir.root().join(SAVED_EXPORTS_FILE)).unwrap();
        assert!(!text.contains(&acme.to_string()), "{text}");
        assert!(!text.contains(&luca.to_string()), "{text}");
    }

    #[test]
    fn the_list_of_one_account_does_not_put_away_the_marks_of_another_account() {
        let (harbor, acme, gone) = (
            ServerId::generate(),
            ServerId::generate(),
            ServerId::generate(),
        );
        let (ada, luca) = (AccountId::generate(), AccountId::generate());
        let mut saved = SavedExports::default();
        saved.insert(harbor, ada, export(1));
        saved.insert(harbor, ada, export(2));
        saved.insert(harbor, luca, export(3));
        saved.insert(harbor, luca, export(4));
        saved.insert(acme, ada, export(5));
        saved.insert(gone, ada, export(6));
        let stale = saved.stale(|server_id| {
            if server_id == harbor {
                Listing::Loaded {
                    account_id: luca,
                    exports: vec![export(4), export(1)],
                }
            } else if server_id == acme {
                Listing::Unknown
            } else {
                Listing::Removed
            }
        });
        assert_eq!(
            stale.iter().copied().collect::<BTreeSet<_>>(),
            BTreeSet::from([(harbor, luca, export(3)), (gone, ada, export(6))])
        );
        for (server_id, account_id, export_id) in stale {
            saved.remove(server_id, account_id, export_id);
        }
        assert!(saved.contains(harbor, ada, export(1)) && saved.contains(harbor, ada, export(2)));
        assert!(saved.contains(harbor, luca, export(4)) && saved.contains(acme, ada, export(5)));
        assert_eq!(saved.len(), 4);

        // The list of Ada is read again: her marks that it lacks are put away, and those of
        // Luca stay.
        let stale = saved.stale(|_| Listing::Loaded {
            account_id: ada,
            exports: vec![export(2)],
        });
        assert_eq!(
            stale.into_iter().collect::<BTreeSet<_>>(),
            BTreeSet::from([(harbor, ada, export(1)), (acme, ada, export(5))])
        );
    }

    #[test]
    fn the_file_holds_a_bounded_number_of_exports_and_keeps_the_newest() {
        let (_directory, data_dir, store) = store();
        let total = SAVED_EXPORTS_STORED as u128 + 9;
        let written = store
            .update(|saved| {
                for number in 1..=total {
                    saved.insert(ServerId::generate(), AccountId::generate(), export(number));
                }
            })
            .unwrap();
        assert_eq!(written.len(), SAVED_EXPORTS_STORED);
        let kept: BTreeSet<ExportId> = written.iter().map(|(_, _, export_id)| export_id).collect();
        assert_eq!(kept.first(), Some(&export(10)));
        assert_eq!(kept.last(), Some(&export(total)));
        let size = fs::metadata(data_dir.root().join(SAVED_EXPORTS_FILE))
            .unwrap()
            .len();
        assert!(size < SAVED_EXPORTS.limit, "{size}");
        assert_eq!(store.load().unwrap(), written);
    }

    #[test]
    fn a_file_with_more_exports_than_the_bound_is_read_with_the_newest() {
        let (_directory, data_dir, store) = store();
        let (server_id, account_id) = (ServerId::generate(), AccountId::generate());
        let total = SAVED_EXPORTS_STORED as u128 + 3;
        let file = SavedExportsFile {
            format_version: SAVED_EXPORTS_FORMAT_VERSION,
            saved: BTreeMap::from([(
                server_id,
                BTreeMap::from([(account_id, (1..=total).map(export).collect())]),
            )]),
        };
        fs::write(
            data_dir.root().join(SAVED_EXPORTS_FILE),
            serde_json::to_vec(&file).unwrap(),
        )
        .unwrap();
        let read = store.load().unwrap();
        assert_eq!(read.len(), SAVED_EXPORTS_STORED);
        assert!((1..=3).all(|number| !read.contains(server_id, account_id, export(number))));
        assert!((4..=total).all(|number| read.contains(server_id, account_id, export(number))));
    }

    #[test]
    fn a_file_of_a_newer_format_stays_and_an_unreadable_file_is_replaced() {
        let (_directory, data_dir, store) = store();
        let file = data_dir.root().join(SAVED_EXPORTS_FILE);
        let (server_id, account_id, export_id) = (
            ServerId::generate(),
            AccountId::generate(),
            ExportId::generate(),
        );
        let newer = r#"{"format_version":2,"later":true}"#;
        fs::write(&file, newer).unwrap();
        assert!(matches!(
            store.load(),
            Err(StorageError::NewerFormat { found: 2, .. })
        ));
        assert!(matches!(
            store.update(|saved| saved.insert(server_id, account_id, export_id)),
            Err(StorageError::NewerFormat { .. })
        ));
        assert_eq!(fs::read_to_string(&file).unwrap(), newer);

        // A list of exports directly under the server is not the form of version 1.
        let unreadable =
            format!(r#"{{"format_version":1,"saved":{{"{server_id}":["{export_id}"]}}}}"#);
        fs::write(&file, unreadable).unwrap();
        assert!(matches!(store.load(), Err(StorageError::Corrupted { .. })));
        let written = store
            .update(|saved| saved.insert(server_id, account_id, export_id))
            .unwrap();
        assert!(written.contains(server_id, account_id, export_id));
        assert_eq!(store.load().unwrap(), written);
    }

    #[test]
    fn a_file_over_the_size_limit_stays_and_the_change_fails() {
        let (_directory, data_dir, store) = store();
        let file = data_dir.root().join(SAVED_EXPORTS_FILE);
        let large = " ".repeat(usize::try_from(SAVED_EXPORTS.limit).unwrap() + 1);
        fs::write(&file, &large).unwrap();
        assert!(matches!(store.load(), Err(StorageError::TooLarge { .. })));
        assert!(matches!(
            store.update(|saved| {
                saved.insert(
                    ServerId::generate(),
                    AccountId::generate(),
                    ExportId::generate(),
                );
            }),
            Err(StorageError::TooLarge { .. })
        ));
        assert_eq!(fs::read_to_string(&file).unwrap(), large);
    }
}
