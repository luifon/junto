//! The server book: the saved servers of this client (ADR 0101, ADR 0710).

use std::collections::BTreeSet;
use std::fmt;

use junto_protocol::ids::{AccountId, DeviceId, JoinRequestId, ServerId};
use junto_protocol::server::ServerInfo;
use serde::{Deserialize, Deserializer, Serialize};

use crate::address::ServerAddress;
use crate::arrangement::{ArrangeError, Arrangement, Change};
use crate::paths::{BOOK_FILE, DataDir};
use crate::storage::{StorageError, Versioned};

pub const BOOK_FORMAT_VERSION: u32 = 7;

const BOOK: Versioned = Versioned {
    name: BOOK_FILE,
    kind: "server book",
    version: BOOK_FORMAT_VERSION,
    migrations: &[
        unchanged, unchanged, unchanged, unchanged, unchanged, unchanged,
    ],
    limit: 4 * 1024 * 1024,
};

/// Version 2 added the state `refused` of the profile preset, version 3 the moderation
/// notices that the person put away, version 4 the logout of the person, and version 5 the
/// memberships `inactive` and `deleted` (ADR 0121), version 6 the kept keys of removed
/// servers (ADR 0712), version 7 the arrangement of the person (ADR 0775). Every book of an
/// earlier version is a
/// book of the next one, with no notice put away and no logout; the version tells an older
/// client that it cannot read the file, so that it does not drop the field or the entry
/// when it writes the book.
fn unchanged(value: serde_json::Value) -> Result<serde_json::Value, String> {
    Ok(value)
}

const LONGEST_NAME: usize = 64;
const LONGEST_LOGO_PATH: usize = 512;
const LONGEST_CURSOR: usize = 512;
const MOST_API_VERSIONS: usize = 16;
/// How many moderation notices of one server the book remembers as put away: a page of
/// the server (ADR 0120).
pub const MOST_SHOWN_NOTICES: usize = 100;

/// When the client holds the event stream of a server (ADR 0107).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionMode {
    #[default]
    Always,
    WhenSelected,
}

/// What this device is on a server, as far as the client knows from its last contact.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Membership {
    #[default]
    None,
    /// A join request of this device. It is stored without an identifier before the
    /// request is sent, so that a client that stops in between asks the server for it.
    JoinRequested {
        join_request_id: Option<JoinRequestId>,
        name: String,
    },
    Member {
        account_id: AccountId,
        device_id: DeviceId,
    },
    Banned {
        account_id: Option<AccountId>,
    },
    /// The server no longer knows the key that was a device of an account.
    Removed,
    /// The account left the last workspace of which it was a member. The key stays, so that
    /// an invite or an approved join request returns the same account (ADR 0121).
    Inactive {
        account_id: AccountId,
    },
    /// The account was deleted on another device. The key no longer belongs to an account.
    Deleted {
        account_id: Option<AccountId>,
    },
}

/// The position of this device in the event stream of one server. The client stores it and
/// does not read it.
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct EventCursor(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("an event cursor has 1 to 512 bytes and no control character")]
pub struct InvalidCursor;

impl EventCursor {
    pub fn new(text: impl Into<String>) -> Result<Self, InvalidCursor> {
        let text = text.into();
        if text.is_empty() || text.len() > LONGEST_CURSOR || text.chars().any(char::is_control) {
            return Err(InvalidCursor);
        }
        Ok(Self(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for EventCursor {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "EventCursor({})", self.0)
    }
}

impl<'de> Deserialize<'de> for EventCursor {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

/// Whether the profile preset still has to be sent to the account that this device
/// created on a server (ADR 0105).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProfileSync {
    /// Nothing to send: the account was not created here, or the preset has no avatar.
    #[default]
    NotNeeded,
    /// The preset waits to be sent. `attempts` counts the tries that failed.
    Pending {
        attempts: u32,
    },
    Applied,
    /// The server refused the image itself. It is not sent again.
    Refused,
}

/// Names one entry of the book as a client read it. A server that is removed and added
/// again gets a new generation, so a change that was meant for the removed entry does not
/// reach the new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntryKey {
    pub server_id: ServerId,
    pub generation: u64,
}

/// One entry of the book and the account that this device has on its server. What the
/// client stores of a server belongs to one owner, and its key names that owner: a file,
/// an entry of a cache, a remembered choice and a prompt that waits (ADR 0745).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Owner {
    pub entry: EntryKey,
    pub account_id: AccountId,
}

/// One saved server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEntry {
    pub server_id: ServerId,
    /// A random number that the book gives the entry when it is added.
    pub generation: u64,
    pub address: ServerAddress,
    pub name: String,
    /// The path of the logo on the server. It is a reference, never a local path.
    pub logo_path: Option<String>,
    /// The API versions that the server reported at the last contact.
    pub api_versions: Vec<u32>,
    pub connection_mode: ConnectionMode,
    pub membership: Membership,
    pub last_cursor: Option<EventCursor>,
    #[serde(default)]
    pub profile_sync: ProfileSync,
    /// The private moderation notices of the account that the person put away on this
    /// device, the oldest first (ADR 0120). The server keeps a notice for 30 days.
    #[serde(default)]
    pub moderation_notices_shown: Vec<uuid::Uuid>,
    /// The person logged out of the account of the membership on this device. The client
    /// logs in again only when the person asks for it, also after a restart (ADR 0712).
    #[serde(default)]
    pub logged_out: bool,
}

impl ServerEntry {
    pub fn key(&self) -> EntryKey {
        EntryKey {
            server_id: self.server_id,
            generation: self.generation,
        }
    }

    /// A new entry from what the server at `address` says about itself.
    pub fn new(address: ServerAddress, info: &ServerInfo, connection_mode: ConnectionMode) -> Self {
        let mut entry = Self {
            server_id: info.server_id,
            generation: 0,
            address,
            name: String::new(),
            logo_path: None,
            api_versions: Vec::new(),
            connection_mode,
            membership: Membership::None,
            last_cursor: None,
            profile_sync: ProfileSync::NotNeeded,
            moderation_notices_shown: Vec::new(),
            logged_out: false,
        };
        entry.refresh(info);
        entry
    }

