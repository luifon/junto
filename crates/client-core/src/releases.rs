//! Signed client releases and bounded downloads (ADR 0724).

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::{Engine, prelude::BASE64_STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::updates::{NewestFuture, Release, UpdateError, UpdateSource, Version};

pub mod archive;
pub mod install;
#[cfg(target_os = "macos")]
pub mod macos;

pub trait Installer {
    fn install(&self, archive: &mut VerifiedArchive) -> Result<PathBuf, Error>;
}

pub const TEST_CONFIGURATION: bool = env!("JUNTO_TEST_CONFIGURATION").as_bytes()[0] == b'1';

pub fn build_label() -> String {
    crate::release_configuration::label(&Version::THIS_BUILD.to_string(), TEST_CONFIGURATION)
}

pub const MAX_METADATA: usize = 64 * 1024;
pub const MAX_ARCHIVE: u64 = 512 * 1024 * 1024;
const METADATA_DOMAIN: &[u8] = b"junto-release-metadata-v1\0";
const FRESHNESS_DOMAIN: &[u8] = b"junto-release-freshness-v1\0";
const ARCHIVE_DOMAIN: &[u8] = b"junto-release-archive-sha256-v1\0";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid release: {0}")]
    Invalid(&'static str),
    #[error("the release is not newer than this application")]
    NotNewer,
    #[error(
        "no valid freshness statement is available; check the system clock and try again after the source is refreshed"
    )]
    Expired,
    #[error("the update information is not signed by a key this version trusts")]
    UntrustedKeys,
    #[error("move Junto to a writable directory of your account before updating")]
    NotWritable,
    #[error(
        "cannot open or acquire .Junto.update.lock; check its permissions or another update in progress"
    )]
    Locked,
    #[error("the release request timed out")]
    Timeout,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Network(#[from] reqwest::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug)]
pub struct TrustedKeys {
    release: Vec<VerifyingKey>,
    freshness: Vec<VerifyingKey>,
}

impl TrustedKeys {
    pub fn new(keys: Vec<VerifyingKey>) -> Self {
        Self {
            release: keys,
            freshness: Vec::new(),
        }
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self, Error> {
        let texts: Vec<String> = serde_json::from_slice(bytes)?;
        if texts.len() > 32 {
            return Err(Error::Invalid("too many public keys"));
        }
        let mut keys = Vec::new();
        for text in texts {
            let bytes = decode::<32>(&text)?;
            keys.push(VerifyingKey::from_bytes(&bytes).map_err(|_| Error::Invalid("public key"))?);
        }
        Ok(Self {
            release: keys,
            freshness: Vec::new(),
        })
    }

    pub fn with_freshness(mut self, freshness: Self) -> Self {
        self.freshness = freshness.release;
        self
    }

    pub fn contains_freshness(&self, key: &VerifyingKey) -> bool {
        self.freshness.contains(key)
    }

    pub fn compiled() -> Result<Self, Error> {
        Ok(Self::from_json(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/update-keys.json"
        )))?
        .with_freshness(Self::from_json(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/freshness-keys.json"
        )))?))
    }

    pub fn contains(&self, key: &VerifyingKey) -> bool {
        self.release.contains(key)
    }

    pub fn is_empty(&self) -> bool {
        self.release.is_empty() || self.freshness.is_empty()
    }

    fn verify(
        &self,
        domain: &[u8],
        content: &[u8],
        signatures: &[NamedSignature],
    ) -> Result<(), Error> {
        if signatures.len() > 32 {
            return Err(Error::Invalid("too many signatures"));
        }
        let message = message(domain, content);
        let mut recognized = false;
        for entry in signatures {
            let Ok(bytes) = decode::<32>(&entry.key) else {
                continue;
            };
            let Some(key) = self.release.iter().find(|key| key.as_bytes() == &bytes) else {
                continue;
            };
            recognized = true;
            let Ok(bytes) = decode::<64>(&entry.signature) else {
                continue;
            };
            if key
                .verify_strict(&message, &Signature::from_bytes(&bytes))
                .is_ok()
            {
                return Ok(());
            }
        }
        Err(if recognized {
            Error::Invalid("untrusted signature")
        } else {
            Error::UntrustedKeys
        })
    }
}

