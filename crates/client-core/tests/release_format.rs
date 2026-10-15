#![allow(clippy::unwrap_used)]
use base64::{Engine, prelude::BASE64_STANDARD};
use ed25519_dalek::{Signer, SigningKey};
use junto_client_core::releases::{self, Error, TrustedKeys};
use junto_client_core::updates::Version;
use serde_json::json;
use sha2::{Digest, Sha256};

fn target(system: &str, architecture: &str) -> junto_client_core::releases::Target {
    junto_client_core::releases::Target::new(system, "bundle", architecture).unwrap()
}

fn trust(keys: Vec<ed25519_dalek::VerifyingKey>) -> TrustedKeys {
    TrustedKeys::new(keys.clone()).with_freshness(TrustedKeys::new(keys))
}

fn key() -> SigningKey {
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).unwrap();
    SigningKey::from_bytes(&seed)
}
/// An expiry within the accepted range: one day after the clock of the test.
fn soon() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 24 * 60 * 60
}
fn payload(expires: u64) -> serde_json::Value {
    json!({"format":1,"version":"2.0.0","system":"macos","package":"bundle","architecture":"aarch64",
        "archive":"release.junto","size":7,"sha256":"00".repeat(32),"expires":expires,
        "archive_signatures":[]})
}
fn wire(payload: &serde_json::Value, keys: &[&SigningKey]) -> Vec<u8> {
    let bytes = serde_json::to_vec(payload).unwrap();
    let signed = [b"junto-release-metadata-v1\0".as_slice(), &bytes].concat();
    let signatures: Vec<_>=keys.iter().map(|key| json!({"key":BASE64_STANDARD.encode(key.verifying_key().as_bytes()),"signature":BASE64_STANDARD.encode(key.sign(&signed).to_bytes())})).collect();
    let freshness = serde_json::to_vec(&json!({"format":1,"version":payload["version"],"release_sha256":Sha256::digest(&bytes).iter().map(|byte|format!("{byte:02x}")).collect::<String>(),"expires":payload["expires"]})).unwrap();
    let fresh_signed = [b"junto-release-freshness-v1\0".as_slice(), &freshness].concat();
    let fresh_signatures: Vec<_> = keys.iter().map(|key| json!({"key":BASE64_STANDARD.encode(key.verifying_key().as_bytes()),"signature":BASE64_STANDARD.encode(key.sign(&fresh_signed).to_bytes())})).collect();
    serde_json::to_vec(&json!({"payload":BASE64_STANDARD.encode(bytes),"signatures":signatures,"freshness":{"payload":BASE64_STANDARD.encode(freshness),"signatures":fresh_signatures}}))
        .unwrap()
}
#[test]
fn named_signatures_accept_any_trusted_key_and_cover_extension_bytes() {
    let old = key();
    let new = key();
    let keys = trust(vec![old.verifying_key()]);
    let mut payload = payload(soon());
    payload["future_feature"] = json!({"text":"signed extension"});
    let bytes = wire(&payload, &[&new, &old]);
    assert!(
        releases::verify_manifest(
            &bytes,
            &keys,
            Version::new(1, 0, 0),
            &target("macos", "aarch64")
        )
        .is_ok()
    );
    assert!(matches!(
        releases::verify_manifest(
            &wire(&payload, &[&new]),
            &keys,
            Version::new(1, 0, 0),
            &target("macos", "aarch64")
        ),
        Err(Error::UntrustedKeys)
    ));
    let mut envelope: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    payload["future_feature"] = json!("tampered");
    envelope["payload"] = json!(BASE64_STANDARD.encode(serde_json::to_vec(&payload).unwrap()));
    assert!(
        releases::verify_manifest(
            &serde_json::to_vec(&envelope).unwrap(),
            &keys,
            Version::new(1, 0, 0),
            &target("macos", "aarch64")
        )
        .is_err()
    );
}
#[test]
fn expired_information_is_not_reported_as_current() {
    let key = key();
    let keys = trust(vec![key.verifying_key()]);
    assert!(matches!(
        releases::verify_manifest(
            &wire(&payload(1), &[&key]),
            &keys,
            Version::new(2, 0, 0),
            &target("macos", "aarch64")
        ),
        Err(Error::Expired)
    ));
}
#[cfg(unix)]
#[test]
fn extraction_modes_do_not_depend_on_umask() {
    if std::env::var_os("JUNTO_MODE_CHILD").is_none() {
        let result = std::process::Command::new("/bin/sh")
            .args(["-c", "umask 000; exec \"$@\"", "sh"])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", "extraction_modes_do_not_depend_on_umask"])
            .env("JUNTO_MODE_CHILD", "1")
            .status()
            .unwrap();
        assert!(result.success());
        return;
    }
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir_all(source.join("Junto.app/Contents/MacOS")).unwrap();
    std::fs::write(source.join("Junto.app/Contents/MacOS/junto"), b"binary").unwrap();
    let mut bytes = Vec::new();
    releases::archive::pack(&source, &mut bytes).unwrap();
    let output = root.path().join("out");
    releases::archive::unpack(&mut bytes.as_slice(), &output).unwrap();
    for path in [
        "Junto.app",
        "Junto.app/Contents",
        "Junto.app/Contents/MacOS",
    ] {
        assert_eq!(
            std::fs::metadata(output.join(path))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
    }
    assert_eq!(
        std::fs::metadata(output.join("Junto.app/Contents/MacOS/junto"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o644
    );
}

#[test]
fn public_wire_fixture_fixes_both_signature_domains() {
    let keys = TrustedKeys::from_json(include_bytes!("data/release-public.json"))
        .unwrap()
        .with_freshness(
            TrustedKeys::from_json(include_bytes!("data/freshness-public.json")).unwrap(),
        );
    // The fixture's statement expires at 1794623717; it is checked one day before.
    let metadata = releases::verify_manifest_at(
        include_bytes!("data/release-v1.json"),
        &keys,
        Version::new(1, 0, 0),
        &target("macos", "aarch64"),
        1_794_623_717 - 24 * 60 * 60,
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("archive");
    std::fs::write(&file, include_bytes!("data/release-v1.junto")).unwrap();
    releases::verify_archive(&file, &metadata, &keys).unwrap();
}
#[test]
fn format_hash_and_compiled_configuration_are_checked() {
    let key = key();
    let keys = trust(vec![key.verifying_key()]);
    for field in ["format", "hash"] {
        let mut data = payload(soon());
        if field == "format" {
            data["format"] = json!(2);
        } else {
            data["sha256"] = json!("AB".repeat(32));
        }
        assert!(
            releases::verify_manifest(
                &wire(&data, &[&key]),
                &keys,
                Version::new(1, 0, 0),
                &target("macos", "aarch64")
            )
            .is_err()
        );
    }
    assert_eq!(
        include_str!("../../../packaging/update-keys.json").trim(),
        "[]"
    );
    assert_eq!(
        include_str!("../../../packaging/freshness-keys.json").trim(),
        "[]"
    );
    if option_env!("JUNTO_TEST_UPDATE_KEYS_FILE").is_none() {
        assert!(TrustedKeys::compiled().unwrap().is_empty());
        assert!(releases::HttpSource::compiled().unwrap().is_none());
    }
}

#[tokio::test]
async fn expiry_and_missing_trusted_keys_replace_an_old_available_state() {
    use junto_client_core::{
        runtime::Runtime,
        updates::{FakeSource, Release, UpdateCheck, UpdateError, UpdateState},
    };
    let source = FakeSource::new(Ok(Release::new(Version::new(2, 0, 0), None)));
    let check = UpdateCheck::new(
        Version::new(1, 0, 0),
        Some(source.clone()),
        Runtime::current_or_new().unwrap(),
    );
    assert!(matches!(check.check().await, UpdateState::Available(_)));
    source.set(Err(UpdateError::Expired));
    assert_eq!(check.check().await, UpdateState::Expired);
    source.set(Err(UpdateError::UntrustedKeys));
    assert_eq!(check.check().await, UpdateState::UntrustedKeys);
}
