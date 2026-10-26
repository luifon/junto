#![allow(clippy::unwrap_used)]

use ed25519_dalek::{Signer, SigningKey};
use junto_client_core::releases::{Manifest, TrustedKeys, verify_manifest};
use junto_client_core::updates::Version;

/// An expiry within the accepted range: one day after the clock of the test.
fn soon() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 24 * 60 * 60
}

fn target(system: &str, architecture: &str) -> junto_client_core::releases::Target {
    junto_client_core::releases::Target::new(system, "bundle", architecture).unwrap()
}

fn trust(keys: Vec<ed25519_dalek::VerifyingKey>) -> TrustedKeys {
    TrustedKeys::new(keys.clone()).with_freshness(TrustedKeys::new(keys))
}

fn sign_manifest(
    manifest: &Manifest,
    key: &SigningKey,
) -> Result<Vec<u8>, junto_client_core::releases::Error> {
    junto_client_core::releases::sign_manifest(manifest, key, key)
}

fn key() -> SigningKey {
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).unwrap();
    SigningKey::from_bytes(&seed)
}

fn manifest() -> Manifest {
    Manifest {
        format: 1,
        version: "0.2.0".into(),
        system: "macos".into(),
        package: "bundle".into(),
        architecture: "aarch64".into(),
        archive: "junto-0.2.0.junto".into(),
        size: 6,
        sha256: "00".repeat(32),
        expires: soon(),
        archive_signatures: Vec::new(),
    }
}

#[test]
fn signed_metadata_accepts_only_newer_matching_releases_and_trusted_keys() {
    let signing = key();
    let keys = trust(vec![signing.verifying_key()]);
    let current = Version::new(0, 1, 0);
    let wire = sign_manifest(&manifest(), &signing).unwrap();
    assert!(verify_manifest(&wire, &keys, current, &target("macos", "aarch64")).is_ok());
    for version in [Version::new(0, 2, 0), Version::new(0, 3, 0)] {
        assert!(verify_manifest(&wire, &keys, version, &target("macos", "aarch64")).is_err());
    }
    assert!(verify_manifest(&wire, &keys, current, &target("windows", "aarch64")).is_err());
    assert!(verify_manifest(&wire, &keys, current, &target("macos", "x86_64")).is_err());
    let replacement = key();
    let replaced = trust(vec![replacement.verifying_key()]);
    assert!(verify_manifest(&wire, &replaced, current, &target("macos", "aarch64")).is_err());
    let new_wire = sign_manifest(&manifest(), &replacement).unwrap();
    assert!(verify_manifest(&new_wire, &replaced, current, &target("macos", "aarch64")).is_ok());
    let mut changed: serde_json::Value = serde_json::from_slice(&wire).unwrap();
    changed["signatures"][0]["signature"] = serde_json::Value::String(base64::Engine::encode(
        &base64::prelude::BASE64_STANDARD,
        signing.sign(b"wrong").to_bytes(),
    ));
    assert!(
        verify_manifest(
            &serde_json::to_vec(&changed).unwrap(),
            &keys,
            current,
            &target("macos", "aarch64")
        )
        .is_err()
    );
}

use junto_client_core::releases::{HttpSource, sign_archive, verify_archive};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[test]
fn archive_requires_the_signed_size_hash_and_signature() {
    let root = tempfile::Builder::new()
        .prefix("junto-release-")
        .tempdir()
        .unwrap();
    let file = root.path().join("archive");
    std::fs::write(&file, b"archive contents").unwrap();
    let signing = key();
    let keys = trust(vec![signing.verifying_key()]);
    let mut metadata = manifest();
    sign_archive(&file, &mut metadata, &signing).unwrap();
    assert!(verify_archive(&file, &metadata, &keys).is_ok());
    let mut wrong = metadata.clone();
    wrong.size += 1;
    assert!(verify_archive(&file, &wrong, &keys).is_err());
    wrong = metadata.clone();
    wrong.sha256 = "ff".repeat(32);
    assert!(verify_archive(&file, &wrong, &keys).is_err());
    wrong = metadata.clone();
    wrong.archive_signatures[0].signature = base64::Engine::encode(
        &base64::prelude::BASE64_STANDARD,
        signing.sign(b"wrong").to_bytes(),
    );
    assert!(verify_archive(&file, &wrong, &keys).is_err());
    std::fs::write(&file, b"archive contentX").unwrap();
    assert!(verify_archive(&file, &metadata, &keys).is_err());
    std::fs::write(&file, b"archive").unwrap();
    assert!(verify_archive(&file, &metadata, &keys).is_err());
}