    /// Takes the name, the logo and the versions from a later answer of the same server.
    /// Text that the server sends is cut to what the client shows and stores.
    pub fn refresh(&mut self, info: &ServerInfo) {
        self.name = shown_name(&info.name, &self.address);
        self.logo_path = info
            .logo_path
            .as_deref()
            .filter(|path| path.len() <= LONGEST_LOGO_PATH)
            .filter(|path| self.address.logo_url(path).is_some())
            .map(str::to_owned);
        self.api_versions = known_versions(&info.api_versions);
    }

    fn check(&self) -> Result<(), String> {
        let id = self.server_id;
        if self.generation == 0 {
            return Err(format!("server {id} has no generation"));
        }
        if self.name.is_empty()
            || self.name.chars().count() > LONGEST_NAME
            || self.name.chars().any(char::is_control)
        {
            return Err(format!("server {id} has an invalid name"));
        }
        if let Some(path) = &self.logo_path
            && (path.len() > LONGEST_LOGO_PATH || self.address.logo_url(path).is_none())
        {
            return Err(format!("server {id} has an invalid logo path"));
        }
        if self.api_versions.len() > MOST_API_VERSIONS {
            return Err(format!("server {id} has too many API versions"));
        }
        if self.moderation_notices_shown.len() > MOST_SHOWN_NOTICES {
            return Err(format!("server {id} has too many shown moderation notices"));
        }
        Ok(())
    }
}

fn shown_name(raw: &str, address: &ServerAddress) -> String {
    let name: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(LONGEST_NAME)
        .collect();
    if name.trim().is_empty() {
        address.host().to_owned()
    } else {
        name.trim().to_owned()
    }
}

fn known_versions(reported: &[u32]) -> Vec<u32> {
    let unique: BTreeSet<u32> = reported.iter().copied().collect();
    unique.into_iter().take(MOST_API_VERSIONS).collect()
}

#[derive(Serialize, Deserialize)]
struct BookFile {
    format_version: u32,
    servers: Vec<ServerEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    kept_keys: Vec<KeptKey>,
    #[serde(default, skip_serializing_if = "Arrangement::is_empty")]
    arrangement: Arrangement,
}

impl BookFile {
    fn order(&self) -> Vec<ServerId> {
        self.servers.iter().map(|entry| entry.server_id).collect()
    }
}

/// How many kept keys the book records at most.
const MOST_KEPT_KEYS: usize = 64;

/// The key file of a server that was removed while a join request of this device could
/// still wait and the server did not confirm its withdrawal (ADR 0712). The key stays so
/// that an approval cannot make an account for a key that the device no longer holds. The
/// key is not deleted by time: an approval while it is kept makes an account that only this
/// key reaches. It goes when the server confirms that no request of it waits and no account
/// has it, or when the same server is added again and uses it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeptKey {
    pub server_id: ServerId,
    /// When the removal kept the key, in seconds since 1970.
    pub kept_unix: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum BookError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("the server {server_id} is already saved with the address {address}")]
    AlreadySaved {
        server_id: ServerId,
        address: ServerAddress,
    },
    #[error("the address {address} is already saved for the server {server_id}")]
    AddressTaken {
        server_id: ServerId,
        address: ServerAddress,
    },
    #[error("the server {server_id} is not saved")]
    NotSaved { server_id: ServerId },
    #[error(transparent)]
    Arrange(#[from] ArrangeError),
}

/// The server book of one data directory. Every change reads the file again under the lock
/// of the data directory, so two processes do not lose each other's changes.
#[derive(Debug, Clone)]
pub struct ServerBook {
    data_dir: DataDir,
}

impl ServerBook {
    pub fn new(data_dir: DataDir) -> Self {
        Self { data_dir }
    }

    pub fn entries(&self) -> Result<Vec<ServerEntry>, StorageError> {
        Ok(self.file()?.servers)
    }

    /// The arrangement of the person for the servers of the book (ADR 0775).
    pub fn arrangement(&self) -> Result<Arrangement, StorageError> {
        Ok(self.file()?.arrangement)
    }

    /// The kept keys of removed servers (ADR 0712).
    pub fn kept_keys(&self) -> Result<Vec<KeptKey>, StorageError> {
        Ok(self.file()?.kept_keys)
    }

    fn file(&self) -> Result<BookFile, StorageError> {
        let files = self.data_dir.files();
        let Some(file) = BOOK.load::<BookFile>(files)? else {
            return Ok(BookFile {
                format_version: BOOK_FORMAT_VERSION,
                servers: Vec::new(),
                kept_keys: Vec::new(),
                arrangement: Arrangement::default(),
            });
        };
        let corrupted = |detail: String| BOOK.corrupted(files, detail);
        let mut ids = BTreeSet::new();
        let mut addresses = BTreeSet::new();
        for entry in &file.servers {
            entry.check().map_err(corrupted)?;
            if !ids.insert(entry.server_id) || !addresses.insert(entry.address.as_str()) {
                return Err(corrupted(format!(
                    "server {} is listed twice",
                    entry.server_id
                )));
            }
        }
        if file.kept_keys.len() > MOST_KEPT_KEYS {
            return Err(corrupted("too many kept keys".to_owned()));
        }
        file.arrangement.check().map_err(corrupted)?;
        let mut file = file;
        file.arrangement = file.arrangement.normalised(&file.order());
        Ok(file)
    }

    pub fn entry(&self, server_id: ServerId) -> Result<Option<ServerEntry>, StorageError> {
        Ok(self
            .entries()?
            .into_iter()
            .find(|entry| entry.server_id == server_id))
    }

    /// Adds a server and returns the stored entry with its generation. A server identifier
    /// and an address are saved once each: a second address that reports a saved
    /// identifier is refused, so the key of a saved server is only ever used with the
    /// address that the user saved for it.
    pub fn add(&self, entry: ServerEntry) -> Result<ServerEntry, BookError> {
        self.add_after(entry, || Ok::<(), BookError>(()))
    }