fn decode<const N: usize>(text: &str) -> Result<[u8; N], Error> {
    BASE64_STANDARD
        .decode(text)
        .map_err(|_| Error::Invalid("base64"))?
        .try_into()
        .map_err(|_| Error::Invalid("encoded length"))
}

fn message(domain: &[u8], bytes: &[u8]) -> Vec<u8> {
    [domain, bytes].concat()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub version: String,
    pub system: String,
    pub architecture: String,
    pub archive: String,
    pub size: u64,
    pub sha256: String,
    #[serde(skip)]
    pub expires: u64,
    pub archive_signatures: Vec<NamedSignature>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamedSignature {
    pub key: String,
    pub signature: String,
}

fn sign(domain: &[u8], content: &[u8], key: &SigningKey) -> NamedSignature {
    NamedSignature {
        key: BASE64_STANDARD.encode(key.verifying_key().as_bytes()),
        signature: BASE64_STANDARD.encode(key.sign(&message(domain, content)).to_bytes()),
    }
}

#[derive(Serialize, Deserialize)]
struct Envelope {
    payload: String,
    signatures: Vec<NamedSignature>,
    #[serde(default)]
    freshness: Option<FreshnessEnvelope>,
}

#[derive(Serialize, Deserialize)]
struct FreshnessEnvelope {
    payload: String,
    signatures: Vec<NamedSignature>,
}
#[derive(Serialize, Deserialize)]
struct Freshness {
    format: u32,
    version: String,
    release_sha256: String,
    expires: u64,
}

pub fn expires_in_thirty_days() -> Result<u64, Error> {
    Ok(now()? + 30 * 24 * 60 * 60)
}
fn now() -> Result<u64, Error> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(|_| Error::Invalid("system clock"))
}

pub fn sign_manifest(
    manifest: &Manifest,
    release: &SigningKey,
    freshness: &SigningKey,
) -> Result<Vec<u8>, Error> {
    sign_manifest_many(manifest, &[release], &[freshness])
}

pub fn sign_manifest_many(
    manifest: &Manifest,
    release: &[&SigningKey],
    freshness: &[&SigningKey],
) -> Result<Vec<u8>, Error> {
    let payload = serde_json::to_vec(manifest)?;
    let mut envelope = Envelope {
        signatures: release
            .iter()
            .map(|key| sign(METADATA_DOMAIN, &payload, key))
            .collect(),
        payload: BASE64_STANDARD.encode(payload),
        freshness: None,
    };
    set_freshness(
        &mut envelope,
        &manifest.version,
        manifest.expires,
        freshness,
    )?;
    Ok(serde_json::to_vec(&envelope)?)
}

fn set_freshness(
    envelope: &mut Envelope,
    version: &str,
    expires: u64,
    keys: &[&SigningKey],
) -> Result<(), Error> {
    let release = BASE64_STANDARD
        .decode(&envelope.payload)
        .map_err(|_| Error::Invalid("payload"))?;
    let payload = serde_json::to_vec(&Freshness {
        format: 1,
        version: version.into(),
        release_sha256: hex(&Sha256::digest(release)),
        expires,
    })?;
    envelope.freshness = Some(FreshnessEnvelope {
        signatures: keys
            .iter()
            .map(|key| sign(FRESHNESS_DOMAIN, &payload, key))
            .collect(),
        payload: BASE64_STANDARD.encode(payload),
    });
    Ok(())
}

pub fn refresh_release(
    bytes: &[u8],
    archive: &Path,
    expected: Version,
    keys: &TrustedKeys,
    freshness: &[&SigningKey],
) -> Result<Vec<u8>, Error> {
    let manifest = verify_release(bytes, keys)?;
    if Version::parse(&manifest.version).map_err(|_| Error::Invalid("version"))? != expected {
        return Err(Error::Invalid(
            "release version differs from the version to refresh",
        ));
    }
    verify_archive(archive, &manifest, keys)?;
    if freshness.is_empty()
        || freshness
            .iter()
            .any(|key| !keys.contains_freshness(&key.verifying_key()))
    {
        return Err(Error::UntrustedKeys);
    }
    let mut envelope: Envelope = serde_json::from_slice(bytes)?;
    set_freshness(
        &mut envelope,
        &manifest.version,
        expires_in_thirty_days()?,
        freshness,
    )?;
    Ok(serde_json::to_vec(&envelope)?)
}

