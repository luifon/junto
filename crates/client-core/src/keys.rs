//! Device keys: one Ed25519 key per server, and where it is stored (ADR 0103, ADR 0711).

use std::fmt;

use ed25519_dalek::{Signer, SigningKey};
use junto_protocol::ids::ServerId;
use junto_protocol::keys::{PublicKey, Signature};
use zeroize::Zeroizing;

use crate::address::ServerAddress;
use crate::paths::{DataDir, key_file_name};
use crate::storage::StorageError;

pub const SEED_LENGTH: usize = 32;

const FILE_MAGIC: &[u8; 20] = b"junto-device-key-v1\n";
const LONGEST_ORIGIN: usize = 300;
/// The first line, the secret bytes, the public key and the length of the origin.
const FIXED_LENGTH: usize = FILE_MAGIC.len() + SEED_LENGTH + PublicKey::LENGTH + 2;
const LONGEST_FILE: usize = FIXED_LENGTH + LONGEST_ORIGIN;

/// The private key of this device on one server. The secret bytes are overwritten when the
/// value is dropped, and no formatting of the value shows them.
pub struct DeviceKey {
    signing: SigningKey,
}

impl DeviceKey {
    /// Generates a key from the random source of the operating system.
    pub fn generate() -> Result<Self, KeyStoreError> {
        let mut seed = Zeroizing::new([0u8; SEED_LENGTH]);
        getrandom::fill(seed.as_mut_slice()).map_err(|_| KeyStoreError::RandomnessUnavailable)?;
        Ok(Self::from_seed(&seed))
    }

    /// Builds the key from its 32 secret bytes. Only an implementation of [`KeyStore`]
    /// needs this.
    pub fn from_seed(seed: &Zeroizing<[u8; SEED_LENGTH]>) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// The 32 secret bytes. Only an implementation of [`KeyStore`] needs this.
    pub fn seed(&self) -> Zeroizing<[u8; SEED_LENGTH]> {
        Zeroizing::new(self.signing.to_bytes())
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey::from_bytes(self.signing.verifying_key().to_bytes())
    }

    /// Only the code that has compared a challenge with what the client asked for signs
    /// (`api::proof`).
    pub(crate) fn sign(&self, message: &[u8]) -> Signature {
        Signature::from_bytes(self.signing.sign(message).to_bytes())
    }
}

impl fmt::Debug for DeviceKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "DeviceKey(public {})", self.public_key())
    }
}

/// The content of a key file: the key and the origin that it was created for.
fn encode(key: &DeviceKey, origin: &ServerAddress) -> Option<Zeroizing<Vec<u8>>> {
    let origin = origin.as_str().as_bytes();
    let length = u16::try_from(origin.len())
        .ok()
        .filter(|length| usize::from(*length) <= LONGEST_ORIGIN)?;
    let mut bytes = Zeroizing::new(Vec::with_capacity(FIXED_LENGTH + origin.len()));
    bytes.extend_from_slice(FILE_MAGIC);
    bytes.extend_from_slice(key.seed().as_slice());
    bytes.extend_from_slice(key.public_key().as_bytes());
    bytes.extend_from_slice(&length.to_be_bytes());
    bytes.extend_from_slice(origin);
    Some(bytes)
}

fn decode(bytes: &[u8]) -> Option<(DeviceKey, ServerAddress)> {
    let (magic, rest) = bytes.split_at_checked(FILE_MAGIC.len())?;
    let (seed_bytes, rest) = rest.split_at_checked(SEED_LENGTH)?;
    let (public, rest) = rest.split_at_checked(PublicKey::LENGTH)?;
    let (length, origin) = rest.split_at_checked(2)?;
    let stated = usize::from(u16::from_be_bytes([length[0], length[1]]));
    if magic != FILE_MAGIC || stated != origin.len() {
        return None;
    }
    let mut seed = Zeroizing::new([0u8; SEED_LENGTH]);
    seed.copy_from_slice(seed_bytes);
    let key = DeviceKey::from_seed(&seed);
    if key.public_key().as_bytes().as_slice() != public {
        return None;
    }
    let origin = std::str::from_utf8(origin).ok()?;
    let address = ServerAddress::parse(origin).ok()?;
    (address.as_str() == origin).then_some((key, address))
}

