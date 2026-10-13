#![allow(clippy::unwrap_used)]
use base64::{Engine, prelude::BASE64_STANDARD};
use ed25519_dalek::SigningKey;
use junto_client_core::{
    releases::{self, Error, Manifest, TrustedKeys},
    updates::Version,
};
use std::{fs, path::PathBuf, process::Command};

fn key() -> SigningKey {
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).unwrap();
    SigningKey::from_bytes(&seed)
}
fn manifest() -> Manifest {
    Manifest {
        format: 1,
        version: "2.0.0".into(),
        system: "macos".into(),
        architecture: "aarch64".into(),
        archive: "release.junto".into(),
        size: 7,
        sha256: "00".repeat(32),
        expires: u64::MAX,
        archive_signatures: vec![],
    }
}
fn trusted(release: &SigningKey, freshness: &SigningKey) -> TrustedKeys {
    TrustedKeys::new(vec![release.verifying_key()])
        .with_freshness(TrustedKeys::new(vec![freshness.verifying_key()]))
}
fn verify(wire: &[u8], keys: &TrustedKeys) -> Result<Manifest, Error> {
    releases::verify_manifest(wire, keys, Version::new(1, 0, 0), "macos", "aarch64")
}
#[test]
fn release_information_without_freshness_is_refused() {
    let release = key();
    let freshness = key();
    let wire = releases::sign_manifest(&manifest(), &release, &freshness).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&wire).unwrap();
    value.as_object_mut().unwrap().remove("freshness");
    assert!(matches!(
        verify(
            &serde_json::to_vec(&value).unwrap(),
            &trusted(&release, &freshness)
        ),
        Err(Error::Expired)
    ));
}
#[test]
fn invalid_freshness_signature_reports_that_no_valid_statement_is_available() {
    let release = key();
    let freshness = key();
    let wire = releases::sign_manifest(&manifest(), &release, &freshness).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&wire).unwrap();
    value["freshness"]["signatures"][0]["signature"] =
        serde_json::json!(BASE64_STANDARD.encode([0; 64]));
    let error = verify(
        &serde_json::to_vec(&value).unwrap(),
        &trusted(&release, &freshness),
    )
    .unwrap_err();
    assert!(matches!(error, Error::Expired));
    assert!(error.to_string().contains("check the system clock"));
}
#[test]
fn freshness_keys_cannot_authorize_code_and_statements_bind_exact_release_bytes() {
    let release = key();
    let freshness = key();
    let keys = trusted(&release, &freshness);
    assert!(matches!(
        verify(
            &releases::sign_manifest(&manifest(), &freshness, &freshness).unwrap(),
            &keys
        ),
        Err(Error::UntrustedKeys)
    ));
    let wire = releases::sign_manifest(&manifest(), &release, &freshness).unwrap();
    assert!(verify(&wire, &keys).is_ok());
    let mut other = manifest();
    other.version = "3.0.0".into();
    let other: serde_json::Value =
        serde_json::from_slice(&releases::sign_manifest(&other, &release, &freshness).unwrap())
            .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&wire).unwrap();
    value["freshness"] = other["freshness"].clone();
    assert!(matches!(
        verify(&serde_json::to_vec(&value).unwrap(), &keys),
        Err(Error::Invalid("freshness release binding"))
    ));
    value.as_object_mut().unwrap().remove("payload");
    assert!(verify(&serde_json::to_vec(&value).unwrap(), &keys).is_err());
    let mut value: serde_json::Value = serde_json::from_slice(&wire).unwrap();
    value["payload"] = serde_json::json!(BASE64_STANDARD.encode(b"changed information"));
    assert!(verify(&serde_json::to_vec(&value).unwrap(), &keys).is_err());
}
#[test]
fn both_key_roles_can_rotate_independently() {
    let old_release = key();
    let new_release = key();
    let old_freshness = key();
    let new_freshness = key();
    let wire = releases::sign_manifest_many(
        &manifest(),
        &[&old_release, &new_release],
        &[&old_freshness, &new_freshness],
    )
    .unwrap();
    for release in [&old_release, &new_release] {
        for freshness in [&old_freshness, &new_freshness] {
            assert!(verify(&wire, &trusted(release, freshness)).is_ok());
        }
    }
    let old = releases::sign_manifest(&manifest(), &old_release, &old_freshness).unwrap();
    assert!(verify(&old, &trusted(&new_release, &old_freshness)).is_err());
    assert!(verify(&old, &trusted(&old_release, &new_freshness)).is_err());
}

