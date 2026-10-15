#![allow(clippy::unwrap_used)]
use base64::{Engine, prelude::BASE64_STANDARD};
use ed25519_dalek::SigningKey;
use junto_client_core::{
    releases::{self, Error, Manifest, TrustedKeys},
    updates::Version,
};
use std::{fs, path::PathBuf, process::Command};

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
        package: "bundle".into(),
        architecture: "aarch64".into(),
        archive: "release.junto".into(),
        size: 7,
        sha256: "00".repeat(32),
        expires: soon(),
        archive_signatures: vec![],
    }
}
fn trusted(release: &SigningKey, freshness: &SigningKey) -> TrustedKeys {
    TrustedKeys::new(vec![release.verifying_key()])
        .with_freshness(TrustedKeys::new(vec![freshness.verifying_key()]))
}
fn verify(wire: &[u8], keys: &TrustedKeys) -> Result<Manifest, Error> {
    releases::verify_manifest(
        wire,
        keys,
        Version::new(1, 0, 0),
        &target("macos", "aarch64"),
    )
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
fn release_keys_cannot_sign_freshness_and_the_binding_compares_the_release_bytes() {
    let release = key();
    let freshness = key();
    let keys = trusted(&release, &freshness);
    // A freshness statement signed by a trusted release key is signed by no trusted key.
    assert!(matches!(
        verify(
            &releases::sign_manifest(&manifest(), &release, &release).unwrap(),
            &keys
        ),
        Err(Error::UntrustedKeys)
    ));
    // A statement for other release information of the same version.
    let wire = releases::sign_manifest(&manifest(), &release, &freshness).unwrap();
    let mut changed = manifest();
    changed.sha256 = "11".repeat(32);
    let other: serde_json::Value =
        serde_json::from_slice(&releases::sign_manifest(&changed, &release, &freshness).unwrap())
            .unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&wire).unwrap();
    value["freshness"] = other["freshness"].clone();
    assert!(matches!(
        verify(&serde_json::to_vec(&value).unwrap(), &keys),
        Err(Error::Invalid("freshness release binding"))
    ));
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

fn seed(path: &std::path::Path) -> SigningKey {
    SigningKey::from_bytes(&fs::read(path).unwrap().try_into().unwrap())
}
fn refresh(
    metadata: &std::path::Path,
    archive: &std::path::Path,
    version: &str,
    keys: &TrustedKeys,
    secrets: &[&PathBuf],
) -> Result<(), Error> {
    let secrets: Vec<SigningKey> = secrets.iter().map(|path| seed(path)).collect();
    let wire = releases::refresh_release(
        &fs::read(metadata).unwrap(),
        archive,
        Version::parse(version).unwrap(),
        keys,
        &secrets.iter().collect::<Vec<_>>(),
    )?;
    releases::write_atomic(metadata, &wire)
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
                .args([version, "macos", "bundle", "aarch64"])
                .status()
                .unwrap()
                .success()
        );
        Self {
            metadata: output.join("latest-macos-bundle-aarch64.json"),
            archive: output.join(format!("junto-{version}-macos-bundle-aarch64.junto")),
            root,
            public,
            fresh_public,
            secret,
            fresh_secret,
        }
    }
    /// The refresh command of the release tool, with the trust sets of these files: the
    /// tool itself uses the sets compiled into it.
    fn refresh(&self, version: &str) -> Result<(), Error> {
        refresh(
            &self.metadata,
            &self.archive,
            version,
            &self.keys(),
            &[&self.fresh_secret],
        )
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
            result.is_err(),
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
    files.refresh("2.0.0").unwrap();
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
            .args(["2.0.0", "macos", "bundle", "aarch64"])
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
    let metadata = output.join("latest-macos-bundle-aarch64.json");
    let archive = output.join("junto-2.0.0-macos-bundle-aarch64.junto");
    let keys = TrustedKeys::from_json(&fs::read(&files.public).unwrap())
        .unwrap()
        .with_freshness(TrustedKeys::from_json(&fs::read(&both_fresh).unwrap()).unwrap());
    refresh(
        &metadata,
        &archive,
        "2.0.0",
        &keys,
        &[&files.fresh_secret, &next_fresh],
    )
    .unwrap();
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
