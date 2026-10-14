#![allow(clippy::unwrap_used)]

use junto_client_core::releases::{TrustedKeys, verify_archive, verify_manifest};
use junto_client_core::updates::Version;
use std::process::Command;

#[test]
fn release_tool_generates_external_keys_and_publishes_verifiable_files() {
    let root = tempfile::Builder::new()
        .prefix("junto-release-")
        .tempdir()
        .unwrap();
    let secret = root.path().join("secret");
    let public = root.path().join("public.json");
    let tool = env!("CARGO_BIN_EXE_junto-release");
    assert!(
        Command::new(tool)
            .arg("keygen")
            .arg(&secret)
            .arg(&public)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        !Command::new(tool)
            .arg("keygen")
            .arg(&secret)
            .arg(&public)
            .status()
            .unwrap()
            .success()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&secret).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("payload"), b"release content").unwrap();
    let output = root.path().join("output");
    assert!(
        Command::new(tool)
            .arg("package")
            .arg(&source)
            .arg(&output)
            .arg(&secret)
            .arg(&secret)
            .args(["0.2.0", "macos", "aarch64"])
            .status()
            .unwrap()
            .success()
    );
    let keys = TrustedKeys::from_json(&std::fs::read(&public).unwrap())
        .unwrap()
        .with_freshness(TrustedKeys::from_json(&std::fs::read(&public).unwrap()).unwrap());
    let wire = std::fs::read(output.join("latest-macos-aarch64.json")).unwrap();
    let manifest =
        verify_manifest(&wire, &keys, Version::new(0, 1, 0), "macos", "aarch64").unwrap();
    verify_archive(&output.join(manifest.archive.clone()), &manifest, &keys).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        manifest.expires > now + 29 * 24 * 60 * 60 && manifest.expires <= now + 30 * 24 * 60 * 60
    );
    if option_env!("JUNTO_UPDATE_SOURCE").is_none() {
        let result = Command::new(tool)
            .arg("publish")
            .arg("missing.app")
            .arg(&output)
            .arg(&secret)
            .arg(&secret)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(
            String::from_utf8_lossy(&result.stderr).contains("release and freshness trust sets")
        );
    }
}