struct Files {
    root: tempfile::TempDir,
    metadata: PathBuf,
    archive: PathBuf,
    public: PathBuf,
    fresh_public: PathBuf,
    secret: PathBuf,
    fresh_secret: PathBuf,
}
impl Files {
    fn new(version: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let tool = env!("CARGO_BIN_EXE_junto-release");
        let secret = root.path().join("release.seed");
        let public = root.path().join("release.json");
        let fresh_secret = root.path().join("freshness.seed");
        let fresh_public = root.path().join("freshness.json");
        for (secret, public) in [(&secret, &public), (&fresh_secret, &fresh_public)] {
            assert!(
                Command::new(tool)
                    .arg("keygen")
                    .arg(secret)
                    .arg(public)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        let source = root.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(source.join("file"), "authentic").unwrap();
        let output = root.path().join("release");
        assert!(
            Command::new(tool)
                .arg("package")
                .arg(&source)
                .arg(&output)
                .arg(&secret)
                .arg(&fresh_secret)
                .args([version, "macos", "aarch64"])
                .status()
                .unwrap()
                .success()
        );
        Self {
            metadata: output.join("latest-macos-aarch64.json"),
            archive: output.join(format!("junto-{version}-macos-aarch64.junto")),
            root,
            public,
            fresh_public,
            secret,
            fresh_secret,
        }
    }
    fn refresh(&self, version: &str) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_junto-release"))
            .arg("--test-configuration")
            .arg(&self.public)
            .arg(&self.fresh_public)
            .arg("refresh")
            .arg(&self.metadata)
            .arg(&self.archive)
            .arg(version)
            .arg(&self.fresh_secret)
            .output()
            .unwrap()
    }
    fn keys(&self) -> TrustedKeys {
        TrustedKeys::from_json(&fs::read(&self.public).unwrap())
            .unwrap()
            .with_freshness(TrustedKeys::from_json(&fs::read(&self.fresh_public).unwrap()).unwrap())
    }
}
#[test]
fn refresh_refuses_changed_archive_information_unknown_keys_and_wrong_version() {
    for case in ["archive", "information", "unknown", "older"] {
        let files = Files::new(if case == "older" { "1.5.0" } else { "2.0.0" });
        if case == "archive" {
            fs::write(&files.archive, b"attacker bytes").unwrap();
        }
        if case == "information" {
            let mut value: serde_json::Value =
                serde_json::from_slice(&fs::read(&files.metadata).unwrap()).unwrap();
            let mut payload: serde_json::Value = serde_json::from_slice(
                &BASE64_STANDARD
                    .decode(value["payload"].as_str().unwrap())
                    .unwrap(),
            )
            .unwrap();
            payload["archive"] = serde_json::json!("altered.junto");
            value["payload"] =
                serde_json::json!(BASE64_STANDARD.encode(serde_json::to_vec(&payload).unwrap()));
            fs::write(&files.metadata, serde_json::to_vec(&value).unwrap()).unwrap();
        }
        if case == "unknown" {
            fs::write(&files.public, b"[]").unwrap();
        }
        let before = fs::read(&files.metadata).unwrap();
        let result = files.refresh("2.0.0");
        assert!(
            !result.status.success(),
            "{case}: refresh accepted unverified release"
        );
        assert_eq!(fs::read(&files.metadata).unwrap(), before);
        assert!(
            !fs::read_dir(files.metadata.parent().unwrap())
                .unwrap()
                .any(|item| item
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".Junto.write-"))
        );
    }
}
#[test]
fn refresh_uses_only_freshness_keys_and_preserves_release_authorization() {
    let files = Files::new("2.0.0");
    let before: serde_json::Value =
        serde_json::from_slice(&fs::read(&files.metadata).unwrap()).unwrap();
    fs::remove_file(&files.secret).unwrap();
    let result = files.refresh("2.0.0");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let wire = fs::read(&files.metadata).unwrap();
    let after: serde_json::Value = serde_json::from_slice(&wire).unwrap();
    assert_eq!(after["payload"], before["payload"]);
    assert_eq!(after["signatures"], before["signatures"]);
    let manifest = verify(&wire, &files.keys()).unwrap();
    releases::verify_archive(&files.archive, &manifest, &files.keys()).unwrap();
    assert!(files.root.path().is_dir());
}

#[test]
fn release_tool_supports_overlapping_keys_for_both_roles() {
    let files = Files::new("2.0.0");
    let tool = env!("CARGO_BIN_EXE_junto-release");
    let next_release = files.root.path().join("next-release.seed");
    let next_release_public = files.root.path().join("next-release.json");
    let next_fresh = files.root.path().join("next-fresh.seed");
    let next_fresh_public = files.root.path().join("next-fresh.json");
    for (seed, public) in [
        (&next_release, &next_release_public),
        (&next_fresh, &next_fresh_public),
    ] {
        assert!(
            Command::new(tool)
                .arg("keygen")
                .arg(seed)
                .arg(public)
                .status()
                .unwrap()
                .success()
        );
    }
    let output = files.root.path().join("overlap");
    assert!(
        Command::new(tool)
            .arg("package")
            .arg(files.root.path().join("source"))
            .arg(&output)
            .arg(&files.secret)
            .arg(&files.fresh_secret)
            .args(["2.0.0", "macos", "aarch64"])
            .arg(&next_release)
            .status()
            .unwrap()
            .success()
    );
    let mut fresh_keys: Vec<String> =
        serde_json::from_slice(&fs::read(&files.fresh_public).unwrap()).unwrap();
    fresh_keys.extend(
        serde_json::from_slice::<Vec<String>>(&fs::read(&next_fresh_public).unwrap()).unwrap(),
    );
    let both_fresh = files.root.path().join("both-fresh.json");
    fs::write(&both_fresh, serde_json::to_vec(&fresh_keys).unwrap()).unwrap();
    let metadata = output.join("latest-macos-aarch64.json");
    let archive = output.join("junto-2.0.0-macos-aarch64.junto");
    assert!(
        Command::new(tool)
            .arg("--test-configuration")
            .arg(&files.public)
            .arg(&both_fresh)
            .arg("refresh")
            .arg(&metadata)
            .arg(&archive)
            .arg("2.0.0")
            .arg(&files.fresh_secret)
            .arg(&next_fresh)
            .status()
            .unwrap()
            .success()
    );
    let wire = fs::read(metadata).unwrap();
    for release in [&files.public, &next_release_public] {
        for freshness in [&files.fresh_public, &next_fresh_public] {
            let keys = TrustedKeys::from_json(&fs::read(release).unwrap())
                .unwrap()
                .with_freshness(TrustedKeys::from_json(&fs::read(freshness).unwrap()).unwrap());
            let manifest = verify(&wire, &keys).unwrap();
            releases::verify_archive(&archive, &manifest, &keys).unwrap();
        }
    }
}