    /// Like [`ServerBook::add`], after `check` succeeded under the lock of the book.
    pub(crate) fn add_after<E: From<BookError>>(
        &self,
        mut entry: ServerEntry,
        check: impl FnOnce() -> Result<(), E>,
    ) -> Result<ServerEntry, E> {
        let _lock = self.data_dir.lock().map_err(BookError::from)?;
        let added = self.change::<_, E>(|servers| {
            for saved in servers.iter() {
                if saved.server_id == entry.server_id {
                    return Err(BookError::AlreadySaved {
                        server_id: saved.server_id,
                        address: saved.address.clone(),
                    }
                    .into());
                }
                if saved.address == entry.address {
                    return Err(BookError::AddressTaken {
                        server_id: saved.server_id,
                        address: saved.address.clone(),
                    }
                    .into());
                }
            }
            check()?;
            entry.generation = new_generation().map_err(BookError::from)?;
            servers.push(entry.clone());
            Ok(entry)
        })?;
        // The server uses its kept key again: it is no longer only kept.
        let server_id = added.server_id;
        self.change_file(|file| {
            file.kept_keys.retain(|kept| kept.server_id != server_id);
            Ok::<(), BookError>(())
        })?;
        Ok(added)
    }

    /// Removes one entry and then runs `then`, both under the lock of the book. An entry
    /// of another generation is not removed.
    pub fn remove<E: From<BookError>>(
        &self,
        key: EntryKey,
        then: impl FnOnce() -> Result<(), E>,
    ) -> Result<ServerEntry, E> {
        let _lock = self.data_dir.lock().map_err(BookError::from)?;
        let removed = self.change(|servers| {
            let index = position(servers, key)?;
            Ok::<_, BookError>(servers.remove(index))
        })?;
        then()?;
        Ok(removed)
    }

    /// Like [`ServerBook::remove`], and records that the key of the server was kept at
    /// `kept_unix` (ADR 0712). The oldest record gives way when the book holds
    /// [`MOST_KEPT_KEYS`]; its key stays.
    pub fn remove_keeping_key(
        &self,
        key: EntryKey,
        kept_unix: i64,
    ) -> Result<ServerEntry, BookError> {
        self.change_file(|file| {
            let index = position(&file.servers, key)?;
            let removed = file.servers.remove(index);
            file.kept_keys
                .retain(|kept| kept.server_id != key.server_id);
            if file.kept_keys.len() >= MOST_KEPT_KEYS {
                file.kept_keys.remove(0);
            }
            file.kept_keys.push(KeptKey {
                server_id: key.server_id,
                kept_unix,
            });
            Ok(removed)
        })
    }

    /// Changes one entry and returns its new content. The identifier, the generation and
    /// the address of an entry do not change.
    pub fn update(
        &self,
        key: EntryKey,
        change: impl FnOnce(&mut ServerEntry),
    ) -> Result<ServerEntry, BookError> {
        self.transact(key, |entry| {
            change(entry);
            Ok::<(), BookError>(())
        })
        .map(|(entry, ())| entry)
    }

    /// Runs `work` on one entry under the lock of the book and writes the book when it
    /// succeeded. An entry of another generation is "not saved".
    pub(crate) fn transact<R, E: From<BookError>>(
        &self,
        key: EntryKey,
        work: impl FnOnce(&mut ServerEntry) -> Result<R, E>,
    ) -> Result<(ServerEntry, R), E> {
        self.change(|servers| {
            let index = position(servers, key)?;
            let entry = &mut servers[index];
            let address = entry.address.clone();
            let result = work(entry)?;
            entry.server_id = key.server_id;
            entry.generation = key.generation;
            entry.address = address;
            Ok((entry.clone(), result))
        })
    }

    /// Moves one entry to `position` of the list and returns the keys of all entries in
    /// the new order. A position past the end is the end. An entry of another generation
    /// is "not saved". The order of the list is the order of the file, so the content of
    /// no entry changes.
    pub fn move_entry(&self, key: EntryKey, position: usize) -> Result<Vec<EntryKey>, BookError> {
        self.reorder(key, |_| position)
    }

    /// Like [`ServerBook::move_entry`], with the position chosen under the lock of the
    /// book: `place` gets the other entries as the file holds them now and returns the
    /// position among them.
    pub(crate) fn reorder(
        &self,
        key: EntryKey,
        place: impl FnOnce(&[ServerEntry]) -> usize,
    ) -> Result<Vec<EntryKey>, BookError> {
        self.change(|servers| {
            let moved = servers.remove(position(servers, key)?);
            let place = place(servers).min(servers.len());
            servers.insert(place, moved);
            Ok(servers.iter().map(ServerEntry::key).collect())
        })
    }

    /// Makes one change of the arrangement under the lock of the book and returns the keys
    /// of all entries in the new order with the new arrangement. A change that names a
    /// server or a group that the file does not hold changes nothing.
    pub(crate) fn arrange(
        &self,
        change: Change,
    ) -> Result<(Vec<EntryKey>, Arrangement), BookError> {
        self.change_file(|file| {
            let mut order = file.order();
            file.arrangement.apply(&mut order, change)?;
            file.servers.sort_by_key(|entry| {
                order
                    .iter()
                    .position(|server| *server == entry.server_id)
                    .unwrap_or(usize::MAX)
            });
            Ok::<_, BookError>((
                file.servers.iter().map(ServerEntry::key).collect(),
                file.arrangement.clone(),
            ))
        })
    }

    /// True while the book has the entry that `key` names. The caller holds the lock.
    pub(crate) fn has(&self, key: EntryKey) -> Result<bool, StorageError> {
        Ok(self.entries()?.iter().any(|entry| entry.key() == key))
    }

    pub(crate) fn lock(&self) -> Result<crate::storage::DirectoryLock, StorageError> {
        self.data_dir.lock()
    }

    fn change<R, E: From<BookError>>(
        &self,
        change: impl FnOnce(&mut Vec<ServerEntry>) -> Result<R, E>,
    ) -> Result<R, E> {
        self.change_file(|file| change(&mut file.servers))
    }

    fn change_file<R, E: From<BookError>>(
        &self,
        change: impl FnOnce(&mut BookFile) -> Result<R, E>,
    ) -> Result<R, E> {
        let _lock = self.data_dir.lock().map_err(BookError::from)?;
        let mut file = self.file().map_err(BookError::from)?;
        let (servers, kept_keys, arrangement) = (
            file.servers.clone(),
            file.kept_keys.clone(),
            file.arrangement.clone(),
        );
        let result = change(&mut file)?;
        // A removed server takes its place in the arrangement with it.
        file.arrangement = file.arrangement.normalised(&file.order());
        if file.servers == servers && file.kept_keys == kept_keys && file.arrangement == arrangement
        {
            return Ok(result);
        }
        let files = self.data_dir.files();
        for entry in &file.servers {
            entry
                .check()
                .map_err(|detail| BookError::from(BOOK.corrupted(files, detail)))?;
        }
        file.format_version = BOOK_FORMAT_VERSION;
        BOOK.store(files, &file).map_err(BookError::from)?;
        Ok(result)
    }

