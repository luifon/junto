#![allow(clippy::unwrap_used)]

use junto_client_core::releases::{TrustedKeys, verify_archive, verify_manifest};
use junto_client_core::updates::Version;
use std::process::Command;

fn target(system: &str, architecture: &str) -> junto_client_core::releases::Target {
    junto_client_core::releases::Target::new(system, "bundle", architecture).unwrap()
}

#[test]
fn release_tool_generates_external_keys_and_publishes_verifiable_files() {
    let root = tempfile::Builder::new()
        .prefix("junto-release-")
        .tempdir()
        .unwrap();
    let secret = root.path().join("secret");
    let public = root.path().join("public.json");
    let fresh_secret = root.path().join("fresh-secret");
    let fresh_public = root.path().join("fresh-public.json");
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
    assert!(
        Command::new(tool)
            .arg("keygen")
            .arg(&fresh_secret)
            .arg(&fresh_public)
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
            .arg(&fresh_secret)
            .args(["0.2.0", "macos", "bundle", "aarch64"])
            .status()
            .unwrap()
            .success()
    );
    let keys = TrustedKeys::from_json(&std::fs::read(&public).unwrap())
        .unwrap()
        .with_freshness(TrustedKeys::from_json(&std::fs::read(&fresh_public).unwrap()).unwrap());
    let wire = std::fs::read(output.join("latest-macos-bundle-aarch64.json")).unwrap();
    // The source keeps the information of each version beside the newest.
    assert_eq!(
        std::fs::read(output.join("release-0.2.0-macos-bundle-aarch64.json")).unwrap(),
        wire
    );
    let manifest = verify_manifest(
        &wire,
        &keys,
        Version::new(0, 1, 0),
        &target("macos", "aarch64"),
    )
    .unwrap();
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
            .args(["macos", "bundle", "aarch64"])
            .arg("missing")
            .arg(&output)
            .arg(&secret)
            .arg(&secret)
            .output()
            .unwrap();
        // A build without a source refuses to publish: with the tracked keys for the missing
        // source, with empty trust files (a test configuration) for the missing keys.
        assert!(!result.status.success());
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(
            stderr.contains("no build-time update source")
                || stderr.contains("release and freshness trust sets"),
            "{stderr}"
        );
    }
}

#[test]
fn a_product_build_refuses_test_trust_and_refreshes_with_its_compiled_keys_only() {
    if option_env!("JUNTO_TEST_UPDATE_KEYS_FILE").is_some() {
        return;
    }
    let tool = env!("CARGO_BIN_EXE_junto-release");
    let root = tempfile::tempdir().unwrap();
    let result = Command::new(tool)
        .args(["--test-configuration", "keygen"])
        .arg(root.path().join("secret"))
        .arg(root.path().join("public.json"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr)
            .contains("--test-configuration is refused by a product build")
    );
    assert!(!root.path().join("secret").exists());
    // The compiled sets of this build are empty: no release information verifies.
    std::fs::write(root.path().join("metadata.json"), b"{}").unwrap();
    let result = Command::new(tool)
        .arg("refresh")
        .arg(root.path().join("metadata.json"))
        .arg(root.path().join("archive.junto"))
        .arg("2.0.0")
        .arg(root.path().join("freshness.seed"))
        .output()
        .unwrap();
    assert!(!result.status.success());
    assert_eq!(
        std::fs::read(root.path().join("metadata.json")).unwrap(),
        b"{}"
    );
}
