//! Updates and rollbacks of the Linux packages with the system tools of Linux, a local source
//! and real files (ADR 0754). The AppImage states its version only to the program that it
//! runs, so the test names that version as the running program would.
#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ed25519_dalek::SigningKey;
use junto_client_core::releases::install::{Installer, Package, Previous};
use junto_client_core::releases::linux::{LinuxTools, exchange_by_renames};
use junto_client_core::releases::{
    self, HttpSource, Manifest, Target, TrustedKeys, sign_archive, sign_manifest,
};
use junto_client_core::updates::Version;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const MARK: &[u8] = b"\x7fELF\x02\x01\x01\x00AI\x02";

fn key() -> SigningKey {
    let mut seed = [0; 32];
    getrandom::fill(&mut seed).unwrap();
    SigningKey::from_bytes(&seed)
}

fn soon() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 24 * 60 * 60
}

fn executable(path: &Path, content: &[u8]) {
    use std::os::unix::fs::PermissionsExt;
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

struct Release {
    root: tempfile::TempDir,
    target: Target,
    release: SigningKey,
    freshness: SigningKey,
    files: HashMap<String, Vec<u8>>,
}

impl Release {
    fn new(package: Package) -> Self {
        Self {
            root: tempfile::tempdir().unwrap(),
            target: Target::this_build(package),
            release: key(),
            freshness: key(),
            files: HashMap::new(),
        }
    }
    fn keys(&self) -> TrustedKeys {
        TrustedKeys::new(vec![self.release.verifying_key()])
            .with_freshness(TrustedKeys::new(vec![self.freshness.verifying_key()]))
    }
    fn description(&self, version: &str) -> String {
        format!(
            "{{\"format\":1,\"version\":\"{version}\",\"system\":\"linux\",\"package\":\"{}\",\"architecture\":\"{}\"}}",
            self.target.package, self.target.architecture
        )
    }
    /// The files of one version: the program states its version in its content.
    fn tree(&self, directory: &Path, version: &str) {
        match self.target.package.as_str() {
            "appimage" => {
                executable(
                    &directory.join("Junto.AppImage"),
                    &[MARK, version.as_bytes()].concat(),
                );
                executable(
                    &directory.join("junto-recover"),
                    format!("recovery {version}").as_bytes(),
                );
                fs::write(
                    directory.join("junto-package.json"),
                    self.description(version),
                )
                .unwrap();
            }
            _ => {
                executable(
                    &directory.join("junto/junto"),
                    format!("program {version}").as_bytes(),
                );
                executable(
                    &directory.join("junto/junto-recover"),
                    format!("recovery {version}").as_bytes(),
                );
                fs::write(
                    directory.join("junto/junto-package.json"),
                    self.description(version),
                )
                .unwrap();
                fs::create_dir_all(directory.join("junto/lib")).unwrap();
                fs::write(directory.join("junto/lib/libxcb.so.1"), version).unwrap();
            }
        }
    }
    /// Packs a version and serves it: as the newest release, or as the release of its version.
    fn publish(&mut self, version: &str, newest: bool) {
        let source = self.root.path().join(format!("source-{version}"));
        self.tree(&source, version);
        let parsed = Version::parse(version).unwrap();
        let archive_name = self.target.archive(parsed);
        let archive = self.root.path().join(&archive_name);
        let mut file = fs::File::create(&archive).unwrap();
        releases::archive::pack(&source, &mut file).unwrap();
        let mut manifest = Manifest {
            format: 1,
            version: version.into(),
            system: self.target.system.clone(),
            package: self.target.package.clone(),
            architecture: self.target.architecture.clone(),
            archive: archive_name.clone(),
            size: 0,
            sha256: String::new(),
            expires: soon(),
            archive_signatures: vec![],
        };
        sign_archive(&archive, &mut manifest, &self.release).unwrap();
        let wire = sign_manifest(&manifest, &self.release, &self.freshness).unwrap();
        self.files.insert(archive_name, fs::read(&archive).unwrap());
        self.files.insert(self.target.release(parsed), wire.clone());
        if newest {
            self.files.insert(self.target.latest(), wire);
        }
    }
    /// The installation of `version` as a person places it from the first-install package.
    fn install(&self, version: &str) -> PathBuf {
        let place = self.root.path().join(format!("first-{version}"));
        self.tree(&place, version);
        let installed = self.root.path().join("installed");
        fs::create_dir_all(&installed).unwrap();
        let (from, to) = match self.target.package.as_str() {
            "appimage" => ("Junto.AppImage", "Junto-1.0.0-x86_64.AppImage"),
            _ => ("junto", "junto"),
        };
        fs::rename(place.join(from), installed.join(to)).unwrap();
        installed.join(to)
    }
}

/// Serves the files of a release by their names and records the names asked for.
async fn serve(files: HashMap<String, Vec<u8>>) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}/", listener.local_addr().unwrap());
    let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
    let record = asked.clone();
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let mut request = [0; 4096];
            let count = stream.read(&mut request).await.unwrap_or(0);
            let text = String::from_utf8_lossy(&request[..count]).into_owned();
            let path = text
                .split(' ')
                .nth(1)
                .unwrap_or("/")
                .trim_start_matches('/')
                .to_owned();
            record.lock().unwrap().push(path.clone());
            let response = match files.get(&path) {
                Some(body) => [
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .into_bytes(),
                    body.clone(),
                ]
                .concat(),
                None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec(),
            };
            let _ = stream.write_all(&response).await;
        }
    });
    (address, asked)
}