async fn server(response: Vec<u8>, delay: Duration) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}/", listener.local_addr().unwrap());
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 4096];
        let _ = stream.read(&mut request).await;
        tokio::time::sleep(delay).await;
        let _ = stream.write_all(&response).await;
    });
    (address, task)
}

#[tokio::test]
async fn source_accepts_signed_metadata_from_loopback_and_refuses_redirects() {
    let signing = key();
    let keys = trust(vec![signing.verifying_key()]);
    let body = sign_manifest(&manifest(), &signing).unwrap();
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend(body);
    let (address, task) = server(response.clone(), Duration::ZERO).await;
    let source = HttpSource::new(
        &address,
        keys.clone(),
        Version::new(0, 1, 0),
        target("macos", "aarch64"),
    )
    .unwrap();
    assert_eq!(source.check().await.unwrap().version, "0.2.0");
    task.await.unwrap();
    let (redirect_address, redirected) = server(response, Duration::ZERO).await;
    let redirect_address = redirect_address.replace("127.0.0.1", "localhost");
    let redirect =
        format!("HTTP/1.1 302 Found\r\nLocation: {redirect_address}\r\nContent-Length: 0\r\n\r\n");
    let (address, task) = server(redirect.into_bytes(), Duration::ZERO).await;
    let source = HttpSource::new(
        &address,
        keys,
        Version::new(0, 1, 0),
        target("macos", "aarch64"),
    )
    .unwrap();
    assert!(source.check().await.is_err());
    assert!(
        !redirected.is_finished(),
        "the other host received a request"
    );
    redirected.abort();
    let _ = redirected.await;
    task.await.unwrap();
}

#[tokio::test]
async fn source_bounds_the_complete_request_even_when_it_never_answers() {
    for delay in [Duration::from_millis(300), Duration::from_secs(60)] {
        let (address, task) = server(Vec::new(), delay).await;
        let source = HttpSource::new(
            &address,
            trust(vec![key().verifying_key()]),
            Version::new(0, 1, 0),
            target("macos", "aarch64"),
        )
        .unwrap()
        .with_timeout(Duration::from_millis(50))
        .unwrap();
        let start = Instant::now();
        assert!(source.check().await.is_err());
        assert!(start.elapsed() >= Duration::from_millis(40));
        assert!(start.elapsed() < Duration::from_secs(10));
        task.abort();
        let _ = task.await;
    }
}

#[test]
fn source_refuses_credentials_fragments_and_nonlocal_plain_http() {
    for address in [
        "http://example.invalid/",
        "https://user@example.invalid/",
        "https://example.invalid/#x",
        "https://example.invalid/?x",
        "file:///tmp/",
        "http://localhost/",
    ] {
        assert!(
            HttpSource::new(
                address,
                trust(vec![]),
                Version::new(0, 1, 0),
                target("macos", "aarch64")
            )
            .is_err(),
            "{address}"
        );
    }
}

#[tokio::test]
async fn download_is_bounded_and_verified_before_it_is_returned() {
    let root = tempfile::Builder::new()
        .prefix("junto-release-")
        .tempdir()
        .unwrap();
    let original = root.path().join("original");
    std::fs::write(&original, b"archive").unwrap();
    let signing = key();
    let keys = trust(vec![signing.verifying_key()]);
    let mut metadata = manifest();
    sign_archive(&original, &mut metadata, &signing).unwrap();
    let wire = sign_manifest(&metadata, &signing).unwrap();
    for body in [&b"archive"[..], &b"changed"[..], &b"arc"[..]] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/", listener.local_addr().unwrap());
        let wire = wire.clone();
        let body = body.to_vec();
        let valid = body == b"archive";
        let task = tokio::spawn(async move {
            for bytes in [wire, body] {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).await;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(&bytes).await.unwrap();
            }
        });
        let source = HttpSource::new(
            &address,
            keys.clone(),
            Version::new(0, 1, 0),
            target("macos", "aarch64"),
        )
        .unwrap();
        let destination = root.path().join(if valid { "valid" } else { "invalid" });
        let result = source.download(&destination).await;
        assert_eq!(result.is_ok(), valid);
        if valid {
            assert_eq!(std::fs::read(&destination).unwrap(), b"archive");
        } else {
            assert!(!destination.exists());
        }
        task.await.unwrap();
    }
}