#[derive(Debug, thiserror::Error)]
pub enum KeyStoreError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error("the stored device key of server {server_id} is damaged")]
    Damaged { server_id: ServerId },
    #[error("a device key of server {server_id} is already stored")]
    AlreadyStored { server_id: ServerId },
    /// The key was created for another address than the one that asks for it. It is not
    /// given out.
    #[error("the device key of server {server_id} was created for the address {stored}")]
    OtherOrigin {
        server_id: ServerId,
        stored: ServerAddress,
    },
    #[error("the operating system gave no random bytes")]
    RandomnessUnavailable,
    #[error("the key store failed: {0}")]
    Other(String),
}

/// Where device keys are kept. The file store is the one implementation in the first
/// version, on every system; a store that uses a secret service of the system implements
/// the same trait (ADR 0711).
///
/// A key belongs to one server identifier and to one origin: the address that the client
/// verified when the key was created. The store keeps the origin with the key and gives
/// the key out for that origin only, whatever the server book says.
pub trait KeyStore: Send + Sync + 'static {
    /// The key of a server, when it was created for `origin`. A key of another origin is
    /// [`KeyStoreError::OtherOrigin`].
    fn load(
        &self,
        server_id: ServerId,
        origin: &ServerAddress,
    ) -> Result<Option<DeviceKey>, KeyStoreError>;

    /// The origin of the stored key of a server.
    fn origin(&self, server_id: ServerId) -> Result<Option<ServerAddress>, KeyStoreError>;

    /// Stores the key of a server that has none. A stored key is never replaced.
    fn insert(
        &self,
        server_id: ServerId,
        origin: &ServerAddress,
        key: &DeviceKey,
    ) -> Result<(), KeyStoreError>;

    /// Deletes the key of a server. Deleting an absent key succeeds.
    fn remove(&self, server_id: ServerId) -> Result<(), KeyStoreError>;
}

/// The stored key of a server, or a new key that is stored before it is returned, so that
/// a key is never shown to a server before it is safe on this device.
pub fn load_or_create(
    store: &dyn KeyStore,
    server_id: ServerId,
    origin: &ServerAddress,
) -> Result<DeviceKey, KeyStoreError> {
    if let Some(key) = store.load(server_id, origin)? {
        return Ok(key);
    }
    let key = DeviceKey::generate()?;
    match store.insert(server_id, origin, &key) {
        Ok(()) => Ok(key),
        Err(KeyStoreError::AlreadyStored { .. }) => store
            .load(server_id, origin)?
            .ok_or(KeyStoreError::Damaged { server_id }),
        Err(error) => Err(error),
    }
}

/// Key files in the key directory of the data directory: `keys/<server identifier>.key`.
/// The directory and each file belong to the user and are closed to everybody else; a file
/// or directory that fails this is refused. Each file is opened through the handle of the
/// key directory without following a link, and the opened file is what is checked.
#[derive(Debug, Clone)]
pub struct FileKeyStore {
    data_dir: DataDir,
}

type Stored = (DeviceKey, ServerAddress);

impl FileKeyStore {
    pub fn new(data_dir: DataDir) -> Self {
        Self { data_dir }
    }

    fn read(&self, server_id: ServerId) -> Result<Option<Stored>, KeyStoreError> {
        let keys = self.data_dir.keys();
        keys.check()?;
        let mut bytes = Zeroizing::new([0u8; LONGEST_FILE]);
        let length = match keys.read_private(&key_file_name(server_id), bytes.as_mut_slice()) {
            Ok(Some(length)) => length,
            Ok(None) => return Ok(None),
            Err(StorageError::TooLarge { .. }) => {
                return Err(KeyStoreError::Damaged { server_id });
            }
            Err(error) => return Err(error.into()),
        };
        decode(&bytes[..length])
            .map(Some)
            .ok_or(KeyStoreError::Damaged { server_id })
    }
}

