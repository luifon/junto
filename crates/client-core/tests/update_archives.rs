#![allow(clippy::unwrap_used)]

use junto_client_core::releases::archive::{pack, unpack};
use std::io::Write;

#[test]
fn archive_round_trip_preserves_files_and_refuses_existing_destinations() {
    let root = tempfile::Builder::new()
        .prefix("junto-release-")
        .tempdir()
        .unwrap();
    let source = root.path().join("source");
    std::fs::create_dir_all(source.join("Junto.app/Contents/MacOS")).unwrap();
    std::fs::write(source.join("Junto.app/Contents/MacOS/junto"), b"binary").unwrap();
    let mut bytes = Vec::new();
    pack(&source, &mut bytes).unwrap();
    let destination = root.path().join("destination");
    unpack(&mut bytes.as_slice(), &destination).unwrap();
    assert_eq!(
        std::fs::read(destination.join("Junto.app/Contents/MacOS/junto")).unwrap(),
        b"binary"
    );
    assert!(unpack(&mut bytes.as_slice(), &destination).is_err());
}

#[test]
fn archive_refuses_paths_outside_its_directory_and_truncation() {
    for name in [
        "../escape",
        "/escape",
        "Junto.app/../../escape",
        "C:/escape",
        "a\\escape",
        "a//escape",
        "a/./escape",
        "CON",
        "dir/AUX.txt",
        "a./escape",
    ] {
        let root = tempfile::Builder::new()
            .prefix("junto-release-")
            .tempdir()
            .unwrap();
        let mut bytes = b"JUNTO-ARCHIVE-1\n".to_vec();
        bytes.extend_from_slice(&(name.len() as u16).to_be_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&1u64.to_be_bytes());
        bytes.write_all(b"x").unwrap();
        bytes.extend_from_slice(&0u16.to_be_bytes());
        assert!(
            unpack(&mut bytes.as_slice(), &root.path().join("dest")).is_err(),
            "{name}"
        );
        assert!(!root.path().join("escape").exists());
    }
    let root = tempfile::Builder::new()
        .prefix("junto-release-")
        .tempdir()
        .unwrap();
    assert!(
        unpack(
            &mut &b"JUNTO-ARCHIVE-1\n\0\x05ab"[..],
            &root.path().join("dest")
        )
        .is_err()
    );
}

#[cfg(unix)]
#[test]
fn archive_refuses_symbolic_links() {
    let root = tempfile::Builder::new()
        .prefix("junto-release-")
        .tempdir()
        .unwrap();
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::os::unix::fs::symlink("/etc/passwd", source.join("link")).unwrap();
    assert!(pack(&source, &mut Vec::new()).is_err());
}

#[test]
fn archive_refuses_duplicate_entries_special_modes_and_trailing_bytes() {
    for (duplicate, mode, trailing) in [(true, 0, false), (false, 2, false), (false, 0, true)] {
        let root = tempfile::Builder::new()
            .prefix("junto-release-")
            .tempdir()
            .unwrap();
        let mut bytes = b"JUNTO-ARCHIVE-1\n".to_vec();
        for _ in 0..if duplicate { 2 } else { 1 } {
            bytes.extend_from_slice(&1u16.to_be_bytes());
            bytes.push(b'a');
            bytes.push(mode);
            bytes.extend_from_slice(&1u64.to_be_bytes());
            bytes.push(b'x');
        }
        bytes.extend_from_slice(&0u16.to_be_bytes());
        if trailing {
            bytes.push(b'x');
        }
        assert!(unpack(&mut bytes.as_slice(), &root.path().join("dest")).is_err());
    }
}

#[test]
fn limits_case_duplicates_and_cleanup_are_enforced() {
    for case in ["files", "size", "case"] {
        let root = tempfile::tempdir().unwrap();
        let destination = root.path().join("out");
        let mut bytes = b"JUNTO-ARCHIVE-1\n".to_vec();
        for index in 0..if case == "files" { 10001 } else { 2 } {
            let name = if case == "case" {
                if index == 0 {
                    "Name".into()
                } else {
                    "name".into()
                }
            } else {
                format!("file{index}")
            };
            bytes.extend_from_slice(&(name.len() as u16).to_be_bytes());
            bytes.extend_from_slice(name.as_bytes());
            bytes.push(0);
            bytes.extend_from_slice(
                &(if case == "size" {
                    junto_client_core::releases::MAX_ARCHIVE + 1
                } else {
                    0
                })
                .to_be_bytes(),
            );
        }
        bytes.extend_from_slice(&0u16.to_be_bytes());
        assert!(
            unpack(&mut bytes.as_slice(), &destination).is_err(),
            "{case}"
        );
        assert!(!destination.exists(), "{case}");
    }
}