pub fn verify_release(bytes: &[u8], keys: &TrustedKeys) -> Result<Manifest, Error> {
    if bytes.len() > MAX_METADATA {
        return Err(Error::Invalid("metadata size"));
    }
    let envelope: Envelope = serde_json::from_slice(bytes)?;
    let payload = BASE64_STANDARD
        .decode(&envelope.payload)
        .map_err(|_| Error::Invalid("payload"))?;
    keys.verify(METADATA_DOMAIN, &payload, &envelope.signatures)?;
    let manifest: Manifest = serde_json::from_slice(&payload)?;
    if manifest.format != 1 {
        return Err(Error::Invalid("format"));
    }
    Version::parse(&manifest.version).map_err(|_| Error::Invalid("version"))?;
    if !filename(&manifest.archive) || manifest.size == 0 || manifest.size > MAX_ARCHIVE {
        return Err(Error::Invalid("archive name or size"));
    }
    hash_bytes(&manifest.sha256)?;
    if manifest.archive_signatures.len() > 32 {
        return Err(Error::Invalid("too many archive signatures"));
    }
    Ok(manifest)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub fn verify_manifest(
    bytes: &[u8],
    keys: &TrustedKeys,
    current: Version,
    system: &str,
    architecture: &str,
) -> Result<Manifest, Error> {
    verify_manifest_at(bytes, keys, current, system, architecture, now()?)
}

pub fn verify_manifest_at(
    bytes: &[u8],
    keys: &TrustedKeys,
    current: Version,
    system: &str,
    architecture: &str,
    now: u64,
) -> Result<Manifest, Error> {
    let mut manifest = verify_release(bytes, keys)?;
    let envelope: Envelope = serde_json::from_slice(bytes)?;
    let freshness = envelope.freshness.ok_or(Error::Expired)?;
    let payload = BASE64_STANDARD
        .decode(&freshness.payload)
        .map_err(|_| Error::Invalid("freshness payload"))?;
    TrustedKeys::new(keys.freshness.clone())
        .verify(FRESHNESS_DOMAIN, &payload, &freshness.signatures)
        .map_err(|error| match error {
            Error::UntrustedKeys => Error::UntrustedKeys,
            _ => Error::Expired,
        })?;
    let freshness: Freshness = serde_json::from_slice(&payload)?;
    let release = BASE64_STANDARD
        .decode(envelope.payload)
        .map_err(|_| Error::Invalid("payload"))?;
    if freshness.format != 1
        || freshness.version != manifest.version
        || freshness.release_sha256 != hex(&Sha256::digest(release))
    {
        return Err(Error::Invalid("freshness release binding"));
    }
    if freshness.expires <= now {
        return Err(Error::Expired);
    }
    manifest.expires = freshness.expires;
    if manifest.system != system || manifest.architecture != architecture {
        return Err(Error::Invalid("release target"));
    }
    if Version::parse(&manifest.version).map_err(|_| Error::Invalid("version"))? <= current {
        return Err(Error::NotNewer);
    }
    Ok(manifest)
}

fn filename(text: &str) -> bool {
    let stem = text.split('.').next().unwrap_or("").to_ascii_uppercase();
    let device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9');
    !text.is_empty()
        && text.len() <= 128
        && !text.ends_with('.')
        && !device
        && text
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
}

fn hash_bytes(text: &str) -> Result<[u8; 32], Error> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(Error::Invalid("SHA-256"));
    }
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| Error::Invalid("SHA-256"))?;
    }
    Ok(bytes)
}

fn hash_file(file: &mut File) -> Result<(u64, [u8; 32]), Error> {
    let mut hash = Sha256::new();
    let mut size = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        size += count as u64;
        if size > MAX_ARCHIVE {
            return Err(Error::Invalid("archive too large"));
        }
        hash.update(&buffer[..count]);
    }
    Ok((size, hash.finalize().into()))
}

pub fn sign_archive(path: &Path, manifest: &mut Manifest, key: &SigningKey) -> Result<(), Error> {
    let (size, hash) = hash_file(&mut File::open(path)?)?;
    manifest.size = size;
    manifest.sha256 = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    manifest
        .archive_signatures
        .retain(|entry| entry.key != BASE64_STANDARD.encode(key.verifying_key().as_bytes()));
    manifest
        .archive_signatures
        .push(sign(ARCHIVE_DOMAIN, &hash, key));
    Ok(())
}

