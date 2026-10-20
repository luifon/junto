#![allow(clippy::unwrap_used)]
use super::*;
use crate::releases::{Manifest, RemoveFile, TrustedKeys, sign_archive};
use ed25519_dalek::SigningKey;
use std::cell::RefCell;

const APPIMAGE_MARK: &[u8] = b"\x7fELF\x02\x01\x01\x00AI\x02";

#[derive(Default)]
struct Fake {
    events: RefCell<Vec<String>>,
    wrong_requirement: bool,
    invalid_signature: bool,
    fail_move: bool,
    fail_exchange: bool,
    /// The exchange stops halfway and cannot be undone.
    incomplete_exchange: bool,
    /// The installed bundle has a broken code signature; candidates are intact.
    broken_installed: bool,
    // Modes exist on Unix only.
    #[cfg_attr(not(unix), allow(dead_code))]
    bad_mode: bool,
}
impl System for Fake {
    fn requirement(&self, bundle: &Path) -> Result<String, Error> {
        self.events.borrow_mut().push("requirement".into());
        if self.broken_installed && !bundle.to_string_lossy().contains("unpacked") {
            return Err(Error::Invalid("bundle verification failed"));
        }
        let identity = fs::read_to_string(bundle.join("identity"))?;
        if identity == "adhoc" {
            return Ok("cdhash unsigned".into());
        }
        Ok(format!(
            "identifier \"org.junto.app\" and certificate leaf = H\"{}\"",
            if self.wrong_requirement && bundle.to_string_lossy().contains("unpacked") {
                "b".repeat(40)
            } else if identity == "certificate" {
                "a".repeat(40)
            } else {
                "b".repeat(40)
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
    /// A bundle states its version in a file; an AppImage of these tests carries it after
    /// its mark, as the running program would state it.
    fn version(&self, application: &Path) -> Result<Version, Error> {
        #[cfg(unix)]
        if self.bad_mode && application.to_string_lossy().contains("unpacked") {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(application, fs::Permissions::from_mode(0o777))?;
        }
        let text = if application.is_dir() {
            fs::read_to_string(application.join("version"))?
        } else {
            String::from_utf8_lossy(&fs::read(application)?[APPIMAGE_MARK.len()..]).into_owned()
        };
        Version::parse(&text).map_err(|_| Error::Invalid("version"))
    }
    fn rename(&self, first: &Path, second: &Path) -> Result<(), Error> {
        // The move of the new record into place.
        if self.fail_move
            && first.file_name().is_some_and(|name| name == "record")
            && second.file_name().is_some_and(|name| name == "record")
        {
            return Err(Error::Invalid("move failed"));
        }
        Ok(fs::rename(first, second)?)
    }
    fn exchange(&self, application: &Path, candidate: &Path) -> Result<(), Error> {
        self.events.borrow_mut().push("exchange".into());
        if self.fail_exchange {
            return Err(Error::Invalid("exchange failed"));
        }
        if self.incomplete_exchange {
            return Err(Error::ExchangeIncomplete);
        }
        let hold = candidate.with_extension("hold");
        fs::rename(application, &hold)?;
        fs::rename(candidate, application)?;
        fs::rename(hold, candidate)?;
        Ok(())
    }
}

/// A temporary installation of one package, with its keys.
struct Setup {
    root: tempfile::TempDir,
    package: Package,
    target: Target,
    release: SigningKey,
    freshness: SigningKey,
}

fn key() -> SigningKey {
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).unwrap();
    SigningKey::from_bytes(&seed)
}

impl Setup {
    fn new(package: Package, system: &str) -> Self {
        let setup = Self {
            root: tempfile::tempdir().unwrap(),
            package,
            target: Target::new(system, package.name(), std::env::consts::ARCH).unwrap(),
            release: key(),
            freshness: key(),
        };
        setup.place("1.0.0", "certificate");
        setup
    }
    fn macos() -> Self {
        Self::new(Package::Bundle, "macos")
    }
    fn layout(&self) -> Layout {
        layout(&self.target).unwrap()
    }
    fn app(&self) -> PathBuf {
        self.root.path().join(self.layout().application)
    }
    fn keys(&self) -> TrustedKeys {
        TrustedKeys::new(vec![self.release.verifying_key()])
            .with_freshness(TrustedKeys::new(vec![self.freshness.verifying_key()]))
    }
    fn description(&self, version: &str) -> String {
        format!(
            "{{\"format\":1,\"version\":\"{version}\",\"system\":\"{}\",\"package\":\"{}\",\"architecture\":\"{}\"}}",
            self.target.system, self.target.package, self.target.architecture
        )
    }
    /// Writes the source tree of a release into `directory`.
    fn tree(&self, directory: &Path, version: &str, identity: &str) {
        let layout = self.layout();
        let recovery = directory.join(layout.recovery);
        fs::create_dir_all(recovery.parent().unwrap()).unwrap();
        fs::write(&recovery, format!("recovery program {version}")).unwrap();
        let application = directory.join(layout.application);
        match self.package {
            Package::Bundle => {
                fs::write(application.join("version"), version).unwrap();
                fs::write(application.join("identity"), identity).unwrap();
            }
            Package::Directory => {
                fs::write(application.join("program"), format!("program {version}")).unwrap();
            }
            Package::AppImage => {
                fs::write(&application, [APPIMAGE_MARK, version.as_bytes()].concat()).unwrap();
            }
        }
        if let Some(description) = layout.description {
            fs::write(directory.join(description), self.description(version)).unwrap();
        }
        #[cfg(unix)]
        if self.package == Package::AppImage {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&application, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    /// Installs `version` by placing its files, as a first install from a package does.
    fn place(&self, version: &str, identity: &str) {
        let source = self.root.path().join(format!("first-{version}"));
        self.tree(&source, version, identity);
        let app = self.app();
        if app.is_dir() {
            fs::remove_dir_all(&app).unwrap();
        } else if app.exists() {
            fs::remove_file(&app).unwrap();
        }
        fs::rename(source.join(self.layout().application), &app).unwrap();
        fs::remove_dir_all(source).unwrap();
    }
    fn manifest(&self, version: &str) -> Manifest {
        Manifest {
            format: 1,
            version: version.into(),
            system: self.target.system.clone(),
            package: self.target.package.clone(),
            architecture: self.target.architecture.clone(),
            archive: format!("junto-{version}.junto"),
            size: 0,
            sha256: String::new(),
            expires: u64::MAX,
            archive_signatures: vec![],
        }
    }
    /// A verified download of a release, as the source gives it.
    fn release(&self, version: &str, identity: &str) -> VerifiedArchive {
        let mut random = [0; 8];
        getrandom::fill(&mut random).unwrap();
        let source = self
            .root
            .path()
            .join(format!("source-{}", crate::releases::hex(&random)));
        self.tree(&source, version, identity);
        let path = self
            .root
            .path()
            .join(format!("download-{}", crate::releases::hex(&random)));
        let mut file = File::create(&path).unwrap();
        archive::pack(&source, &mut file).unwrap();
        drop(file);
        fs::remove_dir_all(source).unwrap();
        let mut manifest = self.manifest(version);
        sign_archive(&path, &mut manifest, &self.release).unwrap();
        let wire =
            crate::releases::sign_manifest(&manifest, &self.release, &self.freshness).unwrap();
        VerifiedArchive {
            file: Some(File::open(&path).unwrap()),
            manifest,
            wire,
            cleanup: RemoveFile(path),
            keys: self.keys(),
        }
    }
    fn installer(&self, fake: Fake) -> Installer<Fake> {
        Installer::with_system(&self.app(), self.package, fake)
            .unwrap()
            .with_trust(self.keys(), "a".repeat(40))
            .with_target(self.target.clone())
    }
    fn version(&self, installer: &Installer<Fake>) -> Version {
        installer.installed_version().unwrap()
    }
    fn update_directory(&self) -> PathBuf {
        self.root
            .path()
            .join(format!(".{}{UPDATE_DIRECTORY}", self.layout().application))
    }
    fn stages(&self) -> Vec<String> {
        fs::read_dir(self.update_directory())
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .filter(|name| name.starts_with("stage-"))
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn exchanged(installer: &Installer<Fake>) -> bool {
    installer
        .system
        .events
        .borrow()
        .iter()
        .any(|event| event == "exchange")
}

#[test]
fn an_installation_without_a_record_keeps_the_installed_release_from_the_source() {
    for (package, system) in [
        (Package::Bundle, "macos"),
        (Package::Directory, "linux"),
        (Package::Directory, "windows"),
        (Package::AppImage, "linux"),
    ] {
        let setup = Setup::new(package, system);
        let installer = setup.installer(Fake::default());
        let mut download = setup.release("2.0.0", "certificate");
        let fetched = Fetched::Archive(Box::new(setup.release("1.0.0", "certificate")));
        assert!(!installer.has_record_of(Version::new(1, 0, 0)));
        let installed = installer.install(&mut download, fetched).unwrap();
        assert_eq!(
            installed,
            Installed {
                version: Version::new(2, 0, 0),
                previous: Previous::Kept(Version::new(1, 0, 0)),
            },
            "{system} {package:?}"
        );
        assert_eq!(setup.version(&installer), Version::new(2, 0, 0));
        let record = installer.record().unwrap();
        assert!(record.join("installed/archive.junto").is_file());
        assert!(record.join("previous/archive.junto").is_file());
        let recovery = record.join(if system == "windows" {
            "previous/recover.exe"
        } else {
            "previous/recover"
        });
        assert_eq!(
            fs::read_to_string(recovery).unwrap(),
            "recovery program 1.0.0"
        );
        assert!(installer.has_record_of(Version::new(2, 0, 0)));
        assert!(setup.stages().is_empty());
        installer.rollback().unwrap();
        assert_eq!(setup.version(&installer), Version::new(1, 0, 0));
    }
}

#[test]
fn a_file_added_inside_the_installed_bundle_does_not_stop_an_update() {
    let setup = Setup::macos();
    // The added file breaks the seal of the installed bundle's signature; the update does not
    // ask the installed bundle for its identity.
    let installer = setup.installer(Fake {
        broken_installed: true,
        ..Fake::default()
    });
    fs::write(setup.app().join(".DS_Store"), b"added by the file manager").unwrap();
    let mut download = setup.release("2.0.0", "certificate");
    let fetched = Fetched::Archive(Box::new(setup.release("1.0.0", "certificate")));
    let installed = installer.install(&mut download, fetched).unwrap();
    assert_eq!(installed.previous, Previous::Kept(Version::new(1, 0, 0)));
    fs::write(setup.app().join(".DS_Store"), b"added again").unwrap();
    let mut download = setup.release("3.0.0", "certificate");
    let installed = installer
        .install(&mut download, Fetched::NotNeeded)
        .unwrap();
    assert_eq!(installed.previous, Previous::Kept(Version::new(2, 0, 0)));
    assert_eq!(setup.version(&installer), Version::new(3, 0, 0));
}

#[test]
fn an_update_without_a_release_of_the_installed_version_installs_and_states_why() {
    let setup = Setup::macos();
    let installer = setup.installer(Fake::default());
    let mut download = setup.release("2.0.0", "certificate");
    let installed = installer
        .install(
            &mut download,
            Fetched::Unavailable("invalid release: HTTP status".into()),
        )
        .unwrap();
    assert_eq!(setup.version(&installer), Version::new(2, 0, 0));
    match installed.previous {
        Previous::Missing(reason) => assert!(reason.contains("HTTP status"), "{reason}"),
        other => panic!("{other:?}"),
    }
    let record = installer.record().unwrap();
    assert!(record.join("installed/archive.junto").is_file());
    assert!(!record.join("previous").exists());
    assert!(installer.rollback().is_err());
    assert_eq!(setup.version(&installer), Version::new(2, 0, 0));
}

#[test]
fn a_changed_retained_archive_is_not_a_record_and_is_downloaded_again() {
    let setup = Setup::macos();
    let installer = setup.installer(Fake::default());
    let mut download = setup.release("2.0.0", "certificate");
    installer
        .install(
            &mut download,
            Fetched::Archive(Box::new(setup.release("1.0.0", "certificate"))),
        )
        .unwrap();
    let retained = installer.record().unwrap().join("installed/archive.junto");
    let mut bytes = fs::read(&retained).unwrap();
    let last = bytes.len() - 3;
    bytes[last] ^= 1;
    fs::remove_file(&retained).unwrap();
    fs::write(&retained, bytes).unwrap();
    // A changed archive is not a record: the update downloads the release again.
    assert!(!installer.has_record_of(Version::new(2, 0, 0)));
    let mut download = setup.release("3.0.0", "certificate");
    let installed = installer
        .install(&mut download, Fetched::NotNeeded)
        .unwrap();
    match installed.previous {
        Previous::Missing(reason) => assert!(reason.contains("no release"), "{reason}"),
        other => panic!("{other:?}"),
    }
    assert!(!installer.record().unwrap().join("previous").exists());
    let mut download = setup.release("4.0.0", "certificate");
    let installed = installer
        .install(
            &mut download,
            Fetched::Archive(Box::new(setup.release("3.0.0", "certificate"))),
        )
        .unwrap();
    assert_eq!(installed.previous, Previous::Kept(Version::new(3, 0, 0)));
}

#[test]
fn target_and_version_are_checked_before_extraction_and_system_tools() {
    for case in ["system", "package", "equal", "older"] {
        let setup = Setup::macos();
        let installer = setup.installer(Fake::default());
        let version = match case {
            "equal" => "1.0.0",
            "older" => "0.9.0",
            _ => "2.0.0",
        };
        let mut download = setup.release(version, "certificate");
        match case {
            "system" => download.manifest.system = "linux".into(),
            "package" => download.manifest.package = "directory".into(),
            _ => {}
        }
        // Changed bytes: an extraction before these checks would fail on the archive hash.
        fs::write(download.path(), b"changed bytes").unwrap();
        let result = installer.install(&mut download, Fetched::NotNeeded);
        assert!(
            matches!(
                (case, &result),
                ("system" | "package", Err(Error::Invalid("release target")))
                    | ("equal" | "older", Err(Error::NotNewer))
            ),
            "{case}: {result:?}"
        );
        assert!(installer.system.events.borrow().is_empty(), "{case}");
        assert!(setup.stages().is_empty(), "{case}");
        assert!(!installer.record().unwrap().exists(), "{case}");
    }
}

#[test]
fn verification_precedes_platform_tools_and_exchange() {
    let setup = Setup::macos();
    let installer = setup.installer(Fake::default());
    let mut download = setup.release("2.0.0", "certificate");
    download.manifest.archive_signatures.clear();
    assert!(
        installer
            .install(&mut download, Fetched::NotNeeded)
            .is_err()
    );
    assert!(!exchanged(&installer));
    assert_eq!(setup.version(&installer), Version::new(1, 0, 0));
}

#[test]
fn every_candidate_identity_and_version_check_prevents_exchange() {
    for case in [
        "adhoc", "other", "mismatch", "invalid", "version", "mode", "extra",
    ] {
        if case == "mode" && !cfg!(unix) {
            continue;
        }
        let setup = Setup::macos();
        let identity = match case {
            "adhoc" => "adhoc",
            "other" => "other",
            _ => "certificate",
        };
        let mut download = if case == "extra" {
            // A second top-level entry beside the bundle.
            let source = setup.root.path().join("extra-source");
            setup.tree(&source, "2.0.0", identity);
            fs::write(source.join("extra"), b"extra").unwrap();
            let path = setup.root.path().join("extra-download");
            let mut file = File::create(&path).unwrap();
            archive::pack(&source, &mut file).unwrap();
            let mut manifest = setup.manifest("2.0.0");
            sign_archive(&path, &mut manifest, &setup.release).unwrap();
            VerifiedArchive {
                file: Some(File::open(&path).unwrap()),
                wire: crate::releases::sign_manifest(&manifest, &setup.release, &setup.freshness)
                    .unwrap(),
                manifest,
                cleanup: RemoveFile(path),
                keys: setup.keys(),
            }
        } else {
            setup.release("2.0.0", identity)
        };
        if case == "version" {
            download.manifest.version = "3.0.0".into();
        }
        let installer = setup.installer(Fake {
            invalid_signature: case == "invalid",
            wrong_requirement: case == "mismatch",
            bad_mode: case == "mode",
            ..Fake::default()
        });
        assert!(
            installer
                .install(&mut download, Fetched::NotNeeded)
                .is_err(),
            "{case}"
        );
        assert!(!exchanged(&installer), "{case}");
        assert_eq!(setup.version(&installer), Version::new(1, 0, 0));
        assert!(setup.stages().is_empty(), "{case}");
    }
}

#[test]
fn a_description_of_another_target_or_version_prevents_exchange() {
    for case in ["system", "version"] {
        let setup = Setup::new(Package::Directory, "linux");
        let installer = setup.installer(Fake::default());
        let mut download = setup.release("2.0.0", "certificate");
        // The candidate's description must name the target and the offered version.
        let other = Setup {
            target: Target::new(
                if case == "system" { "windows" } else { "linux" },
                "directory",
                std::env::consts::ARCH,
            )
            .unwrap(),
            ..Setup::new(Package::Directory, "linux")
        };
        let source = setup.root.path().join("other-source");
        setup.tree(&source, "2.0.0", "certificate");
        fs::write(
            source.join("junto/junto-package.json"),
            other.description(if case == "version" { "2.0.1" } else { "2.0.0" }),
        )
        .unwrap();
        let path = setup.root.path().join("other-download");
        let mut file = File::create(&path).unwrap();
        archive::pack(&source, &mut file).unwrap();
        let mut manifest = setup.manifest("2.0.0");
        sign_archive(&path, &mut manifest, &setup.release).unwrap();
        download.file = Some(File::open(&path).unwrap());
        download.manifest = manifest;
        download.cleanup = RemoveFile(path);
        assert!(
            installer
                .install(&mut download, Fetched::NotNeeded)
                .is_err(),
            "{case}"
        );
        assert!(!exchanged(&installer), "{case}");
        assert_eq!(setup.version(&installer), Version::new(1, 0, 0));
    }
}

#[test]
fn failures_restore_the_record_and_the_application() {
    for failure in ["move", "exchange", "none"] {
        let setup = Setup::macos();
        let first = setup.installer(Fake::default());
        let mut download = setup.release("2.0.0", "certificate");
        first
            .install(
                &mut download,
                Fetched::Archive(Box::new(setup.release("1.0.0", "certificate"))),
            )
            .unwrap();
        let record = first.record().unwrap();
        let before = fs::read(record.join("installed/release.json")).unwrap();
        let installer = setup.installer(Fake {
            fail_move: failure == "move",
            fail_exchange: failure == "exchange",
            ..Fake::default()
        });
        let mut download = setup.release("3.0.0", "certificate");
        let result = installer.install(&mut download, Fetched::NotNeeded);
        if failure == "none" {
            assert_eq!(
                result.unwrap().previous,
                Previous::Kept(Version::new(2, 0, 0))
            );
            assert_eq!(setup.version(&installer), Version::new(3, 0, 0));
            assert_ne!(
                fs::read(record.join("installed/release.json")).unwrap(),
                before
            );
        } else {
            assert!(result.is_err(), "{failure}");
            assert_eq!(setup.version(&installer), Version::new(2, 0, 0));
            // The new record is moved in with the exchange only.
            assert_eq!(
                fs::read(record.join("installed/release.json")).unwrap(),
                before
            );
            assert!(record.join("previous/archive.junto").is_file());
        }
        assert!(setup.stages().is_empty(), "{failure}");
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
        let setup = Setup::macos();
        let mut installer = setup.installer(Fake::default());
        let mut download = setup.release("2.0.0", "certificate");
        installer
            .install(
                &mut download,
                Fetched::Archive(Box::new(setup.release("1.0.0", "certificate"))),
            )
            .unwrap();
        let record = installer.record().unwrap();
        fs::write(setup.app().join("identity"), "damaged current").unwrap();
        match fault {
            "archive" => {
                let path = record.join("previous/archive.junto");
                let mut bytes = fs::read(&path).unwrap();
                let offset = bytes
                    .windows(b"recovery program".len())
                    .position(|part| part == b"recovery program")
                    .unwrap();
                bytes[offset..offset + b"modified program".len()]
                    .copy_from_slice(b"modified program");
                fs::remove_file(&path).unwrap();
                fs::write(path, bytes).unwrap();
            }
            "proof" => tamper_proof(&record.join("previous/release.json")),
            "installed" => tamper_proof(&record.join("installed/release.json")),
            "not-older" => {
                fs::copy(
                    record.join("previous/release.json"),
                    record.join("installed/release.json"),
                )
                .unwrap();
                fs::write(setup.app().join("version"), "1.0.0").unwrap();
            }
            "pin" => installer.pin = "b".repeat(40),
            "platform" => installer.system.invalid_signature = true,
            _ => {}
        }
        installer.system.events.borrow_mut().clear();
        let result = installer.rollback();
        if fault == "none" {
            assert_eq!(result.unwrap(), Version::new(1, 0, 0));
            assert_eq!(setup.version(&installer), Version::new(1, 0, 0));
            assert!(installer.rollback().is_err());
        } else {
            assert!(result.is_err(), "{fault}");
            assert!(!exchanged(&installer), "{fault}");
        }
        assert!(setup.stages().is_empty(), "{fault}");
    }
}

#[test]
fn rollback_of_a_package_whose_version_cannot_be_read_still_requires_an_authentic_release() {
    let setup = Setup::new(Package::Directory, "linux");
    let installer = setup.installer(Fake::default());
    let mut download = setup.release("2.0.0", "certificate");
    installer
        .install(
            &mut download,
            Fetched::Archive(Box::new(setup.release("1.0.0", "certificate"))),
        )
        .unwrap();
    fs::remove_file(setup.app().join(DESCRIPTION)).unwrap();
    assert_eq!(installer.rollback().unwrap(), Version::new(1, 0, 0));
    assert_eq!(
        fs::read_to_string(setup.app().join("program")).unwrap(),
        "program 1.0.0"
    );
}

#[test]
fn concurrent_install_is_refused_before_tools() {
    let setup = Setup::macos();
    let installer = setup.installer(Fake::default());
    installer.prepare().unwrap();
    let _lock = installer.lock().unwrap();
    let second = setup.installer(Fake::default());
    assert!(matches!(second.lock(), Err(Error::Locked)));
    let mut download = setup.release("2.0.0", "certificate");
    assert!(matches!(
        second.install(&mut download, Fetched::NotNeeded),
        Err(Error::Locked)
    ));
    assert!(matches!(second.rollback(), Err(Error::Locked)));
    assert!(second.system.events.borrow().is_empty());
}

#[test]
fn retained_previous_release_is_not_an_application_bundle() {
    let setup = Setup::macos();
    let installer = setup.installer(Fake::default());
    let mut download = setup.release("2.0.0", "certificate");
    installer
        .install(
            &mut download,
            Fetched::Archive(Box::new(setup.release("1.0.0", "certificate"))),
        )
        .unwrap();
    let mut found = Vec::new();
    let mut directories = vec![setup.update_directory()];
    while let Some(directory) = directories.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                found.push(path.clone());
                directories.push(path);
            }
        }
    }
    assert!(
        !found
            .iter()
            .any(|path| path.extension().is_some_and(|e| e == "app"))
    );
    assert!(
        installer
            .record()
            .unwrap()
            .join("previous/archive.junto")
            .is_file()
    );
}

#[test]
fn a_finished_staging_directory_is_removed_and_an_unfinished_one_is_kept() {
    let setup = Setup::macos();
    let installer = setup.installer(Fake::default());
    let directory = installer.prepare().unwrap();
    fs::create_dir_all(directory.join("stage-finished/unpacked")).unwrap();
    fs::write(directory.join("finished-stage-finished"), b"").unwrap();
    fs::create_dir_all(directory.join("stage-unfinished/record")).unwrap();
    let mut download = setup.release("2.0.0", "certificate");
    installer
        .install(&mut download, Fetched::NotNeeded)
        .unwrap();
    assert!(!directory.join("stage-finished").exists());
    assert!(!directory.join("finished-stage-finished").exists());
    assert!(directory.join("stage-unfinished/record").is_dir());
}

#[test]
fn translocated_and_readonly_parents_cannot_update() {
    let root = tempfile::tempdir().unwrap();
    let app = root.path().join("AppTranslocation/random/Junto.app");
    fs::create_dir_all(&app).unwrap();
    assert!(matches!(install_location(&app), Err(Error::NotWritable)));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let parent = root.path().join("readonly");
        let app = parent.join("Junto.app");
        fs::create_dir_all(&app).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o555)).unwrap();
        let result = install_location(&app);
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(result, Err(Error::NotWritable)));
    }
}

