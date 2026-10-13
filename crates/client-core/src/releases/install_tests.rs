#![allow(clippy::unwrap_used)]
use super::*;
use crate::releases::{Manifest, RemoveFile, TrustedKeys, sign_archive};
use ed25519_dalek::SigningKey;
use std::cell::RefCell;

#[derive(Default)]
struct Fake {
    events: RefCell<Vec<String>>,
    wrong_requirement: bool,
    invalid_signature: bool,
    fail_move: bool,
    fail_exchange: bool,
    bad_mode: bool,
}
impl System for Fake {
    fn requirement(&self, bundle: &Path) -> Result<String, Error> {
        self.events.borrow_mut().push("requirement".into());
        let identity = fs::read_to_string(bundle.join("identity"))?;
        if identity == "adhoc" {
            return Ok("cdhash unsigned".into());
        }
        Ok(format!(
            "identifier \"org.junto.app\" and certificate leaf = H\"{}\"",
            if self.wrong_requirement && bundle.to_string_lossy().contains("unpacked") {
                "b".repeat(40)
            } else {
                if identity == "certificate" {
                    "a".repeat(40)
                } else {
                    "b".repeat(40)
                }
            }
        ))
    }
    fn matches(&self, _bundle: &Path, _required: &str) -> Result<(), Error> {
        self.events.borrow_mut().push("matches".into());
        if self.invalid_signature {
            Err(Error::Invalid("signature"))
        } else {
            Ok(())
        }
    }
    fn version(&self, bundle: &Path) -> Result<Version, Error> {
        #[cfg(unix)]
        if self.bad_mode && bundle.to_string_lossy().contains("unpacked") {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(bundle, fs::Permissions::from_mode(0o777))?;
        }
        Version::parse(&fs::read_to_string(bundle.join("version"))?)
            .map_err(|_| Error::Invalid("version"))
    }
    fn rename(&self, first: &Path, second: &Path) -> Result<(), Error> {
        if self.fail_move && first.file_name().is_some_and(|name| name == "backup") {
            return Err(Error::Invalid("move failed"));
        }
        Ok(fs::rename(first, second)?)
    }
    fn exchange(&self, first: &Path, second: &Path) -> Result<(), Error> {
        self.events.borrow_mut().push("exchange".into());
        if self.fail_exchange {
            return Err(Error::Invalid("exchange failed"));
        }
        let hold = first.with_extension("hold");
        fs::rename(first, &hold)?;
        fs::rename(second, first)?;
        fs::rename(hold, second)?;
        Ok(())
    }
}
fn bundle(root: &Path, version: &str, identity: &str) {
    fs::create_dir_all(root).unwrap();
    fs::write(root.join("version"), version).unwrap();
    fs::write(root.join("identity"), identity).unwrap();
    fs::create_dir_all(root.join("Contents/MacOS")).unwrap();
    fs::write(
        root.join("Contents/MacOS/junto-release"),
        b"recovery executable",
    )
    .unwrap();
}
fn archive(root: &Path, version: &str, identity: &str) -> VerifiedArchive {
    let source = root.join("source");
    bundle(
        &source.join("Junto.app"),
        version,
        if identity == "extra" {
            "certificate"
        } else {
            identity
        },
    );
    if identity == "extra" {
        fs::write(source.join("extra"), b"extra").unwrap();
    }
    let path = root.join("archive");
    let mut file = File::create(&path).unwrap();
    super::super::archive::pack(&source, &mut file).unwrap();
    drop(file);
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).unwrap();
    let key = SigningKey::from_bytes(&seed);
    let mut manifest = Manifest {
        format: 1,
        version: version.into(),
        system: "macos".into(),
        architecture: std::env::consts::ARCH.into(),
        archive: "archive".into(),
        size: 0,
        sha256: String::new(),
        expires: u64::MAX,
        archive_signatures: vec![],
    };
    sign_archive(&path, &mut manifest, &key).unwrap();
    let wire = crate::releases::sign_manifest(&manifest, &key, &key).unwrap();
    let app = root.join("Junto.app");
    if app.exists() {
        let old_path = root.join("old-archive");
        let mut old_file = File::create(&old_path).unwrap();
        super::super::archive::pack_bundle(&app, &mut old_file).unwrap();
        let mut old = manifest.clone();
        old.version = fs::read_to_string(app.join("version")).unwrap();
        sign_archive(&old_path, &mut old, &key).unwrap();
        fs::create_dir_all(root.join(".Junto.releases")).unwrap();
        fs::write(
            root.join(".Junto.releases")
                .join(format!("{}.json", old.version)),
            crate::releases::sign_manifest(&old, &key, &key).unwrap(),
        )
        .unwrap();
    }
    VerifiedArchive {
        file: File::open(&path).unwrap(),
        manifest,
        wire,
        cleanup: RemoveFile(path),
        keys: TrustedKeys::new(vec![key.verifying_key()])
            .with_freshness(TrustedKeys::new(vec![key.verifying_key()])),
    }
}
#[test]
fn verification_precedes_platform_tools_and_exchange() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("Junto.app");
    bundle(&app, "1.0.0", "certificate");
    let mut data = archive(root.path(), "2.0.0", "certificate");
    let installer = BundleInstaller::with_system(&app, Fake::default()).unwrap();
    data.manifest.archive_signatures.clear();
    assert!(installer.install(&mut data).is_err());
    assert!(installer.system.events.borrow().is_empty());
    assert_eq!(
        installer.system.version(&app).unwrap(),
        Version::new(1, 0, 0)
    );
}
#[test]
fn every_candidate_identity_and_version_check_prevents_exchange() {
    for case in [
        "adhoc", "other", "mismatch", "invalid", "equal", "older", "system", "arch", "version",
        "extra", "mode",
    ] {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("Junto.app");
        bundle(&app, "1.0.0", "certificate");
        let version = match case {
            "equal" => "1.0.0",
            "older" => "0.9.0",
            _ => "2.0.0",
        };
        let identity = match case {
            "adhoc" => "adhoc",
            "other" => "other",
            "extra" => "extra",
            _ => "certificate",
        };
        let mut data = archive(root.path(), version, identity);
        let fake = Fake {
            invalid_signature: case == "invalid",
            wrong_requirement: case == "mismatch",
            bad_mode: case == "mode",
            ..Fake::default()
        };
        if case == "mode" && !cfg!(unix) {
            continue;
        }
        match case {
            "system" => data.manifest.system = "linux".into(),
            "arch" => data.manifest.architecture = "wrong".into(),
            "version" => {
                data.manifest.version = "3.0.0".into();
            }
            _ => {}
        }
        let installer = BundleInstaller::with_system(&app, fake).unwrap();
        assert!(installer.install(&mut data).is_err(), "{case}");
        assert!(
            !installer
                .system
                .events
                .borrow()
                .iter()
                .any(|event| event == "exchange"),
            "{case}"
        );
        assert_eq!(
            installer.system.version(&app).unwrap(),
            Version::new(1, 0, 0)
        );
    }
}
#[test]
fn failures_restore_previous_and_success_replaces_damaged_backup() {
    for failure in ["move", "exchange", "none"] {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("Junto.app");
        bundle(&app, "1.0.0", "certificate");
        let previous = root.path().join(".Junto.previous");
        fs::create_dir(&previous).unwrap();
        fs::write(previous.join("marker"), "old recovery").unwrap();
        let mut data = archive(root.path(), "2.0.0", "certificate");
        let installer = BundleInstaller::with_system(
            &app,
            Fake {
                fail_move: failure == "move",
                fail_exchange: failure == "exchange",
                ..Fake::default()
            },
        )
        .unwrap();
        let result = installer.install(&mut data);
        if failure == "none" {
            assert!(result.is_ok());
            assert!(previous.join("archive.junto").is_file());
            assert!(!previous.join("marker").exists());
            assert_eq!(
                installer.system.version(&app).unwrap(),
                Version::new(2, 0, 0)
            );
        } else {
            assert!(result.is_err());
            assert_eq!(
                installer.system.version(&app).unwrap(),
                Version::new(1, 0, 0)
            );
            assert_eq!(
                fs::read_to_string(previous.join("marker")).unwrap(),
                "old recovery"
            );
        }
        assert!(!fs::read_dir(root.path()).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".Junto.stage-")
        }));
    }
}
#[test]
fn rollback_verifies_retained_archive_and_proof_without_trusting_broken_current() {
    for fault in [
        "none",
        "archive",
        "proof",
        "installed",
        "not-older",
        "pin",
        "platform",
    ] {
        let root = tempfile::tempdir().unwrap();
        let app = root.path().join("Junto.app");
        bundle(&app, "1.0.0", "certificate");
        let mut data = archive(root.path(), "2.0.0", "certificate");
        let mut installer = BundleInstaller::with_system(&app, Fake::default())
            .unwrap()
            .with_trust(data.keys.clone(), "a".repeat(40));
        installer.install(&mut data).unwrap();
        let previous = installer.previous().unwrap();
        fs::write(app.join("identity"), "damaged current").unwrap();
        match fault {
            "archive" => {
                let mut bytes = fs::read(previous.join("archive.junto")).unwrap();
                let offset = bytes
                    .windows(b"recovery executable".len())
                    .position(|part| part == b"recovery executable")
                    .unwrap();
                bytes[offset..offset + b"modified executable".len()]
                    .copy_from_slice(b"modified executable");
                fs::write(previous.join("archive.junto"), bytes).unwrap();
            }
            "proof" => {
                tamper_proof(&previous.join("release.json"));
            }
            "installed" => {
                tamper_proof(&previous.join("installed.json"));
            }
            "not-older" => {
                fs::copy(
                    previous.join("release.json"),
                    previous.join("installed.json"),
                )
                .unwrap();
                fs::write(app.join("version"), "1.0.0").unwrap();
            }
            "pin" => {
                installer.pin = "b".repeat(40);
            }
            "platform" => {
                installer.system.invalid_signature = true;
            }
            _ => {}
        }
        installer.system.events.borrow_mut().clear();
        let result = installer.rollback();
        if fault == "none" {
            result.unwrap();
            assert_eq!(
                installer.system.version(&app).unwrap(),
                Version::new(1, 0, 0)
            );
            assert!(installer.rollback().is_err());
        } else {
            assert!(result.is_err(), "{fault}");
            assert!(
                !installer
                    .system
                    .events
                    .borrow()
                    .iter()
                    .any(|event| event == "exchange"),
                "{fault}"
            );
        }
        assert!(!root.path().join("Junto.previous.app").exists());
    }
}
#[test]
fn concurrent_install_is_refused_before_tools() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("Junto.app");
    bundle(&app, "1.0.0", "certificate");
    let installer = BundleInstaller::with_system(&app, Fake::default()).unwrap();
    let _lock = installer.lock().unwrap();
    let second = BundleInstaller::with_system(&app, Fake::default()).unwrap();
    assert!(matches!(second.lock(), Err(Error::Locked)));
    let mut data = archive(root.path(), "2.0.0", "certificate");
    assert!(matches!(second.install(&mut data), Err(Error::Locked)));
    assert!(matches!(second.rollback(), Err(Error::Locked)));
    assert!(second.system.events.borrow().is_empty());
}