#[test]
fn signed_metadata_rejects_unsafe_names_and_resource_limits() {
    let signing = key();
    let keys = trust(vec![signing.verifying_key()]);
    for name in [
        "../archive",
        "/archive",
        "https://elsewhere.invalid/a",
        "a/b",
        "a\\b",
        "a?x",
        "a#x",
        "..",
        ".",
    ] {
        let mut metadata = manifest();
        metadata.archive = name.into();
        let wire = sign_manifest(&metadata, &signing).unwrap();
        assert!(
            verify_manifest(
                &wire,
                &keys,
                Version::new(0, 1, 0),
                &target("macos", "aarch64")
            )
            .is_err(),
            "{name}"
        );
    }
    for size in [0, u64::MAX] {
        let mut metadata = manifest();
        metadata.size = size;
        assert!(
            verify_manifest(
                &sign_manifest(&metadata, &signing).unwrap(),
                &keys,
                Version::new(0, 1, 0),
                &target("macos", "aarch64")
            )
            .is_err()
        );
    }
    assert!(
        verify_manifest(
            &vec![b' '; 65537],
            &keys,
            Version::new(0, 1, 0),
            &target("macos", "aarch64")
        )
        .is_err()
    );
}

#[tokio::test]
async fn failed_download_does_not_remove_a_file_that_already_existed() {
    let root = tempfile::Builder::new()
        .prefix("junto-release-")
        .tempdir()
        .unwrap();
    let original = root.path().join("original");
    std::fs::write(&original, b"keep").unwrap();
    let signing = key();
    let mut metadata = manifest();
    sign_archive(&original, &mut metadata, &signing).unwrap();
    let body = sign_manifest(&metadata, &signing).unwrap();
    let mut response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend(body);
    let (address, task) = server(response, Duration::ZERO).await;
    let source = HttpSource::new(
        &address,
        trust(vec![signing.verifying_key()]),
        Version::new(0, 1, 0),
        target("macos", "aarch64"),
    )
    .unwrap();
    assert!(source.download(&original).await.is_err());
    assert_eq!(std::fs::read(&original).unwrap(), b"keep");
    task.await.unwrap();
}

#[test]
fn a_freshness_date_more_than_45_days_ahead_is_not_valid() {
    let signing = key();
    let keys = trust(vec![signing.verifying_key()]);
    let now = 1_800_000_000;
    let day = 24 * 60 * 60;
    for (days, valid) in [(1, true), (45, true), (46, false), (3650, false)] {
        let mut metadata = manifest();
        metadata.expires = now + days * day;
        let result = junto_client_core::releases::verify_manifest_at(
            &sign_manifest(&metadata, &signing).unwrap(),
            &keys,
            Version::new(0, 1, 0),
            &target("macos", "aarch64"),
            now,
        );
        assert_eq!(result.is_ok(), valid, "{days} days");
        if !valid {
            assert!(matches!(
                result,
                Err(junto_client_core::releases::Error::Expired)
            ));
        }
    }
}