#[test]
fn two_adhoc_bundles_do_not_establish_a_certificate_identity() {
    let setup = Setup::macos();
    setup.place("1.0.0", "adhoc");
    let installer = setup.installer(Fake::default());
    let mut download = setup.release("2.0.0", "adhoc");
    assert!(
        installer
            .install(&mut download, Fetched::NotNeeded)
            .is_err()
    );
    assert!(!exchanged(&installer));
}

#[test]
fn location_check_does_not_write_to_the_application_directory() {
    let setup = Setup::macos();
    let before = fs::metadata(setup.root.path()).unwrap().modified().unwrap();
    install_location(&setup.app()).unwrap();
    assert_eq!(
        before,
        fs::metadata(setup.root.path()).unwrap().modified().unwrap()
    );
    assert!(
        application_for_executable(
            &setup
                .root
                .path()
                .join("Junto.previous.app/Contents/MacOS/junto")
        )
        .is_err()
    );
}

#[test]
fn the_package_of_a_named_path_is_found_by_its_shape() {
    let bundle = Setup::macos();
    assert_eq!(package_at(&bundle.app()).unwrap(), Package::Bundle);
    let directory = Setup::new(Package::Directory, "linux");
    assert_eq!(package_at(&directory.app()).unwrap(), Package::Directory);
    let appimage = Setup::new(Package::AppImage, "linux");
    assert_eq!(package_at(&appimage.app()).unwrap(), Package::AppImage);
    let other = appimage.root.path().join("other");
    fs::write(&other, b"\x7fELF\x02\x01\x01\x00\x00\x00\x00").unwrap();
    assert!(matches!(package_at(&other), Err(Error::NotInstalled)));
    assert!(matches!(
        package_at(appimage.root.path()),
        Err(Error::NotInstalled)
    ));
}

