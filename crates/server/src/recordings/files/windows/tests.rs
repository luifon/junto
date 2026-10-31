#![allow(clippy::unwrap_used, clippy::expect_used)]

use super::*;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!("junto-files-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        Self(root)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn place(parts: &[&str]) -> Place {
    Place(parts.iter().map(|part| (*part).to_owned()).collect())
}

#[test]
fn writes_finishes_lists_moves_and_removes_keep_their_file_contract() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    create_directory(root, &place(&["recording", "pieces"])).unwrap();
    let mut held = create_file(root, &place(&["recording", "first.partial"])).unwrap();
    held.write_all(b"first").unwrap();
    held.sync_all().unwrap();
    finish(
        root,
        &place(&["recording", "first.partial"]),
        &place(&["recording", "video.mp4"]),
    )
    .unwrap();
    drop(held);
    write_new(root, &place(&["recording", "second.partial"]), b"second").unwrap();
    finish(
        root,
        &place(&["recording", "second.partial"]),
        &place(&["recording", "video.mp4"]),
    )
    .unwrap();
    assert_eq!(
        fs::read(root.join("recording/video.mp4")).unwrap(),
        b"first"
    );
    assert!(
        listed(root, &place(&["recording"]), |name| name
            .ends_with(".partial"))
        .unwrap()
        .is_empty()
    );
    assert!(write_new(root, &place(&["recording", "video.mp4"]), b"replace").is_err());
    for name in ["part_10.ts", "part_2.ts", "other.ts"] {
        write_new(root, &place(&["recording", "pieces", name]), b"piece").unwrap();
    }
    assert_eq!(
        pieces(root, &place(&["recording", "pieces"])).unwrap(),
        ["part_2.ts", "part_10.ts"]
    );
    rename(
        root,
        &place(&["recording", "video.mp4"]),
        &place(&["recording", "saved.mp4"]),
    )
    .unwrap();
    assert!(open_file(root, &place(&["recording", "saved.mp4"])).is_ok());
    assert!(open_file(root, &place(&["recording", "pieces"])).is_err());
    remove_if_empty(root, &place(&["recording"])).unwrap();
    assert!(root.join("recording").is_dir());
    let entries = entries_after(root, &place(&[]), None, 1, |_| true).unwrap();
    assert_eq!(
        entries
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["recording"]
    );
    remove(root, &place(&["recording"])).unwrap();
    assert!(!root.join("recording").exists());
}

#[test]
fn a_directory_cannot_be_replaced_while_its_path_is_in_use() {
    let scratch = Scratch::new();
    fs::create_dir(scratch.0.join("held")).unwrap();
    let held = directory(&scratch.0, &["held".to_owned()]).unwrap();
    assert!(fs::rename(scratch.0.join("held"), scratch.0.join("moved")).is_err());
    drop(held);
    fs::rename(scratch.0.join("held"), scratch.0.join("moved")).unwrap();
}

#[test]
fn path_components_refuse_windows_devices_and_path_escapes() {
    for name in [
        "",
        ".",
        "..",
        "../outside",
        "a\\outside",
        "C:outside",
        "file:stream",
        "name.",
        "name ",
        "NUL",
        "con.txt",
        "COM1",
        "lpt9.txt",
    ] {
        assert!(component(name).is_err(), "{name:?}");
    }
    for name in [
        "recording",
        "file.json",
        "part_0001.ts",
        "a.partial",
        "1-abc",
    ] {
        assert!(component(name).is_ok(), "{name:?}");
    }
}

#[tokio::test]
async fn a_junction_cannot_be_followed_for_reads_writes_or_removal() {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let scratch = Scratch::new();
    let root = scratch.0.join("root");
    let outside = scratch.0.join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("kept"), b"outside").unwrap();
    let mut command = tokio::process::Command::new("cmd.exe");
    command.args(["/D", "/V:OFF", "/C"]);
    command
        .as_std_mut()
        .raw_arg("mklink /J \"%JUNTO_FILES_LINK%\" \"%JUNTO_FILES_TARGET%\"")
        .creation_flags(CREATE_NO_WINDOW);
    command
        .env("JUNTO_FILES_LINK", root.join("linked"))
        .env("JUNTO_FILES_TARGET", &outside)
        .kill_on_drop(true)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut child = command.spawn().unwrap();
    let status = match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
        Ok(status) => status.unwrap(),
        Err(_) => {
            let _ = child.start_kill();
            tokio::time::timeout(Duration::from_secs(10), child.wait())
                .await
                .unwrap()
                .unwrap();
            panic!("the junction fixture did not finish");
        }
    };
    assert!(status.success(), "the junction fixture failed: {status}");
    assert!(open_file(&root, &place(&["linked", "kept"])).is_err());
    assert!(create_directory(&root, &place(&["linked", "new-directory"])).is_err());
    assert!(create_file(&root, &place(&["linked", "new-file"])).is_err());
    assert!(rename(&root, &place(&["linked", "kept"]), &place(&["taken"])).is_err());
    remove(&root, &place(&["linked"])).unwrap();
    assert_eq!(fs::read(outside.join("kept")).unwrap(), b"outside");
    assert!(!outside.join("new-directory").exists());
    assert!(!outside.join("new-file").exists());
}

#[test]
fn every_operation_refuses_escaping_components() {
    let scratch = Scratch::new();
    let root = scratch.0.join("root");
    fs::create_dir(&root).unwrap();
    fs::write(scratch.0.join("kept"), b"outside").unwrap();
    let safe = place(&["safe"]);
    write_new(&root, &safe, b"inside").unwrap();
    for parts in [vec!["..", "kept"], vec!["../kept"], vec!["NUL"]] {
        let invalid = place(&parts);
        assert!(open_file(&root, &invalid).is_err());
        assert!(create_file(&root, &invalid).is_err());
        assert!(write_new(&root, &invalid, b"replace").is_err());
        assert!(create_directory(&root, &invalid).is_err());
        assert!(remove(&root, &invalid).is_err());
        assert!(remove_if_empty(&root, &invalid).is_err());
        assert!(rename(&root, &safe, &invalid).is_err());
        assert!(rename(&root, &invalid, &safe).is_err());
        assert!(finish(&root, &safe, &invalid).is_err());
        assert!(finish(&root, &invalid, &safe).is_err());
        assert!(listed(&root, &invalid, |_| true).is_err());
        assert!(pieces(&root, &invalid).is_err());
        assert!(entries_after(&root, &invalid, None, 1, |_| true).is_err());
    }
    assert_eq!(fs::read(scratch.0.join("kept")).unwrap(), b"outside");
    assert_eq!(fs::read(root.join("safe")).unwrap(), b"inside");
}

#[test]
fn cursor_entries_keep_only_the_first_matching_names_after_the_cursor() {
    let scratch = Scratch::new();
    for name in ["d", "b", "a", "c", "ignored.partial"] {
        write_new(&scratch.0, &place(&[name]), b"entry").unwrap();
    }
    let selected = entries_after(&scratch.0, &place(&[]), Some("a"), 2, |name| {
        !name.contains('.')
    })
    .unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        ["b", "c"]
    );
    assert!(
        entries_after(&scratch.0, &place(&[]), None, 0, |_| true)
            .unwrap()
            .is_empty()
    );
}