fn verify_contents(file: &mut File, manifest: &Manifest, keys: &TrustedKeys) -> Result<(), Error> {
    let (size, hash) = hash_file(file)?;
    verify_digest(size, hash, manifest, keys)
}

fn verify_digest(
    size: u64,
    hash: [u8; 32],
    manifest: &Manifest,
    keys: &TrustedKeys,
) -> Result<(), Error> {
    if size != manifest.size || size == 0 || hash != hash_bytes(&manifest.sha256)? {
        return Err(Error::Invalid("archive size or hash"));
    }
    keys.verify(ARCHIVE_DOMAIN, &hash, &manifest.archive_signatures)
}

pub fn verify_archive(path: &Path, manifest: &Manifest, keys: &TrustedKeys) -> Result<(), Error> {
    verify_contents(&mut File::open(path)?, manifest, keys)
}

/// Owns a verified download. Dropping it removes its temporary file.
pub struct VerifiedArchive {
    pub(crate) file: File,
    pub(crate) manifest: Manifest,
    cleanup: RemoveFile,
    keys: TrustedKeys,
    pub(crate) wire: Vec<u8>,
}

impl VerifiedArchive {
    pub fn verify(&mut self) -> Result<(), Error> {
        use std::io::{Seek, SeekFrom};
        self.file.seek(SeekFrom::Start(0))?;
        verify_contents(&mut self.file, &self.manifest, &self.keys)
    }

    pub fn unpack(&mut self, destination: &Path) -> Result<(), Error> {
        self.verify()?;
        use std::io::{Seek, SeekFrom};
        self.file.seek(SeekFrom::Start(0))?;
        archive::unpack(&mut self.file, destination)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }
    pub fn path(&self) -> &Path {
        &self.cleanup.0
    }
}

pub fn write_atomic(destination: &Path, bytes: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    let parent = destination
        .parent()
        .ok_or(Error::Invalid("metadata parent"))?;
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|_| Error::Invalid("random source"))?;
    let temporary = parent.join(format!(".Junto.write-{}", hex(&random)));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let _cleanup = RemoveFile(temporary.clone());
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(temporary, destination)?;
    Ok(())
}

struct RemoveFile(PathBuf);
impl Drop for RemoveFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Clone)]
pub struct HttpSource {
    base: url::Url,
    client: reqwest::Client,
    keys: TrustedKeys,
    current: Version,
    system: String,
    architecture: String,
    timeout: Duration,
    idle: Duration,
}