#[tokio::test]
async fn an_appimage_is_replaced_while_its_file_is_open_and_rolled_back() {
    let mut release = Release::new(Package::AppImage);
    release.publish("1.0.0", false);
    release.publish("2.0.0", true);
    let app = release.install("1.0.0");
    // A running AppImage holds its file open; the replacement does not change what it reads.
    let mut running = fs::File::open(&app).unwrap();
    let (address, asked) = serve(release.files.clone()).await;
    let installer = Installer::with_system(
        &app,
        Package::AppImage,
        LinuxTools::with_running(&app, Version::new(1, 0, 0)).unwrap(),
    )
    .unwrap()
    .with_trust(release.keys(), String::new());
    let source = HttpSource::new(
        &address,
        release.keys(),
        Version::new(1, 0, 0),
        release.target.clone(),
    )
    .unwrap();
    let installed = releases::update(installer, &source).await.unwrap();
    assert_eq!(installed.version, Version::new(2, 0, 0));
    assert_eq!(installed.previous, Previous::Kept(Version::new(1, 0, 0)));
    assert_eq!(fs::read(&app).unwrap(), [MARK, b"2.0.0"].concat());
    let mut old = Vec::new();
    running.read_to_end(&mut old).unwrap();
    assert_eq!(old, [MARK, b"1.0.0"].concat());
    let arch = &release.target.architecture;
    assert_eq!(
        *asked.lock().unwrap(),
        [
            format!("latest-linux-appimage-{arch}.json"),
            format!("junto-2.0.0-linux-appimage-{arch}.junto"),
            format!("release-1.0.0-linux-appimage-{arch}.json"),
            format!("junto-1.0.0-linux-appimage-{arch}.junto"),
        ]
    );
    let update = app
        .parent()
        .unwrap()
        .join(".Junto-1.0.0-x86_64.AppImage.junto-update");
    assert_eq!(
        fs::read_to_string(update.join("record/previous/recover")).unwrap(),
        "recovery 1.0.0"
    );
    // The recovery program knows no running version: the record decides (ADR 0726).
    let recovery = Installer::with_system(&app, Package::AppImage, LinuxTools::default())
        .unwrap()
        .with_trust(release.keys(), String::new());
    assert_eq!(recovery.rollback().unwrap(), Version::new(1, 0, 0));
    assert_eq!(fs::read(&app).unwrap(), [MARK, b"1.0.0"].concat());
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        fs::metadata(&app).unwrap().permissions().mode() & 0o777,
        0o755
    );
    assert!(!fs::read_dir(&update).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("stage-")
    }));
}

#[tokio::test]
async fn a_directory_package_is_exchanged_in_one_step_and_rolled_back() {
    let mut release = Release::new(Package::Directory);
    release.publish("1.0.0", false);
    release.publish("2.0.0", true);
    let app = release.install("1.0.0");
    let (address, _asked) = serve(release.files.clone()).await;
    let installer = Installer::with_system(&app, Package::Directory, LinuxTools::default())
        .unwrap()
        .with_trust(release.keys(), String::new());
    let source = HttpSource::new(
        &address,
        release.keys(),
        Version::new(1, 0, 0),
        release.target.clone(),
    )
    .unwrap();
    let installed = releases::update(installer, &source).await.unwrap();
    assert_eq!(installed.previous, Previous::Kept(Version::new(1, 0, 0)));
    assert_eq!(
        fs::read_to_string(app.join("junto")).unwrap(),
        "program 2.0.0"
    );
    assert_eq!(
        fs::read_to_string(app.join("lib/libxcb.so.1")).unwrap(),
        "2.0.0"
    );
    let recovery = Installer::with_system(&app, Package::Directory, LinuxTools::default())
        .unwrap()
        .with_trust(release.keys(), String::new());
    assert_eq!(recovery.rollback().unwrap(), Version::new(1, 0, 0));
    assert_eq!(
        fs::read_to_string(app.join("junto")).unwrap(),
        "program 1.0.0"
    );
    assert!(recovery.rollback().is_err());
}

#[test]
fn the_exchange_by_two_renames_puts_the_candidate_in_place_or_restores_the_application() {
    let root = tempfile::tempdir().unwrap();
    let application = root.path().join("junto");
    let candidate = root.path().join("stage/junto");
    fs::create_dir_all(&application).unwrap();
    fs::write(application.join("junto"), "old").unwrap();
    fs::create_dir_all(&candidate).unwrap();
    fs::write(candidate.join("junto"), "new").unwrap();
    exchange_by_renames(&application, &candidate).unwrap();
    assert_eq!(
        fs::read_to_string(application.join("junto")).unwrap(),
        "new"
    );
    assert_eq!(
        fs::read_to_string(root.path().join("stage/junto.replaced/junto")).unwrap(),
        "old"
    );
    // A candidate that is missing: the application goes back to its place.
    assert!(exchange_by_renames(&application, &root.path().join("stage/missing")).is_err());
    assert_eq!(
        fs::read_to_string(application.join("junto")).unwrap(),
        "new"
    );
}