#[tokio::test]
async fn the_release_of_an_installed_version_is_requested_by_its_name_without_freshness() {
    let root = tempfile::Builder::new()
        .prefix("junto-release-")
        .tempdir()
        .unwrap();
    let original = root.path().join("original");
    std::fs::write(&original, b"archive").unwrap();
    let signing = key();
    let keys = trust(vec![signing.verifying_key()]);
    for (stated, valid) in [("0.1.0", true), ("0.2.0", false)] {
        let mut metadata = manifest();
        metadata.version = stated.into();
        metadata.expires = 1;
        sign_archive(&original, &mut metadata, &signing).unwrap();
        let mut information: serde_json::Value =
            serde_json::from_slice(&sign_manifest(&metadata, &signing).unwrap()).unwrap();
        information.as_object_mut().unwrap().remove("freshness");
        let information = serde_json::to_vec(&information).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}/", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let mut paths = Vec::new();
            for bytes in [information, b"archive".to_vec()] {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut request = [0; 4096];
                let count = stream.read(&mut request).await.unwrap();
                let line = String::from_utf8_lossy(&request[..count]).into_owned();
                paths.push(line.split(' ').nth(1).unwrap_or_default().to_owned());
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                stream.write_all(response.as_bytes()).await.unwrap();
                stream.write_all(&bytes).await.unwrap();
                if !valid {
                    break;
                }
            }
            paths
        });
        let source = HttpSource::new(
            &address,
            keys.clone(),
            Version::new(0, 3, 0),
            target("macos", "aarch64"),
        )
        .unwrap();
        let destination = root.path().join(format!("download-{stated}"));
        let result = source
            .download_version(Version::new(0, 1, 0), &destination)
            .await;
        assert_eq!(result.is_ok(), valid, "{stated}");
        let paths = task.await.unwrap();
        assert_eq!(paths[0], "/release-0.1.0-macos-bundle-aarch64.json");
        if valid {
            assert_eq!(paths[1], "/junto-0.2.0.junto");
            assert_eq!(std::fs::read(&destination).unwrap(), b"archive");
        } else {
            assert_eq!(paths.len(), 1);
            assert!(!destination.exists());
        }
    }
}

#[tokio::test]
async fn a_product_build_refuses_another_release_source_before_it_reads_the_installation() {
    if junto_client_core::releases::TEST_CONFIGURATION {
        return;
    }
    let refused =
        junto_client_core::releases::update_running_from(Some("http://127.0.0.1:9/")).await;
    assert!(
        matches!(&refused, Err(junto_client_core::releases::Error::Invalid(reason)) if reason.contains("test configuration")),
        "{:?}",
        refused.err()
    );
}

/// The update source is the directory `updates/` of the release site (ADR 0727): the client
/// takes the download page and the notes page of a version from it.
#[test]
fn a_source_in_the_updates_directory_names_the_site_and_the_notes_of_a_version() {
    let keys = trust(vec![key().verifying_key()]);
    let source = HttpSource::new(
        "https://releases.example.test/junto/updates/",
        keys.clone(),
        Version::new(0, 1, 0),
        target("linux", "x86_64"),
    )
    .unwrap();
    assert_eq!(
        source.site().map(String::from).as_deref(),
        Some("https://releases.example.test/junto/")
    );
    assert_eq!(
        source.notes_page(Version::new(0, 1, 1)).as_deref(),
        Some("https://releases.example.test/junto/notes/0.1.1.html")
    );
    let other = HttpSource::new(
        "https://releases.example.test/junto/",
        keys,
        Version::new(0, 1, 0),
        target("linux", "x86_64"),
    )
    .unwrap();
    assert_eq!(other.site(), None);
    assert_eq!(other.notes_page(Version::new(0, 1, 1)), None);
}

/// A directory that the Debian package installed names the package `deb` in its
/// description; the client then installs no update itself (ADR 0728).
#[test]
fn a_directory_of_the_debian_package_is_a_system_package() {
    use junto_client_core::releases::install;
    let directory = tempfile::tempdir().unwrap();
    let described = |package: &str| {
        std::fs::write(
            directory.path().join(install::DESCRIPTION),
            format!(
                r#"{{"format":1,"version":"0.1.0","system":"linux","package":"{package}","architecture":"x86_64"}}"#
            ),
        )
        .unwrap();
    };
    described("deb");
    assert!(install::is_system_package(directory.path()));
    described("directory");
    assert!(!install::is_system_package(directory.path()));
    std::fs::remove_file(directory.path().join(install::DESCRIPTION)).unwrap();
    assert!(!install::is_system_package(directory.path()));
}