impl HttpSource {
    pub fn new(
        base: &str,
        keys: TrustedKeys,
        current: Version,
        system: &str,
        architecture: &str,
    ) -> Result<Self, Error> {
        let mut base = crate::release_configuration::source(base).map_err(Error::Invalid)?;
        if !filename(system) || !filename(architecture) {
            return Err(Error::Invalid("release target"));
        }
        if !base.path().ends_with('/') {
            base.set_path(&format!("{}/", base.path()));
        }
        let client = reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(15))
            .build()?;
        Ok(Self {
            base,
            client,
            keys,
            current,
            system: system.into(),
            architecture: architecture.into(),
            timeout: Duration::from_secs(60),
            idle: Duration::from_secs(60),
        })
    }

    pub fn compiled() -> Result<Option<Self>, Error> {
        let Some(base) = option_env!("JUNTO_UPDATE_SOURCE").filter(|base| !base.is_empty()) else {
            return Ok(None);
        };
        let keys = TrustedKeys::compiled()?;
        if keys.is_empty() {
            return Ok(None);
        }
        Self::new(
            base,
            keys,
            Version::THIS_BUILD,
            std::env::consts::OS,
            std::env::consts::ARCH,
        )
        .map(Some)
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Result<Self, Error> {
        if timeout.is_zero() || timeout > Duration::from_secs(600) {
            return Err(Error::Invalid("timeout"));
        }
        self.timeout = timeout;
        self.idle = timeout;
        Ok(self)
    }

    async fn get(&self, filename: &str) -> Result<reqwest::Response, Error> {
        let address = self
            .base
            .join(filename)
            .map_err(|_| Error::Invalid("release address"))?;
        let response = self.client.get(address).send().await?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(Error::Invalid("HTTP status"));
        }
        Ok(response)
    }

    pub async fn check(&self) -> Result<Manifest, Error> {
        self.check_with_wire().await.map(|(manifest, _)| manifest)
    }

    async fn check_with_wire(&self) -> Result<(Manifest, Vec<u8>), Error> {
        tokio::time::timeout(self.timeout, async {
            let mut response = self
                .get(&format!(
                    "latest-{}-{}.json",
                    self.system, self.architecture
                ))
                .await?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                if bytes.len() + chunk.len() > MAX_METADATA {
                    return Err(Error::Invalid("metadata size"));
                }
                bytes.extend_from_slice(&chunk);
            }
            let manifest = verify_manifest(
                &bytes,
                &self.keys,
                self.current,
                &self.system,
                &self.architecture,
            )?;
            Ok((manifest, bytes))
        })
        .await
        .map_err(|_| Error::Timeout)?
    }

    pub async fn download(&self, destination: &Path) -> Result<VerifiedArchive, Error> {
        let (manifest, wire) = self.check_with_wire().await?;
        let total = self.idle + Duration::from_secs(manifest.size.div_ceil(16 * 1024));
        tokio::time::timeout(total, async {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let file = options.open(destination)?;
            let cleanup = RemoveFile(destination.to_owned());
            let mut file = tokio::fs::File::from_std(file);
            use tokio::io::AsyncWriteExt;
            let mut response = tokio::time::timeout(self.idle, self.get(&manifest.archive))
                .await
                .map_err(|_| Error::Timeout)??;
            if response
                .content_length()
                .is_some_and(|size| size != manifest.size)
            {
                return Err(Error::Invalid("download size"));
            }
            let mut size = 0;
            while let Some(chunk) = idle(self.idle, response.chunk()).await?? {
                size += chunk.len() as u64;
                if size > manifest.size {
                    return Err(Error::Invalid("download size"));
                }
                file.write_all(&chunk).await?;
            }
            file.flush().await?;
            if size != manifest.size {
                return Err(Error::Invalid("download size"));
            }
            file.sync_all().await?;
            let mut archive = VerifiedArchive {
                file: file.into_std().await,
                manifest,
                cleanup,
                keys: self.keys.clone(),
                wire,
            };
            tokio::task::spawn_blocking(move || {
                archive.verify()?;
                Ok(archive)
            })
            .await
            .map_err(|_| Error::Invalid("archive verification task stopped"))?
        })
        .await
        .map_err(|_| Error::Timeout)?
    }
}

impl UpdateSource for HttpSource {
    fn newest(&self) -> NewestFuture {
        let source = self.clone();
        Box::pin(async move {
            match source.check().await {
                Ok(manifest) => Ok(Release::new(
                    Version::parse(&manifest.version).map_err(|_| UpdateError::Invalid)?,
                    None,
                )),
                Err(Error::NotNewer) => Ok(Release::new(source.current, None)),
                Err(Error::Expired) => Err(UpdateError::Expired),
                Err(Error::UntrustedKeys) => Err(UpdateError::UntrustedKeys),
                Err(Error::Network(_) | Error::Timeout) => Err(UpdateError::Unreachable),
                Err(_) => Err(UpdateError::Invalid),
            }
        })
    }
}

async fn idle<F: std::future::Future>(duration: Duration, future: F) -> Result<F::Output, Error> {
    tokio::time::timeout(duration, future)
        .await
        .map_err(|_| Error::Timeout)
}

#[cfg(test)]
mod timing_tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn continued_bytes_reset_idle_time_and_silence_reaches_the_deadline() {
        let start = tokio::time::Instant::now();
        for _ in 0..3 {
            assert!(
                idle(
                    Duration::from_secs(60),
                    tokio::time::sleep(Duration::from_secs(40))
                )
                .await
                .is_ok()
            );
        }
        assert_eq!(start.elapsed(), Duration::from_secs(120));
        assert!(matches!(
            idle(Duration::from_secs(60), std::future::pending::<()>()).await,
            Err(Error::Timeout)
        ));
        assert_eq!(start.elapsed(), Duration::from_secs(180));
    }
}