impl KeyStore for FileKeyStore {
    fn load(
        &self,
        server_id: ServerId,
        origin: &ServerAddress,
    ) -> Result<Option<DeviceKey>, KeyStoreError> {
        match self.read(server_id)? {
            None => Ok(None),
            Some((key, stored)) if stored == *origin => Ok(Some(key)),
            Some((_, stored)) => Err(KeyStoreError::OtherOrigin { server_id, stored }),
        }
    }

    fn origin(&self, server_id: ServerId) -> Result<Option<ServerAddress>, KeyStoreError> {
        Ok(self.read(server_id)?.map(|(_, origin)| origin))
    }

    fn insert(
        &self,
        server_id: ServerId,
        origin: &ServerAddress,
        key: &DeviceKey,
    ) -> Result<(), KeyStoreError> {
        let content = encode(key, origin)
            .ok_or_else(|| KeyStoreError::Other("the address is too long".to_owned()))?;
        let _lock = self.data_dir.lock()?;
        let keys = self.data_dir.keys();
        keys.check()?;
        if keys.create(&key_file_name(server_id), &content)? {
            Ok(())
        } else {
            Err(KeyStoreError::AlreadyStored { server_id })
        }
    }

    fn remove(&self, server_id: ServerId) -> Result<(), KeyStoreError> {
        let _lock = self.data_dir.lock()?;
        self.data_dir.keys().remove(&key_file_name(server_id))?;
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;

    type Stored = (Zeroizing<[u8; SEED_LENGTH]>, ServerAddress);

    /// A second implementation of the trait: keys in memory.
    #[derive(Default)]
    pub(crate) struct MemoryKeyStore {
        keys: Mutex<BTreeMap<ServerId, Stored>>,
    }

    impl KeyStore for MemoryKeyStore {
        fn load(
            &self,
            server_id: ServerId,
            origin: &ServerAddress,
        ) -> Result<Option<DeviceKey>, KeyStoreError> {
            match self.keys.lock().unwrap().get(&server_id) {
                None => Ok(None),
                Some((seed, stored)) if stored == origin => Ok(Some(DeviceKey::from_seed(seed))),
                Some((_, stored)) => Err(KeyStoreError::OtherOrigin {
                    server_id,
                    stored: stored.clone(),
                }),
            }
        }

        fn origin(&self, server_id: ServerId) -> Result<Option<ServerAddress>, KeyStoreError> {
            let keys = self.keys.lock().unwrap();
            Ok(keys.get(&server_id).map(|(_, origin)| origin.clone()))
        }

        fn insert(
            &self,
            server_id: ServerId,
            origin: &ServerAddress,
            key: &DeviceKey,
        ) -> Result<(), KeyStoreError> {
            let mut keys = self.keys.lock().unwrap();
            if keys.contains_key(&server_id) {
                return Err(KeyStoreError::AlreadyStored { server_id });
            }
            keys.insert(server_id, (key.seed(), origin.clone()));
            Ok(())
        }

        fn remove(&self, server_id: ServerId) -> Result<(), KeyStoreError> {
            self.keys.lock().unwrap().remove(&server_id);
            Ok(())
        }
    }

    pub(crate) fn origin(name: &str) -> ServerAddress {
        ServerAddress::parse(&format!("{name}.example.test")).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::origin;
    use super::*;
    use ed25519_dalek::{Verifier, VerifyingKey};
    use std::fs;

    fn store() -> (tempfile::TempDir, DataDir, FileKeyStore) {
        let directory = tempfile::tempdir().unwrap();
        let data_dir = DataDir::open(directory.path().join("junto")).unwrap();
        let store = FileKeyStore::new(data_dir.clone());
        (directory, data_dir, store)
    }

    #[test]
    fn a_key_signs_and_its_public_key_verifies() {
        let key = DeviceKey::generate().unwrap();
        let signature = key.sign(b"message");
        let verifying = VerifyingKey::from_bytes(key.public_key().as_bytes()).unwrap();
        verifying
            .verify(
                b"message",
                &ed25519_dalek::Signature::from_bytes(signature.as_bytes()),
            )
            .unwrap();
        assert_ne!(
            DeviceKey::generate().unwrap().public_key(),
            key.public_key()
        );
    }

    #[test]
    fn formatting_a_key_shows_the_public_key_only() {
        let key = DeviceKey::generate().unwrap();
        let shown = format!("{key:?}");
        assert_eq!(shown, format!("DeviceKey(public {})", key.public_key()));
        let seed = key.seed();
        let seed_text = PublicKey::from_bytes(*seed).to_string();
        assert!(!shown.contains(&seed_text));
    }

    #[test]
    fn a_stored_key_is_read_back_and_one_server_cannot_get_the_key_of_another() {
        let (_directory, data_dir, store) = store();
        let (first, second) = (ServerId::generate(), ServerId::generate());
        let acme = origin("acme");
        let key = load_or_create(&store, first, &acme).unwrap();
        assert_eq!(
            store.load(first, &acme).unwrap().unwrap().public_key(),
            key.public_key()
        );
        assert_eq!(
            load_or_create(&store, first, &acme).unwrap().public_key(),
            key.public_key()
        );
        assert!(store.load(second, &origin("globex")).unwrap().is_none());
        let other = load_or_create(&store, second, &origin("globex")).unwrap();
        assert_ne!(other.public_key(), key.public_key());
        assert_eq!(
            fs::read(data_dir.key_file(first)).unwrap().len(),
            FIXED_LENGTH + acme.as_str().len()
        );
    }

    #[test]
    fn a_key_is_given_out_for_the_origin_it_was_created_for_and_for_no_other() {
        let (_directory, data_dir, store) = store();
        let server_id = ServerId::generate();
        let (acme, relay) = (origin("acme"), origin("relay"));
        let key = load_or_create(&store, server_id, &acme).unwrap();
        assert_eq!(store.origin(server_id).unwrap(), Some(acme.clone()));
        assert_eq!(store.origin(ServerId::generate()).unwrap(), None);

        assert!(matches!(
            store.load(server_id, &relay),
            Err(KeyStoreError::OtherOrigin { stored, .. }) if stored == acme
        ));
        assert!(matches!(
            load_or_create(&store, server_id, &relay),
            Err(KeyStoreError::OtherOrigin { .. })
        ));
        assert!(matches!(
            store.insert(server_id, &relay, &DeviceKey::generate().unwrap()),
            Err(KeyStoreError::AlreadyStored { .. })
        ));
        let stored = fs::read(data_dir.key_file(server_id)).unwrap();
        assert!(stored.ends_with(b"https://acme.example.test"));
        assert_eq!(
            store.load(server_id, &acme).unwrap().unwrap().public_key(),
            key.public_key()
        );

        let other_port = ServerAddress::parse("acme.example.test:8443").unwrap();
        assert!(matches!(
            store.load(server_id, &other_port),
            Err(KeyStoreError::OtherOrigin { .. })
        ));
    }

    #[test]
    fn a_stored_key_is_never_replaced() {
        let (_directory, _data_dir, store) = store();
        let server_id = ServerId::generate();
        let acme = origin("acme");
        let key = load_or_create(&store, server_id, &acme).unwrap();
        let other = DeviceKey::generate().unwrap();
        assert!(matches!(
            store.insert(server_id, &acme, &other),
            Err(KeyStoreError::AlreadyStored { .. })
        ));
        assert_eq!(
            store.load(server_id, &acme).unwrap().unwrap().public_key(),
            key.public_key()
        );
    }

    #[test]
    fn a_removed_key_is_gone_and_removing_twice_succeeds() {
        let (_directory, data_dir, store) = store();
        let server_id = ServerId::generate();
        let acme = origin("acme");
        load_or_create(&store, server_id, &acme).unwrap();
        store.remove(server_id).unwrap();
        store.remove(server_id).unwrap();
        assert!(store.load(server_id, &acme).unwrap().is_none());
        assert!(!data_dir.key_file(server_id).exists());
    }

    #[cfg(unix)]
    #[test]
    fn the_key_file_has_mode_0600_in_a_directory_with_mode_0700() {
        use std::os::unix::fs::PermissionsExt;
        let (_directory, data_dir, store) = store();
        let server_id = ServerId::generate();
        load_or_create(&store, server_id, &origin("acme")).unwrap();
        let file = fs::metadata(data_dir.key_file(server_id)).unwrap();
        assert!(file.is_file());
        assert_eq!(file.permissions().mode() & 0o7777, 0o600);
        let directory = fs::metadata(data_dir.keys_dir()).unwrap();
        assert_eq!(directory.permissions().mode() & 0o7777, 0o700);
    }

    #[cfg(unix)]
    #[test]
    fn a_key_that_others_can_read_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let (_directory, data_dir, store) = store();
        let server_id = ServerId::generate();
        let acme = origin("acme");
        load_or_create(&store, server_id, &acme).unwrap();

        let path = data_dir.key_file(server_id);
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            store.load(server_id, &acme),
            Err(KeyStoreError::Storage(StorageError::NotPrivate { .. }))
        ));
        assert!(matches!(
            store.origin(server_id),
            Err(KeyStoreError::Storage(StorageError::NotPrivate { .. }))
        ));
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        fs::set_permissions(data_dir.keys_dir(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            store.load(server_id, &acme),
            Err(KeyStoreError::Storage(StorageError::NotPrivate { .. }))
        ));
        assert!(matches!(
            store.insert(ServerId::generate(), &acme, &DeviceKey::generate().unwrap()),
            Err(KeyStoreError::Storage(StorageError::NotPrivate { .. }))
        ));
        fs::set_permissions(data_dir.keys_dir(), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(store.load(server_id, &acme).unwrap().is_some());
    }