#[test]
fn the_source_of_a_release_holds_exactly_the_entries_of_its_package() {
    let setup = Setup::new(Package::AppImage, "linux");
    let source = setup.root.path().join("release-source");
    setup.tree(&source, "2.0.0", "certificate");
    verify_source(&setup.target, &source, Version::new(2, 0, 0)).unwrap();
    assert!(verify_source(&setup.target, &source, Version::new(2, 0, 1)).is_err());
    fs::write(source.join("extra"), b"").unwrap();
    assert!(verify_source(&setup.target, &source, Version::new(2, 0, 0)).is_err());
}

#[test]
fn entries_are_replaced_one_by_one_and_a_failure_moves_them_back() {
    let root = tempfile::tempdir().unwrap();
    let application = root.path().join("Junto");
    let candidate = root.path().join("stage/Junto");
    fs::create_dir_all(application.join("licences")).unwrap();
    fs::write(application.join("junto.exe"), b"old program").unwrap();
    fs::write(application.join("licences/LICENSE"), b"old licence").unwrap();
    fs::create_dir_all(candidate.join("licences")).unwrap();
    fs::write(candidate.join("junto.exe"), b"new program").unwrap();
    fs::write(candidate.join("junto-package.json"), b"new").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // A candidate whose entries cannot be moved: the old entries go back.
        fs::set_permissions(
            root.path().join("stage/Junto"),
            fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        let result = crate::releases::windows::exchange_entries(&application, &candidate);
        fs::set_permissions(
            root.path().join("stage/Junto"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert!(matches!(result, Err(Error::Io(_))), "{result:?}");
        assert_eq!(
            fs::read(application.join("junto.exe")).unwrap(),
            b"old program"
        );
        assert!(application.join("licences/LICENSE").is_file());
        fs::remove_dir(root.path().join("stage/Junto.replaced")).unwrap();
    }
    crate::releases::windows::exchange_entries(&application, &candidate).unwrap();
    assert_eq!(
        fs::read(application.join("junto.exe")).unwrap(),
        b"new program"
    );
    assert!(application.join("junto-package.json").is_file());
    assert!(!application.join("licences/LICENSE").exists());
    assert_eq!(
        fs::read(root.path().join("stage/Junto.replaced/junto.exe")).unwrap(),
        b"old program"
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

#[test]
fn an_exchange_that_cannot_be_undone_keeps_the_staging_directory() {
    let setup = Setup::macos();
    let installer = setup.installer(Fake {
        incomplete_exchange: true,
        ..Fake::default()
    });
    let mut download = setup.release("2.0.0", "certificate");
    assert!(matches!(
        installer.install(&mut download, Fetched::NotNeeded),
        Err(Error::ExchangeIncomplete)
    ));
    assert_eq!(setup.stages().len(), 1);
    let setup = Setup::macos();
    let mut installer = setup.installer(Fake::default());
    let mut download = setup.release("2.0.0", "certificate");
    installer
        .install(
            &mut download,
            Fetched::Archive(Box::new(setup.release("1.0.0", "certificate"))),
        )
        .unwrap();
    installer.system.incomplete_exchange = true;
    assert!(matches!(
        installer.rollback(),
        Err(Error::ExchangeIncomplete)
    ));
    assert_eq!(setup.stages().len(), 1);
    // The next operation keeps a staging directory without a mark.
    installer.system.incomplete_exchange = false;
    installer.rollback().unwrap();
    assert_eq!(setup.stages().len(), 1);
}

#[test]
fn after_a_rollback_the_record_describes_the_restored_release() {
    let setup = Setup::macos();
    let installer = setup.installer(Fake::default());
    let mut download = setup.release("2.0.0", "certificate");
    installer
        .install(
            &mut download,
            Fetched::Archive(Box::new(setup.release("1.0.0", "certificate"))),
        )
        .unwrap();
    installer.rollback().unwrap();
    let record = installer.record().unwrap();
    assert!(!record.join("previous").exists());
    assert!(installer.has_record_of(Version::new(1, 0, 0)));
    assert!(!installer.has_record_of(Version::new(2, 0, 0)));
    assert!(installer.rollback().is_err());
    let mut download = setup.release("3.0.0", "certificate");
    let installed = installer
        .install(&mut download, Fetched::NotNeeded)
        .unwrap();
    assert_eq!(installed.previous, Previous::Kept(Version::new(1, 0, 0)));
}

#[test]
fn two_installations_in_one_directory_have_records_of_their_own() {
    let setup = Setup::new(Package::AppImage, "linux");
    let other = setup.root.path().join("Other.AppImage");
    fs::copy(setup.app(), &other).unwrap();
    let first = setup.installer(Fake::default());
    let second = Installer::with_system(&other, Package::AppImage, Fake::default())
        .unwrap()
        .with_trust(setup.keys(), "a".repeat(40))
        .with_target(setup.target.clone());
    assert_ne!(first.record().unwrap(), second.record().unwrap());
    let mut download = setup.release("2.0.0", "certificate");
    first
        .install(
            &mut download,
            Fetched::Archive(Box::new(setup.release("1.0.0", "certificate"))),
        )
        .unwrap();
    assert!(second.rollback().is_err());
    assert_eq!(second.installed_version().unwrap(), Version::new(1, 0, 0));
}

#[test]
fn a_candidate_that_cannot_be_read_leaves_every_entry_in_place() {
    let root = tempfile::tempdir().unwrap();
    let application = root.path().join("Junto");
    fs::create_dir_all(&application).unwrap();
    fs::write(application.join("junto.exe"), b"old program").unwrap();
    let result =
        crate::releases::windows::exchange_entries(&application, &root.path().join("missing"));
    assert!(matches!(result, Err(Error::Io(_))), "{result:?}");
    assert_eq!(
        fs::read(application.join("junto.exe")).unwrap(),
        b"old program"
    );
    assert!(!root.path().join("missing.replaced").exists());
}

#[test]
fn the_previous_archive_is_a_second_name_on_unix_and_a_copy_on_windows() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("installed.junto");
    let destination = directory.path().join("previous.junto");
    fs::write(&source, b"first").unwrap();
    keep_archive(&source, &destination).unwrap();
    fs::write(&source, b"other").unwrap();
    let expected: &[u8] = if cfg!(windows) { b"first" } else { b"other" };
    assert_eq!(fs::read(&destination).unwrap(), expected);
}