#[test]
fn retained_previous_release_is_not_an_application_bundle() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("Junto.app");
    bundle(&app, "1.0.0", "certificate");
    let mut data = archive(root.path(), "2.0.0", "certificate");
    let installer = BundleInstaller::with_system(&app, Fake::default()).unwrap();
    installer.install(&mut data).unwrap();
    assert!(!root.path().join("Junto.previous.app").exists());
    assert!(
        installer
            .previous()
            .unwrap()
            .join("archive.junto")
            .is_file()
    );
}
#[test]
fn translocated_and_readonly_parents_cannot_update() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("AppTranslocation/random/Junto.app");
    bundle(&app, "1.0.0", "certificate");
    assert!(matches!(install_location(&app), Err(Error::NotWritable)));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let parent = root.path().join("readonly");
        let app = parent.join("Junto.app");
        bundle(&app, "1.0.0", "certificate");
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
        let result = install_location(&app);
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(result, Err(Error::NotWritable)));
    }
}

#[test]
fn two_adhoc_bundles_do_not_establish_a_certificate_identity() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("Junto.app");
    bundle(&app, "1.0.0", "adhoc");
    let mut data = archive(root.path(), "2.0.0", "adhoc");
    let installer = BundleInstaller::with_system(&app, Fake::default()).unwrap();
    assert!(installer.install(&mut data).is_err());
    assert!(
        !installer
            .system
            .events
            .borrow()
            .iter()
            .any(|event| event == "exchange")
    );
}

#[test]
fn location_check_does_not_write_to_the_application_directory() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("Junto.app");
    bundle(&app, "1.0.0", "certificate");
    let before = fs::metadata(root.path()).unwrap().modified().unwrap();
    install_location(&app).unwrap();
    assert_eq!(
        before,
        fs::metadata(root.path()).unwrap().modified().unwrap()
    );
    assert!(
        application_for_executable(&root.path().join("Junto.previous.app/Contents/MacOS/junto"))
            .is_err()
    );
}

fn tamper_proof(path: &Path) {
    use base64::{Engine, prelude::BASE64_STANDARD};
    let mut value: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let mut payload: serde_json::Value = serde_json::from_slice(
        &BASE64_STANDARD
            .decode(value["payload"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    payload["archive"] = serde_json::json!("changed.junto");
    value["payload"] =
        serde_json::json!(BASE64_STANDARD.encode(serde_json::to_vec(&payload).unwrap()));
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}