    #[cfg(unix)]
    #[test]
    fn a_key_file_that_is_swapped_for_a_link_is_never_read_through_the_link() {
        use crate::storage::Dir;
        use std::sync::atomic::{AtomicBool, Ordering};
        let (directory, data_dir, store) = store();
        let server_id = ServerId::generate();
        let acme = origin("acme");
        let key = load_or_create(&store, server_id, &acme).unwrap();
        let path = data_dir.key_file(server_id);
        let genuine = fs::read(&path).unwrap();

        let decoy_key = DeviceKey::generate().unwrap();
        let outside = Dir::open(&directory.path().join("outside")).unwrap();
        outside
            .write("decoy.key", &encode(&decoy_key, &acme).unwrap())
            .unwrap();
        let decoy = outside.path().join("decoy.key");

        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let swapper = {
            let (stop, path, data_dir) = (stop.clone(), path.clone(), data_dir.clone());
            let staging = data_dir.keys_dir().join("staging");
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::os::unix::fs::symlink(&decoy, &staging).unwrap();
                    fs::rename(&staging, &path).unwrap();
                    data_dir.keys().write("staging", &genuine).unwrap();
                    fs::rename(&staging, &path).unwrap();
                }
            })
        };
        let started = std::time::Instant::now();
        let mut through_the_link = false;
        let mut refused = 0;
        while started.elapsed() < std::time::Duration::from_secs(2) && !through_the_link {
            match store.load(server_id, &acme) {
                Ok(Some(loaded)) => {
                    through_the_link = loaded.public_key() == decoy_key.public_key();
                    assert!(through_the_link || loaded.public_key() == key.public_key());
                }
                Ok(None) => {}
                Err(error) => {
                    assert!(
                        matches!(
                            error,
                            KeyStoreError::Storage(StorageError::UnexpectedKind { .. })
                        ),
                        "{error}"
                    );
                    refused += 1;
                }
            }
        }
        stop.store(true, Ordering::Relaxed);
        swapper.join().unwrap();
        assert!(!through_the_link, "a key was read through a symbolic link");
        assert!(refused > 0, "the link was never met");
    }

    #[cfg(unix)]
    #[test]
    fn a_key_is_stored_under_the_book_lock_through_another_spelling_of_the_data_directory() {
        let directory = tempfile::tempdir().unwrap();
        fs::create_dir(directory.path().join("real")).unwrap();
        std::os::unix::fs::symlink(
            directory.path().join("real"),
            directory.path().join("alias"),
        )
        .unwrap();
        let direct = directory.path().join("real").join("junto");
        let aliased = directory.path().join("alias").join("junto");
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let data_dir = DataDir::open(&direct).unwrap();
            let store = FileKeyStore::new(DataDir::open(&aliased).unwrap());
            let _lock = data_dir.lock().unwrap();
            let key = load_or_create(&store, ServerId::generate(), &origin("acme"));
            sender.send(key.is_ok()).unwrap();
        });
        let stored = receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the key store waits for the lock that its own thread holds");
        assert!(stored);
    }

    #[test]
    fn a_damaged_key_file_is_reported_and_not_used() {
        let (_directory, data_dir, store) = store();
        let server_id = ServerId::generate();
        let acme = origin("acme");
        let key = load_or_create(&store, server_id, &acme).unwrap();
        let name = key_file_name(server_id);
        let good = fs::read(data_dir.key_file(server_id)).unwrap();

        let mut flipped_seed = good.clone();
        flipped_seed[FILE_MAGIC.len() + 3] ^= 0x01;
        let mut flipped_public = good.clone();
        flipped_public[FILE_MAGIC.len() + SEED_LENGTH + 31] ^= 0x80;
        let mut other_magic = good.clone();
        other_magic[18] = b'2';
        let mut longer = good.clone();
        longer.push(0);
        let mut other_origin_length = good.clone();
        other_origin_length[FIXED_LENGTH - 1] ^= 0x01;
        let mut not_an_origin = good.clone();
        let last = not_an_origin.len() - 1;
        not_an_origin[last] = b'/';
        let cases = [
            flipped_seed,
            flipped_public,
            other_magic,
            longer,
            other_origin_length,
            not_an_origin,
            good[..good.len() - 1].to_vec(),
            good[..FIXED_LENGTH - 2].to_vec(),
            vec![0u8; LONGEST_FILE + 1],
            Vec::new(),
        ];
        for content in cases {
            data_dir.keys().write(&name, &content).unwrap();
            let error = store.load(server_id, &acme).unwrap_err();
            assert!(
                matches!(error, KeyStoreError::Damaged { .. }),
                "{} bytes: {error}",
                content.len()
            );
            assert!(matches!(
                load_or_create(&store, server_id, &acme),
                Err(KeyStoreError::Damaged { .. })
            ));
        }
        data_dir.keys().write(&name, &good).unwrap();
        assert_eq!(
            store.load(server_id, &acme).unwrap().unwrap().public_key(),
            key.public_key()
        );
    }

    #[test]
    fn errors_name_the_server_and_never_the_key() {
        let (_directory, data_dir, store) = store();
        let server_id = ServerId::generate();
        let acme = origin("acme");
        let key = load_or_create(&store, server_id, &acme).unwrap();
        let seed_text = PublicKey::from_bytes(*key.seed()).to_string();
        let mut damaged = fs::read(data_dir.key_file(server_id)).unwrap();
        damaged[FILE_MAGIC.len() + SEED_LENGTH] ^= 1;
        data_dir
            .keys()
            .write(&key_file_name(server_id), &damaged)
            .unwrap();
        let error = store.load(server_id, &acme).unwrap_err();
        let shown = format!("{error} {error:?}");
        assert!(shown.contains(&server_id.to_string()));
        assert!(!shown.contains(&seed_text));
    }

    #[test]
    fn the_memory_store_follows_the_same_rules() {
        let store = testing::MemoryKeyStore::default();
        let server_id = ServerId::generate();
        let acme = origin("acme");
        let key = load_or_create(&store, server_id, &acme).unwrap();
        assert_eq!(
            load_or_create(&store, server_id, &acme)
                .unwrap()
                .public_key(),
            key.public_key()
        );
        assert!(matches!(
            store.insert(server_id, &acme, &DeviceKey::generate().unwrap()),
            Err(KeyStoreError::AlreadyStored { .. })
        ));
        assert!(matches!(
            store.load(server_id, &origin("relay")),
            Err(KeyStoreError::OtherOrigin { .. })
        ));
        assert_eq!(store.origin(server_id).unwrap(), Some(acme.clone()));
        store.remove(server_id).unwrap();
        assert!(store.load(server_id, &acme).unwrap().is_none());
    }
}