    /// Moves a book that cannot be read out of the way, so that the client starts with an
    /// empty book. The key files stay. Returns the new path of the moved file.
    pub fn quarantine(&self, now_unix: i64) -> Result<std::path::PathBuf, StorageError> {
        let _lock = self.data_dir.lock()?;
        let name = format!("{BOOK_FILE}.corrupt-{now_unix}");
        self.data_dir.files().rename(BOOK_FILE, &name)?;
        Ok(self.data_dir.root().join(name))
    }
}

fn position(servers: &[ServerEntry], key: EntryKey) -> Result<usize, BookError> {
    servers
        .iter()
        .position(|entry| entry.key() == key)
        .ok_or(BookError::NotSaved {
            server_id: key.server_id,
        })
}

fn new_generation() -> Result<u64, StorageError> {
    loop {
        let generation = getrandom::u64().map_err(|_| StorageError::Io {
            action: "number",
            path: BOOK_FILE.into(),
            source: std::io::Error::other("no random bytes"),
        })?;
        // A JSON reader keeps whole numbers exactly up to 2^53.
        let generation = generation >> 11;
        if generation != 0 {
            return Ok(generation);
        }
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    pub(crate) fn info(name: &str) -> ServerInfo {
        ServerInfo {
            server_id: ServerId::generate(),
            name: name.to_owned(),
            logo_path: None,
            api_versions: vec![1],
        }
    }

    pub(crate) fn entry(address: &str, name: &str) -> ServerEntry {
        ServerEntry::new(
            ServerAddress::parse(address).unwrap(),
            &info(name),
            ConnectionMode::Always,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::testing::{entry, info};
    use super::*;
    use std::fs;

    fn book() -> (tempfile::TempDir, DataDir, ServerBook) {
        let directory = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(directory.path().join("junto")).unwrap();
        let book = ServerBook::new(data_dir.clone());
        (directory, data_dir, book)
    }

    #[test]
    fn an_empty_data_directory_has_an_empty_book() {
        let (_directory, data_dir, book) = book();
        assert!(book.entries().unwrap().is_empty());
        assert!(!data_dir.book_file().exists());
    }

    #[test]
    fn entries_are_stored_with_a_format_version_and_read_back_in_order() {
        let (_directory, data_dir, book) = book();
        let mut first = entry("https://acme.example.test", "Acme");
        first.connection_mode = ConnectionMode::WhenSelected;
        first.membership = Membership::Member {
            account_id: AccountId::generate(),
            device_id: DeviceId::generate(),
        };
        first.last_cursor = Some(EventCursor::new("0198f1c2:42").unwrap());
        let second = entry("http://127.0.0.1:8080", "Local");
        let first = book.add(first).unwrap();
        let second = book.add(second).unwrap();
        assert_ne!(first.generation, 0);
        assert_ne!(first.generation, second.generation);

        assert_eq!(book.entries().unwrap(), [first.clone(), second]);
        assert_eq!(book.entry(first.server_id).unwrap().unwrap(), first);

        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(data_dir.book_file()).unwrap()).unwrap();
        assert_eq!(stored["format_version"], BOOK_FORMAT_VERSION);
        assert_eq!(stored["servers"][0]["address"], "https://acme.example.test");
        assert_eq!(stored["servers"][0]["connection_mode"], "when_selected");
        assert_eq!(stored["servers"][0]["membership"]["state"], "member");
        assert_eq!(stored["servers"][0]["last_cursor"], "0198f1c2:42");
        assert_eq!(stored["servers"][1]["membership"]["state"], "none");
    }

    #[cfg(unix)]
    #[test]
    fn the_book_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let (_directory, data_dir, book) = book();
        book.add(entry("https://acme.example.test", "Acme"))
            .unwrap();
        let mode = fs::metadata(data_dir.book_file())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn a_server_identifier_and_an_address_are_saved_once() {
        let (_directory, _data_dir, book) = book();
        let saved = book
            .add(entry("https://acme.example.test", "Acme"))
            .unwrap();

        let mut same_id_elsewhere = entry("https://other.example.test", "Copy");
        same_id_elsewhere.server_id = saved.server_id;
        assert!(matches!(
            book.add(same_id_elsewhere),
            Err(BookError::AlreadySaved { server_id, .. }) if server_id == saved.server_id
        ));
        assert!(matches!(
            book.add(entry("https://acme.example.test", "Other")),
            Err(BookError::AddressTaken { server_id, .. }) if server_id == saved.server_id
        ));
        assert_eq!(book.entries().unwrap(), [saved]);
    }

    #[test]
    fn an_inactive_and_a_deleted_membership_are_stored_and_read_back() {
        let (_directory, data_dir, book) = book();
        let account_id = AccountId::generate();
        let first = book
            .add(entry("https://acme.example.test", "Acme"))
            .unwrap();
        let second = book
            .add(entry("https://globex.example.test", "Globex"))
            .unwrap();
        book.update(first.key(), |entry| {
            entry.membership = Membership::Inactive { account_id };
        })
        .unwrap();
        book.update(second.key(), |entry| {
            entry.membership = Membership::Deleted {
                account_id: Some(account_id),
            };
        })
        .unwrap();
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(data_dir.book_file()).unwrap()).unwrap();
        assert_eq!(stored["format_version"], BOOK_FORMAT_VERSION);
        assert_eq!(stored["servers"][0]["membership"]["state"], "inactive");
        assert_eq!(stored["servers"][1]["membership"]["state"], "deleted");
        let entries = book.entries().unwrap();
        assert_eq!(entries[0].membership, Membership::Inactive { account_id });
        assert_eq!(
            entries[1].membership,
            Membership::Deleted {
                account_id: Some(account_id)
            }
        );
    }

    #[test]
    fn an_update_keeps_the_identifier_and_the_address() {
        let (_directory, _data_dir, book) = book();
        let saved = book
            .add(entry("https://acme.example.test", "Acme"))
            .unwrap();
        let updated = book
            .update(saved.key(), |entry| {
                entry.server_id = ServerId::generate();
                entry.generation += 1;
                entry.address = ServerAddress::parse("https://other.example.test").unwrap();
                entry.membership = Membership::Removed;
            })
            .unwrap();
        assert_eq!(updated.key(), saved.key());
        assert_eq!(updated.address, saved.address);
        assert_eq!(updated.membership, Membership::Removed);
        assert_eq!(book.entries().unwrap(), [updated]);

        let unknown = EntryKey {
            server_id: ServerId::generate(),
            generation: saved.generation,
        };
        assert!(matches!(
            book.update(unknown, |_| {}),
            Err(BookError::NotSaved { server_id }) if server_id == unknown.server_id
        ));
        assert!(matches!(
            book.remove(unknown, || Ok::<(), BookError>(())),
            Err(BookError::NotSaved { .. })
        ));
        assert_eq!(
            book.remove(saved.key(), || Ok::<(), BookError>(()))
                .unwrap()
                .server_id,
            saved.server_id
        );
        assert!(book.entries().unwrap().is_empty());
    }

    #[test]
    fn a_book_of_format_version_1_is_read_and_written_in_the_current_version() {
        let (_directory, data_dir, book) = book();
        let written = br#"{"format_version":1,"servers":[
            {"server_id":"0198f1c2-7d3a-7b1e-9c55-000000000001","generation":7,
             "address":"https://acme.example.test","name":"Acme","logo_path":null,
             "api_versions":[1],"connection_mode":"always",
             "membership":{"state":"member",
                           "account_id":"0198f1c2-7d3a-7b1e-9c55-0000000000aa",
                           "device_id":"0198f1c2-7d3a-7b1e-9c55-0000000000bb"},
             "last_cursor":"0198f1c2:42",
             "profile_sync":{"state":"pending","attempts":3}},
            {"server_id":"0198f1c2-7d3a-7b1e-9c55-000000000002","generation":8,
             "address":"http://127.0.0.1:8080","name":"Local","logo_path":"/api/v1/logo",
             "api_versions":[1],"connection_mode":"when_selected",
             "membership":{"state":"none"},"last_cursor":null}]}"#;
        fs::write(data_dir.book_file(), written).unwrap();
        let entries = book.entries().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].generation, 7);
        assert_eq!(entries[0].address.as_str(), "https://acme.example.test");
        assert!(matches!(entries[0].membership, Membership::Member { .. }));
        assert_eq!(
            entries[0].profile_sync,
            ProfileSync::Pending { attempts: 3 }
        );
        assert_eq!(entries[1].address.as_str(), "http://127.0.0.1:8080");
        assert_eq!(entries[1].profile_sync, ProfileSync::NotNeeded);

        book.update(entries[0].key(), |entry| {
            entry.profile_sync = ProfileSync::Refused;
        })
        .unwrap();
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(data_dir.book_file()).unwrap()).unwrap();
        assert_eq!(stored["format_version"], BOOK_FORMAT_VERSION);
        assert_eq!(stored["servers"][0]["profile_sync"]["state"], "refused");
        assert_eq!(stored["servers"][1]["address"], "http://127.0.0.1:8080");
        assert_eq!(book.entries().unwrap().len(), 2);
    }

    #[test]
    fn the_moderation_notices_put_away_are_kept_and_a_book_of_version_2_has_none() {
        let (_directory, data_dir, book) = book();
        let written = br#"{"format_version":2,"servers":[
            {"server_id":"0198f1c2-7d3a-7b1e-9c55-000000000001","generation":7,
             "address":"https://acme.example.test","name":"Acme","logo_path":null,
             "api_versions":[1],"connection_mode":"always",
             "membership":{"state":"none"},"last_cursor":null}]}"#;
        fs::write(data_dir.book_file(), written).unwrap();
        let entries = book.entries().unwrap();
        assert!(entries[0].moderation_notices_shown.is_empty());
        let notice = uuid::Uuid::now_v7();
        book.update(entries[0].key(), |entry| {
            entry.moderation_notices_shown.push(notice);
        })
        .unwrap();
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(data_dir.book_file()).unwrap()).unwrap();
        assert_eq!(stored["format_version"], BOOK_FORMAT_VERSION);
        assert_eq!(
            stored["servers"][0]["moderation_notices_shown"][0],
            notice.to_string()
        );
        assert_eq!(
            book.entries().unwrap()[0].moderation_notices_shown,
            [notice]
        );
        // More than the bound is an entry that the book does not hold.
        let mut over = book.entries().unwrap()[0].clone();
        over.moderation_notices_shown = (0..=MOST_SHOWN_NOTICES)
            .map(|_| uuid::Uuid::now_v7())
            .collect();
        assert!(over.check().is_err());
    }

    #[test]
    fn a_logout_of_the_person_is_kept_and_a_book_of_version_3_has_none() {
        let (_directory, data_dir, book) = book();
        let written = br#"{"format_version":3,"servers":[
            {"server_id":"0198f1c2-7d3a-7b1e-9c55-000000000001","generation":7,
             "address":"https://acme.example.test","name":"Acme","logo_path":null,
             "api_versions":[1],"connection_mode":"always",
             "membership":{"state":"member",
                           "account_id":"0198f1c2-7d3a-7b1e-9c55-0000000000aa",
                           "device_id":"0198f1c2-7d3a-7b1e-9c55-0000000000bb"},
             "last_cursor":null,"moderation_notices_shown":[]}]}"#;
        fs::write(data_dir.book_file(), written).unwrap();
        let entries = book.entries().unwrap();
        assert!(!entries[0].logged_out);
        book.update(entries[0].key(), |entry| entry.logged_out = true)
            .unwrap();
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(data_dir.book_file()).unwrap()).unwrap();
        assert_eq!(stored["format_version"], BOOK_FORMAT_VERSION);
        assert_eq!(stored["servers"][0]["logged_out"], true);
        assert!(book.entries().unwrap()[0].logged_out);
    }

    #[test]
    fn every_state_of_the_preset_is_stored_and_read_back() {
        let (_directory, data_dir, book) = book();
        let saved = book
            .add(entry("https://acme.example.test", "Acme"))
            .unwrap();
        for (state, written) in [
            (ProfileSync::NotNeeded, r#"{"state":"not_needed"}"#),
            (
                ProfileSync::Pending { attempts: 2 },
                r#"{"state":"pending","attempts":2}"#,
            ),
            (ProfileSync::Applied, r#"{"state":"applied"}"#),
            (ProfileSync::Refused, r#"{"state":"refused"}"#),
        ] {
            book.update(saved.key(), |entry| entry.profile_sync = state)
                .unwrap();
            assert_eq!(book.entries().unwrap()[0].profile_sync, state);
            let stored: serde_json::Value =
                serde_json::from_slice(&fs::read(data_dir.book_file()).unwrap()).unwrap();
            let expected: serde_json::Value = serde_json::from_str(written).unwrap();
            assert_eq!(stored["servers"][0]["profile_sync"], expected);
        }
    }

    #[test]
    fn a_change_for_a_removed_entry_does_not_reach_the_entry_that_replaced_it() {
        let (_directory, _data_dir, book) = book();
        let first = book
            .add(entry("https://acme.example.test", "Acme"))
            .unwrap();
        book.remove(first.key(), || Ok::<(), BookError>(()))
            .unwrap();
        let mut again = entry("https://acme.example.test", "Acme");
        again.server_id = first.server_id;
        let second = book.add(again).unwrap();
        assert_ne!(second.generation, first.generation);

        assert!(matches!(
            book.update(first.key(), |entry| entry.membership = Membership::Removed),
            Err(BookError::NotSaved { .. })
        ));
        let mut ran = false;
        assert!(matches!(
            book.remove(first.key(), || {
                ran = true;
                Ok::<(), BookError>(())
            }),
            Err(BookError::NotSaved { .. })
        ));
        assert!(!ran);
        assert_eq!(book.entries().unwrap(), [second]);
    }

    #[test]
    fn the_step_after_a_removal_runs_while_the_book_is_locked() {
        let (_directory, data_dir, book) = book();
        let saved = book
            .add(entry("https://acme.example.test", "Acme"))
            .unwrap();
        let other = ServerBook::new(DataDir::open(data_dir.root()).unwrap());
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut adder = None;
        book.remove(saved.key(), || {
            adder = Some(std::thread::spawn(move || {
                other
                    .add(entry("https://again.example.test", "Again"))
                    .unwrap();
                sender.send(()).unwrap();
            }));
            let added_meanwhile = receiver
                .recv_timeout(std::time::Duration::from_millis(200))
                .is_ok();
            assert!(!added_meanwhile);
            Ok::<(), BookError>(())
        })
        .unwrap();
        adder.unwrap().join().unwrap();
        assert_eq!(book.entries().unwrap().len(), 1);
    }

    #[test]
    fn two_handles_on_one_data_directory_do_not_lose_changes() {
        let (_directory, data_dir, book) = book();
        let other = ServerBook::new(data_dir);
        let workers: Vec<_> = (0..8)
            .map(|index| {
                let book = if index % 2 == 0 {
                    book.clone()
                } else {
                    other.clone()
                };
                std::thread::spawn(move || {
                    let address = format!("https://server-{index}.example.test");
                    book.add(entry(&address, "Acme")).unwrap();
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(book.entries().unwrap().len(), 8);
    }

    fn three(book: &ServerBook) -> [ServerEntry; 3] {
        ["acme", "globex", "initech"].map(|name| {
            book.add(entry(&format!("https://{name}.example.test"), name))
                .unwrap()
        })
    }

    fn stored_order(data_dir: &DataDir) -> Vec<String> {
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(data_dir.book_file()).unwrap()).unwrap();
        stored["servers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|server| server["server_id"].as_str().unwrap().to_owned())
            .collect()
    }

    fn ids(entries: &[&ServerEntry]) -> Vec<String> {
        entries
            .iter()
            .map(|entry| entry.server_id.to_string())
            .collect()
    }

    #[test]
    fn an_entry_moves_up_and_down_and_the_file_holds_the_order() {
        let (_directory, data_dir, book) = book();
        let [first, second, third] = three(&book);

        let order = book.move_entry(third.key(), 0).unwrap();
        assert_eq!(order, [third.key(), first.key(), second.key()]);
        assert_eq!(stored_order(&data_dir), ids(&[&third, &first, &second]));
        assert_eq!(
            book.entries().unwrap(),
            [third.clone(), first.clone(), second.clone()]
        );

        let order = book.move_entry(third.key(), 1).unwrap();
        assert_eq!(order, [first.key(), third.key(), second.key()]);
        assert_eq!(stored_order(&data_dir), ids(&[&first, &third, &second]));

        let order = book.move_entry(first.key(), 2).unwrap();
        assert_eq!(order, [third.key(), second.key(), first.key()]);
        assert_eq!(stored_order(&data_dir), ids(&[&third, &second, &first]));

        // The place that the entry has already.
        let order = book.move_entry(second.key(), 1).unwrap();
        assert_eq!(order, [third.key(), second.key(), first.key()]);
        assert_eq!(
            book.entries().unwrap(),
            [third.clone(), second.clone(), first.clone()]
        );
    }

    #[test]
    fn the_arrangement_is_written_with_the_order_and_a_book_of_version_6_has_none() {
        use crate::arrangement::{GroupColour, Item, Spot};
        use junto_protocol::ids::WorkspaceId;

        let (_directory, data_dir, book) = book();
        let written = br#"{"format_version":6,"servers":[
            {"server_id":"0198f1c2-7d3a-7b1e-9c55-000000000001","generation":7,
             "address":"https://acme.example.test","name":"Acme","logo_path":null,
             "api_versions":[1],"connection_mode":"always",
             "membership":{"state":"none"},"last_cursor":null},
            {"server_id":"0198f1c2-7d3a-7b1e-9c55-000000000002","generation":8,
             "address":"https://globex.example.test","name":"Globex","logo_path":null,
             "api_versions":[1],"connection_mode":"always",
             "membership":{"state":"none"},"last_cursor":null},
            {"server_id":"0198f1c2-7d3a-7b1e-9c55-000000000003","generation":9,
             "address":"https://initech.example.test","name":"Initech","logo_path":null,
             "api_versions":[1],"connection_mode":"always",
             "membership":{"state":"none"},"last_cursor":null}],
            "kept_keys":[]}"#;
        fs::write(data_dir.book_file(), written).unwrap();
        assert!(book.arrangement().unwrap().is_empty());
        let [first, second, third] = <[ServerEntry; 3]>::try_from(book.entries().unwrap())
            .unwrap_or_else(|_| panic!("three entries"));

        // The third server dropped on the first: a group of the two at the first place.
        let (order, arrangement) = book
            .arrange(Change::GroupWith {
                server: third.server_id,
                target: first.server_id,
            })
            .unwrap();
        assert_eq!(order, [first.key(), third.key(), second.key()]);
        assert_eq!(stored_order(&data_dir), ids(&[&first, &third, &second]));
        let group = arrangement.group_of(first.server_id).unwrap();
        book.arrange(Change::RenameGroup {
            group,
            name: "Work".to_owned(),
        })
        .unwrap();
        book.arrange(Change::ColourGroup {
            group,
            colour: GroupColour::Olive,
        })
        .unwrap();
        book.arrange(Change::CollapseGroup {
            group,
            collapsed: true,
        })
        .unwrap();
        book.arrange(Change::CollapseWorkspaces {
            server: second.server_id,
            collapsed: true,
        })
        .unwrap();
        let workspaces = [WorkspaceId::generate(), WorkspaceId::generate()];
        book.arrange(Change::MoveWorkspace {
            server: second.server_id,
            workspace: workspaces[1],
            to: crate::arrangement::WorkspaceSpot::Before(workspaces[0]),
            shown: workspaces.to_vec(),
        })
        .unwrap();

        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(data_dir.book_file()).unwrap()).unwrap();
        assert_eq!(stored["format_version"], BOOK_FORMAT_VERSION);
        assert_eq!(stored["arrangement"]["groups"][0]["name"], "Work");
        assert_eq!(stored["arrangement"]["groups"][0]["colour"], "olive");
        assert_eq!(stored["arrangement"]["groups"][0]["collapsed"], true);

        // A second reader of the file finds the same arrangement.
        let again = ServerBook::new(DataDir::open(data_dir.root()).unwrap());
        let read = again.arrangement().unwrap();
        let held = read.group(group).unwrap();
        assert_eq!(
            (held.name.as_str(), held.colour, held.collapsed),
            ("Work", GroupColour::Olive, true)
        );
        assert_eq!(read.group_of(third.server_id), Some(group));
        assert_eq!(read.group_of(second.server_id), None);
        assert!(read.workspaces_collapsed(second.server_id));
        assert_eq!(
            read.place(second.server_id).unwrap().workspace_order,
            [workspaces[1], workspaces[0]]
        );

        // A server that leaves the book takes its place in a group with it; the group
        // that it leaves empty goes.
        book.arrange(Change::MoveServer {
            server: third.server_id,
            to: Spot::After(Item::Server(second.server_id)),
        })
        .unwrap();
        book.remove(first.key(), || Ok::<(), BookError>(()))
            .unwrap();
        let read = book.arrangement().unwrap();
        assert!(read.groups.is_empty());
        assert_eq!(read.group_of(first.server_id), None);
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(data_dir.book_file()).unwrap()).unwrap();
        assert!(stored["arrangement"]["groups"].is_null());

        // A change that names a server that the book does not hold changes nothing.
        assert!(matches!(
            book.arrange(Change::NewGroup {
                server: first.server_id,
                name: String::new(),
            }),
            Err(BookError::Arrange(_))
        ));
    }

    #[test]
    fn an_arrangement_outside_its_bounds_is_a_corrupted_book() {
        let (_directory, data_dir, book) = book();
        let written = br#"{"format_version":7,"servers":[
            {"server_id":"0198f1c2-7d3a-7b1e-9c55-000000000001","generation":7,
             "address":"https://acme.example.test","name":"Acme","logo_path":null,
             "api_versions":[1],"connection_mode":"always",
             "membership":{"state":"none"},"last_cursor":null}],
            "arrangement":{"servers":[
              {"server_id":"0198f1c2-7d3a-7b1e-9c55-000000000001","workspaces_collapsed":true},
              {"server_id":"0198f1c2-7d3a-7b1e-9c55-000000000001"}]}}"#;
        fs::write(data_dir.book_file(), written).unwrap();
        assert!(matches!(
            book.entries(),
            Err(StorageError::Corrupted { .. })
        ));
    }

    #[test]
    fn a_position_past_the_end_is_the_end() {
        let (_directory, data_dir, book) = book();
        let [first, second, third] = three(&book);
        for position in [3, 4, usize::MAX] {
            let order = book.move_entry(first.key(), position).unwrap();
            assert_eq!(order, [second.key(), third.key(), first.key()]);
            assert_eq!(stored_order(&data_dir), ids(&[&second, &third, &first]));
        }
        let (_other_directory, _, single) = self::book();
        let only = single
            .add(entry("https://acme.example.test", "Acme"))
            .unwrap();
        assert_eq!(single.move_entry(only.key(), 7).unwrap(), [only.key()]);
    }

    #[test]
    fn an_entry_that_is_gone_is_not_moved() {
        let (_directory, data_dir, book) = book();
        let [first, second, third] = three(&book);
        let unknown = EntryKey {
            server_id: ServerId::generate(),
            generation: first.generation,
        };
        assert!(matches!(
            book.move_entry(unknown, 0),
            Err(BookError::NotSaved { server_id }) if server_id == unknown.server_id
        ));

        book.remove(third.key(), || Ok::<(), BookError>(()))
            .unwrap();
        let mut again = entry("https://initech.example.test", "initech");
        again.server_id = third.server_id;
        let replaced = book.add(again).unwrap();
        assert!(matches!(
            book.move_entry(third.key(), 0),
            Err(BookError::NotSaved { server_id }) if server_id == third.server_id
        ));
        assert_eq!(stored_order(&data_dir), ids(&[&first, &second, &replaced]));
        assert_eq!(
            book.move_entry(replaced.key(), 0).unwrap(),
            [replaced.key(), first.key(), second.key()]
        );
    }

    #[test]
    fn a_move_keeps_a_server_that_another_program_added() {
        let (_directory, data_dir, book) = book();
        let [first, second, third] = three(&book);
        // This program read the book before the other one changed it.
        assert_eq!(book.entries().unwrap().len(), 3);
        let other = ServerBook::new(DataDir::open(data_dir.root()).unwrap());
        let added = other
            .add(entry("https://hooli.example.test", "hooli"))
            .unwrap();
        other
            .update(second.key(), |entry| {
                entry.connection_mode = ConnectionMode::WhenSelected;
            })
            .unwrap();

        let order = book.move_entry(third.key(), 0).unwrap();
        assert_eq!(order, [third.key(), first.key(), second.key(), added.key()]);
        assert_eq!(
            stored_order(&data_dir),
            ids(&[&third, &first, &second, &added])
        );
        let entries = other.entries().unwrap();
        assert_eq!(entries[3], added);
        assert_eq!(entries[2].connection_mode, ConnectionMode::WhenSelected);

        // The position is chosen from what the file holds under the lock.
        let order = book
            .reorder(first.key(), |others| {
                assert_eq!(
                    others.iter().map(ServerEntry::key).collect::<Vec<_>>(),
                    [third.key(), second.key(), added.key()]
                );
                others.len()
            })
            .unwrap();
        assert_eq!(order, [third.key(), second.key(), added.key(), first.key()]);
    }

    #[test]
    fn text_from_a_server_is_cut_to_what_the_client_stores() {
        let address = ServerAddress::parse("https://acme.example.test").unwrap();
        let mut reported = info("  Ac\u{0007}me\n  ");
        reported.logo_path = Some("/api/v1/logo".to_owned());
        reported.api_versions = vec![2, 1, 2, 1];
        let entry = ServerEntry::new(address.clone(), &reported, ConnectionMode::Always);
        assert_eq!(entry.name, "Acme");
        assert_eq!(entry.logo_path.as_deref(), Some("/api/v1/logo"));
        assert_eq!(entry.api_versions, [1, 2]);

        let mut hostile = info(&"x".repeat(5000));
        hostile.logo_path = Some("//other.example.test/../../logo".to_owned());
        hostile.api_versions = (0..1000).collect();
        let mut entry = ServerEntry::new(address.clone(), &hostile, ConnectionMode::Always);
        entry.generation = 1;
        assert_eq!(entry.name.chars().count(), 64);
        assert_eq!(entry.logo_path, None);
        assert_eq!(entry.api_versions.len(), 16);
        entry.check().unwrap();

        let unnamed = ServerEntry::new(address, &info(" \u{0000} "), ConnectionMode::Always);
        assert_eq!(unnamed.name, "acme.example.test");
    }

    #[test]
    fn a_corrupted_book_is_reported_and_left_in_place() {
        let (_directory, data_dir, book) = book();
        let saved = entry("https://acme.example.test", "Acme");
        book.add(saved.clone()).unwrap();
        let good = fs::read(data_dir.book_file()).unwrap();

        let truncated = &good[..good.len() / 2];
        fs::write(data_dir.book_file(), truncated).unwrap();
        assert!(matches!(
            book.entries(),
            Err(StorageError::Corrupted { .. })
        ));
        assert!(matches!(
            book.add(entry("https://other.example.test", "Other")),
            Err(BookError::Storage(StorageError::Corrupted { .. }))
        ));
        assert_eq!(fs::read(data_dir.book_file()).unwrap(), truncated);

        let moved = book.quarantine(1_800_000_000).unwrap();
        assert_eq!(fs::read(moved).unwrap(), truncated);
        assert!(book.entries().unwrap().is_empty());
        book.add(saved).unwrap();
    }

    #[test]
    fn a_book_with_invalid_entries_is_corrupted() {
        let (_directory, data_dir, book) = book();
        let id = ServerId::generate();
        let server = |address: &str, extra: &str| {
            format!(
                r#"{{"server_id":"{id}","generation":7,"address":"{address}","name":"Acme","logo_path":null,
                "api_versions":[1],"connection_mode":"always","membership":{{"state":"none"}},
                "last_cursor":null{extra}}}"#
            )
        };
        let cases = [
            format!(
                r#"{{"format_version":4,"servers":[{},{}]}}"#,
                server("https://a.example.test", ""),
                server("https://b.example.test", "")
            ),
            format!(
                r#"{{"format_version":4,"servers":[{}]}}"#,
                server("http://a.example.test", "")
            ),
            format!(
                r#"{{"format_version":4,"servers":[{}]}}"#,
                server("https://a.example.test/../../etc", "")
            ),
            format!(
                r#"{{"format_version":4,"servers":[{}]}}"#,
                server("https://a.example.test", "").replace("\"Acme\"", "\"\"")
            ),
            format!(
                r#"{{"format_version":4,"servers":[{}]}}"#,
                server("https://a.example.test", "")
                    .replace("\"logo_path\":null", "\"logo_path\":\"//b.example.test/x\"")
            ),
            format!(
                r#"{{"format_version":4,"servers":[{}]}}"#,
                server("https://a.example.test", "").replace("\"always\"", "\"sometimes\"")
            ),
            format!(
                r#"{{"format_version":4,"servers":[{}]}}"#,
                server("https://a.example.test", "")
                    .replace("\"generation\":7", "\"generation\":0")
            ),
            r#"{"format_version":2}"#.to_owned(),
        ];
        for content in cases {
            fs::write(data_dir.book_file(), &content).unwrap();
            assert!(
                matches!(book.entries(), Err(StorageError::Corrupted { .. })),
                "{content}"
            );
        }
        fs::write(
            data_dir.book_file(),
            format!(
                r#"{{"format_version":4,"servers":[{}]}}"#,
                server("https://a.example.test", "")
            ),
        )
        .unwrap();
        assert_eq!(book.entries().unwrap().len(), 1);
    }

    #[test]
    fn a_book_of_a_newer_client_is_refused_and_not_overwritten() {
        let (_directory, data_dir, book) = book();
        let newer = br#"{"format_version":8,"servers":[],"folders":[]}"#;
        fs::write(data_dir.book_file(), newer).unwrap();
        assert!(matches!(
            book.entries(),
            Err(StorageError::NewerFormat {
                found: 8,
                supported: 7,
                ..
            })
        ));
        assert!(matches!(
            book.add(entry("https://acme.example.test", "Acme")),
            Err(BookError::Storage(StorageError::NewerFormat { .. }))
        ));
        assert_eq!(fs::read(data_dir.book_file()).unwrap(), newer);
    }

    #[test]
    fn a_cursor_is_bounded_text() {
        assert!(EventCursor::new("").is_err());
        assert!(EventCursor::new("a\nb").is_err());
        assert!(EventCursor::new("c".repeat(513)).is_err());
        assert_eq!(
            EventCursor::new("c".repeat(512)).unwrap().as_str().len(),
            512
        );
        assert!(serde_json::from_str::<EventCursor>("\"\"").is_err());
    }
}
