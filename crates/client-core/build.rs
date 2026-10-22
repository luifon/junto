#![allow(clippy::unwrap_used, clippy::expect_used)]
#[allow(dead_code)]
#[path = "src/release_configuration.rs"]
mod configuration;
use std::{env, fs, path::PathBuf};

fn main() {
    for name in [
        "JUNTO_UPDATE_SOURCE",
        "JUNTO_TEST_UPDATE_KEYS_FILE",
        "JUNTO_TEST_FRESHNESS_KEYS_FILE",
        "JUNTO_TEST_CERTIFICATE_FILE",
        "JUNTO_TEST_BUILD_VERSION",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let mut sets = Vec::new();
    for (variable, tracked, output) in [
        (
            "JUNTO_TEST_UPDATE_KEYS_FILE",
            "../../packaging/update-keys.json",
            "update-keys.json",
        ),
        (
            "JUNTO_TEST_FRESHNESS_KEYS_FILE",
            "../../packaging/freshness-keys.json",
            "freshness-keys.json",
        ),
        (
            "JUNTO_TEST_CERTIFICATE_FILE",
            "../../packaging/certificate.sha1",
            "certificate.sha1",
        ),
    ] {
        let path = env::var(variable).unwrap_or_else(|_| tracked.into());
        println!("cargo:rerun-if-changed={path}");
        let bytes = fs::read(&path).unwrap_or_else(|error| panic!("cannot read {path}: {error}"));
        if output.ends_with("keys.json") {
            configuration::keys(&bytes).unwrap_or_else(|error| panic!("{error}"));
            sets.push(bytes.clone());
        } else {
            configuration::certificate(&bytes).unwrap_or_else(|error| panic!("{error}"));
        }
        fs::write(out.join(output), bytes).unwrap();
    }
    configuration::distinct_roles(&sets[0], &sets[1]).unwrap_or_else(|error| panic!("{error}"));
    if let Ok(source) = env::var("JUNTO_UPDATE_SOURCE")
        && !source.is_empty()
    {
        configuration::source(&source).unwrap_or_else(|error| panic!("{error}"));
    }
    let test_configuration = [
        "JUNTO_TEST_UPDATE_KEYS_FILE",
        "JUNTO_TEST_FRESHNESS_KEYS_FILE",
        "JUNTO_TEST_CERTIFICATE_FILE",
        "JUNTO_TEST_BUILD_VERSION",
    ]
    .iter()
    .any(|name| env::var_os(name).is_some());
    println!(
        "cargo:rustc-env=JUNTO_TEST_CONFIGURATION={}",
        u8::from(test_configuration)
    );

    let version = env::var("JUNTO_TEST_BUILD_VERSION")
        .unwrap_or_else(|_| env::var("CARGO_PKG_VERSION").unwrap());
    let parts: Vec<_> = version.split('.').collect();
    assert!(
        parts.len() == 3
            && parts.iter().all(|part| !part.is_empty()
                && (*part == "0" || !part.starts_with('0'))
                && part.bytes().all(|c| c.is_ascii_digit())
                && part.parse::<u32>().is_ok()),
        "invalid build version"
    );
    for (name, value) in ["MAJOR", "MINOR", "PATCH"].into_iter().zip(parts) {
        println!("cargo:rustc-env=JUNTO_BUILD_{name}={value}");
    }

    // The recovery program of the Windows package carries the icon of the client, which the MSVC
    // linker reads from a compiled resource file (ADR 0756).
    if env::var("CARGO_CFG_TARGET_OS").unwrap() == "windows" {
        assert!(
            env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc"),
            "the Windows build uses the MSVC tools (ADR 0753), whose linker reads the icon"
        );
        let resource = out.join("junto-recover.res");
        fs::write(&resource, junto_icon::windows_resource().unwrap()).unwrap();
        println!(
            "cargo:rustc-link-arg-bin=junto-recover={}",
            resource.display()
        );
    }
}
